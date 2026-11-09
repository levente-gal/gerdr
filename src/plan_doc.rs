//! Show in plan: Plan review's suggestions as flagged comments on the lines
//! of `plan.md` they apply to, in a doc tab of their own (`DocFile::Suggestions`).

use crate::Gerdr;

impl Gerdr {
    /// Review feature `slug`'s suggestions where they apply in the plan (the
    /// sheet, for now).
    pub(crate) fn review_in_plan(&mut self, project: &str, slug: &str) {
        self.open_plan_sheet(project, slug);
    }

    /// Bring what shows Plan review's suggestions in line with the record:
    /// after a save, and as the watch reads a rewrite in.
    pub(crate) fn plan_doc_follows(&mut self) {
        self.plan_sheet_follows();
    }
}
