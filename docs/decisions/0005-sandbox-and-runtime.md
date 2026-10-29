# 0005 Sandbox and runtime

`run` executes each plan row in a sandbox with the network off. This record names the planned stack and the reason for each part. A crate gets a line in `0001-dependencies.md` when it lands.

Decision:

- `rustix`: safe wrappers for `unshare`, `mount`, `pivot_root` and `prctl(PR_SET_NO_NEW_PRIVS)`. It keeps `unsafe` out of the launcher. https://docs.rs/rustix
- `libc`: `getrusage`, `wait4`, and the syscall numbers the seccomp filter denies. `rustix` does not cover these. https://docs.rs/libc
- `landlock`: filesystem rules at the best ABI the kernel offers. https://docs.kernel.org/userspace-api/landlock.html
- `seccompiler`: compiles the BPF syscall filter in pure Rust. Firecracker uses it. https://github.com/rust-vmm/seccompiler
- Docker or Podman: builds the run environment image. The box calls the tool with an argument array and adds no crate for it.
- `uv`: writes the Python lockfile, so the environment rebuilds from pinned versions. https://docs.astral.sh/uv/

Content store:

- The store keeps blobs under their SHA-256 in `store/`. It adds no crate: `sha2` and `hex` already exist.
- A blob is written once and never changes. The same bytes always land at the same path.

Why: each part has one job, and the unsafe system calls stay in one module behind safe functions.
