mod common;

use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::cli::register::{self, approve};
use ignorarium::cli::{CliError, Done};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::planlog::commitment;
use ignorarium::planlog::store::{Leaf, PlanLog};
use ignorarium::prior::normalized_sha256;
use serde_json::Value;
use std::io::{BufRead, Cursor, Read};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PROMPT: &str = "Type the first 8 characters of the spec hash to approve:";

struct Fixture {
    _dir: TempDir,
    home: Home,
}

/// A box home with `max_level = 1`, and a spec made from a template in a
/// study folder that holds `data/example.txt`.
fn setup(template: &str, edit: impl Fn(String) -> String) -> (Fixture, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("home");
    let home = Home::open_at(&root).unwrap();
    std::fs::write(root.join("machine.toml"), "max_level = 1\n").unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let study = dir.path().join("work");
    std::fs::create_dir_all(study.join("data")).unwrap();
    std::fs::write(study.join("data/example.txt"), "Hello  World").unwrap();
    let text = std::fs::read_to_string(format!("../../templates/{template}.toml")).unwrap();
    let spec = study.join("spec.toml");
    std::fs::write(&spec, edit(text)).unwrap();
    (Fixture { _dir: dir, home }, spec)
}

fn same(t: String) -> String {
    t
}

fn go(f: &Fixture, spec: &Path, typed: &str, tty: bool) -> (Result<Done, CliError>, String) {
    let mut input = Cursor::new(typed.as_bytes().to_vec());
    let mut output = Vec::new();
    let r = register::run(spec, &f.home, &mut input, &mut output, tty);
    (r, String::from_utf8(output).unwrap())
}

fn hash_of(spec: &Path) -> String {
    ignorarium::spec::load(spec).unwrap().hash
}

fn answer(spec: &Path) -> String {
    format!("{}\n", &hash_of(spec)["sha256:".len()..][..8])
}

