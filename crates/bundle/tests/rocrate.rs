use bundle::citation::{Author, Citation, Release, Tool};
use bundle::licence::{Input, Kind, Licensing, Record};
use bundle::manifest::{Entry, sha256_bytes};
use bundle::rocrate::{Artifact, Crate, Image, Item, Run, Status, metadata, missing, run_uuid};
use serde_json::{Value, json};

const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const H3: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const ORCID: &str = "https://orcid.org/0000-0002-1825-0097";
const DOI_URL: &str = "https://doi.org/10.5281/zenodo.1";

struct Fixture {
    citation: Citation,
    release: Release,
    tool: Tool,
    licensing: Licensing,
    files: Vec<Entry>,
    runs: Vec<Run>,
    artifacts: Vec<Artifact>,
}

fn author(given: &str, family: &str, orcid: Option<&str>, aff: Option<&str>) -> Author {
    Author {
        family_names: Some(family.into()),
        given_names: Some(given.into()),
        name: None,
        orcid: orcid.map(Into::into),
        affiliation: aff.map(Into::into),
    }
}

fn entry(path: &str, size: u64) -> Entry {
    Entry {
        path: path.into(),
        sha256: H1.into(),
        size,
    }
}

fn image() -> Image {
    Image {
        registry: "docker.io".into(),
        name: "library/python".into(),
        tag: "3.12".into(),
        sha256: H1.into(),
    }
}

fn run(id: &str, status: Status) -> Run {
    Run {
        id: id.into(),
        name: format!("Run {id}"),
        start: "2026-01-01T00:00:00Z".into(),
        end: "2026-01-01T00:01:00Z".into(),
        status,
        object: vec![],
        result: vec![],
        image: None,
        env: vec![],
    }
}

fn hash(name: &str, sha: &str) -> Item {
    Item::Hash {
        name: name.into(),
        sha256: sha.into(),
    }
}

fn fixture() -> Fixture {
    let mut r1 = run("row-1", Status::Completed);
    r1.object = vec![Item::Path("data/in.csv".into()), hash("big.bin", H2)];
    r1.result = vec![Item::Path("results/out.csv".into())];
    r1.image = Some(image());
    r1.env = vec![("LANG".into(), "C".into())];
    let mut r2 = run("row-2", Status::Failed("exit 1".into()));
    r2.object = vec![hash("other-name", H2)];
    r2.image = Some(image());
    r2.env = vec![("LANG".into(), "C".into())];
    Fixture {
        citation: Citation {
            title: "A study".into(),
            message: "Cite it.".into(),
            summary: "What it found.".into(),
            keywords: vec!["alpha".into(), "beta".into()],
            authors: vec![
                author("Josiah", "Carberry", Some(ORCID), Some("Brown University")),
                author("Ada", "Lovelace", None, None),
                Author {
                    family_names: None,
                    given_names: None,
                    name: Some("The Lab".into()),
                    orcid: None,
                    affiliation: None,
                },
            ],
            references: vec![],
        },
        release: Release {
            version: "1.0.0".into(),
            date: "2026-10-29".into(),
            commit: None,
            doi: Some("10.5281/zenodo.1".into()),
            publisher: "Zenodo".into(),
        },
        tool: Tool {
            name: "ignorarium".into(),
            version: "0.1.0".into(),
            url: "https://example.org/tool".into(),
            author: "Tool Authors".into(),
        },
        licensing: Licensing {
            code: "Apache-2.0".into(),
            data: "CC0-1.0".into(),
            inputs: vec![Input {
                path: "data/in.csv".into(),
                sha256: H1.into(),
                licence: Record {
                    spdx: "CC-BY-4.0".into(),
                    kind: Kind::Common,
                    source: "s".into(),
                    creators: vec!["C".into()],
                },
            }],
        },
        files: vec![
            entry("src/run.py", 10),
            entry("data/in.csv", 20),
            entry("results/out.csv", 30),
            entry("LICENSES/CC0-1.0.txt", 40),
        ],
        runs: vec![r1, r2],
        artifacts: vec![Artifact {
            name: "big.bin".into(),
            sha256: H2.into(),
            size: 5_000_000_000,
        }],
    }
}

fn build(f: &Fixture) -> Value {
    metadata(&Crate {
        study_id: "fixture-numeric",
        citation: &f.citation,
        release: &f.release,
        tool: &f.tool,
        licensing: &f.licensing,
        files: &f.files,
        runs: &f.runs,
        artifacts: &f.artifacts,
    })
}

fn entity<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["@id"] == id)
        .unwrap_or_else(|| panic!("no entity {id}"))
}

