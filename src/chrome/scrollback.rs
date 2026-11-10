//! The scrollback viewer over a terminal pane (see `crate::scrollback`):
//! a title row, the pane's output in the monospace font on the terminal's
//! own colours (in those it was drawn in, as far as known), and the key
//! hints under it. Each row is a buffer of its
//! own, so scrolling only moves them. The search (see `search`) goes in
//! the title row, its ticks over the scrollbar. F1 lists the keys in
//! place of the text.

use std::ops::Range;

use super::{
    FIELD_GAP, FONT_SIZE, FOOTER_PAD, FOOTER_ROW, FieldSpot, HELP_PADDING, HELP_RADIUS,
    HINT_KEY_GAP, Painter, Palette, ROW_BUTTON, Rect, SHADOW, SMALL_FONT_SIZE, SearchBar,
    SearchPart, band, hint_spots, hint_strs, layers::Part, measure, measure_mono, mix, phys,
    physical, rect_px, search_field_text, search_spots,
};
use crate::scrollback::{Fit, Ink, Style};
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The title row's height, and the footer's under the list.
const TITLE: i32 = 34;
const FOOTER: i32 = 34;
/// The room around the list inside the panel.
const PAD: i32 = 6;
/// The room between the list's edges and its text.
const TEXT_INSET: i32 = 8;
/// The text's size, and a row's height.
const MONO_SIZE: f32 = 13.0;
const ROW: i32 = 18;
/// How far bold text is drawn again to the right, logical pixels, when
/// the monospace font has no bold face (see `Painter::has_mono_bold`).
const BOLD_SHIFT: f32 = 0.6;
/// How far dim text goes towards the ground.
const DIM: f32 = 0.4;
/// The scrollbar at the list's right, and the gap beside it (its thumb's
/// least height is every scrollbar's, see `Thumb::of`).
const SCROLLBAR: i32 = 6;
const SCROLLBAR_GAP: i32 = 4;
/// The title's room from the panel's sides.
const TITLE_INSET: i32 = 14;
/// The most the open search takes of the title row, its buttons too.
const SEARCH_MAX: i32 = 520;
/// The keys listed (F1): a row's height, the heading's over them, the
/// room for a row's keycaps before what it does, and the least width of
/// a column of them.
const KEYS_ROW: i32 = 26;
const KEYS_HEADING: i32 = 34;
const KEYS_CAPS: i32 = 190;
const KEYS_COLUMN: i32 = 480;

/// What the scrollback viewer shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollbackView {
    /// Where it goes on the output: over its pane.
    pub panel: Rect,
    pub title: String,
    /// How long the text is, or how much of it is selected.
    pub note: String,
    /// The rows from `first`, as many as show.
    pub rows: Vec<ScrollbackRow>,
    pub first: usize,
    /// How far the first row is scrolled past, 0 to 1, and how far the rows
    /// are moved down past an end, logical pixels (see `Scroll`).
    pub part: f64,
    pub overscroll: f64,
    /// How many rows there are in all.
    pub total: usize,
    pub hints: Vec<(String, String)>,
    /// What is under the pointer.
    pub hover: Option<ScrollbackHit>,
    /// The search, in the title row in place of the title while open.
    pub search: Option<SearchBar>,
    /// Where the places it found are on the scrollbar: shares of all the
    /// rows from the top, each with whether it is the one gone to.
    pub ticks: Vec<(f64, bool)>,
    /// Every key it takes and what it does, listed in place of the text
    /// while not empty (F1).
    pub keys: Vec<(String, String)>,
}

/// A row of the viewer's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackRow {
    pub text: String,
    /// From and to which of its characters it is selected, and whether
    /// the selection goes on past its end (see `Scrollback::row_selection`).
    pub selected: Option<(usize, usize, bool)>,
    /// The places the search found on it, by character of it, each with
    /// whether it is the one gone to.
    pub found: Vec<(Range<usize>, bool)>,
    /// The part of the word at the caret on it, by character of it: what
    /// `*` searches for, underlined.
    pub word: Option<Range<usize>>,
    /// The character of it the caret is on.
    pub caret: Option<usize>,
    /// The runs of it the terminal drew other than plainly, by character
    /// of it.
    pub styles: Vec<(Range<usize>, Style)>,
}

/// Where the viewer's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The list's ground, and where its rows go in it.
    pub list: Rect,
    pub text: Rect,
    pub footer: Rect,
    /// Where each of the hints that fit goes, by index.
    pub hints: Vec<(usize, Rect)>,
}

/// What the pointer is over in the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbackHit {
    Text,
    Hint(usize),
    Search(SearchPart),
    /// Anywhere else on it.
    Panel,
}

/// The width of a character of the text, in logical pixels.
fn advance() -> f64 {
    measure_mono("0", MONO_SIZE)
        .filter(|a| *a > 0.0)
        .unwrap_or(MONO_SIZE * 0.6) as f64
}

