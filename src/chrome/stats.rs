//! The stats panel on the features' AI work (see `crate::stats_panel`),
//! centred on the output: its title and scope (the open project or all of
//! them; a feature's detail has Back there instead), the tabs Features |
//! Agents | Checks beside the search, a table of rows that scrolls, and
//! the key hints under it. Rows a read hasn't brought, or couldn't, say so
//! in their place, and F1 lists every key there.

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    utils::{Logical, Point, Rectangle, Size},
};

use super::{
    BUTTON_RADIUS, ButtonStyle, Canvas, icon_sdf, trash, FONT_SIZE, FieldSpot, HELP_MARGIN, HELP_PADDING,
    HINT_KEY_GAP, LABEL_FONT_SIZE, ListBar, POPUP_RADIUS, PROJECT_MENU_INPUT, PROJECT_MENU_PADDING,
    Painter, Palette, ROW_BUTTON, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, Thumb, band, button_width,
    focus_ring, gutter_track, hint_at, hint_spots, hint_strs, measure, phys, query_text,
};
use crate::text_field::Line;

/// The least the panel takes of the output, room allowing: it fits an
/// output 870 px wide with no room to spare.
const STATS_WIDTH: i32 = 870;
const STATS_HEIGHT: i32 = 640;
/// The most it grows to on a big output, as a share of it then: a history
/// read in tables wants the room.
const STATS_MAX_WIDTH: i32 = 1400;
const STATS_MAX_HEIGHT: i32 = 1100;
/// The title row, the row of tabs and search under it, the columns'
/// titles and the footer.
const TITLE: i32 = 44;
const BAR: i32 = PROJECT_MENU_INPUT;
const HEAD: i32 = 26;
const FOOTER: i32 = 36;
/// A row of the table.
pub const STATS_ROW: i32 = 28;
/// The room around the list, and between its columns.
const PAD: i32 = PROJECT_MENU_PADDING;
const COL_GAP: i32 = 10;
/// The room for the scrollbar right of the rows.
const GUTTER: i32 = 6;
/// The tabs' segments, and the scope's.
const TAB_SEGMENT: i32 = 86;
const SCOPE_ALL: i32 = 108;
const SCOPE_MIN: i32 = 96;
const SCOPE_MAX: i32 = 220;
/// The room either side of a segment's label.
const SEGMENT_PAD: i32 = 24;
/// The keys listed (F1): a row's height, the heading's over them, the
/// room for a row's keycaps before what it does.
const KEYS_ROW: i32 = 26;
const KEYS_HEADING: i32 = 34;
const KEYS_CAPS: i32 = 220;

/// The tabs' labels, in order.
pub const STATS_TABS: [&str; 3] = ["Features", "Agents", "Checks"];

/// Where the keys go in the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatsFocus {
    Tabs,
    Scope,
    #[default]
    List,
}

/// A column of the table: its title, how wide it is (0 takes the room
/// left) and whether its cells end at its right edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsColumn {
    pub title: String,
    pub width: i32,
    pub right: bool,
}

/// A row of the table, laid out in the columns of set `set`; a heading
/// names them. `delete`: a trash button in its last column, which takes no
/// cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsRow {
    pub set: usize,
    pub cells: Vec<String>,
    pub heading: bool,
    pub delete: bool,
}

/// The side of a row's trash button.
const DELETE_BUTTON: i32 = 24;
/// The width of the column a row's trash button goes in.
pub const STATS_DELETE_COLUMN: i32 = DELETE_BUTTON;

/// What the stats panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsView {
    pub title: String,
    /// The tab shown, by index into `STATS_TABS`.
    pub tab: usize,
    /// The scope's choice: the open project's name (none without one) and
    /// whether all projects are shown. `None` in a feature's detail.
    pub scope: Option<(Option<String>, bool)>,
    /// A feature's detail: a Back button in the title row.
    pub back: bool,
    pub focus: StatsFocus,
    pub query: Line,
    /// The query has the keys.
    pub searching: bool,
    pub placeholder: String,
    /// The sets of columns rows are laid out in.
    pub columns: Vec<Vec<StatsColumn>>,
    /// The first set's titles over the list.
    pub header: bool,
    pub rows: Vec<StatsRow>,
    /// The first row shown.
    pub first: usize,
    pub selected: Option<usize>,
    /// Said in place of the rows: reading, nothing kept, or why the read
    /// failed (the bool: it is an error).
    pub note: Option<(String, bool)>,
    pub hints: Vec<(String, String)>,
    /// Every key it takes and what it does, listed in place of the rows
    /// while not empty (F1).
    pub keys: Vec<(String, String)>,
    /// What is under the pointer.
    pub hover: Option<StatsHit>,
}

