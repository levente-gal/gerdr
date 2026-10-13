//! Closing gerdr with a project open: keep its session running in the
//! background (the default), or stop it. "Don't ask again" remembers the
//! choice; the quit action (prefix, Shift+Q) asks again regardless.

#![allow(dead_code)] // contract: implemented by the overlay state work

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{Gerdr, chrome::QuitView};

/// The quit dialog's state, while it is open.
#[derive(Debug, Default)]
pub struct QuitDialog {
    /// 0 Cancel, 1 Stop project, 2 Keep running (as `QuitView::selected`).
    pub selected: usize,
    pub dont_ask: bool,
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

impl Gerdr {
    /// The window is closing: quit right away without a project, do what
    /// was remembered, or ask.
    pub fn request_quit(&mut self) {
        todo!("overlay state")
    }

    /// Ask, even if a choice was remembered.
    pub fn open_quit(&mut self) {
        todo!("overlay state")
    }

    /// A key while the quit dialog has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn quit_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let _ = (mods, sym, typed);
        todo!("overlay state")
    }

    /// Do what the quit dialog asked.
    pub fn apply_quit(&mut self, outcome: Outcome) {
        let _ = outcome;
        todo!("overlay state")
    }

    /// A left click while the quit dialog is open; `false` if it isn't.
    pub fn quit_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let _ = pos;
        todo!("overlay state")
    }

    /// The pointer moved while the quit dialog is open.
    pub fn quit_hover(&mut self, pos: Point<f64, Logical>) {
        let _ = pos;
        todo!("overlay state")
    }

    pub fn quit_view(&self) -> Option<QuitView> {
        todo!("overlay state")
    }
}
