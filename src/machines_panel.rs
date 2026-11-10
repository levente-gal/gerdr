//! The machines panel: every machine saved, how its link stands, and what of
//! this gerdr's features runs there, with buttons to connect, disconnect,
//! remove or fix a machine and to show or stop what runs on it (see
//! `chrome::machines_panel`). It opens with the prefix, then O, from the
//! launcher, and from a machine's chip in the footer.
//!
//! Up/Down move through the machines, Tab/Shift+Tab through the buttons of
//! the one selected, Enter presses the focused button (on the list: the
//! machine's primary one), Del arms and then removes the machine, Esc closes.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, MachinesFocus, MachinesHit, MachinesPanelView},
    feature::Stage,
};

/// The machines panel's state, while it is open.
#[derive(Debug, Default)]
pub struct MachinesPanel {
    /// The list row selected: past the machines is the add row.
    pub selected: usize,
    pub first: usize,
    pub focus: MachinesFocus,
    pub runs_first: usize,
    pub hover: Option<MachinesHit>,
    /// The machine Del was pressed on once: a second Del removes it.
    pub armed: Option<String>,
}

/// A run on a machine, as the panel's buttons act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunRef {
    /// A stage's agent of a feature: project, slug, stage.
    Agent(String, String, Stage),
    /// A feature's Verify checks or setup: project, slug.
    Checks(String, String),
    /// A feature's sandbox: project, slug.
    Sandbox(String, String),
    /// What gone features left on the machine of this id.
    Left(String),
}

/// What a press in the machines panel does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// Open the dialog connecting a machine.
    Add,
    /// Connect, disconnect or remove the machine of this name.
    Connect(String),
    Disconnect(String),
    Remove(String),
    /// Press the button fixing what failed on the machine of this name
    /// (Copy command, Open terminal).
    Fix(String),
    /// Show what runs: its tab here.
    Show(RunRef),
    /// Stop it, asking as its own page does.
    Stop(RunRef),
    /// Open the feature's tab: project, slug.
    OpenFeature(String, String),
}

/// What the machines panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Press(Press),
}

impl Gerdr {
    /// Open the machines panel, on the machine of name `on` if given.
    /// The other panels close: it would cover them.
    pub fn open_machines(&mut self, on: Option<&str>) {
        let _ = on;
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.machines_panel = Some(MachinesPanel::default());
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the machines panel.
    pub fn close_machines(&mut self) {
        self.machines_panel = None;
        self.refresh_pointer_focus();
    }

    pub fn machines_panel_view(&self) -> Option<MachinesPanelView> {
        let panel = self.machines_panel.as_ref()?;
        Some(MachinesPanelView {
            rows: Vec::new(),
            selected: panel.selected,
            first: panel.first,
            detail: None,
            focus: panel.focus,
            runs_first: panel.runs_first,
            hover: panel.hover,
            hints: vec![("Esc".to_owned(), "close".to_owned())],
        })
    }

    /// A key while the machines panel has the keyboard.
    pub fn machines_panel_key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let _ = mods;
        self.machines_panel.as_ref()?;
        (sym == Keysym::Escape).then_some(Outcome::Close)
    }

    /// Do what the machines panel asked.
    pub fn apply_machines_panel(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_machines(),
            Outcome::Press(_) => {}
        }
    }

    /// A left click while the machines panel is open: off it closes it.
    pub fn machines_panel_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.machines_panel_view() else {
            return;
        };
        if chrome::machines_layout(&view, self.output_size()).hit(pos).is_none() {
            self.close_machines();
        }
    }

    /// The pointer moved while the machines panel is open.
    pub fn machines_panel_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = (self.machines_panel_view())
            .and_then(|view| chrome::machines_layout(&view, self.output_size()).hit(pos));
        if let Some(panel) = self.machines_panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel or touchpad over the machines panel: `rows` rows on, in the
    /// runs when the pointer is on them (see `MachinesPanel::hover`), else
    /// in the list.
    pub fn machines_panel_scroll(&mut self, rows: i32) {
        let _ = rows;
    }

    /// The list rows of the machines panel, as laid out now: for how far a
    /// notch of the wheel goes.
    pub fn machines_panel_rows(&self) -> Vec<chrome::Rect> {
        (self.machines_panel_view())
            .map_or_else(Vec::new, |v| chrome::machines_layout(&v, self.output_size()).rows)
    }

    /// What the pointer can press in the machines panel, while it is open.
    pub fn machines_panel_controls(&self) -> Vec<chrome::Rect> {
        (self.machines_panel_view())
            .map_or_else(Vec::new, |v| chrome::machines_layout(&v, self.output_size()).controls())
    }
}
