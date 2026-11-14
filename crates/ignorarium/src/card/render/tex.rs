//! The card as LaTeX fragments: plain `tabular` tables and one file per inline value.
use super::{
    Table, bound_text, deviations_table, e_text, flat, pct, results_table, sensitivity_table, sig4,
    slug,
};
use crate::card::Card;
use crate::card::sections::ResultRow;

/// `\ { } $ & # ^ _ % ~` made safe in LaTeX text.
pub fn tex_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\textbackslash{}".into(),
            '^' => "\\textasciicircum{}".into(),
            '~' => "\\textasciitilde{}".into(),
            '{' | '}' | '$' | '&' | '#' | '_' | '%' => format!("\\{c}"),
            c => c.to_string(),
        })
        .collect()
}

fn tabular(t: &Table) -> String {
    let row = |cells: Vec<String>| format!("{} \\\\ \\hline\n", cells.join(" & "));
    let spec = vec!["l"; t.head.len()].join(" ");
    let mut s = format!("\\begin{{tabular}}{{{spec}}}\n\\hline\n");
    s += &row(t.head.iter().map(|h| tex_escape(h)).collect());
    for r in &t.rows {
        s += &row(r.iter().map(|c| tex_escape(&flat(c))).collect());
    }
    s + "\\end{tabular}\n"
}

/// The inline values of one result row: field name and text.
fn values(r: &ResultRow) -> Vec<(String, String)> {
    let mut v = vec![("estimate".to_string(), sig4(r.estimate))];
    v.extend(
        r.percent_change
            .map(|p| ("percent-change".into(), pct(p, true))),
    );
    v.extend(r.e_value.map(|e| ("e-value".into(), e_text(e))));
    for i in &r.intervals {
        v.extend(bound_text(i).map(|b| (format!("bound-{}", slug(&i.side)), b)));
    }
    v
}

/// The LaTeX fragments of the card: (file name, contents). Every table is a plain `tabular`.
/// Two values that name the same file are refused.
pub fn latex(c: &Card) -> Result<Vec<(String, String)>, String> {
    let mut out = vec![
        ("study.tex".to_string(), tex_escape(&c.study)),
        ("results.tex".into(), tabular(&results_table(c))),
        ("deviations.tex".into(), tabular(&deviations_table(c))),
        ("sensitivity.tex".into(), tabular(&sensitivity_table(c))),
    ];
    let mut from = std::collections::BTreeMap::new();
    for r in &c.results {
        for (name, text) in values(r) {
            let (h, g) = (slug(&r.hypothesis), slug(&r.stratum));
            let file = format!("value-{h}-{g}-{name}.tex");
            let source = format!("({:?}, {:?}, {name})", r.hypothesis, r.stratum);
            if let Some(first) = from.insert(file.clone(), source.clone()) {
                return Err(format!(
                    "{file}: two values (hypothesis, group, field) share it: {first} and {source}"
                ));
            }
            out.push((file, tex_escape(&text)));
        }
    }
    Ok(out)
}
