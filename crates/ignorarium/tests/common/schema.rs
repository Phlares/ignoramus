//! The check of a `--json` output against its file in `docs/schemas/`.
use serde_json::{Value, json};
use std::path::PathBuf;

/// Check `v` against the file of its own number: `card_version` for `report`, else `schema`.
pub fn valid(output: &str, v: &Value) {
    let key = if output == "report" {
        "card_version"
    } else {
        "schema"
    };
    let n = v
        .as_object()
        .unwrap_or_else(|| panic!("{output} --json is not an object: {v}"))
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{output} --json holds no number under {key}: {v}"));
    valid_as(output, n, v);
}

/// Check `v` against `docs/schemas/<output>.v<n>.schema.json`, read closed.
pub fn valid_as(output: &str, n: u64, v: &Value) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/schemas")
        .join(format!("{output}.v{n}.schema.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no schema file {}: {e}", path.display()));
    let file: Value = serde_json::from_str(&text).unwrap();
    let validator = jsonschema::draft202012::new(&closed(file))
        .unwrap_or_else(|e| panic!("{} is not a schema: {e}", path.display()));
    let errors: Vec<String> = validator
        .iter_errors(v)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "schema {output} v{n}:\n{}\n{v}",
        errors.join("\n")
    );
}

/// The files leave objects open, so a new key keeps the number. The test closes each object
/// that names its keys, so a key the file does not name fails.
fn closed(schema: Value) -> Value {
    match schema {
        Value::Object(mut m) => {
            for x in m.values_mut() {
                *x = closed(x.take());
            }
            if m.contains_key("properties") && !m.contains_key("additionalProperties") {
                m.insert("additionalProperties".into(), json!(false));
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(closed).collect()),
        x => x,
    }
}
