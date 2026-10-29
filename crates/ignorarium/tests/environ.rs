mod common;
use ignorarium::environ::{
    Builder, EnvError, Recipe, build, check_pinned, rebuild_rate, recipe_hash,
};
use ignorarium::store::Store;
use std::path::{Path, PathBuf};

const D: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn recipe(d: &str, l: &str) -> Recipe {
    Recipe {
        dockerfile: d.into(),
        lockfile: l.into(),
    }
}

#[test]
fn recipe_hash_matches_the_known_vector() {
    let r = recipe("FROM x@sha256:ab\n", "a=1\n");
    assert_eq!(
        recipe_hash(&r),
        "7e60c89dec401837e1401afc38b10ad252486088efa7f264526baf697dc2ec5d"
    );
    assert_eq!(recipe_hash(&r), recipe_hash(&r.clone()));
}

#[test]
fn recipe_hash_changes_with_each_file() {
    let base = recipe_hash(&recipe("a", "b"));
    assert_ne!(base, recipe_hash(&recipe("a2", "b")));
    assert_ne!(base, recipe_hash(&recipe("a", "b2")));
    assert_ne!(base, recipe_hash(&recipe("b", "a")));
}

#[test]
fn check_pinned_rejects_an_untagged_base() {
    assert!(!check_pinned("FROM debian:bookworm\n"));
    assert!(!check_pinned("FROM debian\n"));
    assert!(check_pinned(&format!("FROM debian@sha256:{D}\n")));
    assert!(check_pinned(&format!("FROM debian:12@sha256:{D}\n")));
}

#[test]
fn check_pinned_needs_a_from_and_a_full_digest() {
    assert!(!check_pinned(""));
    assert!(!check_pinned("RUN echo hi\n"));
    assert!(!check_pinned("FROM\n"));
    assert!(!check_pinned("FROM debian@sha256:abc\n"));
    assert!(!check_pinned(&format!(
        "FROM debian@sha256:{}\n",
        D.to_uppercase()
    )));
    assert!(!check_pinned(&format!("FROM debian@sha256:{D}0\n")));
    assert!(!check_pinned(&format!("FROM debian@sha256:{}\n", &D[..63])));
}

#[test]
fn check_pinned_covers_every_from() {
    assert!(check_pinned(&format!(
        "FROM a@sha256:{D}\nFROM b@sha256:{D}\n"
    )));
    assert!(!check_pinned(&format!("FROM a@sha256:{D}\nFROM b:1\n")));
}

#[test]
fn check_pinned_handles_options_stages_case_and_variables() {
    assert!(check_pinned(&format!(
        "FROM --platform=linux/amd64 a@sha256:{D}\n"
    )));
    assert!(check_pinned(&format!(
        "from a@sha256:{D} AS Build\nFROM build AS final\nFROM final\n"
    )));
    assert!(!check_pinned(&format!(
        "FROM later\nFROM a@sha256:{D} AS later\n"
    )));
    assert!(!check_pinned(&format!("FROM $BASE@sha256:{D}\n")));
    assert!(!check_pinned("FROM ${BASE}\n"));
    assert!(check_pinned(&format!("FROM \\\n  a@sha256:{D}\n")));
    assert!(check_pinned(&format!("# FROM x\nFROM a@sha256:{D}\n")));
}

#[test]
fn the_fixture_dockerfile_is_pinned() {
    let d = std::fs::read_to_string("tests/fixtures/env/Dockerfile").unwrap();
    assert!(check_pinned(&d));
}

// Build and rebuild_rate run against a fake engine, a shell script.

/// `build` writes the next id from `ids` (cycling) to the --iidfile and logs
/// its arguments. `save` prints a tar marker that names the id.
fn fake(dir: &Path, ids: &[&str]) -> (Builder, PathBuf) {
    let log = dir.join("calls.log");
    let script = format!(
        "#!/bin/sh\n\
         echo \"$@\" >> '{log}'\n\
         case \"$1\" in\n\
         build)\n\
           iid=\"\"; prev=\"\"\n\
           for a in \"$@\"; do [ \"$prev\" = \"--iidfile\" ] && iid=\"$a\"; prev=\"$a\"; done\n\
           n=$(cat '{cnt}' 2>/dev/null || echo 0)\n\
           set -- {list}\n\
           i=$((n % $#)); shift $i\n\
           printf 'sha256:%s' \"$1\" > \"$iid\"\n\
           echo $((n + 1)) > '{cnt}'\n\
           ;;\n\
         save) printf 'TAR:%s' \"$2\" ;;\n\
         esac\n",
        log = log.display(),
        cnt = dir.join("count").display(),
        list = ids.join(" "),
    );
    let p = dir.join("engine");
    common::write_exec(&p, &script);
    (Builder::with_program(&p), log)
}

fn recipe_dir(root: &Path) -> PathBuf {
    let d = root.join("recipe");
    std::fs::create_dir(&d).unwrap();
    std::fs::copy("tests/fixtures/env/Dockerfile", d.join("Dockerfile")).unwrap();
    std::fs::copy("tests/fixtures/env/uv.lock", d.join("uv.lock")).unwrap();
    d
}

