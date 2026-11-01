//! A dialog asking to confirm something that can't be undone, such as
//! closing an agent, a pane or a tab where something runs: a title, a line saying what it does, Cancel
//! and the button doing it.
//!
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one (the one doing it, by default), Esc cancels.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    agents::AgentKey,
    chrome::{self, AgentView, ConfirmView},
};

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

/// What a confirm dialog about closing `card`, which runs, says: its
/// title and the line under it.
fn close_question(card: &AgentView) -> (String, String) {
    let title = format!("Close \u{201c}{}\u{201d}?", card.heading());
    let body = match &card.key {
        AgentKey::Cursor(_) => {
            "It keeps running in Cursor; gerdr shows it again once you write to it"
        }
        AgentKey::T3(_) => "It keeps running in T3 Code; gerdr shows it again once you write to it",
        AgentKey::Test(_) => "It is still running: closing its pane stops it",
        _ if card.status == crate::herdr::AgentStatus::Blocked => {
            "It waits for you in the middle of a run: closing its pane stops it"
        }
        _ => "It is working: closing its pane stops it",
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

/// What a confirm dialog about closing the tab `tab`, where `running`
/// runs, says: its title and the line under it.
fn close_tab_question(tab: &str, running: &[String]) -> (String, String) {
    let title = format!("Close the tab \u{201c}{tab}\u{201d}?");
    let body = match running {
        [what] => format!("\u{201c}{what}\u{201d} is still running: closing the tab stops it"),
        _ => format!(
            "{} commands are still running: closing the tab stops them",
            running.len()
        ),
    };
    (title, body)
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
    /// `AgentView::close_label`): one that runs asks first.
    pub fn close_agent(&mut self, key: &AgentKey) {
        let Some(card) = self.agent_card(key).filter(|card| card.closable) else {
            return;
        };
        if !card.running() {
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
        let (title, body) = close_tab_question(&view.title, running);
        self.ask(title, body, "Close tab", Then::CloseTab(panes));
    }

    /// Open the confirm dialog: `title`, `body` under it, and the `go`
    /// button doing `then`.
    fn ask(&mut self, title: String, body: String, go: &'static str, then: Then) {
        self.context_menu = None;
        self.confirm = Some(ConfirmDialog {
            title,
            body,
            go,
            then,
            selected: GO,
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
            AgentKey::T3(thread) => self.delete_t3_thread(thread),
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
    use crate::herdr::AgentStatus;

    fn dialog() -> ConfirmDialog {
        ConfirmDialog {
            title: String::new(),
            body: String::new(),
            go: "Close pane",
            then: Then::CloseAgent(AgentKey::Herdr("p1".into())),
            selected: GO,
            hover: None,
        }
    }

    #[test]
    fn enter_goes_and_escape_cancels() {
        let mut d = dialog();
        assert_eq!(d.key(Keysym::Return), Some(Outcome::Go));
        assert_eq!(d.key(Keysym::Tab), None);
        assert_eq!(d.key(Keysym::Return), Some(Outcome::Cancel));
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
        card.key = AgentKey::Cursor("c1".into());
        assert!(close_question(&card).1.contains("keeps running in Cursor"));
        let (title, body) = close_pane_question("npm run dev");
        assert_eq!(title, "Close \u{201c}npm run dev\u{201d}?");
        assert!(body.contains("its pane stops it"));
    }

    #[test]
    fn the_tab_question_names_one_command_and_counts_more() {
        let one = ["npm run dev".to_owned()];
        let (title, body) = close_tab_question("web", &one);
        assert_eq!(title, "Close the tab \u{201c}web\u{201d}?");
        assert_eq!(
            body,
            "\u{201c}npm run dev\u{201d} is still running: closing the tab stops it"
        );
        let two = ["npm run dev".to_owned(), "claude".to_owned()];
        assert!(close_tab_question("web", &two).1.starts_with("2 commands"));
    }
}
