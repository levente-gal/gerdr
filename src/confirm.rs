//! A dialog asking to confirm something that can't be undone: closing a
//! pane, a tab, a workspace or an agent where something runs, stopping a
//! project's session, forgetting a project whose session runs. A title, a
//! line saying what it stops, Cancel and the button doing it, in the
//! error colour.
//!
//! Every way to close or stop goes through here (see `Gerdr::close_pane`,
//! `close_tab`, `close_workspace`, `close_agent`, `stop_project`,
//! `forget_project`): it asks only when something runs or would be lost,
//! and does it at once otherwise.
//!
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one (Cancel, by default: the action is never a slip of Enter), Esc
//! cancels.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    agents::AgentKey,
    chrome::{self, AgentView, BarStatus, ConfirmView},
    herdr::{self, AgentStatus},
    keep,
    projects::Stops,
};

/// Cancel, the first and the default.
const CANCEL: usize = 0;
/// The button doing it, after Cancel.
const GO: usize = 1;

/// What confirming does.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Then {
    /// Close the agent of this card (see `Gerdr::close_agent`).
    CloseAgent(AgentKey),
    /// Close this pane of the active tab (see `Gerdr::close_pane`).
    ClosePane(u32),
    /// Close the tab of the active workspace holding these panes (see
    /// `Gerdr::close_tab`), wherever it is by then.
    CloseTab(Vec<u32>),
    /// Close the workspace holding these panes (see
    /// `Gerdr::close_workspace`), wherever it is by then.
    CloseWorkspace(Vec<u32>),
    /// Stop what `Stops` says of this project (see `Gerdr::stop_project`).
    StopProject(String, Stops),
    /// Stop this project's session and apps, then forget it (see
    /// `Gerdr::forget_project`).
    ForgetProject(String),
}

/// The confirm dialog's state, while it is open.
#[derive(Debug)]
pub struct ConfirmDialog {
    pub title: String,
    /// What confirming does, in a line.
    pub body: String,
    /// The label of the button doing it.
    pub go: &'static str,
    pub then: Then,
    /// The selected button: 0 Cancel, 1 the one doing it.
    pub selected: usize,
    /// The button under the pointer.
    pub hover: Option<usize>,
}

/// What the confirm dialog asks gerdr to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Go,
    Cancel,
}

impl ConfirmDialog {
    fn press(&self, button: usize) -> Outcome {
        if button == GO {
            Outcome::Go
        } else {
            Outcome::Cancel
        }
    }