/// What the pointer is over in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsHit {
    /// A tab, by index into `STATS_TABS`.
    Tab(usize),
    /// The scope's choice: all projects, or the open one.
    Scope(bool),
    Back,
    Query,
    Row(usize),
    /// A row's trash button.
    Delete(usize),
    Hint(usize),
    /// Anywhere else on it.
    Panel,
}

/// Where the panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The scope's track, and its segments: the open project's, then all.
    pub scope: Option<(Rect, [Rect; 2])>,
    pub back: Option<Rect>,
    /// The tabs' track, and a segment each.
    pub tabs: (Rect, [Rect; 3]),
    pub query: Rect,
    /// The first set's column titles.
    pub header: Option<Rect>,
    /// The list's ground, and the rows shown on it (by index).
    pub list: Rect,
    pub rows: Vec<(usize, Rect)>,
    /// The trash buttons of the rows shown that have one (by index).
    pub deletes: Vec<(usize, Rect)>,
    pub footer: Rect,
    /// Where each of the hints that fit goes, by index.
    pub hints: Vec<(usize, Rect)>,
}

/// How wide `text` is at `size`, logical pixels; without a font, a guess.
fn width(text: &str, size: f32) -> i32 {
    measure(text, size)
        .unwrap_or(text.chars().count() as f32 * size * 0.6)
        .ceil() as i32
}

/// The panel's place on an output of `size`.
fn panel_rect(size: Size<i32, Logical>) -> Rect {
    let w = (size.w * 3 / 5).clamp(STATS_WIDTH, STATS_MAX_WIDTH);
    let h = (size.h * 3 / 4).clamp(STATS_HEIGHT, STATS_MAX_HEIGHT);
    let w = w.min(size.w - 2 * HELP_MARGIN).max(1);
    let h = h.min(size.h - 2 * HELP_MARGIN).max(1);
    Rectangle::new(
        (((size.w - w) / 2).max(0), ((size.h - h) / 2).max(0)).into(),
        (w, h).into(),
    )
}

