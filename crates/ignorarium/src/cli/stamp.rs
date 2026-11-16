//! `stamp --request`: write the file to stamp and print the commands that stamp it.
use super::{CliError, Ctx, Done, Run, located, not_registered, out};
use crate::stamp::{self, StampError};
use crate::{config, keys};
use std::path::Path;

pub fn request(ctx: &Ctx, id: &str, with: Option<&Path>) -> Run {
    let (home, _) = located(ctx, id)?;
    let key = keys::load(home)
        .map_err(|e| CliError::usage(format!("box key: {e}")))?
        .ok_or_else(|| not_registered(id))?;
    let cfg = config::load(&home.root).map_err(CliError::usage)?;
    let file = stamp::request(home, id, &key.verifying_key(), with).map_err(|e| match e {
        StampError::NotRegistered => not_registered(id),
        e => CliError::usage(format!("cannot request a stamp of {id}: {e}")),
    })?;
    let file = file.display();
    let mut text = format!("{file}\n");
    let urls = if cfg.stamp.authorities.is_empty() {
        text.push_str("no authority is set in config.toml\n");
        vec!["<authority url>".to_string()]
    } else {
        cfg.stamp.authorities
    };
    for (i, url) in urls.iter().enumerate() {
        let n = i + 1;
        text.push_str(&format!(
            "openssl ts -query -data {file} -sha256 -cert -out {file}.tsq\n\
             curl -sS -H \"Content-Type: application/timestamp-query\" --data-binary @{file}.tsq {url} -o {file}.{n}.tsr\n\
             ignorarium stamp {id} --reply {file}.{n}.tsr\n"
        ));
    }
    out(&text)?;
    Ok(Done::exit(0))
}
