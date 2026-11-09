//! The drafts of a stage's file (`brief.md`, `plan.md`): each version its
//! agent wrote for review, kept as it was, with who wrote it, when, the
//! comments it was sent back with and when it was approved. Discover's and
//! Plan's pages list them as cards, and each shows what it changed from the
//! one before.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Stage, artifacts, fix_round::Asked};

/// The folder of a feature's data folder the drafts are kept in.
pub const DIR: &str = "drafts";

/// One draft of `stage`'s file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub stage: Stage,
    /// Counts the stage's drafts, from 1, across its agents' runs.
    pub k: u32,
    /// The stage's run that wrote it.
    #[serde(default)]
    pub run: u32,
    /// Its agent and model; empty if not known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Unix seconds it came to wait for review.
    pub written: u64,
    /// Unix seconds it was sent back, and the comments it went with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_back: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Asked>,
    /// Unix seconds it was approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<u64>,
}

/// Where draft `k` of `stage`'s file is kept, under feature data dir `dir`:
/// `drafts/plan-2.md`.
pub fn path(dir: &Path, stage: Stage, k: u32) -> Option<PathBuf> {
    let name = artifacts::name(stage)?;
    let stem = name.strip_suffix(".md").unwrap_or(name);
    Some(dir.join(DIR).join(format!("{stem}-{k}.md")))
}

/// `stage`'s drafts in `drafts`, newest first.
pub fn of(drafts: &[Draft], stage: Stage) -> Vec<&Draft> {
    let mut out: Vec<&Draft> = drafts.iter().filter(|d| d.stage == stage).collect();
    out.sort_by_key(|d| std::cmp::Reverse(d.k));
    out
}

/// The number the next draft of `stage` takes.
pub fn next(drafts: &[Draft], stage: Stage) -> u32 {
    of(drafts, stage).first().map_or(1, |d| d.k + 1)
}

/// Keep `stage`'s file as draft `k`, under feature data dir `dir`; why it
/// couldn't be.
pub fn keep(dir: &Path, stage: Stage, k: u32) -> Result<(), String> {
    let (Some(from), Some(to)) = (artifacts::path(dir, stage), path(dir, stage, k)) else {
        return Ok(());
    };
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("making {}: {e}", parent.display()))?;
    }
    std::fs::copy(&from, &to)
        .map(drop)
        .map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))
}

/// The latest draft of `stage` in `drafts`, to change.
pub fn latest(drafts: &mut [Draft], stage: Stage) -> Option<&mut Draft> {
    (drafts.iter_mut())
        .filter(|d| d.stage == stage)
        .max_by_key(|d| d.k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(stage: Stage, k: u32) -> Draft {
        Draft {
            stage,
            k,
            run: 1,
            agent: String::new(),
            model: String::new(),
            written: 0,
            sent_back: None,
            comments: Vec::new(),
            approved: None,
        }
    }

    #[test]
    fn drafts_count_per_stage_and_list_newest_first() {
        let mut drafts = vec![
            draft(Stage::Plan, 1),
            draft(Stage::Discover, 1),
            draft(Stage::Plan, 2),
        ];
        let ks: Vec<u32> = of(&drafts, Stage::Plan).iter().map(|d| d.k).collect();
        assert_eq!(ks, [2, 1]);
        assert_eq!(next(&drafts, Stage::Plan), 3);
        assert_eq!(next(&drafts, Stage::Discover), 2);
        assert_eq!(next(&drafts, Stage::Build), 1);
        latest(&mut drafts, Stage::Plan).unwrap().approved = Some(5);
        assert_eq!(drafts[2].approved, Some(5));
        let dir = Path::new("/d");
        assert_eq!(
            path(dir, Stage::Plan, 2),
            Some(PathBuf::from("/d/drafts/plan-2.md"))
        );
        assert_eq!(
            path(dir, Stage::Discover, 1),
            Some(PathBuf::from("/d/drafts/brief-1.md"))
        );
    }

    #[test]
    fn a_draft_kept_is_the_file_as_it_was() {
        let dir = std::env::temp_dir().join(format!("gerdr-drafts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plan.md"), "# Plan\nOne\n").unwrap();
        keep(&dir, Stage::Plan, 1).unwrap();
        std::fs::write(dir.join("plan.md"), "# Plan\nTwo\n").unwrap();
        let kept = std::fs::read_to_string(path(&dir, Stage::Plan, 1).unwrap()).unwrap();
        assert_eq!(kept, "# Plan\nOne\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
