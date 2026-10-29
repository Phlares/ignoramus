use bundle::citation::{
    self, Author, Citation, RefKind, Reference, Release, Tool, check_release, is_doi,
};
use bundle::yaml;
use serde_json::{Value, json};

fn paths(ps: &[bundle::Problem]) -> Vec<String> {
    ps.iter().map(|p| p.path.clone()).collect()
}

#[test]
fn block_prints_nested_values() {
    let v = json!({"b": [1, {"x": "y", "z": []}], "a": "1.10", "c": {}, "d": [[true, null]], "Key Two": 2});
    assert_eq!(
        yaml::block(&v),
        "\"Key Two\": 2\na: \"1.10\"\nb:\n  - 1\n  - x: \"y\"\n    z: []\nc: {}\nd:\n  -\n    - true\n    - null\n"
    );
}

#[test]
fn block_prints_top_level_scalars_and_numbers() {
    assert_eq!(yaml::block(&json!("a")), "\"a\"\n");
    assert_eq!(yaml::block(&json!(1.5)), "1.5\n");
    assert_eq!(yaml::block(&json!(false)), "false\n");
    assert_eq!(yaml::block(&json!({})), "{}\n");
    assert_eq!(
        yaml::block(&json!({"a-1": 7, "a_1": 8})),
        "a-1: 7\n\"a_1\": 8\n"
    );
}

#[test]
fn block_prints_top_level_sequences() {
    assert_eq!(yaml::block(&json!([1])), "- 1\n");
    assert_eq!(yaml::block(&json!([])), "[]\n");
    assert_eq!(yaml::block(&json!([[1]])), "-\n  - 1\n");
}

#[test]
fn block_indents_nested_maps_and_sequences() {
    assert_eq!(yaml::block(&json!({"key": {"b": 1}})), "key:\n  b: 1\n");
    assert_eq!(
        yaml::block(&json!({"key": [{}, []]})),
        "key:\n  - {}\n  - []\n"
    );
    assert_eq!(
        yaml::block(&json!({"refs": [{"names": [{"n": "x"}]}]})),
        "refs:\n  - names:\n      - n: \"x\"\n"
    );
}

#[test]
fn block_quotes_yaml_special_text() {
    let cases = [
        ("no", "v: \"no\"\n"),
        ("1.10", "v: \"1.10\"\n"),
        ("@home", "v: \"@home\"\n"),
        ("a: b # c", "v: \"a: b # c\"\n"),
        ("say \"hi\"\\", "v: \"say \\\"hi\\\"\\\\\"\n"),
        ("Zoë Müller 李", "v: \"Zoë Müller 李\"\n"),
        ("tab\there", "v: \"tab\\there\"\n"),
        ("\u{85}", "v: \"\\u0085\"\n"),
        ("\u{1}", "v: \"\\u0001\"\n"),
        ("\u{7f}\u{ffff}\u{fffe}", "v: \"\\u007f\\uffff\\ufffe\"\n"),
        ("a\nb\rc", "v: \"a\\nb\\rc\"\n"),
    ];
    for (input, want) in cases {
        assert_eq!(yaml::block(&json!({"v": input})), want, "input {input:?}");
    }
}

const FULL: &str = r#"
title = "Study"
message = "Cite this."
abstract = "What we did."
keywords = ["a", "b"]

[[authors]]
family-names = "Example"
given-names = "Ada"
orcid = "https://orcid.org/0000-0002-1825-0097"
affiliation = "Lab"

[[authors]]
name = "Team"

[[references]]
type = "data"
title = "Data"
doi = "10.5072/zenodo.1"

[[references.authors]]
name = "C"
"#;

#[test]
fn parse_reads_a_full_citation() {
    let c = citation::parse(FULL).unwrap();
    assert_eq!(c.title, "Study");
    assert_eq!(c.message, "Cite this.");
    assert_eq!(c.summary, "What we did.");
    assert_eq!(c.keywords, vec!["a", "b"]);
    assert_eq!(c.authors.len(), 2);
    assert_eq!(c.authors[0].family_names.as_deref(), Some("Example"));
    assert_eq!(c.authors[0].given_names.as_deref(), Some("Ada"));
    assert_eq!(
        c.authors[0].orcid.as_deref(),
        Some("https://orcid.org/0000-0002-1825-0097")
    );
    assert_eq!(c.authors[0].affiliation.as_deref(), Some("Lab"));
    assert_eq!(c.authors[1].name.as_deref(), Some("Team"));
    assert_eq!(c.references.len(), 1);
    assert_eq!(c.references[0].kind, RefKind::Data);
    assert_eq!(c.references[0].doi.as_deref(), Some("10.5072/zenodo.1"));
    assert_eq!(c.references[0].url, None);
}

