//! Live test against the Zenodo sandbox. Run it with `-- --ignored`.
//! `IGNORARIUM_ZENODO_STAGE_TOKEN_FILE` and `IGNORARIUM_ZENODO_PUBLISH_TOKEN_FILE`
//! name files that each hold one token. The test never prints a token.
use bundle::Facts;
use bundle::citation::{Release, Tool};
use bundle::licence::{Input, Kind, Licensing, Record};
use bundle::manifest::sha256_bytes;
use bundle::rocrate::{Image, Item, Run, Status};
use deposit::Secret;
use deposit::http::{Body, Curl, Method, Request, Transport};
use deposit::zenodo::{PublishToken, StageToken, Step, Zenodo};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const INPUT: &str = "data/texts/a.txt";

fn token(var: &str) -> Secret {
    let file = std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set"));
    let text = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{var}: cannot read: {e}"));
    let t = text.trim();
    assert!(!t.is_empty(), "{var}: the token file is empty");
    Secret::new(t)
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../bundle/tests/fixtures/studies/numeric")
}

fn facts() -> Facts {
    Facts {
        study_id: "fixture-numeric".into(),
        label: "signal".into(),
        licensing: Licensing {
            code: "Apache-2.0".into(),
            data: "CC0-1.0".into(),
            inputs: vec![Input {
                path: INPUT.into(),
                sha256: sha256_bytes(b"Fixture text A, written for the bundle tests.\n"),
                licence: Record {
                    spdx: "CC-BY-SA-4.0".into(),
                    kind: Kind::Common,
                    source: "https://example.org/texts/a".into(),
                    creators: vec!["Example Creator".into()],
                },
            }],
        },
        tool: Tool {
            name: "ignorarium".into(),
            version: "0.0.1".into(),
            url: "https://github.com/Phlares/ignoramus".into(),
            author: "The ignorarium contributors".into(),
        },
        runs: vec![Run {
            id: "row-1".into(),
            name: "Row 1".into(),
            start: "2026-10-29T10:00:00Z".into(),
            end: "2026-10-29T10:01:00Z".into(),
            status: Status::Completed,
            object: vec![Item::Path("spec.toml".into()), Item::Path(INPUT.into())],
            result: vec![Item::Path("results/summary.json".into())],
            image: Some(Image {
                registry: "docker.io".into(),
                name: "library/python".into(),
                tag: "3.12".into(),
                sha256: "1".repeat(64),
            }),
            env: vec![("OMP_NUM_THREADS".into(), "1".into())],
        }],
        attachments: vec![(
            "records/planlog-head.json".into(),
            b"{\"size\": 1}\n".to_vec(),
        )],
        artifacts: vec![],
    }
}

fn record(log: &Path, step: &Step) {
    let line = serde_json::to_string(step).expect("a step serializes");
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .expect("the steps file opens");
    writeln!(f, "{line}").expect("the steps file takes a line");
}

#[test]
#[ignore = "needs Zenodo sandbox tokens"]
fn sandbox_deposit_end_to_end() {
    let stage = StageToken(token("IGNORARIUM_ZENODO_STAGE_TOKEN_FILE"));
    let publish = PublishToken(token("IGNORARIUM_ZENODO_PUBLISH_TOKEN_FILE"));
    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let log = work.join("sandbox-steps.jsonl");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos();
    let run_dir = work.join(format!("sandbox-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&run_dir).unwrap();
    println!("steps: {}", log.display());

    let z = Zenodo::sandbox(Curl::new());
    let (draft, s) = z.create(&stage).expect("create");
    record(&log, &s);

    let release = Release {
        version: "1.0.0".into(),
        date: "2026-10-29".into(),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        doi: Some(draft.doi.clone()),
        publisher: "Zenodo".into(),
    };
    let built = bundle::build(&fixture(), &run_dir.join("bundle"), &facts(), &release)
        .unwrap_or_else(|e| panic!("build: {e:?}"));
    let name = bundle::zip_name("fixture-numeric", "1.0.0");
    let zip = run_dir.join(&name);
    let zip_sha = bundle::zip::pack(&built.dir, &zip).expect("pack");

    let datacite: serde_json::Value =
        serde_json::from_slice(&fs::read(built.dir.join("datacite.json")).unwrap()).unwrap();
    record(
        &log,
        &z.describe(&stage, &draft, &datacite, &built.manifest_sha256)
            .expect("describe"),
    );
    record(
        &log,
        &z.upload(&stage, &draft, &name, &zip).expect("upload"),
    );
    record(
        &log,
        &z.verify(&stage, &draft, &[(name.clone(), zip_sha.clone())])
            .expect("verify"),
    );

    // Zenodo does not enforce deposit:write against publishing, so the split is
    // enforced by the client's types, not by the server.
    record(&log, &z.publish(&publish, &draft).expect("publish"));

    let url = format!(
        "https://sandbox.zenodo.org/records/{}/files/{name}?download=1",
        draft.recid
    );
    println!("record: https://sandbox.zenodo.org/records/{}", draft.recid);
    let r = Curl::new()
        .send(&Request {
            method: Method::Get,
            url,
            token: None,
            body: Body::None,
            keep_body: false,
            follow: true,
        })
        .expect("public download");
    assert_eq!(r.status, 200);
    assert_eq!(r.sha256, zip_sha);
}