/// Where the viewer's parts go in `panel`, with `hints` under the list.
pub fn scrollback_layout(panel: Rect, hints: &[(String, String)]) -> ScrollbackLayout {
    let title = Rectangle::new(panel.loc, (panel.size.w, TITLE).into());
    let list = Rectangle::new(
        (panel.loc.x + PAD, panel.loc.y + TITLE).into(),
        (
            (panel.size.w - 2 * PAD).max(1),
            (panel.size.h - TITLE - FOOTER).max(1),
        )
            .into(),
    );
    let text = Rectangle::new(
        (list.loc.x + TEXT_INSET, list.loc.y + PAD).into(),
        (
            (list.size.w - TEXT_INSET - 2 * SCROLLBAR_GAP - SCROLLBAR).max(1),
            (list.size.h - 2 * PAD).max(1),
        )
            .into(),
    );
    let footer = Rectangle::new(
        (panel.loc.x, list.loc.y + list.size.h).into(),
        (panel.size.w, FOOTER).into(),
    );
    let hints = footer_spots(&hint_strs(hints), footer);
    ScrollbackLayout {
        panel,
        title,
        list,
        text,
        footer,
        hints,
    }
}

/// Where `hints` go in the footer's one row: all that fit; else, the
/// last two (the keys and Esc) kept, as many before them as fit.
fn footer_spots(hints: &[(&str, &str)], footer: Rect) -> Vec<(usize, Rect)> {
    let row = Rectangle::new(
        (footer.loc.x + HELP_PADDING, footer.loc.y + FOOTER_PAD).into(),
        ((footer.size.w - 2 * HELP_PADDING).max(0), FOOTER_ROW).into(),
    );
    let mut keep: Vec<usize> = (0..hints.len()).collect();
    loop {
        let shown: Vec<(&str, &str)> = keep.iter().map(|&i| hints[i]).collect();
        let spots = hint_spots(&shown, row);
        if spots.len() == keep.len() || keep.len() <= 2 {
            return spots.into_iter().map(|(j, r)| (keep[j], r)).collect();
        }
        keep.remove(keep.len() - 3);
    }
}

/// Where the keys listed go in the list of a viewer laid out as
/// `layout`, `n` of them: the heading over them, and each row, down as
/// many columns side by side as they need.
pub fn scrollback_keys_spots(layout: &ScrollbackLayout, n: usize) -> (Rect, Vec<Rect>) {
    let list = layout.list;
    let inset = TITLE_INSET - PAD;
    let heading = Rectangle::new(
        (list.loc.x + inset, list.loc.y + PAD).into(),
        ((list.size.w - 2 * inset).max(1), KEYS_HEADING).into(),
    );
    let top = heading.loc.y + KEYS_HEADING;
    let per_column = ((list.loc.y + list.size.h - PAD - top) / KEYS_ROW).max(1) as usize;
    let columns = n.div_ceil(per_column).max(1);
    let w = heading.size.w / columns as i32;
    let rows = (0..n)
        .map(|i| {
            let (column, row) = (i / per_column, i % per_column);
            Rectangle::new(
                (
                    heading.loc.x + column as i32 * w,
                    top + row as i32 * KEYS_ROW,
                )
                    .into(),
                (w, KEYS_ROW).into(),
            )
        })
        .collect();
    (heading, rows)
}

/// Whether `n` keys listed fit in a viewer at `panel`, each column as
/// wide as `KEYS_COLUMN` at least.
pub fn scrollback_keys_fit(panel: Rect, n: usize) -> bool {
    let layout = scrollback_layout(panel, &[]);
    let (_, rows) = scrollback_keys_spots(&layout, n);
    rows.first().is_none_or(|r| r.size.w >= KEYS_COLUMN)
}

/// How the text fits in a viewer at `panel`.
pub fn scrollback_fit(panel: Rect) -> Fit {
    let text = scrollback_layout(panel, &[]).text;
    Fit {
        cols: ((text.size.w as f64 / advance()).floor() as usize).max(1),
        page: (text.size.h / ROW).max(1) as usize,
        row_h: ROW as f64,
        view_h: text.size.h as f64,
    }
}

/// How many rows of the text show at most in a viewer at `panel`, the
/// first and the last perhaps in part.
pub fn scrollback_shown(panel: Rect) -> usize {
    (scrollback_layout(panel, &[]).text.size.h / ROW) as usize + 2
}

/// Where the search's parts go in the title row, left of the note (see
/// `search_spots`): its button while closed; open, its field (as wide as
/// there is room for, up to `SEARCH_MAX` with the rest), its buttons,
/// and where its count goes.
pub fn scrollback_search(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
) -> (Vec<(SearchPart, Rect)>, Option<Rect>) {
    let title = layout.title;
    let note = measure(&view.note, SMALL_FONT_SIZE).unwrap_or(0.0).ceil() as i32;
    let right = title.loc.x + title.size.w - TITLE_INSET - note - FIELD_GAP;
    let left = (title.loc.x + TITLE_INSET).max(right - SEARCH_MAX);
    let y = title.loc.y + (TITLE - ROW_BUTTON) / 2;
    match &view.search {
        None => {
            let open = Rectangle::new(
                (right - ROW_BUTTON, y).into(),
                (ROW_BUTTON, ROW_BUTTON).into(),
            );
            (vec![(SearchPart::Open, open)], None)
        }
        Some(_) => {
            let (parts, count) = search_spots(left, right, y, ROW_BUTTON);
            (parts, Some(count))
        }
    }
}

