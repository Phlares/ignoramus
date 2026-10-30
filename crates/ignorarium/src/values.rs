//! The values a row reports. The payload writes `values.json` in its output folder: one JSON
//! object with one finite number for each measure of the spec. The file is untrusted output.
use crate::backend::output::{hash_rows, open_regular, tree_rows};
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use sha2::{Digest, Sha256};
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

/// At most 80 characters of payload text, for a reason that goes into a record.
fn clip(s: &str) -> String {
    match s.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// `read`, for a row whose output hashed to `output_hash`: refused when the folder no
/// longer hashes to it, or when the bytes parsed are not the bytes the hash covered.
pub fn read_covered(
    out_dir: &Path,
    measures: &[&str],
    output_hash: &str,
) -> Result<BTreeMap<String, f64>, String> {
    let (values, digest) = read(out_dir, measures)?;
    let rows = tree_rows(out_dir).map_err(|e| format!("cannot hash output: {e}"))?;
    let covered = rows.iter().any(|r| r.path == FILE && r.digest == digest);
    let same = hash_rows(&rows).is_ok_and(|h| h == output_hash);
    if covered && same {
        Ok(values)
    } else {
        Err(format!("{FILE} changed after the output was hashed"))
    }
}

/// The value of each of `measures` in `out_dir/values.json`. Refused: a file that is not a
/// regular file, one over `LIMIT` bytes, text that is not one JSON object of numbers, a
/// non-finite number, a repeated key, and any key that is not a measure or any measure
/// that is missing.
pub fn read(out_dir: &Path, measures: &[&str]) -> Result<(BTreeMap<String, f64>, String), String> {
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
            format!(
                "{FILE} must be one object of numbers: {}",
                clip(&e.to_string())
            )
        } else {
            format!("{FILE} is not valid JSON: {}", clip(&e.to_string()))
        }
    })?;
    let mut got = BTreeMap::new();
    for (k, v) in entries {
        if !v.is_finite() {
            return Err(format!("{} is not a finite number", clip(&k)));
        }
        if !measures.contains(&k.as_str()) {
            return Err(format!("unknown measure {}", clip(&k)));
        }
        if got.insert(k.clone(), v).is_some() {
            return Err(format!("measure {} is given twice", clip(&k)));
        }
    }
    match measures.iter().find(|m| !got.contains_key(**m)) {
        Some(m) => Err(format!("missing measure {m}")),
        None => Ok((got, hex::encode(Sha256::digest(&bytes)))),
    }
}