/// Where the parts of `view` go on an output of `size`.
pub fn stats_layout(view: &StatsView, size: Size<i32, Logical>) -> StatsLayout {
    let panel = panel_rect(size);
    let (x, top, w) = (panel.loc.x, panel.loc.y, panel.size.w);
    let right = x + w - HELP_PADDING;
    let title = Rectangle::new(
        (x + HELP_PADDING, top).into(),
        (w - 2 * HELP_PADDING, TITLE).into(),
    );
    let button_y = top + (TITLE - ROW_BUTTON) / 2;
    let scope = view.scope.as_ref().map(|(name, _)| {
        let own = name.as_deref().map_or(SCOPE_MIN, |n| {
            (width(n, SMALL_FONT_SIZE) + SEGMENT_PAD).clamp(SCOPE_MIN, SCOPE_MAX)
        });
        let all_x = right - SCOPE_ALL;
        let own_x = all_x - own;
        let seg = |x, w| Rectangle::new((x, button_y).into(), (w, ROW_BUTTON).into());
        let track = seg(own_x, own + SCOPE_ALL);
        (track, [seg(own_x, own), seg(all_x, SCOPE_ALL)])
    });
    let back = view.back.then(|| {
        let bw = button_width("Back");
        Rectangle::new((right - bw, button_y).into(), (bw, ROW_BUTTON).into())
    });
    let bar_top = top + TITLE;
    let tab_y = bar_top + (BAR - ROW_BUTTON) / 2;
    let tab = |i: i32| {
        Rectangle::new(
            (x + HELP_PADDING + i * TAB_SEGMENT, tab_y).into(),
            (TAB_SEGMENT, ROW_BUTTON).into(),
        )
    };
    let tabs_track = Rectangle::new(
        (x + HELP_PADDING, tab_y).into(),
        (3 * TAB_SEGMENT, ROW_BUTTON).into(),
    );
    let query_x = tabs_track.loc.x + tabs_track.size.w + COL_GAP;
    let query = Rectangle::new(
        (query_x, bar_top).into(),
        ((x + w - query_x).max(1), BAR).into(),
    );
    let mut list_top = bar_top + BAR;
    let header = view.header.then(|| {
        let rect = Rectangle::new(
            (x + PAD, list_top).into(),
            ((w - 2 * PAD).max(1), HEAD).into(),
        );
        list_top += HEAD;
        rect
    });
    let footer = Rectangle::new((x, top + panel.size.h - FOOTER).into(), (w, FOOTER).into());
    let list = Rectangle::new(
        (x + PAD, list_top).into(),
        ((w - 2 * PAD).max(1), (footer.loc.y - list_top).max(1)).into(),
    );
    let row_w = (list.size.w - 2 * PAD - GUTTER).max(1);
    let fits = ((list.size.h - 2 * PAD) / STATS_ROW).max(0) as usize;
    let rows = match view.keys.is_empty() && view.note.is_none() {
        true => (view.first..view.rows.len().min(view.first + fits))
            .enumerate()
            .map(|(k, i)| {
                let y = list.loc.y + PAD + k as i32 * STATS_ROW;
                (
                    i,
                    Rectangle::new((list.loc.x + PAD, y).into(), (row_w, STATS_ROW).into()),
                )
            })
            .collect(),
        false => Vec::new(),
    };
    let deletes = (rows.iter())
        .filter(|(i, _)| view.rows[*i].delete)
        .filter_map(|&(i, row)| {
            let columns = view.columns.get(view.rows[i].set)?;
            let last = *cells(columns, row).last()?;
            let side = DELETE_BUTTON.min(row.size.h);
            let x = last.loc.x + last.size.w - side;
            let y = row.loc.y + (row.size.h - side) / 2;
            Some((i, Rectangle::new((x, y).into(), (side, side).into())))
        })
        .collect();
    let hint_row = Rectangle::new(
        (x + HELP_PADDING, footer.loc.y).into(),
        ((w - 2 * HELP_PADDING).max(0), FOOTER).into(),
    );
    let hints = hint_spots(&hint_strs(&view.hints), hint_row);
    StatsLayout {
        panel,
        title,
        scope,
        back,
        tabs: (tabs_track, [tab(0), tab(1), tab(2)]),
        query,
        header,
        list,
        rows,
        deletes,
        footer,
        hints,
    }
}

/// The scrollbar of a list at `list` showing `shown` of `view`'s rows,
/// while they don't all show.
fn list_thumb(view: &StatsView, list: Rect, shown: usize) -> Option<Thumb> {
    let right = list.loc.x + list.size.w - PAD - GUTTER;
    let inner = Rectangle::new(
        (list.loc.x, list.loc.y + PAD).into(),
        (list.size.w, (list.size.h - 2 * PAD).max(1)).into(),
    );
    let bar = ListBar {
        down: view.first as f64 * STATS_ROW as f64,
        total: view.rows.len() as f64 * STATS_ROW as f64,
    };
    Thumb::of(
        gutter_track(right, inner),
        (shown as i32 * STATS_ROW) as f64,
        bar,
    )
}

/// The list's scrollbar, in window coordinates, while the rows show and
/// don't all fit: its thumb can be held.
pub fn stats_thumb(view: &StatsView, layout: &StatsLayout) -> Option<Thumb> {
    if !view.keys.is_empty() || view.note.is_some() {
        return None;
    }
    list_thumb(view, layout.list, layout.rows.len())
}

/// How many rows the list shows on an output of `size`, with the columns'
/// titles over it.
pub fn stats_page(size: Size<i32, Logical>) -> usize {
    let view = StatsView {
        header: true,
        ..StatsView::default()
    };
    let list = stats_layout(&view, size).list;
    ((list.size.h - 2 * PAD) / STATS_ROW).max(1) as usize
}

