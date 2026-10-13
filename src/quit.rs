//! Closing gerdr with a project open: keep its session running in the
//! background (the default), or stop it. "Don't ask again" remembers the
//! choice; the quit action (prefix, Shift+Q) asks again regardless.
//!
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one, K and S press Keep running and Stop project, Space checks "Don't ask
//! again", Esc cancels.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, QuitHit, QuitView},
    herdr,
    projects::QuitChoice,
};

const STOP: usize = 1;
const KEEP: usize = 2;

/// The quit dialog's state, while it is open.
#[derive(Debug)]
pub struct QuitDialog {
    /// 0 Cancel, 1 Stop project, 2 Keep running (as `QuitView::selected`).
    pub selected: usize,
    pub dont_ask: bool,
    /// What the pointer is over.
    pub hover: Option<QuitHit>,
}

impl Default for QuitDialog {
    /// Keep running selected, as the safe choice.
    fn default() -> Self {
        Self {
            selected: KEEP,
            dont_ask: false,
            hover: None,
        }
    }
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
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return Some(self.press(self.selected)),
            Keysym::Left | Keysym::KP_Left | Keysym::ISO_Left_Tab => self.step(-1),
            Keysym::Tab if mods.shift => self.step(-1),
            Keysym::Right | Keysym::KP_Right | Keysym::Tab => self.step(1),
            Keysym::space if plain => self.dont_ask = !self.dont_ask,
            Keysym::k | Keysym::K if plain => return Some(self.press(KEEP)),
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

    /// A left click while the quit dialog is open; `false` if it isn't.
    pub fn quit_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let layout = chrome::quit_layout(self.output_size());
        let Some(dialog) = self.quit.as_mut() else {
            return false;
        };
        let outcome = match quit_hit(&layout, pos) {
            Some(QuitHit::Button(button)) => Some(dialog.press(button)),
            Some(QuitHit::Checkbox) => {
                dialog.dont_ask = !dialog.dont_ask;
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_quit(outcome);
        }
        true
    }

    /// The pointer moved while the quit dialog is open.
    pub fn quit_hover(&mut self, pos: Point<f64, Logical>) {
        let layout = chrome::quit_layout(self.output_size());
        if let Some(dialog) = self.quit.as_mut() {
            dialog.hover = quit_hit(&layout, pos);
        }
    }

    pub fn quit_view(&self) -> Option<QuitView> {
        let dialog = self.quit.as_ref()?;
        let project = self
            .project
            .as_deref()
            .map(|id| {
                self.projects
                    .project(id)
                    .map_or_else(|| id.to_owned(), |p| p.name.clone())
            })
            .unwrap_or_default();
        Some(QuitView {
            project,
            selected: dialog.selected,
            dont_ask: dialog.dont_ask,
            hover: dialog.hover,
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
