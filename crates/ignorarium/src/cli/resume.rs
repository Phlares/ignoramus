//! `resume`: clear a pause and run the rows that have no record.
use super::{Ctx, Run};

pub fn run(ctx: &Ctx, id: &str, box_changed: Option<String>) -> Run {
    super::run::go(ctx, id, None, true, box_changed)
}
