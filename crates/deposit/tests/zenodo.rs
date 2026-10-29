mod common;
use common::{Call, Fake, download, reply};
use deposit::http::{Body, Method};
use deposit::zenodo::{Draft, PublishToken, StageToken, Zenodo, deposit_metadata};
use deposit::{Error, Secret};
use serde_json::{Value, json};
use std::path::Path;
use tempfile::TempDir;

const CREATE: &str = include_str!("fixtures/zenodo/create.json");
const DESCRIBE: &str = include_str!("fixtures/zenodo/describe.json");
const UPLOAD: &str = include_str!("fixtures/zenodo/upload.json");
const BUCKET_LIST: &str = include_str!("fixtures/zenodo/bucket.json");
const PUBLISH: &str = include_str!("fixtures/zenodo/publish.json");

const BUCKET: &str = "https://sandbox.zenodo.org/api/files/568377dd-daf8-4a0d-9d9d-c1c1d8b3c2c1";
const DOI: &str = "10.5072/zenodo.123456";
const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const WORLD_SHA: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
const PIN: &str = "abc123";

fn stage() -> StageToken {
    StageToken(Secret::new("stage-token"))
}

fn publish_token() -> PublishToken {
    PublishToken(Secret::new("publish-token"))
}

fn draft() -> Draft {
    Draft {
        id: 123456,
        recid: 123456,
        doi: DOI.to_string(),
        bucket: BUCKET.to_string(),
    }
}

fn datacite() -> Value {
    json!({
        "schemaVersion": "http://datacite.org/schema/kernel-4",
        "titles": [{"title": "Sleep study"}],
        "creators": [
            {
                "name": "Doe, Jane",
                "nameType": "Personal",
                "familyName": "Doe",
                "givenName": "Jane",
                "nameIdentifiers": [{
                    "nameIdentifier": "https://orcid.org/0000-0002-1825-0097",
                    "nameIdentifierScheme": "ORCID",
                    "schemeUri": "https://orcid.org"
                }],
                "affiliation": [{"name": "Example University"}]
            },
            {"name": "Example Lab", "nameType": "Organizational"}
        ],
        "publisher": {"name": "Example"},
        "publicationYear": 2026,
        "types": {"resourceType": "Study bundle", "resourceTypeGeneral": "Dataset"},
        "version": "1.0.0",
        "dates": [{"date": "2026-10-29", "dateType": "Issued"}],
        "descriptions": [{"description": "a < b & c", "descriptionType": "Abstract"}],
        "subjects": [{"subject": "sleep"}, {"subject": "eeg"}],
        "doi": DOI,
        "relatedIdentifiers": [{
            "relatedIdentifier": "https://github.com/example/tool",
            "relatedIdentifierType": "URL",
            "relationType": "References"
        }],
        "rightsList": [{
            "rights": "CC0-1.0",
            "rightsIdentifier": "cc0-1.0",
            "rightsIdentifierScheme": "SPDX"
        }]
    })
}

fn expected_metadata() -> Value {
    json!({
        "upload_type": "dataset",
        "title": "Sleep study",
        "creators": [
            {"name": "Doe, Jane", "affiliation": "Example University", "orcid": "0000-0002-1825-0097"},
            {"name": "Example Lab"}
        ],
        "description": "a &lt; b &amp; c",
        "publication_date": "2026-10-29",
        "access_right": "open",
        "license": "cc0-1.0",
        "version": "1.0.0",
        "keywords": ["sleep", "eeg"],
        "related_identifiers": [
            {"identifier": "https://github.com/example/tool", "relation": "references"}
        ],
        "notes": "SHA-256 of the bundle manifest (MANIFEST.sha256): abc123",
        "prereserve_doi": true
    })
}

fn file_of(dir: &TempDir, text: &str) -> std::path::PathBuf {
    let p = dir.path().join("b.zip");
    std::fs::write(&p, text).unwrap();
    p
}