/// Where the cells of a row in `row` go, one per column of `columns`: the
/// fixed ones as wide as they say, the one of width 0 taking the rest.
fn cells(columns: &[StatsColumn], row: Rect) -> Vec<Rect> {
    let inner = row.size.w - 2 * PAD;
    let fixed: i32 = columns.iter().map(|c| c.width).sum();
    let gaps = COL_GAP * (columns.len() as i32 - 1).max(0);
    let flex = (inner - fixed - gaps).max(0);
    let mut x = row.loc.x + PAD;
    columns
        .iter()
        .map(|c| {
            let w = if c.width == 0 { flex } else { c.width };
            let rect = Rectangle::new((x, row.loc.y).into(), (w, row.size.h).into());
            x += w + COL_GAP;
            rect
        })
        .collect()
}

/// What of the panel is at `pos`, if it is on it.
pub fn stats_hit(
    view: &StatsView,
    layout: &StatsLayout,
    pos: Point<f64, Logical>,
) -> Option<StatsHit> {
    if !layout.panel.to_f64().contains(pos) {
        return None;
    }
    let on = |r: &Rect| r.to_f64().contains(pos);
    if layout.back.as_ref().is_some_and(on) {
        return Some(StatsHit::Back);
    }
    if let Some((_, [own, all])) = &layout.scope {
        if on(own) {
            return Some(StatsHit::Scope(false));
        }
        if on(all) {
            return Some(StatsHit::Scope(true));
        }
    }
    if let Some(i) = layout.tabs.1.iter().position(on) {
        return Some(StatsHit::Tab(i));
    }
    if on(&layout.query) {
        return Some(StatsHit::Query);
    }
    if let Some((i, _)) = layout.deletes.iter().find(|(_, r)| on(r)) {
        return Some(StatsHit::Delete(*i));
    }
    if let Some((i, _)) = layout.rows.iter().find(|(_, r)| on(r)) {
        return Some(StatsHit::Row(*i));
    }
    let hints = hint_strs(&view.hints);
    if let Some(i) = hint_at(&hints, &layout.hints, pos) {
        return Some(StatsHit::Hint(i));
    }
    Some(StatsHit::Panel)
}

/// The query's field, for the caret and selection, while it has the keys.
pub fn stats_field(view: &StatsView, size: Size<i32, Logical>, scale: f64) -> Option<FieldSpot> {
    if !view.searching {
        return None;
    }
    let area = stats_layout(view, size).query;
    Some(FieldSpot {
        size: FONT_SIZE,
        area,
        text: query_text(area, scale),
        line: view.query.clone(),
    })
}

/// The panel's rects a press can land on, for outlining them.
pub(super) fn stats_hit_rects(view: &StatsView, size: Size<i32, Logical>) -> Vec<Rect> {
    let layout = stats_layout(view, size);
    let mut rects: Vec<Rect> = layout.tabs.1.to_vec();
    rects.extend(layout.scope.iter().flat_map(|(_, segs)| *segs));
    rects.extend(layout.back);
    rects.push(layout.query);
    rects.extend(layout.rows.iter().map(|(_, r)| *r));
    rects.extend(layout.deletes.iter().map(|(_, r)| *r));
    rects.extend(layout.hints.iter().map(|(_, r)| *r));
    rects
}

impl Painter {
    /// `text` in `rect` (buffer coordinates) at `size`, its end at the
    /// rect's right edge when `right`, cut with "…" where it runs past.
    #[allow(clippy::too_many_arguments)]
    fn stats_cell(
        &self,
        c: &mut Canvas,
        text: &str,
        rect: Rect,
        right: bool,
        (size, bold): (f32, bool),
        scale: f64,
        color: Rgb,
    ) {
        let (left, end) = phys(rect, scale);
        let w = match bold {
            true => self.bold_width(text, size, scale),
            false => self.text_width(text, size, scale),
        };
        let x = if right { (end - w).max(left) } else { left };
        let cell_band = band(rect, scale);
        match bold {
            true => self.bold_text(c, text, size, x, end, cell_band, scale, color),
            false => self.text(c, text, size, x, end, cell_band, scale, color),
        };
    }