/// The search's field, for the caret and selection, while it has the
/// keyboard.
pub fn scrollback_field(view: &ScrollbackView, scale: f64) -> Option<FieldSpot> {
    let bar = view.search.as_ref().filter(|bar| bar.focused)?;
    let layout = scrollback_layout(view.panel, &view.hints);
    let (parts, _) = scrollback_search(view, &layout);
    let &(_, area) = parts.iter().find(|(part, _)| *part == SearchPart::Field)?;
    Some(FieldSpot {
        size: FONT_SIZE,
        area,
        text: search_field_text(area, scale),
        line: bar.query.clone(),
    })
}

/// What of the viewer is at `pos`, if it is on it.
pub fn scrollback_hit(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
    pos: Point<f64, Logical>,
) -> Option<ScrollbackHit> {
    if !layout.panel.to_f64().contains(pos) {
        return None;
    }
    if layout.list.to_f64().contains(pos) {
        return Some(ScrollbackHit::Text);
    }
    let (search, _) = scrollback_search(view, layout);
    if let Some(&(part, _)) = search.iter().find(|(_, r)| r.to_f64().contains(pos)) {
        return Some(ScrollbackHit::Search(part));
    }
    // A choice of keys too ("Esc / Q"): a click presses the first.
    let hint = layout
        .hints
        .iter()
        .find(|(_, spot)| spot.to_f64().contains(pos));
    Some(match hint {
        Some(&(i, _)) => ScrollbackHit::Hint(i),
        None => ScrollbackHit::Panel,
    })
}

/// Which row and character `pos` is at, the row counted in all of them:
/// before the first row or past the last when it is above or below them.
/// The character is the one whose left edge is nearest, as a caret goes.
pub fn scrollback_point(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
    pos: Point<f64, Logical>,
) -> (i64, usize) {
    let (row, x) = scrollback_spot(view, layout, pos);
    (row, x.round().max(0.0) as usize)
}

/// Which row and character `pos` is on, as `scrollback_point` finds them
/// but the character the one under it.
pub fn scrollback_char(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
    pos: Point<f64, Logical>,
) -> (i64, usize) {
    let (row, x) = scrollback_spot(view, layout, pos);
    (row, x.floor().max(0.0) as usize)
}

/// The row `pos` is at and how many characters it is in from the left.
fn scrollback_spot(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
    pos: Point<f64, Logical>,
) -> (i64, f64) {
    let text = layout.text;
    let y = pos.y - text.loc.y as f64 + view.part * ROW as f64 - view.overscroll;
    let row = view.first as i64 + (y / ROW as f64).floor() as i64;
    (row, (pos.x - text.loc.x as f64) / advance())
}

/// How far `pos` is past the top of the text (negative) or its bottom
/// (positive), logical pixels; 0 within it.
pub fn scrollback_edge(layout: &ScrollbackLayout, pos: Point<f64, Logical>) -> f64 {
    let text = layout.text;
    let (top, bottom) = (text.loc.y as f64, (text.loc.y + text.size.h) as f64);
    if pos.y < top {
        pos.y - top
    } else if pos.y >= bottom {
        pos.y - bottom + 1.0
    } else {
        0.0
    }
}

/// The scrollbar's track, at the list's right.
fn track(layout: &ScrollbackLayout) -> Rect {
    let list = layout.list;
    Rectangle::new(
        (
            list.loc.x + list.size.w - SCROLLBAR_GAP - SCROLLBAR,
            list.loc.y + PAD,
        )
            .into(),
        (SCROLLBAR, (list.size.h - 2 * PAD).max(0)).into(),
    )
}

/// The scrollbar for `view`, while its text does not all show.
pub fn scrollback_thumb(view: &ScrollbackView, layout: &ScrollbackLayout) -> Option<super::Thumb> {
    let page = (layout.text.size.h / ROW).max(1);
    let bar = super::ListBar {
        down: view.first as f64 + view.part,
        total: view.total as f64,
    };
    super::Thumb::of(track(layout), page as f64, bar)
}

/// What the frame is drawn from: the view without its rows and where they
/// stand, so scrolling leaves it be.
fn frame_view(view: &ScrollbackView) -> ScrollbackView {
    ScrollbackView {
        rows: Vec::new(),
        first: 0,
        part: 0.0,
        overscroll: 0.0,
        ticks: Vec::new(),
        ..view.clone()
    }
}

/// The colour `ink` names: the terminal's own for the first 16 of its
/// 256, xterm's for the rest.
fn ink_color(ink: Ink, p: &Palette) -> super::Rgb {
    let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
    match ink {
        Ink::Rgb(rgb) => rgb,
        Ink::Index(i) if i < 16 => p.ansi[i as usize],
        Ink::Index(i) if i < 232 => {
            let i = i - 16;
            [level(i / 36), level(i / 6 % 6), level(i % 6)]
        }
        Ink::Index(i) => {
            let v = 8 + (i - 232) * 10;
            [v, v, v]
        }
    }
}

/// The ground under the selection, on the terminal's background.
fn selection_color(p: &Palette) -> [u8; 3] {
    mix(p.terminal_background, p.accent, 0.35)
}

