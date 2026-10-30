//! The Zenodo sandbox: stage a draft, describe it, upload files, verify them, publish.
//! The adapter never talks to the real archive. `send` refuses any other host.
use crate::http::{Body, Method, Request, Response, Transport};
use crate::{Error, Secret};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const API: &str = "https://sandbox.zenodo.org/api";
/// The DOI prefix of published records on this instance. Drafts reserve 10.5281 here too.
pub const DOI_PREFIX: &str = "10.5072";
const SANDBOX: &str = "https://sandbox.zenodo.org/";
const BODY_CHARS: usize = 2000;

/// Creates, describes, uploads and verifies. It cannot publish.
pub struct StageToken(pub Secret);
/// Publishes. It is a separate token so a staging mistake cannot publish.
pub struct PublishToken(pub Secret);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: u64,
    pub recid: u64,
    pub doi: String,
    pub bucket: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub step: &'static str,
    pub method: &'static str,
    pub url: String,
    pub status: u16,
    pub detail: Value,
}

pub struct Zenodo<T: Transport> {
    transport: T,
    doi_prefix: &'static str,
}

/// Server text with control characters written as escapes.
fn esc(s: &str) -> String {
    s.escape_debug().to_string()
}

fn protocol(step: &'static str, what: impl Into<String>) -> Error {
    Error::Protocol {
        step,
        what: what.into(),
    }
}

fn mismatch(step: &'static str, what: impl Into<String>) -> Error {
    Error::Mismatch {
        step,
        what: what.into(),
    }
}

fn text<'a>(step: &'static str, v: &'a Value, path: &str) -> Result<&'a str, Error> {
    v.pointer(path)
        .and_then(Value::as_str)
        .ok_or_else(|| protocol(step, format!("no text at {path}")))
}

fn number(step: &'static str, v: &Value, path: &str) -> Result<u64, Error> {
    v.pointer(path)
        .and_then(Value::as_u64)
        .ok_or_else(|| protocol(step, format!("no number at {path}")))
}

impl<T: Transport> Zenodo<T> {
    /// The only constructor.
    pub fn sandbox(transport: T) -> Self {
        Zenodo {
            transport,
            doi_prefix: DOI_PREFIX,
        }
    }

    fn send(&self, step: &'static str, accept: &[u16], req: Request) -> Result<Response, Error> {
        if !req.url.starts_with(SANDBOX) {
            return Err(protocol(
                step,
                format!("refusing a URL outside the sandbox: {}", esc(&req.url)),
            ));
        }
        let r = self.transport.send(&req)?;
        if !accept.contains(&r.status) {
            let head: String = String::from_utf8_lossy(&r.body)
                .chars()
                .take(BODY_CHARS)
                .collect();
            return Err(Error::Status {
                step,
                status: r.status,
                body: esc(&head),
            });
        }
        Ok(r)
    }

    /// Sends one step whose answer is JSON.
    /// Returns the answer and the step record without its detail.
    fn call(
        &self,
        step: &'static str,
        accept: &[u16],
        method: Method,
        url: String,
        token: &Secret,
        body: Body,
    ) -> Result<(Value, Step), Error> {
        let req = Request {
            method,
            url: url.clone(),
            token: Some(token),
            body,
            keep_body: true,
            follow: false,
        };
        let r = self.send(step, accept, req)?;
        let v =
            serde_json::from_slice(&r.body).map_err(|_| protocol(step, "the body is not JSON"))?;
        let record = Step {
            step,
            method: method.as_str(),
            url,
            status: r.status,
            detail: Value::Null,
        };
        Ok((v, record))
    }

    pub fn create(&self, t: &StageToken) -> Result<(Draft, Step), Error> {
        let body = serde_json::to_vec(&json!({"metadata": {"prereserve_doi": true}}))
            .expect("a JSON value serializes");
        let (v, step) = self.call(
            "create",
            &[201],
            Method::Post,
            format!("{API}/deposit/depositions"),
            &t.0,
            Body::Json(body),
        )?;
        let draft = Draft {
            id: number("create", &v, "/id")?,
            recid: number("create", &v, "/metadata/prereserve_doi/recid")?,
            doi: text("create", &v, "/metadata/prereserve_doi/doi")?.to_string(),
            bucket: text("create", &v, "/links/bucket")?.to_string(),
        };
        let detail = json!({
            "id": draft.id, "recid": draft.recid, "doi": draft.doi, "bucket": draft.bucket
        });
        Ok((draft, Step { detail, ..step }))
    }

