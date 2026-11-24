//! The exit codes of the `ignorarium` binary: the one table that every verb returns from and
//! that `dev gen-exit-codes` writes to `docs/exit-codes.md`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Done,
    Refused,
    Fault,
    Pending,
    /// Apart from the 1 to 3 range of the box's own faults, so a caller can tell a failed
    /// sandbox check from a box that could not run it.
    SelftestFailed,
}

impl Code {
    pub const ALL: [Code; 5] = [
        Code::Done,
        Code::Refused,
        Code::Fault,
        Code::Pending,
        Code::SelftestFailed,
    ];

    pub const fn number(self) -> i32 {
        match self {
            Code::Done => 0,
            Code::Refused => 1,
            Code::Fault => 2,
            Code::Pending => 3,
            Code::SelftestFailed => 102,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Code::Done => "done",
            Code::Refused => "refused",
            Code::Fault => "fault",
            Code::Pending => "pending",
            Code::SelftestFailed => "selftest-failed",
        }
    }

    pub const fn meaning(self) -> &'static str {
        match self {
            Code::Done => {
                "The verb did its work. For `check` and `review`: the plan has no error and no value is pending. For `verify`: every check passed, or, without `--study`, the home holds no key and no plan or test log entry."
            }
            Code::Refused => {
                "The box did the work and the answer is no. Examples: `check` found an error in the plan; the study is not registered, not finished, paused or stopped; `verify` found a fault."
            }
            Code::Fault => {
                "The box could not do the work. Examples: a command line it cannot parse; a file or key it cannot read or write. The notes below name the exceptions."
            }
            Code::Pending => {
                "Only `check` and `review`: the plan has no error, and at least one value waits on a study that has not finished."
            }
            Code::SelftestFailed => "Only `dev selftest`: a check of the sandbox failed.",
        }
    }

    /// What clap's parse result exits with: `Fault` when clap prints to stderr, else `Done`.
    pub fn of_parse(e: &clap::Error) -> Code {
        if e.use_stderr() {
            Code::Fault
        } else {
            Code::Done
        }
    }
}

/// `docs/exit-codes.md`.
pub fn page() -> String {
    let rows: String = (Code::ALL.iter())
        .map(|c| format!("| {} | {} | {} |\n", c.number(), c.name(), c.meaning()))
        .collect();
    format!(
        "# Exit codes\n\n\
         `ignorarium dev gen-exit-codes` writes this page from \
         `crates/ignorarium/src/cli/exit.rs`. Do not edit it by hand.\n\n\
         | Code | Name | Meaning |\n|---|---|---|\n{rows}\n\
         ## Notes\n\n\
         - `--help` and `--version` exit 0.\n\
         - A study's record log that fails verification (a torn line, a bad hash, a newer \
         format; by the same code path a bad signature or chain), or that the box opens but \
         cannot read to its end, ends `report` and `verify --study` with 1, and `status`, `run`, \
         `resume`, `pause`, `stop` and `stamp --request` with 2. A log that `report` cannot \
         open ends it with 2.\n\
         - `verify` exits 1 when it cannot read or verify the plan log or the test log.\n"
    )
}

/// Lets a test compare a code with its number.
impl PartialEq<i32> for Code {
    fn eq(&self, n: &i32) -> bool {
        self.number() == *n
    }
}

#[cfg(test)]
mod tests {
    use super::Code;
    use clap::error::ErrorKind;
    use std::collections::BTreeSet;

    #[test]
    fn the_numbers_are_the_published_ones() {
        assert_eq!(Code::ALL.map(Code::number), [0, 1, 2, 3, 102]);
    }

    #[test]
    fn no_two_codes_share_a_number_or_a_name() {
        let numbers: BTreeSet<i32> = Code::ALL.iter().map(|c| c.number()).collect();
        let names: BTreeSet<&str> = Code::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(
            (numbers.len(), names.len()),
            (Code::ALL.len(), Code::ALL.len())
        );
    }

    #[test]
    fn of_parse_agrees_with_clap() {
        for kind in [
            ErrorKind::DisplayHelp,
            ErrorKind::DisplayVersion,
            ErrorKind::UnknownArgument,
        ] {
            let e = clap::Error::new(kind);
            assert_eq!(Code::of_parse(&e).number(), e.exit_code(), "{kind:?}");
        }
    }
}
