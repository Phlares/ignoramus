use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const PARTS: [&str; 7] = [
    "Date",
    "Decision",
    "Why",
    "Case against",
    "What failure this prevents",
    "Constants",
    "Sources",
];

fn after_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    line.strip_prefix(label)?.strip_prefix(':')
}

// A heading, or a line that starts with a capital letter and has a colon before any full stop.
// The colon must end the line or precede a space, so a URL or a time of day is no label.
fn opens_a_part(line: &str) -> bool {
    let labelled = |(label, said): (&str, &str)| {
        !label.contains('.') && (said.is_empty() || said.starts_with(' '))
    };
    line.starts_with('#')
        || (line.starts_with(char::is_uppercase)
            && line.trim_end().split_once(':').is_some_and(labelled))
}

fn body<'a>(text: &'a str, label: &str) -> Option<Vec<&'a str>> {
    let mut lines = text.lines();
    let first = lines.find_map(|line| after_label(line, label))?;
    let below = lines.take_while(|line| !opens_a_part(line));
    Some(std::iter::once(first.trim()).chain(below).collect())
}

// The reason a body gives after "none." (empty for a bare "none"), when the body reads that way.
fn after_none(said: &str) -> Option<&str> {
    let rest = said
        .get(..4)
        .filter(|word| word.eq_ignore_ascii_case("none"))
        .map(|_| &said[4..])?;
    match rest.strip_prefix('.') {
        Some(reason) => Some(reason.trim()),
        None => rest.is_empty().then_some(""),
    }
}

fn held(text: &str) -> BTreeSet<&'static str> {
    let says_something = |body: Vec<&str>| {
        let said = body.join(" ");
        !said.trim().is_empty() && after_none(said.trim()) != Some("")
    };
    PARTS
        .into_iter()
        .filter(|part| body(text, part).is_some_and(says_something))
        .collect()
}

fn constants(text: &str) -> Option<Vec<String>> {
    let body = body(text, "Constants")?;
    if after_none(body.join(" ").trim()).is_some() {
        return None;
    }
    let mut bullets: Vec<String> = Vec::new();
    for line in body {
        let blank = line.trim().is_empty();
        match bullets.last_mut() {
            _ if line.starts_with("- ") => bullets.push(line.to_owned()),
            Some(last) if blank || line.starts_with(' ') => {
                last.push('\n');
                last.push_str(line);
            }
            None if blank => {}
            _ => break,
        }
    }
    Some(bullets)
}

fn dated<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let rest = after_label(line, label)?.strip_prefix(' ')?;
    let date = rest.get(..10)?;
    let shape = date.bytes().enumerate().all(|(at, byte)| match at {
        4 | 7 => byte == b'-',
        _ => byte.is_ascii_digit(),
    });
    let within = |from: usize, most: u8| {
        (1..=most).contains(&date[from..from + 2].parse::<u8>().unwrap_or(0))
    };
    (shape && within(5, 12) && within(8, 31) && rest[10..].starts_with('.')).then_some(date)
}

fn amended_in_order(date: &str, below: &mut std::str::Lines) -> bool {
    let mut dates = vec![date];
    let mut spaced = false;
    for line in below.by_ref() {
        if line.trim().is_empty() {
            spaced = true;
        } else if !line.starts_with("Amended") {
            break;
        } else if let (true, Some(next)) = (spaced, dated(line, "Amended")) {
            dates.push(next);
            spaced = false;
        } else {
            return false;
        }
    }
    dates.is_sorted() && !below.any(|line| line.starts_with("Amended"))
}

fn problems(number: &str, text: &str, debt: &[&str]) -> Vec<String> {
    let mut faults = Vec::new();
    let held = held(text);
    for part in PARTS {
        match (debt.contains(&part), held.contains(part)) {
            (true, true) => faults.push(format!("{number}: holds {part}; remove it from DEBT")),
            (false, false) => faults.push(format!("{number}: lacks {part}")),
            _ => {}
        }
    }
    let mut below = text.lines();
    if below
        .next()
        .and_then(|title| title.strip_prefix("# ")?.strip_prefix(number))
        .is_none_or(|title| !title.starts_with(' '))
    {
        faults.push(format!("{number}: line 1 must read \"# {number} <title>\""));
    }
    let first = below.by_ref().find(|line| !line.trim().is_empty());
    match first.and_then(|line| dated(line, "Date")) {
        None => faults.push(format!(
            "{number}: the first line under the title must read \"Date: YYYY-MM-DD.\""
        )),
        Some(date) if !amended_in_order(date, &mut below) => faults.push(format!(
            "{number}: Amended lines follow the Date line, each after a blank line, read \"Amended: YYYY-MM-DD.\", and run oldest first"
        )),
        Some(_) => {}
    }
    match constants(text) {
        Some(bullets) if bullets.is_empty() => {
            faults.push(format!("{number}: Constants holds no bullet"))
        }
        Some(bullets)
            if !bullets
                .iter()
                .all(|bullet| bullet.contains("Basis:") && bullet.contains("Rule for change:")) =>
        {
            faults.push(format!(
                "{number}: a Constants bullet lacks Basis: or Rule for change:"
            ))
        }
        _ => {}
    }
    faults
}

