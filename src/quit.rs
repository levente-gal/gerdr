//! Closing gerdr with a project open: keep its session running in the
//! background (the default), or stop it. "Don't ask again" remembers the
//! choice; the quit action (prefix, Shift+Q) asks again regardless.
//!
//! Switching to another project asks the same dialog whether to stop the
//! one shown, its apps, terminals and agents, or keep it running and open
//! the other in a new gerdr (the default).
//!
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one, K and S press Keep running and Stop project (N and S, Open in new
//! gerdr and Switch, when switching), Space checks "Don't ask again", Esc
//! cancels.

use std::{os::unix::process::CommandExt, path::PathBuf, process::Stdio};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    chrome::{self, QuitHit, QuitView},
    herdr, launch,
    projects::QuitChoice,
    toast::Level,
};

const STOP: usize = 1;
const KEEP: usize = 2;

/// The quit dialog's state, while it is open.
#[derive(Debug)]
pub struct QuitDialog {
    /// 0 Cancel, 1 Stop project, 2 Keep running (as `QuitView::selected`);
    /// when switching, 0 Cancel, 1 Switch, 2 Open in new gerdr.
    pub selected: usize,
    pub dont_ask: bool,
    /// What the pointer is over.
    pub hover: Option<QuitHit>,
    /// The switch to another project it asks about, if not quitting.
    pub switch: Option<Switch>,
}

impl Default for QuitDialog {
    /// Keep running selected, as the safe choice.
    fn default() -> Self {
        Self {
            selected: KEEP,
            dont_ask: false,
            hover: None,
            switch: None,
        }
    }
}

/// A switch to another project, asked about (see `Gerdr::ask_switch`).
#[derive(Debug)]
pub struct Switch {
    /// The project to show.
    pub id: String,
    pub then: Then,
    /// The project was made for the switch: it goes if the switch is
    /// cancelled.
    pub created: bool,
}

/// What showing the project switched to comes with.
#[derive(Debug)]
pub enum Then {
    /// As `Gerdr::enter_project`: `folders` opened in it, or with none
    /// and `pick`, a folder asked for when it has no workspaces.
    Enter { pick: bool, folders: Vec<PathBuf> },
    /// `folder` opened in it as a workspace of `kind` (see
    /// `Gerdr::create_project_with`).
    Open {
        folder: Option<PathBuf>,
        kind: WorkspaceKind,
    },
}

/// What the quit dialog asks gerdr to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Quit, leaving the session running; `remember` if "Don't ask again" was
    /// checked.
    Keep {
        remember: bool,
    },
    /// Quit and stop the session.
    Stop {
        remember: bool,
    },
    Cancel,
}

impl QuitDialog {
    /// What pressing button `button` asks for.
    fn press(&self, button: usize) -> Outcome {
        let remember = self.dont_ask;
        match button {
            STOP => Outcome::Stop { remember },
            KEEP => Outcome::Keep { remember },
            _ => Outcome::Cancel,
        }
    }

    /// Handle a key: `sym` is the key's own symbol (`typed`, with Shift and
    /// the layout applied, isn't needed).
    fn key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let switching = self.switch.is_some();
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return Some(self.press(self.selected)),
            Keysym::Left | Keysym::KP_Left | Keysym::ISO_Left_Tab => self.step(-1),
            Keysym::Tab if mods.shift => self.step(-1),
            Keysym::Right | Keysym::KP_Right | Keysym::Tab => self.step(1),
            Keysym::space if plain && !switching => self.dont_ask = !self.dont_ask,
            Keysym::k | Keysym::K if plain && !switching => return Some(self.press(KEEP)),
            Keysym::n | Keysym::N if plain && switching => return Some(self.press(KEEP)),
            Keysym::s | Keysym::S if plain => return Some(self.press(STOP)),
            _ => {}
        }
        None
    }

    /// Move the selection `by` buttons, wrapping around.
    fn step(&mut self, by: isize) {
        self.selected = (self.selected as isize + by).rem_euclid(3) as usize;
    }
}

impl Gerdr {
    /// The window is closing: quit right away without a project, do what
    /// was remembered, or ask.
    pub fn request_quit(&mut self) {
        let Some(id) = self.project.as_deref() else {
            self.quit_app(false);
            return;
        };
        // Not connected to its session: ask only if it runs all the same.
        if self.herdr.is_none()
            && !self
                .projects
                .project(id)
                .is_some_and(|p| herdr::is_running(&p.session))
        {
            self.quit_app(false);
            return;
        }
        match self.projects.quit_choice {
            Some(QuitChoice::Keep) => self.quit_app(false),
            Some(QuitChoice::Stop) => self.quit_app(true),
            None => self.open_quit(),
        }
    }

