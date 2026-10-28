//! Restarting a workspace's commands, from the button on its sidebar
//! header: which of them run again.
//!
//! Every command is the user's to choose once: the first restart that
//! finds a command not chosen for or against yet opens the restart dialog,
//! a row per command, the new ones checked and the others as chosen
//! before. The choices are kept in the workspace's record; while no new
//! command comes, restarting runs the chosen ones without asking. The
//! workspace's menu opens the dialog anyway, to change them.
//!
//! Up/Down move between the rows, Space checks the one under the cursor,
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one (Restart, by default), Esc cancels.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, BarStatus, RestartHit, RestartRow, RestartView},
    home_dir, paths,
    projects::{Captured, RestartChoice},
};

/// The Restart button, after Cancel.
const RESTART: usize = 1;

/// What restarting a workspace does with its commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Run these again (by index) without asking: every command was
    /// chosen for or against before.
    Run(Vec<usize>),
    /// Ask, each checked or not as given.
    Ask(Vec<bool>),
}

/// What restarting `commands` does, given the user's `choices`: run the
/// chosen ones while all of them were chosen for or against before, else
/// ask, each checked as chosen before and a new one checked.
pub fn plan(commands: &[&str], choices: &[RestartChoice]) -> Plan {
    let chosen: Vec<Option<bool>> = commands
        .iter()
        .map(|command| {
            choices
                .iter()
                .find(|c| c.command == *command)
                .map(|c| c.restart)
        })
        .collect();
    if chosen.iter().all(Option::is_some) {
        Plan::Run(
            chosen
                .iter()
                .enumerate()
                .filter(|(_, c)| **c == Some(true))
                .map(|(i, _)| i)
                .collect(),
        )
    } else {
        Plan::Ask(chosen.iter().map(|c| c.unwrap_or(true)).collect())
    }
}

/// Keep the user's `picks` in `choices`: a command chosen before takes
/// the new choice, a new one is added. The others are kept: a command
/// that doesn't run now may come back.
pub fn choose(choices: &mut Vec<RestartChoice>, picks: &[(&str, bool)]) {
    for (command, restart) in picks {
        match choices.iter_mut().find(|c| c.command == *command) {
            Some(choice) => choice.restart = *restart,
            None => choices.push(RestartChoice {
                command: (*command).to_owned(),
                restart: *restart,
            }),
        }
    }
}

/// A command the restart dialog offers.
#[derive(Clone, Debug)]
pub struct Row {
    pub pane_id: String,
    pub captured: Captured,
    /// It has exited: it runs again rather than restarting.
    pub exited: bool,
    pub checked: bool,
}

/// The restart dialog's state, while it is open.
#[derive(Debug)]
pub struct RestartDialog {
    /// The workspace whose commands it restarts.
    pub workspace: usize,
    pub rows: Vec<Row>,
    /// The row under the keyboard's cursor.
    pub cursor: usize,
    /// The selected button: 0 Cancel, 1 Restart.
    pub selected: usize,
    /// What the pointer is over.
    pub hover: Option<RestartHit>,
}

/// What the restart dialog asks gerdr to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Keep the choices and restart the checked commands.
    Restart,
    Cancel,
}

impl RestartDialog {
    fn press(&self, button: usize) -> Outcome {
        if button == RESTART {
            Outcome::Restart
        } else {
            Outcome::Cancel
        }
    }

    fn toggle(&mut self, row: usize) {
        if let Some(row) = self.rows.get_mut(row) {
            row.checked = !row.checked;
        }
    }

    /// Handle a key: `sym` is the key's own symbol.
    fn key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let last = self.rows.len().saturating_sub(1);
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return Some(self.press(self.selected)),
            Keysym::Up | Keysym::KP_Up => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Down | Keysym::KP_Down => self.cursor = (self.cursor + 1).min(last),
            Keysym::space if plain => self.toggle(self.cursor),
            Keysym::Left
            | Keysym::KP_Left
            | Keysym::ISO_Left_Tab
            | Keysym::Right
            | Keysym::KP_Right
            | Keysym::Tab => self.selected = 1 - self.selected,
            _ => {}
        }
        None
    }
}

