//! The switcher: its rows, where they go, and how it is drawn.
//!
//! Under the query, a row of category chips ("All", then a chip per
//! section listed) narrows the rows to one section. The rows come in
//! sections (Workspaces, Tabs, Agents, …), each under an upper case
//! heading. A row has two lines: what it is on the first, more about it in
//! muted text on the second, with a small pill at the right naming its
//! kind. The key hints go in a footer, as in the project menu.

use super::Rgb;
use super::layers::{Part, RowCache};
use super::{
    BUTTON_RADIUS, CARET_WIDTH, Canvas, FONT_SIZE, FieldSpot, HELP_MARGIN, HELP_PADDING,
    HELP_RADIUS, MENU_RADIUS, PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette, ROW_ICON_GAP,
    ROW_ICON_WIDTH, Rect, SHADOW, SMALL_FONT_SIZE, SWITCHER_INPUT, SWITCHER_PADDING, SWITCHER_ROWS,
    SWITCHER_SECTION, SWITCHER_TOP, SWITCHER_WIDTH, TINY_FONT_SIZE, app, folder, folder_plus,
    footer_height, footer_hint_spots, hint_at, hint_strs, icon_sdf, mix, pencil, prompt, rect_px,
    status_mark, tab, terminal,
};
use crate::WorkspaceKind;
use crate::app_icons::AppIcon;
use crate::herdr::AgentStatus;
use crate::project_list::RowTops;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// A row: two lines of text.
const ROW_HEIGHT: i32 = 40;
/// A section's heading: the gap above it and its text.
const SECTION_HEIGHT: i32 = SWITCHER_SECTION;
/// The heading's text sits in the lower part of its band.
const SECTION_TEXT: i32 = 22;
/// The gap between the two lines' bands and the row's edges.
const LINE_INSET: i32 = 4;
/// The first line's band; the second takes the rest.
const LABEL_LINE: i32 = 18;
/// The pill at the right of a row and the gap before it.
const PILL_GAP: i32 = 10;
/// A category chip's height, and the gap between chips.
const CHIP_HEIGHT: i32 = 24;
const CHIP_GAP: i32 = 4;
/// A chip's room per character of its label, and around the label: an
/// estimate of the text's width, as the layout has no font to measure with.
const CHIP_CHAR: i32 = 7;
const CHIP_INSET: i32 = 12;
/// The row of chips: a chip and the gap under it.
const CHIPS_ROW: i32 = CHIP_HEIGHT + SWITCHER_PADDING;

/// The icon of a switcher row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemIcon {
    Workspace(WorkspaceKind),
    Tab,
    Agent(AgentStatus),
    /// A folder to open.
    Folder,
    /// An app or command to launch.
    App,
    /// An app's own icon.
    Picture(AppIcon),
    /// Something gerdr does itself: the shortcuts, the project menu.
    Command,
    /// A file to edit: the settings.
    Settings,
}

/// One row of the switcher, with the positions (in chars) of the characters
/// the query matched in its label and detail. `section` is set on the first
/// row of each section shown: the heading drawn above it.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherRow {
    pub icon: ItemIcon,
    pub label: String,
    pub detail: String,
    /// What kind of thing it is, for the pill at the right.
    pub kind: &'static str,
    pub section: Option<&'static str>,
    pub label_hits: Vec<usize>,
    pub detail_hits: Vec<usize>,
}

/// What the switcher shows: the query (or `hint` without one), the
/// category chips, the rows in view, which of them is selected (none when
/// nothing matches), which the pointer is over, and the key hints.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherView {
    pub query: Line,
    pub hint: &'static str,
    /// The category chips' labels, "All" first; none without a choice.
    pub categories: Vec<&'static str>,
    /// The chip on, by index into `categories`.
    pub category: usize,
    /// The chip under the pointer.
    pub category_hover: Option<usize>,
    pub rows: Vec<SwitcherRow>,
    pub selected: Option<usize>,
    pub hover: Option<usize>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
    /// The key hint under the pointer, by index into `hints`.
    pub hint_hover: Option<usize>,
    /// How far the rows are moved up from resting with the first one at
    /// the top, in logical pixels: part of the way to the next row while
    /// scrolled, less than none while pulled down past the start, and
    /// more while pulled up past the end. The rows reach down past the
    /// list, and one may be shown in part at its bottom.
    pub part: f64,
}

