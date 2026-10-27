//! The inbox panel: what wants the user across the shown project, a
//! section per kind (agents, Git, GitHub, todos), as a grouped list on the
//! left under a field filtering it (and composing todos), the selected
//! item's detail on the right, and the footer's key hints.
//!
//! This module only knows what it draws: the model (`crate::inbox`) turns
//! its items into an `InboxView`.

use super::{
    BUTTON_RADIUS, Canvas, DETAIL_FONT_SIZE, FIELD_GAP, FONT_SIZE, FOOTER_ROW, FieldSpot,
    HELP_MARGIN, HELP_PADDING, HELP_RADIUS, KEY_FONT_SIZE, LABEL_FONT_SIZE, MENU_MARK_GAP,
    MENU_MARK_WIDTH, MENU_RADIUS, PADDING, PROJECT_MENU_FOOTER, PROJECT_MENU_INPUT,
    PROJECT_MENU_PADDING, Painter, Palette, ROW_BUTTON, ROW_BUTTON_INSET, Rect, Rgb, SHADOW,
    SMALL_FONT_SIZE, STROKE, band, footer_height, footer_hint_spots, hint_at, hint_strs, icon_sdf,
    magnifier, measure, mix, pencil, phys, plus,
    project_menu::{menu_mark, menu_query_text},
    rect_px,
    repos::scrollbar,
    segment,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The panel: at most this wide and high, a margin in from the output's
/// edges.
const INBOX_MAX_WIDTH: i32 = 1400;
const INBOX_MAX_HEIGHT: i32 = 1000;
/// Its title row, and the field's under it.
const INBOX_TITLE: i32 = 44;
const INBOX_FIELD: i32 = PROJECT_MENU_INPUT;
/// How high an item's row is, and a section's label.
pub const INBOX_ROW: i32 = 44;
pub const INBOX_LABEL: i32 = 28;
/// The list's share of the panel's width, and the narrowest it gets
/// (short of half the panel).
const INBOX_LIST_SHARE: f64 = 0.4;
const INBOX_LIST_MIN: i32 = 260;
/// The room above and below the list's rows, and beside them.
const INBOX_PAD: i32 = PROJECT_MENU_PADDING;
/// The list's scrollbar, and the gap before it.
const SCROLLBAR: i32 = 6;
const SCROLLBAR_GAP: i32 = 4;
/// The scope toggle's segments in the title row: "This workspace" and
/// "All", and how far it keeps from the right edge.
const SCOPE_OWN: i32 = 120;
const SCOPE_ALL: i32 = 48;
const SCOPE_INSET: i32 = 12;
/// The gap between buttons, and the room either side of a label on one.
const BUTTON_GAP: i32 = 4;
const BUTTON_PAD: i32 = 10;
/// How much of an item's row its buttons leave for its title.
const ROW_TEXT_MIN: i32 = 120;
/// An item's dot, and the lines of its row: the title's band from the
/// top, then the subtitle's.
const DOT: i32 = 8;
const ROW_TITLE_TOP: i32 = 4;
const ROW_TITLE: i32 = 20;
const ROW_SUBTITLE: i32 = 16;
/// The detail: its title row, a field's line, the widest its labels'
/// column gets, the heading over its block, a line of the block in the
/// monospace font of this size, and the room inside the block.
const DETAIL_TITLE: i32 = 44;
const DETAIL_LINE: i32 = 24;
const DETAIL_LABEL_MAX: i32 = 140;
const DETAIL_HEADING: i32 = 28;
const BLOCK_LINE: i32 = 19;
const BLOCK_FONT_SIZE: f32 = 13.0;
const BLOCK_PAD: i32 = 6;
/// The detail's buttons: their height, the gap between their rows, and
/// the most rows (the rest are left out; their keys still work).
const DETAIL_BUTTON: i32 = 30;
const DETAIL_BUTTON_ROWS: usize = 2;
/// The chip at the end of the field while composing: its height and the
/// room either side of what it says, the widest share of the field it
/// takes, and its arrow's width.
const CHIP_HEIGHT: i32 = 24;
const CHIP_PAD: i32 = 8;
const CHIP_SHARE: f64 = 0.4;
const CHIP_ARROW: i32 = 12;
/// What the chip says without a workspace.
const NO_WORKSPACE: &str = "no workspace";

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
    /// What matters of `block` is at its end (a pane's last lines, an
    /// agent's question): cut short at its start rather than its end.
    pub block_tail: bool,
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

/// How wide `text` is at `size` in logical pixels; without a font, a
/// guess on the wide side.
fn extent(text: &str, size: f32) -> i32 {
    measure(text, size)
        .unwrap_or(text.chars().count() as f32 * size * 0.6)
        .ceil() as i32
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

/// The list and the detail beside it, between the field and a footer
/// `footer_h` high in `panel`.
fn body(panel: Rect, footer_h: i32) -> (Rect, Rect) {
    let top = panel.loc.y + INBOX_TITLE + INBOX_FIELD;
    let h = (panel.loc.y + panel.size.h - footer_h - top).max(0);
    let w = panel.size.w;
    let list_w = ((w as f64 * INBOX_LIST_SHARE).round() as i32).max(INBOX_LIST_MIN.min(w / 2));
    (
        Rectangle::new((panel.loc.x, top).into(), (list_w, h).into()),
        Rectangle::new((panel.loc.x + list_w, top).into(), (w - list_w, h).into()),
    )
}

/// Where the list's rows go across `list`, as (left, width): short of the
/// scrollbar at its right.
fn row_span(list: Rect) -> (i32, i32) {
    let w = list.size.w - INBOX_PAD - 2 * SCROLLBAR_GAP - SCROLLBAR;
    (list.loc.x + INBOX_PAD, w.max(0))
}

/// The list's scrollbar track, at the right of `list`.
fn scroll_track(list: Rect) -> Rect {
    Rectangle::new(
        (
            list.loc.x + list.size.w - SCROLLBAR_GAP - SCROLLBAR,
            list.loc.y + INBOX_PAD,
        )
            .into(),
        (SCROLLBAR, (list.size.h - 2 * INBOX_PAD).max(0)).into(),
    )
}

/// How many item rows the inbox list shows at once on an output of `size`:
/// as many as fit with the footer at its highest, so a page of rows always
/// fits (a section's label is lower than an item's row).
pub fn inbox_page(size: Size<i32, Logical>) -> usize {
    let (list, _) = body(panel_rect(size), footer_height(usize::MAX));
    ((list.size.h - 2 * INBOX_PAD) / INBOX_ROW).max(1) as usize
}

/// How high the footer is with `hints` across `w`: a row for each they
/// take, so a second row is kept only when they need it.
fn footer_height_for(hints: &[(&str, &str)], w: i32) -> i32 {
    let tall = Rectangle::new((0, 0).into(), (w, footer_height(hints.len())).into());
    let spots = footer_hint_spots(hints, tall);
    let rows = spots
        .iter()
        .map(|(_, r)| r.loc.y)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        .max(1) as i32;
    PROJECT_MENU_FOOTER + (rows - 1) * FOOTER_ROW
}

/// How wide a row's button is: its label and the room either side.
fn row_button_width(button: &InboxButton) -> i32 {
    extent(&button.label, SMALL_FONT_SIZE) + 2 * BUTTON_PAD
}

/// The first of `buttons` that fit at the right end of a row in `rect`,
/// leaving its title `ROW_TEXT_MIN`, vertically centred; by index into
/// `buttons`.
fn row_buttons(buttons: &[InboxButton], rect: Rect) -> Vec<Rect> {
    let room = rect.size.w - ROW_BUTTON_INSET - ROW_TEXT_MIN;
    let mut widths = Vec::new();
    let mut used = -BUTTON_GAP;
    for button in buttons {
        let w = row_button_width(button);
        if used + BUTTON_GAP + w > room {
            break;
        }
        used += BUTTON_GAP + w;
        widths.push(w);
    }
    let mut x = rect.loc.x + rect.size.w - ROW_BUTTON_INSET - used.max(0);
    let top = rect.loc.y + (rect.size.h - ROW_BUTTON) / 2;
    widths
        .into_iter()
        .map(|w| {
            let r = Rectangle::new((x, top).into(), (w, ROW_BUTTON).into());
            x += w + BUTTON_GAP;
            r
        })
        .collect()
}

/// How wide a detail's button is: its label, then its key with `keys`.
fn detail_button_width(button: &InboxButton, keys: bool) -> i32 {
    let key = if button.key.is_empty() || !keys {
        0
    } else {
        MENU_MARK_GAP + extent(&button.key, KEY_FONT_SIZE)
    };
    2 * BUTTON_PAD + extent(&button.label, SMALL_FONT_SIZE) + key
}

/// Where the parts of the detail showing `detail` go in `rect`.
struct DetailParts {
    title: Rect,
    /// A rect per field shown, the first ones that fit.
    fields: Vec<Rect>,
    /// How wide the fields' labels' column is.
    label_w: i32,
    /// The heading over the block, and the block, when they fit.
    heading: Option<Rect>,
    block: Option<Rect>,
    /// The first of its buttons that fit, along the bottom.
    buttons: Vec<Rect>,
    /// They show their keys: they do unless that takes them a second row.
    keys: bool,
}

/// The detail's parts in `rect`: its title row; its fields, then the
/// heading and its block from the top; its buttons in rows from the
/// bottom.
fn detail_parts(detail: &InboxDetailView, rect: Rect) -> DetailParts {
    let x = rect.loc.x + HELP_PADDING;
    let w = (rect.size.w - 2 * HELP_PADDING).max(0);
    let bottom = rect.loc.y + rect.size.h - HELP_PADDING;

    // The buttons, flowed in rows; those that don't fit are left out.
    let one_row = |keys: bool| {
        let widths = detail.buttons.iter().map(|b| detail_button_width(b, keys));
        widths.sum::<i32>() + BUTTON_GAP * (detail.buttons.len() as i32 - 1) <= w
    };
    let keys = one_row(true) || !one_row(false);
    let mut rows: Vec<Vec<i32>> = Vec::new();
    let mut row_w = 0;
    for button in &detail.buttons {
        let bw = detail_button_width(button, keys).min(w);
        let fits = !rows.is_empty() && row_w + BUTTON_GAP + bw <= w;
        if fits && let Some(row) = rows.last_mut() {
            row.push(bw);
            row_w += BUTTON_GAP + bw;
        } else if rows.len() < DETAIL_BUTTON_ROWS && bw > 0 {
            rows.push(vec![bw]);
            row_w = bw;
        } else {
            break;
        }
    }
    let row_step = DETAIL_BUTTON + 2 * BUTTON_GAP;
    let buttons_top = bottom - rows.len() as i32 * row_step + 2 * BUTTON_GAP;
    let buttons = rows
        .iter()
        .enumerate()
        .flat_map(|(r, widths)| {
            let y = buttons_top + r as i32 * row_step;
            let mut bx = x;
            widths.iter().map(move |&bw| {
                let b = Rectangle::new((bx, y).into(), (bw, DETAIL_BUTTON).into());
                bx += bw + BUTTON_GAP;
                b
            })
        })
        .collect();
    let end = if rows.is_empty() {
        bottom
    } else {
        buttons_top - HELP_PADDING
    };

    let title = Rectangle::new(rect.loc, (rect.size.w, DETAIL_TITLE).into());
    let mut y = rect.loc.y + DETAIL_TITLE + BLOCK_PAD;
    let mut fields = Vec::new();
    for _ in &detail.fields {
        if y + DETAIL_LINE > end {
            break;
        }
        fields.push(Rectangle::new((x, y).into(), (w, DETAIL_LINE).into()));
        y += DETAIL_LINE;
    }
    let widest = detail
        .fields
        .iter()
        .map(|(label, _)| extent(label, SMALL_FONT_SIZE))
        .max()
        .unwrap_or(0);
    let label_w = (widest + 2 * FIELD_GAP).min(DETAIL_LABEL_MAX).min(w / 2);

    let mut heading = None;
    if detail.block_title.is_some() && y + BLOCK_PAD + DETAIL_HEADING <= end {
        y += BLOCK_PAD;
        heading = Some(Rectangle::new((x, y).into(), (w, DETAIL_HEADING).into()));
        y += DETAIL_HEADING;
    } else if !fields.is_empty() {
        y += BLOCK_PAD;
    }
    let mut block = None;
    let room = end - y;
    if !detail.block.is_empty() && room >= BLOCK_LINE + 2 * BLOCK_PAD {
        let wanted = detail.block.len() as i32 * BLOCK_LINE + 2 * BLOCK_PAD;
        block = Some(Rectangle::new((x, y).into(), (w, wanted.min(room)).into()));
    }
    DetailParts {
        title,
        fields,
        label_w,
        heading,
        block,
        buttons,
        keys,
    }
}

/// What the chip at the end of the field says while composing.
fn chip_label(compose: &InboxCompose) -> &str {
    compose.workspace.as_deref().unwrap_or(NO_WORKSPACE)
}

/// The chip at the right end of the field `query` saying which workspace
/// the todo being composed is linked to.
fn compose_chip(compose: &InboxCompose, query: Rect) -> Rect {
    let arrow = if compose.workspace.is_some() {
        CHIP_ARROW + BUTTON_GAP
    } else {
        0
    };
    let w = (2 * CHIP_PAD + arrow + extent(chip_label(compose), SMALL_FONT_SIZE))
        .min((query.size.w as f64 * CHIP_SHARE) as i32);
    Rectangle::new(
        (
            query.loc.x + query.size.w - HELP_PADDING - w,
            query.loc.y + (query.size.h - CHIP_HEIGHT) / 2,
        )
            .into(),
        (w, CHIP_HEIGHT).into(),
    )
}

/// Where the text of the field `query` goes (physical pixels): after its
/// mark, up to its end or, while composing, the chip.
fn field_text(view: &InboxView, query: Rect, scale: f64) -> (f32, f32) {
    let (x, end) = menu_query_text(query, scale);
    match &view.compose {
        Some(compose) => {
            let chip = compose_chip(compose, query);
            let to = ((chip.loc.x - FIELD_GAP) as f64 * scale).round() as f32;
            (x, end.min(to).max(x))
        }
        None => (x, end),
    }
}

/// Where the parts of the inbox panel showing `view` go, on an output of
/// `size`: the title row (the scope toggle at its right), the field, the
/// list at the left and the detail at the right, then the footer.
pub fn inbox_layout(view: &InboxView, size: Size<i32, Logical>) -> InboxLayout {
    let panel = panel_rect(size);
    let (x, top, w) = (panel.loc.x, panel.loc.y, panel.size.w);
    let title = Rectangle::new(panel.loc, (w, INBOX_TITLE).into());
    let scope = if view.all.is_some() {
        let y = top + (INBOX_TITLE - ROW_BUTTON) / 2;
        let all_x = x + w - SCOPE_INSET - SCOPE_ALL;
        let own_x = all_x - BUTTON_GAP - SCOPE_OWN;
        vec![
            (
                false,
                Rectangle::new((own_x, y).into(), (SCOPE_OWN, ROW_BUTTON).into()),
            ),
            (
                true,
                Rectangle::new((all_x, y).into(), (SCOPE_ALL, ROW_BUTTON).into()),
            ),
        ]
    } else {
        Vec::new()
    };
    let query = Rectangle::new((x, top + INBOX_TITLE).into(), (w, INBOX_FIELD).into());
    let hints = hint_strs(&view.hints);
    let footer_h = footer_height_for(&hints, w);
    let footer = Rectangle::new(
        (x, top + panel.size.h - footer_h).into(),
        (w, footer_h).into(),
    );
    let (list, detail) = body(panel, footer_h);

    let (row_x, row_w) = row_span(list);
    let mut y = list.loc.y + INBOX_PAD;
    let list_end = list.loc.y + list.size.h - INBOX_PAD;
    let mut full = false;
    let with_buttons = match view.hover {
        Some(InboxHit::Row(i) | InboxHit::Button(i, _)) => Some(i),
        _ => view.selected,
    };
    let rows = view
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let h = match row {
                InboxRow::Label { .. } => INBOX_LABEL,
                InboxRow::Item(_) => INBOX_ROW,
            };
            // More rows than fit (more than a page) take no room.
            full |= y + h > list_end;
            let h = if full { 0 } else { h };
            let rect = Rectangle::new((row_x, y).into(), (row_w, h).into());
            y += h;
            let buttons = match row {
                _ if h == 0 => Vec::new(),
                InboxRow::Item(line) if with_buttons == Some(i) => row_buttons(&line.buttons, rect),
                _ => Vec::new(),
            };
            (rect, buttons)
        })
        .collect();
    let detail_buttons = view
        .detail
        .as_ref()
        .map(|d| detail_parts(d, detail).buttons)
        .unwrap_or_default();
    InboxLayout {
        panel,
        title,
        scope,
        query,
        list,
        rows,
        detail,
        detail_buttons,
        footer,
        hints: footer_hint_spots(&hints, footer),
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
    let on = |r: &Rect| r.to_f64().contains(pos);
    if let Some(i) = hint_at(&hint_strs(&view.hints), &layout.hints, pos) {
        return Some(InboxHit::Hint(i));
    }
    if let Some((all, _)) = layout.scope.iter().find(|(_, r)| on(r)) {
        return Some(InboxHit::Scope(*all));
    }
    if on(&layout.query) {
        return Some(InboxHit::Query);
    }
    for (i, (rect, buttons)) in layout.rows.iter().enumerate() {
        if let Some(k) = buttons.iter().position(on) {
            return Some(InboxHit::Button(i, k));
        }
        if on(rect) {
            return Some(InboxHit::Row(i));
        }
    }
    if let Some(k) = layout.detail_buttons.iter().position(on) {
        return Some(InboxHit::DetailButton(k));
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
        text: field_text(view, area, scale),
        line,
    }
}

/// The colour of an item's dot of `tone` in `p`.
fn tone_color(tone: InboxTone, p: &Palette) -> Rgb {
    match tone {
        InboxTone::Blocked | InboxTone::Attention => p.warning,
        InboxTone::Done => p.success,
        InboxTone::Working => p.accent,
        InboxTone::Failed => p.error,
        // ANSI magenta: apart from the rest, readable in both looks.
        InboxTone::Review => p.ansi[5],
        InboxTone::Info => p.muted,
    }
}

/// Signed distance to a right arrow centred on the origin: the compose
/// chip's link to a workspace.
fn link_arrow(x: f32, y: f32, s: f32) -> f32 {
    let shaft = segment(x, y, -5.0 * s, 0.0, 5.0 * s, 0.0);
    let up = segment(x, y, 5.0 * s, 0.0, 1.5 * s, -3.5 * s);
    let down = segment(x, y, 5.0 * s, 0.0, 1.5 * s, 3.5 * s);
    shaft.min(up).min(down) - STROKE * s
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
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let divider = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };

        let scope: Vec<_> = layout.scope.iter().map(|(a, r)| (*a, shift(*r))).collect();
        self.inbox_title(&mut c, view, shift(layout.title), &scope, scale, p);
        let query = shift(layout.query);
        self.inbox_query(&mut c, view, query, scale, p);
        divider(&mut c, px(query.loc.y + query.size.h) - border);

        let list = shift(layout.list);
        if view.rows.is_empty() {
            self.centered(&mut c, &view.empty, FONT_SIZE, list, scale, p.placeholder);
        }
        for (i, ((rect, buttons), row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            if rect.size.h == 0 {
                continue;
            }
            let buttons: Vec<_> = buttons.iter().map(|r| shift(*r)).collect();
            self.inbox_row(&mut c, view, i, row, shift(*rect), &buttons, scale, p);
        }
        let shown = view.rows.len();
        scrollbar(
            &mut c,
            scroll_track(list),
            (view.first, shown, view.total.max(shown)),
            scale,
            p,
        );
        // The line between the list and the detail.
        c.fill(
            px(list.loc.x + list.size.w) - border,
            px(list.loc.y),
            border,
            px(list.size.h),
            p.border_variant,
        );

        let detail = shift(layout.detail);
        match &view.detail {
            Some(d) => self.inbox_detail(&mut c, view, d, detail, scale, p),
            None if !view.rows.is_empty() => {
                self.centered(
                    &mut c,
                    "Nothing selected",
                    FONT_SIZE,
                    detail,
                    scale,
                    p.placeholder,
                );
            }
            None => {}
        }

        let footer = shift(layout.footer);
        divider(&mut c, px(footer.loc.y));
        let spots: Vec<_> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(InboxHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hint_strs(&view.hints), &spots, hover, scale, p);
        c.into_buffer(false)
    }

    /// Draw `text` as `text` does, but the runs of characters the UI font
    /// lacks (`→`, `↑`) in the monospace font; cut off with "…" before
    /// `max_x`. Returns where it ends.
    #[allow(clippy::too_many_arguments)]
    fn any_text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let lacks = |ch: char| ch != ' ' && !self.has_glyph(ch);
        if !text.chars().any(lacks) {
            return self.text(c, text, size, x, max_x, band, scale, color);
        }
        let mut pen = x;
        let mut rest = text;
        while let Some(first) = rest.chars().next() {
            let mono = lacks(first);
            let end = rest
                .char_indices()
                .find(|&(_, ch)| lacks(ch) != mono)
                .map_or(rest.len(), |(i, _)| i);
            let (run, after) = rest.split_at(end);
            let width = if mono {
                self.mono_width(run, size, scale)
            } else {
                self.text_width(run, size, scale)
            };
            // The last run, or one running past the end: what fits of it,
            // with "…", from the UI font.
            let ellipsis = self.text_width("…", size, scale);
            if !after.is_empty() && pen + width + ellipsis > max_x || pen + width > max_x {
                if mono {
                    return self.text(c, "…", size, pen, max_x, band, scale, color);
                }
                // Cut short even where it would just fit, for the "…".
                let limit = if after.is_empty() {
                    max_x
                } else {
                    max_x.min(pen + width - 0.5)
                };
                return self.text(c, run, size, pen, limit, band, scale, color);
            }
            pen = if mono {
                self.mono_text(c, run, size, pen, f32::MAX, band, scale, color)
            } else {
                self.text(c, run, size, pen, f32::MAX, band, scale, color)
            };
            rest = after;
        }
        pen
    }

    /// A dot of `color` `DOT` across, centred on (`cx`, `cy`) (logical,
    /// buffer coordinates).
    fn inbox_dot(&self, c: &mut Canvas, cx: i32, cy: i32, scale: f64, color: Rgb) {
        let dot = Rectangle::new((cx - DOT / 2, cy - DOT / 2).into(), (DOT, DOT).into());
        let (x, y, w, h) = rect_px(dot, scale);
        c.rounded(x, y, w, h, w / 2.0, color);
    }

    /// The title row in `title`: "Inbox", the project's name, a tag while
    /// hidden items are listed, and the scope toggle's segments.
    fn inbox_title(
        &self,
        c: &mut Canvas,
        view: &InboxView,
        title: Rect,
        scope: &[(bool, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as f32;
        let s = scale as f32;
        let gap = (FIELD_GAP as f32 * s).round();
        let (x, mut right) = phys(title, scale);
        let x = x + ((PROJECT_MENU_PADDING as f32 + PADDING) * s).round();
        if let Some((_, first)) = scope.first() {
            right = px(first.loc.x) - gap;
        }
        let title_band = band(title, scale);
        let mut after = self.text(
            c,
            "Inbox",
            LABEL_FONT_SIZE,
            x,
            right,
            title_band,
            scale,
            p.text,
        );
        if let Some(project) = &view.project {
            after = self.any_text(
                c,
                project,
                FONT_SIZE,
                after + 1.5 * gap,
                right,
                title_band,
                scale,
                p.placeholder,
            );
        }
        if view.show_hidden {
            self.text(
                c,
                "· hidden items",
                SMALL_FONT_SIZE,
                after + gap,
                right,
                title_band,
                scale,
                p.warning,
            );
        }
        for &(all, rect) in scope {
            self.inbox_scope(c, view, all, rect, scale, p);
        }
    }

    /// The scope toggle's segment for every workspace (`all`) or the
    /// active one's, in `rect`: an outlined pill, filled when it is the
    /// scope shown, lighter under the pointer.
    fn inbox_scope(
        &self,
        c: &mut Canvas,
        view: &InboxView,
        all: bool,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let on = view.all == Some(all);
        let fill = if on {
            p.selected
        } else if view.hover == Some(InboxHit::Scope(all)) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        self.inbox_pill(c, rect, fill, scale, p);
        let label = if all { "All" } else { "This workspace" };
        let color = if on { p.text } else { p.muted };
        self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
    }

    /// An outlined pill in `rect`, filled with `fill`.
    fn inbox_pill(&self, c: &mut Canvas, rect: Rect, fill: Rgb, scale: f64, p: &Palette) {
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * scale as f32;
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
    }

    /// The field in `query`: a magnifier before the filter, or a plus (a
    /// pencil when editing) before the todo being composed, with the chip
    /// saying which workspace it is linked to at its end.
    fn inbox_query(&self, c: &mut Canvas, view: &InboxView, query: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let mark = Rectangle::new(
            (
                query.loc.x + PROJECT_MENU_PADDING + PADDING as i32,
                query.loc.y,
            )
                .into(),
            (MENU_MARK_WIDTH, query.size.h).into(),
        );
        let line = match &view.compose {
            Some(compose) if compose.editing => {
                icon_sdf(c, mark, scale, p.accent, |x, y| pencil(x, y, s));
                &compose.text
            }
            Some(compose) => {
                icon_sdf(c, mark, scale, p.accent, |x, y| {
                    plus(x, y, 0.0, 0.0, 5.5 * s, s)
                });
                &compose.text
            }
            None => {
                icon_sdf(c, mark, scale, p.placeholder, |x, y| magnifier(x, y, s));
                &view.query
            }
        };
        self.edit_line(
            c,
            line,
            &view.placeholder,
            field_text(view, query, scale),
            band(query, scale),
            true,
            FONT_SIZE,
            scale,
            p,
        );
        let Some(compose) = &view.compose else {
            return;
        };
        let chip = compose_chip(compose, query);
        self.inbox_pill(c, chip, p.element, scale, p);
        let (x, right) = phys(chip, scale);
        let mut pen = x + (CHIP_PAD as f32 * s).round();
        let color = match compose.workspace {
            Some(_) => {
                let arrow = Rectangle::new(
                    (chip.loc.x + CHIP_PAD, chip.loc.y).into(),
                    (CHIP_ARROW, chip.size.h).into(),
                );
                icon_sdf(c, arrow, scale, p.accent, |x, y| link_arrow(x, y, s));
                pen += ((CHIP_ARROW + BUTTON_GAP) as f32 * s).round();
                p.text
            }
            None => p.placeholder,
        };
        let end = right - (CHIP_PAD as f32 * s).round() + s;
        self.any_text(
            c,
            chip_label(compose),
            SMALL_FONT_SIZE,
            pen,
            end,
            band(chip, scale),
            scale,
            color,
        );
    }

    /// Row `i` of the list in `rect` (buffer coordinates), with its
    /// `buttons`: a section's label, or an item's dot, title and age over
    /// its subtitle, on the selection's ground when selected and a lighter
    /// one under the pointer; a hidden item's muted.
    #[allow(clippy::too_many_arguments)]
    fn inbox_row(
        &self,
        c: &mut Canvas,
        view: &InboxView,
        i: usize,
        row: &InboxRow,
        rect: Rect,
        buttons: &[Rect],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let gap = (FIELD_GAP as f32 * s).round();
        let mark = menu_mark(rect);
        let (x, y, w, h) = rect_px(rect, scale);
        let right = x + w - PADDING * s;
        let line = match row {
            InboxRow::Label {
                title,
                count,
                hidden,
            } => {
                let label_band = band(rect, scale);
                let after = self.text(
                    c,
                    &title.to_uppercase(),
                    DETAIL_FONT_SIZE,
                    px(mark.loc.x) as f32,
                    right,
                    label_band,
                    scale,
                    p.placeholder,
                );
                let after = self.text(
                    c,
                    &count.to_string(),
                    DETAIL_FONT_SIZE,
                    after + gap * 0.75,
                    right,
                    label_band,
                    scale,
                    p.muted,
                );
                if *hidden > 0 {
                    self.text(
                        c,
                        &format!("· {hidden} hidden"),
                        DETAIL_FONT_SIZE,
                        after + gap * 0.75,
                        right,
                        label_band,
                        scale,
                        p.placeholder,
                    );
                }
                return;
            }
            InboxRow::Item(line) => line,
        };

        // The highlight stops short of the rows above and below.
        let (hy, hh) = (y + px(2) as f32, h - 2.0 * px(2) as f32);
        let r = (MENU_RADIUS - 2.0) * s;
        let hovered = matches!(
            view.hover,
            Some(InboxHit::Row(j) | InboxHit::Button(j, _)) if j == i
        );
        if view.selected == Some(i) {
            c.rounded(x, hy, w, hh, r, p.selected);
        } else if hovered {
            c.rounded(x, hy, w, hh, r, mix(p.elevated, p.selected, 0.5));
        }

        // The title's line, and the subtitle's under it; without a
        // subtitle, the title across the row.
        let (title_rect, subtitle_rect) = if line.subtitle.is_empty() {
            (rect, None)
        } else {
            let t = Rectangle::new(
                (rect.loc.x, rect.loc.y + ROW_TITLE_TOP).into(),
                (rect.size.w, ROW_TITLE).into(),
            );
            let st = Rectangle::new(
                (rect.loc.x, rect.loc.y + ROW_TITLE_TOP + ROW_TITLE).into(),
                (rect.size.w, ROW_SUBTITLE).into(),
            );
            (t, Some(st))
        };
        let tone = tone_color(line.tone, p);
        let dot = if line.hidden {
            mix(p.elevated, tone, 0.45)
        } else {
            tone
        };
        self.inbox_dot(
            c,
            mark.loc.x + MENU_MARK_WIDTH / 2,
            title_rect.loc.y + title_rect.size.h / 2,
            scale,
            dot,
        );

        for (k, &button) in buttons.iter().enumerate() {
            let label = line.buttons.get(k).map_or("", |b| b.label.as_str());
            let hot = view.hover == Some(InboxHit::Button(i, k));
            let fill = if hot { p.selected } else { p.element };
            self.inbox_pill(c, button, fill, scale, p);
            self.centered(c, label, SMALL_FONT_SIZE, button, scale, p.text);
        }
        let end = match buttons.first() {
            Some(first) => px(first.loc.x) as f32 - gap,
            None => right,
        };
        let (title_color, subtitle_color) = if line.hidden {
            (p.placeholder, p.placeholder)
        } else {
            (p.text, p.muted)
        };
        let title_band = band(title_rect, scale);
        let mut title_end = end;
        if !line.age.is_empty() {
            let age_w = self.text_width(&line.age, SMALL_FONT_SIZE, scale);
            let age_x = end - age_w;
            let text_x = px(mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP) as f32;
            // Only where it leaves the title some room.
            if age_x > text_x + 4.0 * gap {
                self.text(
                    c,
                    &line.age,
                    SMALL_FONT_SIZE,
                    age_x,
                    end + s,
                    title_band,
                    scale,
                    p.placeholder,
                );
                title_end = age_x - gap;
            }
        }
        let text_x = px(mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP) as f32;
        self.any_text(
            c,
            &line.title,
            FONT_SIZE,
            text_x,
            title_end,
            title_band,
            scale,
            title_color,
        );
        if let Some(st) = subtitle_rect {
            self.any_text(
                c,
                &line.subtitle,
                SMALL_FONT_SIZE,
                text_x,
                end,
                band(st, scale),
                scale,
                subtitle_color,
            );
        }
    }

    /// The selected item's `detail` in `rect` (buffer coordinates): its
    /// dot and title, its fields as label and value, the heading over its
    /// block of monospace lines (cut short with a line saying how many
    /// more there are, or at its start with `block_tail`), and its buttons
    /// along the bottom.
    #[allow(clippy::too_many_arguments)]
    fn inbox_detail(
        &self,
        c: &mut Canvas,
        view: &InboxView,
        detail: &InboxDetailView,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let gap = (FIELD_GAP as f32 * s).round();
        let border = px(1).max(1);
        let parts = detail_parts(detail, rect);

        let title = parts.title;
        let (tx, tright) = phys(title, scale);
        let dot_x = rect.loc.x + HELP_PADDING + DOT / 2;
        self.inbox_dot(
            c,
            dot_x,
            title.loc.y + title.size.h / 2,
            scale,
            tone_color(detail.tone, p),
        );
        self.any_text(
            c,
            &detail.title,
            LABEL_FONT_SIZE,
            tx + px(HELP_PADDING + DOT) as f32 + gap,
            tright - px(HELP_PADDING) as f32,
            band(title, scale),
            scale,
            p.text,
        );
        c.fill(
            px(rect.loc.x) + border,
            px(title.loc.y + title.size.h) - border,
            px(rect.loc.x + rect.size.w) - px(rect.loc.x) - 2 * border,
            border,
            p.border_variant,
        );

        for (field, (label, value)) in parts.fields.iter().zip(&detail.fields) {
            let (fx, fright) = phys(*field, scale);
            let field_band = band(*field, scale);
            let value_x = fx + px(parts.label_w) as f32;
            self.text(
                c,
                label,
                SMALL_FONT_SIZE,
                fx,
                value_x - gap,
                field_band,
                scale,
                p.placeholder,
            );
            self.any_text(
                c, value, FONT_SIZE, value_x, fright, field_band, scale, p.text,
            );
        }

        if let (Some(heading), Some(text)) = (parts.heading, &detail.block_title) {
            let (hx, hright) = phys(heading, scale);
            self.text(
                c,
                &text.to_uppercase(),
                DETAIL_FONT_SIZE,
                hx,
                hright,
                band(heading, scale),
                scale,
                p.placeholder,
            );
        }
        if let Some(block) = parts.block {
            let (bx, by, bw, bh) = rect_px(block, scale);
            c.rounded(bx, by, bw, bh, MENU_RADIUS * s, p.tab_active);
            let fit = ((block.size.h - 2 * BLOCK_PAD) / BLOCK_LINE).max(0) as usize;
            let cut = detail.block.len() > fit && fit >= 2;
            let shown = if cut { fit.saturating_sub(1) } else { fit };
            let (lx, lright) = (
                bx + px(BLOCK_PAD + 2) as f32,
                bx + bw - px(BLOCK_PAD + 2) as f32,
            );
            let line_at = |n: usize| {
                Rectangle::new(
                    (block.loc.x, block.loc.y + BLOCK_PAD + n as i32 * BLOCK_LINE).into(),
                    (block.size.w, BLOCK_LINE).into(),
                )
            };
            let more = detail.block.len().saturating_sub(shown);
            // With the tail kept, the line saying what is cut goes first.
            let (lines, top, cut_at) = if detail.block_tail {
                (&detail.block[more..], usize::from(cut), 0)
            } else {
                (&detail.block[..shown.min(detail.block.len())], 0, shown)
            };
            for (n, text) in lines.iter().enumerate() {
                let text = text.replace('\t', "    ");
                self.mono_text(
                    c,
                    &text,
                    BLOCK_FONT_SIZE,
                    lx,
                    lright,
                    band(line_at(top + n), scale),
                    scale,
                    p.text,
                );
            }
            if cut {
                let what = if detail.block_tail { "earlier" } else { "more" };
                self.text(
                    c,
                    &format!("… {more} {what}"),
                    SMALL_FONT_SIZE,
                    lx,
                    lright,
                    band(line_at(cut_at), scale),
                    scale,
                    p.placeholder,
                );
            }
        }

        for (k, (&button, b)) in parts.buttons.iter().zip(&detail.buttons).enumerate() {
            let hot = view.hover == Some(InboxHit::DetailButton(k));
            // The first is what Enter does: in the accent.
            let primary = k == 0;
            let (fill, label_color, key_color) = match (primary, hot) {
                (true, false) => (p.accent, p.on_accent, mix(p.accent, p.on_accent, 0.8)),
                (true, true) => (
                    mix(p.accent, p.text, 0.12),
                    p.on_accent,
                    mix(p.accent, p.on_accent, 0.8),
                ),
                (false, hot) => (
                    if hot { p.selected } else { p.element },
                    p.text,
                    p.placeholder,
                ),
            };
            if primary {
                let (x, y, w, h) = rect_px(button, scale);
                c.rounded(x, y, w, h, BUTTON_RADIUS * s, fill);
            } else {
                self.inbox_pill(c, button, fill, scale, p);
            }
            let (x, right) = phys(button, scale);
            let button_band = band(button, scale);
            let pad = px(BUTTON_PAD) as f32;
            let after = self.text(
                c,
                &b.label,
                SMALL_FONT_SIZE,
                x + pad,
                right - pad + s,
                button_band,
                scale,
                label_color,
            );
            if parts.keys && !b.key.is_empty() {
                self.text(
                    c,
                    &b.key,
                    KEY_FONT_SIZE,
                    after + px(MENU_MARK_GAP) as f32,
                    right - pad + s,
                    button_band,
                    scale,
                    key_color,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{Theme, inside};

    fn button(label: &str, key: &str) -> InboxButton {
        InboxButton {
            label: label.into(),
            key: key.into(),
        }
    }

    fn item(id: &str, title: &str, subtitle: &str, tone: InboxTone, age: &str) -> InboxRow {
        InboxRow::Item(InboxLine {
            id: id.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            tone,
            age: age.into(),
            hidden: false,
            buttons: vec![
                button("Go to", "Enter"),
                button("Snooze", "Ctrl+S"),
                button("Dismiss", "Delete"),
            ],
        })
    }

    fn label(title: &str, count: usize, hidden: usize) -> InboxRow {
        InboxRow::Label {
            title: title.into(),
            count,
            hidden,
        }
    }

    fn detail_buttons() -> Vec<InboxButton> {
        vec![
            button("Go to", "Enter"),
            button("Snooze 1h", "Ctrl+S"),
            button("Until tomorrow", "Ctrl+Shift+S"),
            button("Dismiss", "Delete"),
        ]
    }

    fn agent_detail() -> InboxDetailView {
        let mut block: Vec<String> = (1..=17)
            .map(|n| format!("  {n:>3} │ let value = compute(input, {n});"))
            .collect();
        block.push("Do you want to make this edit to src/chrome/inbox.rs?".into());
        block.push("❯ 1. Yes".into());
        block.push("  2. Yes, and don't ask again this session (shift+tab)".into());
        block.push("  3. No, and tell Claude what to do differently (esc)".into());
        InboxDetailView {
            title: "claude · reviewer".into(),
            tone: InboxTone::Blocked,
            fields: vec![
                ("Status".into(), "Blocked, waiting for input".into()),
                ("Model".into(), "claude-opus-5-5".into()),
                ("Workspace".into(), "backend".into()),
                ("Waiting".into(), "12m".into()),
            ],
            block_title: Some("Last output".into()),
            block,
            block_tail: true,
            buttons: detail_buttons(),
        }
    }

    fn git_detail() -> InboxDetailView {
        InboxDetailView {
            title: "backend".into(),
            tone: InboxTone::Attention,
            fields: vec![
                ("Branch".into(), "feat/inbox → origin/feat/inbox".into()),
                ("Ahead".into(), "2 ahead, 1 behind".into()),
                ("Stashes".into(), "1".into()),
                ("In progress".into(), "merge".into()),
                ("Worktrees".into(), ".claude/worktrees/topic (dirty)".into()),
            ],
            block_title: Some("Changes".into()),
            block: vec![
                "M  src/inbox.rs".into(),
                "M  src/chrome/inbox.rs".into(),
                "A  src/inbox_store.rs".into(),
                "?? notes/a-very-long-path/that/goes/on/and/on/and/on/for/a/while/longer/than/the/pane/is/wide.md".into(),
                "UU src/main.rs".into(),
            ],
            block_tail: false,
            buttons: detail_buttons(),
        }
    }

    fn ci_detail() -> InboxDetailView {
        InboxDetailView {
            title: "Show every repository's changes as one diff".into(),
            tone: InboxTone::Failed,
            fields: vec![
                ("Pull request".into(), "#142".into()),
                ("Repository".into(), "lgal/gerdr".into()),
                ("State".into(), "Failed".into()),
            ],
            block_title: Some("Checks".into()),
            block: vec![
                "failed  clippy".into(),
                "failed  test (ubuntu)".into(),
                "passed  build".into(),
            ],
            block_tail: false,
            buttons: vec![
                button("Open", "Enter"),
                button("Copy link", "Ctrl+C"),
                button("Snooze", "Ctrl+S"),
                button("Dismiss", "Delete"),
            ],
        }
    }

    fn issue_detail() -> InboxDetailView {
        InboxDetailView {
            title: "Inbox: Linear issues assigned to me".into(),
            tone: InboxTone::Info,
            fields: vec![
                ("Source".into(), "Linear".into()),
                ("Key".into(), "ENG-1234".into()),
                ("State".into(), "In Progress".into()),
                ("Labels".into(), "feature, ui".into()),
                ("Updated".into(), "3h ago".into()),
                (
                    "Link".into(),
                    "https://linear.app/acme/issue/ENG-1234/inbox-linear-issues-assigned-to-me"
                        .into(),
                ),
            ],
            block_title: None,
            block: Vec::new(),
            block_tail: false,
            buttons: vec![
                button("Open", "Enter"),
                button("Copy link", "Ctrl+C"),
                button("Snooze", "Ctrl+S"),
                button("Dismiss", "Delete"),
            ],
        }
    }

    fn todo_detail() -> InboxDetailView {
        InboxDetailView {
            title: "Write the release notes for the inbox".into(),
            tone: InboxTone::Info,
            fields: vec![
                ("Workspace".into(), "backend".into()),
                ("Created".into(), "2d ago".into()),
            ],
            block_title: None,
            block: Vec::new(),
            block_tail: false,
            buttons: vec![
                button("Go to", "Enter"),
                button("Edit", "Ctrl+E"),
                button("Done", "Delete"),
                button("Delete", "Shift+Delete"),
            ],
        }
    }

    fn rows() -> Vec<InboxRow> {
        vec![
            label("Agents", 2, 1),
            item(
                "agent:p1",
                "claude · reviewer",
                "backend · blocked",
                InboxTone::Blocked,
                "12m",
            ),
            item(
                "agent:p2",
                "codex",
                "frontend · working",
                InboxTone::Working,
                "3m",
            ),
            label("Git", 1, 0),
            item(
                "git:/src/backend",
                "backend",
                "feat/inbox · 2↑ 1↓ · merge in progress",
                InboxTone::Attention,
                "",
            ),
            label("GitHub", 2, 0),
            item(
                "ci:github.com/lgal/gerdr:pr/142",
                "Show every repository's changes as one diff in the repositories panel",
                "#142 · CI failed",
                InboxTone::Failed,
                "1h",
            ),
            item(
                "ci:github.com/lgal/gerdr:pr/140",
                "Make the help panel's rows clickable",
                "#140 · review requested",
                InboxTone::Review,
                "5h",
            ),
            label("Todos", 2, 0),
            item(
                "linear:abc",
                "Inbox: Linear issues assigned to me",
                "ENG-1234 · In Progress",
                InboxTone::Info,
                "3h",
            ),
            item(
                "todo:t1",
                "Write the release notes for the inbox",
                "backend",
                InboxTone::Info,
                "2d",
            ),
        ]
    }

    fn hints() -> Vec<(String, String)> {
        [
            ("Enter", "go to"),
            ("Delete", "dismiss"),
            ("Ctrl+S", "snooze"),
            ("Ctrl+N", "new todo"),
            ("Ctrl+H", "hidden"),
            ("Up / Down", "select"),
            ("Esc", "close"),
        ]
        .into_iter()
        .map(|(k, a)| (k.into(), a.into()))
        .collect()
    }

    fn view() -> InboxView {
        let rows = rows();
        InboxView {
            project: Some("gerdr".into()),
            all: Some(true),
            query: Line::default(),
            placeholder: "Filter the inbox…".into(),
            compose: None,
            total: rows.len() + 6,
            first: 0,
            rows,
            selected: Some(1),
            hover: None,
            detail: Some(agent_detail()),
            hints: hints(),
            empty: "Nothing needs you".into(),
            show_hidden: false,
        }
    }

    fn with_detail(selected: usize, detail: InboxDetailView) -> InboxView {
        InboxView {
            selected: Some(selected),
            detail: Some(detail),
            ..view()
        }
    }

    fn compose(workspace: Option<&str>, editing: bool) -> InboxView {
        let text = "Ask about the deploy window";
        InboxView {
            compose: Some(InboxCompose {
                text: Line::from(text),
                workspace: workspace.map(str::to_owned),
                editing,
            }),
            placeholder: "New todo…".into(),
            ..with_detail(10, todo_detail())
        }
    }

    fn hidden() -> InboxView {
        let rows = vec![
            label("Agents", 0, 1),
            InboxRow::Item(InboxLine {
                hidden: true,
                subtitle: "frontend · dismissed".into(),
                buttons: vec![button("Restore", "Delete"), button("Go to", "Enter")],
                ..match item("agent:p3", "aider", "", InboxTone::Done, "1d") {
                    InboxRow::Item(line) => line,
                    _ => unreachable!(),
                }
            }),
            label("GitHub", 0, 1),
            InboxRow::Item(InboxLine {
                hidden: true,
                subtitle: "#139 · snoozed until 09:00".into(),
                buttons: vec![button("Restore", "Delete")],
                ..match item("ci:x", "Fix the CI cache", "", InboxTone::Failed, "2d") {
                    InboxRow::Item(line) => line,
                    _ => unreachable!(),
                }
            }),
        ];
        InboxView {
            total: rows.len(),
            rows,
            show_hidden: true,
            selected: Some(1),
            detail: Some(InboxDetailView {
                title: "aider".into(),
                tone: InboxTone::Done,
                fields: vec![
                    ("Status".into(), "Done".into()),
                    ("Hidden".into(), "dismissed 1d ago".into()),
                ],
                block_title: Some("Last output".into()),
                block: vec!["All tests pass.".into()],
                block_tail: true,
                buttons: vec![button("Restore", "Delete"), button("Go to", "Enter")],
            }),
            ..view()
        }
    }

    fn empty() -> InboxView {
        InboxView {
            rows: Vec::new(),
            total: 0,
            selected: None,
            detail: None,
            all: None,
            ..view()
        }
    }

    fn views() -> Vec<(&'static str, InboxView)> {
        vec![
            ("agent", view()),
            ("git", with_detail(4, git_detail())),
            ("ci", with_detail(6, ci_detail())),
            ("issue", with_detail(9, issue_detail())),
            ("todo", with_detail(10, todo_detail())),
            ("compose", compose(Some("backend"), false)),
            ("compose_none", compose(None, true)),
            ("hidden", hidden()),
            ("empty", empty()),
            (
                "hover",
                InboxView {
                    hover: Some(InboxHit::Button(2, 1)),
                    ..view()
                },
            ),
            (
                "hover_detail",
                InboxView {
                    hover: Some(InboxHit::DetailButton(1)),
                    ..view()
                },
            ),
        ]
    }

    fn centre(r: Rect) -> Point<f64, Logical> {
        (
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        )
            .into()
    }

    fn apart(a: Rect, b: Rect) -> bool {
        a.loc.x + a.size.w <= b.loc.x
            || b.loc.x + b.size.w <= a.loc.x
            || a.loc.y + a.size.h <= b.loc.y
            || b.loc.y + b.size.h <= a.loc.y
    }

    const SIZES: [(i32, i32); 5] = [
        (1280, 800),
        (1920, 1080),
        (3840, 2160),
        (900, 600),
        (700, 500),
    ];

    #[test]
    fn every_part_is_inside_the_panel_and_apart() {
        for (w, h) in SIZES {
            let size = Size::from((w, h));
            for (name, v) in views() {
                let l = inbox_layout(&v, size);
                let what = format!("{name} at {w}×{h}");
                assert!(inside(Rectangle::from_size(size), l.panel), "{what}");
                assert!(l.panel.size.w <= INBOX_MAX_WIDTH, "{what}");
                for r in [l.title, l.query, l.list, l.detail, l.footer] {
                    assert!(inside(l.panel, r), "{what}: {r:?}");
                }
                assert!(apart(l.list, l.detail), "{what}");
                assert!(l.list.size.w >= INBOX_LIST_MIN.min(l.panel.size.w / 2));
                assert!(apart(l.query, l.list) && apart(l.list, l.footer), "{what}");
                assert!(
                    apart(l.title, l.query) && apart(l.detail, l.footer),
                    "{what}"
                );
                for (_, r) in &l.scope {
                    assert!(inside(l.title, *r), "{what}");
                }
                for (rect, buttons) in &l.rows {
                    assert!(inside(l.list, *rect), "{what}: {rect:?}");
                    for b in buttons {
                        assert!(inside(*rect, *b), "{what}");
                    }
                    assert!(buttons.windows(2).all(|b| apart(b[0], b[1])), "{what}");
                }
                assert!(l.rows.windows(2).all(|r| apart(r[0].0, r[1].0)), "{what}");
                for b in &l.detail_buttons {
                    assert!(inside(l.detail, *b), "{what}: {b:?}");
                }
                assert!(
                    l.detail_buttons.windows(2).all(|b| apart(b[0], b[1])),
                    "{what}"
                );
                for (_, r) in &l.hints {
                    assert!(inside(l.footer, *r), "{what}");
                }
                // The detail's own parts stay in it, above its buttons.
                if let Some(d) = &v.detail {
                    let parts = detail_parts(d, l.detail);
                    let tops = parts.buttons.iter().map(|b| b.loc.y).min();
                    let above = |r: Rect| tops.is_none_or(|t| r.loc.y + r.size.h <= t);
                    let mut all = parts.fields.clone();
                    all.extend(parts.heading);
                    all.extend(parts.block);
                    for r in all {
                        assert!(inside(l.detail, r) && above(r), "{what}: {r:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_page_of_rows_fits_the_list() {
        for (w, h) in SIZES {
            let size = Size::from((w, h));
            let page = inbox_page(size);
            let rows: Vec<_> = (0..page)
                .map(|n| item(&n.to_string(), "t", "s", InboxTone::Info, ""))
                .collect();
            let v = InboxView { rows, ..view() };
            let l = inbox_layout(&v, size);
            assert_eq!(l.rows.len(), page);
            let last = l.rows.last().map(|(r, _)| *r).unwrap();
            assert!(inside(l.list, last), "{w}×{h}");
            // And one more would not have.
            assert!(
                last.loc.y + last.size.h + INBOX_ROW > l.list.loc.y + l.list.size.h - INBOX_PAD
                    || l.footer.size.h < footer_height(usize::MAX),
                "{w}×{h}"
            );
        }
        assert!(inbox_page((1280, 800).into()) >= 10);
        assert_eq!(inbox_page((300, 120).into()), 1);
    }

    #[test]
    fn hits_land_on_what_is_under_them() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let v = view();
        let l = inbox_layout(&v, size);
        assert_eq!(inbox_hit(&v, &l, (1.0, 1.0).into()), None);
        assert_eq!(inbox_hit(&v, &l, centre(l.title)), Some(InboxHit::Panel));
        assert_eq!(inbox_hit(&v, &l, centre(l.query)), Some(InboxHit::Query));
        // The footer's key hints are buttons, but not one offering keys to
        // choose from.
        assert_eq!(l.hints.len(), v.hints.len());
        for (i, r) in &l.hints {
            let want = (!v.hints[*i].0.contains(" / ")).then_some(InboxHit::Hint(*i));
            let got = inbox_hit(&v, &l, centre(*r));
            assert!(got == want || want.is_none() && got == Some(InboxHit::Panel));
        }
        // The scope toggle's segments.
        assert_eq!(l.scope.len(), 2);
        for (all, r) in &l.scope {
            assert_eq!(inbox_hit(&v, &l, centre(*r)), Some(InboxHit::Scope(*all)));
        }
        assert!(inbox_layout(&empty(), size).scope.is_empty());
        // Rows, by index into the rows in view; the selected one has its
        // buttons, the others none.
        for (i, (r, buttons)) in l.rows.iter().enumerate() {
            let at = (r.loc.x as f64 + 30.0, r.loc.y as f64 + 4.0).into();
            assert_eq!(inbox_hit(&v, &l, at), Some(InboxHit::Row(i)));
            assert_eq!(buttons.is_empty(), i != 1, "row {i}");
        }
        assert_eq!(l.rows[1].1.len(), 3);
        for (k, b) in l.rows[1].1.iter().enumerate() {
            assert_eq!(inbox_hit(&v, &l, centre(*b)), Some(InboxHit::Button(1, k)));
        }
        // Hovering another item moves the buttons there.
        let hovered = InboxView {
            hover: Some(InboxHit::Row(2)),
            ..v.clone()
        };
        let lh = inbox_layout(&hovered, size);
        assert!(lh.rows[1].1.is_empty() && lh.rows[2].1.len() == 3);
        assert!(lh.rows[0].1.is_empty());
        // The detail's buttons.
        assert_eq!(l.detail_buttons.len(), 4);
        for (k, b) in l.detail_buttons.iter().enumerate() {
            assert_eq!(
                inbox_hit(&v, &l, centre(*b)),
                Some(InboxHit::DetailButton(k))
            );
        }
        assert_eq!(inbox_hit(&v, &l, centre(l.detail)), Some(InboxHit::Panel));
        // Below the last row, still in the list: on nothing.
        let (last, _) = l.rows.last().unwrap();
        let below = (centre(*last).x, (last.loc.y + last.size.h + 10) as f64).into();
        assert_eq!(inbox_hit(&v, &l, below), Some(InboxHit::Panel));
    }

    #[test]
    fn a_narrow_row_keeps_room_for_its_title() {
        let size: Size<i32, Logical> = (700, 500).into();
        let v = InboxView {
            rows: vec![InboxRow::Item(InboxLine {
                buttons: (0..8).map(|n| button(&format!("Act {n}"), "")).collect(),
                ..match item("a", "t", "s", InboxTone::Info, "1m") {
                    InboxRow::Item(line) => line,
                    _ => unreachable!(),
                }
            })],
            selected: Some(0),
            ..view()
        };
        let l = inbox_layout(&v, size);
        let (rect, buttons) = &l.rows[0];
        assert!(!buttons.is_empty() && buttons.len() < 8);
        assert!(buttons[0].loc.x >= rect.loc.x + ROW_TEXT_MIN);
    }

    #[test]
    fn the_field_spot_is_the_field() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let v = InboxView {
            query: Line::from("git"),
            ..view()
        };
        let spot = inbox_field(&v, size, 1.0);
        let l = inbox_layout(&v, size);
        assert_eq!(spot.area, l.query);
        assert_eq!(spot.line, Line::from("git"));
        assert_eq!(spot.text, menu_query_text(l.query, 1.0));
        // While composing, the todo's text, ending before the chip.
        let c = compose(Some("backend"), false);
        let spot = inbox_field(&c, size, 2.0);
        assert_eq!(spot.line, Line::from("Ask about the deploy window"));
        let chip = compose_chip(c.compose.as_ref().unwrap(), spot.area);
        assert!(inside(spot.area, chip));
        assert!(spot.text.1 <= (chip.loc.x * 2) as f32);
        assert!(spot.text.0 < spot.text.1);
        assert_eq!(
            inbox_hit(&c, &inbox_layout(&c, size), centre(chip)),
            Some(InboxHit::Query)
        );
    }

    #[test]
    fn it_paints() {
        let painter = Painter::new();
        for (_, v) in views() {
            for (w, h) in [(1280, 800), (700, 500), (200, 100)] {
                let _ = painter.inbox(&v, (w, h).into(), 1.0, Theme::Dark.palette());
            }
            let _ = painter.inbox(&v, (1280, 800).into(), 2.0, Theme::Light.palette());
        }
    }

    /// With `GERDR_DUMP_DIR` set, write the panel there as images to look
    /// at (both themes, scale 1 and 2, and a narrow output).
    #[test]
    fn dump_inbox() {
        let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let painter = Painter::new();
        let size: Size<i32, Logical> = (1280, 800).into();
        let narrow = Size::from((700, 500));
        for (theme, scale) in [
            (Theme::Dark, 1.0),
            (Theme::Light, 1.0),
            (Theme::Dark, 2.0),
            (Theme::Light, 2.0),
        ] {
            let cases = views()
                .into_iter()
                .map(|(name, v)| (name.to_owned(), v, size))
                .chain([
                    ("narrow".to_owned(), view(), narrow),
                    (
                        "narrow_git".to_owned(),
                        with_detail(4, git_detail()),
                        narrow,
                    ),
                ]);
            for (name, view, size) in cases {
                let panel = inbox_layout(&view, size).panel;
                let w = ((panel.size.w + 2 * SHADOW) as f64 * scale).round() as usize;
                let mut buffer = painter.inbox(&view, size, scale, theme.palette());
                let mut out = Vec::new();
                let _ = buffer.render().draw(|mem| {
                    let h = mem.len() / 4 / w;
                    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
                    for px in mem.as_chunks::<4>().0 {
                        let a = px[3] as u32;
                        // Premultiplied over a grey ground.
                        for c in [px[2], px[1], px[0]] {
                            out.push((c as u32 + 0x80 * (255 - a) / 255) as u8);
                        }
                    }
                    Ok::<_, ()>(Vec::new())
                });
                let file = format!("inbox_{name}_{}_{scale}.ppm", theme.name());
                let _ = std::fs::write(dir.join(file), out);
            }
        }
    }
}
