//! The welcome screen: shown when no project is open (first run, or the last
//! project was stopped), and on demand to switch projects. A greeting, the
//! recent projects to filter and pick from, and a button to start a new one.

#![allow(dead_code)] // contract: implemented by the overlay state work

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{Gerdr, chrome::WelcomeView};

/// The welcome screen's state, while it is shown.
#[derive(Debug, Default)]
pub struct Welcome {
    /// Opened from inside a project (Esc goes back to it), rather than shown
    /// because none is open.
    pub in_app: bool,
}

/// What the welcome screen asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Open project `id`.
    Pick(String),
    /// Start a new project with this name.
    Create(String),
    /// Forget project `id` (its herdr session stays).
    Forget(String),
    /// Go back to the project behind it.
    Closed,
}

impl Gerdr {
    /// Show the welcome screen, and find out which projects' sessions run.
    pub fn open_welcome(&mut self, in_app: bool) {
        let _ = in_app;
        todo!("overlay state")
    }

    /// A key while the welcome screen has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn welcome_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let _ = (mods, sym, typed);
        todo!("overlay state")
    }

    /// Do what the welcome screen asked.
    pub fn apply_welcome(&mut self, outcome: Outcome) {
        let _ = outcome;
        todo!("overlay state")
    }

    /// A left click while the welcome screen is shown; `false` if it isn't.
    pub fn welcome_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let _ = pos;
        todo!("overlay state")
    }

    /// The pointer moved while the welcome screen is shown.
    pub fn welcome_hover(&mut self, pos: Point<f64, Logical>) {
        let _ = pos;
        todo!("overlay state")
    }

    pub fn welcome_view(&self) -> Option<WelcomeView> {
        todo!("overlay state")
    }
}
