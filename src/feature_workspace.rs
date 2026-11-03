//! A feature's own workspace: the workspace of its home project open on the
//! feature folder. Its agents, squash tab and Zed open there, not in
//! whichever workspace is active. It is a normal workspace in every other
//! way, found again by its folder.

use std::path::Path;

use crate::Gerdr;

impl Gerdr {
    /// The index of feature `slug` of `project`'s workspace, if one is open.
    pub fn feature_workspace(&self, project: &str, slug: &str) -> Option<usize> {
        // Contract stub (wave 0): wave 1 finds it by folder.
        let _ = (project, slug);
        None
    }

    /// Open a tab in feature `slug` of `project`'s workspace, whose shell
    /// starts in `cd` running `command` if any, and show it; the workspace
    /// opens with that tab first when none is open. The herdr pane of the
    /// tab's terminal, when herdr says; `None` (with a toast) when it
    /// couldn't open.
    pub fn open_feature_tab(
        &mut self,
        project: &str,
        slug: &str,
        cd: &Path,
        command: Option<String>,
    ) -> Option<String> {
        // Contract stub (wave 0): the active workspace, as before.
        let _ = (project, slug);
        if self.active_workspace >= self.workspaces.len() {
            self.show_error("No workspace to open the feature's tab in: open one first".to_owned());
            return None;
        }
        let w = self.active_workspace;
        self.new_tab_pane(w, cd, command)
    }
}
