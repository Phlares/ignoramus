# Litmus

The litmus is a small synthetic study whose answers are known before it runs.
One arm must pass, and two arms must not.
If the box gives other verdicts, the box or its setup is wrong, and no real study on it should be trusted.

## What it holds

`litmus gen` writes a study folder:

- `data/`: one text file per cluster, one line per unit. Each line holds the seed, so two seeds share no file.
- `payload/run`: a copy of the `litmus` program that ran `gen`. It is the payload of every row.
- `spec/pilot.toml`: an exploratory pilot, id `litmus-pilot-<seed>`, 8 clusters. It measures the spread.
- `spec/litmus.toml`: a confirmatory study, id `litmus-<seed>`. It borrows the pilot's estimates, runs a first batch of 12 clusters, and re-sizes once from a reserve of 60.
- `ignorarium.toml`: the empty file that marks the study root.
- `expected.json`: the verdicts the box must give.

Each row writes one value `y`. The reference arm `ref` is exactly 0, so a paired difference equals the other arm's value.
Every claim is "mean below 0" on the range [-1, 1].

| Arm | True mean | Expected verdict |
|---|---|---|
| `good` | -0.6 | `pass` |
| `bad` | +0.3 | `fail` |
| `edge` | 0.0, on the margin | `fail` |

Each value is the arm's mean plus noise within 0.2.
`edge` sits on the edge of the null, so it passes only by chance, at most at the test's level.

## Run it by hand

Build both programs and put them on your `PATH`:

```sh
cargo build --release -p ignorarium -p litmus
export PATH="$PWD/target/release:$PATH"
```

Use a box home made for the litmus, so its studies stay apart from real ones:

```sh
export IGNORARIUM_HOME="$PWD/litmus-home"
litmus gen --seed demo --out litmus-demo
cd litmus-demo

ignorarium check spec/pilot.toml
ignorarium register spec/pilot.toml
ignorarium run litmus-pilot-demo

ignorarium check spec/litmus.toml
ignorarium register spec/litmus.toml
ignorarium run litmus-demo

ignorarium report litmus-demo --json > report.json
litmus verify --report report.json --expected expected.json
```

`litmus verify` exits 0 when every verdict matches. Otherwise it exits 1 and names each hypothesis that differs.

Notes:

- Run `run` from inside the study folder. The box reads the inputs from the folder that holds `ignorarium.toml`.
- `register` needs a terminal. It prints the spec hash and waits for you to type its first 8 characters. A script or pipe cannot approve a plan.
- Until the pilot has finished, `check` on `spec/litmus.toml` exits 3: its estimates wait on the pilot.
- Both specs set `test_log = true`. Their plan leaves and stream slot go to the test log, so the litmus never spends real data or a real stream slot.
- A new seed gives fresh data. The same seed writes the same bytes again, the payload aside. A replay needs a new box home, since the box refuses a study id it already holds.

## Time

At level 0, the end-to-end test (`crates/litmus/tests/e2e.rs`, seed `e2e`) runs both studies in about 10 s in a debug build.
The pilot runs 64 rows. The litmus study runs 96 first-batch rows and 8 rows per reserve cluster it appends; seed `e2e` appends 20, for 256 rows.
The count of appended clusters depends on the first batch, so another seed may run more or fewer rows.

## Higher isolation

`gen --min-isolation N` sets `scope.min_isolation` in both specs.
Level 2 runs rows only inside an environment image.
Import the saved image with `ignorarium import <image> --source <text>`, then pass its store digest: `litmus gen --environment <digest>`.
The payload then runs inside the image, so build `litmus` as a static binary before `gen`.
Without `--environment`, the litmus spec declares a deviation from the pilot's environment: a pilot on the host names no image.

## Soak runs

Two flags make each row cost more, so a long run needs no new code:

- `--work-ms N`: each row burns N ms of CPU time.
- `--mem-mib N`: each row writes N MiB of memory and holds it until the row ends.

Raise `--first` and `--reserve` to run more rows.
`check` refuses counts too small for the power gate (E-CNF-002). At seed `e2e`, a first batch of 12 needs a reserve of at least 30.
