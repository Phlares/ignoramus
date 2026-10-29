use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::cli::register::{self, approve};
use ignorarium::cli::{CliError, Done};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::planlog::commitment;
use ignorarium::planlog::store::{Leaf, PlanLog};
use ignorarium::prior::normalized_sha256;
use serde_json::Value;
use std::io::Cursor;
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
