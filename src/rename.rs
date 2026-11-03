//! Renaming a workspace or a tab in place: the field being edited, the
//! keys that edit it, and writing the new name where it is kept.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, FieldSpot},
    input::BTN_LEFT,
    text_field::TextField,
};

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
                // A feature's panel tab and a file's tab keep their names.
                .filter(|tab| !tab.is_feature() && tab.review().is_none())
                .map(|tab| tab.label.clone().unwrap_or_default()),
        };
        let Some(current) = current else {
            return;
        };
        let mut field = TextField::from(current);
        field.select_all();
        self.rename = Some(Rename { target, field });
    }

    /// A key while renaming: Enter keeps the name, Esc drops it, the rest
    /// edits it. Whether a rename took the key (every key while one runs).
    pub fn rename_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> bool {
        if self.rename.is_none() {
            return false;
        }
        match sym {
            Keysym::Return | Keysym::KP_Enter => self.commit_rename(),
            Keysym::Escape => self.cancel_rename(),
            _ => {
                if let Some(rename) = self.rename.as_mut() {
                    rename.field.key(mods, sym, typed);
                }
            }
        }
        true
    }

    /// A press at `pos` while renaming: a left press inside the field puts
    /// the caret there (Shift: the selection reaches there). Whether the
    /// press landed in the field; any other press commits the name.
    pub fn rename_press(&mut self, pos: Point<f64, Logical>, button: u32) -> bool {
        if button != BTN_LEFT {
            return false;
        }
        let Some(field) = self
            .rename_field_spot()
            .filter(|field| field.area.to_f64().contains(pos))
        else {
            return false;
        };
        let scale = self.scale();
        let at = self.chrome.caret_at(&field, pos.x, scale);
        let extend = self.seat.get_keyboard().unwrap().modifier_state().shift;
        if let Some(rename) = self.rename.as_mut() {
            rename.field.click(at, extend, 1);
        }
        true
    }

    /// Where the name being typed is drawn and what it shows, while the
    /// rename is on screen (a tab's only while its workspace is shown).
    pub fn rename_field_spot(&self) -> Option<FieldSpot> {
        let rename = self.rename.as_ref()?;
        match rename.target {
            RenameTarget::Tab(w, _) if w == self.active_workspace => {
                let view = self.tab_strip_view();
                chrome::tab_rename_field(&view, self.tab_strip_rect(), self.scale())
            }
            RenameTarget::Tab(..) => None,
            RenameTarget::Workspace(w) => {
                let area = self.sidebar_layout().rows.get(w)?.rename_field();
                Some(chrome::sidebar_rename_field(
                    area,
                    &rename.field.line(),
                    self.scale(),
                ))
            }
        }
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
            RenameTarget::Workspace(w) => {
                if let Some(old) = self.workspaces.get(w).map(|ws| ws.name.clone()) {
                    self.show_notice(format!("A workspace needs a name: it stays {old}"));
                }
            }
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
        let id = ws.record.clone();
        if let Some(project) = self.project.clone()
            && let Some(record) = self.projects.record_mut(&project, &id)
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
