//! Closing gerdr with a project open asks what of it to stop: its herdr
//! session (its terminals and agents), its apps kept running (Cursor, see
//! `keep`), both or neither; the rest keeps running in the background.
//! The picks are checked again the next time. "Don't ask again" does them
//! without asking; the quit action (prefix, Shift+Q) asks again regardless.
//!
//! Switching to another project asks the same of the one shown, or to keep
//! it running and open the other in a new gerdr.
//!
//! Quit (Switch) is selected when it stops nothing; when it stops what is
//! checked it is painted in the error colour and Cancel is selected
//! instead, so Enter never stops a session by a slip.
//!
//! Up/Down move between the checkboxes and Space checks one; Left/Right
//! and Tab move between the buttons, Enter presses the selected one, Q
//! presses Quit (N and S, Open in new gerdr and Switch, when switching),
//! Esc cancels. The dialog says these keys by its buttons.

use std::{os::unix::process::CommandExt, path::PathBuf, process::Stdio};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    chrome::{self, QuitHit, QuitRow, QuitView},
    herdr, keep, launch,
    projects::{QuitChoice, Stops},
    toast::Level,
};

/// Cancel, the first.
const CANCEL: usize = 0;
/// Quit, or Switch when switching.
const GO: usize = 1;
/// Open in new gerdr, when switching.
const NEW_GERDR: usize = 2;

/// A checkbox of the quit dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// Stop the herdr session.
    Session,
    /// Stop the apps kept running.
    Apps,
    /// Stop T3 Code's server, which runs in the background for every
    /// project (see `t3_server`).
    T3,
    DontAsk,
}

/// The quit dialog's state, while it is open.
#[derive(Debug)]
pub struct QuitDialog {
    /// 0 Cancel, 1 Quit (as `QuitView::selected`); when switching,
    /// 0 Cancel, 1 Switch, 2 Open in new gerdr.
    pub selected: usize,
    /// What is checked to stop.
    pub stops: Stops,
    pub dont_ask: bool,
    /// The apps the project keeps running, as named for people: no row
    /// for them when there are none.
    pub apps: Vec<String>,
    /// Whether T3 Code's server runs: a row for it when quitting.
    pub t3: bool,
    /// The row under the keyboard's cursor.
    pub cursor: usize,
    /// What the pointer is over.
    pub hover: Option<QuitHit>,
    /// The switch to another project it asks about, if not quitting.
    pub switch: Option<Switch>,
}

impl QuitDialog {
    /// Asking to quit, with `stops` checked: Quit selected unless it
    /// stops something, Cancel then.
    fn quitting(stops: Stops, apps: Vec<String>, dont_ask: bool) -> Self {
        Self {
            selected: if stops.any() { CANCEL } else { GO },
            stops,
            dont_ask,
            apps,
            t3: false,
            cursor: 0,
            hover: None,
            switch: None,
        }
    }

    /// Asking about `switch`, with `stops` checked: Switch selected,
    /// what was asked for, unless it stops something, Cancel then.
    fn switching(stops: Stops, apps: Vec<String>, switch: Switch) -> Self {
        Self {
            switch: Some(switch),
            ..Self::quitting(stops, apps, false)
        }
    }

    /// With a row for T3 Code's server, if `runs`.
    fn with_t3(mut self, runs: bool) -> Self {
        self.t3 = runs;
        self
    }

    /// The checkboxes, in order.
    fn rows(&self) -> Vec<Check> {
        let mut rows = vec![Check::Session];
        if !self.apps.is_empty() {
            rows.push(Check::Apps);
        }
        if self.t3 && self.switch.is_none() {
            rows.push(Check::T3);
        }
        if self.switch.is_none() {
            rows.push(Check::DontAsk);
        }
        rows
    }

    fn checked(&self, check: Check) -> bool {
        match check {
            Check::Session => self.stops.session,
            Check::Apps => self.stops.apps,
            Check::T3 => self.stops.t3,
            Check::DontAsk => self.dont_ask,
        }
    }

    /// Check row `i`, or uncheck it.
    fn toggle(&mut self, i: usize) {
        match self.rows().get(i) {
            Some(Check::Session) => self.stops.session = !self.stops.session,
            Some(Check::Apps) => self.stops.apps = !self.stops.apps,
            Some(Check::T3) => self.stops.t3 = !self.stops.t3,
            Some(Check::DontAsk) => self.dont_ask = !self.dont_ask,
            None => {}
        }
    }