fn call(method: Method, url: &str, token: Option<&str>, body: Body, keep: bool) -> Call {
    Call {
        method,
        url: url.to_string(),
        token: token.map(str::to_string),
        body,
        keep_body: keep,
    }
}

// create

#[test]
fn create_reserves_a_doi() {
    let fake = Fake::new(vec![reply(201, CREATE)]);
    let (d, step) = Zenodo::sandbox(&fake).create(&stage()).unwrap();
    assert_eq!(d, draft());
    assert_eq!(step.step, "create");
    assert_eq!(step.method, "POST");
    assert_eq!(
        step.url,
        "https://sandbox.zenodo.org/api/deposit/depositions"
    );
    assert_eq!(step.status, 201);
    assert_eq!(
        step.detail,
        json!({"id": 123456, "recid": 123456, "doi": DOI, "bucket": BUCKET})
    );
    assert_eq!(
        fake.calls(),
        vec![call(
            Method::Post,
            "https://sandbox.zenodo.org/api/deposit/depositions",
            Some("stage-token"),
            Body::Json(br#"{"metadata":{"prereserve_doi":true}}"#.to_vec()),
            true,
        )]
    );
}

#[test]
fn create_without_a_reserved_doi_is_a_protocol_error() {
    let body = CREATE.replace("\"doi\": \"10.5072/zenodo.123456\",", "");
    let fake = Fake::new(vec![reply(201, body)]);
    let err = Zenodo::sandbox(&fake).create(&stage()).unwrap_err();
    assert!(
        matches!(err, Error::Protocol { step: "create", .. }),
        "{err:?}"
    );
}

#[test]
fn create_with_a_non_json_body_is_a_protocol_error() {
    let fake = Fake::new(vec![reply(201, "<html>")]);
    let err = Zenodo::sandbox(&fake).create(&stage()).unwrap_err();
    assert!(
        matches!(err, Error::Protocol { step: "create", .. }),
        "{err:?}"
    );
}

#[test]
fn bad_status_is_an_error_with_the_body() {
    let fake = Fake::new(vec![reply(
        400,
        r#"{"status":400,"message":"Validation error: title is missing"}"#,
    )]);
    match Zenodo::sandbox(&fake).create(&stage()).unwrap_err() {
        Error::Status { step, status, body } => {
            assert_eq!((step, status), ("create", 400));
            assert!(body.contains("title is missing"), "{body}");
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn control_characters_in_a_server_body_are_escaped() {
    let fake = Fake::new(vec![reply(500, "bad\x1b[31m\nred")]);
    match Zenodo::sandbox(&fake).create(&stage()).unwrap_err() {
        Error::Status { body, .. } => assert_eq!(body, "bad\\u{1b}[31m\\nred"),
        e => panic!("{e:?}"),
    }
}

// describe

#[test]
fn describe_maps_datacite() {
    let fake = Fake::new(vec![reply(200, DESCRIBE)]);
    let step = Zenodo::sandbox(&fake)
        .describe(&stage(), &draft(), &datacite(), PIN)
        .unwrap();
    assert_eq!(step.step, "describe");
    assert_eq!(step.method, "PUT");
    assert_eq!(
        step.url,
        "https://sandbox.zenodo.org/api/deposit/depositions/123456"
    );
    assert_eq!(step.status, 200);
    assert_eq!(step.detail, json!({"doi": DOI, "title": "Sleep study"}));
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, Method::Put);
    assert_eq!(calls[0].token.as_deref(), Some("stage-token"));
    assert!(calls[0].keep_body);
    let Body::Json(sent) = &calls[0].body else {
        panic!("{:?}", calls[0].body)
    };
    let text = std::str::from_utf8(sent).unwrap();
    assert!(
        text.starts_with("{\"metadata\":{\"access_right\":"),
        "{text}"
    );
    let sent: Value = serde_json::from_str(text).unwrap();
    assert_eq!(sent, json!({"metadata": expected_metadata()}));
}

#[test]
fn describe_refuses_a_bundle_for_another_doi() {
    let fake = Fake::new(vec![]);
    let mut dc = datacite();
    dc["doi"] = json!("10.5072/zenodo.999");
    let err = Zenodo::sandbox(&fake)
        .describe(&stage(), &draft(), &dc, PIN)
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Mismatch {
                step: "describe",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn describe_checks_the_doi_stays_reserved() {
    let body = DESCRIBE.replace("10.5072/zenodo.123456", "10.5072/zenodo.777");
    let fake = Fake::new(vec![reply(200, body)]);
    let err = Zenodo::sandbox(&fake)
        .describe(&stage(), &draft(), &datacite(), PIN)
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Mismatch {
                step: "describe",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn deposit_metadata_exact() {
    assert_eq!(
        deposit_metadata(&datacite(), PIN).unwrap(),
        expected_metadata()
    );
}

#[test]
fn deposit_metadata_omits_unset_optional_fields() {
    let mut dc = datacite();
    dc.as_object_mut().unwrap().remove("subjects");
    dc.as_object_mut().unwrap().remove("relatedIdentifiers");
    let m = deposit_metadata(&dc, PIN).unwrap();
    assert!(m.get("keywords").is_none());
    assert!(m.get("related_identifiers").is_none());
}

#[test]
fn deposit_metadata_refuses_what_it_cannot_map() {
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "type",
            Box::new(|v| v["types"]["resourceTypeGeneral"] = json!("Software")),
        ),
        (
            "title",
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("titles");
            }),
        ),
        ("creators", Box::new(|v| v["creators"] = json!([]))),
        ("description", Box::new(|v| v["descriptions"] = json!([]))),
        ("date", Box::new(|v| v["dates"] = json!([]))),
        ("licence", Box::new(|v| v["rightsList"] = json!([]))),
        (
            "orcid",
            Box::new(|v| v["creators"][0]["nameIdentifiers"][0]["nameIdentifier"] = json!("0000")),
        ),
    ];
    for (name, edit) in cases {
        let mut dc = datacite();
        edit(&mut dc);
        let err = deposit_metadata(&dc, PIN).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Protocol {
                    step: "describe",
                    ..
                }
            ),
            "{name}: {err:?}"
        );
    }
}

// upload

#[test]
fn upload_puts_the_file_in_the_bucket() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello world");
    let fake = Fake::new(vec![reply(201, UPLOAD)]);
    let step = Zenodo::sandbox(&fake)
        .upload(&stage(), &draft(), "b.zip", &path)
        .unwrap();
    assert_eq!(step.step, "upload");
    assert_eq!(step.method, "PUT");
    assert_eq!(step.url, format!("{BUCKET}/b.zip"));
    assert_eq!(step.status, 201);
    assert_eq!(
        step.detail,
        json!({"key": "b.zip", "size": 11, "sha256": WORLD_SHA, "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3"})
    );
    assert_eq!(
        fake.calls(),
        vec![call(
            Method::Put,
            &format!("{BUCKET}/b.zip"),
            Some("stage-token"),
            Body::File(path),
            true,
        )]
    );
}

