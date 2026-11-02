//! The AI review stage of features, run by gerdr (inc 5, agent A): the
//! reviewer agent, its `review.json` read in as findings (see
//! `feature::review_import`), the user's Fix and Dismiss, and the fixes
//! sent to the Build agent.

#![allow(dead_code)] // inc 5 contract: filled in by agent A

use crate::{Gerdr, feature::review_import::Decision};

impl Gerdr {
    /// Decide finding `id` of feature `slug`'s AI review: Fix, Dismiss
    /// with a reason, or back to undecided. Saves the record; tells the
    /// user why when it can't (the feature isn't at AI review, no such
    /// finding, an empty reason).
    pub fn decide_finding(&mut self, project: &str, slug: &str, id: &str, decision: Decision) {
        let _ = (project, slug, id, decision);
    }

    /// Send `text` (Markdown) to feature `slug`'s Build agent to act on:
    /// into its tab while it lives, else to a fresh Build agent with it.
    /// `what` names it for the log and the toast: "review comments",
    /// "2 findings". Whether it went.
    pub(crate) fn send_to_build(
        &mut self,
        project: &str,
        slug: &str,
        what: &str,
        text: String,
    ) -> bool {
        let _ = (project, slug, what, text);
        false
    }
}