impl Gerdr {
    /// Restart workspace `w`'s commands (see `workspace_restarts`): the
    /// ones chosen before, or ask which once a new one comes (see `plan`).
    pub fn restart_workspace(&mut self, w: usize) {
        let restarts = self.workspace_restarts(w);
        if restarts.is_empty() {
            return self.show_notice("Nothing to restart in this workspace".into());
        }
        let commands: Vec<&str> = restarts.iter().map(|(_, c)| c.command.as_str()).collect();
        match plan(&commands, self.restart_choices(w)) {
            Plan::Run(chosen) if chosen.is_empty() => {
                self.show_notice("No command chosen to restart".into());
            }
            Plan::Run(chosen) => {
                let picked = chosen.into_iter().map(|i| restarts[i].clone()).collect();
                self.restart_commands(picked);
            }
            Plan::Ask(checks) => self.open_restart(w, restarts, checks),
        }
    }

    /// Ask which of workspace `w`'s commands restarting it runs again,
    /// as chosen before.
    pub fn choose_restarts(&mut self, w: usize) {
        let restarts = self.workspace_restarts(w);
        if restarts.is_empty() {
            return self.show_notice("Nothing to restart in this workspace".into());
        }
        let commands: Vec<&str> = restarts.iter().map(|(_, c)| c.command.as_str()).collect();
        let checks = match plan(&commands, self.restart_choices(w)) {
            Plan::Ask(checks) => checks,
            Plan::Run(chosen) => (0..restarts.len()).map(|i| chosen.contains(&i)).collect(),
        };
        self.open_restart(w, restarts, checks);
    }

    /// The choices kept for workspace `w`.
    pub(crate) fn restart_choices(&self, w: usize) -> &[RestartChoice] {
        self.workspaces
            .get(w)
            .and_then(|ws| self.record(&ws.record))
            .map_or(&[], |r| r.restart_choices.as_slice())
    }

    fn open_restart(&mut self, w: usize, restarts: Vec<(String, Captured)>, checks: Vec<bool>) {
        let rows = restarts
            .into_iter()
            .zip(checks)
            .map(|((pane_id, captured), checked)| Row {
                exited: self.pane_bar_content(&pane_id).status == BarStatus::Exited,
                pane_id,
                captured,
                checked,
            })
            .collect();
        self.context_menu = None;
        self.restart = Some(RestartDialog {
            workspace: w,
            rows,
            cursor: 0,
            selected: RESTART,
            hover: None,
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// A key while the restart dialog has the keyboard.
    pub fn restart_key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        self.restart.as_mut()?.key(mods, sym)
    }

    /// Do what the restart dialog asked: keep the choices in the
    /// workspace's record and restart the checked commands.
    pub fn apply_restart(&mut self, outcome: Outcome) {
        let Some(dialog) = self.restart.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        if outcome == Outcome::Cancel {
            return;
        }
        let picks: Vec<(&str, bool)> = dialog
            .rows
            .iter()
            .map(|r| (r.captured.command.as_str(), r.checked))
            .collect();
        let record = self
            .workspaces
            .get(dialog.workspace)
            .map(|ws| ws.record.clone());
        if let (Some(project), Some(record)) = (self.project.clone(), record)
            && let Some(record) = self.projects.record_mut(&project, &record)
        {
            choose(&mut record.restart_choices, &picks);
            self.projects.save();
        }
        let picked: Vec<_> = dialog
            .rows
            .into_iter()
            .filter(|r| r.checked)
            .map(|r| (r.pane_id, r.captured))
            .collect();
        if picked.is_empty() {
            self.show_notice("Nothing restarted; restart asks again for new commands".into());
        } else {
            self.restart_commands(picked);
        }
    }

    /// A left click while the restart dialog is open; `false` if it isn't.
    pub fn restart_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let size = self.output_size();
        let Some(dialog) = self.restart.as_mut() else {
            return false;
        };
        let layout = chrome::restart_layout(size, dialog.rows.len());
        let outcome = match restart_hit(&layout, pos) {
            Some(RestartHit::Button(button)) => Some(dialog.press(button)),
            Some(RestartHit::Row(row)) => {
                dialog.cursor = row;
                dialog.toggle(row);
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_restart(outcome);
        }
        true
    }

    /// The pointer moved while the restart dialog is open.
    pub fn restart_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.restart.as_mut() {
            let layout = chrome::restart_layout(size, dialog.rows.len());
            dialog.hover = restart_hit(&layout, pos);
        }
    }

    /// The controls of the restart dialog, while it is open: what the
    /// pointer can press.
    pub fn restart_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.restart else {
            return Vec::new();
        };
        let layout = chrome::restart_layout(self.output_size(), dialog.rows.len());
        layout.rows.into_iter().chain(layout.buttons).collect()
    }

