//! RO-Crate 1.3 metadata with the Process Run Crate 0.6 profile.
//!
//! The document lists the bundle's files and the runs that made its results.
use crate::Problem;
use crate::citation::{Author, Citation, Release, Tool};
use crate::licence::Licensing;
use crate::manifest::{self, Entry, sha256_bytes};
use crate::tree::shown;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const DESCRIPTOR: &str = "ro-crate-metadata.json";
const ACTION_STATUS: &str = "http://schema.org/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Completed,
    Failed(String),
}

/// A run input or output: a bundle file, or a file held elsewhere and named by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Path(String),
    Hash { name: String, sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub registry: String,
    pub name: String,
    pub tag: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub start: String,
    pub end: String,
    pub status: Status,
    pub object: Vec<Item>,
    pub result: Vec<Item>,
    pub image: Option<Image>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

pub struct Crate<'a> {
    pub study_id: &'a str,
    pub citation: &'a Citation,
    pub release: &'a Release,
    pub tool: &'a Tool,
    pub licensing: &'a Licensing,
    pub files: &'a [Entry],
    pub runs: &'a [Run],
    pub artifacts: &'a [Artifact],
}

/// A version 8 UUID (RFC 9562) derived from the study and run ids, so builds repeat.
pub fn run_uuid(study_id: &str, run_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ignorarium run\0");
    h.update(study_id.as_bytes());
    h.update(b"\0");
    h.update(run_id.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.finalize()[..16]);
    b[6] = (b[6] & 0x0F) | 0x80;
    b[8] = (b[8] & 0x3F) | 0x80;
    let x = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &x[..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..]
    )
}

/// One E-PUB-008 per path item that names no bundle file, in run then item order.
pub fn missing(runs: &[Run], names: &[&str]) -> Vec<Problem> {
    let mut out = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        for (field, items) in [("object", &run.object), ("result", &run.result)] {
            for (j, item) in items.iter().enumerate() {
                let Item::Path(p) = item else { continue };
                if !names.contains(&p.as_str()) {
                    out.push(Problem::new(
                        "E-PUB-008",
                        format!("runs[{i}].{field}[{j}]"),
                        format!("{} is not in the bundle", shown(p)),
                        "Add the file to the bundle, or refer to it by name and SHA-256.",
                    ));
                }
            }
        }
    }
    out
}

