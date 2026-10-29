use bundle::licence::{HEADER_BYTES, Input, Kind, Licensing, Record};
use bundle::manifest::Entry;

const H: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const H2: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn rec(spdx: &str, kind: Kind, source: &str, creators: &[&str]) -> Record {
    Record {
        spdx: spdx.into(),
        kind,
        source: source.into(),
        creators: creators.iter().map(|s| s.to_string()).collect(),
    }
}

fn input(path: &str, sha: &str, licence: Record) -> Input {
    Input {
        path: path.into(),
        sha256: sha.into(),
        licence,
    }
}

fn entry(path: &str, sha: &str) -> Entry {
    Entry {
        path: path.into(),
        sha256: sha.into(),
        size: 1,
    }
}

/// The leading bytes of each `src/` file, as the build hands them to the check.
fn heads<'a>(m: &'a [(&str, &'a [u8])]) -> impl Fn(&str) -> Option<&'a [u8]> + 'a {
    move |p| m.iter().find(|(q, _)| *q == p).map(|(_, b)| *b)
}

fn licensing(inputs: Vec<Input>) -> Licensing {
    Licensing {
        code: "Apache-2.0".into(),
        data: "CC0-1.0".into(),
        inputs,
    }
}

#[test]
fn of_prefers_the_input_record() {
    let l = licensing(vec![
        input(
            "./data/a.txt",
            H,
            rec("CC-BY-SA-4.0", Kind::Common, "s", &["A"]),
        ),
        input("../x", H, rec("MIT", Kind::Common, "s", &["A"])),
    ]);
    assert_eq!(l.of("data/a.txt"), "CC-BY-SA-4.0");
    assert_eq!(l.of("./data/a.txt"), "CC0-1.0");
    assert_eq!(l.of("src/run"), "Apache-2.0");
    assert_eq!(l.of("results/x"), "CC0-1.0");
    assert_eq!(l.of("../x"), "CC0-1.0");
    assert_eq!(l.of("x"), "CC0-1.0");
}

#[test]
fn unbundled_path_shapes_never_match() {
    for p in ["/abs", "a/../b", "a\\b", "a//b", "", "./", "a/"] {
        let l = licensing(vec![input(p, H, rec("MIT", Kind::Common, "s", &["A"]))]);
        assert_eq!(l.of(p), "CC0-1.0", "{p}");
        assert_eq!(l.of(p.trim_start_matches("./")), "CC0-1.0", "{p}");
    }
}

fn all_licences() -> Vec<Entry> {
    ["Apache-2.0", "CC0-1.0", "CC-BY-SA-4.0", "Custom-1"]
        .iter()
        .map(|i| entry(&format!("LICENSES/{i}.txt"), H))
        .collect()
}

#[test]
fn check_finds_every_problem() {
    let l = licensing(vec![
        input(
            "data/a.txt",
            H,
            rec("CC-BY-SA-4.0", Kind::Common, "", &["A"]),
        ),
        input("data/b.txt", H, rec("Custom-1", Kind::Custom, "s", &["A"])),
        input(
            "data/c.txt",
            H,
            rec("CC0-1.0", Kind::Unspecified, "s", &["A"]),
        ),
        input(
            "elsewhere/d.txt",
            H,
            rec("CC0-1.0", Kind::RequestForm, "s", &["A"]),
        ),
        input("data/e.txt", H, rec("CC0-1.0", Kind::Common, "s", &["A"])),
    ]);
    let mut files = vec![
        entry("data/a.txt", H),
        entry("data/b.txt", H),
        entry("data/c.txt", H),
        entry("data/e.txt", H2),
        entry("src/run", H),
        entry("src/blob", H),
    ];
    files.extend(
        all_licences()
            .into_iter()
            .filter(|e| !e.path.contains("CC-BY-SA")),
    );
    let src = [
        ("src/run", b"echo hi\n".as_slice()),
        ("src/blob", [0xFFu8, 0, 1].as_slice()),
    ];
    let (problems, warnings) = l.check(&files, &heads(&src));
    let got: Vec<(&str, &str)> = problems.iter().map(|p| (p.code, p.path.as_str())).collect();
    assert_eq!(
        got,
        vec![
            ("E-PUB-005", "inputs[0].licence"),
            ("E-PUB-005", "inputs[1].licence"),
            ("E-PUB-006", "data/c.txt"),
            ("E-PUB-011", "data/e.txt"),
            ("E-PUB-007", "LICENSES/CC-BY-SA-4.0.txt"),
        ]
    );
    assert_eq!(
        problems[0].instruction,
        "Fill the input's licence record: an SPDX id, the source URL and every creator; a custom licence uses a LicenseRef- id."
    );
    assert_eq!(
        problems[2].instruction,
        "Leave the file out of the bundle and cite its source, or record a licence that allows sharing it."
    );
    assert_eq!(
        problems[3].instruction,
        "Restore the registered file, or register an amendment with the new hash."
    );
    assert_eq!(
        problems[4].instruction,
        "Add the licence text as LICENSES/CC-BY-SA-4.0.txt (https://reuse.software/spec-3.3/)."
    );
    let got: Vec<(&str, &str)> = warnings.iter().map(|p| (p.code, p.path.as_str())).collect();
    assert_eq!(
        got,
        vec![("W-PUB-001", "inputs[3]"), ("W-PUB-002", "src/run")]
    );
    assert_eq!(
        warnings[0].instruction,
        "Check that the published results may use this input."
    );
    assert_eq!(
        warnings[1].instruction,
        "Add an SPDX-License-Identifier line near the top of the file."
    );
}

#[test]
fn check_accepts_a_clean_set() {
    let l = licensing(vec![
        input(
            "./data/a.txt",
            &H.to_uppercase(),
            rec("CC-BY-SA-4.0", Kind::Common, "s", &["A"]),
        ),
        input("cited.txt", H, rec("CC0-1.0", Kind::Common, "s", &["A"])),
        input(
            "../cited2",
            H,
            rec("LicenseRef-x", Kind::Custom, "s", &["A"]),
        ),
    ]);
    let mut files = all_licences();
    files.push(entry("LICENSES/LicenseRef-x.txt", H));
    files.push(entry("data/a.txt", H));
    files.push(entry("src/run", H));
    let src = [(
        "src/run",
        b"# SPDX-License-Identifier: Apache-2.0\n".as_slice(),
    )];
    assert_eq!(l.check(&files, &heads(&src)), (vec![], vec![]));
}

#[test]
fn blank_creator_and_blank_id_are_refused() {
    let l = licensing(vec![
        input("x", H, rec("CC0-1.0", Kind::Common, "s", &[])),
        input("y", H, rec("CC0-1.0", Kind::Common, "s", &[" "])),
        input("z", H, rec(" ", Kind::Common, "s", &["A"])),
    ]);
    let (p, _) = l.check(&all_licences(), &|_| None);
    let paths: Vec<&str> = p.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "inputs[0].licence",
            "inputs[1].licence",
            "inputs[2].licence"
        ]
    );
}

