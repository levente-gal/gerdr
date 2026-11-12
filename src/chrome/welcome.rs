//! The welcome screen covering everything while no project is shown.

use super::{
    BUTTON_RADIUS, CARET_WIDTH, Canvas, FONT_SIZE, FieldSpot, GREETING_FONT_SIZE, GREETING_HEIGHT,
    HELP_MARGIN, HINTS_HEIGHT, LABEL_FONT_SIZE, ListBar, NOTE_GAP, NOTE_HEIGHT, PADDING, Painter,
    Palette, ROW_RADIUS, Rect, SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, SUBTITLE_HEIGHT, Thumb,
    WELCOME_BUTTON, WELCOME_BUTTON_WIDTH, WELCOME_GAP, WELCOME_LABEL, WELCOME_MIDDLE, WELCOME_ROW,
    WELCOME_ROWS, WELCOME_WIDTH, band, coverage, field_text, focus_ring, gutter_track, hint_at,
    hint_spots, hint_strs, icon_sdf, mix, phys, physical, plus, rect_px,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// A project card: its name over its path and age (a row and the second
/// line), and the gap to the next.
pub(super) const WELCOME_CARD: i32 = WELCOME_ROW + 12;
pub(super) const WELCOME_CARD_GAP: i32 = 6;
/// The gap between the button (or the note under it) and the key hints.
pub(super) const HINTS_GAP: i32 = 24;
/// Where a card's two lines sit: the name's band and the detail's, from
/// the card's top.
const CARD_NAME_TOP: i32 = 10;
const CARD_NAME_HEIGHT: i32 = 22;
const CARD_DETAIL_TOP: i32 = 32;
const CARD_DETAIL_HEIGHT: i32 = 18;
/// How far right of the cards their scrollbar stands, besides the gutter's
/// own gap.
const WELCOME_BAR_GAP: i32 = 4;

/// What the welcome screen shows: a greeting, the projects, and the button
/// to start a new one.
#[derive(Clone, Debug, PartialEq)]
pub struct WelcomeView {
    /// "Good evening, lgal".
    pub greeting: String,
    pub subtitle: String,
    /// What was typed to filter the projects.
    pub query: Line,
    /// The projects in view, most recently used first.
    pub rows: Vec<ProjectRow>,
    /// Per row, the characters of "name detail" the query matched.
    pub hits: Vec<Vec<usize>>,
    /// The selected row; `None` when nothing matches or the button is.
    pub selected: Option<usize>,
    pub hover: Option<WelcomeHit>,
    /// How far the hover pill has faded in, 0 to 16.
    pub hover_alpha: u8,
    /// Set while naming a new project: the name typed so far.
    pub naming: Option<Line>,
    /// The row asking to be confirmed for forgetting.
    pub confirm_forget: Option<usize>,
    /// A line under the button, such as "herdr isn't installed".
    pub note: Option<String>,
    /// Opened over a project, which Esc goes back to.
    pub in_app: bool,
    /// The keys that quit gerdr ("Ctrl+B q"), for the bar at the bottom.
    pub quit_keys: Option<String>,
    /// Where the list stands, in rows, while not all projects show: its
    /// scrollbar says so.
    pub scroll: Option<ListBar>,
}

/// A project on the welcome screen.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    /// "3 workspaces · ~/Work/aft".
    pub detail: String,
    /// "2h ago".
    pub ago: String,
    /// Whether its session runs; `None` until known.
    pub running: Option<bool>,
    /// It is the project shown behind the welcome screen.
    pub current: bool,
}

/// What the pointer is over on the welcome screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeHit {
    Row(usize),
    Button,
    /// A key hint under the button, by index into `welcome_hints`.
    Hint(usize),
    /// The quit bar's key hint.
    Quit,
}

/// Where the parts of the welcome screen go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WelcomeLayout {
    pub greeting: Rect,
    pub subtitle: Rect,
    /// The "Recent projects" label over the list.
    pub label: Rect,
    pub list: Rect,
    pub rows: Vec<Rect>,
    /// The "New project" button, or the name field while naming.
    pub button: Rect,
    /// The note under the button; of no height without one.
    pub note: Rect,
    /// The key hints, under the button or its note.
    pub hints: Rect,
    /// The bar at the bottom with the keys that quit.
    pub bar: Rect,
}