fn store_in(root: &Path) -> Store {
    Store::open(&root.join("home")).unwrap()
}

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn build_stores_the_saved_image_with_the_recipe_identity() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, log) = fake(t.path(), &[A]);
    let store = Store::open(&t.path().join("home")).unwrap();
    let built = build(&b, &dir, &store).unwrap();
    let r = Recipe::load(&dir).unwrap();
    assert_eq!(built.recipe_hash, recipe_hash(&r));
    assert_eq!(built.image_digest, A);
    assert!(built.pinned);
    assert!(store.has(&built.store_digest));
    let bytes = std::fs::read(store.path(&built.store_digest).unwrap()).unwrap();
    assert_eq!(bytes, format!("TAR:sha256:{A}").into_bytes());
    assert_eq!(built.store_digest, ignorarium::canon::sha256_hex(&bytes));
    let calls = std::fs::read_to_string(log).unwrap();
    assert!(calls.contains("save sha256:"), "{calls}");
    assert!(!calls.contains("--no-cache"), "{calls}");
}

#[test]
fn build_reports_an_unpinned_base() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    std::fs::write(dir.join("Dockerfile"), "FROM debian:12\n").unwrap();
    let (b, _) = fake(t.path(), &[A]);
    let store = Store::open(&t.path().join("home")).unwrap();
    assert!(!build(&b, &dir, &store).unwrap().pinned);
}

#[test]
fn build_fails_on_a_bad_engine_or_a_missing_recipe() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let store = Store::open(&t.path().join("home")).unwrap();
    let missing = Builder::with_program(t.path().join("no-such-engine"));
    assert!(matches!(
        build(&missing, &dir, &store),
        Err(EnvError::NoBuilder)
    ));
    let (b, _) = fake(t.path(), &["not-a-digest"]);
    assert!(matches!(build(&b, &dir, &store), Err(EnvError::Build(_))));
    let empty = t.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(matches!(build(&b, &empty, &store), Err(EnvError::Io(_))));
}

#[test]
fn rebuild_rate_is_the_fraction_equal_to_the_first() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, log) = fake(t.path(), &[A]);
    assert_eq!(rebuild_rate(&b, &dir, &store_in(t.path()), 3).unwrap(), 1.0);
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.matches("--no-cache").count(), 3, "{calls}");
    assert!(!calls.contains("save"));

    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, _) = fake(t.path(), &[A, B]);
    assert_eq!(rebuild_rate(&b, &dir, &store_in(t.path()), 4).unwrap(), 0.5);

    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, _) = fake(t.path(), &[A, B, B]);
    assert!((rebuild_rate(&b, &dir, &store_in(t.path()), 3).unwrap() - 1.0 / 3.0).abs() < 1e-12);
}

#[test]
fn rebuild_rate_of_one_build_is_one_and_zero_builds_is_an_error() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, _) = fake(t.path(), &[A]);
    assert_eq!(rebuild_rate(&b, &dir, &store_in(t.path()), 1).unwrap(), 1.0);
    assert!(matches!(
        rebuild_rate(&b, &dir, &store_in(t.path()), 0),
        Err(EnvError::Build(_))
    ));
}

#[test]
#[ignore = "needs docker/podman on the host and a real base digest in the fixture"]
fn build_stores_the_image_and_reports_a_rebuild_rate() {
    let b = Builder::detect().expect("a container engine on PATH");
    let t = tempfile::tempdir().unwrap();
    let store = Store::open(t.path()).unwrap();
    let dir = Path::new("tests/fixtures/env");
    let built = build(&b, dir, &store).unwrap();
    assert!(store.has(&built.store_digest));
    let rate = rebuild_rate(&b, dir, &store, 2).unwrap();
    assert!((0.0..=1.0).contains(&rate));
    println!("rebuild rate over 2 builds: {rate}");
}

#[test]
fn check_pinned_treats_scratch_as_pinned() {
    assert!(check_pinned(
        "FROM scratch
"
    ));
    assert!(check_pinned(
        "from SCRATCH AS base
"
    ));
    assert!(!check_pinned(
        "FROM scratch AS a
FROM debian:12
"
    ));
}

#[test]
fn the_id_file_lives_under_the_store_root() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let (b, log) = fake(t.path(), &[A]);
    let store = store_in(t.path());
    rebuild_rate(&b, &dir, &store, 1).unwrap();
    let calls = std::fs::read_to_string(log).unwrap();
    let iid = calls
        .split_whitespace()
        .skip_while(|w| *w != "--iidfile")
        .nth(1)
        .unwrap();
    assert!(Path::new(iid).starts_with(&store.root), "{iid}");
    assert!(!Path::new(iid).exists(), "scratch file removed");
}

#[test]
fn an_id_file_that_is_a_link_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let dir = recipe_dir(t.path());
    let real = t.path().join("real");
    std::fs::write(&real, format!("sha256:{A}")).unwrap();
    let p = t.path().join("engine");
    common::write_exec(
        &p,
        &format!(
            "#!/bin/sh
prev=\"\"
for a in \"$@\"; do [ \"$prev\" = \"--iidfile\" ] && ln -s '{}' \"$a\"; prev=\"$a\"; done
",
            real.display()
        ),
    );
    let b = Builder::with_program(&p);
    let r = rebuild_rate(&b, &dir, &store_in(t.path()), 1);
    assert!(matches!(r, Err(EnvError::Io(_))), "{r:?}");
}