fn leaves(dir: &Path) -> Vec<Leaf> {
    std::fs::read_to_string(dir.join("leaves.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn records(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn approve_needs_the_exact_prefix() {
    let h = "sha256:7ce680b2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert!(approve("7ce680b2", h));
    assert!(!approve("7ce680b3", h));
    assert!(!approve("", h));
    assert!(!approve("7ce680b", h));
    assert!(!approve("7ce680b2a", h));
    assert!(!approve("7CE680B2", "sha256:7CE680B2aaaa"));
    assert!(!approve("zzzzzzzz", "sha256:zzzzzzzz"));
    assert!(!approve("7ce680b2", "7ce680b2aaaa"));
    assert!(!approve("7ce680", "sha256:7ce680"));
    assert!(approve(" 7ce680b2 ", h));
    assert!(approve(
        "	7ce680b2
",
        h
    ));
    assert!(!approve("7ce 680b2", h));
}

#[test]
fn success_writes_four_files_a_plan_leaf_and_a_record() {
    let (f, spec) = setup("numeric-ablation", same);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    let done = r.unwrap();
    assert_eq!(done.code, 0);
    assert_eq!(done.spec_hash.as_deref(), Some(hash_of(&spec).as_str()));
    assert_eq!(done.leaf, Some(0));
    assert!(out.contains(PROMPT), "{out}");
    let dir = f.home.study_dir("my-study");
    assert_eq!(
        std::fs::read(dir.join("spec.toml")).unwrap(),
        std::fs::read(&spec).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("spec.hash")).unwrap(),
        format!("{}\n", hash_of(&spec))
    );
    let canon = to_canonical(&ignorarium::spec::load(&spec).unwrap().value).unwrap();
    assert_eq!(std::fs::read(dir.join("spec.json")).unwrap(), canon);
    let salt = std::fs::read(dir.join("salt")).unwrap();
    assert_eq!(salt.len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("salt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The main log gets the leaf; the test log stays empty.
    let ls = leaves(&f.home.root.join("planlog"));
    assert_eq!(ls.len(), 1);
    assert!(leaves(&f.home.root.join("testlog")).is_empty());
    let leaf = &ls[0];
    assert_eq!(leaf.kind, "plan");
    assert_eq!(leaf.mode.as_deref(), Some("exploratory"));
    assert_eq!(leaf.label, "signal");
    let salt: [u8; 32] = salt.try_into().unwrap();
    assert_eq!(
        leaf.commitment.as_deref(),
        Some(commitment(&salt, &canon).as_str())
    );
    assert_eq!(leaf.family.as_ref().map(String::len), Some(16));
    // The input exists, so its hashes are in the leaf.
    assert_eq!(
        leaf.data_hashes,
        [(sha256_hex(b"Hello  World"), sha256_hex(b"hello world"))]
    );
    assert_eq!(
        normalized_sha256("Hello  World"),
        sha256_hex(b"hello world")
    );
    let key = keys::load_or_create(&f.home).unwrap();
    let v = PlanLog::open(&f.home.root.join("planlog"))
        .unwrap()
        .verify(&key.verifying_key())
        .unwrap();
    assert_eq!((v.signed, v.leaves), (1, 1));
    let rp = dir.join("records.jsonl");
    assert_eq!(
        ignorarium::records::verify(&rp, &key.verifying_key()).unwrap(),
        1
    );
    let rec = &records(&rp)[0]["record"];
    assert_eq!(rec["kind"], "register");
    assert_eq!(rec["study"], "my-study");
    assert_eq!(rec["body"]["spec_hash"], hash_of(&spec).as_str());
    assert_eq!(rec["body"]["leaf"], 0);
    assert_eq!(rec["body"]["log"], "planlog");
    assert!(rec["body"].get("timestamp").is_none());
}

#[test]
fn defaults_are_recorded_and_the_stored_spec_is_the_authors() {
    let (f, spec) = setup("numeric-ablation", |t| t.replace("alpha = 0.005\n", ""));
    go(&f, &spec, &answer(&spec), true).0.unwrap();
    let dir = f.home.study_dir("my-study");
    let body = records(&dir.join("records.jsonl"))[0]["record"]["body"].clone();
    let d = body["defaults"].as_array().unwrap();
    assert!(
        d.iter()
            .any(|x| x["path"] == "design.alpha" && x["value"] == 0.005),
        "{d:?}"
    );
    let json: Value =
        serde_json::from_slice(&std::fs::read(dir.join("spec.json")).unwrap()).unwrap();
    assert!(json["design"].get("alpha").is_none());
}

#[test]
fn register_records_the_absolute_root() {
    let (f, spec) = setup("numeric-ablation", same);
    let work = spec.parent().unwrap().to_path_buf();
    go(&f, &spec, &answer(&spec), true).0.unwrap();
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    // No marker: the root is the spec's folder.
    assert_eq!(
        records(&rp)[0]["record"]["body"]["root"],
        work.to_str().unwrap()
    );
    // With a marker above the spec, the root is the marker's folder.
    let (f, spec) = setup("numeric-ablation", same);
    let work = spec.parent().unwrap().to_path_buf();
    std::fs::create_dir(work.join("spec")).unwrap();
    std::fs::rename(&spec, work.join("spec/s.toml")).unwrap();
    std::fs::write(work.join("ignorarium.toml"), "").unwrap();
    let spec = work.join("spec/s.toml");
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    assert_eq!(
        records(&rp)[0]["record"]["body"]["root"],
        work.to_str().unwrap()
    );
}

#[test]
fn the_planted_spawn_template_registers_into_the_test_log() {
    let (f, spec) = setup("planted-spawn", same);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(leaves(&f.home.root.join("planlog")).is_empty());
    let ls = leaves(&f.home.root.join("testlog"));
    assert_eq!(ls.len(), 1);
    assert_eq!(ls[0].kind, "plan");
    let rp = f.home.study_dir("my-spawn-study").join("records.jsonl");
    assert_eq!(records(&rp)[0]["record"]["body"]["log"], "testlog");
}

#[test]
fn a_wrong_prefix_writes_nothing() {
    let (f, spec) = setup("numeric-ablation", same);
    for typed in ["00000000\n", "\n", ""] {
        let (r, out) = go(&f, &spec, typed, true);
        assert_eq!(r.unwrap_err().code, 1, "{typed:?}");
        assert!(out.contains(PROMPT));
    }
    assert!(!f.home.study_dir("my-study").exists());
    assert!(leaves(&f.home.root.join("planlog")).is_empty());
}

#[test]
fn a_crlf_answer_is_accepted() {
    let (f, spec) = setup("numeric-ablation", same);
    let typed = answer(&spec).replace('\n', "\r\n");
    assert_eq!(go(&f, &spec, &typed, true).0.unwrap().code, 0);
}

#[test]
fn a_second_register_of_the_same_id_is_refused() {
    let (f, spec) = setup("numeric-ablation", same);
    go(&f, &spec, &answer(&spec), true).0.unwrap();
    let log = f.home.root.join("planlog/leaves.jsonl");
    let before = std::fs::read_to_string(&log).unwrap();
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    let e = r.unwrap_err();
    assert_eq!(e.code, 1);
    assert!(e.msg.contains("E-REG-002"), "{}", e.msg);
    assert!(e.msg.contains("already registered"), "{}", e.msg);
    assert!(e.msg.contains("docs/rules/E-REG-002.md"), "{}", e.msg);
    assert!(!out.contains(PROMPT));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    assert_eq!(records(&rp).len(), 1);
}

#[test]
fn a_failure_before_the_log_append_leaves_no_study_folder() {
    let (f, spec) = setup("numeric-ablation", same);
    let key = f.home.root.join("keys/box.ed25519");
    let heads = f.home.root.join("planlog/heads.jsonl");
    let dir = f.home.study_dir("my-study");
    // A short key, then a log whose head file lacks its final line feed.
    std::fs::write(&key, [0u8; 31]).unwrap();
    let (r, _) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap_err().code, 2);
    assert!(!dir.exists());
    std::fs::write(&key, [7u8; 32]).unwrap();
    std::fs::create_dir_all(heads.parent().unwrap()).unwrap();
    std::fs::write(&heads, b"{}").unwrap();
    let (r, _) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap_err().code, 2);
    assert!(!dir.exists());
    // The same for the leaves file, and nothing was appended anywhere.
    std::fs::write(&heads, b"").unwrap();
    let leaves = f.home.root.join("planlog/leaves.jsonl");
    std::fs::write(&leaves, b"{}").unwrap();
    let (r, _) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap_err().code, 2);
    assert!(!dir.exists());
    assert_eq!(std::fs::read(&leaves).unwrap(), b"{}");
    // Repaired, the same id registers.
    std::fs::write(&leaves, b"").unwrap();
    let (r, _) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0);
    assert!(dir.join("records.jsonl").is_file());
}