    /// Ask, even if a choice was remembered.
    pub fn open_quit(&mut self) {
        self.project_menu = None;
        self.quit = Some(QuitDialog {
            dont_ask: self.projects.quit_choice.is_some(),
            ..QuitDialog::default()
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// A key while the quit dialog has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn quit_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let _ = typed;
        self.quit.as_mut()?.key(mods, sym)
    }

    /// Do what the quit dialog asked.
    pub fn apply_quit(&mut self, outcome: Outcome) {
        if let Some(switch) = self.quit.as_mut().and_then(|d| d.switch.take()) {
            self.quit = None;
            self.refresh_pointer_focus();
            return match outcome {
                Outcome::Stop { .. } => self.switch_now(switch, true),
                Outcome::Keep { .. } => self.open_in_new_gerdr(switch),
                Outcome::Cancel if switch.created => self.forget_project(&switch.id),
                Outcome::Cancel => {}
            };
        }
        let (choice, remember) = match outcome {
            Outcome::Cancel => {
                self.quit = None;
                // Gone, what is under the pointer gets it back.
                self.refresh_pointer_focus();
                return;
            }
            Outcome::Keep { remember } => (QuitChoice::Keep, remember),
            Outcome::Stop { remember } => (QuitChoice::Stop, remember),
        };
        self.projects.quit_choice = remember.then_some(choice);
        self.projects.save();
        self.quit = None;
        self.quit_app(choice == QuitChoice::Stop);
    }

    /// Show project `switch.id` as `switch.then` says: right away when no
    /// other project is shown, else once asked whether to stop the one
    /// shown or keep it and open the other in a new gerdr.
    pub fn ask_switch(&mut self, switch: Switch) {
        let shown = self.project.as_deref();
        if shown.is_none_or(|id| id == switch.id) {
            return self.switch_now(switch, false);
        }
        // Not a question: it can't be shown here anyway.
        if self.open_elsewhere(&switch.id) {
            return self.refuse_open(&switch.id);
        }
        self.project_menu = None;
        self.quit = Some(QuitDialog {
            switch: Some(switch),
            ..QuitDialog::default()
        });
        self.refresh_pointer_focus();
    }

    /// Show the project `switch` is about, stopping the one left if
    /// `stop` (see `show_project`).
    fn switch_now(&mut self, switch: Switch, stop: bool) {
        self.stop_left = stop.then(|| self.project.clone()).flatten();
        match switch.then {
            Then::Enter { pick, folders } => self.enter_project(&switch.id, pick, folders),
            Then::Open { folder, kind } => self.enter_project_with(&switch.id, folder, kind),
        }
    }

    /// Show the project `switch` is about in a new gerdr, which keeps the
    /// one shown running here.
    fn open_in_new_gerdr(&mut self, switch: Switch) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => return self.toast(format!("Couldn't start gerdr: {err}"), Level::Error),
        };
        let folders = match switch.then {
            Then::Enter { folders, .. } => folders,
            Then::Open {
                folder: Some(folder),
                kind: WorkspaceKind::Project,
            } => vec![folder],
            Then::Open { .. } => Vec::new(),
        };
        let mut cmd = std::process::Command::new(exe);
        // gerdr's own environment: the new one opens on the same host.
        cmd.arg("--project")
            .arg(&switch.id)
            .arg("--")
            .args(folders)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0);
        if let Some(log) = launch::log_file("gerdr") {
            cmd.stderr(Stdio::from(log));
        }
        match cmd.spawn() {
            Ok(child) => {
                self.children.push(child);
                let name = self
                    .projects
                    .project(&switch.id)
                    .map_or(switch.id.clone(), |p| p.name.clone());
                self.show_notice(format!("Opening {name} in a new gerdr"));
            }
            Err(err) => self.toast(format!("Couldn't start gerdr: {err}"), Level::Error),
        }
    }

    /// A left click while the quit dialog is open; `false` if it isn't.
    pub fn quit_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let layout = self.quit_layout();
        let Some(dialog) = self.quit.as_mut() else {
            return false;
        };
        let outcome = match quit_hit(&layout, pos) {
            Some(QuitHit::Button(button)) => Some(dialog.press(button)),
            Some(QuitHit::Checkbox) if dialog.switch.is_none() => {
                dialog.dont_ask = !dialog.dont_ask;
                None
            }
            Some(QuitHit::Checkbox) => None,
            None if layout.panel.to_f64().contains(pos) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_quit(outcome);
        }
        true
    }

    /// Where the quit dialog's parts go, as it asks now.
    pub fn quit_layout(&self) -> chrome::QuitLayout {
        let switching = self.quit.as_ref().is_some_and(|d| d.switch.is_some());
        chrome::quit_layout(self.output_size(), switching)
    }

    /// The pointer moved while the quit dialog is open.
    pub fn quit_hover(&mut self, pos: Point<f64, Logical>) {
        let layout = self.quit_layout();
        if let Some(dialog) = self.quit.as_mut() {
            dialog.hover = quit_hit(&layout, pos);
        }
    }

    pub fn quit_view(&self) -> Option<QuitView> {
        let dialog = self.quit.as_ref()?;
        let name = |id: &str| {
            self.projects
                .project(id)
                .map_or_else(|| id.to_owned(), |p| p.name.clone())
        };
        Some(QuitView {
            project: self.project.as_deref().map(name).unwrap_or_default(),
            selected: dialog.selected,
            dont_ask: dialog.dont_ask,
            hover: dialog.hover,
            switch_to: dialog.switch.as_ref().map(|s| name(&s.id)),
        })
    }
}

