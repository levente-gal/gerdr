//! The files agent stages write into the feature data dir, and the checks
//! that make one good enough to review: `brief.md` (Discover) and
//! `plan.md` (Plan).
//!
//! A brief needs an `## Outcome` section, `## In scope` and `## Out of
//! scope`, and at least one acceptance criterion in Given/When/Then form.
//! A plan needs `## Milestones` with at least one item, `## Repositories`
//! naming the touched ones, `## Merge order`, and a `Migrations:` line.
//! Headings match case-insensitively.

use std::path::{Path, PathBuf};

use super::Stage;

/// The file `stage` writes, if any: `brief.md`, `plan.md`.
pub fn name(stage: Stage) -> Option<&'static str> {
    todo!("artifacts: name {stage:?}")
}

/// Where `stage`'s file is, under feature data dir `dir`.
pub fn path(dir: &Path, stage: Stage) -> Option<PathBuf> {
    Some(dir.join(name(stage)?))
}

/// Whether `text` is a good `stage` file; else what it lacks, one line
/// each ("no Outcome section").
pub fn check(stage: Stage, text: &str) -> Result<(), Vec<String>> {
    todo!("artifacts: check {stage:?} {}", text.len())
}
