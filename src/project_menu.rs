//! The project menu: a dropdown from the sidebar header, while a project is
//! shown, to find, switch to, start, rename, stop and forget projects.
//!
//! Typing filters the projects; under them, rows start a new project, named
//! as typed or in a folder zoxide knows that matches. Enter opens the
//! selected entry, Alt+1 to 9 a recent project, Ctrl+N the form for a new
//! one, Ctrl+R renames the selected project in place, Ctrl+S stops its
//! session, Delete twice forgets it. The form asks for the name, an
//! optional folder (typed, picked from zoxide's, or browsed for) and what
//! the folder opens as. Esc backs out one step at a time.

// Contract: wave 1 fills it in and wires it up.
#![allow(dead_code)]

use std::path::PathBuf;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    chrome::{MenuHit, ProjectMenuView},
};

/// The project menu's state, while it is open.
#[derive(Debug, Default)]
pub struct ProjectMenu {
    /// What the pointer is over.
    pub hover: Option<MenuHit>,
}

/// What the project menu asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Show project `id`.
    Switch(String),
    /// Start a new project and show it, opening `folder` in it as `kind`.
    Create {
        name: String,
        folder: Option<PathBuf>,
        kind: WorkspaceKind,
    },
    /// Rename project `id` (the first) to the second.
    Rename(String, String),
    /// Stop project `id`'s session; not the shown one.
    Stop(String),
    /// Forget project `id`; not the shown one.
    Forget(String),
    /// Ask zenity for the form's folder, starting in this one.
    Browse(Option<PathBuf>),
    /// Close the menu.
    Closed,
}

impl Gerdr {
    /// Open the project menu, and find out which projects' sessions run.
    pub fn open_project_menu(&mut self) {
        todo!("open the project menu")
    }

    /// Open the project menu, or close it if it is open.
    pub fn toggle_project_menu(&mut self) {
        todo!("toggle the project menu")
    }

    /// A key while the project menu has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn project_menu_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let _ = (mods, sym, typed);
        todo!("a key in the project menu")
    }

    /// Do what the project menu asked.
    pub fn apply_project_menu(&mut self, outcome: Outcome) {
        let _ = outcome;
        todo!("apply the project menu's outcome")
    }

    /// A left click while the project menu is open.
    pub fn project_menu_click(&mut self, pos: Point<f64, Logical>) {
        let _ = pos;
        todo!("a click in the project menu")
    }

    /// The pointer moved while the project menu is open.
    pub fn project_menu_hover(&mut self, pos: Point<f64, Logical>) {
        let _ = pos;
        todo!("hover in the project menu")
    }

    /// The wheel turned `rows` rows (down positive) over the project menu.
    pub fn project_menu_scroll(&mut self, rows: i32) {
        let _ = rows;
        todo!("scroll the project menu")
    }

    pub fn project_menu_view(&self) -> Option<ProjectMenuView> {
        // Never open before wave 1.
        None
    }

    /// zenity gave back `dir` for the form's folder.
    pub fn project_menu_folder(&mut self, dir: PathBuf) {
        let _ = dir;
        todo!("take the browsed folder")
    }

    /// The projects, or which of them run, changed: show them again.
    pub fn refresh_project_menu(&mut self) {
        // Nothing to refresh before wave 1.
    }
}
