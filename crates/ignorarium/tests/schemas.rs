//! Each `--json` output against its file in `docs/schemas/`.
mod common;

use ignorarium::cli::schema;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn guide() -> Value {
    let a = common::bin(&common::lab())
        .args(["guide", "--json"])
        .assert()
        .success();
    serde_json::from_str(&common::text(&a)).unwrap()
}

#[test]
fn guide_json_is_an_object_with_its_schema_number() {
    let v = guide();
    assert_eq!(v["schema"], schema::GUIDE);
    assert_eq!(v["steps"].as_array().unwrap().len(), 13);
    common::schema::valid("guide", &v);
}

#[test]
#[should_panic(expected = "no schema file")]
fn a_number_without_a_file_fails() {
    common::schema::valid_as("guide", 99, &json!({ "schema": 99, "steps": [] }));
}

#[test]
#[should_panic(expected = "schema")]
fn a_key_the_file_does_not_name_fails() {
    let mut v = guide();
    v["steps"][0]["extra"] = json!(1);
    common::schema::valid("guide", &v);
}

#[test]
#[should_panic(expected = "schema")]
fn a_value_of_the_wrong_type_fails() {
    let mut v = guide();
    v["steps"][0]["title"] = json!(7);
    common::schema::valid("guide", &v);
}

#[test]
#[should_panic(expected = "schema")]
fn a_missing_key_fails() {
    let mut v = guide();
    v["steps"][0].as_object_mut().unwrap().remove("command");
    common::schema::valid("guide", &v);
}

fn printed(a: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&a.get_output().stdout).unwrap()
}

#[test]
fn check_json_fits_its_schema() {
    let lab = common::lab();
    common::plain(&lab, "s");
    let spec = common::work(&lab, "s").join("spec.toml");
    let v = printed(
        &common::bin(&lab)
            .arg("check")
            .arg(&spec)
            .arg("--json")
            .assert(),
    );
    assert_eq!(v["schema"], schema::CHECK);
    assert!(v["hash"].is_string(), "{v}");
    common::schema::valid("check", &v);
}

#[test]
fn the_check_error_shape_fits_its_schema() {
    let lab = common::lab();
    let spec = lab.root.join("bad.toml");
    std::fs::write(&spec, "[study\n").unwrap();
    let bad = common::bin(&lab)
        .arg("check")
        .arg(&spec)
        .arg("--json")
        .assert()
        .code(1);
    let missing = (common::bin(&lab).args(["check", "--json", "missing.toml"]))
        .assert()
        .code(2);
    for v in [printed(&bad), printed(&missing)] {
        assert_eq!(v["schema"], schema::CHECK);
        assert!(v["error"].is_string() && v["hash"].is_null(), "{v}");
        common::schema::valid("check", &v);
    }
}

#[test]
fn status_json_fits_its_schema_before_and_after_the_rows() {
    let lab = common::lab();
    common::plain(&lab, "s");
    let v = common::status(&lab, "s");
    assert_eq!(v["schema"], schema::STATUS);
    assert!(v["rows_per_hour"].is_null(), "{v}");
    (common::bin_at(&lab, "s").args(["run", "s"]))
        .assert()
        .success();
    common::status(&lab, "s");
}

#[test]
fn status_telemetry_json_fits_its_schema_before_and_after_the_rows() {
    let lab = common::lab();
    common::plain(&lab, "s");
    let v = common::telemetry(&lab, "s");
    assert_eq!(v["schema"], schema::STATUS_TELEMETRY);
    assert_eq!(v["telemetry"]["rows"], 0);
    let blocks = v["telemetry"]["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty());
    assert!(blocks.iter().all(|b| b["rows"] == 0), "{v}");
    (common::bin_at(&lab, "s").args(["run", "s"]))
        .assert()
        .success();
    assert_ne!(common::telemetry(&lab, "s")["telemetry"]["rows"], 0);
}

#[test]
fn every_output_has_its_file_and_every_file_has_its_output() {
    let want: BTreeSet<String> = (schema::OUTPUTS.iter())
        .map(|(name, n)| format!("{name}.v{n}.schema.json"))
        .chain(["README.md".to_string()])
        .collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schemas");
    let have: BTreeSet<String> = (std::fs::read_dir(dir).unwrap())
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(have, want);
}

#[test]
fn report_json_fits_its_schema() {
    let lab = common::lab();
    common::plain(&lab, "s");
    (common::bin_at(&lab, "s").args(["run", "s"]))
        .assert()
        .code(0);
    let a = (common::bin(&lab).args(["report", "s", "--json"]))
        .assert()
        .success();
    let v: Value = serde_json::from_str(&common::text(&a)).unwrap();
    assert_eq!(v["card_version"], ignorarium::card::CARD_VERSION);
    common::schema::valid("report", &v);
}
