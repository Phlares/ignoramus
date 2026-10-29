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
- `rustix`: safe system calls. The runner kills a process group, waits on a child with `waitid` and reads the user id; level 1 makes the pipes its sandbox inherits (feature `pipe`) and opens the sockets the sandbox self-test tries (feature `net`). The guest crate also mounts the payload's view, sets the clock, sets `no_new_privs`, makes FIFOs when it unpacks outputs, and powers the guest off.
- `seccompiler` (from the Firecracker project): compiles the level 1 seccomp filter to BPF on the host, in pure Rust, for bubblewrap to load. https://github.com/rust-vmm/seccompiler
- `libc`: the system calls rustix lacks, called only from each crate's `src/sys.rs`: a child's CPU time, signal handlers, vsock sockets, the kernel random-pool reseed, a kill of every other process, and `close_range`, which marks inherited descriptors close-on-exec in a level 1 sandbox launcher's child; also the `O_NOFOLLOW` and `O_NONBLOCK` open flags. The level 1 seccomp filter takes its system call numbers from it.

Development:

- `assert_cmd`: runs the built binary in tests.
- `predicates`: states what the binary output must contain.
- `tempfile`: makes scratch folders in tests.

Minimum Rust version: 1.89, for `std::fs::File::lock`.
