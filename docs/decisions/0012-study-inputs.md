# 0012 Study inputs: root, store, clusters, payload

Date: 2026-10-31.

A plan names its files by path. This record fixes where a path starts, where the bytes come from, how a file becomes clusters and units, and which code counts as part of the plan.

Decision:

- **Study root.** Paths start at the study root: the nearest folder, from the spec's own folder upward, that holds a regular file named `ignorarium.toml`. The box does not read its content. The search checks the first folder that holds a `.git` entry and goes no further. It never uses a marker in the home folder or the filesystem root. With no marker, the spec's own folder is the root. Inputs and follow-on specs resolve from the root. The path rules stay: no absolute path, no `.` or `..` segment, no link. A signed record holds no absolute or home path, because a published bundle may carry it and it cannot be edited. So `register` records an identity of the root, not its path: the SHA-256 of a label, the box key's secret bytes and the root's canonical path. A published record shows no path, and without the key no reader can test a guessed one. `run` and `resume` find the root by the same walk, from the current folder. They read an input absent at registration only from a root with the registered identity, and refuse otherwise.
- **Content store.** `ignorarium import <file>… --source <text>` adds each file to the store once, under its SHA-256. Beside each blob, `store/meta/<digest>.json` holds its size and its normalized hash. One signed `import` record per file goes to `store/imports.jsonl`, with the source text and the file name only, never the path as given. The box refuses a source text or a file name that looks like a secret or personal data, because a signed record cannot be edited later. An existing blob is hashed again and refused when its bytes differ.
- **Resolve.** A pinned input (a non-empty `sha256`) resolves from the store first: the blob must exist, and its size must equal its meta. Only when the store lacks it does the box read `<root>/<path>`. The box never fetches from the network. `register` imports every input present at the root and hashes every stored input once.
- **Frozen at the first run.** An input absent at registration is read from the root when the study runs. The first `prepare` that passes its checks signs an `inputs` record in the study log: each input's digest, or its absence. A later `run` or `resume` that finds other bytes, a file gone, or a file where there was none is refused, so no study pools rows from two versions of a file.
- **Clusters and units.** Each input with role `selection` or `confirmation` is one cluster, with its path as its id. It declares `units`, the units the file holds (at least 1), and `stratum` when the design names strata. The box has no tokenizer, so it does not count units itself. The study takes `design.units_per_cluster` units from each cluster: `first` takes indices 0 to N − 1; `seeded` (the default) takes the sorted sample of N indices drawn from the seed `units/<path>` (0009). A cluster with fewer units gives all it has. A unit's key is `<path>#<index>`, and the payload gets `--unit <path>#<index>`. A plan of more than 100,000 rows is refused.
- **Disjoint data.** Selection and confirmation inputs are compared by content (exact and normalized hash), not by name.
- **Payload folder.** The payload hash is the tree hash of `payload/` under the root: the sorted rows of [path, `"file"`, SHA-256] of every file, as canonical JSON (0002), hashed with SHA-256. It is the same function as the output hash. A link or any file that is not a regular file is refused. The folder must hold an executable file `run`, which each row starts; `register` refuses it otherwise (E-REG-003). `register` copies the folder into the study folder in the box and records its hash; `pins.payload`, when set, must equal it.

Why:

- One root lets a ladder of studies keep its specs in `spec/` beside one `data/` folder.
- A large model is hashed once per box, not once per check. The run uses the pinned bytes even when the file at the root changes later.
- Content, not names, decides whether two inputs hold the same data.
- The code that measures is part of the plan, so the whole folder is pinned, not one entry file.

Cost: a declared unit count can be wrong. Rows past the end of a file then fail as payload results, so the payload must refuse a unit index the file does not hold.

Case against:

- The box takes the unit count from the author and trusts the payload to refuse an index past the end (Cost). #11 proposed that the checker count units from the file.
- #20 offered a second way to pin the code that measures: the payload writes raw numbers and the box computes the outcomes. The record pins the whole payload folder instead, so the payload still writes its own outcome values (0008, Case against).
- One input file is one cluster, and a spec cannot name a source that clusters share. When clusters share one, the level breaks: in #111's simulation, 16 groups that share half the variance pass 13.7% of null runs at level 0.005. #111 proposes a group above the cluster.
- The root identity hashes the box key's secret bytes and the root's canonical path. A new key or a moved study root gives another identity, and `run` then refuses any study with an input absent at registration (Study root).
- An input absent at registration is fixed only at the first `prepare`, after the plan is in the plan log (Frozen at the first run). So its bytes can be chosen after the plan, and only an outside stamp taken after that `prepare` dates them on a clock other than the box's (0033).

(written 2026-11-19)

What failure this prevents: a study that pools rows from two versions of one input; two inputs that hold the same data counted as two clusters; a change to a payload file other than its entry file that no pin catches; and a signed record that publishes a home path. #20 observed the payload case in a repro. The rest is expected, not observed in a study. (written 2026-11-19)

Constants:

- most rows in a plan = 100,000 (`ROW_CAP`). Basis: `check` builds the whole schedule to count its rows, so a mistyped unit count must stop there before it fills the memory (E-UNT-002); for 100,000, none on file. Rule for change: none stated.
- units per input = 1 up to 4,294,967,295. Basis: a value outside the range is meaningless (E-RNG-001). Rule for change: none stated.

(written 2026-11-19)

Sources: none. The record rests on 0002 and 0009 and cites no outside work. (written 2026-11-19)
