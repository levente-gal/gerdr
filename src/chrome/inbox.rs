//! The inbox panel: what wants the user across the shown project, a
//! section per kind (agents, Git, GitHub, todos), as a grouped list on the
//! left under a field filtering it (and composing todos), the selected
//! item's detail on the right, and the footer's key hints.
//!
//! This module only knows what it draws: the model (`crate::inbox`) turns
//! its items into an `InboxView`.

#![allow(dead_code)]

use super::{
    Canvas, FONT_SIZE, FieldSpot, HELP_MARGIN, HELP_RADIUS, LABEL_FONT_SIZE, PADDING, Painter,
    Palette, Rect, SHADOW, band, footer_hint_spots, hint_at, hint_strs, phys, physical,
    project_menu::menu_query_text,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The panel: at most this wide and high, a margin in from the output's
/// edges.
const INBOX_MAX_WIDTH: i32 = 1400;
const INBOX_MAX_HEIGHT: i32 = 1000;
/// Its title row.
const INBOX_TITLE: i32 = 44;
/// How high an item's row is, and a section's label.
pub const INBOX_ROW: i32 = 44;
pub const INBOX_LABEL: i32 = 28;

/// What the inbox panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxView {
    /// The shown project's name, after "Inbox" in the title row.
    pub project: Option<String>,
    /// The scope toggle: every workspace's items (`true`) or the active
    /// one's. `None` without a choice (no active workspace).
    pub all: Option<bool>,
    /// What the list is filtered by.
    pub query: Line,
    /// What the field says while empty.
    pub placeholder: String,
    /// A todo being written (Ctrl+N, Ctrl+E): it takes the field's place.
    pub compose: Option<InboxCompose>,
    /// The list's rows in view, scrolled: section labels and items.
    pub rows: Vec<InboxRow>,
    /// Where `rows` start among all the list's rows, and how many there
    /// are: where the list's scrollbar stands.
    pub first: usize,
    pub total: usize,
    /// The selected row, by index into `rows`.
    pub selected: Option<usize>,
    pub hover: Option<InboxHit>,
    /// The selected item's detail; `None` with nothing selected.
    pub detail: Option<InboxDetailView>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
    /// What the list says when it is empty.
    pub empty: String,
    /// Hidden (dismissed, snoozed) items are listed too (Ctrl+H).
    pub show_hidden: bool,
}

/// A todo being written in the inbox panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxCompose {
    pub text: Line,
    /// The workspace it is linked to, by name (Tab cycles it); `None`: no
    /// workspace.
    pub workspace: Option<String>,
    /// Editing an existing todo, rather than a new one.
    pub editing: bool,
}

/// A row of the inbox list.
#[derive(Clone, Debug, PartialEq)]
pub enum InboxRow {
    /// A section's label, as `Git 2 · 1 hidden`; not selectable.
    Label {
        title: String,
        count: usize,
        hidden: usize,
    },
    Item(InboxLine),
}

/// An item's row.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxLine {
    /// The item's id, as `crate::inbox::ItemId` holds it.
    pub id: String,
    pub title: String,
    /// Under the title: `workspace · status`.
    pub subtitle: String,
    /// The dot before the title.
    pub tone: InboxTone,
    /// At the right: `5m`, `2h`, `3d`; empty when unknown.
    pub age: String,
    /// Dismissed or snoozed, listed with Ctrl+H: muted.
    pub hidden: bool,
    /// What it can have done, shown on it when selected or hovered.
    pub buttons: Vec<InboxButton>,
}

/// What colour an item's dot (and its detail's title mark) is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InboxTone {
    /// An agent waiting for input: the blocked colour.
    Blocked,
    /// An agent done with unseen work.
    Done,
    /// An agent at work.
    Working,
    /// CI failed.
    Failed,
    /// Git needing a look: dirty, ahead/behind, an op in progress.
    Attention,
    /// A pull request asking for review.
    Review,
    /// The rest: todos, issues.
    #[default]
    Info,
}

/// A button on an item's row or in the detail: its label and the key that
/// does the same (shown in tooltips and the detail), as `("Snooze",
/// "Ctrl+S")`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxButton {
    pub label: String,
    pub key: String,
}

/// The right side: what the selected item is, in detail.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxDetailView {
    pub title: String,
    pub tone: InboxTone,
    /// Label and value lines: `Status`, `Workspace`, `Branch`…
    pub fields: Vec<(String, String)>,
    /// A heading over `block`: `Last output`, `Changes`, `Checks`.
    pub block_title: Option<String>,
    /// Monospace lines: the pane's last lines, the changed files, the
    /// failing checks.
    pub block: Vec<String>,
    /// What can be done to it.
    pub buttons: Vec<InboxButton>,
}