    pub fn describe(
        &self,
        t: &StageToken,
        d: &Draft,
        datacite: &Value,
        pin: &str,
    ) -> Result<Step, Error> {
        let doi = datacite.get("doi").and_then(Value::as_str);
        if doi != Some(d.doi.as_str()) {
            return Err(mismatch(
                "describe",
                format!(
                    "the bundle is for DOI {}, the draft holds {}",
                    doi.map_or("(none)".to_string(), esc),
                    esc(&d.doi)
                ),
            ));
        }
        let metadata = deposit_metadata(datacite, pin)?;
        let title = metadata["title"].clone();
        let body =
            serde_json::to_vec(&json!({"metadata": metadata})).expect("a JSON value serializes");
        let (v, step) = self.call(
            "describe",
            &[200],
            Method::Put,
            format!("{API}/deposit/depositions/{}", d.id),
            &t.0,
            Body::Json(body),
        )?;
        let kept = v
            .pointer("/metadata/prereserve_doi/doi")
            .and_then(Value::as_str);
        if kept != Some(d.doi.as_str()) {
            return Err(mismatch(
                "describe",
                format!(
                    "the draft no longer reserves DOI {}; it reports {}",
                    esc(&d.doi),
                    kept.map_or("(none)".to_string(), esc)
                ),
            ));
        }
        Ok(Step {
            detail: json!({"doi": d.doi, "title": title}),
            ..step
        })
    }

    pub fn upload(
        &self,
        t: &StageToken,
        d: &Draft,
        name: &str,
        path: &Path,
    ) -> Result<Step, Error> {
        if !safe_name(name) {
            return Err(Error::Request(format!(
                "a file name is letters, digits, dot, underscore or hyphen; got {}",
                esc(name)
            )));
        }
        let (sha256, size) = file_sha256(path)?;
        let (v, step) = self.call(
            "upload",
            &[200, 201],
            Method::Put,
            format!("{}/{name}", d.bucket),
            &t.0,
            Body::File(path.to_path_buf()),
        )?;
        let key = text("upload", &v, "/key")?;
        if key != name {
            return Err(mismatch(
                "upload",
                format!("the server stored {} for {name}", esc(key)),
            ));
        }
        let remote = number("upload", &v, "/size")?;
        if remote != size {
            return Err(mismatch(
                "upload",
                format!("the server holds {remote} bytes, the file has {size}"),
            ));
        }
        let sum = text("upload", &v, "/checksum")?;
        let md5 = sum.strip_prefix("md5:").unwrap_or(sum);
        let detail = json!({"key": name, "size": size, "sha256": sha256, "md5": md5});
        Ok(Step { detail, ..step })
    }

    pub fn verify(
        &self,
        t: &StageToken,
        d: &Draft,
        expected: &[(String, String)],
    ) -> Result<Step, Error> {
        for (i, (name, _)) in expected.iter().enumerate() {
            if expected[..i].iter().any(|(n, _)| n == name) {
                return Err(Error::Request(format!(
                    "the expected files name {} twice",
                    esc(name)
                )));
            }
        }
        let (v, step) = self.call(
            "verify",
            &[200],
            Method::Get,
            format!("{API}/deposit/depositions/{}/files", d.id),
            &t.0,
            Body::None,
        )?;
        let entries = v
            .as_array()
            .ok_or_else(|| protocol("verify", "the file list is not a list"))?;
        let mut listed: Vec<(&str, &str)> = Vec::new();
        for e in entries {
            listed.push((
                text("verify", e, "/filename")?,
                text("verify", e, "/links/download")?,
            ));
        }
        let mut problems: Vec<String> = Vec::new();
        let mut links = Vec::new();
        for (name, want) in expected {
            let mut hits = listed.iter().filter(|(k, _)| k == name);
            match (hits.next(), hits.next()) {
                (None, _) => problems.push(format!("missing {}", esc(name))),
                (Some((_, link)), None) => links.push((name, want, *link)),
                _ => problems.push(format!("listed twice {}", esc(name))),
            }
        }
        for (k, _) in &listed {
            if !expected.iter().any(|(n, _)| n == k) {
                problems.push(format!("extra {}", esc(k)));
            }
        }
        if !problems.is_empty() {
            return Err(mismatch("verify", problems.join(", ")));
        }
        let mut files = Vec::new();
        for (name, want, link) in links {
            let got = self.send(
                "verify",
                &[200],
                Request {
                    method: Method::Get,
                    url: link.to_string(),
                    token: Some(&t.0),
                    body: Body::None,
                    keep_body: false,
                    follow: true,
                },
            )?;
            if &got.sha256 != want {
                return Err(mismatch(
                    "verify",
                    format!(
                        "{} downloads as SHA-256 {}, expected {}",
                        esc(name),
                        esc(&got.sha256),
                        esc(want)
                    ),
                ));
            }
            files.push(json!({"key": name, "sha256": got.sha256}));
        }
        Ok(Step {
            detail: json!({"files": files}),
            ..step
        })
    }

