# 0005 Sandbox and runtime

`run` executes each plan row on a backend. Level 0 is a bare process on the host, with no network or filesystem isolation. Level 1 runs the process in a bubblewrap sandbox (0008). This record names the stack that ships and the reason for each part. A crate gets a line in `0001-dependencies.md` when it lands.

Decision:

- `rustix`: safe wrappers to kill a row's process group, to wait on it with `waitid`, and to read the user id. https://docs.rs/rustix
- `libc`: `wait4`, which returns a child's CPU time, and `sigaction`, which installs the SIGINT, SIGTERM and SIGHUP handlers. `rustix` does not cover these. Only `src/sys.rs` calls it. https://docs.rs/libc
- bubblewrap 0.8 or later, found on `PATH`: the level 1 launcher. The box calls it with an argument array and adds no crate for it. A launcher of our own is evaluated against it later (0008). https://github.com/containers/bubblewrap
- Docker or Podman: builds the run environment image. The box calls the tool with an argument array and adds no crate for it.
- `uv`: writes the Python lockfile, so the environment rebuilds from pinned versions. https://docs.astral.sh/uv/

Content store:

- The store keeps blobs under their SHA-256 in `store/`. It adds no crate: `sha2` and `hex` already exist.
- A blob is written once and never changes. The same bytes always land at the same path.

Why: each part has one job, and the unsafe system calls stay in one module behind safe functions.
