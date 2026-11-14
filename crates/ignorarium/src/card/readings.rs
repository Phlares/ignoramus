//! The author slots of a card: `readings.md`, a file beside the card. A slot holds prose; a
//! number in it is a reference `{{card:<card-id>:<json-pointer>}}`, so the author types none.
//! The file names the card by SHA-256 in its first line. The card names the file by path only,
//! so filling a slot never changes the card.
use super::Card;

/// What a reader sees for a slot with no prose. The Markdown card reads no file; the renderer
/// that joins `readings.md` to a card takes this text from `read_slots`.
pub const NOT_READ: &str = "not yet read by the author";

/// A digit in prose: the slot it sits in, its 1-based line and the line.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotError {
    pub slot: String,
    pub line: u32,
    pub excerpt: String,
}

/// The file name of the author slots.
pub const FILE: &str = "readings.md";

const HEADER: &str = "<!-- card sha256: ";

fn is_header(line: &str) -> bool {
    (line
        .strip_prefix(HEADER)
        .and_then(|r| r.strip_suffix(" -->")))
    .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The heading of a slot, without its marker.
fn slot_name(line: &str) -> Option<&str> {
    line.strip_prefix("## ")
}

fn is_slot_heading(line: &str) -> bool {
    slot_name(line).is_some_and(|n| n == "TL;DR" || n.starts_with("Reading: "))
}

/// Whether `line` holds an ASCII digit outside a card reference and a link target.
fn has_digit(line: &str) -> bool {
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let skip = [("{{card:", "}}"), ("](", ")")]
            .iter()
            .find_map(|(open, close)| {
                let tail = rest.strip_prefix(open)?;
                Some(open.len() + tail.find(close)? + close.len())
            });
        match skip {
            Some(n) => rest = &rest[n..],
            None if c.is_ascii_digit() => return true,
            None => rest = &rest[c.len_utf8()..],
        }
    }
    false
}

/// Every line of `text` that holds a digit the author typed. The first line is exempt when it
/// is the card-hash header, and so is each slot heading.
pub fn check(text: &str) -> Vec<SlotError> {
    let mut slot = String::new();
    let mut errors = vec![];
    for (i, line) in text.lines().enumerate() {
        if i == 0 && is_header(line) {
            continue;
        }
        if let Some(n) = slot_name(line) {
            slot = n.to_string();
        }
        if !is_slot_heading(line) && has_digit(line) {
            errors.push(SlotError {
                slot: slot.clone(),
                line: u32::try_from(i + 1).unwrap_or(u32::MAX),
                excerpt: line.trim().to_string(),
            });
        }
    }
    errors
}

/// `text` without its `<!-- ... -->` comments.
fn uncommented(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("<!--") {
        out.push_str(&rest[..at]);
        match rest[at..].find("-->") {
            Some(end) => rest = &rest[at + end + 3..],
            None => return out,
        }
    }
    out + rest
}

/// Each slot of `text` as (heading, prose). A slot with no prose reads `NOT_READ`.
pub fn read_slots(text: &str) -> Vec<(String, String)> {
    let mut slots: Vec<(String, String)> = vec![];
    for line in text.lines() {
        match slot_name(line) {
            Some(n) => slots.push((n.to_string(), String::new())),
            None => {
                if let Some((_, body)) = slots.last_mut() {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    for (_, body) in &mut slots {
        let prose = uncommented(body).trim().to_string();
        *body = if prose.is_empty() {
            NOT_READ.to_string()
        } else {
            prose
        };
    }
    slots
}

/// An empty `readings.md` for `card`: the hash header, `## TL;DR`, and one
/// `## Reading: <hypothesis>/<stratum>` per results row.
pub fn template(card: &Card) -> String {
    let note = format!(
        "<!-- Prose only. Write a number as {{{{card:{}:<json-pointer>}}}}. -->\n",
        card.study
    );
    let mut s = format!("{HEADER}{} -->\n\n## TL;DR\n\n{note}", card.sha256());
    for r in &card.results {
        s.push_str(&format!(
            "\n## Reading: {}/{}\n\n{note}",
            r.hypothesis, r.stratum
        ));
    }
    s
}
