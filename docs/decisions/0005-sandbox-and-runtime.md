# 0005 Sandbox and runtime

Date: 2026-10-29.

Amended: 2026-10-30. Level 1 ships as a bubblewrap sandbox (#25).

Amended: 2026-10-31. Rows see a view of the declared inputs only (#31).

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

Case against:

- Level 0 has no isolation, and no rule keeps a confirmatory study off it: `scope.min_isolation` takes 0 to 3 in every mode (`rules/v0.toml`). At level 0 the payload can read the box home (#94).
- bubblewrap is a host program found on `PATH`. Each row record names its version, but no study pins it (0008, Launcher). A launcher of our own is the alternative (0008, Case against).
- A built environment needs a file named `env/uv.lock` (`environ.rs`). The box only hashes it. A payload in another language carries a lockfile that pins nothing, or pins a saved image instead (0008, Environment).
- A view entry is a hard link to the stored blob, not a copy. At level 0 the payload runs, with no isolation, as the user who owns the store (Input view; `store.rs`). So it can make a link writable and change the one blob that every study shares; a copy per study would confine the change. The next `prepare` then refuses every study that holds the blob (0006).

(written 2026-11-19)

What failure this prevents: an unsafe system call made from many places, where one misuse breaks memory safety; and a row that reads a file the plan did not declare, so a result rests on bytes no pin names. The second was the design before #31 (Input view). Both are expected, not observed in a study. (written 2026-11-19)

Constants:

- bubblewrap 0.8 or later. Basis: set in 0008. Rule for change: as in 0008.

(written 2026-11-19)

Sources:

- https://docs.rs/rustix
- https://docs.rs/libc
- https://github.com/containers/bubblewrap
- https://docs.astral.sh/uv/

(written 2026-11-19)