// The parts that records written before the template still lack, by part. A record leaves a
// list in the change that writes the part. No list grows, and no record from 0030 on may enter.
#[rustfmt::skip]
const DEBT: &[(&str, &[&str])] = &[
    ("Decision", &["0001"]),
    ("Why", &["0001", "0019", "0026", "0028", "0029"]),
    ("Case against", &[
        "0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0010", "0011", "0012",
        "0015", "0018", "0028", "0029",
    ]),
    ("What failure this prevents", &[
        "0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0010", "0011", "0012",
        "0015", "0018", "0019", "0026", "0028", "0029",
    ]),
    ("Constants", &[
        "0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0010", "0011", "0012",
        "0015", "0018", "0019", "0026", "0028", "0029",
    ]),
    ("Sources", &["0001", "0003", "0005", "0011", "0012"]),
];

fn debt(number: &str) -> Vec<&'static str> {
    DEBT.iter()
        .filter(|(_, records)| records.contains(&number))
        .map(|(part, _)| *part)
        .collect()
}

fn records_in(dir: &Path) -> BTreeMap<String, String> {
    let mut records = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let numbered = name.get(..5).is_some_and(|head| {
            head.ends_with('-') && head[..4].bytes().all(|b| b.is_ascii_digit())
        });
        assert!(
            numbered && name.ends_with(".md") && path.is_file(),
            "{name} is not a record named NNNN-<slug>.md"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            records.insert(name[..4].to_owned(), text).is_none(),
            "{name} repeats a record number"
        );
    }
    records
}

fn records() -> BTreeMap<String, String> {
    records_in(Path::new("../../docs/decisions"))
}

fn template() -> String {
    let guide = std::fs::read_to_string("../../CONTRIBUTING.md").unwrap();
    let (_, section) = guide
        .split_once("\n## Decision records\n")
        .expect("CONTRIBUTING.md has a section \"Decision records\"");
    let fenced = section
        .split("\n```")
        .nth(1)
        .expect("the section holds a fenced template");
    fenced
        .split_once('\n')
        .map_or("", |(_, block)| block)
        .to_owned()
}

#[test]
fn a_part_needs_a_label_at_line_start_and_a_body() {
    let has = |text: &str, part: &str| held(text).contains(part);
    assert!(has("Why: one reason.", "Why"));
    assert!(has("Why:\n\n- one reason.", "Why"));
    assert!(has("Why:\r\n\r\n- one reason.\r\n", "Why"));
    assert!(has(
        "Why:\n\n- **Order.** These, in this order:\n  1. a",
        "Why"
    ));
    assert!(has(
        "Sources: none. The record cites no outside work.",
        "Sources"
    ));
    assert!(!has("Why:\n", "Why"));
    assert!(!has("Why:\n\nLimits:\n\n- one limit.", "Why"));
    assert!(!has("Why:\n\n## Sources\n\n- a", "Why"));
    assert!(!has("Sources: none", "Sources"));
    assert!(!has("Sources: None.", "Sources"));
    assert!(!has("Sources:\n\nnone.\n", "Sources"));
    assert!(!has("## Sources\n\n- a", "Sources"));
    assert!(!has("Why 3/4 and not higher:\n\n- a", "Why"));
    assert!(!has("- **Case against.** a", "Case against"));
    assert!(!has("  Why: a", "Why"));
    assert!(!has("Why:\n\nSources: none. No outside work.", "Why"));
    assert!(!has("Why:\n\nCost: one cost.", "Why"));
    assert!(!has(
        "Constants: none.\n\nSources: https://example.org",
        "Constants"
    ));
    assert!(has("Why:\n\nEvidence. One study: five of five.", "Why"));
    assert!(has(
        "Why:\n\nSee https://example.org for the reason.",
        "Why"
    ));
}

#[test]
fn constants_bullets_end_at_the_next_part() {
    let text = "Constants:\n\n- a = 1. Basis: b. Rule for change: c.\n  more\n- d = 2. Basis: e.\n\nOpen:\n\n- not a constant\n";
    assert_eq!(constants(text).unwrap().len(), 2);
    assert_eq!(
        constants("Constants: none. The record sets no value."),
        None
    );
    assert_eq!(constants("Why: a"), None);
    assert_eq!(
        constants("Constants:\n\nNone of the values is fixed yet.\n\n- a = 1."),
        Some(Vec::new())
    );
}

