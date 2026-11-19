# 0006 Runner and budget

Date: 2026-10-29.

Amended: 2026-10-30. Level 1 joins as a backend and ends a row's detached processes; a pause writes its state before its event (#25).

Amended: 2026-10-31. Rows run the registered payload copy and read inputs through the input view; a CPU bound for rows the box ended; the payload hash covers the whole folder (#31).

Amended: 2026-11-01. The estimate charges each row a wall time equal to its CPU time (#34).

`run` executes a registered plan row by row and meters cost against the declared budget. Pausing is the safe default.

Decision:

- A backend runs one row and returns an outcome. The run engine owns order, records and resume. Level 0 runs a bare child process with no network or filesystem isolation. Level 1 runs it in a bubblewrap sandbox (0008). `prepare` picks the highest level the machine reaches that meets `min_isolation`; `run --max-level 0` forces level 0.
- Level 0 has no PID namespace. If the box dies, a payload process group can outlive it. A later `stop` kills that group only while its leader still lives. A payload that detaches from its group is out of reach at this level.
- Level 1 closes that gap (0008). Each row has its own PID namespace, which the kernel empties when its PID 1 dies, a detached process included. PID 1 dies with bubblewrap, so a stop, a timeout or the death of the box ends the whole sandbox.
- A row is done when its signed record exists. A crash before the record reruns the row. A voided row has a record, so it counts as done on resume and does not run again.
- Spend goes into signed records: one per row, and one per attempt that has no row record, such as a retried attempt or a row ended early. A resume sums them, so no spend counts twice, and a crash loses at most the in-flight row's spend.
- An infrastructure fault, where the payload could not run or be reaped, retries the row at most three times. Each retry first passes the health check, the budget, the re-hash and the signal check. A payload result is void and never retried; an output the payload left unhashable is such a result.
- Rows run the payload copy that `register` took and see the inputs through the study's input view: one hard link per declared input to its stored blob, nothing else. `prepare`, once per `run` or `resume`, hashes the payload copy and every input in the view in full; a stored input that is gone or no longer hashes to its registered digest refuses the run. Before each attempt the box hashes the payload copy again and compares each view file's device, inode, size and modification time, and the view's list of entries, with what `prepare` saw. A file whose change time alone moved (a chmod, another study's link to the same blob, or a write whose old modification time was put back) is hashed in full and counts only when its bytes differ; no unprivileged writer can set the change time back. When anything changed, the run pauses, so a row record never names a payload or input that changed before the row started. A full re-hash per attempt would read a large model again before every row. File times tick every few milliseconds: a write in the same tick as `prepare`'s scan that keeps the size is caught by the next `prepare`, not by the next attempt.
- Fail-safe pause: when any budget dimension reaches its cap, or the next row's estimate would reach it, the run pauses at the row boundary. The row that would cross never starts. A meter that cannot read a cost, or any failed internal check, pauses the run too. A pause writes the paused state before its `run.pause` event. An event that cannot be written during a row, a budget alert at a retry included, starts no new attempt and pauses the run after the row; the attempt's record lands first. When the event log missed the pause or the row, the pause also stays a pending request: the next run pauses at its first boundary, and `ignorarium run` refuses until `ignorarium resume`. The run reports the failure.
- CPU bound (amended). A row the box ended (a stop, a pause, a signal or a wall timeout) may have used more CPU than was measured, and so may a row whose CPU time could not be read. Such a row is charged max(measured CPU, wall seconds × the CPUs the backend lets a row use). A row whose CPU time could not be read is also charged at least its row estimate. Its row or spend record carries `"cpu_bound": true`. At levels 0 and 1 a row may use the box process's available parallelism; a level 2 backend reports its machine's vCPUs. On a 16-thread host a row killed after 60 s costs 16 CPU-minutes, so the run pauses early, which is the safe side.
- Estimate (amended). Rows run one at a time, so the estimate charges each row a wall time equal to its CPU time, `design.row_seconds`: wall hours equal CPU hours, money is wall hours times the machine's rate, and a long job is one whose CPU time passes `long_job_minutes`. `machine.cpus` stays in `machine.toml` and leaves the estimate. The CPU bound above still charges a row the box ended at wall time times the backend's CPUs, the safe side.
- Payload folder (amended). The payload hash is the tree hash of the whole `payload/` folder, not of one entry file (0012). `run` refuses a payload copy whose hash differs from the register record, and hashes it again before each attempt (above).
- Alerts fire once each at 50%, 80% and 95% of every declared dimension. The fractions and the safety margin are constants in `budget.rs`.
- Wall time uses a monotonic clock, so a system clock change does not move the total.
- `CostMeter` is a trait. The host meter reads CPU from `wait4` and wall from a monotonic timer. A fake meter plays back planted readings and faults, so tests can force an overspend or a meter failure.
- `stop` always works. It sends its request before it reads the record log, kills the row in flight, writes one stop record and one deviation row, and lowers the earnable label to `signal`. A stop cannot be undone by `resume`.
- SIGINT, SIGTERM or SIGHUP ends the row in flight as a stop does: the box kills its process group, records its spend, and pauses the run. A second SIGINT ends the box the default way. A signal the box was started with ignored stays ignored. The command line installs these handlers; the library leaves signal handling to its caller.
- When SIGCHLD is ignored, the kernel reaps each row before the box can see it end, so the run pauses before the next attempt. The command line sets SIGCHLD back to its default first.
- A heartbeat event every 15 seconds shows the run is alive.

Why: a failure that spends money is worse than a run that stops early. Records, not memory, hold the state, so a crash costs at most one row of work.

Case against:

- A run that reaches its cap cannot go on: `resume` meets the same cap, and no command raises it. So an author sets a cap that never binds: the first confirmatory study set a CPU cap of 2,640 CPU hours and used 1.3% of it (#93).
- At level 2 the CPU charge counts the guest's work twice (0015, Case against). A level 2 CPU cap then trips at about half the CPU it names (#83).
- `check` admits a budget on the row estimate, and the charge can sit above it: in a capped rehearsal the charge ran 1.5 times that estimate (#83). The run takes the larger of the estimate and the mean charge so far (`runner.rs`), so the first row runs on the estimate alone. The safety margin of 1.0 adds no room for the gap (Constants). #83 proposes an estimate from the charge a probe row measured.
- The CPU bound charges a row the box ended for every CPU the backend allows (CPU bound). #26 named a per-row cgroup count as its replacement. Each level 1 row's cgroup now records that CPU time, but only as an observation (0008, Limits).

(written 2026-11-19)

What failure this prevents: a run that spends past its declared budget; a crash that loses a row's spend or counts it twice; and a row record that names a payload or an input that changed before the row started. A killed row charged a few CPU seconds was observed in a repro (#26). The rest is expected, not observed in a study. (written 2026-11-19)

Constants:

- infrastructure retries per row = 3 (`MAX_RETRIES`). Basis: none on file. Rule for change: none stated.
- alert fractions = 50%, 80% and 95% of each dimension (`ALERT_FRACTIONS`). Basis: none on file. Rule for change: none stated.
- safety margin = 1.0 (`SAFETY_MARGIN`): a row does not start when (spent + the next row's estimate) × the margin reaches a cap. Basis: none on file. Rule for change: none stated.
- heartbeat = every 15 s (`HEARTBEAT`). Basis: none on file. Rule for change: none stated.
- row wall timeout = 10 × `row_seconds`, at least 60 s (`TIMEOUT_FACTOR`, `MIN_TIMEOUT`). Basis: none on file. Rule for change: none stated.

(written 2026-11-19)

Sources: https://man7.org/linux/man-pages/man2/wait4.2.html, https://man7.org/linux/man-pages/man7/pid_namespaces.7.html and https://man7.org/linux/man-pages/man7/signal.7.html
