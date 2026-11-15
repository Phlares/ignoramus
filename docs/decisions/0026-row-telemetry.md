# 0026 Row telemetry

Date: 2026-11-10.

Amended: 2026-11-15. `status --telemetry` reads the row telemetry (#102).

Amended: 2026-11-15. A level 2 row gets its own cgroup leaf (#116).

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
- **Level 2.** Level 2 makes a leaf per row as level 1 does. The host calls a hook on the VMM's command once, after it built the disks and just before the VMM starts; the hook makes the leaf, and the VMM enters it before exec. The leaf holds the VMM and every thread it starts. Its reading is the machine's total, not the payload's memory: guest RAM the machine touched, the VMM process, and page cache the VMM charged. The card names it `VM total (guest RAM touched + VMM), bytes`, and its CPU line `Cgroup counters, level 2 (VMM and guest)`. `cpu.stat` holds the VMM with guest vCPU time; `cpu_seconds` stays the guest's report and `box_cpu_seconds` the `wait4` count of the VMM. A row that fails or stops before the VMM starts makes no leaf and reads `the box made no cgroup for the row`.

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
- Level 2's page cache part is bounded. The box builds the input and payload disks with a `mkfs.ext4` child before the VMM starts, so those pages are charged to the box's cgroup. The rootfs disk is kept between rows, so only the first level 2 row after a host boot pays for the pages it reads. The leaf takes the VMM's writes to the output disk, at most its size (256 MiB by default). Under host memory pressure, input and payload pages can be evicted and re-read by the VMM, which charges them to the leaf.
- An out-of-memory kill inside the guest does not show in `memory.events`. The box sets no `memory.max`, so `oom_kill` counts only host kills of the VMM. A row past its cap fails inside the guest as a payload failure (0019).
- Cloud Hypervisor's disks use the host page cache by default (`direct=off`). This is not checked against its documentation; the bounds test checks its effect.
- Measured 2026-11-15 on the 12 GB test VPS, 1 vCPU, a 1024 MiB cap (a 1784 MiB machine): a row whose payload touches 512 MiB and writes 1 MiB peaks at 637 MiB, and the VMM's `VmHWM` reads 638 MiB, so the page cache part was under 1%. A row running `true` peaks at 127 MiB. The bounds test takes 160 MiB as the VMM's overhead beyond guest RAM and the output disk: the 125 MiB seen above the touched memory, rounded up to 128, plus 25%. Those 125 MiB include the guest kernel and agent, which also sit inside guest RAM, so the bound is loose.
- The ownership check does not prove `Delegate=yes`. The operator starts the run with `systemd-run --user --scope -p Delegate=yes`.

Case against: a scope ties the run's life to the systemd user manager, which a run from a plain shell does not depend on.

Sources:

- https://docs.kernel.org/admin-guide/cgroup-v2.html (`memory.peak`, `memory.events`, `cpu.stat`, moving a process through `cgroup.procs`, the no-internal-process rule, delegation)
- https://systemd.io/CGROUP_DELEGATION/ (`Delegate=yes`, scopes)
- https://man7.org/linux/man-pages/man7/cgroup_namespaces.7.html (a new cgroup namespace is rooted at the creator's cgroup)
- https://man7.org/linux/man-pages/man5/proc_stat.5.html (the `cpu` line of `/proc/stat`)
- FDA (2019), "Adaptive Designs for Clinical Trials of Drugs and Biologics", https://www.fda.gov/media/78495/download (section VII, limited access to comparative interim results)

## Reader

`ignorarium status <id> --telemetry` prints what the row records hold about the host. `--json` prints the same figures.

It prints these figures:

- The total row count.
- The cgroup states, with a row count for each. A row with no cgroup reading counts as `not recorded`.
- Each host CPU model and flags hash, with a row count.
- Rows below level 2 and level 2 rows print their cgroup states apart (`cgroup` and `cgroup, level 2`), because a level 2 leaf holds a machine, not the payload's processes. A study with no level 2 rows prints as before. The card sums cgroup counters and takes the peak per class the same way.
- Steal seconds per row and iowait seconds per row: largest and mean, each with its row count.
- The rows with no host CPU and the rows with no usable CPU ticks: each as a count of rows whose reading is a reason or unusable, and a count of rows that never recorded it.
- Wall seconds per row and CPU seconds per row for the whole study: largest and mean.
- Ten blocks of the schedule. Each block prints the rows recorded, the first and last row of the block in schedule order, and the steal share and iowait share: largest and mean.
- When the study is not blind, each block also prints wall seconds per row and CPU seconds per row: largest and mean.

Every count of rows adds up to the total. A row from before this record has no `telemetry` key and counts as not recorded. A tick reading with a zero rate or too few fields counts as unusable.

The share of steal or iowait is its tick delta over the deltas of user, nice, system, idle, iowait, irq, softirq and steal. The kernel counts guest time inside user and guest_nice inside nice, so those two stay out of the sum.

The two numbers of a block are the first and last row of the block in schedule order. Rows run by stage, cluster, then condition. The two numbers need not be consecutive, and the first can exceed the last. The block edges come from the full schedule, reserve and child rows included. A row that never runs has no record in its block. The edges do not move when a re-size or trigger record lands.

The command reads verified records. For each row record it copies the row index, the `telemetry` key, `isolation` and the wall and CPU seconds. It never copies `values`, `output_hash`, `outputs` or `data_hashes`. It works while a run holds the run lock. In `--json`, `cgroup` holds the states of rows below level 2 and `cgroup_machine`, present only when level 2 rows exist, those of level 2 rows. Each spread is `{rows, max, mean}`: `steal_s`, `iowait_s`, `wall_s` and `cpu_s` for the study, and `steal_share`, `iowait_share` and, when not blind, `wall_s` and `cpu_s` for each block.

A blind study prints no per-block wall or CPU figure. A row index joins to a condition through the schedule, and row time can track an outcome. A block of row times would let a blind reader infer the condition. This follows the rule that comparative interim results stay limited to people independent of the study's conduct (FDA 2019, section VII). Steal and iowait shares come from host counters, not from a row's result. They stay.

The whole-study wall and CPU figures still print in a blind study. They join to no condition, so no condition's time can be read off them.

No decision reads this output. Nothing in `output_hash`, `compare`, charge, schedule order or budgets uses it.
