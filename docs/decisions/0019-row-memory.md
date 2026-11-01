# 0019 Row memory

A plan can declare the most memory one payload process may use. The box enforces that one size at every level, so a row's result does not depend on the level it ran at.

Decision:

- **The field.** `design.row_memory_mib`, an optional whole number of MiB from 1 to 1,048,576 (E-TYP-001, E-RNG-001). It sits beside `design.row_seconds`, the plan's other per-row figure: both describe one row, and both enter the spec hash that a person approves. `scope.budget` holds study totals, so it is the wrong table.
- **Absent.** A plan without it keeps the old behaviour: no cap at levels 0 and 1, and a 1024 MiB machine at level 2.
- **The cap at every level.** The payload process runs under `RLIMIT_DATA` of that size, as its soft and its hard limit. One function sets it in the child, between fork and exec, so the box itself is never capped:
  - level 0 sets it on the payload;
  - level 1 sets it on bubblewrap, which passes it to the payload; bubblewrap starts within 256 KiB, below the 1 MiB floor;
  - level 2 sends it in the job, and the guest agent sets it on the payload after it drops to the payload's user.

  Each process the payload starts inherits the limit, and the payload cannot raise a hard limit.
- **The echo.** The agent reports the limit the payload ran under. The host refuses a report whose limit differs from the job's, as an infrastructure fault: `the agent reported the data limit none, not the job's <n> bytes: rebuild the agent`. So an agent built before the field can never run a capped payload uncapped.
- **Past the cap.** An allocation past the cap fails with `ENOMEM`, and the row fails as the payload's own result, never a retried infrastructure fault. Shapes measured with numpy 2.5.4 and its OpenBLAS 0.3.34:
  - an array past the cap raises `MemoryError`, which the payload can catch; uncaught, it exits 1;
  - a BLAS call whose work buffer cannot be had prints `OpenBLAS error: Memory allocation still failed after 10 retries, giving up.` and exits 1, with no exception to catch;
  - an OpenBLAS thread that cannot start makes OpenBLAS raise SIGINT itself: exit 130;
  - a shell (dash) dies by SIGSEGV: exit 139.
- **The level 2 machine.** For a row with a cap, the machine's memory is ceil((cap + `/out` tmpfs + `/tmp` tmpfs + 320 MiB) × 32 / 31), summed in bytes and rounded up to a whole MiB. `/out` takes half the output disk and `/tmp` all of it (0015); tmpfs pages live in guest memory, outside the payload's limit. The 320 MiB holds the guest kernel, the agent, the page cache and the agent's 128 MiB floor (below). The 32/31 holds what grows with the machine: the kernel's page data (64 bytes per 4 KiB page, 1/64), zone reserves and page tables, about 1/32 as measured. With the 256 MiB output disk, a 2048 MiB cap gives 2841 MiB. The host checks every machine against this sum, with a cap of 0 for a row without one, so a hand-set machine smaller than it is refused before boot.
- **No bounce buffer.** The guest kernel boots with `swiotlb=noforce`. Virtio devices without an IOMMU need no bounce buffer, which took 64 MiB of a guest past 4 GiB.
- **The boot check.** Before the payload, the agent reads `MemAvailable` and refuses the row when it is under the cap plus both tmpfs plus 128 MiB. The refusal is an infrastructure fault that names the numbers. So a size model that falls short shows as a named fault, never as an out-of-memory kill.
- **The host bound.** A machine larger than the host's `MemTotal`, or a host whose memory size cannot be read, makes level 2 unavailable for the study, as an image that cannot become a disk does (0015). `prepare` runs a lower level with a `run.isolation` event, or refuses at `min_isolation = 2`: `a level 2 machine for rows of <cap> MiB needs <n> MiB, more than the host's MemTotal of <m> MiB`.

Measured (one cloud host with nested virtualization; the pinned kernel; the debug agent the tests boot; a 256 MiB output disk; the payload at its cap less 8 MiB with both tmpfs full for "at full load"):

| Machine | Cap | Free at boot beyond the cap and both tmpfs | Available at full load |
|---|---|---|---|
| 991 MiB | 256 MiB | 171 MiB | 183 MiB |
| 1124 MiB | 384 MiB | 176 MiB | 197 MiB |
| 2841 MiB | 2048 MiB | 180 MiB | 194 MiB |
| 3106 MiB | 2304 MiB | 153 MiB | 169 MiB |
| 7069 MiB | 6144 MiB | 188 MiB | 189 MiB |

Every size clears the 128 MiB floor, by 25 to 60 MiB. The least room is just past 3 GiB, where Cloud Hypervisor starts to place memory above 4 GiB and the kernel keeps a reserve for it.

What counts:

- Since Linux 4.7, `RLIMIT_DATA` counts every private writable mapping that is not a stack: the heap, anonymous `mmap` (where numpy arrays live), thread stacks, which glibc maps private and writable, and copy-on-write file maps. It leaves out read-only and shared file maps, the main thread's stack and `PROT_NONE` reservations. The counter is `VmData` in `/proc/<pid>/status`.
- So a model file that a payload maps read-only, as safetensors does, does not count; the tensors it copies out do. The cap counts mapped size, not resident memory: size it from the peak `VmData`, not from RSS.
- OpenBLAS starts one thread per visible CPU, about 40 MiB of data each (its buffer and the thread's stack), unless `OPENBLAS_NUM_THREADS=1`. At levels 0 and 1 a payload sees the host's CPUs, and a 1-vCPU level 2 machine shows one. Measured on a 6-CPU host: `import numpy` needs about 48 MiB with one thread and about 248 MiB with six. A payload that uses OpenBLAS sets the thread count, or one cap passes at level 2 and fails at level 1.
- glibc sizes each new thread's stack from the inherited `RLIMIT_STACK`. At levels 0 and 1 that is the limit the box was started with, from its shell. At level 2 it is the kernel's default for the agent, 8 MiB. A box started under another stack limit gives a threaded payload another need at levels 0 and 1.

Why `RLIMIT_DATA`, not `RLIMIT_AS`: `RLIMIT_AS` counts all address space, a read-only model map, glibc's 64 MiB reservation per malloc arena and stack guard space included. Under it the same row would need another cap after a library update.

Limits:

- The cap is per process, and a level 2 machine holds one cap. A payload that starts several large processes can run out of memory there and not at levels 0 and 1.
- The constants come from one host and the pinned kernel. A guest kernel or agent that grows by more than 25 MiB turns rows near 3 GiB into a named boot refusal.
- Only a test run by hand checks the large sizes. It needs a host that can back a machine past 4 GiB.
- `MemTotal` is a static bound, checked once at prepare. Memory that other processes hold when a row starts is not checked: a machine that cannot start is an infrastructure fault, retried.

Case against: memory a payload reserves writable and never touches counts against it.

Sources:

- https://man7.org/linux/man-pages/man2/setrlimit.2.html (`RLIMIT_DATA`, `RLIMIT_AS`, `RLIMIT_STACK`)
- https://man7.org/linux/man-pages/man5/proc_pid_status.5.html (`VmData`)
- https://docs.kernel.org/filesystems/tmpfs.html (tmpfs pages live in memory, `size`)
