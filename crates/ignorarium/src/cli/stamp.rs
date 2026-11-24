//! `stamp`: write the file to stamp and print the commands that stamp it, or record a reply.
use super::exit::Code;
use super::{CliError, Ctx, Done, Run, keyed, not_registered, out, refusal};
use crate::config;
use crate::stamp::{self, StampError, Stamped};
use std::path::Path;

fn failed(id: &str, what: &str, e: StampError) -> CliError {
    match e {
        StampError::NotRegistered => not_registered(id),
        StampError::Refused { code, found } => refusal(code, &found),
        e => CliError::usage(format!("cannot {what} of {id}: {e}")),
    }
}

pub fn request(ctx: &Ctx, id: &str, with: Option<&Path>) -> Run {
    let (home, _, key) = keyed(ctx, id)?;
    let cfg = config::load(&home.root).map_err(CliError::usage)?;
    let file = stamp::request(home, id, &key.verifying_key(), with)
        .map_err(|e| failed(id, "request a stamp", e))?;
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
            "openssl ts -query -data \"{file}\" -sha256 -cert -out \"{file}.tsq\"\n\
             curl -sS -H \"Content-Type: application/timestamp-query\" --data-binary @\"{file}.tsq\" {url} -o \"{file}.{n}.tsr\"\n\
             ignorarium stamp {id} --reply \"{file}.{n}.tsr\"\n"
        ));
    }
    out(&text)?;
    Ok(Done::exit(Code::Done))
}

pub fn reply(ctx: &Ctx, id: &str, reply: &Path, file: Option<&Path>) -> Run {
    let (home, _, key) = keyed(ctx, id)?;
    match stamp::record(home, id, &key, reply, file).map_err(|e| failed(id, "record a stamp", e))? {
        Stamped::New(r) => out(&format!("stamp record: {}\n", r.seq))?,
        Stamped::Already => out("already recorded\n")?,
    }
    Ok(Done::exit(Code::Done))
}
