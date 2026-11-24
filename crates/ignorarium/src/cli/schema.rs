//! The schema number of each `--json` output. A number rises when a key is removed or renamed,
//! or a value changes type; a new key keeps it. `docs/schemas/<name>.v<n>.schema.json` describes
//! each pair of `OUTPUTS`.

pub const GUIDE: u32 = 1;
pub const CHECK: u32 = 1;
pub const STATUS: u32 = 1;
pub const STATUS_TELEMETRY: u32 = 1;

/// `report --json` is the card, whose `card_version` is its number.
pub const OUTPUTS: [(&str, u32); 5] = [
    ("guide", GUIDE),
    ("check", CHECK),
    ("status", STATUS),
    ("status-telemetry", STATUS_TELEMETRY),
    ("report", crate::card::CARD_VERSION),
];