/// What the welcome screen makes room for.
#[derive(Clone, Copy, Debug, Default)]
pub struct WelcomeParts {
    /// The projects in view.
    pub rows: usize,
    /// The name field takes the button's place.
    pub naming: bool,
    /// A note sits under the button.
    pub note: bool,
}

impl WelcomeParts {
    /// What `view` shows.
    pub fn of(view: &WelcomeView) -> Self {
        Self {
            rows: view.rows.len(),
            naming: view.naming.is_some(),
            note: view.note.is_some(),
        }
    }
}

/// Where the welcome screen's parts go on an output of `size` with
/// `parts`: a column centred on it, its middle at `WELCOME_MIDDLE` of the
/// height as far as it fits, the quit bar at the bottom. While naming, the
/// name field takes the button's place, as wide as the column. With a
/// note, it sits between the button and the hints.
pub fn welcome_layout(size: Size<i32, Logical>, parts: WelcomeParts) -> WelcomeLayout {
    let WelcomeParts { rows, naming, note } = parts;
    let w = WELCOME_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let x = (size.w - w) / 2;
    let rect = |y: i32, h: i32| Rectangle::new((x, y).into(), (w, h).into());
    let bar = rect((size.h - HELP_MARGIN - HINTS_HEIGHT).max(0), HINTS_HEIGHT);
    let list_h = list_height(rows);
    let note_h = if note { NOTE_GAP + NOTE_HEIGHT } else { 0 };
    let block = WELCOME_FIXED - (NOTE_GAP + NOTE_HEIGHT) + note_h + list_h;
    let middle = (size.h as f64 * WELCOME_MIDDLE).round() as i32;
    let top = (middle - block / 2)
        .min(bar.loc.y - HELP_MARGIN - block)
        .max(HELP_MARGIN.min(size.h / 2));

    let greeting = rect(top, GREETING_HEIGHT);
    let subtitle = rect(greeting.loc.y + GREETING_HEIGHT, SUBTITLE_HEIGHT);
    let label = rect(
        subtitle.loc.y + SUBTITLE_HEIGHT + WELCOME_GAP,
        WELCOME_LABEL,
    );
    let list = rect(label.loc.y + WELCOME_LABEL, list_h);
    let row_rects = (0..rows as i32)
        .map(|i| {
            rect(
                list.loc.y + i * (WELCOME_CARD + WELCOME_CARD_GAP),
                WELCOME_CARD,
            )
        })
        .collect();
    let button_y = list.loc.y + list_h + WELCOME_GAP;
    let button = if naming {
        rect(button_y, WELCOME_BUTTON)
    } else {
        let bw = WELCOME_BUTTON_WIDTH.min(w);
        Rectangle::new(
            ((size.w - bw) / 2, button_y).into(),
            (bw, WELCOME_BUTTON).into(),
        )
    };
    let note = rect(
        button_y + WELCOME_BUTTON + if note { NOTE_GAP } else { 0 },
        if note { NOTE_HEIGHT } else { 0 },
    );
    let hints = rect(note.loc.y + note.size.h + HINTS_GAP, HINTS_HEIGHT);
    WelcomeLayout {
        greeting,
        subtitle,
        label,
        list,
        rows: row_rects,
        button,
        note,
        hints,
        bar,
    }
}

/// The key hints under the welcome screen's button for `view`.
pub fn welcome_hints(view: &WelcomeView) -> Vec<(String, String)> {
    crate::welcome::hints(view.naming.is_some(), view.in_app)
}

/// The key hints of the welcome screen showing `view` with its parts
/// where `layout` has them: those under the button by index into
/// `welcome_hints`, and the quit bar's.
fn hint_layout(view: &WelcomeView, layout: &WelcomeLayout) -> (Vec<(usize, Rect)>, Option<Rect>) {
    let hints = hint_spots(&hint_strs(&welcome_hints(view)), layout.hints);
    let quit = view.quit_keys.as_deref().and_then(|keys| {
        hint_spots(&[(keys, "quit")], layout.bar)
            .first()
            .map(|(_, r)| *r)
    });
    (hints, quit)
}