    /// The stats panel for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow.
    pub(super) fn stats_panel(
        &self,
        view: &StatsView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = stats_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);

        // The title, up to the scope or Back.
        let title = shift(layout.title);
        let (left, right) = phys(title, scale);
        let title_end = match (&layout.scope, layout.back) {
            (Some((track, _)), _) => phys(shift(*track), scale).0,
            (None, Some(back)) => phys(shift(back), scale).0,
            (None, None) => right,
        } - (HELP_PADDING as f64 * scale) as f32;
        self.bold_text(
            &mut c,
            &view.title,
            LABEL_FONT_SIZE,
            left,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        if let (Some((track, [own, all])), Some((name, everything))) = (&layout.scope, &view.scope)
        {
            if view.focus == StatsFocus::Scope {
                focus_ring(
                    &mut c,
                    shift(*track),
                    BUTTON_RADIUS,
                    p.elevated,
                    scale,
                    p.accent,
                );
            }
            let segments = [
                (name.as_deref().unwrap_or("This project"), shift(*own)),
                ("All projects", shift(*all)),
            ];
            let hover = match view.hover {
                Some(StatsHit::Scope(all)) => Some(all as usize),
                _ => None,
            };
            // Without a project, only all of them can be shown.
            let off: &[usize] = if name.is_none() { &[0] } else { &[] };
            let on = *everything as usize;
            self.segmented_off(&mut c, shift(*track), &segments, (on, hover, off), scale, p);
        }
        if let Some(back) = layout.back {
            let hovered = view.hover == Some(StatsHit::Back);
            let style = ButtonStyle::Plain;
            self.button(
                &mut c,
                "Back",
                shift(back),
                (hovered, false),
                style,
                scale,
                p,
            );
        }

        // The tabs, and the search beside them.
        let (track, tabs) = &layout.tabs;
        if view.focus == StatsFocus::Tabs {
            focus_ring(
                &mut c,
                shift(*track),
                BUTTON_RADIUS,
                p.elevated,
                scale,
                p.accent,
            );
        }
        let segments: Vec<(&str, Rect)> = STATS_TABS
            .iter()
            .zip(tabs)
            .map(|(label, rect)| (*label, shift(*rect)))
            .collect();
        let hover = match view.hover {
            Some(StatsHit::Tab(i)) => Some(i),
            _ => None,
        };
        self.segmented(&mut c, shift(*track), &segments, view.tab, hover, scale, p);
        self.query_field(
            &mut c,
            &view.query,
            &view.placeholder,
            shift(layout.query),
            view.searching,
            scale,
            p,
        );

        // The columns' titles.
        if let (Some(header), Some(columns)) = (layout.header, view.columns.first()) {
            let row = Rectangle::new(
                (header.loc.x + PAD, header.loc.y).into(),
                ((header.size.w - 2 * PAD - GUTTER).max(1), header.size.h).into(),
            );
            for (column, rect) in columns.iter().zip(cells(columns, row)) {
                let font = (SMALL_FONT_SIZE, false);
                self.stats_cell(
                    &mut c,
                    &column.title,
                    shift(rect),
                    column.right,
                    font,
                    scale,
                    p.muted,
                );
            }
        }

        // The list: the keys, a note in place of rows, or the rows.
        let list = shift(layout.list);
        if !view.keys.is_empty() {
            self.stats_keys(&mut c, view, list, scale, p);
        } else if let Some((note, error)) = &view.note {
            let color = if *error { p.error } else { p.muted };
            let rect = Rectangle::new(
                (list.loc.x + HELP_PADDING, list.loc.y + PAD).into(),
                ((list.size.w - 2 * HELP_PADDING).max(1), 3 * STATS_ROW).into(),
            );
            if self.text_width(note, FONT_SIZE, scale) <= phys(rect, scale).1 - phys(rect, scale).0
            {
                self.centered(&mut c, note, FONT_SIZE, rect, scale, color);
            } else {
                self.stats_cell(&mut c, note, rect, false, (FONT_SIZE, false), scale, color);
            }
        } else {
            self.stats_rows(&mut c, view, &layout, by, scale, p);
        }

        let hints = hint_strs(&view.hints);
        let spots: Vec<(usize, Rect)> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(StatsHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hints, &spots, hover, scale, p);
        c.into_buffer(false)
    }

