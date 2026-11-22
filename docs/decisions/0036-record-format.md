# 0036 The record format has a number

Date: 2026-11-21. Decided and built.

No record said which format a study's log is in. A later box had no way to tell an old log from a new one, and no stored log showed that an old one still reads (#90).

Decision:

- **The number.** A study's log states its record format as `format`, an integer, in its `register` record. The `register` record is the first record of the log. A `register` record without `format` is format 0. Every log written before this record is format 0.
- **Format 1.** This box writes format 1. Format 1 differs from format 0 by the number alone: no record changes shape. So this box reads both, and it appends to a study of either.
- **The check.** Every function that reads a log from its first line goes through one check in `records.rs`. An append reads only the last line; each verb that appends to an existing log reads it first. The check reads `format` from the raw body of the first record when that record is a `register`, after that record passed its hash, signature and chain checks. A number above the box's own refuses the read. So does a value that is not a count: a string, a fraction, a negative number, `null`, or a number above 2^32 − 1.
- **What refuses.** Every verb that reads a study's log refuses a study whose format the check refuses, and writes nothing in the study's folder. With the box key, `stop` sends no stop request to such a study. One write remains: with no key, or with a first record the key does not verify, `stop` still sends its request, as 0034 rules for a log that cannot be read. A study of such a format that holds `done` gets the format message, not `E-DON-001` (0034, as amended). `check`, `review`, `register` and `report` also read the first record of every study in the box home before they do their work, and refuse when the check refuses one; the message names that study. `run` and `resume` of a study that takes records from another study read every log of the box home, and fail on such a study as they fail on any log there that does not verify.
- **Other faults stay where they were.** In `check`, `review`, `register` and `report`, a log that cannot be read for another reason (no key, a broken chain), and a studies folder that cannot be listed, fail only the rule that needs the log, as before.
- **`verify`** prints `record format: <n>`. For format 0 it prints `record format: 0 (not recorded)`.
- **Later formats.** A change to the shape of any record of a study's log takes a new number and a new record under `docs/decisions/`. That record states which older formats the box still appends to.
- **Stored logs.** `crates/ignorarium/tests/fixtures/logs/format-<n>/` holds one finished study per format: its record log, as plain text, and the public key that checks it. Each is the smallest study of the test suite, run through the binary: 14 records, about 15 kB. One test reads every folder with the current box. A folder is never edited, since a signed log cannot be made again. A new format adds a folder. The `resize` kind, which this small study lacks, has one stored line in `tests/compat.rs`. Other kinds have no stored sample yet, among them `child-estimate`, `opened`, `selection`, `spawn-stop`, `spend`, `stamp`, `stop` and `trigger`.
- **What the number does not cover.** The plan log, the stream log, the event log and the card file, nor the files beside the log in a study's folder (the control file, the state file, the deviation log, the stamps). The card file has its own number, `card_version`.

Why:

- A log is the only thing a study leaves. A reader years later holds the log and a newer box. Without a number, a changed record shape reads as a damaged log or, worse, reads wrongly with no message.
- The `register` record already holds what identifies the box that registered the plan (0028). The format is one more fact of the same kind, added the same way: an optional field that older logs lack.
- One place for the check means one place to get it right. Each verb reads a log through the same functions, so none can forget the check.
- A program that meets a file of a format it does not know changes nothing in it. Git must not operate on a repository whose version it does not understand, since that risks wrong results and lost data. SQLite neither reads nor writes a file with a newer read version (Sources). The box has one number, so a format it does not read refuses the read and every write to the log. With the box key, `stop` sends no request either.
- The check reads the raw body, not the typed record. A later format may add a field that this box's typed reader refuses. The number must still be readable then, or the box would say "not registered" where it should say "newer format".
- A stored log is evidence that old logs read. A test that builds its records with the current code shows only that the box reads what it writes today. Lucene tests its old formats both ways: unit tests that write then read, and one test that loads indices written by earlier versions (Sources).
- An old log is stored because it cannot be made again. Recomputation replaces a stored result only when a deterministic process still runs (Adams). Each record holds its time, and the code that wrote format 0 leaves the tree.

Case against:

- A log can hold records of two builds of the box: a run may continue on a changed binary when the change is declared (0028). The number sits in one record, so it cannot say that the format changed part of the way through. We rejected a number in every record: it changes the envelope of every record and the canonical line that each hash covers. The rule for later formats answers the case instead: a box appends only to the formats its record names.
- A box older than this record gives no format message. It reads a `register` record that holds `format` as no register (`deny_unknown_fields`), as it does for `gate` and `box`, and says the study is not registered.
- One study of a newer format stops `check`, `review`, `register` and `report` for the whole box home. A skip with a warning would let work go on. We refuse because a plan is checked against the other studies of the box, and a study the box cannot read is one the check cannot see. `report` follows the same rule, so the four verbs agree.
- With the box key, a `stop` typed on this box does not reach a live run of a newer-format study. A newer box holds that run, and it goes on until it ends or that box stops it. 0034 sent the request to every log it could not read; it is amended. We send nothing because this box cannot read that log: it cannot see `done`, so its request could stay on a study that has ended. The refusal names the format, and the operator repeats the stop on the newer box. SQLite still reads a file whose write version alone is newer; the box has no such middle case.
- The stored logs name the machine that wrote them: its CPU model, a hash of its CPU flags, a cgroup path, process ids and the time. They are public, and a signed log cannot be changed later.

What failure this prevents: a later box that reads an old log by new rules and gives another result with no message; and a change to a record's shape that breaks old logs unseen, because no test read one. Expected, not observed.

Constants:

- `FORMAT` = 1 (`crates/ignorarium/src/records.rs`). Basis: the first format that states its number; format 0 is every log without one. Rule for change: it rises by one when the shape of a record of a study's log changes, with a new decision record and a new stored log.

Sources:

- Git, "repository-version": every repository states a numeric format version; an implementation that does not understand the version "MUST NOT operate on that repository; doing so risks not only producing wrong results, but actually losing data". https://git-scm.com/docs/repository-version
- SQLite, "Database File Format", section 1.3.3: a file states a read version and a write version; a file with a newer write version "must be treated as read-only", and one with a newer read version "cannot be read or written". https://sqlite.org/fileformat.html
- Apache Lucene, "Index backwards compatibility": older formats are tested by unit tests that write then read with each old format, and by a test that "loads indices created in previous versions and checks that we can search them". https://github.com/apache/lucene/blob/main/lucene/backward-codecs/README.md
- Adams et al., "Maximizing Efficiency By Trading Storage for Computation" (HotCloud 2009): recomputation can replace a stored result only when the process is known, deterministic, and can be run again over the result's whole lifetime. https://static.usenix.org/events/hotcloud09/tech/full_papers/adams/adams_html/
- This record follows 0002 (the canonical line) and 0028 (the box identity in the register record).