#[test]
fn upload_accepts_200() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello world");
    let fake = Fake::new(vec![reply(200, UPLOAD)]);
    let step = Zenodo::sandbox(&fake)
        .upload(&stage(), &draft(), "b.zip", &path)
        .unwrap();
    assert_eq!(step.status, 200);
}

#[test]
fn upload_checks_the_size() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello worlds");
    let fake = Fake::new(vec![reply(201, UPLOAD)]);
    let err = Zenodo::sandbox(&fake)
        .upload(&stage(), &draft(), "b.zip", &path)
        .unwrap_err();
    assert!(
        matches!(err, Error::Mismatch { step: "upload", .. }),
        "{err:?}"
    );
}

#[test]
fn upload_checks_the_key() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello world");
    let fake = Fake::new(vec![reply(201, UPLOAD.replace("\"b.zip\",", "\"c.zip\","))]);
    let err = Zenodo::sandbox(&fake)
        .upload(&stage(), &draft(), "b.zip", &path)
        .unwrap_err();
    assert!(
        matches!(err, Error::Mismatch { step: "upload", .. }),
        "{err:?}"
    );
}

#[test]
fn upload_refuses_unsafe_names() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello world");
    let fake = Fake::new(vec![]);
    for name in ["", ".", "..", "a/b", "a b", "a\nb", "\u{e9}"] {
        let err = Zenodo::sandbox(&fake)
            .upload(&stage(), &draft(), name, &path)
            .unwrap_err();
        assert!(matches!(err, Error::Request(_)), "{name:?}: {err:?}");
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn upload_of_a_missing_file_is_an_io_error() {
    let fake = Fake::new(vec![]);
    let err = Zenodo::sandbox(&fake)
        .upload(&stage(), &draft(), "b.zip", Path::new("/no/such/file"))
        .unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    assert!(fake.calls().is_empty());
}

// verify

fn expected() -> Vec<(String, String)> {
    vec![
        ("b.zip".to_string(), WORLD_SHA.to_string()),
        ("a.txt".to_string(), HELLO_SHA.to_string()),
    ]
}

#[test]
fn verify_hashes_each_download() {
    let fake = Fake::new(vec![
        reply(200, BUCKET_LIST),
        download(WORLD_SHA, 11),
        download(HELLO_SHA, 5),
    ]);
    let step = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &expected())
        .unwrap();
    assert_eq!(step.step, "verify");
    assert_eq!(step.method, "GET");
    assert_eq!(step.url, BUCKET);
    assert_eq!(step.status, 200);
    assert_eq!(
        step.detail,
        json!({"files": [
            {"key": "b.zip", "sha256": WORLD_SHA},
            {"key": "a.txt", "sha256": HELLO_SHA}
        ]})
    );
    let st = Some("stage-token");
    assert_eq!(
        fake.calls(),
        vec![
            call(Method::Get, BUCKET, st, Body::None, true),
            call(
                Method::Get,
                &format!("{BUCKET}/b.zip"),
                st,
                Body::None,
                false
            ),
            call(
                Method::Get,
                &format!("{BUCKET}/a.txt"),
                st,
                Body::None,
                false
            ),
        ]
    );
}

