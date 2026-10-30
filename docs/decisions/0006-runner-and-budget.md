# 0006 Runner and budget

`run` executes a registered plan row by row and meters cost against the declared budget. Pausing is the safe default.

Decision:

- A backend runs one row and returns an outcome. The run engine owns order, records and resume. Level 0 runs a bare child process with no network or filesystem isolation. Level 1 runs it in a bubblewrap sandbox (0008). `prepare` picks the highest level the machine reaches that meets `min_isolation`; `run --max-level 0` forces level 0.
- Level 0 has no PID namespace. If the box dies, a payload process group can outlive it. A later `stop` kills that group only while its leader still lives. A payload that detaches from its group is out of reach at this level.
- Level 1 closes that gap (0008). Each row has its own PID namespace, which the kernel empties when its PID 1 dies, a detached process included. PID 1 dies with bubblewrap, so a stop, a timeout or the death of the box ends the whole sandbox.
- A row is done when its signed record exists. A crash before the record reruns the row. A voided row has a record, so it counts as done on resume and does not run again.
- Spend goes into signed records: one per row, and one per attempt that has no row record, such as a retried attempt or a row ended early. A resume sums them, so no spend counts twice, and a crash loses at most the in-flight row's spend.
- An infrastructure fault, where the payload could not run or be reaped, retries the row at most three times. Each retry first passes the health check, the budget, the re-hash and the signal check. A payload result is void and never retried; an output the payload left unhashable is such a result.
- Before each attempt the box hashes the payload and the inputs again. When they differ from the hashes the run began with, the run pauses, so a row record never names a `payload/run` or declared input that changed before the row started.
- Fail-safe pause: when any budget dimension reaches its cap, or the next row's estimate would reach it, the run pauses at the row boundary. The row that would cross never starts. A meter that cannot read a cost, or any failed internal check, pauses the run too. A pause writes the paused state before its `run.pause` event. An event that cannot be written during a row, a budget alert at a retry included, starts no new attempt and pauses the run after the row; the attempt's record lands first. When the event log missed the pause or the row, the pause also stays a pending request: the next run pauses at its first boundary, and `ignorarium run` refuses until `ignorarium resume`. The run reports the failure.
- Alerts fire once each at 50%, 80% and 95% of every declared dimension. The fractions and the safety margin are constants in `budget.rs`.
- Wall time uses a monotonic clock, so a system clock change does not move the total.
- `CostMeter` is a trait. The host meter reads CPU from `wait4` and wall from a monotonic timer. A fake meter plays back planted readings and faults, so tests can force an overspend or a meter failure.
- `stop` always works. It sends its request before it reads the record log, kills the row in flight, writes one stop record and one deviation row, and lowers the earnable label to `signal`. A stop cannot be undone by `resume`.
- SIGINT, SIGTERM or SIGHUP ends the row in flight as a stop does: the box kills its process group, records its spend, and pauses the run. A second SIGINT ends the box the default way. A signal the box was started with ignored stays ignored. The command line installs these handlers; the library leaves signal handling to its caller.
- When SIGCHLD is ignored, the kernel reaps each row before the box can see it end, so the run pauses before the next attempt. The command line sets SIGCHLD back to its default first.
- A heartbeat event every 15 seconds shows the run is alive.

Why: a failure that spends money is worse than a run that stops early. Records, not memory, hold the state, so a crash costs at most one row of work.

Sources: https://man7.org/linux/man-pages/man2/wait4.2.html, https://man7.org/linux/man-pages/man7/pid_namespaces.7.html and https://man7.org/linux/man-pages/man7/signal.7.html