/// Percent-encodes a bundle path as a relative URI reference.
fn uri(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn id(s: impl Into<String>) -> Value {
    json!({"@id": s.into()})
}

fn ids<S: Into<String>>(items: impl IntoIterator<Item = S>) -> Value {
    Value::Array(items.into_iter().map(id).collect())
}

fn item_id(i: &Item) -> String {
    match i {
        Item::Path(p) => uri(p),
        Item::Hash { sha256, .. } => format!("#sha256-{sha256}"),
    }
}

fn author_id(a: &Author, n: usize) -> String {
    a.orcid.clone().unwrap_or_else(|| format!("#author-{n}"))
}

fn person(a: &Author, n: usize) -> Value {
    let mut m = Map::new();
    m.insert("@id".into(), json!(author_id(a, n)));
    match (&a.given_names, &a.family_names) {
        (Some(given), Some(family)) => {
            m.insert("@type".into(), json!("Person"));
            m.insert("name".into(), json!(format!("{given} {family}")));
            m.insert("givenName".into(), json!(given));
            m.insert("familyName".into(), json!(family));
            if let Some(aff) = &a.affiliation {
                m.insert("affiliation".into(), json!(aff));
            }
        }
        _ => {
            m.insert("@type".into(), json!("Organization"));
            m.insert("name".into(), json!(a.name));
        }
    }
    Value::Object(m)
}

fn file(c: &Crate, e: &Entry) -> Value {
    let mut m = Map::new();
    m.insert("@id".into(), json!(uri(&e.path)));
    m.insert("@type".into(), json!("File"));
    m.insert("contentSize".into(), json!(e.size.to_string()));
    let own = e
        .path
        .strip_prefix("LICENSES/")
        .and_then(|n| n.strip_suffix(".txt"));
    match own {
        Some(spdx) => {
            m.insert("name".into(), json!(spdx));
            m.insert("identifier".into(), json!(spdx));
            m.insert(
                "description".into(),
                json!(format!("Licence text for {spdx}.")),
            );
        }
        None => {
            let spdx = c.licensing.of(&e.path);
            m.insert("license".into(), id(uri(&format!("LICENSES/{spdx}.txt"))));
        }
    }
    Value::Object(m)
}

fn action(c: &Crate, r: &Run) -> Value {
    let mut m = Map::new();
    m.insert(
        "@id".into(),
        json!(format!("urn:uuid:{}", run_uuid(c.study_id, &r.id))),
    );
    m.insert("@type".into(), json!("CreateAction"));
    m.insert("name".into(), json!(r.name));
    m.insert("instrument".into(), id("#tool"));
    m.insert("startTime".into(), json!(r.start));
    m.insert("endTime".into(), json!(r.end));
    let status = match &r.status {
        Status::Completed => "CompletedActionStatus",
        Status::Failed(why) => {
            m.insert("error".into(), json!(why));
            "FailedActionStatus"
        }
    };
    m.insert(
        "actionStatus".into(),
        id(format!("{ACTION_STATUS}{status}")),
    );
    m.insert("object".into(), ids(r.object.iter().map(item_id)));
    m.insert("result".into(), ids(r.result.iter().map(item_id)));
    if let Some(img) = &r.image {
        m.insert(
            "containerImage".into(),
            id(format!("#image-{}", img.sha256)),
        );
    }
    let mut env: Vec<String> = Vec::new();
    for (k, v) in &r.env {
        let eid = env_id(k, v);
        if !env.contains(&eid) {
            env.push(eid);
        }
    }
    if !env.is_empty() {
        m.insert("environment".into(), ids(env));
    }
    Value::Object(m)
}

fn env_id(name: &str, value: &str) -> String {
    format!(
        "#env-{}",
        &sha256_bytes(format!("{name}\0{value}").as_bytes())[..12]
    )
}

/// A `#sha256-<hex>` entity under construction, keyed by hash in first-seen order.
#[derive(Default)]
struct Hashes {
    order: Vec<String>,
    by_hash: BTreeMap<String, (String, Option<u64>)>,
}

impl Hashes {
    fn see(&mut self, sha256: &str, name: &str, size: Option<u64>) {
        let slot = self.by_hash.entry(sha256.to_string()).or_insert_with(|| {
            self.order.push(sha256.to_string());
            (name.to_string(), None)
        });
        slot.1 = slot.1.or(size);
    }

    fn entities(&self) -> impl Iterator<Item = Value> + '_ {
        self.order.iter().map(|h| {
            let (name, size) = &self.by_hash[h];
            let mut m = Map::new();
            m.insert("@id".into(), json!(format!("#sha256-{h}")));
            m.insert("@type".into(), json!("CreativeWork"));
            m.insert("name".into(), json!(name));
            m.insert("identifier".into(), json!(format!("sha256:{h}")));
            if let Some(s) = size {
                m.insert("contentSize".into(), json!(s.to_string()));
            }
            Value::Object(m)
        })
    }
}

