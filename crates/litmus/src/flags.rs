use std::collections::BTreeMap;
use std::num::ParseIntError;
use std::str::FromStr;

/// The `--name value` pairs of a command line. Each flag is read once; `done` refuses the rest.
pub struct Flags(BTreeMap<String, String>);

impl Flags {
    pub fn parse(args: &[String]) -> Result<Flags, String> {
        let mut map = BTreeMap::new();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let val = it.next().ok_or(format!("{flag} needs a value"))?;
            if map.insert(flag.clone(), val.clone()).is_some() {
                return Err(format!("{flag} is given twice"));
            }
        }
        Ok(Flags(map))
    }

    pub fn take(&mut self, name: &str) -> Option<String> {
        self.0.remove(name)
    }

    pub fn require(&mut self, name: &str) -> Result<String, String> {
        self.take(name).ok_or(format!("{name} is required"))
    }

    /// The flag as a number, or `default` when absent.
    pub fn number<T: FromStr<Err = ParseIntError>>(
        &mut self,
        name: &str,
        default: T,
    ) -> Result<T, String> {
        match self.take(name) {
            Some(v) => v.parse().map_err(|e| format!("{name}: {e}")),
            None => Ok(default),
        }
    }

    pub fn done(self) -> Result<(), String> {
        match self.0.keys().next() {
            Some(flag) => Err(format!("unknown flag {flag}")),
            None => Ok(()),
        }
    }
}
