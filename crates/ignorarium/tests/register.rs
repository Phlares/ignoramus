mod common;

use ignorarium::canon::{sha256_hex, to_canonical};
use ignorarium::cli::register::{self, approve};
use ignorarium::cli::{CliError, Done};
use ignorarium::home::Home;
use ignorarium::keys;
use ignorarium::planlog::commitment;
use ignorarium::planlog::store::{Leaf, PlanLog};
use ignorarium::prior::normalized_sha256;
use ignorarium::stream::{Log, LogName};
use serde_json::{Value, json};
use std::io::{BufRead, Cursor, Read};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PROMPT: &str = "Type the first 8 characters of the spec hash to approve:";

struct Fixture {
    tmp: TempDir,
    home: Home,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::unseal(self.tmp.path());
    }
}

/// A box home with `max_level = 1`, and a spec made from a template in a
/// study folder that holds `data/example.txt` and `payload/run`.
fn setup(template: &str, edit: impl Fn(String) -> String) -> (Fixture, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("home");
    let home = Home::open_at(&root).unwrap();
    std::fs::write(root.join("machine.toml"), "max_level = 1\n").unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let study = dir.path().join("work");
    std::fs::create_dir_all(study.join("data")).unwrap();
    std::fs::write(study.join("data/example.txt"), "Hello  World").unwrap();
    std::fs::create_dir(study.join("payload")).unwrap();
    common::write_exec(&study.join("payload/run"), common::PAYLOAD);
    let text = std::fs::read_to_string(format!("../../templates/{template}.toml")).unwrap();
    let spec = study.join("spec.toml");
    std::fs::write(&spec, edit(text)).unwrap();
    (Fixture { tmp: dir, home }, spec)
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
    let (f, spec) = setup("numeric-ablation", |t| t.replace("power = 0.8\n", ""));
    go(&f, &spec, &answer(&spec), true).0.unwrap();
    let dir = f.home.study_dir("my-study");
    let body = records(&dir.join("records.jsonl"))[0]["record"]["body"].clone();
    let d = body["defaults"].as_array().unwrap();
    assert!(
        d.iter()
            .any(|x| x["path"] == "design.power" && x["value"] == 0.8),
        "{d:?}"
    );
    let json: Value =
        serde_json::from_slice(&std::fs::read(dir.join("spec.json")).unwrap()).unwrap();
    assert!(json["design"].get("power").is_none());
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

/// Run `register` on `spec` with `hook` called at the approval prompt.
fn at_prompt(f: &Fixture, spec: &Path, hook: impl FnOnce()) -> (Result<Done, CliError>, String) {
    let mut input = Hooked {
        inner: Cursor::new(answer(spec).into_bytes()),
        hook: Some(hook),
    };
    let mut out = Vec::new();
    let r = register::run(spec, &f.home, &mut input, &mut out, true);
    (r, String::from_utf8(out).unwrap())
}

/// The names in the box's studies folder.
fn studies(f: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(f.home.root.join("studies"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Nothing is registered under `my-study`, and only one staging folder is left, which the
/// error names.
fn only_staging_left(f: &Fixture, e: &CliError) {
    let names = studies(f);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with(".my-study.") && names[0].ends_with(".tmp"),
        "{names:?}"
    );
    assert!(
        e.msg.contains(&names[0]),
        "names the staging folder: {}",
        e.msg
    );
    assert!(e.msg.contains("the study was not registered"), "{}", e.msg);
    assert!(!f.home.study_dir("my-study").exists());
    assert!(leaves(&f.home.root.join("planlog")).is_empty());
}

#[test]
fn a_payload_edited_at_the_prompt_is_refused_and_the_id_stays_free() {
    let (f, spec) = setup("numeric-ablation", same);
    let run = spec.parent().unwrap().join("payload/run");
    let shown = common::PAYLOAD.to_string();
    let (r, out) = at_prompt(&f, &spec, || {
        common::write_exec(&run, &format!("{shown}# edited\n"))
    });
    assert!(out.contains(PROMPT), "{out}");
    let e = r.unwrap_err();
    assert_eq!(e.code, 1);
    assert!(
        e.msg.contains("the payload changed after the check"),
        "{}",
        e.msg
    );
    only_staging_left(&f, &e);
    // The id is free: with the payload back, the same plan registers.
    common::write_exec(&run, &shown);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert_eq!(leaves(&f.home.root.join("planlog")).len(), 1);
}

#[test]
fn a_fault_before_publish_leaves_only_the_staging_folder() {
    use std::os::unix::fs::PermissionsExt;
    let (f, spec) = setup("numeric-ablation", same);
    let helper = spec.parent().unwrap().join("payload/helper.sh");
    std::fs::write(&helper, "echo helper\n").unwrap();
    // The copy cannot read the helper: an I/O fault while the study is staged.
    let locked = helper.clone();
    let (r, _) = at_prompt(&f, &spec, move || {
        std::fs::set_permissions(&locked, PermissionsExt::from_mode(0o000)).unwrap()
    });
    let e = r.unwrap_err();
    assert_eq!(e.code, 2, "{}", e.msg);
    only_staging_left(&f, &e);
    std::fs::set_permissions(&helper, PermissionsExt::from_mode(0o644)).unwrap();
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
}

#[test]
fn a_study_folder_made_during_the_prompt_is_refused_at_publish() {
    type Make = fn(&Path);
    let cases: [(&str, Make); 2] = [
        ("a folder with a file", |d| {
            std::fs::create_dir(d).unwrap();
            std::fs::write(d.join("x"), "").unwrap();
        }),
        ("a file", |d| std::fs::write(d, "").unwrap()),
    ];
    for (name, make) in cases {
        let (f, spec) = setup("numeric-ablation", same);
        let dir = f.home.study_dir("my-study");
        let (r, _) = at_prompt(&f, &spec, || make(&dir));
        let e = r.unwrap_err();
        assert_eq!(e.code, 1, "{name}");
        assert!(e.msg.starts_with("E-REG-002: "), "{name}: {}", e.msg);
        // No plan leaf: the rename is the claim, and it was lost.
        assert!(leaves(&f.home.root.join("planlog")).is_empty(), "{name}");
        assert!(!dir.join("records.jsonl").exists(), "{name}");
    }
    // An empty folder there holds nothing, so the publish replaces it.
    let (f, spec) = setup("numeric-ablation", same);
    let dir = f.home.study_dir("my-study");
    let (r, out) = at_prompt(&f, &spec, || std::fs::create_dir(&dir).unwrap());
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(dir.join("records.jsonl").is_file());
    assert_eq!(studies(&f), ["my-study"]);
}

#[test]
fn a_store_fault_at_register_is_an_io_error_not_a_changed_input() {
    let (f, spec) = setup("numeric-ablation", same);
    // The import log cannot be appended: it is a folder.
    std::fs::create_dir_all(f.home.root.join("store/imports.jsonl")).unwrap();
    let (r, _) = go(&f, &spec, &answer(&spec), true);
    let e = r.unwrap_err();
    assert_eq!(e.code, 2, "{}", e.msg);
    assert!(e.msg.starts_with("store: "), "{}", e.msg);
    assert!(e.msg.contains("imports.jsonl"), "{}", e.msg);
    assert!(!e.msg.contains("changed"), "{}", e.msg);
    assert!(!f.home.study_dir("my-study").exists());
}

#[test]
fn the_copy_modes_do_not_depend_on_the_umask() {
    // Run the copy test again in a child whose umask takes every group and other bit.
    let name = "register_copies_the_payload_folder_and_records_its_hash";
    let out = std::process::Command::new("sh")
        .args(["-c", "umask 077 && exec \"$0\" \"$@\""])
        .arg(std::env::current_exe().unwrap())
        .args([name, "--exact"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains(" 1 passed"), "{text}");
}

#[test]
fn an_input_changed_after_the_check_is_refused_and_the_study_is_not_registered() {
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
        assert!(
            e.msg.ends_with("; the study was not registered"),
            "{name}: {}",
            e.msg
        );
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
    let (f, spec) = setup("numeric-ablation", |t| t.replace("power = 0.8\n", ""));
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
    common::pilot(&f.home, vec![common::arm_a()]);
    let edited = common::confirmatory(std::fs::read_to_string(&spec).unwrap());
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

/// The first slot's level: 0.05 x 1/10, as f64 arithmetic gives it.
const FIRST: f64 = 0.005000000000000001;

fn stream_of(f: &Fixture, log: &str) -> Vec<Value> {
    records(&f.home.root.join(format!("stream/{log}.jsonl")))
        .into_iter()
        .map(|l| l["record"].clone())
        .collect()
}

#[test]
fn check_shows_the_level_and_register_spends_it() {
    let (f, spec) = setup("numeric-ablation", common::confirmatory);
    common::pilot(&f.home, vec![common::arm_a()]);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    // The person approves the level with the plan; register spends that slot.
    let shown = out.find("\nstream: slot 1, level 0.005\n").expect(&out);
    assert!(shown < out.find(PROMPT).unwrap(), "{out}");
    assert!(
        out.ends_with("registered my-study: leaf 0, stream slot 1, level 0.005\n"),
        "{out}"
    );
    assert!(!out.contains("level rose"), "{out}");
    let stream = stream_of(&f, "planlog");
    let kinds: Vec<&Value> = stream.iter().map(|r| &r["kind"]).collect();
    assert_eq!(kinds, [&json!("stream.config"), &json!("stream.open")]);
    assert_eq!(
        stream[1]["body"],
        json!({ "log": "planlog", "study": "my-study", "slot": 1, "level": FIRST })
    );
    let body = &records(&f.home.study_dir("my-study").join("records.jsonl"))[0]["record"]["body"];
    assert_eq!(
        body["alpha"],
        json!({ "log": "planlog", "slot": 1, "level": FIRST })
    );
    // The next plan would get slot 2.
    let key = keys::load(&f.home).unwrap().unwrap();
    let next = Log::of(&f.home, LogName::Plan)
        .next(&key.verifying_key())
        .unwrap();
    assert_eq!((next.slot, next.level), (2, FIRST));
    // An exploratory plan takes no slot.
    let (f, spec) = setup("numeric-ablation", same);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(!out.contains("stream"), "{out}");
    let body = &records(&f.home.study_dir("my-study").join("records.jsonl"))[0]["record"]["body"];
    assert_eq!(body["alpha"], Value::Null);
    assert!(stream_of(&f, "planlog").is_empty());
}

#[test]
fn a_test_study_spends_a_slot_of_the_test_stream() {
    let (f, spec) = setup("numeric-ablation", |t| {
        common::confirmatory(t).replace("test_log = false", "test_log = true")
    });
    common::pilot(&f.home, vec![common::arm_a()]);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    let rec = &records(&f.home.study_dir("my-study").join("records.jsonl"))[0]["record"];
    assert_eq!(rec["body"]["log"], "testlog");
    assert_eq!(
        rec["body"]["alpha"],
        json!({ "log": "testlog", "slot": 1, "level": FIRST })
    );
    assert_eq!(stream_of(&f, "testlog").len(), 2);
    assert!(stream_of(&f, "planlog").is_empty());
}

#[test]
fn a_slot_lost_at_the_prompt_is_refused_before_the_plan_leaf() {
    let (f, spec) = setup("numeric-ablation", common::confirmatory);
    common::pilot(&f.home, vec![common::arm_a()]);
    let key = keys::load_or_create(&f.home).unwrap();
    let log = Log::of(&f.home, LogName::Plan);
    for i in 1..=9 {
        log.open(&key, &format!("s{i}"), 0.0).unwrap();
    }
    // The check shows slot 10; another study takes it while the person reads the prompt.
    let (r, out) = at_prompt(&f, &spec, || {
        log.open(&key, "rival", 0.0).unwrap();
    });
    assert!(out.contains("\nstream: slot 10, level 0.005\n"), "{out}");
    let e = r.unwrap_err();
    assert_eq!(e.code, 1, "{}", e.msg);
    assert!(e.msg.starts_with("E-STR-001: "), "{}", e.msg);
    assert!(
        e.msg
            .contains(" Found: slot 11 has level 0, below the 0.005 the check showed."),
        "{}",
        e.msg
    );
    // No slot for it and no plan leaf; the study folder went back to staging, so the id is
    // free again.
    taken_back(&f, &e);
    let stream = stream_of(&f, "planlog");
    assert_eq!(stream.len(), 11);
    assert!(stream.iter().all(|r| r["study"] != "my-study"));
}

/// The published study folder went back to its one staging folder, which the error names;
/// there is no `my-study` and no plan leaf.
fn taken_back(f: &Fixture, e: &CliError) {
    let staged: Vec<String> = (studies(f).into_iter())
        .filter(|n| n.starts_with(".my-study.") && n.ends_with(".tmp"))
        .collect();
    assert_eq!(staged.len(), 1, "{:?}", studies(f));
    assert!(e.msg.contains(&staged[0]), "{}", e.msg);
    assert!(e.msg.contains("the study was not registered"), "{}", e.msg);
    assert!(!f.home.study_dir("my-study").exists());
    assert!(leaves(&f.home.root.join("planlog")).is_empty());
}

#[test]
fn a_stream_fault_after_the_publish_takes_the_folder_back() {
    let (f, spec) = setup("numeric-ablation", common::confirmatory);
    common::pilot(&f.home, vec![common::arm_a()]);
    // The stream log cannot be opened for append: a folder stands at its path.
    let stream = f.home.root.join("stream/planlog.jsonl");
    let (r, _) = at_prompt(&f, &spec, || std::fs::create_dir(&stream).unwrap());
    let e = r.unwrap_err();
    assert_eq!(e.code, 2, "{}", e.msg);
    assert!(e.msg.starts_with("stream: "), "{}", e.msg);
    taken_back(&f, &e);
    // With the folder moved aside, the same plan registers.
    std::fs::rename(&stream, f.tmp.path().join("moved")).unwrap();
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert_eq!(leaves(&f.home.root.join("planlog")).len(), 1);
}

#[test]
fn a_level_that_rose_since_the_check_is_spent_and_said() {
    let (f, spec) = setup("numeric-ablation", common::confirmatory);
    common::pilot(&f.home, vec![common::arm_a()]);
    let key = keys::load_or_create(&f.home).unwrap();
    let log = Log::of(&f.home, LogName::Plan);
    log.open(&key, "other", 0.0).unwrap();
    // The check shows slot 2 at 0.005; slot 1 closes as a discovery at the prompt.
    let (r, out) = at_prompt(&f, &spec, || {
        assert!(log.close(&key, "other", 1, 200.0).unwrap());
    });
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(out.contains("\nstream: slot 2, level 0.005\n"), "{out}");
    assert!(
        out.ends_with(
            "level rose from 0.005 to 0.01 since the check\nregistered my-study: leaf 0, stream slot 2, level 0.01\n"
        ),
        "{out}"
    );
    let body = &records(&f.home.study_dir("my-study").join("records.jsonl"))[0]["record"]["body"];
    assert_eq!(
        body["alpha"],
        json!({ "log": "planlog", "slot": 2, "level": 2.0 * FIRST })
    );
}

#[test]
fn a_refusal_names_each_study_once() {
    // Three hypotheses: two wait on my-pilot, one on later.
    let more = |id: &str, from: &str| {
        format!(
            "[[question.hypotheses]]
id = \"{id}\"
statement = \"\"
rival = \"\"
outcome = \"ppl-ratio\"
contrast = {{ arm-a = 1.0 }}
claim = {{ below = 0.0 }}
statistic = {{ quantile = 0.5 }}
power_at = -1.0
estimates = {{ from = \"{from}\" }}
alpha_share = 0.3

"
        )
    };
    let (f, spec) = setup("numeric-ablation", |t| {
        common::confirmatory(t)
            .replacen("alpha_share = 1.0", "alpha_share = 0.4", 1)
            .replacen(
                "[[outcomes]]",
                &format!(
                    "{}{}[[outcomes]]",
                    more("H2", "my-pilot"),
                    more("H3", "later")
                ),
                1,
            )
    });
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(out.matches("warning[W-REF-001]").count(), 3, "{out}");
    assert_eq!(
        r.unwrap_err().msg,
        "W-REF-001: a value waits on a study that has not finished; register after my-pilot finishes; register after later finishes. See docs/rules/W-REF-001.md"
    );
}

#[test]
fn register_refuses_pending_values() {
    let (f, spec) = setup("numeric-ablation", common::confirmatory);
    // Not registered, then registered and not finished: no analysis record yet.
    let unfinished = common::finished(vec![common::arm_a()]);
    for planted in [&[][..], &unfinished[..1]] {
        common::plant(&f.home, "my-pilot", planted);
        let (r, out) = go(&f, &spec, &answer(&spec), true);
        let e = r.unwrap_err();
        assert_eq!(e.code, 1, "{out}");
        assert_eq!(
            e.msg,
            "W-REF-001: a value waits on a study that has not finished; register after my-pilot finishes. See docs/rules/W-REF-001.md"
        );
        assert!(out.contains("warning[W-REF-001]"), "{out}");
        // The refusal comes before the prompt, and nothing is written.
        assert!(!out.contains(PROMPT), "{out}");
        assert!(!f.home.study_dir("my-study").exists());
        assert!(leaves(&f.home.root.join("planlog")).is_empty());
    }
}

#[test]
fn register_freezes_the_estimate_records_it_used() {
    // Two more strata, each with the 15 clusters the power gate needs.
    let (f, spec) = setup("numeric-ablation", |t| {
        common::confirmatory(t)
            .replace(
                "strata = []",
                "strata = [\"fiction\", \"code\", \"article\"]",
            )
            .replacen(
                "[pins]",
                &format!(
                    "{}{}[pins]",
                    common::clusters("code", 15),
                    common::clusters("article", 15)
                ),
                1,
            )
    });
    let other = common::estimate(&[("arm-b", 1.0, serde_json::json!({}))], "", 1.0, Some(0.0));
    let fiction = common::estimate(
        &[("arm-a", 1.0, serde_json::json!({}))],
        "fiction",
        9.0,
        Some(0.2),
    );
    let hashes = common::pilot(&f.home, vec![common::arm_a(), other, fiction]);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    // The person approves the values with the plan: they are shown before the prompt.
    let shown = "takes question.hypotheses[0].estimates from my-pilot: \
                 stratum article sd 2, icc 0.1 (the pooled estimate); \
                 stratum code sd 2, icc 0.1 (the pooled estimate); stratum fiction sd 3, icc 0.2\n";
    let at = out.find(shown).unwrap_or_else(|| panic!("{out}"));
    assert!(at < out.find(PROMPT).unwrap(), "{out}");
    // A stratum's own estimate, else the pooled one, never another contrast's; each record
    // is named once.
    assert_eq!(
        body_of(&f)["resolved"],
        serde_json::json!([{
            "path": "question.hypotheses[0].estimates",
            "study": "my-pilot",
            "records": [hashes[2], hashes[0]],
            "value": {
                "fiction": { "sd": 3.0, "icc": 0.2 },
                "code": { "sd": 2.0, "icc": 0.1, "pooled": true },
                "article": { "sd": 2.0, "icc": 0.1, "pooled": true }
            }
        }])
    );
    // An exploratory plan takes nothing from another study, claim or not.
    let (g, spec) = setup("numeric-ablation", common::with_claim);
    let (r, out) = go(&g, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert_eq!(body_of(&g)["resolved"], serde_json::json!([]));
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

fn body_of(f: &Fixture) -> Value {
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    records(&rp)[0]["record"]["body"].clone()
}

#[cfg(unix)]
fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    p.symlink_metadata().unwrap().permissions().mode() & 0o777
}

#[test]
fn register_copies_the_payload_folder_and_records_its_hash() {
    let (f, spec) = setup("numeric-ablation", same);
    let work = spec.parent().unwrap().join("payload");
    std::fs::create_dir(work.join("lib")).unwrap();
    std::fs::write(work.join("lib/util.sh"), "echo util\n").unwrap();
    let hash = ignorarium::payload::tree_hash(&work).unwrap();
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(out.contains(&format!("payload sha256:{hash}")), "{out}");
    let copy = f.home.study_dir("my-study").join("payload");
    for rel in ["run", "lib/util.sh"] {
        assert_eq!(
            std::fs::read(copy.join(rel)).unwrap(),
            std::fs::read(work.join(rel)).unwrap(),
            "{rel}"
        );
    }
    assert_eq!(ignorarium::payload::tree_hash(&copy).unwrap(), hash);
    // The copy is read-only: an executable source stays executable, folders take no entry.
    assert_eq!(mode_of(&copy.join("run")), 0o555);
    assert_eq!(mode_of(&copy.join("lib/util.sh")), 0o444);
    assert_eq!(mode_of(&copy), 0o555);
    assert_eq!(mode_of(&copy.join("lib")), 0o555);
    let body = body_of(&f);
    assert_eq!(body["payload"], hash.as_str());
    assert_eq!(
        body["inputs"],
        serde_json::json!([["data/example.txt", sha256_hex(b"Hello  World")]])
    );
}

#[test]
fn register_refuses_without_a_payload_folder() {
    use std::os::unix::fs::PermissionsExt;
    type Edit = fn(&Path);
    let cases: [(&str, Edit, &str); 5] = [
        (
            "no folder",
            |w| std::fs::rename(w.join("payload"), w.join("payload.off")).unwrap(),
            "Found: the study root has no payload folder.",
        ),
        (
            "a file named payload",
            |w| {
                std::fs::rename(w.join("payload"), w.join("payload.off")).unwrap();
                std::fs::write(w.join("payload"), "").unwrap();
            },
            "Found: payload is a file, not a folder.",
        ),
        (
            "a run file that is not executable",
            |w| {
                let p = PermissionsExt::from_mode(0o644);
                std::fs::set_permissions(w.join("payload/run"), p).unwrap();
            },
            "Found: payload/run is not executable.",
        ),
        (
            "no run file",
            |w| std::fs::rename(w.join("payload/run"), w.join("payload/start")).unwrap(),
            "Found: the payload folder has no run file.",
        ),
        (
            "a link",
            |w| std::os::unix::fs::symlink("run", w.join("payload/again")).unwrap(),
            "payload/again is a link or not a regular file.",
        ),
    ];
    for (name, edit, found) in cases {
        let (f, spec) = setup("numeric-ablation", same);
        edit(spec.parent().unwrap());
        let (r, out) = go(&f, &spec, &answer(&spec), true);
        let e = r.unwrap_err();
        assert_eq!(e.code, 1, "{name}");
        assert!(e.msg.starts_with("E-REG-003: "), "{name}: {}", e.msg);
        assert!(e.msg.contains(found), "{name}: {}", e.msg);
        assert!(
            e.msg.ends_with(" See docs/rules/E-REG-003.md"),
            "{name}: {}",
            e.msg
        );
        // Refused before the approval: no prompt, no study folder, no leaf, no import.
        assert!(!out.contains(PROMPT), "{name}: {out}");
        assert!(!f.home.study_dir("my-study").exists(), "{name}");
        assert!(leaves(&f.home.root.join("planlog")).is_empty(), "{name}");
        assert!(!f.home.root.join("store/imports.jsonl").exists(), "{name}");
    }
}

#[test]
fn register_refuses_a_pin_that_is_not_the_folder_hash() {
    let (f, spec) = setup("numeric-ablation", same);
    let work = spec.parent().unwrap().join("payload");
    let folder = ignorarium::payload::tree_hash(&work).unwrap();
    // The hash of payload/run alone is not the pin.
    let alone = sha256_hex(common::PAYLOAD.as_bytes());
    let pinned = |pin: &str| {
        let t = std::fs::read_to_string(&spec).unwrap();
        let t = t.replace("payload = \"\"", &format!("payload = \"{pin}\""));
        std::fs::write(&spec, t).unwrap();
    };
    pinned(&alone);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    let e = r.unwrap_err();
    assert_eq!(e.code, 1);
    assert!(e.msg.contains("E-REG-003"), "{}", e.msg);
    assert!(e.msg.contains(&folder), "names the folder hash: {}", e.msg);
    assert!(!out.contains(PROMPT), "{out}");
    assert!(!f.home.study_dir("my-study").exists());
    // The folder hash, bare or with its prefix, is the pin.
    for (id, pin) in [
        ("bare", folder.clone()),
        ("prefixed", format!("sha256:{folder}")),
    ] {
        let (f, spec) = setup("numeric-ablation", |t| {
            t.replace("id = \"my-study\"", &format!("id = \"{id}\""))
                .replace("payload = \"\"", &format!("payload = \"{pin}\""))
        });
        let (r, out) = go(&f, &spec, &answer(&spec), true);
        assert_eq!(r.unwrap().code, 0, "{id}: {out}");
        let rp = f.home.study_dir(id).join("records.jsonl");
        assert_eq!(
            records(&rp)[0]["record"]["body"]["payload"],
            folder.as_str()
        );
    }
}

#[test]
fn register_imports_root_inputs_into_the_store() {
    let (f, spec) = setup("numeric-ablation", same);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    let digest = sha256_hex(b"Hello  World");
    let store = ignorarium::store::Store::open(&f.home.root).unwrap();
    assert_eq!(
        std::fs::read(store.path(&digest).unwrap()).unwrap(),
        b"Hello  World"
    );
    let meta = store.meta(&digest).unwrap().unwrap();
    assert_eq!(meta.size, 12);
    assert_eq!(meta.normalized_sha256, sha256_hex(b"hello world"));
    // One signed import record names the study and the file name.
    let key = keys::load_or_create(&f.home).unwrap();
    let log = f.home.root.join("store/imports.jsonl");
    assert_eq!(
        ignorarium::records::verify(&log, &key.verifying_key()).unwrap(),
        1
    );
    let rec = &records(&log)[0]["record"];
    assert_eq!(rec["kind"], "import");
    assert_eq!(rec["body"]["digest"], digest.as_str());
    assert_eq!(rec["body"]["source"], "register my-study");
    assert_eq!(rec["body"]["file"], "example.txt");
}

#[test]
fn a_stored_input_whose_blob_changed_after_the_check_is_refused() {
    let (f, spec) = setup("numeric-ablation", same);
    // The input lives only in the store, pinned by digest.
    let data = spec.parent().unwrap().join("data/example.txt");
    let store = ignorarium::store::Store::open(&f.home.root).unwrap();
    let (digest, _) = store.import(&data).unwrap();
    std::fs::remove_file(&data).unwrap();
    let t = std::fs::read_to_string(&spec).unwrap();
    let pin = format!("sha256 = \"{digest}\"");
    std::fs::write(&spec, t.replacen("sha256 = \"\"", &pin, 1)).unwrap();
    // Same size, other bytes, after the check: only a full hash sees it.
    let blob = store.path(&digest).unwrap();
    let mut input = Hooked {
        inner: Cursor::new(answer(&spec).into_bytes()),
        hook: Some(move || {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o644)).unwrap();
            std::fs::write(&blob, "Hello  Worle").unwrap();
        }),
    };
    let mut out = Vec::new();
    let e = register::run(&spec, &f.home, &mut input, &mut out, true).unwrap_err();
    assert_eq!(e.code, 1);
    assert!(e.msg.contains("example.txt"), "{}", e.msg);
    assert!(e.msg.contains("changed"), "{}", e.msg);
    assert!(!f.home.study_dir("my-study").exists());
}

/// The template with arm-a's `bits` taken from study `my-chooser`.
fn taking(t: String) -> String {
    t.replace(
        "# params = { bits = 4 }",
        "params = { bits = { from = \"my-chooser\" } }  #",
    )
}

#[test]
fn register_freezes_the_selected_condition() {
    let (f, spec) = setup("numeric-ablation", taking);
    let planted = common::plant(
        &f.home,
        "my-chooser",
        &common::finished_selecting("arm-b", vec![]),
    );
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 0, "{out}");
    assert!(
        out.contains(
            "takes design.conditions[1].params.bits from my-chooser: condition arm-b
"
        ),
        "{out}"
    );
    let rp = f.home.study_dir("my-study").join("records.jsonl");
    // The selection record is the second record of the chooser: after its register record.
    assert_eq!(
        records(&rp)[0]["record"]["body"]["resolved"],
        serde_json::json!([{
            "path": "design.conditions[1].params.bits",
            "study": "my-chooser",
            "records": [planted[1]],
            "value": "arm-b",
        }])
    );
}

#[test]
fn register_refuses_a_param_that_waits_or_has_no_selection() {
    let (f, spec) = setup("numeric-ablation", taking);
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(
        r.unwrap_err().msg,
        "W-REF-001: a value waits on a study that has not finished; register after my-chooser finishes. See docs/rules/W-REF-001.md"
    );
    assert!(out.contains("warning[W-REF-001]"), "{out}");
    common::plant(&f.home, "my-chooser", &common::finished(vec![]));
    let (r, out) = go(&f, &spec, &answer(&spec), true);
    assert_eq!(r.unwrap().code, 1, "{out}");
    assert!(out.contains("error[E-REF-002]"), "{out}");
    assert!(!out.contains(PROMPT), "{out}");
    assert!(!f.home.study_dir("my-study").exists());
}