#[test]
fn parse_reports_every_problem_at_once() {
    let text = r#"
title = " "
message = "M"
abstract = "A"

[[authors]]
name = "Team"
family-names = "Example"

[[authors]]
family-names = "Example"
given-names = "Ada"
orcid = "0000-0002-1825-0097"

[[references]]
type = "data"
title = "D"
[[references.authors]]
name = "C"
"#;
    let ps = citation::parse(text).unwrap_err();
    assert_eq!(
        paths(&ps),
        [
            "citation.toml:title",
            "citation.toml:authors[0]",
            "citation.toml:authors[1].orcid",
            "citation.toml:references[0]",
        ]
    );
    for p in &ps {
        assert_eq!(p.code, "E-PUB-004");
        assert_eq!(
            p.instruction,
            "Fix citation.toml; its keys are the Citation File Format 1.2.0 keys (https://github.com/citation-file-format/citation-file-format/blob/main/schema-guide.md)."
        );
    }
}

#[test]
fn parse_refuses_unknown_keys_and_bad_toml() {
    let ps = citation::parse(&format!("licence = \"x\"\n{FULL}")).unwrap_err();
    assert_eq!(paths(&ps), ["citation.toml"]);
    assert_eq!(ps[0].code, "E-PUB-004");
    let ps = citation::parse("title = \"\u{202e}x").unwrap_err();
    assert_eq!(paths(&ps), ["citation.toml"]);
    assert!(!ps[0].message.contains('\u{202e}'));
}

#[test]
fn parse_checks_authors_and_references() {
    let text = r#"
title = "T"
message = "M"
abstract = " "
authors = []
[[references]]
type = "software"
title = ""
url = "ftp://x"
doi = "10.1/x"
authors = []
"#;
    let ps = citation::parse(text).unwrap_err();
    assert_eq!(
        paths(&ps),
        [
            "citation.toml:abstract",
            "citation.toml:authors",
            "citation.toml:references[0].title",
            "citation.toml:references[0].authors",
            "citation.toml:references[0].doi",
            "citation.toml:references[0].url",
        ]
    );
    let text = r#"
title = "T"
message = "M"
abstract = "A"
[[authors]]
given-names = "Ada"
[[authors]]
name = "T"
orcid = "https://orcid.org/0000-0002-1825-009X"
[[authors]]
name = "U"
orcid = "https://orcid.org/0000-0002-1825-00X7"
[[references]]
type = "article"
title = "R"
url = "https://example.org"
[[references.authors]]
name = " "
"#;
    let ps = citation::parse(text).unwrap_err();
    assert_eq!(
        paths(&ps),
        [
            "citation.toml:authors[0]",
            "citation.toml:authors[2].orcid",
            "citation.toml:references[0].authors[0]",
        ]
    );
}

#[test]
fn load_reports_a_read_error() {
    let d = tempfile::tempdir().unwrap();
    let ps = citation::load(&d.path().join("citation.toml")).unwrap_err();
    assert_eq!(paths(&ps), ["citation.toml"]);
    assert_eq!(ps[0].code, "E-PUB-004");
    std::fs::write(d.path().join("citation.toml"), FULL).unwrap();
    assert_eq!(
        citation::load(&d.path().join("citation.toml")).unwrap(),
        citation::parse(FULL).unwrap()
    );
    std::fs::write(d.path().join("citation.toml"), b"title = \"\xFF\"\n").unwrap();
    let ps = citation::load(&d.path().join("citation.toml")).unwrap_err();
    assert_eq!(paths(&ps), ["citation.toml"]);
    assert_eq!(ps[0].message, "is not UTF-8 text");
    assert_eq!(
        citation::parse_bytes(FULL.as_bytes()).unwrap(),
        citation::parse(FULL).unwrap()
    );
}

#[test]
fn is_doi_bounds() {
    for ok in ["10.5072/zenodo.123456", "10.1234/x", "10.123456789/x"] {
        assert!(is_doi(ok), "{ok}");
    }
    for bad in [
        "10.123/x",
        "10.1234567890/x",
        "10.1234/",
        "11.1234/x",
        "10.1234/a b",
        "10.1234",
        "10.12a4/x",
        "10.1234/a\tb",
    ] {
        assert!(!is_doi(bad), "{bad}");
    }
}