    /// The rows shown, the selected one lit, the one under the pointer
    /// lighter; a heading's titles muted. The scrollbar beside them while
    /// they don't all show. `by` moves the layout onto the buffer.
    fn stats_rows(
        &self,
        c: &mut Canvas,
        view: &StatsView,
        layout: &StatsLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        for &(i, rect) in &layout.rows {
            let row = &view.rows[i];
            let rect = shift(rect);
            let on_delete = view.hover == Some(StatsHit::Delete(i));
            let hovered = (view.hover == Some(StatsHit::Row(i)) || on_delete) && !row.heading;
            let selected = view.selected == Some(i);
            self.row_highlight(
                c,
                rect,
                (selected, if hovered { 1.0 } else { 0.0 }),
                scale,
                p,
            );
            let Some(columns) = view.columns.get(row.set) else {
                continue;
            };
            for (k, ((cell, column), spot)) in row
                .cells
                .iter()
                .zip(columns)
                .zip(cells(columns, rect))
                .enumerate()
            {
                let (font, color) = match (row.heading, k) {
                    (true, 0) => ((SMALL_FONT_SIZE, true), p.text),
                    (true, _) => ((SMALL_FONT_SIZE, false), p.muted),
                    (false, _) => ((SMALL_FONT_SIZE, false), p.text),
                };
                self.stats_cell(c, cell, spot, column.right, font, scale, color);
            }
        }
        // Each trash button shows on its row, not only the one pointed at.
        let s = scale as f32;
        for &(i, rect) in &layout.deletes {
            let rect = shift(rect);
            let on = view.hover == Some(StatsHit::Delete(i));
            if on {
                self.button(c, "", rect, (true, false), ButtonStyle::Plain, scale, p);
            }
            let color = if on { p.error } else { p.muted };
            icon_sdf(c, rect, scale, color, |x, y| trash(x, y, s));
        }
        if let Some(thumb) = list_thumb(view, shift(layout.list), layout.rows.len()) {
            self.thumb_in(c, &thumb, scale, p);
        }
    }