    pub fn publish(&self, t: &PublishToken, d: &Draft) -> Result<Step, Error> {
        let (v, step) = self.call(
            "publish",
            &[202],
            Method::Post,
            format!("{API}/deposit/depositions/{}/actions/publish", d.id),
            &t.0,
            Body::None,
        )?;
        let doi = text("publish", &v, "/doi")?;
        let record = number("publish", &v, "/record_id")?;
        let suffix = format!("zenodo.{record}");
        let reserved = d.doi.split_once('/').map(|(_, s)| s);
        if doi != format!("{}/{suffix}", self.doi_prefix) || reserved != Some(suffix.as_str()) {
            return Err(mismatch(
                "publish",
                format!(
                    "the record has DOI {}, expected {}/{suffix} for the draft that reserved {}",
                    esc(doi),
                    self.doi_prefix,
                    esc(&d.doi)
                ),
            ));
        }
        let detail = json!({
            "doi": doi,
            "record_id": record,
            "url": format!("https://sandbox.zenodo.org/records/{record}"),
        });
        Ok(Step { detail, ..step })
    }
}

fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The file's SHA-256 and byte count.
fn file_sha256(path: &Path) -> Result<(String, u64), Error> {
    let mut h = Sha256::new();
    let size = std::io::copy(&mut std::fs::File::open(path)?, &mut h)?;
    Ok((hex::encode(h.finalize()), size))
}

/// The DataCite entry of `list` whose `key` field equals `want`.
fn entry<'a>(v: &'a Value, list: &str, key: &str, want: &str) -> Option<&'a Value> {
    v.get(list)?
        .as_array()?
        .iter()
        .find(|e| e.get(key).and_then(Value::as_str) == Some(want))
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_lowercase().chain(c).collect())
        .unwrap_or_default()
}

fn creator(c: &Value) -> Result<Value, Error> {
    let mut m = Map::new();
    m.insert("name".into(), json!(text("describe", c, "/name")?));
    if let Some(a) = c.pointer("/affiliation/0/name").and_then(Value::as_str) {
        m.insert("affiliation".into(), json!(a));
    }
    if let Some(id) = entry(c, "nameIdentifiers", "nameIdentifierScheme", "ORCID") {
        let full = text("describe", id, "/nameIdentifier")?;
        let bare = full.strip_prefix("https://orcid.org/").ok_or_else(|| {
            protocol(
                "describe",
                format!("an ORCID is https://orcid.org/<id>; got {}", esc(full)),
            )
        })?;
        m.insert("orcid".into(), json!(bare));
    }
    Ok(Value::Object(m))
}

/// Zenodo deposit metadata from DataCite metadata. `pin` is the SHA-256 of `MANIFEST.sha256`.
pub fn deposit_metadata(datacite: &Value, pin: &str) -> Result<Value, Error> {
    const S: &str = "describe";
    if text(S, datacite, "/types/resourceTypeGeneral")? != "Dataset" {
        return Err(protocol(S, "only a Dataset resource type is deposited"));
    }
    let creators = datacite
        .get("creators")
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| protocol(S, "no creators"))?
        .iter()
        .map(creator)
        .collect::<Result<Vec<_>, _>>()?;
    let abstract_ = entry(datacite, "descriptions", "descriptionType", "Abstract")
        .ok_or_else(|| protocol(S, "no Abstract description"))
        .and_then(|e| text(S, e, "/description"))?;
    let issued = entry(datacite, "dates", "dateType", "Issued")
        .ok_or_else(|| protocol(S, "no Issued date"))
        .and_then(|e| text(S, e, "/date"))?;
    let mut m = Map::new();
    m.insert("upload_type".into(), json!("dataset"));
    m.insert("title".into(), json!(text(S, datacite, "/titles/0/title")?));
    m.insert("creators".into(), Value::Array(creators));
    m.insert(
        "description".into(),
        json!(
            abstract_
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        ),
    );
    m.insert("publication_date".into(), json!(issued));
    m.insert("access_right".into(), json!("open"));
    m.insert(
        "license".into(),
        json!(text(S, datacite, "/rightsList/0/rightsIdentifier")?),
    );
    if let Some(v) = datacite.get("version").and_then(Value::as_str) {
        m.insert("version".into(), json!(v));
    }
    let subjects: Vec<&str> = datacite
        .get("subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("subject").and_then(Value::as_str))
        .collect();
    if !subjects.is_empty() {
        m.insert("keywords".into(), json!(subjects));
    }
    let mut related = Vec::new();
    for r in datacite
        .get("relatedIdentifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        related.push(json!({
            "identifier": text(S, r, "/relatedIdentifier")?,
            "relation": lower_first(text(S, r, "/relationType")?),
        }));
    }
    if !related.is_empty() {
        m.insert("related_identifiers".into(), Value::Array(related));
    }
    m.insert(
        "notes".into(),
        json!(format!(
            "SHA-256 of the bundle manifest (MANIFEST.sha256): {pin}"
        )),
    );
    m.insert("prereserve_doi".into(), json!(true));
    Ok(Value::Object(m))
}