    /// What pressing button `button` asks for.
    fn press(&self, button: usize) -> Outcome {
        match button {
            GO => Outcome::Go {
                stops: self.stops,
                remember: self.dont_ask,
            },
            NEW_GERDR if self.switch.is_some() => Outcome::NewGerdr,
            _ => Outcome::Cancel,
        }
    }

    /// Handle a key: `sym` is the key's own symbol (`typed`, with Shift and
    /// the layout applied, isn't needed).
    pub(crate) fn key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let switching = self.switch.is_some();
        let last = self.rows().len() - 1;
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return Some(self.press(self.selected)),
            Keysym::Up | Keysym::KP_Up => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Down | Keysym::KP_Down => self.cursor = (self.cursor + 1).min(last),
            Keysym::space if plain => self.toggle(self.cursor),
            Keysym::Left | Keysym::KP_Left | Keysym::ISO_Left_Tab => self.step(-1),
            Keysym::Tab if mods.shift => self.step(-1),
            Keysym::Right | Keysym::KP_Right | Keysym::Tab => self.step(1),
            Keysym::q | Keysym::Q if plain && !switching => return Some(self.press(GO)),
            Keysym::n | Keysym::N if plain && switching => return Some(self.press(NEW_GERDR)),
            Keysym::s | Keysym::S if plain && switching => return Some(self.press(GO)),
            _ => {}
        }
        None
    }

    /// Move the selection `by` buttons, wrapping around.
    fn step(&mut self, by: isize) {
        let buttons = if self.switch.is_some() { 3 } else { 2 };
        self.selected = (self.selected as isize + by).rem_euclid(buttons) as usize;
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
    /// Quit, or switch, stopping `stops` of the project shown; `remember`
    /// if "Don't ask again" was checked.
    Go {
        stops: Stops,
        remember: bool,
    },
    /// Switching: keep the project shown and open the other in a new gerdr.
    NewGerdr,
    Cancel,
}

impl Gerdr {
    /// The window is closing: quit right away without a project, do what
    /// was remembered, or ask.
    pub fn request_quit(&mut self) {
        let Some(id) = self.project.as_deref() else {
            self.quit_app(Stops::default());
            return;
        };
        // Not connected to its session: ask only if it or its apps run all
        // the same.
        if self.session.herdr.is_none()
            && !self
                .projects
                .project(id)
                .is_some_and(|p| herdr::is_running(&p.session))
            && keep::app_names(id).is_empty()
            && !self.t3_runs()
        {
            self.quit_app(Stops::default());
            return;
        }
        match self.projects.quit_choice {
            Some(choice) => self.quit_app(choice.stops()),
            None => self.open_quit(),
        }
    }

