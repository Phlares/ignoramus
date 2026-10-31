# 0010 Level 2: one virtual machine per row

Level 2 runs each row in a fresh virtual machine. The crate `ignorarium-guest` holds both sides: the host code that starts the machine, and the guest agent that runs the payload inside it. The crate knows nothing of the box; the box's level 2 backend wraps `host::run_row` (0015).

Decision:

- Hypervisor: Cloud Hypervisor v54.0, the static release binary. The guest kernel is `vmlinux-x86_64` from the cloud-hypervisor/linux release `ch-release-v7.2.8-20261009`. Both are pinned by SHA-256 in `image.rs`. `guest-image` checks the kernel pin; the level 2 backend checks both pins when it probes the machine. The host needs `/dev/kvm`.
- Guest image: three parts, each hashed.
  - The kernel, as pinned.
  - An initramfs that holds only the agent, as `/init`, with `/dev/console` and `/dev/null`. The crate writes it in the kernel's `newc` format, with fixed times and owners, so the same agent gives the same bytes.
  - The environment rootfs as a read-only ext4 disk, built without root by `mkfs.ext4 -d` from the exported environment image. The environment stays exactly the environment; the agent is not added to it.
- Disks: the rootfs, the inputs and the payload's folder are read-only ext4 disks that the host builds. Each disk's root holds exactly its folder, with no `lost+found`. The output disk is a raw, zero-filled file.
- Outputs: the payload writes to `/out`, a tmpfs capped at half the output disk's size (see the view below). After the payload, the agent writes `/out` to the raw disk as a pack: a simple stream of directory, file, link and other entries. The host reads it with `pack::unpack`, which refuses `..`, absolute paths, NUL bytes, duplicates, children of anything but an earlier directory entry, over-long names, a path of 4096 bytes or more, and a total over the limit. It makes each entry relative to its folder, so the folder's own path does not count against `PATH_MAX`. A link may hold any target but an empty one, one with a NUL, or one of 4096 bytes or more. `unpack` records each link's path and target and never makes the link, so nothing on the host can follow it. Each entry costs one 4 KiB block against the limit, so the limit also bounds the entry count. The guest's `pack::write` charges the same limit (the output disk size) by the same rule and refuses the same link targets, and a tree whose paths pass `PATH_MAX` (a payload can make one with calls relative to a folder); the agent reports that refusal as the payload's result (`pack_refused`), with no outputs. Both sides hash the pack stream as it passes, with the rows of levels 0 and 1, and the host compares its hash with the one the guest reported.
- Control channel: one vsock connection, guest to host, on port 7201. Cloud Hypervisor delivers it to a Unix socket on the host. The host accepts one connection, then removes the listening socket, so no second host path exists. Frames are a 4-byte length and JSON, at most 1 MiB. Every read and write on the host side has one absolute deadline: boot, then the row's wall time plus slack.
- No network device. No API socket. The serial console is off unless a log file is asked for.
- Agent sequence, as PID 1: mount `/proc`, `/sys`, `/dev`; turn io_uring off (`io_uring_disabled = 2`); find the four disks by their serial numbers; mount the rootfs; build the payload's view; connect; read the job; write the host seed to `/dev/urandom` and ask the kernel to reseed (`RNDRESEEDCRNG`); set the clock from the host's time; run the self-test; run the payload only if the self-test passes; kill every other process and reap it; pack `/out`, hashing the stream as it goes; report; power off. Any error after connecting is reported as `Failed`. Every path ends in power off.
- Self-test, before the payload: the only network interface is `lo`; a UDP connect to a documentation address finds no route, for IPv4 and IPv6 (a route lookup, which sends nothing); a second connect to the host's control port fails; `io_uring_setup` fails.
- The payload's view (level 1 should match it):
  - `usr bin lib lib64 etc` from the environment: directories bound read-only, nosuid, nodev; symbolic links copied as links; missing names left out;
  - `/in`: the input disk, read-only, nosuid, nodev;
  - `/payload`: the payload disk, read-only, nosuid, nodev;
  - `/out`: tmpfs, nosuid, nodev, owned by the payload's user, with `size` half the output disk and `nr_inodes` that half divided by the most one pack entry costs besides file data, so a full `/out` always fits the pack;
  - `/proc` with `hidepid=2`; `/dev`: tmpfs with `null zero full random urandom tty`; `/tmp`: tmpfs, mode 1777, capped at the output disk size;
  - the root is a tmpfs, read-only once built.
- The payload runs as uid and gid 65534 with no supplementary groups and with `no_new_privs` set, in a clean environment (the job's variables plus `IGNORARIUM_SEED`), with null standard streams, in its own process group, starting in `/out`, under the row's wall timeout.

Measured (31 fresh machines per series, first dropped; 1 vCPU, 1 GiB guest memory; nested virtualization on a 6-vCPU cloud host; ± is the 95% Student t half-width of the mean):

| Row | Boot (spawn to first message) | Whole row | Row minus the same payload outside the VM |
|---|---|---|---|
| `/bin/true` | 260 ± 3 ms (median 258) | 307 ± 4 ms | 306 ± 4 ms |
| numpy 1500×1500 matmul ×5, one BLAS thread | 262 ± 5 ms (median 261) | 1623 ± 14 ms | 465 ± 25 ms |

The numpy payload itself took 1.14 times as long inside the machine (1323 ms against 1158 ms in a bubblewrap sandbox on the host). That figure includes page-in from the disk image, not only CPU speed. The host could not pin its CPU frequency.

Limits:

- The host refuses an input or payload file or directory that an unprivileged user cannot read, because the payload does not run as the user who owns the files.
- `/out` (up to half the output disk size) and `/tmp` (up to all of it) live in guest memory, so the host refuses an output disk of zero bytes or of more than a quarter of the guest memory.
- `/dev` has no `shm`, so programs that need POSIX shared memory fail.
- Still to do: storing the three image parts in the content store, and caching input disks by tree hash (0015).

Why:

- A virtual machine does not share the host kernel with the payload, which a container does.
- The host never parses a file system the guest wrote. A guest that controls its own kernel can craft any bytes on its disks; a small parser that only creates children of its own directory entries cannot be steered outside the output directory.
- The guest connects once and the host then closes the door, so the payload cannot open a second channel. The payload also runs without privileges, so it cannot reach the agent's channel or disks through `/proc`.
- A fresh boot per row gives each row a clean state. Snapshots and forks are later work.

Sources:

- https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/vsock.md
- https://github.com/firecracker-microvm/firecracker/blob/main/docs/vsock.md
- https://man7.org/linux/man-pages/man4/random.4.html
- https://docs.kernel.org/filesystems/ramfs-rootfs-initramfs.html
- https://docs.kernel.org/driver-api/early-userspace/buffer-format.html
- https://man7.org/linux/man-pages/man8/mke2fs.8.html
- Measurement method: Georges, Buytaert, Eeckhout, "Statistically rigorous Java performance evaluation", OOPSLA 2007, https://dri.es/files/oopsla07-georges.pdf; Laaber, Scheuner, Leitner, "Software microbenchmarking in the cloud. How bad is it really?", 2019, https://research.chalmers.se/publication/511491/file/511491_Fulltext.pdf; Kalibera, Jones, "Rigorous benchmarking in reasonable time", ISMM 2013, https://kar.kent.ac.uk/33611/45/p63-kaliber.pdf