#[test]
fn display_text_is_escaped() {
    let l = licensing(vec![input(
        "d/\u{1b}x",
        H,
        rec("Custom\u{202e}1", Kind::Unspecified, "s\u{1b}", &["A"]),
    )]);
    let files = vec![entry("d/\u{1b}x", H)];
    let (p, _) = l.check(&files, &|_| None);
    for pr in &p {
        let all = format!("{} {} {}", pr.path, pr.message, pr.instruction);
        assert!(
            !all.contains('\u{1b}') && !all.contains('\u{202e}'),
            "{all}"
        );
    }
    assert!(p.iter().any(|x| x.path == "d/\\u{1b}x"));
    assert!(p.iter().any(|x| x.path == "LICENSES/Custom\\u{202e}1.txt"));
}

fn header_at(line: usize) -> String {
    let mut s = String::new();
    for i in 1..line {
        s.push_str(&format!("line {i}\n"));
    }
    s.push_str("# SPDX-License-Identifier: Apache-2.0\n");
    s
}

#[test]
fn spdx_header_must_be_in_the_first_ten_lines() {
    let (ten, eleven) = (header_at(10), header_at(11));
    let mut files = all_licences();
    files.push(entry("src/ten", H));
    files.push(entry("src/eleven", H));
    let src = [
        ("src/ten", ten.as_bytes()),
        ("src/eleven", eleven.as_bytes()),
    ];
    let (p, w) = licensing(vec![]).check(&files, &heads(&src));
    assert!(p.is_empty());
    assert_eq!(w.len(), 1);
    assert_eq!((w[0].code, w[0].path.as_str()), ("W-PUB-002", "src/eleven"));
}