/// The key hint of the welcome screen showing `view` on an output of
/// `size` under `pos`, if any.
pub fn welcome_hint(
    view: &WelcomeView,
    size: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<WelcomeHit> {
    let layout = welcome_layout(size, WelcomeParts::of(view));
    let (hints, quit) = hint_layout(view, &layout);
    if let Some(i) = hint_at(&hint_strs(&welcome_hints(view)), &hints, pos) {
        return Some(WelcomeHit::Hint(i));
    }
    quit.filter(|r| r.to_f64().contains(pos))
        .map(|_| WelcomeHit::Quit)
}

/// Where the mark of the project card in `row` goes, which says whether its
/// session runs.
pub fn welcome_mark(row: Rect) -> Rect {
    Rectangle::new(
        (row.loc.x + PADDING as i32, row.loc.y).into(),
        (STATUS_WIDTH, row.size.h).into(),
    )
}

/// What a project's mark says, by whether its session runs (`None` until
/// known).
pub fn running_label(running: Option<bool>) -> &'static str {
    match running {
        Some(true) => "Running",
        Some(false) => "Not running",
        None => "Finding out whether it runs",
    }
}

/// The scrollbar right of the welcome screen's cards in `layout`, while
/// not all of `view`'s projects show.
pub fn welcome_thumb(view: &WelcomeView, layout: &WelcomeLayout) -> Option<Thumb> {
    let list = layout.list;
    let track = gutter_track(list.loc.x + list.size.w + WELCOME_BAR_GAP, list);
    Thumb::of(track, view.rows.len() as f64, view.scroll?)
}

/// How high the list of `rows` cards is.
fn list_height(rows: usize) -> i32 {
    let rows = rows as i32;
    rows * WELCOME_CARD + (rows - 1).max(0) * WELCOME_CARD_GAP
}

/// The welcome screen's height above the quit bar without its rows, with
/// a note.
pub(super) const WELCOME_FIXED: i32 = GREETING_HEIGHT
    + SUBTITLE_HEIGHT
    + WELCOME_GAP
    + WELCOME_LABEL
    + WELCOME_GAP
    + WELCOME_BUTTON
    + NOTE_GAP
    + NOTE_HEIGHT
    + HINTS_GAP
    + HINTS_HEIGHT;

/// How many project rows fit on the welcome screen of an output of `size`.
pub fn welcome_page(size: Size<i32, Logical>) -> usize {
    let room = size.h - WELCOME_FIXED - 3 * HELP_MARGIN - HINTS_HEIGHT;
    ((room + WELCOME_CARD_GAP) / (WELCOME_CARD + WELCOME_CARD_GAP)).clamp(1, WELCOME_ROWS) as usize
}

/// The welcome screen's text field with the keyboard, on an output of
/// `size`: the new project's name, while asked for. (The filter, typed
/// over the list, is no field to click in.)
pub fn welcome_field(
    view: &WelcomeView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let line = view.naming.clone()?;
    let area = welcome_layout(size, WelcomeParts::of(view)).button;
    Some(FieldSpot {
        size: FONT_SIZE,
        area,
        text: field_text(area, scale),
        line,
    })
}

/// The label over the welcome screen's list: what stands in for the list
/// without rows, else the heading.
pub fn welcome_label(view: &WelcomeView) -> &'static str {
    match (view.rows.is_empty(), view.query.is_empty()) {
        (true, true) => "No projects yet",
        (true, false) => "Nothing matches",
        (false, _) => "RECENT PROJECTS",
    }
}

