//! `key export`: print the box's public key. Also reads a public key file for `verify`.
use super::{CliError, Ctx, Done, Run, out};
use crate::keys;
use ed25519_dalek::VerifyingKey;
use std::path::Path;

pub fn export(ctx: &Ctx) -> Run {
    let home = ctx.home()?;
    let key = keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| CliError::refuse("no box key: this box has not registered a study"))?;
    out(&format!(
        "{}\n",
        hex::encode(key.verifying_key().to_bytes())
    ))?;
    Ok(Done::exit(0))
}

/// The public key a file holds: 64 hex characters, white space around them ignored. A refusal
/// never repeats what the file holds, since it may be a private key.
pub fn read_public(path: &Path) -> Result<VerifyingKey, CliError> {
    let bad = |why: &str| CliError::usage(format!("the public key file {why}"));
    let bytes = std::fs::read(path)
        .map_err(|e| CliError::usage(format!("cannot read the public key file: {}", e.kind())))?;
    let text = bytes.trim_ascii();
    if text.len() != 64 {
        return Err(bad("must hold 64 hex characters and nothing else"));
    }
    let mut raw = [0u8; 32];
    hex::decode_to_slice(text, &mut raw).map_err(|_| bad("holds characters that are not hex"))?;
    VerifyingKey::from_bytes(&raw).map_err(|_| bad("does not hold a valid ed25519 public key"))
}
