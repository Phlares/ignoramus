//! The schema number of each `--json` output. A number rises when a key is removed or renamed,
//! or a value changes type; a new key keeps it. `docs/schemas/<name>.v<n>.schema.json` describes
//! each pair of `OUTPUTS`.

pub const GUIDE: u32 = 1;

pub const OUTPUTS: [(&str, u32); 1] = [("guide", GUIDE)];
