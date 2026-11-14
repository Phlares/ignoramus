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

/// The card hash in the first line of `text`, when that line is the header.
pub fn header_hash(text: &str) -> Option<&str> {
    let line = text.lines().next()?;
    (line
        .strip_prefix(HEADER)
        .and_then(|r| r.strip_suffix(" -->")))
    .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The heading of a slot, without its marker.
fn slot_name(line: &str) -> Option<&str> {
    line.strip_prefix("## ")
}

fn is_slot_heading(line: &str) -> bool {
    slot_name(line).is_some_and(|n| n == "TL;DR" || n.starts_with("Reading: "))
}

fn is_reference(body: &str) -> bool {
    let word = |c: char, extra: &str| c.is_ascii_alphanumeric() || extra.contains(c);
    body.split_once(':').is_some_and(|(id, pointer)| {
        !id.is_empty()
            && id.chars().all(|c| word(c, "._-"))
            && pointer.starts_with('/')
            && pointer.chars().all(|c| c == '/' || word(c, "._~-"))
    })
}

/// Whether `line` breaks the slot grammar: a digit outside a reference and a link target, a
/// reference that is not exactly `{{card:<id>:<pointer>}}`, or a link target with whitespace.
/// A `](` opens a link target only after link text `[...]` on the same line; elsewhere it is plain text.
fn is_bad(line: &str) -> bool {
    let mut rest = line;
    // Whether a `[` opened link text that no `]` has closed.
    let mut in_text = false;
    while let Some(c) = rest.chars().next() {
        if let Some(tail) = rest.strip_prefix("{{card:") {
            let Some(end) = tail.find("}}") else {
                return true;
            };
            if !is_reference(&tail[..end]) {
                return true;
            }
            rest = &tail[end + 2..];
        } else if let Some(tail) = rest.strip_prefix("](").filter(|_| in_text) {
            in_text = false;
            match tail.find(')') {
                Some(end) if tail[..end].contains(char::is_whitespace) => return true,
                Some(end) => rest = &tail[end + 1..],
                None => rest = tail,
            }
        } else if c.is_ascii_digit() {
            return true;
        } else {
            match c {
                '[' => in_text = true,
                ']' => in_text = false,
                _ => {}
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    false
}

/// Every line of `text` that breaks the slot grammar (see `is_bad`). The first line is exempt when it
/// is the card-hash header, and so is each slot heading.
pub fn check(text: &str) -> Vec<SlotError> {
    let mut slot = String::new();
    let mut errors = vec![];
    for (i, line) in text.lines().enumerate() {
        if i == 0 && header_hash(text).is_some() {
            continue;
        }
        if let Some(n) = slot_name(line) {
            slot = n.to_string();
        }
        if !is_slot_heading(line) && is_bad(line) {
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

/// Each slot of `text` as (heading, prose). A slot with no prose reads `NOT_READ`. `Err`
/// holds every `check` error: the slots refuse a typed number.
pub fn read_slots(text: &str) -> Result<Vec<(String, String)>, Vec<SlotError>> {
    let errors = check(text);
    if !errors.is_empty() {
        return Err(errors);
    }
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
    Ok(slots)
}

/// An empty `readings.md` for `card`: the hash header, `## TL;DR`, and one
/// `## Reading: <hypothesis>/<stratum>` per results row.
pub fn template(card: &Card) -> String {
    let note = format!(
        "<!-- Prose only. Write a number as a reference, for example {{{{card:{}:/results/0/estimate}}}}. -->\n",
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
