//! Paths of this machine out of text that goes into a record: each named folder becomes a
//! fixed placeholder, so a backend's message can name what failed without naming the host.
use std::path::Path;

/// Spellings of folders and what replaces each, longest first, so a folder inside another
/// is replaced whole.
#[derive(Debug, Clone, Default)]
pub struct Redact(Vec<(String, &'static str)>);

impl Redact {
    /// Each folder of `named` in its given spelling, its absolute one (a relative folder
    /// joined to the working folder) and its canonical one (links resolved), as
    /// `placeholder`. Only absolute spellings count: a relative one or `/` would match inside
    /// any path.
    pub fn new(named: &[(&Path, &'static str)]) -> Redact {
        let mut all: Vec<(String, &'static str)> = Vec::new();
        for &(folder, placeholder) in named {
            let abs = std::path::absolute(folder).ok();
            let real = std::fs::canonicalize(folder).ok();
            for p in [Some(folder), abs.as_deref(), real.as_deref()]
                .into_iter()
                .flatten()
            {
                let text = p.to_string_lossy();
                let text = text.trim_end_matches('/');
                if p.is_absolute() && !text.is_empty() && !all.iter().any(|(t, _)| t == text) {
                    all.push((text.to_string(), placeholder));
                }
            }
        }
        all.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
        Redact(all)
    }

    /// `text` with every spelling replaced.
    pub fn text(&self, text: &str) -> String {
        (self.0.iter()).fold(text.to_string(), |t, (from, to)| t.replace(from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::Redact;
    use std::path::Path;

    #[test]
    fn the_longest_folder_goes_first_and_no_root_or_relative_spelling_counts() {
        let r = Redact::new(&[
            (Path::new("/u"), "~"),
            (Path::new("/u/box/"), "<home>"),
            (Path::new("/"), "<root>"),
            (Path::new("u"), "<rel>"),
        ]);
        assert_eq!(r.text("/u/box/x /u/y u/z /w"), "<home>/x ~/y u/z /w");
        assert_eq!(Redact::default().text("/u/x"), "/u/x");
    }

    #[test]
    fn a_relative_folder_counts_in_its_absolute_spelling() {
        let rel = Path::new("no-such-folder/box");
        let abs = std::path::absolute(rel).unwrap();
        let r = Redact::new(&[(rel, "<home>")]);
        assert_eq!(r.text(&format!("{}/x", abs.display())), "<home>/x");
    }
}