/// What the pointer is over in the inbox panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxHit {
    /// A row, by index into `InboxView::rows`.
    Row(usize),
    /// Button `k` on row `i`.
    Button(usize, usize),
    /// Button `k` of the detail.
    DetailButton(usize),
    /// The filter (or compose) field.
    Query,
    /// The scope toggle's segment for every workspace (`true`) or the
    /// active one's.
    Scope(bool),
    /// A key hint in the footer, by index into `InboxView::hints`.
    Hint(usize),
    /// Inside the panel, on nothing.
    Panel,
}

/// Where the inbox panel's parts go, in output coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The scope toggle's segments, as (for every workspace, rect); none
    /// without a choice.
    pub scope: Vec<(bool, Rect)>,
    /// The filter (or compose) field.
    pub query: Rect,
    /// The list, left.
    pub list: Rect,
    /// Per row in view, its rect and its buttons' (none unless selected or
    /// hovered).
    pub rows: Vec<(Rect, Vec<Rect>)>,
    /// The detail, right.
    pub detail: Rect,
    pub detail_buttons: Vec<Rect>,
    pub footer: Rect,
    /// The footer's key hints, by index into `InboxView::hints`.
    pub hints: Vec<(usize, Rect)>,
}

/// The panel on an output of `size`: centred, a margin in from its edges.
fn panel_rect(size: Size<i32, Logical>) -> Rect {
    let w = INBOX_MAX_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let h = INBOX_MAX_HEIGHT.min(size.h - 2 * HELP_MARGIN).max(1);
    Rectangle::new(
        (((size.w - w) / 2).max(0), ((size.h - h) / 2).max(0)).into(),
        (w, h).into(),
    )
}

/// How many item rows the inbox list shows at once on an output of `size`.
pub fn inbox_page(size: Size<i32, Logical>) -> usize {
    let room = panel_rect(size).size.h - INBOX_TITLE - INBOX_ROW;
    (room / INBOX_ROW).max(1) as usize
}

/// Where the parts of the inbox panel showing `view` go, on an output of
/// `size`.
pub fn inbox_layout(view: &InboxView, size: Size<i32, Logical>) -> InboxLayout {
    let panel = panel_rect(size);
    let title = Rectangle::new(panel.loc, (panel.size.w, INBOX_TITLE).into());
    let query = Rectangle::new(
        (panel.loc.x, panel.loc.y + INBOX_TITLE).into(),
        (panel.size.w, INBOX_ROW).into(),
    );
    let _ = view;
    InboxLayout {
        panel,
        title,
        query,
        ..InboxLayout::default()
    }
}

/// What `pos` (output coordinates) is over in the inbox panel; `None`
/// outside it.
pub fn inbox_hit(
    view: &InboxView,
    layout: &InboxLayout,
    pos: Point<f64, Logical>,
) -> Option<InboxHit> {
    if !layout.panel.to_f64().contains(pos) {
        return None;
    }
    if let Some(i) = hint_at(&hint_strs(&view.hints), &layout.hints, pos) {
        return Some(InboxHit::Hint(i));
    }
    if layout.query.to_f64().contains(pos) {
        return Some(InboxHit::Query);
    }
    Some(InboxHit::Panel)
}

/// The inbox panel's field with the keyboard (the filter, or the todo
/// being composed), on an output of `size`.
pub fn inbox_field(view: &InboxView, size: Size<i32, Logical>, scale: f64) -> FieldSpot {
    let area = inbox_layout(view, size).query;
    let line = match &view.compose {
        Some(compose) => compose.text.clone(),
        None => view.query.clone(),
    };
    FieldSpot {
        size: FONT_SIZE,
        area,
        text: menu_query_text(area, scale),
        line,
    }
}

impl Painter {
    /// The inbox panel for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow.
    pub(super) fn inbox(
        &self,
        view: &InboxView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = inbox_layout(view, size);
        let panel = layout.panel;
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        c.popup(panel.size, HELP_RADIUS, scale, p);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let title = Rectangle::new(layout.title.loc + by, layout.title.size);
        let (x, right) = phys(title, scale);
        self.text(
            &mut c,
            "Inbox",
            LABEL_FONT_SIZE,
            x + PADDING * scale as f32,
            right,
            band(title, scale),
            scale,
            p.text,
        );
        let _ = footer_hint_spots;
        c.into_buffer(false)
    }
}
