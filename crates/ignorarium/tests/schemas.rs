//! Each `--json` output against its file in `docs/schemas/`.
mod common;

use ignorarium::cli::schema;
use serde_json::{Value, json};

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
