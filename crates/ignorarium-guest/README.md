# ignorarium-guest

Runs one experiment row inside a fresh virtual machine (isolation level 2). The crate holds both sides of the machine boundary and knows nothing of the box that calls it. Design and measured costs: `docs/decisions/0010-level-2-guest.md`.

| Module | Side | Job |
|---|---|---|
| `proto` | both | Control frames (4-byte length + JSON, at most 1 MiB) and the messages. |
| `tree` | both | The one directory walk, and the output hash, the same rule as level 0. |
| `pack` | both | The output stream: `write` in the guest, strict `unpack` on the host. |
| `image` | host | Pinned kernel and hypervisor, the initramfs with the agent, ext4 disks. |
| `host` | host | `run_row`: build the disks, start Cloud Hypervisor, serve the one connection, read the outputs back. |
| `agent` | guest | PID 1: disks, the payload's view, seed, clock, self-test, payload, pack, report. |
| `sys` | guest | The only `unsafe` code: vsock, the kernel reseed, child CPU time, killing every process. |

The binary `ignorarium-guest` is the agent. It runs only as PID 1 inside the machine.

## Host requirements

- Linux with `/dev/kvm`, and the user in the `kvm` group.
- `mkfs.ext4` and `debugfs` (e2fsprogs) on `PATH`.
- Cloud Hypervisor and the guest kernel, pinned in `src/image.rs`:

```sh
curl -fLo cloud-hypervisor https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v54.0/cloud-hypervisor-static
curl -fLo vmlinux-x86_64 https://github.com/cloud-hypervisor/linux/releases/download/ch-release-v7.2.8-20261009/vmlinux-x86_64
printf '%s  %s\n' \
  4f4f98a3d661a40f3e5272627cc895a1b2e525f2f6bedf5f895f9cd21ba5c65b cloud-hypervisor \
  edfeb8a8a6c04ff2979e9f1c700feaa29b8f564f952261834ef0b7d2e673a853 vmlinux-x86_64 \
  | sha256sum -c
chmod +x cloud-hypervisor
```

## Guest image

The agent must be a static binary, because the initramfs holds nothing else:

```sh
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo build --release --target x86_64-unknown-linux-gnu -p ignorarium-guest
```

The environment rootfs is the exported environment image, unchanged:

```sh
cid=$(docker create <image>@sha256:<digest>)
mkdir rootfs && docker export "$cid" | tar -x -C rootfs --exclude='dev/*'
docker rm "$cid"
```

Then build the initramfs and the rootfs disk, and check the kernel pin:

```sh
cargo run -p ignorarium-guest --example guest-image -- \
  --agent target/x86_64-unknown-linux-gnu/release/ignorarium-guest \
  --rootfs-dir rootfs --kernel vmlinux-x86_64 --out image
```

`image/image.json` lists the SHA-256 of the kernel, the agent, the initramfs and the rootfs disk.

## Tests

`cargo test -p ignorarium-guest` runs everything that needs no machine. The VM tests are ignored by default; on a KVM host:

```sh
IGNORARIUM_VM_VMM=$PWD/cloud-hypervisor \
IGNORARIUM_VM_KERNEL=$PWD/vmlinux-x86_64 \
IGNORARIUM_VM_ROOTFS=$PWD/image/rootfs.ext4 \
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo test --target x86_64-unknown-linux-gnu -p ignorarium-guest -- --include-ignored
```

The VM tests expect a Debian-based rootfs with `/bin/sh`, coreutils and `python3` with numpy.

## Overhead

`examples/overhead.rs` boots one machine per run, alternates with a baseline command outside the machine, drops the first run, and prints each run and a summary (mean with a 95% Student t interval, median, min, max) as JSON lines:

```sh
cargo run --release --target x86_64-unknown-linux-gnu -p ignorarium-guest --example overhead -- \
  --baseline /bin/true -- -- /bin/true
```
