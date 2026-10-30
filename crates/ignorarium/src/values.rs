//! The values a row reports. The payload writes `values.json` in its output folder: one JSON
//! object with one finite number for each measure of the spec. The file is untrusted output.
use crate::backend::output::open_regular;
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

pub const FILE: &str = "values.json";
/// The most bytes the box reads from the file.
pub const LIMIT: u64 = 65_536;

/// The object's entries in file order, so that a repeated key can be seen.
struct Entries(Vec<(String, f64)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entries;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Entries, A::Error> {
                let mut out = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    out.push((k, m.next_value::<f64>()?));
                }
                Ok(Entries(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// The value of each of `measures` in `out_dir/values.json`. Refused: a file that is not a
/// regular file, one over `LIMIT` bytes, text that is not one JSON object of numbers, a
/// non-finite number, a repeated key, and any key that is not a measure or any measure
/// that is missing.
pub fn read(out_dir: &Path, measures: &[&str]) -> Result<BTreeMap<String, f64>, String> {
    let path = out_dir.join(FILE);
    let mut file = match open_regular(&path) {
        Ok(Some(f)) => f,
        Ok(None) => return Err(format!("{FILE} is not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{FILE} is missing"));
        }
        Err(e) => return Err(format!("cannot open {FILE}: {e}")),
    };
    let mut bytes = Vec::new();
    (&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {FILE}: {e}"))?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!("{FILE} is larger than {LIMIT} bytes"));
    }
    let Entries(entries) = serde_json::from_slice(&bytes).map_err(|e| {
        if e.is_data() {
            format!("{FILE} must be one object of numbers: {e}")
        } else {
            format!("{FILE} is not valid JSON: {e}")
        }
    })?;
    let mut got = BTreeMap::new();
    for (k, v) in entries {
        if !v.is_finite() {
            return Err(format!("{k} is not a finite number"));
        }
        if !measures.contains(&k.as_str()) {
            return Err(format!("unknown measure {k}"));
        }
        if got.insert(k.clone(), v).is_some() {
            return Err(format!("measure {k} is given twice"));
        }
    }
    match measures.iter().find(|m| !got.contains_key(**m)) {
        Some(m) => Err(format!("missing measure {m}")),
        None => Ok(got),
    }
}