    /// Ask, even if a choice was remembered.
    pub fn open_quit(&mut self) {
        self.overlays.project_menu = None;
        self.overlays.quit = Some(
            QuitDialog::quitting(
                self.stop_picks(),
                self.shown_apps(),
                self.projects.quit_choice.is_some(),
            )
            .with_t3(self.t3_runs()),
        );
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// What to check to stop: as picked last.
    fn stop_picks(&self) -> Stops {
        self.projects
            .stop_picks
            .or(self.projects.quit_choice.map(QuitChoice::stops))
            .unwrap_or_default()
    }

    /// The apps the shown project keeps running (see `keep::app_names`).
    fn shown_apps(&self) -> Vec<String> {
        self.project
            .as_deref()
            .map(keep::app_names)
            .unwrap_or_default()
    }

    /// Do what the quit dialog asked.
    pub fn apply_quit(&mut self, outcome: Outcome) {
        if let Outcome::Go { stops, .. } = outcome {
            self.projects.stop_picks = Some(stops);
        }
        if let Some(switch) = self.overlays.quit.as_mut().and_then(|d| d.switch.take()) {
            self.overlays.quit = None;
            self.refresh_pointer_focus();
            return match outcome {
                Outcome::Go { stops, .. } => {
                    self.projects.save();
                    self.switch_now(switch, stops)
                }
                Outcome::NewGerdr => self.open_in_new_gerdr(switch),
                Outcome::Cancel if switch.created => self.forget_project_now(&switch.id),
                Outcome::Cancel => {}
            };
        }
        let Outcome::Go { stops, remember } = outcome else {
            self.overlays.quit = None;
            // Gone, what is under the pointer gets it back.
            self.refresh_pointer_focus();
            return;
        };
        self.projects.quit_choice = remember.then_some(QuitChoice::Stops(stops));
        self.projects.save();
        self.overlays.quit = None;
        self.quit_app(stops);
    }

    /// Show project `switch.id` as `switch.then` says: right away when no
    /// other project is shown, else once asked whether to stop the one
    /// shown or keep it and open the other in a new gerdr.
    pub fn ask_switch(&mut self, switch: Switch) {
        let shown = self.project.as_deref();
        if shown.is_none_or(|id| id == switch.id) {
            return self.switch_now(switch, Stops::default());
        }
        // Not a question: it can't be shown here anyway.
        if self.open_elsewhere(&switch.id) {
            return self.refuse_open(&switch.id);
        }
        self.overlays.project_menu = None;
        self.overlays.quit = Some(QuitDialog::switching(
            self.stop_picks(),
            self.shown_apps(),
            switch,
        ));
        self.refresh_pointer_focus();
    }

    /// Show the project `switch` is about, stopping what `stops` says of
    /// the one left (see `show_project`).
    fn switch_now(&mut self, switch: Switch, stops: Stops) {
        self.life.stop_left = self
            .project
            .clone()
            .filter(|_| stops.any())
            .map(|id| (id, stops));
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
                self.procs.children.push(child);
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
        let Some(dialog) = self.overlays.quit.as_mut() else {
            return false;
        };
        let outcome = match quit_hit(&layout, pos) {
            Some(QuitHit::Button(button)) => Some(dialog.press(button)),
            Some(QuitHit::Row(row)) => {
                dialog.cursor = row;
                dialog.toggle(row);
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            // A project made for the switch isn't lost to a stray click:
            // only Cancel or Esc lets it go.
            None if dialog.switch.as_ref().is_some_and(|s| s.created) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_quit(outcome);
        }
        true
    }

    /// Where the quit dialog's parts go, as it asks now.
    pub fn quit_layout(&self) -> chrome::QuitLayout {
        let (rows, switching) = self
            .overlays
            .quit
            .as_ref()
            .map_or((0, false), |d| (d.rows().len(), d.switch.is_some()));
        chrome::quit_layout(self.output_size(), rows, switching)
    }

    /// The pointer moved while the quit dialog is open.
    pub fn quit_hover(&mut self, pos: Point<f64, Logical>) {
        let layout = self.quit_layout();
        if let Some(dialog) = self.overlays.quit.as_mut() {
            dialog.hover = quit_hit(&layout, pos);
        }
    }

    pub fn quit_view(&self) -> Option<QuitView> {
        let dialog = self.overlays.quit.as_ref()?;
        let name = |id: &str| {
            self.projects
                .project(id)
                .map_or_else(|| id.to_owned(), |p| p.name.clone())
        };
        // Where asking comes back, once it isn't asked on closing the window.
        let ask_again = self
            .key_state
            .action_keys(crate::Action::Quit)
            .map_or_else(String::new, |keys| format!("{keys} asks again"));
        let rows = dialog
            .rows()
            .into_iter()
            .map(|check| {
                let (label, detail) = match check {
                    Check::Session => (
                        "Terminals and agents".into(),
                        "the project's session".into(),
                    ),
                    Check::Apps => (dialog.apps.join(", "), "kept running".into()),
                    Check::T3 => ("T3 Code server".into(), "shared by every project".into()),
                    Check::DontAsk => ("Don't ask again".into(), ask_again.clone()),
                };
                QuitRow {
                    label,
                    detail,
                    checked: dialog.checked(check),
                }
            })
            .collect();
        Some(QuitView {
            project: self.project.as_deref().map(name).unwrap_or_default(),
            rows,
            cursor: dialog.cursor,
            selected: dialog.selected,
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
        .rows
        .iter()
        .position(|r| r.to_f64().contains(pos))
        .map(QuitHit::Row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(dialog: &mut QuitDialog, sym: Keysym) -> Option<Outcome> {
        dialog.key(&ModifiersState::default(), sym)
    }

    fn cursor() -> Vec<String> {
        vec!["Cursor".into()]
    }

    fn switch() -> Switch {
        Switch {
            id: "aft".into(),
            then: Then::Enter {
                pick: true,
                folders: Vec::new(),
            },
            created: false,
        }
    }

    #[test]
    fn quit_stops_what_was_picked() {
        let stops = Stops {
            session: true,
            apps: false,
            t3: false,
        };
        let mut dialog = QuitDialog::quitting(stops, cursor(), false);
        // It stops the session: Enter cancels, Q quits.
        assert_eq!(dialog.selected, CANCEL);
        assert_eq!(press(&mut dialog, Keysym::Return), Some(Outcome::Cancel));
        assert_eq!(
            press(&mut dialog, Keysym::q),
            Some(Outcome::Go {
                stops,
                remember: false
            })
        );
        // Stopping nothing, Enter quits.
        let mut dialog = QuitDialog::quitting(Stops::default(), cursor(), false);
        assert_eq!(dialog.selected, GO);
        assert_eq!(
            press(&mut dialog, Keysym::Return),
            Some(Outcome::Go {
                stops: Stops::default(),
                remember: false
            })
        );
    }

    #[test]
    fn rows_check_with_space() {
        let mut dialog = QuitDialog::quitting(Stops::default(), cursor(), false);
        assert_eq!(dialog.rows(), [Check::Session, Check::Apps, Check::DontAsk]);
        press(&mut dialog, Keysym::Down);
        assert_eq!(press(&mut dialog, Keysym::space), None);
        assert_eq!(
            dialog.stops,
            Stops {
                session: false,
                apps: true,
                t3: false,
            }
        );
        press(&mut dialog, Keysym::Down);
        press(&mut dialog, Keysym::Down);
        assert_eq!(dialog.cursor, 2);
        press(&mut dialog, Keysym::space);
        press(&mut dialog, Keysym::Up);
        press(&mut dialog, Keysym::Up);
        press(&mut dialog, Keysym::Up);
        press(&mut dialog, Keysym::space);
        assert_eq!(
            press(&mut dialog, Keysym::q),
            Some(Outcome::Go {
                stops: Stops::ALL,
                remember: true
            })
        );
    }

    #[test]
    fn a_t3_row_only_when_it_runs_and_quitting() {
        let quitting = QuitDialog::quitting(Stops::default(), Vec::new(), false);
        assert!(!quitting.rows().contains(&Check::T3));
        let mut dialog = quitting.with_t3(true);
        assert_eq!(
            dialog.rows(),
            vec![Check::Session, Check::T3, Check::DontAsk]
        );
        dialog.cursor = 1;
        dialog.toggle(1);
        let Outcome::Go { stops, .. } = dialog.press(GO) else {
            panic!("quits");
        };
        assert!(stops.t3 && !stops.session);
        let switching = QuitDialog::switching(Stops::default(), Vec::new(), switch()).with_t3(true);
        assert!(!switching.rows().contains(&Check::T3));
    }

    #[test]
    fn no_apps_row_without_apps() {
        let dialog = QuitDialog::quitting(Stops::ALL, Vec::new(), true);
        assert_eq!(dialog.rows(), [Check::Session, Check::DontAsk]);
        let dialog = QuitDialog::switching(Stops::ALL, cursor(), switch());
        assert_eq!(dialog.rows(), [Check::Session, Check::Apps]);
    }

    #[test]
    fn selection_cycles_and_wraps() {
        let mut dialog = QuitDialog::quitting(Stops::default(), Vec::new(), false);
        assert_eq!(press(&mut dialog, Keysym::Right), None);
        assert_eq!(dialog.selected, 0);
        press(&mut dialog, Keysym::Left);
        assert_eq!(dialog.selected, GO);
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        dialog.key(&shift, Keysym::Tab);
        assert_eq!(dialog.selected, 0);
        assert_eq!(press(&mut dialog, Keysym::Return), Some(Outcome::Cancel));
    }

    #[test]
    fn switching_keys() {
        let stops = Stops {
            session: false,
            apps: true,
            t3: false,
        };
        let mut dialog = QuitDialog::switching(stops, cursor(), switch());
        assert_eq!(press(&mut dialog, Keysym::Return), Some(Outcome::Cancel));
        let mut calm = QuitDialog::switching(Stops::default(), cursor(), switch());
        assert_eq!(
            press(&mut calm, Keysym::Return),
            Some(Outcome::Go {
                stops: Stops::default(),
                remember: false
            })
        );
        assert_eq!(press(&mut dialog, Keysym::q), None);
        assert_eq!(press(&mut dialog, Keysym::n), Some(Outcome::NewGerdr));
        assert_eq!(
            press(&mut dialog, Keysym::s),
            Some(Outcome::Go {
                stops,
                remember: false
            })
        );
        press(&mut dialog, Keysym::Tab);
        assert_eq!(dialog.selected, GO);
    }

    #[test]
    fn escape_and_other_keys() {
        let mut dialog = QuitDialog::quitting(Stops::default(), Vec::new(), true);
        assert_eq!(press(&mut dialog, Keysym::Escape), Some(Outcome::Cancel));
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(dialog.key(&ctrl, Keysym::q), None);
        assert_eq!(press(&mut dialog, Keysym::s), None);
        assert_eq!(press(&mut dialog, Keysym::x), None);
    }
}