/// The `ro-crate-metadata.json` document.
pub fn metadata(c: &Crate) -> Value {
    let files: Vec<&Entry> = c
        .files
        .iter()
        .filter(|e| e.path != DESCRIPTOR && e.path != manifest::NAME)
        .collect();
    let run_ids: Vec<String> = c
        .runs
        .iter()
        .map(|r| format!("urn:uuid:{}", run_uuid(c.study_id, &r.id)))
        .collect();

    let mut images: Vec<&Image> = Vec::new();
    let mut seen_images = BTreeSet::new();
    let mut hashes = Hashes::default();
    let mut envs: Vec<(String, &str, &str)> = Vec::new();
    let mut seen_envs = BTreeSet::new();
    for r in c.runs {
        if let Some(img) = &r.image
            && seen_images.insert(&img.sha256)
        {
            images.push(img);
        }
        for item in r.object.iter().chain(&r.result) {
            if let Item::Hash { name, sha256 } = item {
                hashes.see(sha256, name, None);
            }
        }
        for (k, v) in &r.env {
            let eid = env_id(k, v);
            if seen_envs.insert(eid.clone()) {
                envs.push((eid, k, v));
            }
        }
    }
    for a in c.artifacts {
        hashes.see(&a.sha256, &a.name, Some(a.size));
    }

    let data = &c.licensing.data;
    let mut root = Map::new();
    root.insert("@id".into(), json!("./"));
    root.insert("@type".into(), json!("Dataset"));
    root.insert("name".into(), json!(c.citation.title));
    root.insert("description".into(), json!(c.citation.summary));
    root.insert("datePublished".into(), json!(c.release.date));
    root.insert("version".into(), json!(c.release.version));
    root.insert("license".into(), id(uri(&format!("LICENSES/{data}.txt"))));
    root.insert(
        "author".into(),
        ids(c
            .citation
            .authors
            .iter()
            .enumerate()
            .map(|(i, a)| author_id(a, i + 1))),
    );
    root.insert("publisher".into(), id("#publisher"));
    if !c.citation.keywords.is_empty() {
        root.insert("keywords".into(), json!(c.citation.keywords));
    }
    root.insert(
        "conformsTo".into(),
        id("https://w3id.org/ro/wfrun/process/0.6"),
    );
    root.insert("hasPart".into(), ids(files.iter().map(|e| uri(&e.path))));
    if let Some(doi) = &c.release.doi {
        let url = format!("https://doi.org/{doi}");
        root.insert("identifier".into(), id(url.clone()));
        root.insert("cite-as".into(), json!(url));
    }
    let mentions = run_ids
        .iter()
        .cloned()
        .chain(c.artifacts.iter().map(|a| format!("#sha256-{}", a.sha256)));
    root.insert("mentions".into(), ids(mentions));

    let mut graph = vec![
        json!({
            "@id": DESCRIPTOR,
            "@type": "CreativeWork",
            "about": {"@id": "./"},
            "conformsTo": {"@id": "https://w3id.org/ro/crate/1.3"},
        }),
        Value::Object(root),
        json!({
            "@id": "https://w3id.org/ro/wfrun/process/0.6",
            "@type": "CreativeWork",
            "name": "Process Run Crate",
            "version": "0.6",
        }),
    ];
    graph.extend(files.iter().map(|e| file(c, e)));
    if let Some(doi) = &c.release.doi {
        let url = format!("https://doi.org/{doi}");
        graph.push(json!({
            "@id": url,
            "@type": "PropertyValue",
            "propertyID": "https://registry.identifiers.org/registry/doi",
            "value": format!("doi:{doi}"),
            "url": url,
        }));
    }
    graph.push(json!({"@id": "#publisher", "@type": "Organization", "name": c.release.publisher}));
    graph.extend(
        c.citation
            .authors
            .iter()
            .enumerate()
            .map(|(i, a)| person(a, i + 1)),
    );
    graph.push(json!({
        "@id": "#tool",
        "@type": "SoftwareApplication",
        "name": c.tool.name,
        "url": c.tool.url,
        "softwareVersion": c.tool.version,
    }));
    graph.extend(images.iter().map(|i| {
        json!({
            "@id": format!("#image-{}", i.sha256),
            "@type": "ContainerImage",
            "additionalType": {"@id": "https://w3id.org/ro/terms/workflow-run#DockerImage"},
            "registry": i.registry,
            "name": i.name,
            "tag": i.tag,
            "sha256": i.sha256,
        })
    }));
    graph.extend(c.runs.iter().map(|r| action(c, r)));
    graph.extend(hashes.entities());
    graph.extend(
        envs.iter().map(
            |(eid, k, v)| json!({"@id": eid, "@type": "PropertyValue", "name": k, "value": v}),
        ),
    );

    json!({
        "@context": [
            "https://w3id.org/ro/crate/1.3/context",
            "https://w3id.org/ro/terms/workflow-run/context",
        ],
        "@graph": graph,
    })
}