#[test]
fn no_terminal_refuses_before_reading_anything() {
    let (f, spec) = setup("numeric-ablation", same);
    let (r, out) = go(&f, &spec, &answer(&spec), false);
    let e = r.unwrap_err();
    assert_eq!(e.code, 1);
    assert!(e.msg.contains("E-REG-001"), "{}", e.msg);
    assert!(e.msg.contains("interactive terminal"), "{}", e.msg);
    assert!(out.is_empty(), "{out}");
    assert!(!f.home.study_dir("my-study").exists());
}

#[test]
fn a_spec_with_an_error_refuses_before_the_prompt() {
    let (f, spec) = setup("numeric-ablation", |t| {
        t.replace("unit = \"ratio\"", "unit = \"\"")
    });
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 1);
    assert!(out.contains("error[E-OUT-001]"), "{out}");
    assert!(!out.contains(PROMPT), "{out}");
    assert!(!f.home.study_dir("my-study").exists());
}

/// A reader that runs `hook` when the program first reads, that is, at the prompt.
struct Hooked<R, F: FnOnce()> {
    inner: R,
    hook: Option<F>,
}

impl<R: Read, F: FnOnce()> Read for Hooked<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: BufRead, F: FnOnce()> BufRead for Hooked<R, F> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if let Some(h) = self.hook.take() {
            h();
        }
        self.inner.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.inner.consume(n);
    }
}

#[test]
fn an_input_changed_after_the_check_is_refused_and_nothing_is_written() {
    type Edit = fn(&Path);
    let edits: [(&str, Edit); 3] = [
        ("changed", |p| std::fs::write(p, "other").unwrap()),
        ("removed", |p| std::fs::remove_file(p).unwrap()),
        ("pinned-then-changed", |p| {
            std::fs::write(p, "Hello  World!").unwrap()
        }),
    ];
    for (name, edit) in edits {
        let (f, spec) = setup("numeric-ablation", same);
        let data = spec.parent().unwrap().join("data/example.txt");
        let mut input = Hooked {
            inner: Cursor::new(answer(&spec).into_bytes()),
            hook: Some(move || edit(&data)),
        };
        let mut out = Vec::new();
        let e = register::run(&spec, &f.home, &mut input, &mut out, true).unwrap_err();
        assert_eq!(e.code, 1, "{name}");
        assert!(e.msg.contains("example.txt"), "{name}: {}", e.msg);
        assert!(!f.home.study_dir("my-study").exists(), "{name}");
        assert!(leaves(&f.home.root.join("planlog")).is_empty(), "{name}");
    }
}

