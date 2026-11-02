//! Reviewing a stage's file (`brief.md`, `plan.md`) in the diff view: the
//! file shown as all added lines, so line comments work as on code, with
//! Approve and Send back on its header (see `feature_agents`).

use crate::{Gerdr, feature::Stage};

impl Gerdr {
    /// Show `stage`'s file of feature `slug` for review.
    pub fn open_doc_review(&mut self, project: &str, slug: &str, stage: Stage) {
        let _ = (project, slug, stage);
        todo!("doc_review: open_doc_review")
    }
}