const FULL: &str = "# 0099 T\n\nDate: 2026-11-15. On file.\n\nAmended: 2026-11-16. A change (#1).\n\nIntro.\n\nDecision: a\n\nWhy: b\n\nCase against: c\n\nWhat failure this prevents: d, expected.\n\nConstants:\n\n- x = 1. Basis: y. Rule for change: z.\n\nSources: none. No outside work.\n";

#[test]
fn problems_name_the_record_and_the_fault() {
    let none: Vec<String> = Vec::new();
    assert_eq!(problems("0099", FULL, &[]), none);
    let cut = |old: &str, new: &str| problems("0099", &FULL.replace(old, new), &[]);
    assert_eq!(cut("Why: b\n\n", ""), ["0099: lacks Why"]);
    assert_eq!(
        problems("0099", FULL, &["Why"]),
        ["0099: holds Why; remove it from DEBT"]
    );
    assert_eq!(
        problems("0099", &FULL.replace("Why: b\n\n", ""), &["Why"]),
        none
    );
    assert_eq!(
        problems("0099", FULL, &["Date"]),
        ["0099: holds Date; remove it from DEBT"]
    );
    let date = "0099: the first line under the title must read \"Date: YYYY-MM-DD.\"";
    assert_eq!(
        cut("Date: 2026-11-15. On file.", "Date: 2026-13-15. On file."),
        [date]
    );
    assert_eq!(
        cut("Date: 2026-11-15. On file.", "Date: 2026-11-15"),
        [date]
    );
    assert_eq!(cut("# 0099 T\n\n", "# 0099 T\n\nIntro first.\n\n"), [date]);
    assert_eq!(
        cut("# 0099 T", "# 0098 T"),
        ["0099: line 1 must read \"# 0099 <title>\""]
    );
    let amended = "0099: Amended lines follow the Date line, each after a blank line, read \"Amended: YYYY-MM-DD.\", and run oldest first";
    assert_eq!(
        cut("Amended: 2026-11-16.", "Amended: 2026-11-14."),
        [amended]
    );
    assert_eq!(
        cut(
            "Amended: 2026-11-16. A change (#1).\n",
            "Amended: 2026-11-17. B (#2).\n\nAmended: 2026-11-16. A (#1).\n"
        ),
        [amended]
    );
    assert_eq!(
        cut(
            "Amended: 2026-11-16. A change (#1).\n",
            "Amended: 2026-11-16. A change (#1).\n\nAmended: 2026-11-16. B (#2).\n"
        ),
        none
    );
    assert_eq!(cut("On file.\n\nAmended", "On file.\nAmended"), [amended]);
    assert_eq!(
        cut("Amended: 2026-11-16.", "Amended 2026-11-16."),
        [amended]
    );
    assert_eq!(
        cut("Intro.\n", "Intro.\n\nAmended: 2026-11-16. Late (#3).\n"),
        [amended]
    );
    assert_eq!(
        cut(
            "- x = 1. Basis: y. Rule for change: z.",
            "- x = 1. Basis: y."
        ),
        ["0099: a Constants bullet lacks Basis: or Rule for change:"]
    );
    assert_eq!(
        cut("- x = 1. Basis: y. Rule for change: z.", "x is 1."),
        ["0099: Constants holds no bullet"]
    );
}

#[test]
fn every_record_holds_its_parts() {
    let faults: Vec<String> = records()
        .iter()
        .flat_map(|(number, text)| problems(number, text, &debt(number)))
        .collect();
    assert!(faults.is_empty(), "\n{}", faults.join("\n"));
}

#[test]
fn debt_names_only_records_and_parts_that_exist() {
    let records = records();
    for (part, numbers) in DEBT {
        assert!(PARTS.contains(part), "DEBT: {part} is no part");
        for (at, number) in numbers.iter().enumerate() {
            assert!(
                records.contains_key(*number),
                "DEBT {part}: {number} is no record"
            );
            assert!(
                !numbers[..at].contains(number),
                "DEBT {part} lists {number} twice"
            );
            assert!(
                *number < "0030",
                "DEBT {part}: {number}; records from 0030 on hold every part"
            );
        }
    }
    let entries: usize = DEBT.iter().map(|(_, numbers)| numbers.len()).sum();
    assert_eq!(entries, 60);
}

#[test]
#[should_panic(expected = "README.md")]
fn a_stray_file_among_the_records_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("0001-a.md"), "# 0001 A\n").unwrap();
    std::fs::write(dir.path().join("README.md"), "x\n").unwrap();
    records_in(dir.path());
}

#[test]
fn the_template_holds_every_part() {
    let template = template();
    assert_eq!(held(&template).len(), PARTS.len(), "{template}");
    for bullet in constants(&template).expect("the template shows a constant") {
        assert!(
            bullet.contains("Basis:") && bullet.contains("Rule for change:"),
            "{bullet}"
        );
    }
}
