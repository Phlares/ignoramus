use serde_json::Value;
use std::collections::BTreeMap;

/// Compares the verdicts in a box report with the expected ones. `Err` lists each difference.
///
/// The report has `hypotheses`: entries with string fields `hypothesis` and `verdict`.
/// `expected` is one object of id to verdict.
pub fn verify(report: &str, expected: &str) -> Result<(), Vec<String>> {
    let found = verdicts(report).map_err(|e| vec![format!("report: {e}")])?;
    let want = expected_map(expected).map_err(|e| vec![format!("expected: {e}")])?;
    let mut diffs = Vec::new();
    for (id, w) in &want {
        match found.get(id) {
            Some(f) if f == w => {}
            Some(f) => diffs.push(format!("{id}: expected {w}, found {f}")),
            None => diffs.push(format!("{id}: expected {w}, missing from the report")),
        }
    }
    for (id, f) in &found {
        if !want.contains_key(id) {
            diffs.push(format!("{id}: found {f}, not in the expected verdicts"));
        }
    }
    if diffs.is_empty() { Ok(()) } else { Err(diffs) }
}

fn verdicts(report: &str) -> Result<BTreeMap<String, String>, String> {
    let json: Value = serde_json::from_str(report).map_err(|e| e.to_string())?;
    let list = json
        .get("hypotheses")
        .and_then(Value::as_array)
        .ok_or("no hypotheses array")?;
    let mut out = BTreeMap::new();
    for h in list {
        let field = |k: &str| h.get(k).and_then(Value::as_str);
        let (Some(id), Some(verdict)) = (field("hypothesis"), field("verdict")) else {
            return Err("a hypothesis lacks a string hypothesis or verdict".into());
        };
        if out.insert(id.to_string(), verdict.to_string()).is_some() {
            return Err(format!("hypothesis {id} appears twice"));
        }
    }
    Ok(out)
}

fn expected_map(expected: &str) -> Result<BTreeMap<String, String>, String> {
    let json: Value = serde_json::from_str(expected).map_err(|e| e.to_string())?;
    let object = json.as_object().ok_or("not a JSON object")?;
    object
        .iter()
        .map(|(k, v)| match v.as_str() {
            Some(s) => Ok((k.clone(), s.to_string())),
            None => Err(format!("verdict of {k} is not a string")),
        })
        .collect()
}