#[test]
fn verify_finds_a_missing_file() {
    let fake = Fake::new(vec![reply(200, BUCKET_LIST)]);
    let mut want = expected();
    want.push(("c.txt".to_string(), HELLO_SHA.to_string()));
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &want)
        .unwrap_err();
    assert!(
        matches!(err, Error::Mismatch { step: "verify", .. }),
        "{err:?}"
    );
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn verify_finds_an_extra_file() {
    let fake = Fake::new(vec![reply(200, BUCKET_LIST)]);
    let want = vec![("b.zip".to_string(), WORLD_SHA.to_string())];
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &want)
        .unwrap_err();
    assert!(
        matches!(err, Error::Mismatch { step: "verify", .. }),
        "{err:?}"
    );
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn verify_finds_a_wrong_hash() {
    let fake = Fake::new(vec![
        reply(200, BUCKET_LIST),
        download(WORLD_SHA, 11),
        download(WORLD_SHA, 5),
    ]);
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &expected())
        .unwrap_err();
    assert!(
        matches!(err, Error::Mismatch { step: "verify", .. }),
        "{err:?}"
    );
}

#[test]
fn verify_rejects_a_bad_download_status() {
    let mut bad = download(HELLO_SHA, 0);
    bad.status = 404;
    let fake = Fake::new(vec![reply(200, BUCKET_LIST), bad]);
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &expected())
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Status {
                step: "verify",
                status: 404,
                ..
            }
        ),
        "{err:?}"
    );
}