    /// Handle a key: `sym` is the key's own symbol.
    fn key(&mut self, sym: Keysym) -> Option<Outcome> {
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return Some(self.press(self.selected)),
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

/// What a confirm dialog about closing `card`, whose pane runs, says: its
/// title and the line under it.
fn close_question(card: &AgentView) -> (String, String) {
    let title = format!("Close \u{201c}{}\u{201d}?", card.heading());
    let body = match (&card.key, card.status) {
        (AgentKey::Test(_), _) => "It is still running: closing its pane stops it",
        (_, AgentStatus::Blocked) => {
            "It waits for you in the middle of a run: closing its pane stops it"
        }
        (_, AgentStatus::Working) => "It is working: closing its pane stops it",
        // Done or idle, it still runs, with all it was told.
        _ => "Its session is still open: closing its pane ends it",
    };
    (title, body.to_owned())
}

/// What a confirm dialog about closing a pane where `what` runs says: its
/// title and the line under it.
fn close_pane_question(what: &str) -> (String, String) {
    let title = format!("Close \u{201c}{what}\u{201d}?");
    (
        title,
        "It is still running: closing its pane stops it".to_owned(),
    )
}

/// What a confirm dialog about closing the `what` (`"tab"`,
/// `"workspace"`) called `name`, where `running` runs, says: its title
/// and the line under it.
fn close_many_question(what: &str, name: &str, running: &[String]) -> (String, String) {
    let title = format!("Close the {what} \u{201c}{name}\u{201d}?");
    let body = match running {
        [one] => format!("\u{201c}{one}\u{201d} is still running: closing the {what} stops it"),
        _ => format!(
            "{} commands are still running: closing the {what} stops them",
            running.len()
        ),
    };
    (title, body)
}

/// What a confirm dialog about stopping project `name` says: its title and
/// the line under it, from what runs of it: its `session`, and the `apps`
/// it keeps running.
fn stop_question(name: &str, session: bool, apps: &[String]) -> (String, String) {
    let title = format!("Stop \u{201c}{name}\u{201d}?");
    let body = match (session, apps) {
        (true, []) => "Its terminals and agents stop".to_owned(),
        (true, apps) => format!(
            "Its terminals and agents stop, and {} closes",
            apps.join(", ")
        ),
        (false, apps) => format!("{} closes", apps.join(", ")),
    };
    (title, body)
}

/// What a confirm dialog about forgetting project `name`, which still
/// runs, says: its title and the line under it.
fn forget_question(name: &str) -> (String, String) {
    (
        format!("Forget \u{201c}{name}\u{201d}?"),
        "It still runs: forgetting it stops its terminals, agents and apps".to_owned(),
    )
}

impl Gerdr {
    /// The card of agent `key`, if one shows.
    fn agent_card(&self, key: &AgentKey) -> Option<AgentView> {
        self.agent_views()
            .into_iter()
            .flatten()
            .find(|card| card.key == *key)
    }

    /// Close the agent of card `key`, as its "×" does (see
    /// `AgentView::close_label`): one whose pane still runs asks first. A
    /// Cursor chat never does: it runs on in Cursor and comes back.
    pub fn close_agent(&mut self, key: &AgentKey) {
        let Some(card) = self.agent_card(key).filter(|card| card.closable) else {
            return;
        };
        let runs = match &card.key {
            // Done or idle, the agent still runs in its pane.
            AgentKey::Herdr(pane_id) => card.running() || self.herdr_pane_runs(pane_id),
            AgentKey::Test(_) => card.running(),
            AgentKey::Cursor(_) | AgentKey::Ci(_) => false,
        };
        if !runs {
            return self.close_agent_now(&card);
        }
        let (title, body) = close_question(&card);
        self.ask(
            title,
            body,
            card.close_label(),
            Then::CloseAgent(key.clone()),
        );
    }

    /// Ask before closing pane `id` of the active tab, where `what` runs.
    pub fn ask_close_pane(&mut self, id: u32, what: String) {
        let (title, body) = close_pane_question(&what);
        self.ask(title, body, "Close pane", Then::ClosePane(id));
    }

    /// Ask before closing tab `idx` of the active workspace, holding
    /// `panes`, where `running` runs.
    pub fn ask_close_tab(&mut self, idx: usize, panes: Vec<u32>, running: &[String]) {
        let Some(ws) = self.workspace() else {
            return;
        };
        let Some(view) = self.tab_views(ws).into_iter().nth(idx) else {
            return;
        };
        let (title, body) = close_many_question("tab", &view.title, running);
        self.ask(title, body, "Close tab", Then::CloseTab(panes));
    }

    /// Ask before closing workspace `idx`, holding `panes`, where `running`
    /// runs.
    pub fn ask_close_workspace(&mut self, idx: usize, panes: Vec<u32>, running: &[String]) {
        let Some(name) = self.workspaces.get(idx).map(|ws| ws.name.clone()) else {
            return;
        };
        let (title, body) = close_many_question("workspace", &name, running);
        self.ask(title, body, "Close workspace", Then::CloseWorkspace(panes));
    }

    /// Stop what `stops` says of project `id` (see `stop_project_now`):
    /// asked first when any of it runs; nothing to stop says so.
    pub fn stop_project(&mut self, id: &str, stops: Stops) {
        let Some(project) = self.projects.project(id) else {
            return tracing::warn!("no project {id} to stop");
        };
        let name = project.name.clone();
        let session = stops.session && self.session_runs(id);
        let apps = if stops.apps {
            keep::app_names(id)
        } else {
            Vec::new()
        };
        if !session && apps.is_empty() {
            return self.show_notice(format!("{name} isn't running"));
        }
        let (title, body) = stop_question(&name, session, &apps);
        self.ask(
            title,
            body,
            "Stop project",
            Then::StopProject(id.to_owned(), stops),
        );
    }

    /// Forget project `id` (see `forget_project_now`); one that still runs
    /// asks first, and stops before it is forgotten: a session left behind
    /// could never be reached again. Not the shown one.
    pub fn forget_project(&mut self, id: &str) {
        if self.project.as_deref() == Some(id) {
            return tracing::warn!("not forgetting project {id}: it is the one shown");
        }
        let Some(name) = self.projects.project(id).map(|p| p.name.clone()) else {
            return;
        };
        if !self.session_runs(id) && keep::app_names(id).is_empty() {
            return self.forget_project_now(id);
        }
        let (title, body) = forget_question(&name);
        self.ask(
            title,
            body,
            "Stop and forget",
            Then::ForgetProject(id.to_owned()),
        );
    }

    /// Whether project `id`'s herdr session runs, as last found out, else
    /// asked now.
    fn session_runs(&self, id: &str) -> bool {
        if self.stopping.contains(id) {
            return false;
        }
        match self.running.get(id) {
            Some(running) => *running,
            None => self
                .projects
                .project(id)
                .is_some_and(|p| herdr::is_running(&p.session)),
        }
    }

    /// Whether herdr pane `pane_id` runs a command (an agent, say), as its
    /// bar says.
    fn herdr_pane_runs(&self, pane_id: &str) -> bool {
        matches!(
            self.pane_bar_content(pane_id).status,
            BarStatus::Running | BarStatus::Working
        )
    }

    /// Open the confirm dialog: `title`, `body` under it, and the `go`
    /// button doing `then`. Cancel is selected.
    fn ask(&mut self, title: String, body: String, go: &'static str, then: Then) {
        self.context_menu = None;
        self.confirm = Some(ConfirmDialog {
            title,
            body,
            go,
            then,
            selected: CANCEL,
            hover: None,
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the agent of `card` without asking: a terminal agent, or a
    /// test still running, goes with its pane (and its tab, if alone
    /// there); a finished test run's card goes; a Cursor chat is deleted
    /// from gerdr.
    fn close_agent_now(&mut self, card: &AgentView) {
        match &card.key {
            AgentKey::Herdr(pane_id) => self.close_herdr_pane(pane_id),
            AgentKey::Test(pane_id) => {
                if card.running() {
                    self.close_herdr_pane(pane_id);
                }
                self.dismiss_test(pane_id);
            }
            AgentKey::Cursor(chat) => self.delete_cursor_chat(chat),
            AgentKey::Ci(_) => {}
        }
    }

    /// A key while the confirm dialog has the keyboard.
    pub fn confirm_key(&mut self, _mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        self.confirm.as_mut()?.key(sym)
    }

    /// Do what the confirm dialog asked.
    pub fn apply_confirm(&mut self, outcome: Outcome) {
        let Some(dialog) = self.confirm.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        if outcome == Outcome::Cancel {
            return;
        }
        match dialog.then {
            // As it is now: it may have stopped running since.
            Then::CloseAgent(key) => {
                if let Some(card) = self.agent_card(&key) {
                    self.close_agent_now(&card);
                }
            }
            Then::ClosePane(id) => self.close_pane_now(id),
            Then::CloseTab(panes) => {
                let tab = self.workspace().and_then(|ws| {
                    ws.tabs
                        .iter()
                        .position(|tab| tab.panes.iter().any(|p| panes.contains(&p.id)))
                });
                if let Some(idx) = tab {
                    self.close_tab_now(idx);
                }
            }
            Then::CloseWorkspace(panes) => {
                let found = self.workspaces.iter().position(|ws| {
                    ws.tabs
                        .iter()
                        .flat_map(|tab| &tab.panes)
                        .any(|p| panes.contains(&p.id))
                });
                if let Some(idx) = found {
                    self.close_workspace_now(idx);
                }
            }
            Then::StopProject(id, stops) => {
                self.stop_project_now(&id, stops);
                self.refresh_project_menu();
            }
            Then::ForgetProject(id) => {
                self.forget_after_stop.insert(id.clone());
                self.stop_project_now(&id, Stops::ALL);
                self.refresh_project_menu();
            }
        }
    }

    /// A left click while the confirm dialog is open; `false` if it isn't.
    pub fn confirm_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let size = self.output_size();
        let Some(dialog) = self.confirm.as_mut() else {
            return false;
        };
        let layout = chrome::confirm_layout(size);
        let outcome = match button_at(&layout, pos) {
            Some(button) => Some(dialog.press(button)),
            None if layout.panel.to_f64().contains(pos) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_confirm(outcome);
        }
        true
    }

    /// The pointer moved while the confirm dialog is open.
    pub fn confirm_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.confirm.as_mut() {
            dialog.hover = button_at(&chrome::confirm_layout(size), pos);
        }
    }

    /// The buttons of the confirm dialog, while it is open: what the
    /// pointer can press.
    pub fn confirm_controls(&self) -> Vec<chrome::Rect> {
        match &self.confirm {
            Some(_) => chrome::confirm_layout(self.output_size()).buttons.to_vec(),
            None => Vec::new(),
        }
    }

    pub fn confirm_view(&self) -> Option<ConfirmView> {
        let dialog = self.confirm.as_ref()?;
        Some(ConfirmView {
            title: dialog.title.clone(),
            body: dialog.body.clone(),
            go: dialog.go,
            selected: dialog.selected,
            hover: dialog.hover,
        })
    }
}

/// The button at `pos`, if any.
fn button_at(layout: &chrome::ConfirmLayout, pos: Point<f64, Logical>) -> Option<usize> {
    layout
        .buttons
        .iter()
        .position(|rect| rect.to_f64().contains(pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> ConfirmDialog {
        ConfirmDialog {
            title: String::new(),
            body: String::new(),
            go: "Close pane",
            then: Then::CloseAgent(AgentKey::Herdr("p1".into())),
            selected: CANCEL,
            hover: None,
        }
    }

    #[test]
    fn enter_cancels_until_the_action_is_picked() {
        let mut d = dialog();
        assert_eq!(d.key(Keysym::Return), Some(Outcome::Cancel));
        assert_eq!(d.key(Keysym::Tab), None);
        assert_eq!(d.key(Keysym::Return), Some(Outcome::Go));
        assert_eq!(d.key(Keysym::Escape), Some(Outcome::Cancel));
    }

    #[test]
    fn the_question_says_what_closing_stops() {
        let mut card = AgentView {
            key: AgentKey::Herdr("p1".into()),
            dir: None,
            status: AgentStatus::Working,
            kind: "claude".into(),
            name: None,
            model: None,
            lines: Vec::new(),
            closable: true,
            subagents: Vec::new(),
            inbox: false,
            test: None,
            icon: crate::agent_icons::AgentIcon::Claude,
        };
        let (title, body) = close_question(&card);
        assert_eq!(title, "Close \u{201c}claude\u{201d}?");
        assert!(body.contains("its pane stops it"));
        card.status = AgentStatus::Done;
        assert!(close_question(&card).1.contains("closing its pane ends it"));
        let (title, body) = close_pane_question("npm run dev");
        assert_eq!(title, "Close \u{201c}npm run dev\u{201d}?");
        assert!(body.contains("its pane stops it"));
    }

    #[test]
    fn the_tab_question_names_one_command_and_counts_more() {
        let one = ["npm run dev".to_owned()];
        let (title, body) = close_many_question("tab", "web", &one);
        assert_eq!(title, "Close the tab \u{201c}web\u{201d}?");
        assert_eq!(
            body,
            "\u{201c}npm run dev\u{201d} is still running: closing the tab stops it"
        );
        let two = ["npm run dev".to_owned(), "claude".to_owned()];
        let (title, body) = close_many_question("workspace", "api", &two);
        assert_eq!(title, "Close the workspace \u{201c}api\u{201d}?");
        assert_eq!(
            body,
            "2 commands are still running: closing the workspace stops them"
        );
    }

    #[test]
    fn the_stop_and_forget_questions_say_what_stops() {
        let (title, body) = stop_question("aft", true, &[]);
        assert_eq!(title, "Stop \u{201c}aft\u{201d}?");
        assert_eq!(body, "Its terminals and agents stop");
        let cursor = ["Cursor".to_owned()];
        assert_eq!(
            stop_question("aft", true, &cursor).1,
            "Its terminals and agents stop, and Cursor closes"
        );
        assert_eq!(stop_question("aft", false, &cursor).1, "Cursor closes");
        assert_eq!(forget_question("aft").0, "Forget \u{201c}aft\u{201d}?");
    }
}
