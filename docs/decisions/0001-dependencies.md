# 0001 Dependencies

Date: 2026-10-28.

Decision:

Each dependency has one job. Add a line here with its reason when you add a crate.

Runtime:

- `clap`: parses command-line arguments.
- `serde` (feature `derive`): reads and writes typed data, and derives the spec schema structs.
- `serde_json` (feature `float_roundtrip`): reads and writes JSON, the base of the canonical form we hash. The feature makes a 17-digit float read back as the same number, so `verify` does not report a bad hash on an untouched record.
- `toml`: reads study specs, which use TOML.
- `sha2`: computes SHA-256 hashes.
- `hex`: writes hashes as text.
- `digest-io` (from RustCrypto): gives a `sha2` hasher the `io::Write` interface, which `sha2` 0.11 dropped. The box uses it where it streams a hash from a reader: the guest's `tree.rs` and the deposit's `zenodo.rs` (#44), and `canon.rs` (#70).
- `ed25519-dalek`: signs records with a local key.
- `getrandom`: draws random bytes for seeds and keys from the operating system.
- `regex`: matches text patterns in checks.
- `rustix`: safe system calls. The runner kills a process group, waits on a child with `waitid` and reads the user id; level 1 makes the pipes its sandbox inherits (feature `pipe`) and opens the sockets the sandbox self-test tries (feature `net`). The guest crate also mounts the payload's view, sets the clock, sets `no_new_privs`, makes FIFOs when it unpacks outputs, and powers the guest off.
- `seccompiler` (from the Firecracker project): compiles the level 1 seccomp filter to BPF on the host, in pure Rust, for bubblewrap to load. https://github.com/rust-vmm/seccompiler
- `tar` (no default features): reads the layers of a saved container image header by header, so one parser decides what each member is and the box writes the tree itself. The box never calls the `tar` program or the crate's unpack functions. https://github.com/alexcrichton/tar-rs
- `flate2` (default features, pure-Rust backend, no C library): decodes the gzip layers that the containerd image store of Docker saves, so the same `tar` reader parses plain and gzip layers. https://github.com/rust-lang/flate2-rs
- `der` (features `derive`, `oid` and `alloc`): decodes the DER bytes of an RFC 3161 time-stamp reply. The box declares the reply's shapes itself in `src/tsp.rs`: the CMS token, the TSTInfo, and of each certificate only the issuer, serial number and subject. It brings `der_derive` and reuses the `const-oid` 0.10 that `sha2` brings, so it adds no second version of any crate. The box parses the reply and does not verify the authority's signature; the reader does that with `openssl ts -verify`. https://github.com/RustCrypto/formats/tree/master/der
- `evidence` (this workspace's crate, by path; no dependencies of its own): the statistics engine. The seeded draws that pick each cluster's units come from its `Rng`.
- `ignorarium-guest` (a crate of this workspace): the level 2 host side, which starts each row's virtual machine and reads its outputs back, and the guest agent that runs the payload inside it.
- `libc`: the system calls rustix lacks, called only from each crate's `src/sys.rs`: a child's CPU time, signal handlers, vsock sockets, the kernel random-pool reseed, a kill of every other process, and `close_range`, which marks inherited descriptors close-on-exec in a level 1 sandbox launcher's child; also the `O_NOFOLLOW` and `O_NONBLOCK` open flags. The level 1 seccomp filter takes its system call numbers from it.

Development:

- `assert_cmd`: runs the built binary in tests.
- `predicates`: states what the binary output must contain.
- `tempfile`: makes scratch folders in tests.
- `jsonschema` (no default features, so no network crate): checks each `--json` output against its file in `docs/schemas/`.

Minimum Rust version: 1.89, for `std::fs::File::lock`.

Why:

- The reason for the rule is not on file. The PR that added the record (#1) gives none.
- The lines show how the rule is used. Several name what a crate keeps out of the build: a C library (`flate2`), a second version of any crate (`der`), dependencies of its own (`evidence`). The `tar` line names functions the box never calls.

(written 2026-11-21)

Case against:

- A list kept by hand drifts from the manifests, and no test compares them. `digest-io` entered two manifests in #44 and a third in #70, and had no line here until 2026-11-21. In that time 0011 said its two crates use only dependencies listed here, and one of them, `deposit`, used `digest-io`. A reason comment beside each dependency in its manifest would sit where a crate is added. The root manifest already keeps such a comment beside each profile override.
- The list repeats what Cargo holds and adds only the reason. Six manifests and the CI matrix already state the minimum Rust version. Git alone holds the date of each line (`CONTRIBUTING.md`, Decision records).
- "One job" sets no bound on how wide a job is. The `rustix` job is "safe system calls", and its line runs from process control to power-off. The `libc` job is "the system calls rustix lacks".
- The rule states a job for the direct dependencies only. The lock file holds 155 packages. Dependabot's security updates are the only watch on them (`.github/dependabot.yml`); no CI job audits them, and #94 asks for `cargo audit` or `cargo deny` in CI.

(written 2026-11-21)

What failure this prevents: a crate in the build whose job no one has stated, and a crate that stays after its one use is gone. The first happened with `digest-io`, and the rule did not stop it (Case against). The second is expected, not observed in a study. (written 2026-11-21)

Constants:

- minimum Rust version = 1.89 (`rust-version` in each manifest; CI tests 1.89 and stable). Basis: the box uses `std::fs::File::lock` (Minimum Rust version). Rule for change: none stated.

(written 2026-11-21)

Sources:

- `seccompiler`: https://github.com/rust-vmm/seccompiler
- `tar`: https://github.com/alexcrichton/tar-rs
- `flate2`: https://github.com/rust-lang/flate2-rs
- `der`: https://github.com/RustCrypto/formats/tree/master/der

(written 2026-11-21)