    pub fn restart_view(&self) -> Option<RestartView> {
        let dialog = self.restart.as_ref()?;
        let workspace = self
            .workspaces
            .get(dialog.workspace)
            .map(|ws| ws.name.clone())
            .unwrap_or_default();
        let home = home_dir();
        let rows = dialog
            .rows
            .iter()
            .map(|row| {
                let c = &row.captured;
                RestartRow {
                    label: chrome::command_line(&c.command).unwrap_or_else(|| c.command.clone()),
                    place: c.cwd.as_deref().map_or(String::new(), |dir| {
                        paths::display(dir, home.as_deref(), 40)
                    }),
                    exited: row.exited,
                    checked: row.checked,
                }
            })
            .collect();
        Some(RestartView {
            workspace,
            rows,
            cursor: dialog.cursor,
            selected: dialog.selected,
            hover: dialog.hover,
        })
    }
}

/// What `pos` (output coordinates) is over in the restart dialog.
fn restart_hit(layout: &chrome::RestartLayout, pos: Point<f64, Logical>) -> Option<RestartHit> {
    if let Some(button) = layout.buttons.iter().position(|b| b.to_f64().contains(pos)) {
        return Some(RestartHit::Button(button));
    }
    layout
        .rows
        .iter()
        .position(|r| r.to_f64().contains(pos))
        .map(RestartHit::Row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chose(command: &str, restart: bool) -> RestartChoice {
        RestartChoice {
            command: command.into(),
            restart,
        }
    }

    #[test]
    fn a_new_command_asks_with_it_checked_and_the_rest_as_chosen() {
        let choices = [chose("celery worker", true), chose("pytest", false)];
        assert_eq!(
            plan(&["celery worker", "pytest", "npm run dev"], &choices),
            Plan::Ask(vec![true, false, true])
        );
        assert_eq!(plan(&["npm run dev"], &[]), Plan::Ask(vec![true]));
    }

    #[test]
    fn known_commands_run_as_chosen_without_asking() {
        let choices = [
            chose("celery worker", true),
            chose("pytest", false),
            chose("gone", true),
        ];
        assert_eq!(
            plan(&["pytest", "celery worker"], &choices),
            Plan::Run(vec![1])
        );
        assert_eq!(plan(&["pytest"], &choices), Plan::Run(vec![]));
    }

    #[test]
    fn choosing_updates_and_adds_and_keeps_the_rest() {
        let mut choices = vec![chose("celery worker", true), chose("gone", false)];
        choose(&mut choices, &[("celery worker", false), ("pytest", true)]);
        assert_eq!(
            choices,
            [
                chose("celery worker", false),
                chose("gone", false),
                chose("pytest", true)
            ]
        );
    }

    fn dialog(n: usize) -> RestartDialog {
        let row = |i: usize| Row {
            pane_id: format!("w1:p{i}"),
            captured: Captured {
                command: format!("cmd {i}"),
                cwd: None,
                env: Vec::new(),
            },
            exited: false,
            checked: true,
        };
        RestartDialog {
            workspace: 0,
            rows: (0..n).map(row).collect(),
            cursor: 0,
            selected: RESTART,
            hover: None,
        }
    }

    fn press(dialog: &mut RestartDialog, sym: Keysym) -> Option<Outcome> {
        dialog.key(&ModifiersState::default(), sym)
    }

    #[test]
    fn keys_move_check_and_press() {
        let mut d = dialog(3);
        press(&mut d, Keysym::Down);
        press(&mut d, Keysym::Down);
        press(&mut d, Keysym::Down);
        assert_eq!(d.cursor, 2);
        assert_eq!(press(&mut d, Keysym::space), None);
        assert!(!d.rows[2].checked);
        press(&mut d, Keysym::Up);
        assert_eq!(d.cursor, 1);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Restart));
        press(&mut d, Keysym::Tab);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Cancel));
        assert_eq!(press(&mut d, Keysym::Escape), Some(Outcome::Cancel));
    }
}
