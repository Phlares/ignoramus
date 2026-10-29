# 0001 Dependencies

Each dependency has one job. Add a line here with its reason when you add a crate.

Runtime:

- `clap`: parses command-line arguments.
- `serde` (feature `derive`): reads and writes typed data, and derives the spec schema structs.
- `serde_json` (feature `float_roundtrip`): reads and writes JSON, the base of the canonical form we hash. The feature makes a 17-digit float read back as the same number, so `verify` does not report a bad hash on an untouched record.
- `toml`: reads study specs, which use TOML.
- `sha2`: computes SHA-256 hashes.
- `hex`: writes hashes as text.
- `ed25519-dalek`: signs records with a local key.
- `getrandom`: draws random bytes for seeds and keys from the operating system.
- `regex`: matches text patterns in checks.
- `rustix`: kills a process group, waits on a child with `waitid` and reads the user id, through safe interfaces.
- `libc`: reads a child's CPU time with `wait4` and installs signal handlers with `sigaction`; only `src/sys.rs` calls it.
- `libc`: gives the `O_NOFOLLOW` and `O_NONBLOCK` flags for opening output files, and the few calls rustix lacks (virtual sockets, the random-pool reseed, child CPU time, a kill of every other process) in `sys.rs`.
- `rustix` (features `fs`, `mount`, `process`, `system`, `thread`, `time`): makes FIFOs when the host unpacks guest output, sets the guest clock, waits for and signals the payload process group, mounts the payload's view, keeps setuid files from giving the payload root (`no_new_privs`), and powers the guest off, all without `unsafe`.

Development:

- `assert_cmd`: runs the built binary in tests.
- `predicates`: states what the binary output must contain.
- `tempfile`: makes scratch folders in tests.

Minimum Rust version: 1.89, for `std::fs::File::lock`.