// publish

#[test]
fn publish_uses_the_publish_token() {
    let fake = Fake::new(vec![reply(202, PUBLISH)]);
    let step = Zenodo::sandbox(&fake)
        .publish(&publish_token(), &draft())
        .unwrap();
    assert_eq!(step.step, "publish");
    assert_eq!(step.method, "POST");
    assert_eq!(
        step.url,
        "https://sandbox.zenodo.org/api/deposit/depositions/123456/actions/publish"
    );
    assert_eq!(step.status, 202);
    assert_eq!(
        step.detail,
        json!({"doi": DOI, "record_id": 123456, "url": "https://sandbox.zenodo.org/records/123456"})
    );
    assert_eq!(
        fake.calls(),
        vec![call(
            Method::Post,
            "https://sandbox.zenodo.org/api/deposit/depositions/123456/actions/publish",
            Some("publish-token"),
            Body::None,
            true,
        )]
    );
}

#[test]
fn publish_checks_the_doi() {
    let body = PUBLISH.replace(
        "\"doi\": \"10.5072/zenodo.123456\"",
        "\"doi\": \"10.5072/zenodo.5\"",
    );
    let fake = Fake::new(vec![reply(202, body)]);
    let err = Zenodo::sandbox(&fake)
        .publish(&publish_token(), &draft())
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Mismatch {
                step: "publish",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn publish_wants_202() {
    let fake = Fake::new(vec![reply(200, PUBLISH)]);
    let err = Zenodo::sandbox(&fake)
        .publish(&publish_token(), &draft())
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Status {
                step: "publish",
                status: 200,
                ..
            }
        ),
        "{err:?}"
    );
}

// the sandbox guard

#[test]
fn urls_outside_the_sandbox_are_refused() {
    let dir = TempDir::new().unwrap();
    let path = file_of(&dir, "hello world");
    let evil = CREATE.replace(BUCKET, "https://evil.example/files/x");
    let fake = Fake::new(vec![reply(201, evil)]);
    let z = Zenodo::sandbox(&fake);
    let (d, _) = z.create(&stage()).unwrap();
    assert_eq!(d.bucket, "https://evil.example/files/x");
    match z.upload(&stage(), &d, "b.zip", &path).unwrap_err() {
        Error::Protocol { step, what } => {
            assert_eq!(step, "upload");
            assert_eq!(
                what,
                "refusing a URL outside the sandbox: https://evil.example/files/x/b.zip"
            );
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn a_file_link_outside_the_sandbox_is_refused() {
    let list = BUCKET_LIST.replace(&format!("{BUCKET}/a.txt"), "https://evil.example/a.txt");
    let fake = Fake::new(vec![reply(200, list), download(WORLD_SHA, 11)]);
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &draft(), &expected())
        .unwrap_err();
    assert!(
        matches!(err, Error::Protocol { step: "verify", .. }),
        "{err:?}"
    );
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|c| c.url.starts_with("https://sandbox.zenodo.org/"))
    );
}

#[test]
fn a_look_alike_host_is_refused() {
    let fake = Fake::new(vec![]);
    let d = Draft {
        bucket: "https://sandbox.zenodo.org.evil.example/x".to_string(),
        ..draft()
    };
    let err = Zenodo::sandbox(&fake)
        .verify(&stage(), &d, &expected())
        .unwrap_err();
    assert!(
        matches!(err, Error::Protocol { step: "verify", .. }),
        "{err:?}"
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn a_refused_url_is_shown_escaped() {
    let fake = Fake::new(vec![]);
    let d = Draft {
        bucket: "https://evil.example/\x1b[2J".to_string(),
        ..draft()
    };
    match Zenodo::sandbox(&fake)
        .verify(&stage(), &d, &expected())
        .unwrap_err()
    {
        Error::Protocol { what, .. } => assert!(!what.contains('\x1b'), "{what:?}"),
        e => panic!("{e:?}"),
    }
}