impl Painter {
    /// The welcome screen over the whole output of `size`, opaque in the
    /// editor's background: the greeting, the projects as cards, the button
    /// (or the name field while naming), a note, the key hints under them
    /// and the quit bar at the bottom.
    pub(super) fn welcome(
        &self,
        view: &WelcomeView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        c.fill(0, 0, c.w, c.h, p.tab_active);
        let layout = welcome_layout(size, WelcomeParts::of(view));

        self.centered(
            &mut c,
            &view.greeting,
            GREETING_FONT_SIZE,
            layout.greeting,
            scale,
            p.text,
        );
        self.centered(
            &mut c,
            &view.subtitle,
            FONT_SIZE,
            layout.subtitle,
            scale,
            p.muted,
        );

        // The label over the list, centred in the column; the filter typed
        // so far, with its caret, at its right.
        let label = welcome_label(view);
        let label_size = if view.rows.is_empty() {
            FONT_SIZE
        } else {
            SMALL_FONT_SIZE
        };
        self.centered(
            &mut c,
            label,
            label_size,
            layout.label,
            scale,
            p.placeholder,
        );
        if !view.query.is_empty() && view.naming.is_none() {
            let (left, right) = phys(layout.label, scale);
            let label_end = (left + right + self.text_width(label, label_size, scale)) / 2.0;
            let right = right - PADDING * s;
            let end = right - (px(CARET_WIDTH).max(1) + px(1)) as f32;
            let width = self.text_width(&view.query, FONT_SIZE, scale);
            let start = (end - width).max(label_end + px(16) as f32);
            self.edit_line(
                &mut c,
                &view.query,
                "",
                (start, end),
                band(layout.label, scale),
                true,
                FONT_SIZE,
                scale,
                p,
            );
        }

        for (i, (rect, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            let hits = view.hits.get(i).map_or(&[][..], Vec::as_slice);
            self.project_row(&mut c, view, i, row, hits, *rect, scale, p);
        }
        if let Some(thumb) = welcome_thumb(view, &layout) {
            self.thumb_in(&mut c, &thumb, scale, p);
        }

        let button = layout.button;
        match &view.naming {
            Some(name) => self.field(&mut c, name, "Project name", button, true, scale, p),
            None => {
                let (x, y, w, h) = rect_px(button, scale);
                if view.selected.is_none() {
                    focus_ring(&mut c, button, BUTTON_RADIUS, p.tab_active, scale, p.accent);
                }
                let fill = if view.hover == Some(WelcomeHit::Button) {
                    mix(p.accent, p.text, 0.12 * view.hover_alpha as f32 / 16.0)
                } else {
                    p.accent
                };
                c.rounded(x, y, w, h, BUTTON_RADIUS * s, fill);
                // A "+" and the label, centred together.
                let label = "New project";
                let (arm, gap) = ((5.0 * s).round(), (8.0 * s).round());
                let content = 2.0 * arm + gap + self.text_width(label, FONT_SIZE, scale);
                let start = (x + (w - content) / 2.0).round();
                let (cx, cy) = (start + arm, y + h / 2.0);
                let reach = (arm + 2.0 * s).ceil() as i32;
                c.paint_in(
                    cx as i32 - reach,
                    cy as i32 - reach,
                    2 * reach + 1,
                    2 * reach + 1,
                    p.on_accent,
                    |x, y| coverage(plus(x, y, cx, cy, arm, s) - 0.2 * s),
                );
                self.text(
                    &mut c,
                    label,
                    FONT_SIZE,
                    start + 2.0 * arm + gap,
                    x + w,
                    band(button, scale),
                    scale,
                    p.on_accent,
                );
            }
        }

        if let Some(note) = &view.note {
            self.centered(&mut c, note, SMALL_FONT_SIZE, layout.note, scale, p.warning);
        }

        let (spots, quit) = hint_layout(view, &layout);
        let hover = match view.hover {
            Some(WelcomeHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(
            &mut c,
            &hint_strs(&welcome_hints(view)),
            &spots,
            hover,
            scale,
            p,
        );
        if let (Some(keys), Some(quit)) = (&view.quit_keys, quit) {
            let hover = (view.hover == Some(WelcomeHit::Quit)).then_some(0);
            self.hints(&mut c, &[(keys, "quit")], &[(0, quit)], hover, scale, p);
        }
        c.into_buffer(true)
    }

    /// Row `i` of the welcome screen, a card in `rect`: a mark for whether
    /// its session runs, its name with the characters at `hits` lit, and
    /// under it its detail and how long ago it was used; or the question
    /// whether to forget it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn project_row(
        &self,
        c: &mut Canvas,
        view: &WelcomeView,
        i: usize,
        row: &ProjectRow,
        hits: &[usize],
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, ROW_RADIUS * s);
        let mark = welcome_mark(rect);
        let name_x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let right = x + w - PADDING * s;
        let dot = move |x: f32, y: f32| x.hypot(y) - 4.2 * s;
        let confirm = view.confirm_forget == Some(i);
        let hovered = view.hover == Some(WelcomeHit::Row(i));
        // The card: a fill in its border, lit while selected, lifting
        // under the pointer.
        let (edge, fill) = if confirm {
            (p.border, mix(p.tab_active, p.warning, 0.15))
        } else if view.selected == Some(i) {
            (p.border, p.selected)
        } else if hovered {
            let t = 0.6 * view.hover_alpha as f32 / 16.0;
            (p.border_variant, mix(p.panel, p.selected, t))
        } else {
            (p.border_variant, p.panel)
        };
        c.rounded(x, y, w, h, r, edge);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        if confirm {
            icon_sdf(c, mark, scale, p.warning, dot);
            self.text(
                c,
                &format!("Press Delete again to forget {}", row.name),
                FONT_SIZE,
                name_x,
                right,
                band(rect, scale),
                scale,
                p.warning,
            );
            return;
        }

        // A dot while its session runs, a ring while it doesn't, a fainter
        // ring until that is known.
        let ring = move |x: f32, y: f32| (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
        match row.running {
            Some(true) => icon_sdf(c, mark, scale, p.success, dot),
            Some(false) => icon_sdf(c, mark, scale, p.placeholder, ring),
            None => icon_sdf(c, mark, scale, p.border, ring),
        }

        // The hits are over "name detail": split between the two lines.
        let name_len = row.name.chars().count();
        let name_hits: Vec<usize> = hits.iter().copied().filter(|&h| h < name_len).collect();
        let detail_hits: Vec<usize> = hits
            .iter()
            .filter_map(|&h| h.checked_sub(name_len + 1))
            .collect();
        let top = px(rect.loc.y);
        let name_band = (top + px(CARD_NAME_TOP), px(CARD_NAME_HEIGHT));
        let detail_band = (top + px(CARD_DETAIL_TOP), px(CARD_DETAIL_HEIGHT));
        let after = self.marked(
            c,
            &row.name,
            &name_hits,
            LABEL_FONT_SIZE,
            name_x,
            right,
            name_band,
            scale,
            (p.text, p.accent),
        );
        if row.current {
            self.text(
                c,
                "current",
                SMALL_FONT_SIZE,
                after + (8.0 * s).round(),
                right,
                name_band,
                scale,
                p.accent,
            );
        }
        let after = self.marked(
            c,
            &row.detail,
            &detail_hits,
            SMALL_FONT_SIZE,
            name_x,
            right,
            detail_band,
            scale,
            (p.muted, p.accent),
        );
        if !row.ago.is_empty() {
            self.text(
                c,
                &format!(" · {}", row.ago),
                SMALL_FONT_SIZE,
                after,
                right,
                detail_band,
                scale,
                p.muted,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HINTS_GAP, ListBar, ProjectRow, WELCOME_CARD, WELCOME_CARD_GAP, WelcomeHit, WelcomeParts,
        WelcomeView, running_label, welcome_label, welcome_layout, welcome_mark, welcome_page,
        welcome_thumb,
    };
    use crate::chrome::{
        Painter, QuitHit, QuitRow, QuitView, Rect, TabStripView, TabView, Theme, ThemeMode, inside,
    };
    use crate::text_field::Line;
    use smithay::utils::{Logical, Size};

    /// What a welcome screen with `rows` projects in view and nothing else
    /// makes room for.
    fn parts(rows: usize) -> WelcomeParts {
        WelcomeParts {
            rows,
            ..WelcomeParts::default()
        }
    }

    fn row(name: &str, running: Option<bool>, current: bool) -> ProjectRow {
        ProjectRow {
            name: name.into(),
            detail: "3 workspaces · ~/Work/aft".into(),
            ago: "2h ago".into(),
            running,
            current,
        }
    }

    fn view() -> WelcomeView {
        WelcomeView {
            greeting: "Good evening, lgal".into(),
            subtitle: "Pick up where you left off".into(),
            query: "af".into(),
            rows: vec![
                row("aft", Some(true), true),
                row(
                    "a project with a very long name indeed, longer than the row",
                    Some(false),
                    false,
                ),
                row("gerdr", None, false),
            ],
            hits: vec![vec![0, 1], vec![2, 4], vec![14, 15]],
            selected: Some(1),
            hover: Some(WelcomeHit::Row(2)),
            hover_alpha: 8,
            naming: None,
            confirm_forget: Some(2),
            note: Some("herdr isn't installed".into()),
            in_app: true,
            quit_keys: Some("Ctrl+B q".into()),
            scroll: None,
        }
    }

    #[test]
    fn a_cards_mark_is_on_it_and_says_whether_it_runs() {
        let layout = welcome_layout((1280, 800).into(), parts(3));
        for row in &layout.rows {
            let mark = welcome_mark(*row);
            assert!(inside(*row, mark));
        }
        assert_eq!(running_label(Some(true)), "Running");
        assert_eq!(running_label(Some(false)), "Not running");
    }

    #[test]
    fn welcome_layout_fits_the_output() {
        for (w, h) in [(800, 600), (1920, 1080), (1280, 720)] {
            let size: Size<i32, Logical> = (w, h).into();
            let output = Rect::from_size(size);
            let rows = welcome_page(size);
            for (naming, note) in [(false, false), (true, true), (false, true)] {
                let l = welcome_layout(size, WelcomeParts { rows, naming, note });
                for part in [
                    l.greeting, l.subtitle, l.label, l.list, l.button, l.note, l.hints, l.bar,
                ] {
                    assert!(inside(output, part), "{part:?} outside {w}x{h}");
                }
                assert_eq!(l.rows.len(), rows);
                assert!(l.rows.iter().all(|r| inside(l.list, *r)));
                assert!(
                    l.rows
                        .windows(2)
                        .all(|r| r[0].loc.y + r[0].size.h + WELCOME_CARD_GAP == r[1].loc.y)
                );
                assert!(l.rows.iter().all(|r| r.size.h == WELCOME_CARD));
                assert!(l.subtitle.loc.y >= l.greeting.loc.y + l.greeting.size.h);
                assert!(l.label.loc.y + l.label.size.h <= l.list.loc.y);
                assert!(l.button.loc.y >= l.list.loc.y + l.list.size.h);
                assert!(l.note.loc.y >= l.button.loc.y + l.button.size.h);
                assert!(l.hints.loc.y >= l.note.loc.y + l.note.size.h);
                assert!(l.bar.loc.y >= l.hints.loc.y + l.hints.size.h);
                // The bar sits at the bottom; the column is centred.
                assert!(h - (l.bar.loc.y + l.bar.size.h) <= 24);
                assert_eq!(l.list.loc.x, w - (l.list.loc.x + l.list.size.w));
            }
        }
    }

    #[test]
    fn the_label_is_centred_in_the_column() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = welcome_layout(size, parts(3));
        assert_eq!(l.label.loc.x, 1920 - (l.label.loc.x + l.label.size.w));
        assert_eq!(
            (l.label.loc.x, l.label.size.w),
            (l.list.loc.x, l.list.size.w)
        );
        assert_eq!(l.label.size.w, 560);
        let mut v = view();
        assert_eq!(welcome_label(&v), "RECENT PROJECTS");
        v.rows.clear();
        assert_eq!(welcome_label(&v), "Nothing matches");
        v.query = Line::default();
        assert_eq!(welcome_label(&v), "No projects yet");
    }

    #[test]
    fn the_hints_sit_a_gap_under_the_button_or_its_note() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let plain = welcome_layout(size, parts(2));
        assert_eq!(plain.note.size.h, 0);
        assert_eq!(
            plain.hints.loc.y,
            plain.button.loc.y + plain.button.size.h + HINTS_GAP
        );
        let noted = welcome_layout(
            size,
            WelcomeParts {
                note: true,
                ..parts(2)
            },
        );
        assert!(noted.note.size.h > 0);
        assert!(noted.note.loc.y > noted.button.loc.y + noted.button.size.h);
        assert_eq!(
            noted.hints.loc.y,
            noted.note.loc.y + noted.note.size.h + HINTS_GAP
        );
        assert_eq!(noted.hints.loc.x, plain.hints.loc.x);
        // Only the quit bar stays at the bottom.
        assert_eq!(noted.bar.loc.y + noted.bar.size.h, 800 - 16);
        assert!(noted.hints.loc.y + noted.hints.size.h < noted.bar.loc.y);
    }

    #[test]
    fn welcome_layout_without_rows_keeps_the_label() {
        let size: Size<i32, Logical> = (800, 600).into();
        let l = welcome_layout(size, parts(0));
        assert!(l.rows.is_empty());
        assert_eq!(l.list.size.h, 0);
        assert!(l.label.size.h > 0);
        assert!(l.button.loc.y >= l.label.loc.y + l.label.size.h);
    }

    #[test]
    fn naming_widens_the_button_to_the_column() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let button = welcome_layout(size, parts(3)).button;
        let field = welcome_layout(
            size,
            WelcomeParts {
                naming: true,
                ..parts(3)
            },
        )
        .button;
        let list = welcome_layout(
            size,
            WelcomeParts {
                naming: true,
                ..parts(3)
            },
        )
        .list;
        assert!(field.size.w > button.size.w);
        assert_eq!((field.loc.x, field.size.w), (list.loc.x, list.size.w));
        assert_eq!(field.loc.y, button.loc.y);
        // The button is centred on the output.
        assert_eq!(button.loc.x, 1920 - (button.loc.x + button.size.w));
    }

    #[test]
    fn welcome_page_grows_with_the_height() {
        let mut last = 0;
        for h in (100..2000).step_by(10) {
            let rows = welcome_page((1000, h).into());
            assert!(rows >= 1 && rows >= last);
            last = rows;
        }
        assert_eq!(welcome_page((1000, 100).into()), 1);
        assert!(last > 1);
    }

    #[test]
    fn welcome_and_quit_paint() {
        let painter = Painter::new();
        let mut view = view();
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for (w, h) in [(800, 600), (1920, 1080), (60, 40)] {
                    let size = (w, h).into();
                    painter.welcome(&view, size, scale, theme.palette());
                    let quit = QuitView {
                        project: "aft".into(),
                        rows: vec![QuitRow {
                            label: "Don't ask again".into(),
                            detail: String::new(),
                            checked: scale > 1.0,
                        }],
                        cursor: 0,
                        selected: 1,
                        hover: Some(QuitHit::Button(1)),
                        switch_to: None,
                    };
                    painter.quit_dialog(&quit, size, scale, theme.palette());
                }
            }
        }
        // Scrolled, its scrollbar shows.
        view.scroll = Some(ListBar {
            down: 2.0,
            total: 9.0,
        });
        painter.welcome(&view, (800, 600).into(), 1.0, Theme::Light.palette());
        view.scroll = None;
        view.naming = Some(Line::default());
        view.rows.clear();
        view.hits.clear();
        view.selected = None;
        view.note = None;
        painter.welcome(&view, (800, 600).into(), 1.5, Theme::Dark.palette());
        view.naming = None;
        view.hover = Some(WelcomeHit::Button);
        view.quit_keys = None;
        painter.welcome(&view, (800, 600).into(), 1.0, Theme::Light.palette());