    /// The keys listed in place of the rows: a heading, then each key's
    /// keycaps and what it does, down two columns when one isn't enough.
    fn stats_keys(&self, c: &mut Canvas, view: &StatsView, list: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let inset = HELP_PADDING - PAD;
        let heading = Rectangle::new(
            (list.loc.x + inset, list.loc.y + PAD).into(),
            ((list.size.w - 2 * inset).max(1), KEYS_HEADING).into(),
        );
        let (left, right) = phys(heading, scale);
        let text = "Keys in the AI work stats";
        self.text(
            c,
            text,
            FONT_SIZE,
            left,
            right,
            band(heading, scale),
            scale,
            p.text,
        );
        let top = heading.loc.y + KEYS_HEADING;
        let per_column = ((list.loc.y + list.size.h - PAD - top) / KEYS_ROW).max(1) as usize;
        let columns = view.keys.len().div_ceil(per_column).max(1);
        let w = heading.size.w / columns as i32;
        let (caps, gap) = (
            KEYS_CAPS as f32 * s / columns as f32,
            HINT_KEY_GAP as f32 * s,
        );
        for (i, (keys, what)) in view.keys.iter().enumerate() {
            let (column, row) = (i / per_column, i % per_column);
            let rect = Rectangle::new(
                (
                    heading.loc.x + column as i32 * w,
                    top + row as i32 * KEYS_ROW,
                )
                    .into(),
                (w, KEYS_ROW).into(),
            );
            let (left, right) = phys(rect, scale);
            let row_band = band(rect, scale);
            let end = self.keycaps(c, keys, left, row_band, scale, p);
            let x = (left + caps).max(end + gap);
            self.text(
                c,
                what,
                SMALL_FONT_SIZE,
                x,
                right - gap,
                row_band,
                scale,
                p.text,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(rows: usize) -> StatsView {
        let col = |w| StatsColumn {
            title: "x".into(),
            width: w,
            right: false,
        };
        StatsView {
            scope: Some((Some("gerdr".into()), false)),
            header: true,
            columns: vec![vec![col(0), col(100), col(60)]],
            rows: (0..rows)
                .map(|i| StatsRow {
                    set: 0,
                    cells: vec![i.to_string()],
                    heading: false,
                    delete: i == 1,
                })
                .collect(),
            hints: vec![
                ("Ctrl+R".into(), "rebuild".into()),
                ("Esc".into(), "close".into()),
            ],
            ..StatsView::default()
        }
    }

    fn mid(r: Rect) -> Point<f64, Logical> {
        Point::from((
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        ))
    }

    #[test]
    fn it_fits_a_small_output_and_keeps_its_rows_inside() {
        for size in [(870, 600), (1920, 1080), (700, 420)] {
            let size = Size::from(size);
            let layout = stats_layout(&view(200), size);
            assert!(super::super::inside(
                Rectangle::from_size(size),
                layout.panel
            ));
            assert!(layout.panel.size.w <= STATS_MAX_WIDTH);
            assert!(!layout.rows.is_empty());
            let last = layout.rows.last().unwrap().1;
            // Enter never acts on a row out of sight: those shown are inside.
            assert!(last.loc.y + last.size.h <= layout.list.loc.y + layout.list.size.h);
            assert_eq!(layout.rows.len(), stats_page(size));
        }
    }

    #[test]
    fn each_part_is_hit_where_it_is() {
        let v = view(3);
        let layout = stats_layout(&v, (1200, 800).into());
        let (_, [own, all]) = layout.scope.unwrap();
        assert_eq!(
            stats_hit(&v, &layout, mid(own)),
            Some(StatsHit::Scope(false))
        );
        assert_eq!(
            stats_hit(&v, &layout, mid(all)),
            Some(StatsHit::Scope(true))
        );
        assert_eq!(
            stats_hit(&v, &layout, mid(layout.tabs.1[2])),
            Some(StatsHit::Tab(2))
        );
        assert_eq!(
            stats_hit(&v, &layout, mid(layout.query)),
            Some(StatsHit::Query)
        );
        assert_eq!(
            stats_hit(&v, &layout, mid(layout.rows[1].1)),
            Some(StatsHit::Row(1))
        );
        let (i, hint) = layout.hints[0];
        assert_eq!(stats_hit(&v, &layout, mid(hint)), Some(StatsHit::Hint(i)));
        assert_eq!(
            stats_hit(&v, &layout, mid(layout.title)),
            Some(StatsHit::Panel)
        );
        assert_eq!(stats_hit(&v, &layout, Point::from((1.0, 1.0))), None);
    }

    #[test]
    fn a_note_or_the_keys_take_the_rows_place() {
        let mut v = view(3);
        v.note = Some(("Reading…".into(), false));
        assert!(stats_layout(&v, (1200, 800).into()).rows.is_empty());
        v.note = None;
        v.keys = vec![("F1".into(), "List these keys".into())];
        assert!(stats_layout(&v, (1200, 800).into()).rows.is_empty());
    }

    #[test]
    fn the_columns_fill_the_row_the_wide_one_taking_the_rest() {
        let v = view(1);
        let row = Rectangle::new((0, 0).into(), (500, 28).into());
        let rects = cells(&v.columns[0], row);
        assert_eq!(rects[1].size.w, 100);
        assert_eq!(rects[2].size.w, 60);
        let end = rects[2].loc.x + rects[2].size.w;
        assert_eq!(end, row.size.w - PAD);
        assert_eq!(rects[0].loc.x, PAD);
    }

    #[test]
    fn the_field_shows_only_while_searching() {
        let mut v = view(1);
        assert!(stats_field(&v, (1200, 800).into(), 1.0).is_none());
        v.searching = true;
        let spot = stats_field(&v, (1200, 800).into(), 1.0).unwrap();
        assert_eq!(spot.area, stats_layout(&v, (1200, 800).into()).query);
    }
}
