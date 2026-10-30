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

Input view (amended):

- Rows see only the inputs the plan declares. `run` builds `studies/<id>/in/`: one hard link per declared input to its stored blob, at the declared path, and nothing else. The links share the blob's read-only mode, and the view's folders are read-only too. The payload finds the view at `IGNORARIUM_IN`.
- Level 1 binds the view read-only at `/in`. A level 2 backend builds its read-only input disk, mounted at `/in`, from the view (0010). Level 0 hands the payload the view folder itself, with no isolation.
- Before, a row saw the study's `data/` folder. A file the plan did not declare, such as a model kept beside the data, was then either visible without a pin or not visible at all.

Why: each part has one job, and the unsafe system calls stay in one module behind safe functions.
