# 0006 Runner and budget

`run` executes a registered plan row by row and meters cost against the declared budget. Pausing is the safe default.

Decision:

- A backend runs one row in isolation and returns an outcome. The run engine owns order, records and resume. Level 0 is the only backend in this release: a bare child process with the network off in a private output folder.
- Level 0 has no PID namespace. If the box dies, a payload process group can outlive it. A later `stop` kills that group only while its leader still lives. A payload that detaches from its group is out of reach at this level.
- A row is done when its signed record exists. A crash before the record reruns the row. A voided row has a record, so it counts as done on resume and does not run again.
- Spend goes into signed records, one per row and one per row that ends early. A resume sums them, so spend is never lost or counted twice.
- Fail-safe pause: when any budget dimension reaches its cap, or the next row's estimate would reach it, the run pauses at the row boundary. The row that would cross never starts. A meter that cannot read a cost, or any failed internal check, pauses the run too.
- Alerts fire once each at 50%, 80% and 95% of every declared dimension. The fractions and the safety margin are constants in `budget.rs`.
- Wall time uses a monotonic clock, so a system clock change does not move the total.
- `CostMeter` is a trait. The host meter reads CPU from `wait4` and wall from a monotonic timer. A fake meter plays back planted readings and faults, so tests can force an overspend or a meter failure.
- `stop` always works. It kills the row in flight, writes one stop record and one deviation row, and lowers the earnable label to `signal`. A stop cannot be undone by `resume`.
- A heartbeat event every 15 seconds shows the run is alive.

Why: a failure that spends money is worse than a run that stops early. Records, not memory, hold the state, so a crash costs at most one row of work.

Sources: https://man7.org/linux/man-pages/man2/wait4.2.html and https://man7.org/linux/man-pages/man7/pid_namespaces.7.html
