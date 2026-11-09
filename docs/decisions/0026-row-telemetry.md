# 0026 Row telemetry

Each row record holds a `telemetry` object of raw readings for later study: the host CPU, the host's CPU tick counts around the row, and the row cgroup's peak memory, memory events and CPU time. Nothing reads it: no output hash, recheck comparison, charge, budget, schedule, pause or verdict.

Decision:

- **Observation only.** A reading that cannot be had is `{"unavailable": "<why>"}`. It is never an error, a retry, a pause or a changed exit. `cpu_seconds` stays the `wait4` count, even where `cpu.stat` says more.
- **Host CPU.** `host_cpu` holds the first `model name` of `/proc/cpuinfo` and the SHA-256 of its first `flags` value, read once per process. aarch64 names neither, so it reads unavailable.
- **CPU ticks.** `cpu_ticks` holds `user_hz` and the aggregate `cpu` line of `/proc/stat` before and after the row, raw, in kernel order: user nice system idle iowait irq softirq steal guest guest_nice. The counts cover the whole host, so concurrent runs share them. `steal` is the time the hypervisor gave to other guests.
- **One delegated scope per run.** The operator starts the box alone in a delegated cgroup C: `systemd-run --user --scope -p Delegate=yes -q -- ignorarium run <id>`. `systemd-run --scope` execs its command, so the box keeps its process id. At run start the box reads C from `/proc/self/cgroup` and records a reason instead when:
  - C is the root cgroup;
  - C's `cgroup.procs` or `cgroup.subtree_control` is not the box's user's to write;
  - C lacks the `memory` controller.

  Otherwise the box makes `C/ignorarium-box`, moves itself there, and writes `+memory` to C's `cgroup.subtree_control`. The kernel enables a controller for C's children only while C holds no process. When another process is in C, the reason is `the box's cgroup holds other processes; start the box alone in its scope`. The `run.start` and `run.resume` events carry `row_cgroup`: `available`, or the reason.
- **A leaf per row.** Levels 0 and 1 make a leaf `C/row-<pid>-<n>` for each row. The row's first process writes itself into the leaf's `cgroup.procs` between fork and exec, so every process of the row starts there.
  - Level 0's leaf holds the payload.
  - Level 1's leaf holds bubblewrap, the sandbox's PID 1 and the payload. bubblewrap's `--unshare-cgroup` roots the sandbox's cgroup namespace at the leaf. The self-test sandbox runs outside the leaf.

  Once the row is reaped, the box reads the leaf and removes it.
- **The move cannot fail a row.** The hook ignores every error, so the process then runs in the box's own cgroup. A leaf whose `memory.peak` and `usage_usec` both read 0 never held the row, and the reading is `the row did not enter its cgroup`. A leaf the box cannot remove (a process is still in it) stays, and `kept` gives the reason.
- **What is kept.** `cgroup` holds `memory_peak` in bytes, every line of `memory.events`, and `usage_usec`, `user_usec` and `system_usec` from `cpu.stat`.
  - The box enables `memory` only, and sets no limit with it. It never enables `cpu`, `io` or `pids`. With `cpu`, the kernel shares CPU between the box and the rows by cgroup weight, which is a scheduling change.
  - `cpu.stat` times need no controller. Throttling counts need `cpu`, and nothing throttles without `cpu.max`, so none are kept.
  - No `io.stat`: a systemd user manager need not delegate `io`, so it would often read unavailable.
- **Level 2.** `cgroup` reads `level 2 rows run in a machine`. A cgroup around the VMM would count guest memory the machine touched, not payload memory.

What `memory.peak` measures: the most memory charged to the leaf since the box made it. That is anonymous memory, the page cache of files the row reads and writes, shared memory, and kernel memory such as page tables. It sizes host memory and the rows a host can run at once. It does not size `design.row_memory_mib`, which caps `VmData` (0019). The cap stays `RLIMIT_DATA`.

Limits:

- `memory.peak` needs Linux 5.19 or later.
- A file page is charged to the cgroup that first read it. A row that reads a file already in the page cache adds nothing for it; the first row to read it is charged for all of it. So the peak depends on what ran before.
- Memory charged before the move, the forked copy of the box, stays with the box's cgroup.
- At level 1, `cpu_seconds` includes the self-test sandbox and the leaf's `cpu.stat` does not, so the two differ by the self-test.
- A run started outside a delegated scope, from a plain shell or tmux, records every row as unavailable. Check `row_cgroup` in the first event before leaving a run.
- A scope under `user@<uid>.service` ends when the user manager stops, which happens when the user's last session ends unless linger is on. Turn linger on before a long run.
- A box that dies mid-row leaves its leaf behind. The next run ignores it.
- `row_cgroup: "available"` on `run.start` means the parent cgroup is set up. It does not mean every row entered its leaf. A row that did not enter records `the row did not enter its cgroup`.
- An EBUSY refusal happens after the box moved itself into `ignorarium-box`. The box stays there for the rest of the run.
- A removed leaf can stay in the kernel as a dying memory cgroup until its page cache is reclaimed. Across thousands of rows this can add up to hundreds of MB of reclaimable memory. The operator watches `nr_dying_descendants` in the scope's `cgroup.stat`.
- Level 0 has no cgroup namespace, so a payload sees its per-row path in `/proc/self/cgroup`. A payload that prints it changes its output hash.
- Level 2 rows run in a machine and record the cgroup as unavailable. Per-row cgroup readings cover levels 0 and 1 only.
- The ownership check does not prove `Delegate=yes`. The operator starts the run with `systemd-run --user --scope -p Delegate=yes`.

Case against: a scope ties the run's life to the systemd user manager, which a run from a plain shell does not depend on.

Sources:

- https://docs.kernel.org/admin-guide/cgroup-v2.html (`memory.peak`, `memory.events`, `cpu.stat`, moving a process through `cgroup.procs`, the no-internal-process rule, delegation)
- https://systemd.io/CGROUP_DELEGATION/ (`Delegate=yes`, scopes)
- https://man7.org/linux/man-pages/man7/cgroup_namespaces.7.html (a new cgroup namespace is rooted at the creator's cgroup)
- https://man7.org/linux/man-pages/man5/proc_stat.5.html (the `cpu` line of `/proc/stat`)