impl Painter {
    /// The viewer's frame, inside a margin of `SHADOW` holding its drop
    /// shadow: its title and note, the list's ground and the key hints.
    fn scrollback_frame(&self, view: &ScrollbackView, scale: f64, p: &Palette) -> super::Canvas {
        let layout = scrollback_layout(view.panel, &view.hints);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let s = scale as f32;
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);

        let title = shift(layout.title);
        let (left, right) = phys(title, scale);
        let inset = (TITLE_INSET as f32 * s).round();
        let note_w = (measure(&view.note, SMALL_FONT_SIZE).unwrap_or(0.0) * s).ceil();
        let note_x = right - inset - note_w;
        let (search, count) = scrollback_search(view, &layout);
        // Left of the search, open or not, if there is room.
        let title_end = match search.first() {
            Some((_, r)) => (shift(*r).loc.x as f64 * scale) as f32 - inset,
            None => note_x - inset,
        };
        self.text(
            &mut c,
            &view.note,
            SMALL_FONT_SIZE,
            note_x,
            right,
            band(title, scale),
            scale,
            p.muted,
        );
        if title_end > left + inset {
            self.text(
                &mut c,
                &view.title,
                FONT_SIZE,
                left + inset,
                title_end,
                band(title, scale),
                scale,
                p.text,
            );
        }
        let search: Vec<(SearchPart, Rect)> = search
            .into_iter()
            .map(|(part, r)| (part, shift(r)))
            .collect();
        let hover = match view.hover {
            Some(ScrollbackHit::Search(part)) => Some(part),
            _ => None,
        };
        self.search_bar(
            &mut c,
            view.search.as_ref(),
            &search,
            count.map(shift),
            hover,
            scale,
            p,
        );

        let (x, y, w, h) = rect_px(shift(layout.list), scale);
        if view.keys.is_empty() {
            c.rounded(x, y, w, h, 4.0 * s, p.terminal_background);
        } else {
            self.scrollback_keys(&mut c, view, &layout, by, scale, p);
        }

