//! The Markdown and LaTeX renderers and the figure: pure functions of one card.
use ignorarium::card::Card;
use ignorarium::card::render::{figure, latex, markdown, tex_escape};
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
    latex(c)
}

#[test]
fn markdown_golden() {
    let got = markdown(&card());
    let path = fixture("e1-like.md");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(got, want);
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

#[test]
fn display_rounding_is_applied_here() {
    let md = markdown(&with_bound(Some(0.039_212_34), true));
    assert!(
        md.contains("below 0.0392 "),
        "bound to the grid's 4 decimals"
    );
    assert!(
        md.contains("inverted registered e-process"),
        "method beside the bound"
    );
    assert!(md.contains("+2.91%"), "percent change, 2 decimals");
    assert!(md.contains("3.100"), "e-value, 4 significant digits");
    assert!(!md.contains("CLT"));
    let mut c = card();
    c.results[0].e_value = Some(312.456_7);
    assert!(markdown(&c).contains("312.5"));
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
    c.results[1].stratum = String::new();
    let all = files(&c);
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        "value-h-1-x-code-a-estimate.tex",
        "value-h-1-x-code-a-percent-change.tex",
        "value-h-1-x-code-a-e-value.tex",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    assert!(names.contains(&"value-h1-all-estimate.tex"), "{names:?}");
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
