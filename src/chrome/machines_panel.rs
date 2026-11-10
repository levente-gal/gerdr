//! The machines panel: the machines gerdr reaches over SSH listed on the
//! left (and a row adding one), the selected one on the right: how its link
//! stands, what of this gerdr's features runs there with buttons to show or
//! stop it, and buttons acting on the machine (see `machines_panel`).

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{Dot, POPUP_RADIUS, Painter, Palette, Rect};

/// What the machines panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinesPanelView {
    /// The machines saved, by name. The row adding one comes after them.
    pub rows: Vec<MachineRowView>,
    /// The row under the keyboard's cursor: `rows.len()` is the add row.
    pub selected: usize,
    /// The first list row shown, when not all fit.
    pub first: usize,
    /// The selected machine; none on the add row.
    pub detail: Option<MachineDetailView>,
    /// What the keyboard is on.
    pub focus: MachinesFocus,
    /// The first run shown in the detail, when not all fit.
    pub runs_first: usize,
    pub hover: Option<MachinesHit>,
    /// The key hints along the bottom: keys, what they do.
    pub hints: Vec<(String, String)>,
}

/// A machine's row in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRowView {
    pub name: String,
    /// The address its link uses, or the one saved.
    pub address: String,
    /// How its link stands, shortly: "connected", "away", "off"...
    pub state: String,
    pub dot: Dot,
    /// Del was pressed once on it: the row says "Press Del again".
    pub armed: bool,
}

/// The selected machine, on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineDetailView {
    pub name: String,
    pub dot: Dot,
    /// Lines about its link, top first: its address, then "up 2h · herdr
    /// 0.9.3 · load 1.2 / 8", then its id.
    pub facts: Vec<String>,
    /// Why it isn't connected, and what to do about it, when it isn't.
    pub problem: Option<String>,
    pub how: Option<String>,
    /// What of this gerdr's features runs there.
    pub runs: Vec<MachineRunView>,
    /// What the runs list says when it is empty: "Nothing runs there",
    /// "Connect it to see what runs there".
    pub no_runs: String,
    /// The buttons acting on the machine, left to right; the last is the
    /// primary one.
    pub buttons: Vec<String>,
}

/// A run on the machine: an agent, a feature's checks, a sandbox, what a
/// gone feature left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRunView {
    /// "asset-tag · Build agent".
    pub title: String,
    /// "Claude · working · 12m".
    pub detail: String,
    pub dot: Dot,
    pub buttons: Vec<String>,
}

/// What the keyboard is on in the machines panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MachinesFocus {
    /// The list: Up/Down move through the machines.
    #[default]
    List,
    /// Run `.0`'s button `.1`.
    Run(usize, usize),
    /// The machine's button `.0`.
    Button(usize),
}

/// What the pointer is over in the machines panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachinesHit {
    Panel,
    /// The list's row `.0`; `rows.len()` is the add row.
    Row(usize),
    /// Run `.0`, off its buttons.
    Run(usize),
    /// Run `.0`'s button `.1`.
    RunButton(usize, usize),
    /// The machine's button `.0`.
    Button(usize),
    /// Key hint `.0`: a click presses its key.
    Hint(usize),
}

/// Where the machines panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinesLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The list's column.
    pub list: Rect,
    /// The list row shown first: `rows[0]` is row `first`.
    pub first: usize,
    /// The list rows shown, from `first`; the add row is among them when
    /// it shows.
    pub rows: Vec<Rect>,
    /// The list's scrollbar track, when not all rows fit.
    pub track: Option<Rect>,
    /// The detail's column.
    pub detail: Rect,
    /// The run shown first: `runs[0]` is run `runs_first`.
    pub runs_first: usize,
    /// The runs shown, from `runs_first`.
    pub runs: Vec<Rect>,
    /// Each shown run's buttons, in order.
    pub run_buttons: Vec<Vec<Rect>>,
    /// The runs' scrollbar track, when not all fit.
    pub runs_track: Option<Rect>,
    /// The machine's buttons, in order.
    pub buttons: Vec<Rect>,
    /// The key hints, as (index into the view's hints, rect).
    pub hints: Vec<(usize, Rect)>,
}

/// How many list rows (machines and the add row) fit on an output of
/// `output`: at least one.
pub fn machines_shown(rows: usize, output: Size<i32, Logical>) -> usize {
    let _ = output;
    rows.max(1)
}

/// How many runs fit in the detail on an output of `output`: at least one.
pub fn machine_runs_shown(view: &MachinesPanelView, output: Size<i32, Logical>) -> usize {
    let _ = output;
    view.detail.as_ref().map_or(1, |d| d.runs.len().max(1))
}

/// Where the machines panel's parts go on an output of `output`: centred.
pub fn machines_layout(view: &MachinesPanelView, output: Size<i32, Logical>) -> MachinesLayout {
    let _ = view;
    let panel = Rectangle::new((0, 0).into(), output);
    MachinesLayout {
        panel,
        title: panel,
        list: panel,
        first: 0,
        rows: Vec::new(),
        track: None,
        detail: panel,
        runs_first: 0,
        runs: Vec::new(),
        run_buttons: Vec::new(),
        runs_track: None,
        buttons: Vec::new(),
        hints: Vec::new(),
    }
}

impl MachinesLayout {
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<MachinesHit> {
        self.panel.to_f64().contains(pos).then_some(MachinesHit::Panel)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        (self.buttons.iter())
            .chain(self.run_buttons.iter().flatten())
            .chain(self.rows.iter())
            .copied()
            .chain(self.hints.iter().map(|(_, r)| *r))
            .collect()
    }
}

impl Painter {
    /// The machines panel on an output of `output`, inside a margin of
    /// `SHADOW` holding its shadow.
    pub(super) fn machines_panel(
        &self,
        view: &MachinesPanelView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = machines_layout(view, output);
        let c = self.popup(layout.panel.size, POPUP_RADIUS, scale, p);
        c.into_buffer(false)
    }
}
