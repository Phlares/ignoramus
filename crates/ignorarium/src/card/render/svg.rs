//! One figure per hypothesis, as SVG.
use super::{alt_text, group, margins, rows_of, slug};
use crate::card::Card;
use crate::card::sections::ResultRow;

const INK: &str = "#1a1a1a";
const POINT: &str = "#1f5fa8";
const MARGIN: &str = "#b3261e";

/// The file the Markdown links for the figure of hypothesis `h`.
pub(super) fn figure_file(h: &str) -> String {
    format!("figure-{}.svg", slug(h))
}

/// One figure file per claim: (file name, SVG). Two hypotheses that name the same file are refused.
pub fn figures(c: &Card) -> Result<Vec<(String, String)>, String> {
    let mut from = std::collections::BTreeMap::new();
    let mut out = vec![];
    for l in &c.claims {
        let file = figure_file(&l.hypothesis);
        if let Some(first) = from.insert(file.clone(), &l.hypothesis) {
            return Err(format!(
                "{file}: two hypotheses share it: {first:?} and {:?}",
                l.hypothesis
            ));
        }
        out.push((file, figure(c, &l.hypothesis)));
    }
    Ok(out)
}

fn xml(s: &str) -> String {
    (s.replace('&', "&amp;"))
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The figure of hypothesis `h`: each group's estimate as a dot, a line to its interval bound,
/// and a dashed line at each registered margin. Light theme, opaque fixed colours.
pub fn figure(c: &Card, h: &str) -> String {
    const WIDTH: f64 = 560.0;
    const LEFT: f64 = 90.0;
    const ROW: f64 = 36.0;
    const TOP: f64 = 28.0;
    let rows = rows_of(c, h);
    let claim = (c.claims.iter())
        .find(|l| l.hypothesis == h)
        .and_then(|l| l.claim);
    let marks = claim.map(|m| margins(&m)).unwrap_or_default();
    let ends = |r: &ResultRow| {
        let bounds = r.intervals.iter().filter_map(|i| i.bound);
        std::iter::once(r.estimate)
            .chain(bounds)
            .collect::<Vec<_>>()
    };
    let all: Vec<f64> = (rows.iter().flat_map(|r| ends(r)))
        .chain(marks.iter().copied())
        .collect();
    let lo = all.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if lo < hi {
        (lo, hi)
    } else {
        (lo - 1.0, lo + 1.0)
    };
    let pad = (hi - lo) * 0.1;
    let x = |v: f64| LEFT + (v - lo + pad) / (hi - lo + 2.0 * pad) * (WIDTH - LEFT - 20.0);
    let height = TOP + ROW * rows.len() as f64 + 12.0;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" \
         viewBox=\"0 0 {WIDTH} {height}\" role=\"img\" font-family=\"sans-serif\" font-size=\"12\">\n\
         <title>{}</title>\n<rect width=\"{WIDTH}\" height=\"{height}\" fill=\"#ffffff\"/>\n",
        xml(&alt_text(c, h))
    );
    for m in &marks {
        let at = x(*m);
        s += &format!(
            "<line x1=\"{at:.1}\" y1=\"{}\" x2=\"{at:.1}\" y2=\"{}\" stroke=\"{MARGIN}\" \
             stroke-width=\"1.5\" stroke-dasharray=\"4 3\"/>\n\
             <text x=\"{at:.1}\" y=\"14\" fill=\"{MARGIN}\" text-anchor=\"middle\">margin {m}</text>\n",
            TOP - 8.0,
            height - 8.0
        );
    }
    for (n, r) in rows.iter().enumerate() {
        let y = TOP + ROW * (n as f64 + 0.5);
        s += &format!(
            "<text x=\"8\" y=\"{:.1}\" fill=\"{INK}\">{}</text>\n",
            y + 4.0,
            xml(group(&r.stratum))
        );
        for b in r.intervals.iter().filter_map(|i| i.bound) {
            s += &format!(
                "<line x1=\"{:.1}\" y1=\"{y:.1}\" x2=\"{:.1}\" y2=\"{y:.1}\" stroke=\"{POINT}\" stroke-width=\"2\"/>\n",
                x(r.estimate),
                x(b)
            );
        }
        s += &format!(
            "<circle cx=\"{:.1}\" cy=\"{y:.1}\" r=\"4.5\" fill=\"{POINT}\"/>\n",
            x(r.estimate)
        );
    }
    s + "</svg>\n"
}