/// Where the switcher's parts go: the panel, its query field, a rect per
/// category chip, a rect per row shown, the section headings with their
/// text, and the footer.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherLayout {
    pub panel: Rect,
    pub input: Rect,
    pub chips: Vec<Rect>,
    /// A scrolled list's first and last rows, and their headings, reach
    /// out of `list`.
    pub rows: Vec<Rect>,
    /// The headings over the rows starting a section, but the first row.
    pub sections: Vec<(Rect, &'static str)>,
    /// The first row's heading, kept at the top of the list over the rows
    /// scrolled under it until the next heading pushes it out.
    pub kept: Option<(Rect, &'static str)>,
    /// Where the rows and their headings show.
    pub list: Rect,
    pub footer: Rect,
    /// The footer's key hints, by index into `SwitcherView::hints`.
    pub hints: Vec<(usize, Rect)>,
}

/// Where the icon of the switcher row in `row` goes.
pub fn switcher_icon(row: Rect) -> Rect {
    Rectangle::new(
        (row.loc.x + PADDING as i32, row.loc.y).into(),
        (ROW_ICON_WIDTH, row.size.h).into(),
    )
}

/// The switcher showing `view` in the app `area` (window coordinates).
pub fn switcher_geometry(view: &SwitcherView, area: Rect) -> SwitcherLayout {
    switcher_parts(
        view,
        panel_origin(area),
        panel_width(area),
        Some(switcher_room(area)),
    )
}

/// The switcher showing `rows` rows without any section heading, category
/// chip or key hint in the app `area`: the panel, its query field and the
/// rows, for tests with only a count (`switcher_geometry` knows the rest).
#[cfg(test)]
pub fn switcher_layout(rows: usize, area: Rect) -> (Rect, Rect, Vec<Rect>) {
    let layout = parts(
        &vec![None; rows],
        &[],
        0,
        0.0,
        Some(switcher_room(area)),
        panel_origin(area),
        panel_width(area),
    );
    (layout.panel, layout.input, layout.rows)
}

fn panel_width(area: Rect) -> i32 {
    SWITCHER_WIDTH.min(area.size.w - 2 * HELP_MARGIN).max(1)
}

fn panel_origin(area: Rect) -> Point<i32, Logical> {
    (
        area.loc.x + (area.size.w - panel_width(area)) / 2,
        area.loc.y + SWITCHER_TOP,
    )
        .into()
}

/// How high the switcher's list may be in the app `area`: as high as fits
/// above the footer, keeping room for the category chips, up to
/// `SWITCHER_ROWS` rows under two headings; a row under its heading at the
/// least.
pub fn switcher_room(area: Rect) -> i32 {
    let fits = area.size.h
        - SWITCHER_TOP
        - SWITCHER_INPUT
        - 2 * SWITCHER_PADDING
        - HELP_MARGIN
        - CHIPS_ROW
        - footer_height(SWITCHER_HINTS);
    fits.clamp(
        SECTION_HEIGHT + ROW_HEIGHT,
        SWITCHER_ROWS * ROW_HEIGHT + 2 * SECTION_HEIGHT,
    )
}

/// The places of the switcher's rows down its scrolled list, from whether
/// each row starts a section, in order. A row's place is the band over it
/// (its heading, or the first row's kept at the top) and the row, up to
/// where the next row's band would be: a list standing at a row shows it
/// under its band.
pub fn switcher_places(starts: impl IntoIterator<Item = bool>) -> RowTops {
    let starts: Vec<bool> = starts.into_iter().collect();
    RowTops::new((0..starts.len()).map(|i| {
        ROW_HEIGHT
            + match starts.get(i + 1) {
                Some(false) => 0,
                Some(true) | None => SECTION_HEIGHT,
            }
    }))
}

/// How far down the switcher's list of `places` row `i` shows whole: its
/// band and the row.
pub fn switcher_row_span(places: &RowTops, i: usize) -> (f64, f64) {
    let top = places.top(i);
    (top, top + (SECTION_HEIGHT + ROW_HEIGHT) as f64)
}

/// The most key hints the switcher shows: one footer row's worth.
pub const SWITCHER_HINTS: usize = 4;

/// `switcher_geometry` for a panel `w` wide at `loc`, its list `room`
/// high at most (as high as its rows without).
pub(super) fn switcher_parts(
    view: &SwitcherView,
    loc: Point<i32, Logical>,
    w: i32,
    room: Option<i32>,
) -> SwitcherLayout {
    let sections: Vec<_> = view.rows.iter().map(|row| row.section).collect();
    let mut layout = parts(
        &sections,
        &view.categories,
        view.hints.len(),
        view.part,
        room,
        loc,
        w,
    );
    layout.hints = footer_hint_spots(&hint_strs(&view.hints), layout.footer);
    layout
}

/// The row of the switcher laid out as `layout` under `pos`, by index into
/// its rows: none outside the list, where a scrolled list's rows reach, nor
/// under the heading kept over them.
pub fn switcher_row_at(layout: &SwitcherLayout, pos: Point<f64, Logical>) -> Option<usize> {
    let kept = layout
        .kept
        .is_some_and(|(rect, _)| rect.to_f64().contains(pos));
    if kept || !layout.list.to_f64().contains(pos) {
        return None;
    }
    layout
        .rows
        .iter()
        .position(|row| row.to_f64().contains(pos))
}

/// The key hint of the switcher showing `view` under `pos`, by index into
/// its hints, with the switcher's parts where `layout` has them.
pub fn switcher_hint(
    view: &SwitcherView,
    layout: &SwitcherLayout,
    pos: Point<f64, Logical>,
) -> Option<usize> {
    hint_at(&hint_strs(&view.hints), &layout.hints, pos)
}

/// The chips of `labels` in a row from `x` to `right` at `y`: each as
/// wide as its label needs, all narrowed alike when they don't fit.
fn chip_rects(labels: &[&str], x: i32, y: i32, right: i32) -> Vec<Rect> {
    let natural: Vec<i32> = labels
        .iter()
        .map(|label| label.chars().count() as i32 * CHIP_CHAR + 2 * CHIP_INSET)
        .collect();
    let gaps = CHIP_GAP * (labels.len() as i32 - 1).max(0);
    let room = (right - x - gaps).max(0);
    let total: i32 = natural.iter().sum();
    let mut left = x;
    natural
        .into_iter()
        .map(|w| {
            let w = if total > room { w * room / total } else { w };
            let rect = Rectangle::new((left, y).into(), (w, CHIP_HEIGHT).into());
            left += w + CHIP_GAP;
            rect
        })
        .collect()
}

/// Where the rows of `sections` (the heading over each, if any) go down
/// the list, moved up by `part` logical pixels, from the list's top: each
/// row's top, each heading's but the first row's, and the first row's
/// heading kept at the top, pushed up by the next one coming up.
struct Placed {
    rows: Vec<f64>,
    headings: Vec<(f64, &'static str)>,
    kept: Option<(f64, &'static str)>,
    /// How high the rows and their headings are, at rest.
    height: i32,
}

fn place(sections: &[Option<&'static str>], part: f64) -> Placed {
    let band = |i: usize| match sections.get(i) {
        Some(Some(_)) => SECTION_HEIGHT,
        _ => 0,
    };
    let mut y = band(0) as f64 - part;
    let mut rows = Vec::with_capacity(sections.len());
    let mut headings = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        if let (true, Some(label)) = (i > 0, section) {
            headings.push((y, *label));
            y += SECTION_HEIGHT as f64;
        }
        rows.push(y);
        y += ROW_HEIGHT as f64;
    }
    let height = band(0)
        + (1..sections.len()).map(band).sum::<i32>()
        + ROW_HEIGHT * sections.len().max(1) as i32;
    // Pulled down past the start, the kept heading comes down with the rows.
    let top = (-part).max(0.0);
    let kept = sections.first().copied().flatten().map(|label| {
        let mut kept = (top, label);
        for &(y, label) in &headings {
            if y <= top {
                kept = (top, label);
            } else {
                kept.0 = kept.0.min(y - SECTION_HEIGHT as f64);
                break;
            }
        }
        kept
    });
    Placed {
        rows,
        headings,
        kept,
        height,
    }
}

/// The panel `w` wide at `loc`, with the chips of `categories` (none
/// without any), a row per entry of `sections` (the heading over it, if
/// any) and a footer for `hints` key hints; it keeps room for one row
/// without any. The list is as high as its rows at rest, `room` at most;
/// they are moved up by `part` logical pixels.
fn parts(
    sections: &[Option<&'static str>],
    categories: &[&str],
    hints: usize,
    part: f64,
    room: Option<i32>,
    loc: Point<i32, Logical>,
    w: i32,
) -> SwitcherLayout {
    let mut y = loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
    let x = loc.x + SWITCHER_PADDING;
    let row_w = w - 2 * SWITCHER_PADDING;
    let chips = chip_rects(categories, x, y, x + row_w);
    if !chips.is_empty() {
        y += CHIPS_ROW;
    }
    let start = y;
    let placed = place(sections, part);
    let at = |dy: f64| start + dy.round() as i32;
    let rect = |dy: f64, h: i32| Rectangle::new((x, at(dy)).into(), (row_w, h).into());
    let rows = placed.rows.iter().map(|&dy| rect(dy, ROW_HEIGHT)).collect();
    let headings = placed
        .headings
        .iter()
        .map(|&(dy, label)| (rect(dy, SECTION_HEIGHT), label))
        .collect();
    let kept = placed
        .kept
        .map(|(dy, label)| (rect(dy, SECTION_HEIGHT), label));
    y += placed.height.min(room.unwrap_or(i32::MAX));
    let list = Rectangle::new((x, start).into(), (row_w, y - start).into());
    let footer_h = footer_height(hints);
    let footer = Rectangle::new((loc.x, y + SWITCHER_PADDING).into(), (w, footer_h).into());
    let h = y + SWITCHER_PADDING + footer_h - loc.y;
    SwitcherLayout {
        panel: Rectangle::new(loc, (w, h).into()),
        input: Rectangle::new(loc, (w, SWITCHER_INPUT).into()),
        chips,
        rows,
        sections: headings,
        kept,
        list,
        footer,
        hints: Vec::new(),
    }
}

/// Where the text of the switcher's query, in its `input` rect, goes.
pub(super) fn switcher_text(input: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (
        px(input.loc.x + HELP_PADDING),
        px(input.loc.x + input.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
    )
}

/// The switcher's query field, in the app `area`.
pub fn switcher_field(view: &SwitcherView, area: Rect, scale: f64) -> FieldSpot {
    let input = switcher_geometry(view, area).input;
    FieldSpot {
        size: FONT_SIZE,
        area: input,
        text: switcher_text(input, scale),
        line: view.query.clone(),
    }
}

impl Painter {
    /// The switcher `w` wide inside a margin of `SHADOW` holding its drop
    /// shadow, in one buffer: the query (or what to type) with a caret, the
    /// category chips, the rows under their section headings, then the key
    /// hints. The chrome shows it in parts (see `SwitcherLayers`).
    #[cfg(test)]
    pub(super) fn switcher(
        &self,
        view: &SwitcherView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = switcher_parts(view, (SHADOW, SHADOW).into(), w, None);
        let mut c = self.switcher_frame(view, &layout, view.rows.is_empty(), scale, p);
        // The rows and their headings, cut to the list as it scrolls.
        let (x, y, w, h) = rect_px(layout.list, scale);
        c.clipped(x as i32, y as i32, w as i32, h as i32, |c| {
            for (rect, label) in &layout.sections {
                self.switcher_heading(c, label, *rect, scale, p);
            }
            for (i, (slot, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
                let fill = row_fill(view.selected == Some(i), view.hover == Some(i), p);
                self.switcher_row(c, row, fill, *slot, scale, p);
            }
            if let Some((rect, label)) = layout.kept {
                let (x, y, w, h) = rect_px(rect, scale);
                c.fill(x as i32, y as i32, w as i32, h as i32, p.elevated);
                self.switcher_heading(c, label, rect, scale, p);
            }
        });
        c.into_buffer(false)
    }

    /// The switcher laid out as `layout` (in buffer coordinates) but its
    /// rows and headings: the query of `view` with a caret, the category
    /// chips, "Nothing matches" when `empty`, then the key hints.
    fn switcher_frame(
        &self,
        view: &SwitcherView,
        layout: &SwitcherLayout,
        empty: bool,
        scale: f64,
        p: &Palette,
    ) -> Canvas {
        let (panel, input) = (layout.panel, layout.input);
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);

        let band = (px(input.loc.y), px(input.size.h));
        self.edit_line(
            &mut c,
            &view.query,
            view.hint,
            switcher_text(input, scale),
            band,
            true,
            FONT_SIZE,
            scale,
            p,
        );
        let divider = |c: &mut Canvas, y: i32| {
            c.fill(
                px(panel.loc.x) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };
        divider(&mut c, px(input.loc.y + input.size.h) - border);

        for (i, (rect, label)) in layout.chips.iter().zip(&view.categories).enumerate() {
            self.category_chip(&mut c, view, i, label, *rect, scale, p);
        }

        if empty {
            let slot = Rectangle::new(layout.list.loc, (layout.list.size.w, ROW_HEIGHT).into());
            let band = (px(slot.loc.y), px(slot.size.h));
            self.text(
                &mut c,
                "Nothing matches",
                FONT_SIZE,
                px(slot.loc.x) as f32 + PADDING * s,
                px(input.loc.x + input.size.w - HELP_PADDING) as f32,
                band,
                scale,
                p.placeholder,
            );
        }
        divider(&mut c, px(layout.footer.loc.y));
        let hints = hint_strs(&view.hints);
        self.hints(&mut c, &hints, &layout.hints, view.hint_hover, scale, p);
        c
    }

    /// A section's heading, `label`, in `rect`: upper case, muted, in the
    /// lower part of its band.
    fn switcher_heading(&self, c: &mut Canvas, label: &str, rect: Rect, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let band = (
            px(rect.loc.y + rect.size.h - SECTION_TEXT),
            px(SECTION_TEXT),
        );
        self.text(
            c,
            &label.to_uppercase(),
            TINY_FONT_SIZE,
            px(rect.loc.x) as f32 + PADDING * s,
            px(rect.loc.x + rect.size.w) as f32 - PADDING * s,
            band,
            scale,
            p.placeholder,
        );
    }

    /// A buffer of `size` on the panel's colour, drawn into with `draw` at
    /// the rect it fills.
    fn switcher_piece(
        &self,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
        draw: impl FnOnce(&mut Canvas, Rect),
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(super::physical(size, scale));
        let (w, h) = (c.w, c.h);
        c.fill(0, 0, w, h, p.elevated);
        draw(&mut c, Rectangle::from_size(size));
        c.into_buffer(true)
    }

    /// `row` in `slot`: its icon, its two lines of text and the kind
    /// pill, on a pill of `fill` (when selected or under the pointer).
    fn switcher_row(
        &self,
        c: &mut Canvas,
        row: &SwitcherRow,
        fill: Option<Rgb>,
        slot: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = (
            px(slot.loc.x),
            px(slot.loc.y),
            px(slot.size.w),
            px(slot.size.h),
        );
        if let Some(fill) = fill {
            c.rounded(
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                (MENU_RADIUS - 2.0) * s,
                fill,
            );
        }
        let icon = switcher_icon(slot);
        let small = 0.8 * s;
        match &row.icon {
            ItemIcon::Workspace(WorkspaceKind::Project) => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| folder(x, y, small))
            }
            ItemIcon::Workspace(WorkspaceKind::Terminal) => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| terminal(x, y, small))
            }
            ItemIcon::Workspace(WorkspaceKind::Cursor) => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| app(x, y, small))
            }
            ItemIcon::Tab => icon_sdf(c, icon, scale, p.placeholder, |x, y| tab(x, y, small)),
            &ItemIcon::Agent(status) => icon_sdf(c, icon, scale, p.status(status), |x, y| {
                status_mark(status, x, y, s)
            }),
            ItemIcon::Folder => icon_sdf(c, icon, scale, p.placeholder, |x, y| {
                folder_plus(x, y, small)
            }),
            ItemIcon::App => icon_sdf(c, icon, scale, p.placeholder, |x, y| app(x, y, small)),
            ItemIcon::Picture(picture) => c.picture(picture, icon, scale, p.placeholder),
            ItemIcon::Command => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| prompt(x, y, small))
            }
            ItemIcon::Settings => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| pencil(x, y, small))
            }
        }

        // The kind pill at the right; the text stops before it.
        let pill_w =
            self.text_width(row.kind, TINY_FONT_SIZE, scale) + 2.0 * PILL_PADDING as f32 * s;
        let pill_x = ((x + w) as f32 - PADDING * s - pill_w).round();
        let pill_h = px(PILL_HEIGHT) as f32;
        let pill_y = (y as f32 + (h as f32 - pill_h) / 2.0).round();
        c.rounded(
            pill_x,
            pill_y,
            pill_w.round(),
            pill_h,
            pill_h / 2.0,
            mix(fill.unwrap_or(p.elevated), p.muted, 0.18),
        );
        self.text(
            c,
            row.kind,
            TINY_FONT_SIZE,
            pill_x + PILL_PADDING as f32 * s,
            f32::MAX,
            (pill_y as i32, pill_h as i32),
            scale,
            p.muted,
        );

        let right = pill_x - px(PILL_GAP) as f32;
        let text_x = px(icon.loc.x + ROW_ICON_WIDTH + ROW_ICON_GAP) as f32;
        let label_band = (y + px(LINE_INSET), px(LABEL_LINE));
        self.marked(
            c,
            &row.label,
            &row.label_hits,
            FONT_SIZE,
            text_x,
            right,
            label_band,
            scale,
            (p.text, p.accent),
        );
        let detail_top = y + px(LINE_INSET + LABEL_LINE);
        let detail_band = (detail_top, y + h - px(LINE_INSET) - detail_top);
        self.marked(
            c,
            &row.detail,
            &row.detail_hits,
            SMALL_FONT_SIZE,
            text_x,
            right,
            detail_band,
            scale,
            (p.muted, p.accent),
        );
    }

    /// Category chip `i`, `label`, in `rect`: an outlined pill, filled and
    /// outlined in the accent when it is the category shown, lighter under
    /// the pointer.
    #[allow(clippy::too_many_arguments)]
    fn category_chip(
        &self,
        c: &mut Canvas,
        view: &SwitcherView,
        i: usize,
        label: &str,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let on = view.category == i;
        let fill = if on {
            p.selected
        } else if view.category_hover == Some(i) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, if on { p.accent } else { p.border });
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let color = if on { p.text } else { p.muted };
        self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
    }
}