#[test]
fn a_multibyte_char_cut_at_the_read_limit_is_still_text() {
    let mut files = all_licences();
    files.push(entry("src/cut", H));
    let warnings = |head: &[u8]| {
        let src = [("src/cut", head)];
        licensing(vec![]).check(&files, &heads(&src)).1
    };
    // The text, `a` up to 65,535 bytes, then the first byte of the two-byte `é`.
    let cut = |text: &[u8]| {
        let mut b = text.to_vec();
        b.resize(HEADER_BYTES - 1, b'a');
        b.push(0xC3);
        b
    };
    assert!(warnings(&cut(b"# SPDX-License-Identifier: MIT\n")).is_empty());
    let w = warnings(&cut(b""));
    assert_eq!(
        w.iter()
            .map(|p| (p.code, p.path.as_str()))
            .collect::<Vec<_>>(),
        [("W-PUB-002", "src/cut")]
    );
    // Invalid bytes inside the window still mean binary: no warning either.
    assert!(warnings(b"x\n\xFF").is_empty());
    // Plain text without a header warns.
    assert_eq!(warnings(b"x\n").len(), 1);
    // A file the build gave no head for is not checked.
    assert!(licensing(vec![]).check(&files, &|_| None).1.is_empty());
}

const HEAD: &str = "# Licences

Files under `src/` are licensed under Apache-2.0 (`LICENSES/Apache-2.0.txt`).
All other files are licensed under CC0-1.0 (`LICENSES/CC0-1.0.txt`), except the inputs below.
CC0-1.0 covers only the rights of this study's authors.

## Inputs

These files came from others and keep their own licences.

";

#[test]
fn statement_lists_bundled_inputs() {
    let l = licensing(vec![
        input(
            "data/z|.txt",
            H,
            rec("MIT", Kind::Common, "https://example.org/z\r\nmore", &["Z"]),
        ),
        input(
            "./data/a.txt",
            H,
            rec(
                "CC-BY-SA-4.0",
                Kind::Common,
                "https://example.org/a",
                &["Ada Example", "Bo Example"],
            ),
        ),
        input("cited.txt", H, rec("MIT", Kind::Common, "s", &["Q"])),
    ]);
    let files = vec![entry("data/a.txt", H), entry("data/z|.txt", H)];
    let want = format!(
        "{HEAD}| File | Licence | Source | Creators |\n|---|---|---|---|\n\
| `data/a.txt` | CC-BY-SA-4.0 | https://example.org/a | Ada Example; Bo Example |\n\
| `data/z\\|.txt` | MIT | https://example.org/z more | Z |\n"
    );
    assert_eq!(l.statement(&files), want);
}

#[test]
fn statement_without_inputs() {
    let l = licensing(vec![input(
        "cited.txt",
        H,
        rec("MIT", Kind::Common, "s", &["Q"]),
    )]);
    let want = format!("{HEAD}None of the files came from others.\n");
    assert_eq!(l.statement(&[entry("data/a.txt", H)]), want);
}
