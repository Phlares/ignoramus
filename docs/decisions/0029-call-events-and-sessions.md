# 0029 Call events and sessions

Date: 2026-11-15.

Each call of the box writes two events to the box's event log. Each `run` or `resume` call that holds the run lock also writes two signed records to the study's record log. They say who started a call, how long it ran, how much memory it used, and how it ended. Nothing in a study's verdict reads them.

Decision:

- **Call events.** Each call writes `call.start` to `events.jsonl` in the box home before it does its work. The event holds:
  - `verb`;
  - `study`, for a verb that takes one;
  - `pid`;
  - `parent`, the name of the parent process;
  - `stdin_tty`, true when standard input is a terminal.
- **End event.** When the call returns, it writes an event named after the verb, with the same `span_id` as its `call.start`. It holds `exit`, `duration_ms` and `peak_rss_kib`.
- **Peak memory.** `peak_rss_kib` is the peak resident memory from `getrusage(RUSAGE_SELF)`. It counts the box process only. It excludes child processes, such as the rows and a level-2 machine.
- **No event, no failure.** An event the box cannot write prints one warning to stderr. It never changes the exit code. A box home that cannot be opened warns once per call. A parse error and `dev selftest` write no event.
- **Session records.** Once `run` or `resume` holds the run lock, it signs a `session.start` record into the study's log, before any row. The record holds:
  - `session`, an id new for each session;
  - `verb`, `run` or `resume`;
  - `pid`, `box_version`, `rules_sha256` and `binary_sha256` (0028);
  - `started`, an RFC 3339 time;
  - after a declared change of binary, `changed_from`, the old binary hash, and `reason` (0028, Gate).
- **Session end.** When the run ends, after the late-stop check, the box signs `session.end` with the same `session` and a `status`: `done`, `paused`, `stopped` or `failed`. A failed session also holds `error`, with this machine's folders replaced by placeholders. A call refused before it holds the lock writes no session. The box writes `session.end` after it releases the run lock, so a `session.end` can land after the next session's `session.start`. Readers pair a start with its end by `session`, never by position.
- **A start with no end means the process died.** For example, the kernel killed it or the machine lost power. A `session.end` that cannot be appended looks the same in the log. The failed append leaves the run's own result.
- **Read strictly.** A missing end is never read as success. `verify --study` prints `sessions: <n>, <k> without an end record`, and pairs each start with its end by `session`. When `m` session records have a body that does not parse, the line ends with `, <m> unreadable`.
- **Model.** The session pair follows the minimum run record of Process Run Crate 0.5: one action per execution, with the software that ran, a start, an end and a status. The box writes the start and the end as two records because a process that dies cannot write its end.
- **The card reads them.** The card prints `Run sessions: <n>` from the signed `session.start` records, with `, <k> without an end record` when a start has no end. A log with no session record prints `not in the records`, as before. The card also prints `Registered box: rules <hash>, binary <hash>` from the register record, beside the line for the box that built the card. `verify` and the card count sessions with one function, `derived::sessions`.
- **Observation only.** No reader of rows, spend, budgets, schedule order, `output_hash`, `compare` or verdicts reads a session record. `status` and `progress` give the same figures with or without them. Each run adds two records, so a card's record count grows by two per run.

Evidence: during a 22-hour run, the event log gained 282 `status` lines with nothing saying who made them. The kernel killed one `status` call for memory, and it left no event.

Sources:

- Workflow Run RO-Crate, "Process Run Crate" 0.5: https://www.researchobject.org/workflow-run-crate/profiles/0.5/process_run_crate
- getrusage(2), `RUSAGE_SELF` and what it counts: https://man7.org/linux/man-pages/man2/getrusage.2.html
