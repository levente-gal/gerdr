//! Renaming a workspace or a tab in place: the field being edited, and
//! writing the new name where it is kept.

use crate::{Gerdr, text_field::TextField};

/// What is being renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Workspace(usize),
    /// Tab `t` of workspace `w`.
    Tab(usize, usize),
}

/// A rename in progress: the name typed so far.
#[derive(Clone, Debug)]
pub struct Rename {
    pub target: RenameTarget,
    pub field: TextField,
}

impl Gerdr {
    /// Start renaming `target`, its name in the field to edit.
    pub fn start_rename(&mut self, target: RenameTarget) {
        let current = match target {
            RenameTarget::Workspace(w) => self.workspaces.get(w).map(|ws| ws.name.clone()),
            RenameTarget::Tab(w, t) => self
                .workspaces
                .get(w)
                .and_then(|ws| ws.tabs.get(t))
                .map(|tab| tab.label.clone().unwrap_or_default()),
        };
        let Some(current) = current else {
            return;
        };
        let mut field = TextField::from(current);
        field.select_all();
        self.rename = Some(Rename { target, field });
    }

    /// Write the name typed into the rename target, and stop renaming.
    pub fn commit_rename(&mut self) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let name = rename.field.trim().to_owned();
        match rename.target {
            RenameTarget::Workspace(w) if !name.is_empty() => self.rename_workspace(w, name),
            RenameTarget::Tab(w, t) => self.rename_tab(w, t, name),
            RenameTarget::Workspace(_) => {}
        }
    }

    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Call workspace `w` `name`, here and in the project's records.
    pub fn rename_workspace(&mut self, w: usize, name: String) {
        let Some(ws) = self.workspaces.get_mut(w) else {
            return;
        };
        ws.name = name.clone();
        let herdr_id = ws.herdr_id().map(str::to_owned);
        if let Some((project, herdr_id)) = self.project.clone().zip(herdr_id)
            && let Some(record) = self.projects.record_mut(&project, &herdr_id)
            && record.label != name
        {
            record.label = name;
            self.projects.save();
        }
    }

    /// Call tab `t` of workspace `w` `name` (empty: back to its derived
    /// title), here and in the project's records.
    pub fn rename_tab(&mut self, w: usize, t: usize, name: String) {
        let Some(tab) = self.workspaces.get_mut(w).and_then(|ws| ws.tabs.get_mut(t)) else {
            return;
        };
        tab.label = (!name.is_empty()).then_some(name);
        if self.record_tabs() {
            self.projects.save();
        }
    }
}
