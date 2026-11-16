//! The box's settings that are not its machine: `<home>/config.toml`.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub stamp: StampConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StampConfig {
    /// Time-stamp authority URLs, in the order the commands number them.
    pub authorities: Vec<String>,
}

/// An absent file gives the defaults. A bad file is an error, never the defaults.
pub fn load(home_root: &Path) -> Result<Config, String> {
    let file = home_root.join("config.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))
}