/// What `pos` (output coordinates) is over in the quit dialog.
fn quit_hit(layout: &chrome::QuitLayout, pos: Point<f64, Logical>) -> Option<QuitHit> {
    if let Some(button) = layout.buttons.iter().position(|b| b.to_f64().contains(pos)) {
        return Some(QuitHit::Button(button));
    }
    layout
        .checkbox
        .to_f64()
        .contains(pos)
        .then_some(QuitHit::Checkbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(dialog: &mut QuitDialog, sym: Keysym) -> Option<Outcome> {
        dialog.key(&ModifiersState::default(), sym)
    }

    #[test]
    fn keep_running_is_the_default() {
        let mut dialog = QuitDialog::default();
        assert_eq!(dialog.selected, KEEP);
        assert_eq!(
            press(&mut dialog, Keysym::Return),
            Some(Outcome::Keep { remember: false })
        );
    }

    #[test]
    fn selection_cycles_and_wraps() {
        let mut dialog = QuitDialog::default();
        assert_eq!(press(&mut dialog, Keysym::Right), None);
        assert_eq!(dialog.selected, 0);
        press(&mut dialog, Keysym::Left);
        assert_eq!(dialog.selected, KEEP);
        press(&mut dialog, Keysym::Left);
        assert_eq!(dialog.selected, STOP);
        press(&mut dialog, Keysym::Tab);
        assert_eq!(dialog.selected, KEEP);
        press(&mut dialog, Keysym::ISO_Left_Tab);
        assert_eq!(dialog.selected, STOP);
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        dialog.key(&shift, Keysym::Tab);
        assert_eq!(dialog.selected, 0);
        assert_eq!(press(&mut dialog, Keysym::Return), Some(Outcome::Cancel));
    }

    #[test]
    fn enter_presses_the_selected_button() {
        let mut dialog = QuitDialog::default();
        press(&mut dialog, Keysym::Left);
        assert_eq!(
            press(&mut dialog, Keysym::KP_Enter),
            Some(Outcome::Stop { remember: false })
        );
    }

    #[test]
    fn space_checks_dont_ask_again() {
        let mut dialog = QuitDialog::default();
        assert_eq!(press(&mut dialog, Keysym::space), None);
        assert!(dialog.dont_ask);
        assert_eq!(
            press(&mut dialog, Keysym::Return),
            Some(Outcome::Keep { remember: true })
        );
        press(&mut dialog, Keysym::space);
        assert!(!dialog.dont_ask);
    }

    #[test]
    fn switching_keys() {
        let mut dialog = QuitDialog {
            switch: Some(Switch {
                id: "aft".into(),
                then: Then::Enter {
                    pick: true,
                    folders: Vec::new(),
                },
                created: false,
            }),
            ..Default::default()
        };
        assert_eq!(
            press(&mut dialog, Keysym::Return),
            Some(Outcome::Keep { remember: false })
        );
        assert_eq!(press(&mut dialog, Keysym::space), None);
        assert!(!dialog.dont_ask);
        assert_eq!(press(&mut dialog, Keysym::k), None);
        assert_eq!(
            press(&mut dialog, Keysym::n),
            Some(Outcome::Keep { remember: false })
        );
        assert_eq!(
            press(&mut dialog, Keysym::s),
            Some(Outcome::Stop { remember: false })
        );
    }

    #[test]
    fn letters_and_escape() {
        let mut dialog = QuitDialog {
            dont_ask: true,
            ..Default::default()
        };
        assert_eq!(press(&mut dialog, Keysym::Escape), Some(Outcome::Cancel));
        assert_eq!(
            press(&mut dialog, Keysym::k),
            Some(Outcome::Keep { remember: true })
        );
        assert_eq!(
            press(&mut dialog, Keysym::s),
            Some(Outcome::Stop { remember: true })
        );
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(dialog.key(&ctrl, Keysym::s), None);
        assert_eq!(press(&mut dialog, Keysym::x), None);
    }
}