#[test]
fn an_input_that_appears_after_the_check_is_refused() {
    let (f, spec) = setup("numeric-ablation", same);
    let data = spec.parent().unwrap().join("data/example.txt");
    std::fs::remove_file(&data).unwrap();
    let mut input = Hooked {
        inner: Cursor::new(answer(&spec).into_bytes()),
        hook: Some(|| std::fs::write(&data, "late").unwrap()),
    };
    let mut out = Vec::new();
    let e = register::run(&spec, &f.home, &mut input, &mut out, true).unwrap_err();
    assert_eq!(e.code, 1);
    assert!(!f.home.study_dir("my-study").exists());
}

#[test]
fn the_commitment_binds_the_authors_spec_not_the_defaulted_one() {
    let (f, spec) = setup("numeric-ablation", |t| {
        t.replace(
            "alpha = 0.005
",
            "",
        )
    });
    go(&f, &spec, &answer(&spec), true).0.unwrap();
    let dir = f.home.study_dir("my-study");
    let salt: [u8; 32] = std::fs::read(dir.join("salt")).unwrap().try_into().unwrap();
    let stored = std::fs::read(dir.join("spec.json")).unwrap();
    let leaf = &leaves(&f.home.root.join("planlog"))[0];
    assert_eq!(
        leaf.commitment.as_deref(),
        Some(commitment(&salt, &stored).as_str())
    );
    // The defaulted spec has other bytes, and would give another commitment.
    let mut filled = ignorarium::spec::load(&spec).unwrap().value;
    ignorarium::estimate::apply_defaults(&mut filled);
    let filled = to_canonical(&filled).unwrap();
    assert_ne!(filled, stored);
    assert_ne!(
        leaf.commitment.as_deref(),
        Some(commitment(&salt, &filled).as_str())
    );
}

#[test]
fn a_confirmatory_plan_registers_with_a_pending_timestamp() {
    let (f, spec) = setup("numeric-ablation", same);
    let source = common::plant_estimate(&f.home, "ppl-ratio", 4.0);
    let t = std::fs::read_to_string(&spec).unwrap();
    let edited = {
        let ids = |p: &str, n: usize| {
            format!(
                "clusters = [{}]",
                (0..n)
                    .map(|i| format!("\"{p}{i:02}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        t.replace("mode = \"exploratory\"", "mode = \"confirmatory\"")
            .replace("label = \"signal\"", "label = \"confirmed\"")
            .replace("sesoi = 0.0", "sesoi = 1.0")
            .replace("value = 0.0", "value = 4.0")
            .replace(
                "source = \"stated\"",
                &format!("source = \"record:{source}\""),
            )
            .replace("units_per_cluster = 1", "units_per_cluster = 5")
            .replacen("clusters = []", &ids("s", 3), 1)
            .replacen("clusters = []", &ids("c", 11), 1)
            .replace(
                "triggers = []",
                "triggers = [{ id = \"T1\", metric = \"ppl-ratio\", threshold_e = 1.0 }]",
            )
    };
    std::fs::write(&spec, edited).unwrap();
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    let rec = &records(&rp)[0]["record"];
    assert_eq!(rec["body"]["timestamp"], "pending");
    assert_eq!(
        leaves(&f.home.root.join("planlog"))[0].mode.as_deref(),
        Some("confirmatory")
    );
}

#[test]
fn the_salt_appears_in_no_output_record_or_log() {
    let (f, spec) = setup("numeric-ablation", same);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    r.unwrap();
    let salt = std::fs::read(f.home.study_dir("my-study").join("salt")).unwrap();
    let hex = salt.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut seen = vec![out];
    for rel in [
        "studies/my-study/records.jsonl",
        "studies/my-study/spec.hash",
        "planlog/leaves.jsonl",
        "planlog/heads.jsonl",
    ] {
        seen.push(std::fs::read_to_string(f.home.root.join(rel)).unwrap());
    }
    for text in seen {
        assert!(!text.contains(&hex), "salt leaked");
    }
}