fn release() -> Release {
    Release {
        version: "1.0.0".into(),
        date: "2026-10-29".into(),
        commit: Some("0".repeat(40)),
        doi: Some("10.5072/zenodo.9".into()),
        publisher: "Zenodo".into(),
    }
}

#[test]
fn check_release_bounds() {
    assert!(check_release(&release()).is_empty());
    let bad = Release {
        version: "1.0 beta".into(),
        date: "2026-13-01".into(),
        commit: Some("ABC".into()),
        doi: Some("doi:10.1/x".into()),
        publisher: " ".into(),
    };
    let ps = check_release(&bad);
    assert_eq!(
        paths(&ps),
        [
            "release.version",
            "release.date",
            "release.commit",
            "release.doi",
            "release.publisher"
        ]
    );
    for p in &ps {
        assert_eq!(p.code, "E-PUB-013");
        assert_eq!(
            p.instruction,
            "Use a version of letters, digits, dots and hyphens, a YYYY-MM-DD date, a 40-character lowercase hex commit, a DOI that starts with 10. and a publisher name."
        );
    }
    let date = |d: &str| {
        check_release(&Release {
            date: d.into(),
            ..release()
        })
    };
    assert!(date("2026-12-31").is_empty());
    assert!(date("2026-01-01").is_empty());
    assert_eq!(date("2026-00-10").len(), 1);
    assert_eq!(date("2026-01-32").len(), 1);
    assert_eq!(date("2026-1-032").len(), 1);
    assert_eq!(date("2026/01/01").len(), 1);
    assert_eq!(date("").len(), 1);
    let version = |v: &str| {
        check_release(&Release {
            version: v.into(),
            ..release()
        })
    };
    assert!(version("1.0.0-rc.1").is_empty());
    assert_eq!(version("").len(), 1);
    assert_eq!(version("1_0").len(), 1);
    let commit = |c: &str| {
        check_release(&Release {
            commit: Some(c.into()),
            ..release()
        })
    };
    assert!(commit(&"abcdef0123".repeat(4)).is_empty());
    assert_eq!(commit(&"a".repeat(39)).len(), 1);
    assert_eq!(commit(&"a".repeat(41)).len(), 1);
    assert_eq!(commit(&"g".repeat(40)).len(), 1);
    let none = Release {
        commit: None,
        doi: None,
        ..release()
    };
    assert!(check_release(&none).is_empty());
}

fn author(family: &str, given: &str) -> Author {
    Author {
        family_names: Some(family.into()),
        given_names: Some(given.into()),
        name: None,
        orcid: None,
        affiliation: None,
    }
}

fn entity(name: &str) -> Author {
    Author {
        family_names: None,
        given_names: None,
        name: Some(name.into()),
        orcid: None,
        affiliation: None,
    }
}

fn fixture() -> Citation {
    Citation {
        title: "T".into(),
        message: "M".into(),
        summary: "A".into(),
        keywords: vec!["k".into()],
        authors: vec![
            Author {
                orcid: Some("https://orcid.org/0000-0002-1825-0097".into()),
                affiliation: Some("Lab".into()),
                ..author("Example", "Ada")
            },
            entity("Team"),
        ],
        references: vec![Reference {
            kind: RefKind::Data,
            title: "D".into(),
            authors: vec![entity("C")],
            doi: Some("10.5072/zenodo.1".into()),
            url: None,
            version: None,
        }],
    }
}

fn tool() -> Tool {
    Tool {
        name: "ignorarium".into(),
        version: "0.0.1".into(),
        url: "https://github.com/Phlares/ignoramus".into(),
        author: "The ignorarium contributors".into(),
    }
}

fn want_cff() -> Value {
    json!({"abstract":"A","authors":[{"affiliation":"Lab","family-names":"Example","given-names":"Ada","orcid":"https://orcid.org/0000-0002-1825-0097"},{"name":"Team"}],"cff-version":"1.2.0","commit":"0000000000000000000000000000000000000000","date-released":"2026-10-29","doi":"10.5072/zenodo.9","keywords":["k"],"license":"CC0-1.0","message":"M","references":[{"authors":[{"name":"The ignorarium contributors"}],"title":"ignorarium","type":"software","url":"https://github.com/Phlares/ignoramus","version":"0.0.1"},{"authors":[{"name":"C"}],"doi":"10.5072/zenodo.1","title":"D","type":"data"}],"title":"T","type":"dataset","version":"1.0.0"})
}