fn count(v: &Value, id: &str) -> usize {
    v["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["@id"] == id)
        .count()
}

fn ids(v: &Value) -> Vec<&str> {
    v["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["@id"].as_str().unwrap())
        .collect()
}

#[test]
fn run_uuid_vector() {
    assert_eq!(
        run_uuid("fixture-numeric", "row-1"),
        "6b4fc603-1b5d-8ee2-8d11-cd80e72998f4"
    );
}

#[test]
fn descriptor_and_root_meet_the_spec() {
    let mut f = fixture();
    let v = build(&f);
    assert_eq!(
        v["@context"],
        json!([
            "https://w3id.org/ro/crate/1.3/context",
            "https://w3id.org/ro/terms/workflow-run/context"
        ])
    );
    let graph = v["@graph"].as_array().unwrap();
    assert_eq!(
        graph[0],
        json!({
            "@id": "ro-crate-metadata.json",
            "@type": "CreativeWork",
            "about": {"@id": "./"},
            "conformsTo": {"@id": "https://w3id.org/ro/crate/1.3"}
        })
    );
    let root = &graph[1];
    assert_eq!(root["@id"], "./");
    assert_eq!(root["@type"], "Dataset");
    assert_eq!(root["name"], "A study");
    assert_eq!(root["description"], "What it found.");
    assert_eq!(root["datePublished"], "2026-10-29");
    assert_eq!(root["version"], "1.0.0");
    assert_eq!(root["license"], json!({"@id": "LICENSES/CC0-1.0.txt"}));
    assert_eq!(
        root["conformsTo"],
        json!({"@id": "https://w3id.org/ro/wfrun/process/0.6"})
    );
    assert_eq!(root["publisher"], json!({"@id": "#publisher"}));
    assert_eq!(root["keywords"], json!(["alpha", "beta"]));
    assert_eq!(
        root["author"],
        json!([{"@id": ORCID}, {"@id": "#author-2"}, {"@id": "#author-3"}])
    );
    assert_eq!(
        root["hasPart"],
        json!([
            {"@id": "src/run.py"},
            {"@id": "data/in.csv"},
            {"@id": "results/out.csv"},
            {"@id": "LICENSES/CC0-1.0.txt"}
        ])
    );
    assert_eq!(root["identifier"], json!({"@id": DOI_URL}));
    assert_eq!(root["cite-as"], DOI_URL);
    assert_eq!(
        entity(&v, DOI_URL),
        &json!({
            "@id": DOI_URL,
            "@type": "PropertyValue",
            "propertyID": "https://registry.identifiers.org/registry/doi",
            "value": "doi:10.5281/zenodo.1",
            "url": DOI_URL
        })
    );
    assert_eq!(
        entity(&v, "#publisher"),
        &json!({"@id": "#publisher", "@type": "Organization", "name": "Zenodo"})
    );
    assert_eq!(
        entity(&v, "#tool"),
        &json!({
            "@id": "#tool",
            "@type": "SoftwareApplication",
            "name": "ignorarium",
            "url": "https://example.org/tool",
            "softwareVersion": "0.1.0"
        })
    );
    assert_eq!(
        entity(&v, ORCID),
        &json!({
            "@id": ORCID,
            "@type": "Person",
            "name": "Josiah Carberry",
            "givenName": "Josiah",
            "familyName": "Carberry",
            "affiliation": "Brown University"
        })
    );
    assert_eq!(
        entity(&v, "#author-2"),
        &json!({
            "@id": "#author-2",
            "@type": "Person",
            "name": "Ada Lovelace",
            "givenName": "Ada",
            "familyName": "Lovelace"
        })
    );
    assert_eq!(
        entity(&v, "#author-3"),
        &json!({"@id": "#author-3", "@type": "Organization", "name": "The Lab"})
    );

    f.release.doi = None;
    f.citation.keywords.clear();
    let v = build(&f);
    let root = entity(&v, "./");
    for k in ["identifier", "cite-as", "keywords"] {
        assert!(root.get(k).is_none(), "{k}");
    }
    assert_eq!(count(&v, DOI_URL), 0);
}

#[test]
fn graph_order_follows_the_spec() {
    let v = build(&fixture());
    let u1 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-1"));
    let u2 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-2"));
    let env = format!("#env-{}", &sha256_bytes(b"LANG\0C")[..12]);
    let sha2 = format!("#sha256-{H2}");
    let image = format!("#image-{H1}");
    assert_eq!(
        ids(&v),
        [
            "ro-crate-metadata.json",
            "./",
            "https://w3id.org/ro/wfrun/process/0.6",
            "src/run.py",
            "data/in.csv",
            "results/out.csv",
            "LICENSES/CC0-1.0.txt",
            DOI_URL,
            "#publisher",
            ORCID,
            "#author-2",
            "#author-3",
            "#tool",
            &image,
            &u1,
            &u2,
            &sha2,
            &env
        ]
    );
}

#[test]
fn metadata_skips_its_own_files() {
    let mut f = fixture();
    f.files.push(entry("ro-crate-metadata.json", 1));
    f.files.push(entry("MANIFEST.sha256", 1));
    let v = build(&f);
    assert_eq!(count(&v, "MANIFEST.sha256"), 0);
    assert_eq!(entity(&v, "./")["hasPart"].as_array().unwrap().len(), 4);
    assert_eq!(count(&v, "ro-crate-metadata.json"), 1);
}

#[test]
fn files_carry_their_licence() {
    let v = build(&fixture());
    assert_eq!(
        entity(&v, "src/run.py"),
        &json!({
            "@id": "src/run.py",
            "@type": "File",
            "contentSize": "10",
            "license": {"@id": "LICENSES/Apache-2.0.txt"}
        })
    );
    assert_eq!(
        entity(&v, "data/in.csv"),
        &json!({
            "@id": "data/in.csv",
            "@type": "File",
            "contentSize": "20",
            "license": {"@id": "LICENSES/CC-BY-4.0.txt"}
        })
    );
    assert_eq!(
        entity(&v, "LICENSES/CC0-1.0.txt"),
        &json!({
            "@id": "LICENSES/CC0-1.0.txt",
            "@type": "File",
            "contentSize": "40",
            "name": "CC0-1.0",
            "identifier": "CC0-1.0",
            "description": "Licence text for CC0-1.0."
        })
    );
}

#[test]
fn runs_record_status_image_and_environment() {
    let v = build(&fixture());
    let u1 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-1"));
    let u2 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-2"));
    let env = format!("#env-{}", &sha256_bytes(b"LANG\0C")[..12]);
    assert_eq!(
        entity(&v, &u1),
        &json!({
            "@id": u1,
            "@type": "CreateAction",
            "name": "Run row-1",
            "instrument": {"@id": "#tool"},
            "startTime": "2026-01-01T00:00:00Z",
            "endTime": "2026-01-01T00:01:00Z",
            "actionStatus": {"@id": "http://schema.org/CompletedActionStatus"},
            "object": [{"@id": "data/in.csv"}, {"@id": format!("#sha256-{H2}")}],
            "result": [{"@id": "results/out.csv"}],
            "containerImage": {"@id": format!("#image-{H1}")},
            "environment": [{"@id": env}]
        })
    );
    assert_eq!(
        entity(&v, &u2),
        &json!({
            "@id": u2,
            "@type": "CreateAction",
            "name": "Run row-2",
            "instrument": {"@id": "#tool"},
            "startTime": "2026-01-01T00:00:00Z",
            "endTime": "2026-01-01T00:01:00Z",
            "actionStatus": {"@id": "http://schema.org/FailedActionStatus"},
            "error": "exit 1",
            "object": [{"@id": format!("#sha256-{H2}")}],
            "result": [],
            "containerImage": {"@id": format!("#image-{H1}")},
            "environment": [{"@id": env}]
        })
    );
    assert_eq!(
        entity(&v, &format!("#image-{H1}")),
        &json!({
            "@id": format!("#image-{H1}"),
            "@type": "ContainerImage",
            "additionalType": {"@id": "https://w3id.org/ro/terms/workflow-run#DockerImage"},
            "registry": "docker.io",
            "name": "library/python",
            "tag": "3.12",
            "sha256": H1
        })
    );
    assert_eq!(count(&v, &format!("#image-{H1}")), 1);
    assert_eq!(
        entity(&v, &env),
        &json!({"@id": env, "@type": "PropertyValue", "name": "LANG", "value": "C"})
    );
    assert_eq!(count(&v, &env), 1);
    assert_eq!(
        entity(&v, "./")["mentions"],
        json!([{"@id": u1}, {"@id": u2}, {"@id": format!("#sha256-{H2}")}])
    );
}

#[test]
fn optional_run_fields_are_absent_when_unset() {
    let mut f = fixture();
    f.runs = vec![run("only", Status::Completed)];
    f.artifacts.clear();
    let v = build(&f);
    let u = format!("urn:uuid:{}", run_uuid("fixture-numeric", "only"));
    let r = entity(&v, &u);
    for k in ["containerImage", "environment", "error"] {
        assert!(r.get(k).is_none(), "{k}");
    }
    assert_eq!(r["object"], json!([]));
    assert_eq!(r["result"], json!([]));
}

#[test]
fn hash_items_and_artifacts_are_entities() {
    let mut f = fixture();
    f.runs[1].result = vec![hash("r.bin", H3)];
    let v = build(&f);
    let id = format!("#sha256-{H2}");
    assert_eq!(
        entity(&v, &id),
        &json!({
            "@id": id,
            "@type": "CreativeWork",
            "name": "big.bin",
            "identifier": format!("sha256:{H2}"),
            "contentSize": "5000000000"
        })
    );
    assert_eq!(count(&v, &id), 1);
    let id3 = format!("#sha256-{H3}");
    assert_eq!(
        entity(&v, &id3),
        &json!({
            "@id": id3,
            "@type": "CreativeWork",
            "name": "r.bin",
            "identifier": format!("sha256:{H3}")
        })
    );
}

#[test]
fn artifact_only_hash_is_an_entity_and_first_name_wins() {
    let mut f = fixture();
    f.runs[0].object = vec![hash("first", H2)];
    f.artifacts.push(Artifact {
        name: "extra.bin".into(),
        sha256: H3.into(),
        size: 7,
    });
    let v = build(&f);
    assert_eq!(entity(&v, &format!("#sha256-{H2}"))["name"], "first");
    let e = entity(&v, &format!("#sha256-{H3}"));
    assert_eq!(e["name"], "extra.bin");
    assert_eq!(e["contentSize"], "7");
}

#[test]
fn missing_names_absent_paths() {
    let mut f = fixture();
    f.runs[0].object = vec![Item::Path("data/in.csv".into())];
    f.runs[0].result = vec![];
    f.runs[1].object = vec![Item::Path("data/in.csv".into())];
    f.runs[1].result = vec![Item::Path("results/gone\n.csv".into()), hash("x", H3)];
    f.runs[0]
        .object
        .insert(0, Item::Path("data/lost.csv".into()));
    let p = missing(&f.runs, &["data/in.csv"]);
    let got: Vec<(&str, &str, &str)> = p
        .iter()
        .map(|x| (x.code, x.path.as_str(), x.message.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "E-PUB-008",
                "runs[0].object[0]",
                "data/lost.csv is not in the bundle"
            ),
            (
                "E-PUB-008",
                "runs[1].result[0]",
                "results/gone\\u{a}.csv is not in the bundle"
            ),
        ]
    );
    for x in &p {
        assert_eq!(
            x.instruction,
            "Add the file to the bundle, or refer to it by name and SHA-256."
        );
    }
}

#[test]
fn env_ids_do_not_collide_and_repeats_collapse() {
    let mut f = fixture();
    f.runs[0].env = vec![
        ("a".into(), "b=c".into()),
        ("a=b".into(), "c".into()),
        ("a".into(), "b=c".into()),
    ];
    f.runs[1].env.clear();
    let v = build(&f);
    let e1 = format!("#env-{}", &sha256_bytes(b"a\0b=c")[..12]);
    let e2 = format!("#env-{}", &sha256_bytes(b"a=b\0c")[..12]);
    assert_ne!(e1, e2);
    assert_eq!(entity(&v, &e1)["value"], "b=c");
    assert_eq!(entity(&v, &e2)["name"], "a=b");
    let u1 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-1"));
    assert_eq!(
        entity(&v, &u1)["environment"],
        json!([{"@id": e1}, {"@id": e2}])
    );
}

#[test]
fn data_entity_ids_are_percent_encoded() {
    let mut f = fixture();
    f.files.push(entry("data/a b#1%.txt", 3));
    f.files.push(entry("data/\u{e9}.txt", 4));
    f.runs[0].object = vec![Item::Path("data/a b#1%.txt".into())];
    let v = build(&f);
    let enc = "data/a%20b%231%25.txt";
    assert_eq!(entity(&v, enc)["contentSize"], "3");
    assert_eq!(entity(&v, "data/%C3%A9.txt")["contentSize"], "4");
    assert_eq!(count(&v, "data/a b#1%.txt"), 0);
    let parts = entity(&v, "./")["hasPart"].as_array().unwrap().clone();
    assert!(parts.contains(&json!({"@id": enc})));
    assert!(parts.contains(&json!({"@id": "data/%C3%A9.txt"})));
    let u1 = format!("urn:uuid:{}", run_uuid("fixture-numeric", "row-1"));
    assert_eq!(entity(&v, &u1)["object"], json!([{"@id": enc}]));
}

#[test]
fn licence_references_are_encoded() {
    let mut f = fixture();
    f.licensing.data = "LicenseRef-a b".into();
    let v = build(&f);
    assert_eq!(
        entity(&v, "./")["license"],
        json!({"@id": "LICENSES/LicenseRef-a%20b.txt"})
    );
}
