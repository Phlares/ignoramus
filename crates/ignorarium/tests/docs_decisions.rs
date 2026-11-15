use std::collections::BTreeSet;

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

fn opens_a_part(line: &str) -> bool {
    line.starts_with('#') || (line.starts_with(char::is_uppercase) && line.trim_end().ends_with(':'))
}

fn body<'a>(text: &'a str, label: &str) -> Option<Vec<&'a str>> {
    let mut lines = text.lines();
    let first = lines.find_map(|line| after_label(line, label))?;
    let below = lines.take_while(|line| !opens_a_part(line));
    Some(std::iter::once(first.trim()).chain(below).collect())
}

fn held(text: &str) -> BTreeSet<&'static str> {
    let says_something = |body: Vec<&str>| {
        let said = body.join(" ");
        let said = said.trim().trim_end_matches('.');
        !said.is_empty() && !said.eq_ignore_ascii_case("none")
    };
    PARTS
        .into_iter()
        .filter(|part| body(text, part).is_some_and(says_something))
        .collect()
}

fn constants(text: &str) -> Option<Vec<String>> {
    let body = body(text, "Constants")?;
    if body.join(" ").trim().to_ascii_lowercase().starts_with("none") {
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
    let mut last = date;
    let mut head = below.by_ref().take_while(|line| line.starts_with("Amended:"));
    let ordered = head.all(|line| {
        dated(line, "Amended").is_some_and(|next| std::mem::replace(&mut last, next) <= next)
    });
    ordered && !below.any(|line| line.starts_with("Amended:"))
}

fn problems(number: &str, text: &str, debt: &[&str]) -> Vec<String> {
    let mut faults = Vec::new();
    let held = held(text);
    for part in PARTS {
        match (debt.contains(&part), held.contains(part)) {
            (true, _) if part == "Date" => faults.push(format!("{number}: Date cannot be debt")),
            (true, true) => faults.push(format!("{number}: holds {part}; remove it from DEBT")),
            (false, false) => faults.push(format!("{number}: lacks {part}")),
            _ => {}
        }
    }
    let mut below = text.lines();
    let first = below
        .by_ref()
        .skip(1)
        .find(|line| !line.trim().is_empty());
    match first.and_then(|line| dated(line, "Date")) {
        None => faults.push(format!(
            "{number}: the first line under the title must read \"Date: YYYY-MM-DD.\""
        )),
        Some(date) if !amended_in_order(date, &mut below) => faults.push(format!(
            "{number}: Amended lines follow the Date line, read \"Amended: YYYY-MM-DD.\", and run oldest first"
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
}

const FULL: &str = "# 0099 T\n\nDate: 2026-11-15. On file.\nAmended: 2026-11-16. A change (#1).\n\nIntro.\n\nDecision: a\n\nWhy: b\n\nCase against: c\n\nWhat failure this prevents: d, expected.\n\nConstants:\n\n- x = 1. Basis: y. Rule for change: z.\n\nSources: none. No outside work.\n";

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
        problems("0099", FULL, &["Date"])[0],
        "0099: Date cannot be debt"
    );
    let date = "0099: the first line under the title must read \"Date: YYYY-MM-DD.\"";
    assert_eq!(
        cut("Date: 2026-11-15. On file.", "Date: 2026-13-15. On file."),
        [date]
    );
    assert_eq!(cut("Date: 2026-11-15. On file.", "Date: 2026-11-15"), [date]);
    assert_eq!(cut("# 0099 T\n\n", "# 0099 T\n\nIntro first.\n\n"), [date]);
    let amended = "0099: Amended lines follow the Date line, read \"Amended: YYYY-MM-DD.\", and run oldest first";
    assert_eq!(
        cut("Amended: 2026-11-16.", "Amended: 2026-11-14."),
        [amended]
    );
    assert_eq!(
        cut(
            "Amended: 2026-11-16. A change (#1).\n",
            "Amended: 2026-11-17. B (#2).\nAmended: 2026-11-16. A (#1).\n"
        ),
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