#[test]
fn cff_renders_the_release() {
    let got = citation::cff(&fixture(), &release(), &tool(), "CC0-1.0");
    assert_eq!(got, want_cff());

    let r = Release {
        commit: None,
        doi: None,
        ..release()
    };
    let c = Citation {
        keywords: vec![],
        ..fixture()
    };
    let got = citation::cff(&c, &r, &tool(), "CC0-1.0");
    let m = got.as_object().unwrap();
    assert!(!m.contains_key("commit"));
    assert!(!m.contains_key("doi"));
    assert!(!m.contains_key("keywords"));
}

#[test]
fn cff_renders_reference_kinds_and_optional_fields() {
    let mut c = fixture();
    c.references = vec![Reference {
        kind: RefKind::Article,
        title: "P".into(),
        authors: vec![author("Doe", "Jo")],
        doi: None,
        url: Some("https://example.org/p".into()),
        version: Some("2".into()),
    }];
    let got = citation::cff(&c, &release(), &tool(), "CC0-1.0");
    assert_eq!(
        got["references"][1],
        json!({"authors":[{"family-names":"Doe","given-names":"Jo"}],"title":"P","type":"article","url":"https://example.org/p","version":"2"})
    );
}

#[test]
fn datacite_maps_the_same_citation() {
    let got = citation::datacite(&fixture(), &release(), &tool(), "CC0-1.0");
    let want = json!({"creators":[{"affiliation":[{"name":"Lab"}],"familyName":"Example","givenName":"Ada","name":"Example, Ada","nameIdentifiers":[{"nameIdentifier":"https://orcid.org/0000-0002-1825-0097","nameIdentifierScheme":"ORCID","schemeUri":"https://orcid.org"}],"nameType":"Personal"},{"name":"Team","nameType":"Organizational"}],"dates":[{"date":"2026-10-29","dateType":"Issued"}],"descriptions":[{"description":"A","descriptionType":"Abstract"}],"doi":"10.5072/zenodo.9","publicationYear":2026,"publisher":{"name":"Zenodo"},"relatedIdentifiers":[{"relatedIdentifier":"https://github.com/Phlares/ignoramus","relatedIdentifierType":"URL","relationType":"References"},{"relatedIdentifier":"10.5072/zenodo.1","relatedIdentifierType":"DOI","relationType":"References"}],"rightsList":[{"rights":"CC0-1.0","rightsIdentifier":"cc0-1.0","rightsIdentifierScheme":"SPDX","rightsUri":"https://spdx.org/licenses/CC0-1.0.html","schemeUri":"https://spdx.org/licenses/"}],"schemaVersion":"http://datacite.org/schema/kernel-4","subjects":[{"subject":"k"}],"titles":[{"title":"T"}],"types":{"resourceType":"Study bundle","resourceTypeGeneral":"Dataset"},"version":"1.0.0"});
    assert_eq!(got, want);
}

#[test]
fn datacite_handles_url_references_and_absent_fields() {
    let mut c = fixture();
    c.keywords = vec![];
    c.authors = vec![author("Doe", "Jo")];
    c.references[0].doi = None;
    c.references[0].url = Some("https://example.org/d".into());
    let r = Release {
        doi: None,
        date: "bad".into(),
        ..release()
    };
    let got = citation::datacite(&c, &r, &tool(), "CC0-1.0");
    let m = got.as_object().unwrap();
    assert!(!m.contains_key("doi"));
    assert!(!m.contains_key("subjects"));
    assert_eq!(got["publicationYear"], json!(0));
    assert_eq!(
        got["creators"],
        json!([{"familyName":"Doe","givenName":"Jo","name":"Doe, Jo","nameType":"Personal"}])
    );
    assert_eq!(
        got["relatedIdentifiers"][1],
        json!({"relatedIdentifier":"https://example.org/d","relatedIdentifierType":"URL","relationType":"References"})
    );
}

#[test]
fn a_dataset_reference_type_is_refused() {
    let text = r#"
title = "T"
message = "M"
abstract = "A"

[[authors]]
name = "Team"

[[references]]
type = "dataset"
title = "D"
doi = "10.5072/zenodo.1"
[[references.authors]]
name = "C"
"#;
    let ps = citation::parse(text).unwrap_err();
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0].code, "E-PUB-004");
    assert_eq!(ps[0].path, "citation.toml");
}