        let tabs = [
            TabView {
                title: "zsh".into(),
                activity: "cargo watch".into(),
                restart: Some(false),
                ..Default::default()
            },
            TabView::default(),
        ];
        let strip = TabStripView::plain(tabs.to_vec(), 0, false, ThemeMode::Dark);
        painter.tab_strip_view(&strip, Theme::Dark, (900, 48).into(), 1.25);
    }

    #[test]
    fn the_scrollbar_stands_right_of_the_cards_while_not_all_show() {
        let mut v = view();
        let layout = welcome_layout(
            (1280, 800).into(),
            WelcomeParts {
                note: true,
                ..parts(v.rows.len())
            },
        );
        assert_eq!(welcome_thumb(&v, &layout), None);
        v.scroll = Some(ListBar {
            down: 6.0,
            total: 9.0,
        });
        let thumb = welcome_thumb(&v, &layout).expect("a scrollbar");
        let list = layout.list;
        assert!(thumb.track.loc.x >= list.loc.x + list.size.w);
        assert_eq!(thumb.track.loc.y, list.loc.y);
        // At the end of the list, the thumb is at the end of the track.
        let end = (thumb.track.loc.y + thumb.track.size.h) as f64;
        assert_eq!(thumb.top + thumb.h, end);
    }
}
