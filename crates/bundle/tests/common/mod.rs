use bundle::Facts;
use bundle::citation::{Release, Tool};
use bundle::licence::{Input, Kind, Licensing, Record};
use bundle::manifest::sha256_bytes;
use bundle::rocrate::{Image, Item, Run, Status};
use std::path::PathBuf;

pub const INPUT: &str = "data/texts/a.txt";

pub fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/studies/numeric")
}

pub fn release() -> Release {
    Release {
        version: "1.0.0".into(),
        date: "2026-10-29".into(),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        doi: Some("10.5072/zenodo.123456".into()),
        publisher: "Zenodo".into(),
    }
}

pub fn tool() -> Tool {
    Tool {
        name: "ignorarium".into(),
        version: "0.0.1".into(),
        url: "https://github.com/Phlares/ignoramus".into(),
        author: "The ignorarium contributors".into(),
    }
}

pub fn input_sha256() -> String {
    sha256_bytes(b"Fixture text A, written for the bundle tests.\n")
}

pub fn facts() -> Facts {
    Facts {
        study_id: "fixture-numeric".into(),
        label: "signal".into(),
        licensing: Licensing {
            code: "Apache-2.0".into(),
            data: "CC0-1.0".into(),
            inputs: vec![Input {
                path: INPUT.into(),
                sha256: input_sha256(),
                licence: Record {
                    spdx: "CC-BY-SA-4.0".into(),
                    kind: Kind::Common,
                    source: "https://example.org/texts/a".into(),
                    creators: vec!["Example Creator".into()],
                },
            }],
        },
        tool: tool(),
        runs: vec![
            Run {
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
            },
            Run {
                id: "row-2".into(),
                name: "Row 2".into(),
                start: "2026-10-29T10:02:00Z".into(),
                end: "2026-10-29T10:03:00Z".into(),
                status: Status::Failed("payload exited with status 1".into()),
                object: vec![],
                result: vec![Item::Hash {
                    name: "out".into(),
                    sha256: "a".repeat(64),
                }],
                image: None,
                env: vec![],
            },
        ],
        attachments: vec![(
            "records/planlog-head.json".into(),
            b"{\"size\": 1}\n".to_vec(),
        )],
        artifacts: vec![],
    }
}
