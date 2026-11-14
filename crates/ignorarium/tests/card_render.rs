//! The Markdown and LaTeX renderers and the figure: pure functions of one card.
use ignorarium::card::Card;
use ignorarium::card::render::{self, figure, latex, markdown, tex_escape};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/card")
        .join(name)
}

fn card() -> Card {
    let text = std::fs::read_to_string(fixture("e1-like.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The card with its first interval of `H1`'s code row given `bound`.
fn with_bound(bound: Option<f64>, contiguous: bool) -> Card {
    let mut c = card();
    let i = &mut c.results[0].intervals[0];
    (i.bound, i.contiguous) = (bound, contiguous);
    c
}

fn files(c: &Card) -> Vec<(String, String)> {
    latex(c).unwrap()
}

/// The card of `fixture(name)`, rendered, against the golden `name` with `.md`.
fn golden(name: &str) {
    let text = std::fs::read_to_string(fixture(&format!("{name}.json"))).unwrap();
    let got = markdown(&serde_json::from_str(&text).unwrap());
    let path = fixture(&format!("{name}.md"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(got, want);
}

#[test]
fn markdown_golden() {
    golden("e1-like");
}

#[test]
fn markdown_golden_with_formatted_bounds() {
    golden("e1-like-bounds");
}

#[test]
fn markdown_sections_follow_design_order() {
    let md = markdown(&card());
    let order = [
        "## Header",
        "## Claim",
        "## Verdicts",
        "## Claim label",
        "## Results",
        "## Sensitivity",
        "## Figures",
        "## Setup",
        "## Deviations",
        "## Surprises",
        "## Cost",
        "## Runs",
        "## Re-run recipe",
        "## Citation",
        "## Author readings",
    ];
    let at: Vec<usize> = order
        .iter()
        .map(|h| md.find(h).unwrap_or_else(|| panic!("{h}")))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{at:?}");
}

/// The card with the code row's `below` bound `b` at grid step `step`, on side `side`.
fn rounded(side: &str, b: f64, step: f64) -> String {
    let mut c = card();
    let i = &mut c.results[0].intervals[0];
    (i.side, i.bound, i.grid.step) = (side.into(), Some(b), step);
    markdown(&c)
}

#[test]
fn display_rounding_is_applied_here() {
    let md = markdown(&with_bound(Some(0.039_212_34), true));
    assert!(
        md.contains("below 0.0393 "),
        "a below bound rounds up, to the grid's 4 decimals: {md}"
    );
    assert!(
        md.contains("inverted registered e-process"),
        "method beside the bound"
    );
    assert!(md.contains("+2.91%"), "percent change, 2 decimals");
    assert!(md.contains("3.100"), "e-value, 4 significant digits");
    assert!(!md.contains("CLT"));
    assert!(md.contains("0.039221"), "the margin keeps full precision");
}

#[test]
fn the_interval_states_its_search_grid() {
    let md = markdown(&card());
    let grid =
        "grid: steps of 0.0001 from the registered margin, searched from 0.028621 to 0.693147";
    assert!(md.contains(grid), "{md}");
    let mut c = card();
    c.results[0].intervals[0].grid.from = 0.028_470_999_999_999_996;
    assert!(
        markdown(&c).contains("searched from 0.028471 to"),
        "float noise is cut"
    );
}

#[test]
fn bounds_round_outward_on_the_step_grid() {
    assert!(rounded("above", 0.039_281, 1e-4).contains("above 0.0392 "));
    assert!(
        rounded("below", 0.0392, 1e-4).contains("below 0.0392 "),
        "a grid value stays"
    );
    assert!(
        rounded("below", 0.0393, 1e-4).contains("below 0.0393 "),
        "no float drift"
    );
    assert!(
        rounded("below", 0.31, 0.25).contains("below 0.31 "),
        "step 0.25 has 2 decimals"
    );
    assert!(rounded("above", 0.319, 0.25).contains("above 0.31 "));
    assert!(
        rounded("below", 0.31, 0.5).contains("below 0.4 "),
        "step 0.5 has 1 decimal"
    );
}

#[test]
fn an_e_value_never_rounds_up_to_a_pass() {
    let mut c = card();
    c.results[0].e_value = Some(199.96);
    let md = markdown(&c);
    assert!(md.contains("| 199.9 |"), "{md}");
    assert!(!md.contains("200.0 |"));
    c.results[0].e_value = Some(312.456_7);
    assert!(markdown(&c).contains("| 312.4 |"));
}

#[test]
fn thresholds_round_up_and_e_values_down() {
    let mut c = card();
    c.claims[0].pass_e_value = Some(81.300_81);
    c.hypotheses[0].e_value = Some(81.300_5);
    c.surprises.triggers.push(ignorarium::derived::Trigger {
        id: "t1".into(),
        e_value: 81.300_5,
        threshold: 81.300_81,
        fired: false,
        reason: None,
        admitted: vec![],
        skipped: vec![],
    });
    let md = markdown(&c);
    assert!(md.contains("pass rule: e-value at least 81.31 "), "{md}");
    assert!(md.contains("e-value 81.30)"), "{md}");
    assert!(md.contains("e-value 81.30 against 81.31,"), "{md}");
    // A threshold already at 4 digits stays, float noise in its scaling included.
    c.claims[0].pass_e_value = Some(1.0 / 0.005);
    c.surprises.triggers[0].threshold = 10.13;
    let md = markdown(&c);
    assert!(md.contains("pass rule: e-value at least 200.0 "), "{md}");
    assert!(md.contains("against 10.13,"), "{md}");
}

#[test]
fn a_gap_prints_a_note_not_a_bound() {
    let md = markdown(&with_bound(None, false));
    assert!(
        md.contains("no interval: the passing margins leave a gap"),
        "{md}"
    );
}

#[test]
fn missing_and_not_applicable_fields_print_fixed_words() {
    let md = markdown(&card());
    assert!(md.contains("not in the records"));
    let mut c = card();
    c.setup.data_grade = ignorarium::card::sections::Field::NotApplicable;
    assert!(markdown(&c).contains("not applicable"));
}

#[test]
fn latex_plain_tabular() {
    let all = files(&card());
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    for want in ["results.tex", "deviations.tex", "sensitivity.tex"] {
        assert!(names.contains(&want), "{names:?}");
    }
    for (name, text) in &all {
        for banned in ["tabularx", "booktabs", "longtable", r"\toprule"] {
            assert!(!text.contains(banned), "{name}: {banned}");
        }
    }
    let results = &all.iter().find(|(n, _)| n == "results.tex").unwrap().1;
    assert!(results.contains(r"\begin{tabular}") && results.contains(r"\end{tabular}"));
}

#[test]
fn latex_escapes_specials() {
    let mut c = card();
    c.study = "a_b%c&d#e".into();
    let all = files(&c);
    let joined: String = all.iter().map(|(_, t)| t.as_str()).collect();
    assert!(joined.contains(r"a\_b\%c\&d\#e"), "{joined}");
    assert!(!joined.contains("a_b%c"));
    assert_eq!(
        tex_escape(r"\{}$&#^_%~"),
        r"\textbackslash{}\{\}\$\&\#\textasciicircum{}\_\%\textasciitilde{}"
    );
    c.deviations.clear();
    c.hypotheses[0].hypothesis = "H_1".into();
    c.results[0].hypothesis = "H_1".into();
    let table = &files(&c)
        .into_iter()
        .find(|(n, _)| n == "results.tex")
        .unwrap()
        .1;
    assert!(table.contains(r"H\_1"), "{table}");
}

#[test]
fn inline_value_files_named_by_path() {
    let mut c = card();
    c.results[0].hypothesis = "H 1/x".into();
    c.results[0].stratum = "Code_A".into();
    let all = files(&c);
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        "value-h-1-x-code-a-estimate.tex",
        "value-h-1-x-code-a-percent-change.tex",
        "value-h-1-x-code-a-e-value.tex",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    assert!(names.contains(&"value-h1--estimate.tex"), "{names:?}");
    for n in names.iter().filter(|n| n.starts_with("value-")) {
        let stem = n.trim_end_matches(".tex");
        assert!(
            stem.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
            "{n}"
        );
    }
    let pct = &all
        .iter()
        .find(|(n, _)| n == "value-h-1-x-code-a-percent-change.tex")
        .unwrap()
        .1;
    assert_eq!(pct, r"+2.91\%");
}

#[test]
fn figure_has_alt_text_and_margin_line() {
    let c = with_bound(Some(0.0301), true);
    let svg = figure(&c, "H1");
    assert!(svg.starts_with("<svg"));
    let title = svg
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .unwrap();
    for want in ["H1", "code", "0.0287", "0.0301", "0.0392"] {
        assert!(title.contains(want), "{want} in {title}");
    }
    assert_eq!(
        svg.matches("stroke-dasharray").count(),
        1,
        "one dashed margin line"
    );
    for banned in ["rgba", "opacity", "var(", "hsla"] {
        assert!(!svg.contains(banned), "{banned}");
    }
    assert!(svg.contains("<circle"), "a point per stratum");
}

#[test]
fn value_files_that_share_a_name_are_refused() {
    let mut c = card();
    c.results[0].hypothesis = "H 1/x".into();
    c.results[1].hypothesis = "H-1-x".into();
    c.results[1].stratum = c.results[0].stratum.clone();
    let e = latex(&c).unwrap_err();
    assert!(e.contains("H 1/x") && e.contains("H-1-x"), "{e}");
    let mut c = card();
    c.results[0].stratum = "all".into();
    c.results[1].stratum = "all".into();
    assert!(latex(&c).is_err(), "two groups named all");
    let mut c = card();
    c.results[0].stratum = "all".into();
    let names: Vec<String> = files(&c).into_iter().map(|f| f.0).collect();
    assert!(names.contains(&"value-h1-all-estimate.tex".to_string()));
    assert!(
        names.contains(&"value-h1--estimate.tex".to_string()),
        "pooled has its own name"
    );
}

#[test]
fn free_text_cannot_break_the_markdown() {
    use ignorarium::card::DeviationRow;
    use ignorarium::card::sections::{FailedRow, Field};
    let nasty = "a|b `c` <b> [x](y)\nz";
    let mut c = card();
    c.deviations.push(DeviationRow::Signed {
        from: "s0".into(),
        reason: nasty.into(),
        timing: "t".into(),
        differs: vec![],
    });
    c.surprises.failed.push(FailedRow {
        row: Some(1),
        stage: "s".into(),
        exit: "e".into(),
        reason: Some("x\n## Injected".into()),
    });
    c.claims[0].statement = "![img](http://x)".into();
    c.citation.bibtex = Field::Recorded {
        value: "@a{b,\n t = {```}}".into(),
    };
    let md = markdown(&c);
    assert!(md.contains(r"a\|b \`c\` \<b\> \[x\](y) z"), "{md}");
    assert!(!md.contains("\n## Injected"), "a newline ends no bullet");
    assert!(md.contains(r"!\[img\]"), "{md}");
    assert!(
        md.contains("\n````bibtex\n"),
        "a fence longer than the text's backticks"
    );
    for line in md.lines().filter(|l| l.starts_with("| signed")) {
        assert_eq!(line.matches(" | ").count(), 7, "{line}");
    }
    c.hypotheses[0].hypothesis = "H]x".into();
    c.claims[0].hypothesis = "H]x".into();
    c.results
        .iter_mut()
        .for_each(|r| r.hypothesis = "H]x".into());
    assert!(
        markdown(&c).contains(r"![H\]x"),
        "alt text and link text are escaped"
    );
}

use ignorarium::card::readings::{self, NOT_READ};

#[test]
fn slot_allows_references_refuses_digits() {
    let ok = "<!-- card sha256: ab12 -->\n\n## TL;DR\n\nThe code row moved {{card:e1:/results/0/estimate}} points; see [paper](https://x.org/a/2).\n";
    assert_eq!(readings::check(ok), vec![]);
    let bad = "## TL;DR\n\nGood.\n\n## Reading: H1/code\n\nIt moved by 5 points.\n";
    let e = readings::check(bad);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].slot, "Reading: H1/code");
    assert_eq!(e[0].line, 7);
    assert!(e[0].excerpt.contains("5 points"));
    // A digit beside a reference, and a digit in an unclosed reference, both count.
    assert_eq!(readings::check("## TL;DR\n\n{{card:e1:/a}} 7\n").len(), 1);
    assert_eq!(readings::check("## TL;DR\n\n{{card:e1:/a 7\n").len(), 1);
    // A header line that is not the first line gets no exemption.
    assert_eq!(
        readings::check("## TL;DR\n<!-- card sha256: ab12 -->\n").len(),
        1
    );
}

#[test]
fn empty_slot_renders_not_yet_read() {
    let c = card();
    let t = readings::template(&c);
    let shown = readings::read_slots(&t);
    assert_eq!(shown[0], ("TL;DR".to_string(), NOT_READ.to_string()));
    assert_eq!(shown.len(), 1 + c.results.len());
    assert!(shown.iter().all(|(_, s)| s == NOT_READ), "{shown:?}");
    assert_eq!(NOT_READ, "not yet read by the author");
    assert_eq!(readings::check(&t), vec![]);
    let filled = t.replacen("## TL;DR\n", "## TL;DR\n\nIt held.\n", 1);
    assert_eq!(readings::read_slots(&filled)[0].1, "It held.");
}

#[test]
fn card_hash_stable_when_readings_change() {
    let c = card();
    let before = c.sha256();
    let t = readings::template(&c);
    assert!(t.starts_with(&format!("<!-- card sha256: {before} -->\n")));
    assert_eq!(markdown(&c), markdown(&c));
    // The card names the file by path only.
    assert!(!markdown(&c).contains(&before));
    let _filled = t.replacen("## TL;DR\n", "## TL;DR\n\nIt held.\n", 1);
    assert_eq!(c.sha256(), before);
    assert_eq!(c.json(), c.json());
    assert!(c.json().ends_with("}\n"));
}

#[test]
fn figures_are_one_file_per_claim_and_collisions_name_both() {
    let c = card();
    let f = render::figures(&c).unwrap();
    assert_eq!(f.len(), c.claims.len());
    assert!(
        f.iter()
            .all(|(n, s)| n.starts_with("figure-") && s.contains("<svg"))
    );
    let mut dup = c.clone();
    let mut second = dup.claims[0].clone();
    second.hypothesis = dup.claims[0].hypothesis.to_lowercase();
    dup.claims.push(second);
    let err = render::figures(&dup).unwrap_err();
    assert!(err.contains("H1") && err.contains("h1"), "{err}");
}

#[test]
fn slot_grammar_is_closed() {
    let bad = |line: &str| readings::check(&format!("## TL;DR\n\n{line}\n")).len();
    for line in [
        "{{card:e1: 7 points}}",
        "{{card:e1:results/0}}",
        "{{card::/a}}",
        "{{card:e 1:/a}}",
        "{{card:e1:/a b}}",
        "{{card:e1:/a",
        r#"[a](u "5 things")"#,
        r#"[a](u "things")"#,
        "[a](u 5)",
        "foo](7)",
        "costs ](4) percent",
        "[a]x](7)",
    ] {
        assert_eq!(bad(line), 1, "{line}");
    }
    for line in [
        "{{card:e1.v-2_x:/results/0/estimate}} and {{card:e1:/a~1/b/}}",
        "{{card:e1:/}}",
        "[a](https://x.org/a/2) and [b](7)",
        "[a](u7)",
        "bare ( 7 ) is not a target",
    ] {
        let want = usize::from(line.starts_with("bare"));
        assert_eq!(bad(line), want, "{line}");
    }
    // Digits in a reading heading stay allowed.
    assert_eq!(readings::check("## Reading: H1/code\n\nok\n"), vec![]);
}
