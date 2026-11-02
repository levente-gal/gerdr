//! Code review of features (inc 5, agent B): the feature's diff over all
//! its touched repositories in the diff view, its files' viewed ticks (see
//! `feature::viewed`), AI review's findings shown as comments on their
//! lines with Fix and Dismiss, and the user's own comments, sent to the
//! Build agent, or Code review approved once every file is viewed and no
//! comment is left.

#![allow(dead_code)] // inc 5 contract: filled in by agent B

use crate::Gerdr;

impl Gerdr {
    /// Show feature `slug`'s diff across the repositories panel, as a
    /// worktree's diff shows; Esc goes back to the feature panel. With
    /// `finding`, the cursor goes to that AI review finding.
    pub fn open_feature_diff(&mut self, project: &str, slug: &str, finding: Option<&str>) {
        let _ = (project, slug, finding);
    }

    /// What the feature panel's Code review row says of feature `slug`,
    /// once its diff was read: "3 of 7 files viewed · 2 comments". `None`
    /// before.
    pub fn code_review_line(&self, project: &str, slug: &str) -> Option<String> {
        let _ = (project, slug);
        None
    }
}