/// The pill under a switcher row: selected, else under the pointer.
fn row_fill(selected: bool, hovered: bool, p: &Palette) -> Option<Rgb> {
    if selected {
        Some(p.selected)
    } else if hovered {
        Some(p.element)
    } else {
        None
    }
}

/// What the switcher's frame is drawn from: its view without the rows,
/// its layout (in buffer coordinates) without them, whether nothing
/// matches, the theme and the scale.
type FrameKey = (SwitcherView, SwitcherLayout, bool, super::Theme, f64);

/// What a row's buffer is drawn from: the row without its heading, its
/// width, whether it is selected and under the pointer, the theme and the
/// scale.
type RowKey = (SwitcherRow, i32, bool, bool, super::Theme, f64);

/// The switcher as the chrome shows it, fading in and out (see
/// `Overlay`): a frame buffer with what stays put, and a buffer per row
/// and per heading, which scrolling only moves.
#[derive(Default)]
pub(super) struct SwitcherLayers {
    overlay: super::Overlay<FrameKey>,
    rows: RowCache<RowKey>,
    /// By label, width, theme and scale.
    headings: RowCache<(&'static str, i32, super::Theme, f64)>,
    /// How many frames were drawn.
    #[cfg(test)]
    frames: std::cell::Cell<usize>,
}

impl SwitcherLayers {
    /// The parts showing the switcher at the top of the app `area` at
    /// `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<SwitcherView>,
        area: Rect,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<Part> {
        let shown = view.map(|view| {
            let layout = switcher_geometry(&view, area);
            (view, layout)
        });
        let placed = shown.as_ref().map(|(view, layout)| {
            let w = layout.panel.size.w;
            let room = Some(switcher_room(area));
            let frame = SwitcherLayout {
                rows: Vec::new(),
                sections: Vec::new(),
                kept: None,
                ..switcher_parts(view, (SHADOW, SHADOW).into(), w, room)
            };
            let still = SwitcherView {
                rows: Vec::new(),
                selected: None,
                hover: None,
                part: 0.0,
                ..view.clone()
            };
            let (loc, size) = super::shadowed(layout.panel);
            (
                (still, frame, view.rows.is_empty(), theme, scale),
                loc,
                size,
            )
        });
        let parts = self.overlay.parts(
            placed,
            now,
            true,
            |(view, layout, empty, ..)| {
                #[cfg(test)]
                self.frames.set(self.frames.get() + 1);
                painter
                    .switcher_frame(view, layout, *empty, scale, theme.palette())
                    .into_buffer(false)
            },
            |_| {
                let Some((view, layout)) = &shown else {
                    return Vec::new();
                };
                let placed = place(
                    &view.rows.iter().map(|row| row.section).collect::<Vec<_>>(),
                    view.part,
                );
                let (x, top) = (layout.list.loc.x, layout.list.loc.y as f64);
                let (w, bottom) = (layout.list.size.w, top + layout.list.size.h as f64);
                // Only what the list shows of the rows and headings.
                let out = |dy: f64, h: i32| top + dy >= bottom || top + dy + h as f64 <= top;
                let p = theme.palette();
                let mut parts = Vec::new();
                let mut heading = |parts: &mut Vec<Part>, (dy, label): (f64, &'static str)| {
                    if out(dy, SECTION_HEIGHT) {
                        return;
                    }
                    let size = Size::from((w, SECTION_HEIGHT));
                    let buffer = self
                        .headings
                        .get((label, w, theme, scale), size, scale, |_| {
                            painter.switcher_piece(size, scale, p, |c, rect| {
                                painter.switcher_heading(c, label, rect, scale, p)
                            })
                        });
                    let part = Part::new(&buffer, (x, 0).into(), size, 1.0);
                    parts.push(part.at((x as f64, top + dy).into()).clipped(layout.list));
                };
                if let Some(kept) = placed.kept {
                    heading(&mut parts, kept);
                }
                for &kept in &placed.headings {
                    heading(&mut parts, kept);
                }
                for (i, (row, &dy)) in view.rows.iter().zip(&placed.rows).enumerate() {
                    if out(dy, ROW_HEIGHT) {
                        continue;
                    }
                    let (selected, hovered) = (view.selected == Some(i), view.hover == Some(i));
                    let size = Size::from((w, ROW_HEIGHT));
                    let row = SwitcherRow {
                        section: None,
                        ..row.clone()
                    };
                    let key = (row, w, selected, hovered, theme, scale);
                    let buffer = self.rows.get(key, size, scale, |(row, ..)| {
                        painter.switcher_piece(size, scale, p, |c, rect| {
                            let fill = row_fill(selected, hovered, p);
                            painter.switcher_row(c, row, fill, rect, scale, p)
                        })
                    });
                    let part = Part::new(&buffer, (x, 0).into(), size, 1.0);
                    parts.push(part.at((x as f64, top + dy).into()).clipped(layout.list));
                }
                parts
            },
        );
        self.rows.sweep();
        self.headings.sweep();
        if !self.overlay.lingers() {
            self.rows.clear();
            self.headings.clear();
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHIP_GAP, CHIP_HEIGHT, CHIPS_ROW, ItemIcon, ROW_HEIGHT, SECTION_HEIGHT, SWITCHER_INPUT,
        SWITCHER_PADDING, SwitcherLayers, SwitcherRow, SwitcherView, chip_rects, footer_height,
        switcher_geometry, switcher_layout, switcher_places, switcher_room, switcher_row_at,
    };
    use crate::chrome::{Painter, Rect, Theme};
    use smithay::utils::Rectangle;
    use std::time::{Duration, Instant};

    fn row(section: Option<&'static str>) -> SwitcherRow {
        SwitcherRow {
            icon: ItemIcon::Tab,
            label: "a".into(),
            detail: "b".into(),
            kind: "tab",
            section,
            label_hits: Vec::new(),
            detail_hits: Vec::new(),
        }
    }

    #[test]
    fn headings_take_their_own_band_above_the_first_row_of_a_section() {
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let view = SwitcherView {
            query: "".into(),
            hint: "",
            categories: Vec::new(),
            category: 0,
            category_hover: None,
            rows: vec![row(Some("Workspaces")), row(None), row(Some("Apps"))],
            selected: Some(0),
            hover: None,
            hints: Vec::new(),
            hint_hover: None,
            part: 0.0,
        };
        let layout = switcher_geometry(&view, area);
        let top = layout.panel.loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
        let (kept, label) = layout.kept.unwrap();
        assert_eq!((kept.loc.y, label), (top, "Workspaces"));
        assert_eq!(layout.rows[0].loc.y, top + SECTION_HEIGHT);
        assert_eq!(layout.rows[1].loc.y, top + SECTION_HEIGHT + ROW_HEIGHT);
        assert_eq!(layout.sections.len(), 1);
        assert_eq!(
            layout.sections[0].0.loc.y,
            top + SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        assert_eq!(
            layout.rows[2].loc.y,
            top + 2 * SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        let footer = footer_height(0);
        assert_eq!(
            layout.panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 2 * SECTION_HEIGHT + 3 * ROW_HEIGHT + footer
        );
        // The footer closes the panel.
        assert_eq!(
            layout.footer.loc.y + layout.footer.size.h,
            layout.panel.loc.y + layout.panel.size.h
        );
        // The same rows without headings, from the count alone.
        let (panel, input, rows) = switcher_layout(3, area);
        assert_eq!(
            panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 3 * ROW_HEIGHT + footer
        );
        assert_eq!((panel.loc, input.loc), (layout.panel.loc, layout.input.loc));
        assert_eq!(rows[0].loc.y, top);
        // An empty list keeps room for the "Nothing matches" row.
        assert_eq!(
            switcher_layout(0, area).0.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + ROW_HEIGHT + footer
        );
    }

    #[test]
    fn the_category_chips_go_between_the_query_and_the_rows() {
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let view = SwitcherView {
            query: "".into(),
            hint: "",
            categories: vec!["All", "Workspaces", "Apps"],
            category: 0,
            category_hover: None,
            rows: vec![row(Some("Workspaces"))],
            selected: Some(0),
            hover: None,
            hints: vec![("Esc".into(), "close".into())],
            hint_hover: None,
            part: 0.0,
        };
        let layout = switcher_geometry(&view, area);
        let top = layout.panel.loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
        assert_eq!(layout.chips.len(), 3);
        assert!(layout.chips.iter().all(|chip| chip.loc.y == top));
        // Wider for a longer label, in order, apart.
        assert!(layout.chips[1].size.w > layout.chips[0].size.w);
        for pair in layout.chips.windows(2) {
            assert_eq!(pair[0].loc.x + pair[0].size.w + CHIP_GAP, pair[1].loc.x);
        }
        assert_eq!(layout.kept.unwrap().0.loc.y, top + CHIPS_ROW);
        assert_eq!(layout.rows[0].loc.y, top + CHIPS_ROW + SECTION_HEIGHT);
    }

    /// Rows of tabs under `sections`, a row each, `part` scrolled.
    fn scrolled(sections: &[Option<&'static str>], part: f64) -> SwitcherView {
        SwitcherView {
            query: "".into(),
            hint: "",
            categories: Vec::new(),
            category: 0,
            category_hover: None,
            rows: sections.iter().map(|section| row(*section)).collect(),
            selected: Some(0),
            hover: None,
            hints: Vec::new(),
            hint_hover: None,
            part,
        }
    }

    #[test]
    fn a_scrolled_list_moves_up_under_the_heading_kept_at_its_top() {
        // A room for three rows and a heading more than shows.
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let room = switcher_room(area);
        let n = (room / ROW_HEIGHT) as usize + 2;
        let mut sections = vec![None; n];
        sections[0] = Some("Workspaces");
        sections[2] = Some("Tabs");
        let (a, b) = (
            switcher_geometry(&scrolled(&sections, 0.0), area),
            switcher_geometry(&scrolled(&sections, 30.0), area),
        );
        // As high as the room, whatever the scroll; the rows moved up and
        // the heading kept where it was, over them.
        assert_eq!((a.list, a.panel), (b.list, b.panel));
        assert_eq!(a.list.size.h, room);
        assert_eq!(b.rows[0].loc.y, a.rows[0].loc.y - 30);
        assert_eq!(b.sections[0].0.loc.y, a.sections[0].0.loc.y - 30);
        assert_eq!(a.kept, b.kept);
        let bottom = b.list.loc.y + b.list.size.h;
        let last = b.rows.iter().find(|r| r.loc.y + r.size.h > bottom).unwrap();
        assert!(last.loc.y < bottom);
        // Only the rows' parts within the list and below the kept heading
        // are hit.
        let x = b.list.loc.x as f64 + 40.0;
        let at = |y: i32| switcher_row_at(&b, (x, y as f64 + 0.5).into());
        let under = b.list.loc.y + SECTION_HEIGHT;
        assert_eq!(at(b.list.loc.y), None);
        assert_eq!(at(under - 1), None);
        assert_eq!(at(under), Some(0));
        assert_eq!(at(bottom - 1), b.rows.iter().position(|r| r == last));
        assert_eq!(at(bottom), None);
        // Scrolled near the next heading, it pushes the kept one up.
        let near = (ROW_HEIGHT + SECTION_HEIGHT + ROW_HEIGHT - 10) as f64;
        let c = switcher_geometry(&scrolled(&sections, near), area);
        let (kept, label) = c.kept.unwrap();
        assert_eq!(label, "Workspaces");
        assert_eq!(kept.loc.y + kept.size.h, c.sections[0].0.loc.y);
        // Pulled down past the start, it comes down with the rows.
        let d = switcher_geometry(&scrolled(&sections, -20.0), area);
        assert_eq!(d.kept.unwrap().0.loc.y, a.list.loc.y + 20);
        assert_eq!(d.rows[0].loc.y, a.rows[0].loc.y + 20);
    }

    #[test]
    fn places_hold_a_row_and_the_band_over_the_next() {
        let places = switcher_places([true, false, true, false]);
        let (row, band) = (ROW_HEIGHT as f64, SECTION_HEIGHT as f64);
        assert_eq!(places.height(0), row);
        assert_eq!(places.height(1), row + band);
        assert_eq!(places.end(), 4.0 * row + 2.0 * band);
    }

    #[test]
    fn scrolling_by_part_of_a_row_draws_nothing_again() {
        let painter = Painter::new();
        let area: Rect = Rectangle::new((200, 36).into(), (1000, 800).into());
        let mut layers = SwitcherLayers::default();
        let mut sections = vec![None; 8];
        sections[0] = Some("Workspaces");
        sections[5] = Some("Tabs");
        let start = Instant::now();
        let draw = |layers: &mut SwitcherLayers, part: f64, ms: u64| {
            let at = start + Duration::from_millis(ms);
            let view = Some(scrolled(&sections, part));
            layers.parts(view, area, Theme::Dark, 1.25, at, &painter)
        };
        // Faded in first.
        draw(&mut layers, 0.0, 0);
        let first = draw(&mut layers, 0.0, 300);
        let (frames, rows) = (layers.frames.get(), layers.rows.drawn());
        let headings = layers.headings.drawn();
        assert_eq!(frames, 1);
        let moved = draw(&mut layers, 17.5, 400);
        assert_eq!(layers.frames.get(), frames);
        assert_eq!(layers.rows.drawn(), rows);
        assert_eq!(layers.headings.drawn(), headings);
        // Only the rows moved, by as much; the frame stayed.
        assert_eq!(moved.len(), first.len());
        let frame = |parts: &[crate::chrome::layers::Part]| parts.last().unwrap().loc;
        assert_eq!(frame(&moved), frame(&first));
        assert_eq!(moved[2].loc.y, first[2].loc.y - 17.5);
        assert_eq!(moved[0].loc, first[0].loc);
    }

    #[test]
    fn chips_too_wide_together_are_narrowed_to_fit() {
        let labels = [
            "All",
            "Workspaces",
            "Tabs",
            "Agents",
            "Projects",
            "Apps",
            "Folders",
        ];
        let chips = chip_rects(&labels, 10, 0, 310);
        let last = chips.last().unwrap();
        assert!(last.loc.x + last.size.w <= 310);
        assert!(chips.iter().all(|chip| chip.size.h == CHIP_HEIGHT));
        assert!(chip_rects(&[], 0, 0, 100).is_empty());
    }

    #[test]
    fn the_room_fits_the_area_within_bounds() {
        let room = |h: i32| switcher_room(Rectangle::new((0, 0).into(), (1000, h).into()));
        assert!(room(600) < room(800));
        assert_eq!(room(100), SECTION_HEIGHT + ROW_HEIGHT);
        assert_eq!(room(2000), 12 * ROW_HEIGHT + 2 * SECTION_HEIGHT);
    }
}
