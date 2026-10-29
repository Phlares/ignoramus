//! The Zenodo sandbox: stage a draft, describe it, upload files, verify them, publish.
//! The adapter never talks to the real archive. `send` refuses any other host.
use crate::http::{Body, Method, Request, Response, Transport};
use crate::{Error, Secret};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub const API: &str = "https://sandbox.zenodo.org/api";
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

fn parse(step: &'static str, r: &Response) -> Result<Value, Error> {
    serde_json::from_slice(&r.body).map_err(|_| protocol(step, "the body is not JSON"))
}

impl<T: Transport> Zenodo<T> {
    /// The only constructor.
    pub fn sandbox(transport: T) -> Self {
        Zenodo { transport }
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

    pub fn create(&self, t: &StageToken) -> Result<(Draft, Step), Error> {
        let url = format!("{API}/deposit/depositions");
        let body = serde_json::to_vec(&json!({"metadata": {"prereserve_doi": true}}))
            .expect("a JSON value serializes");
        let r = self.send(
            "create",
            &[201],
            Request {
                method: Method::Post,
                url: url.clone(),
                token: Some(&t.0),
                body: Body::Json(body),
                keep_body: true,
            },
        )?;
        let v = parse("create", &r)?;
        let draft = Draft {
            id: number("create", &v, "/id")?,
            recid: number("create", &v, "/metadata/prereserve_doi/recid")?,
            doi: text("create", &v, "/metadata/prereserve_doi/doi")?.to_string(),
            bucket: text("create", &v, "/links/bucket")?.to_string(),
        };
        let detail = json!({
            "id": draft.id, "recid": draft.recid, "doi": draft.doi, "bucket": draft.bucket
        });
        Ok((draft, step("create", "POST", url, r.status, detail)))
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
        let url = format!("{API}/deposit/depositions/{}", d.id);
        let r = self.send(
            "describe",
            &[200],
            Request {
                method: Method::Put,
                url: url.clone(),
                token: Some(&t.0),
                body: Body::Json(body),
                keep_body: true,
            },
        )?;
        let v = parse("describe", &r)?;
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
        Ok(step(
            "describe",
            "PUT",
            url,
            r.status,
            json!({"doi": d.doi, "title": title}),
        ))
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
        let size = std::fs::metadata(path)?.len();
        let sha256 = file_sha256(path)?;
        let url = format!("{}/{name}", d.bucket);
        let r = self.send(
            "upload",
            &[200, 201],
            Request {
                method: Method::Put,
                url: url.clone(),
                token: Some(&t.0),
                body: Body::File(path.to_path_buf()),
                keep_body: true,
            },
        )?;
        let v = parse("upload", &r)?;
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
        Ok(step("upload", "PUT", url, r.status, detail))
    }

    pub fn verify(
        &self,
        t: &StageToken,
        d: &Draft,
        expected: &[(String, String)],
    ) -> Result<Step, Error> {
        let r = self.send(
            "verify",
            &[200],
            Request {
                method: Method::Get,
                url: d.bucket.clone(),
                token: Some(&t.0),
                body: Body::None,
                keep_body: true,
            },
        )?;
        let v = parse("verify", &r)?;
        let contents = v
            .get("contents")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("verify", "no contents list"))?;
        let mut listed: Vec<(&str, &str)> = Vec::new();
        for c in contents {
            listed.push((
                text("verify", c, "/key")?,
                text("verify", c, "/links/self")?,
            ));
        }
        let mut problems: Vec<String> = Vec::new();
        for (name, _) in expected {
            match listed.iter().filter(|(k, _)| k == name).count() {
                0 => problems.push(format!("missing {}", esc(name))),
                1 => {}
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
        for (name, want) in expected {
            let link = listed
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, l)| *l)
                .expect("every expected name is listed");
            let got = self.send(
                "verify",
                &[200],
                Request {
                    method: Method::Get,
                    url: link.to_string(),
                    token: Some(&t.0),
                    body: Body::None,
                    keep_body: false,
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
        Ok(step(
            "verify",
            "GET",
            d.bucket.clone(),
            r.status,
            json!({"files": files}),
        ))
    }

    pub fn publish(&self, t: &PublishToken, d: &Draft) -> Result<Step, Error> {
        let url = format!("{API}/deposit/depositions/{}/actions/publish", d.id);
        let r = self.send(
            "publish",
            &[202],
            Request {
                method: Method::Post,
                url: url.clone(),
                token: Some(&t.0),
                body: Body::None,
                keep_body: true,
            },
        )?;
        let v = parse("publish", &r)?;
        let doi = text("publish", &v, "/doi")?;
        if doi != d.doi {
            return Err(mismatch(
                "publish",
                format!(
                    "the record has DOI {}, the draft reserved {}",
                    esc(doi),
                    esc(&d.doi)
                ),
            ));
        }
        let record = number("publish", &v, "/record_id")?;
        let detail = json!({
            "doi": doi,
            "record_id": record,
            "url": format!("https://sandbox.zenodo.org/records/{record}"),
        });
        Ok(step("publish", "POST", url, r.status, detail))
    }
}

fn step(name: &'static str, method: &'static str, url: String, status: u16, detail: Value) -> Step {
    Step {
        step: name,
        method,
        url,
        status,
        detail,
    }
}

fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn file_sha256(path: &Path) -> Result<String, Error> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(h.finalize()));
        }
        h.update(&buf[..n]);
    }
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