        let hints = hint_strs(&view.hints);
        let spots: Vec<(usize, Rect)> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hover = match view.hover {
            Some(ScrollbackHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(&mut c, &hints, &spots, hover, scale, p);
        c
    }

    /// The keys listed in place of the text, on the panel's own ground:
    /// a heading, then each key's keycaps and what it does. `by` moves
    /// the layout onto the frame's buffer.
    fn scrollback_keys(
        &self,
        c: &mut super::Canvas,
        view: &ScrollbackView,
        layout: &ScrollbackLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let s = scale as f32;
        let (heading, rows) = scrollback_keys_spots(layout, view.keys.len());
        let heading = shift(heading);
        let (left, right) = phys(heading, scale);
        let text = "Keys in the scrollback viewer";
        let heading_band = band(heading, scale);
        self.text(c, text, FONT_SIZE, left, right, heading_band, scale, p.text);
        let (caps, gap) = (KEYS_CAPS as f32 * s, HINT_KEY_GAP as f32 * s);
        for ((keys, what), rect) in view.keys.iter().zip(rows) {
            let rect = shift(rect);
            let (left, right) = phys(rect, scale);
            let row_band = band(rect, scale);
            let end = self.keycaps(c, keys, left, row_band, scale, p);
            let x = (left + caps).max(end + gap);
            let end = right - gap;
            self.text(c, what, SMALL_FONT_SIZE, x, end, row_band, scale, p.text);
        }
    }

    /// A row of the text, `w` wide: on the terminal's background, the
    /// selected part of it on the selection's, the places the search
    /// found tinted.
    fn scrollback_row(
        &self,
        row: &ScrollbackRow,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> super::Canvas {
        let size = Size::from((w, ROW));
        let mut c = super::Canvas::new(physical(size, scale));
        c.fill(0, 0, c.w, c.h, p.terminal_background);
        let adv = self.mono_width("0", MONO_SIZE, scale);
        if let Some((from, to, on)) = row.selected {
            let x = (from as f32 * adv).round();
            let end = to as f32 + if on { 0.5 } else { 0.0 };
            let end = (end * adv).round().min(c.w as f32);
            c.fill(x as i32, 0, (end - x) as i32, c.h, selection_color(p));
        }
        let rect = Rectangle::from_size(size);
        if !row.found.is_empty() {
            let width = |t: &str| t.chars().count() as f32 * adv;
            let ground = p.terminal_background;
            let x = (0.0, c.w as f32);
            self.found_tints(
                &mut c,
                &row.text,
                &row.found,
                x,
                band(rect, scale),
                width,
                ground,
                scale,
                p,
            );
        }
        if let Some(ch) = row.caret {
            let width = |t: &str| t.chars().count() as f32 * adv;
            let x = (0.0, c.w as f32);
            let ground = p.terminal_background;
            let text = &row.text;
            self.caret_block(
                &mut c,
                text,
                ch,
                x,
                band(rect, scale),
                width,
                ground,
                scale,
                p,
            );
        }
        if row.styles.is_empty() {
            self.mono_text(
                &mut c,
                &row.text,
                MONO_SIZE,
                0.0,
                f32::MAX,
                band(rect, scale),
                scale,
                p.terminal_foreground,
            );
        } else {
            self.styled_row(&mut c, row, adv, band(rect, scale), scale, p);
        }
        if let Some(word) = &row.word {
            let width = |t: &str| t.chars().count() as f32 * adv;
            let x = (0.0, c.w as f32);
            self.word_underline(
                &mut c,
                &row.text,
                word,
                x,
                band(rect, scale),
                width,
                scale,
                p,
            );
        }
        c
    }

    /// The text of `row` in the colours, bold (in the bold monospace
    /// face) and dim of its runs, each run where its first character goes
    /// (`adv` apart), the rest in the terminal's colour.
    fn styled_row(
        &self,
        c: &mut super::Canvas,
        row: &ScrollbackRow,
        adv: f32,
        row_band: (i32, i32),
        scale: f64,
        p: &Palette,
    ) {
        let chars: Vec<(usize, char)> = row.text.char_indices().collect();
        let byte = |n: usize| chars.get(n).map_or(row.text.len(), |(b, _)| *b);
        // The runs with those drawn plainly between them.
        let mut runs = Vec::new();
        let mut at = 0;
        for (r, style) in &row.styles {
            if r.start > at {
                runs.push((at..r.start, Style::default()));
            }
            runs.push((r.clone(), *style));
            at = r.end;
        }
        if at < chars.len() {
            runs.push((at..chars.len(), Style::default()));
        }
        for (r, style) in runs {
            let text = &row.text[byte(r.start)..byte(r.end)];
            let mut color = style
                .fg
                .map_or(p.terminal_foreground, |ink| ink_color(ink, p));
            if style.dim {
                color = mix(color, p.terminal_background, DIM);
            }
            let x = r.start as f32 * adv;
            let size = MONO_SIZE;
            match style.bold {
                true if self.has_mono_bold() => {
                    self.mono_bold_text(c, text, size, x, f32::MAX, row_band, scale, color);
                }
                bold => {
                    self.mono_text(c, text, size, x, f32::MAX, row_band, scale, color);
                    // No bold face: struck again a little to the right.
                    if bold {
                        let x = x + BOLD_SHIFT * scale as f32;
                        self.mono_text(c, text, size, x, f32::MAX, row_band, scale, color);
                    }
                }
            }
        }
    }

    /// The scrollbar's thumb, of `size`.
    fn scrollback_thumb(&self, size: Size<i32, Logical>, scale: f64, p: &Palette) -> super::Canvas {
        let mut c = super::Canvas::new(physical(size, scale));
        let (w, h) = (c.w as f32, c.h as f32);
        let ground = mix(p.terminal_background, p.terminal_foreground, 0.35);
        c.rounded(0.0, 0.0, w, h, w / 2.0, ground);
        c
    }

    /// The whole viewer in one buffer, its rows and thumb drawn in, as
    /// `ScrollbackLayers` shows it: to look at in tests.
    #[cfg(test)]
    pub(super) fn scrollback(
        &self,
        view: &ScrollbackView,
        scale: f64,
        p: &Palette,
    ) -> smithay::backend::renderer::element::memory::MemoryRenderBuffer {
        let mut c = self.scrollback_frame(view, scale, p);
        let layout = scrollback_layout(view.panel, &view.hints);
        let px = |v: f64| (v * scale).round() as i32;
        let origin = view.panel.loc - Point::from((SHADOW, SHADOW));
        let list = layout.list;
        let clip = [
            px((list.loc.x - origin.x) as f64),
            px((list.loc.y + 1 - origin.y) as f64),
            px((list.loc.x + list.size.w - origin.x) as f64),
            px((list.loc.y + list.size.h - 1 - origin.y) as f64),
        ];
        if let Some(t) = scrollback_thumb(view, &layout) {
            let thumb = self.scrollback_thumb((t.track.size.w, t.h as i32).into(), scale, p);
            let (x, y) = (
                px((t.track.loc.x - origin.x) as f64),
                px(t.top - origin.y as f64),
            );
            super::inbox::blit(&mut c, &thumb, x, y, clip);
        }
        if !view.ticks.is_empty() {
            let t = track(&layout);
            let at = Rectangle::new(t.loc - origin, t.size);
            self.search_ticks(&mut c, &view.ticks, at, p.terminal_background, scale, p);
        }
        let text = layout.text;
        let top = text.loc.y as f64 - view.part * ROW as f64 + view.overscroll;
        for (i, row) in view.rows.iter().enumerate() {
            let buffer = self.scrollback_row(row, text.size.w, scale, p);
            let y = top + (i as i32 * ROW) as f64 - origin.y as f64;
            super::inbox::blit(
                &mut c,
                &buffer,
                px((text.loc.x - origin.x) as f64),
                px(y),
                clip,
            );
        }
        c.into_buffer(false)
    }
}

type FrameKey = (ScrollbackView, super::Theme, f64);
type TicksKey = (Vec<(f64, bool)>, Size<i32, Logical>, super::Theme, f64);

/// The viewer as the chrome shows it, fading in and out (see `Overlay`):
/// its frame, a buffer per row in view, the scrollbar's thumb and the
/// search's ticks over it.
#[derive(Default)]
pub(super) struct ScrollbackLayers {
    overlay: super::Overlay<FrameKey>,
    rows: super::layers::RowCache<(ScrollbackRow, i32, super::Theme, f64)>,
    thumb: super::Cached<(Size<i32, Logical>, super::Theme, f64)>,
    ticks: super::Cached<TicksKey>,
}

impl ScrollbackLayers {
    /// The parts showing the viewer at `now`, front to back.
    pub(super) fn parts(
        &mut self,
        view: Option<ScrollbackView>,
        theme: super::Theme,
        scale: f64,
        now: std::time::Instant,
        painter: &Painter,
    ) -> Vec<Part> {
        let placed = view.as_ref().map(|view| {
            let (loc, size) = super::shadowed(view.panel);
            ((frame_view(view), theme, scale), loc, size)
        });
        let (rows, thumb, ticks) = (&mut self.rows, &mut self.thumb, &mut self.ticks);
        let p = theme.palette();
        let more = |_: &FrameKey| {
            let Some(view) = &view else {
                return Vec::new();
            };
            let layout = scrollback_layout(view.panel, &view.hints);
            let mut parts = Vec::new();
            if !view.ticks.is_empty() {
                let track = track(&layout);
                let key = (view.ticks.clone(), track.size, theme, scale);
                let buffer = ticks.get(key, |(t, size, ..)| {
                    let ground = p.terminal_background;
                    painter.search_ticks_buffer(t, *size, ground, scale, p)
                });
                parts.push(Part::new(buffer, track.loc, track.size, 1.0));
            }
            if let Some(t) = scrollback_thumb(view, &layout) {
                let size = Size::from((t.track.size.w, t.h as i32));
                let buffer = thumb.get((size, theme, scale), |(size, ..)| {
                    painter.scrollback_thumb(*size, scale, p).into_buffer(false)
                });
                let at = Point::from((t.track.loc.x as f64, t.top));
                parts.push(Part::new(buffer, t.track.loc, size, 1.0).at(at));
            }
            let text = layout.text;
            // Clipped to the list's ground, inside its rounded corners.
            let clip = Rectangle::new(
                (text.loc.x, layout.list.loc.y + 1).into(),
                (text.size.w, layout.list.size.h - 2).into(),
            );
            let top = text.loc.y as f64 - view.part * ROW as f64 + view.overscroll;
            for (i, row) in view.rows.iter().enumerate() {
                let size = Size::from((text.size.w, ROW));
                let key = (row.clone(), text.size.w, theme, scale);
                let buffer = rows.get(key, size, scale, |(row, w, ..)| {
                    painter.scrollback_row(row, *w, scale, p).into_buffer(true)
                });
                let at = Point::from((text.loc.x as f64, top + (i as i32 * ROW) as f64));
                parts.push(Part::new(&buffer, text.loc, size, 1.0).at(at).clipped(clip));
            }
            parts
        };
        let parts = self.overlay.parts(
            placed,
            now,
            false,
            |(frame, ..)| painter.scrollback_frame(frame, scale, p).into_buffer(false),
            more,
        );
        self.rows.sweep();
        // Gone for good: so are its rows.
        if view.is_none() && parts.is_empty() {
            self.rows.clear();
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(total: usize, first: usize) -> ScrollbackView {
        ScrollbackView {
            panel: Rectangle::new((100, 50).into(), (600, 400).into()),
            title: "Scrollback · zsh".into(),
            note: format!("{total} lines"),
            rows: Vec::new(),
            first,
            part: 0.0,
            overscroll: 0.0,
            total,
            hints: vec![
                ("Ctrl+C".into(), "copy".into()),
                ("Esc".into(), "close".into()),
            ],
            hover: None,
            search: None,
            ticks: Vec::new(),
            keys: Vec::new(),
        }
    }

    #[test]
    fn a_point_on_the_text_is_its_row_and_character() {
        let v = view(500, 40);
        let layout = scrollback_layout(v.panel, &v.hints);
        let text = layout.text;
        let adv = advance();
        let at = |row: f64, col: f64| {
            Point::from((
                text.loc.x as f64 + col * adv,
                text.loc.y as f64 + row * ROW as f64,
            ))
        };
        assert_eq!(scrollback_point(&v, &layout, at(0.5, 3.0)), (40, 3));
        assert_eq!(scrollback_point(&v, &layout, at(2.2, 0.4)), (42, 0));
        // Above the text, a row before the first shown.
        assert_eq!(scrollback_point(&v, &layout, at(-0.5, 0.0)).0, 39);
        assert!(scrollback_edge(&layout, at(-0.5, 0.0)) < 0.0);
        assert_eq!(scrollback_edge(&layout, at(1.0, 0.0)), 0.0);
        // Scrolled half a row on, the same place is half a row further.
        let half = ScrollbackView {
            part: 0.5,
            ..v.clone()
        };
        assert_eq!(scrollback_point(&half, &layout, at(0.6, 0.0)).0, 41);
    }

    #[test]
    fn the_text_hits_and_the_hints_press_their_keys() {
        let v = view(500, 0);
        let layout = scrollback_layout(v.panel, &v.hints);
        let centre =
            |r: Rect| r.to_f64().loc + Point::from((r.size.w as f64 / 2.0, r.size.h as f64 / 2.0));
        assert_eq!(
            scrollback_hit(&v, &layout, centre(layout.text)),
            Some(ScrollbackHit::Text)
        );
        let (i, spot) = layout.hints[1];
        assert_eq!(
            scrollback_hit(&v, &layout, centre(spot)),
            Some(ScrollbackHit::Hint(i))
        );
        assert_eq!(
            scrollback_hit(&v, &layout, centre(layout.title)),
            Some(ScrollbackHit::Panel)
        );
        assert_eq!(scrollback_hit(&v, &layout, (10.0, 10.0).into()), None);
    }

    #[test]
    fn a_narrow_footer_keeps_its_last_two_hints_and_a_choice_is_a_hint_too() {
        let mut v = view(500, 0);
        v.hints = [
            ("Ctrl+C", "copy"),
            ("Enter", "copy and close"),
            ("Q", "close"),
            ("F1", "keys"),
            ("Esc / Q", "close"),
        ]
        .iter()
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .collect();
        let wide = scrollback_layout(v.panel, &v.hints);
        let order = |l: &ScrollbackLayout| l.hints.iter().map(|(i, _)| *i).collect::<Vec<_>>();
        assert_eq!(order(&wide), [0, 1, 2, 3, 4]);
        let narrow = Rectangle::new(v.panel.loc, (300, 400).into());
        let layout = scrollback_layout(narrow, &v.hints);
        let shown = order(&layout);
        assert!(shown.len() < 5 && shown.ends_with(&[3, 4]), "{shown:?}");
        assert!(shown.is_sorted());
        // All in the footer's one row.
        let footer = layout.footer;
        assert!(
            layout.hints.iter().all(|(_, r)| footer.contains(r.loc)
                && r.loc.y + r.size.h <= footer.loc.y + footer.size.h)
        );
        let (i, spot) = *wide.hints.last().unwrap();
        let centre = spot.to_f64().loc + Point::from((spot.size.w as f64 / 2.0, 4.0));
        assert_eq!(
            scrollback_hit(&v, &wide, centre),
            Some(ScrollbackHit::Hint(i))
        );
    }

    #[test]
    fn the_keys_go_down_columns_side_by_side_and_need_room() {
        let big = Rectangle::new((0, 0).into(), (1400, 900).into());
        let layout = scrollback_layout(big, &[]);
        let (heading, rows) = scrollback_keys_spots(&layout, 25);
        assert_eq!(rows.len(), 25);
        assert_eq!(
            rows[0].loc,
            (heading.loc.x, heading.loc.y + KEYS_HEADING).into()
        );
        for r in &rows {
            assert!(layout.list.contains(r.loc));
            assert!(r.loc.y + r.size.h <= layout.list.loc.y + layout.list.size.h);
        }
        // Down a column first.
        assert_eq!(rows[1].loc.x, rows[0].loc.x);
        assert!(scrollback_keys_fit(big, 25));
        // Short, they take more columns, too narrow in a small viewer.
        let short = Rectangle::new((0, 0).into(), (1600, 400).into());
        let (_, rows) = scrollback_keys_spots(&scrollback_layout(short, &[]), 25);
        let columns: std::collections::BTreeSet<i32> = rows.iter().map(|r| r.loc.x).collect();
        assert!(columns.len() > 1);
        assert!(scrollback_keys_fit(short, 25));
        let small = Rectangle::new((0, 0).into(), (600, 300).into());
        assert!(!scrollback_keys_fit(small, 25));
    }

    #[test]
    fn the_search_sits_in_the_title_row_and_takes_the_field_spot() {
        let closed = view(500, 0);
        let layout = scrollback_layout(closed.panel, &closed.hints);
        let centre =
            |r: Rect| r.to_f64().loc + Point::from((r.size.w as f64 / 2.0, r.size.h as f64 / 2.0));
        let (parts, count) = scrollback_search(&closed, &layout);
        assert_eq!(parts.len(), 1);
        assert!(count.is_none());
        let (part, open) = parts[0];
        assert_eq!(part, SearchPart::Open);
        assert!(layout.title.contains(open.loc));
        assert_eq!(
            scrollback_hit(&closed, &layout, centre(open)),
            Some(ScrollbackHit::Search(SearchPart::Open))
        );
        assert!(scrollback_field(&closed, 1.0).is_none());
        let bar = SearchBar {
            focused: true,
            ..Default::default()
        };
        let searching = ScrollbackView {
            search: Some(bar),
            ..closed.clone()
        };
        let (parts, _) = scrollback_search(&searching, &layout);
        let order: Vec<SearchPart> = parts.iter().map(|p| p.0).collect();
        use SearchPart::*;
        assert_eq!(order, [Field, Prev, Next, Close]);
        for (part, rect) in &parts {
            assert!(layout.title.contains(rect.loc));
            let hit = scrollback_hit(&searching, &layout, centre(*rect));
            assert_eq!(hit, Some(ScrollbackHit::Search(*part)));
        }
        let field = scrollback_field(&searching, 1.0).unwrap();
        assert_eq!(field.area, parts[0].1);
        // Its text starts after the magnifier, as `search_bar` paints it.
        assert_eq!(field.text, search_field_text(field.area, 1.0));
        // Clicked away, the field leaves the keyboard and the caret.
        let mut blurred = searching.clone();
        blurred.search.as_mut().unwrap().focused = false;
        assert!(scrollback_field(&blurred, 1.0).is_none());
    }

    /// With `GERDR_DUMP_DIR` set, write the viewer there as images to look
    /// at (both themes, scale 1 and 2).
    #[test]
    fn dump_scrollback() {
        use crate::chrome::Theme;
        let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let painter = Painter::new();
        let lines = [
            "$ python app.py",
            "Traceback (most recent call last):",
            "  File \"/srv/app/app.py\", line 12, in <module>",
            "    main()",
            "  File \"/srv/app/app.py\", line 8, in main",
            "    raise ValueError(\"bad input: ❯ ⏺ äöü\")",
            "ValueError: bad input",
            "",
            "❯ ",
        ];
        let ink = |fg: Ink, bold: bool, dim: bool| Style {
            fg: Some(fg),
            bold,
            dim,
        };
        let dim = Style {
            dim: true,
            ..Default::default()
        };
        let mut v = view(400, 120);
        v.rows = (0..30)
            .map(|i| ScrollbackRow {
                text: lines[i % lines.len()].into(),
                selected: match i {
                    2 => Some((7, 22, false)),
                    3..=6 => Some((0, lines[i].chars().count(), true)),
                    7 => Some((0, 3, false)),
                    _ => None,
                },
                found: match i % lines.len() {
                    2 | 4 => vec![(17..23, i == 4)],
                    _ => Vec::new(),
                },
                word: (i == 1).then_some(2..6),
                caret: (i == 1).then_some(3),
                styles: match i % lines.len() {
                    0 => vec![
                        (0..1, ink(Ink::Index(2), true, false)),
                        (2..8, ink(Ink::Index(208), false, false)),
                    ],
                    1 => vec![(0..9, ink(Ink::Index(1), true, false))],
                    2 | 4 => vec![(2..6, dim), (8..22, ink(Ink::Index(4), false, false))],
                    6 => vec![(0..10, ink(Ink::Rgb([0xe0, 0x6c, 0x75]), true, false))],
                    8 => vec![(0..1, ink(Ink::Index(13), false, true))],
                    _ => Vec::new(),
                },
            })
            .collect();
        v.part = 0.4;
        v.hover = Some(ScrollbackHit::Hint(1));
        v.search = Some(SearchBar {
            query: crate::text_field::Line {
                text: "app.py".into(),
                caret: 6,
                anchor: 6,
                scroll: 0,
            },
            placeholder: "Find in the scrollback".into(),
            count: "2 of 9".into(),
            found: true,
            focused: true,
            whole: true,
        });
        v.ticks = vec![(0.1, false), (0.32, true), (0.33, false), (0.8, false)];
        // The keys listed, in place of the text.
        let keys = ScrollbackView {
            panel: Rectangle::new((100, 50).into(), (1100, 480).into()),
            rows: Vec::new(),
            ticks: Vec::new(),
            total: 0,
            search: None,
            keys: [
                ("Arrows / H J K L", "Move the caret"),
                ("Ctrl+U / Ctrl+D", "Go half a page up or down"),
                ("/ / Ctrl+F", "Find"),
                ("Double Click / Triple Click", "Select a word, or a line"),
                ("Esc", "Clear the selection or the search, then close"),
            ]
            .repeat(5)
            .iter()
            .map(|(k, w)| (k.to_string(), w.to_string()))
            .collect(),
            ..v.clone()
        };
        for (view, name, theme, scale) in [
            (&v, "scrollback", Theme::Dark, 1.0),
            (&v, "scrollback", Theme::Light, 1.0),
            (&v, "scrollback", Theme::Dark, 2.0),
            (&v, "scrollback", Theme::Light, 2.0),
            (&keys, "scrollback_keys", Theme::Dark, 1.0),
            (&keys, "scrollback_keys", Theme::Light, 2.0),
        ] {
            let v = view;
            let w = ((v.panel.size.w + 2 * SHADOW) as f64 * scale).round() as usize;
            let mut buffer = painter.scrollback(v, scale, theme.palette());
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
            let file = format!("{name}_{}_{scale}.ppm", theme.name());
            let _ = std::fs::write(dir.join(file), out);
        }
    }

    #[test]
    fn the_thumb_shows_only_for_more_than_fits_and_goes_down_with_the_list() {
        let v = view(10, 0);
        let layout = scrollback_layout(v.panel, &v.hints);
        assert!(scrollback_thumb(&v, &layout).is_none());
        let top = scrollback_thumb(&view(500, 0), &layout).unwrap();
        let fit = scrollback_fit(v.panel);
        let bottom = scrollback_thumb(&view(500, 500 - fit.page), &layout).unwrap();
        assert_eq!(top.top, top.track.loc.y as f64);
        assert_eq!(
            bottom.top + bottom.h,
            (bottom.track.loc.y + bottom.track.size.h) as f64
        );
        assert_eq!(top.last, (500 - fit.page) as f64);
    }
}
