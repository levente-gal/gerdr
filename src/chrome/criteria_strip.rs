//! The criteria strip over a sandbox's app (see `crate::criteria_strip`):
//! a feature's acceptance criteria, one at a time, to try in the app and mark
//! passed or failed there, without going back to the feature tab. It floats at
//! the bottom of the pane, or folds to a chip in its corner.
//!
//! Its one row: ‹ › to step through the criteria, the criterion's dot, id and
//! title (a click shows its steps), the buttons its card on the page has, Page
//! and the fold button. Above the row, while unfolded, its steps, what went
//! wrong, and the note being written with the screenshot button.

use super::{
    BUTTON_RADIUS, ButtonStyle, Canvas, FOCUS_RING, FONT_SIZE, FieldSpot, POPUP_RADIUS, Painter,
    Palette, QUIT_BUTTON, ROW_BUTTON, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, band, button_width,
    chevron_down, chevron_side, field_lines_height, field_text, icon_sdf, measure, rect_px,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle};

/// How far the strip stays in from its pane's sides and bottom.
const MARGIN: i32 = 12;
/// How wide it gets at most.
const MAX_WIDTH: i32 = 760;
/// The row of its buttons, and the room inside its edge.
const ROW: i32 = 40;
const INSET: i32 = 8;
/// The gap between buttons, and before the ones at the right end.
const GAP: i32 = 6;
const GROUP_GAP: i32 = 12;
/// A line of its steps or note.
const LINE: i32 = 20;
/// The room for the dot before the criterion's id.
const DOT_ROOM: i32 = 14;
/// The folded chip: its height, and the room for its chevron.
const CHIP: i32 = 30;
const CHIP_ICON: i32 = 18;
/// How many lines each step wraps to at most.
const STEP_LINES: usize = 2;

/// How the criterion shown went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StripTag {
    NotRun,
    Passed,
    Failed,
}

impl StripTag {
    fn color(self, p: &Palette) -> Rgb {
        match self {
            Self::NotRun => p.muted,
            Self::Passed => p.success,
            Self::Failed => p.error,
        }
    }
}

/// What the strip shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CriteriaStripView {
    /// The pane it floats over, in output coordinates.
    pub pane: Rect,
    /// Folded to its chip.
    pub folded: bool,
    /// Its steps show.
    pub open: bool,
    /// "AC-3 · 3/7".
    pub place: String,
    pub title: String,
    pub tag: StripTag,
    /// "fix made: try again", "sent to Build", or nothing.
    pub state: String,
    /// Its Given, When and Then, a line each.
    pub steps: Vec<String>,
    /// What went wrong, as saved, while no note is written.
    pub note: String,
    /// The note being written.
    pub field: Option<Line>,
    /// "1 screenshot", "taking a screenshot…", or nothing.
    pub shots: String,
    /// The criterion's buttons, as its card on the page has them.
    pub buttons: Vec<(String, ButtonStyle)>,
    /// Whether it has more than one criterion to step through.
    pub steps_through: bool,
    pub hover: Option<StripHit>,
    /// It has the keyboard: its edge is in the accent.
    pub keys: bool,
}

/// What the pointer is over on the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StripHit {
    Prev,
    Next,
    /// The criterion's dot, id and title: its steps show, or hide.
    Title,
    /// One of the criterion's buttons, by index.
    Button(usize),
    Page,
    Fold,
    /// The folded chip: unfolds it.
    Chip,
    Field,
    Shot,
    Done,
    /// Anywhere else on it.
    Panel,
}

/// Where the strip's parts go, in output coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StripLayout {
    pub panel: Rect,
    /// What can be pressed, front to back.
    pub hits: Vec<(StripHit, Rect)>,
    /// Where the dot, the id and the title go.
    pub title: Rect,
    /// The lines above the row: its steps, then what went wrong, each with
    /// whether it is dim.
    pub lines: Vec<(String, Rect, bool)>,
    pub field: Option<Rect>,
    /// Where "1 screenshot" goes, by the field's buttons.
    pub shots: Option<Rect>,
}

impl StripLayout {
    /// What `pos` is over, if it is on the strip.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<StripHit> {
        if !self.panel.to_f64().contains(pos) {
            return None;
        }
        let on = |r: &Rect| r.to_f64().contains(pos);
        Some(
            (self.hits.iter())
                .find(|(_, r)| on(r))
                .map_or(StripHit::Panel, |(hit, _)| *hit),
        )
    }

    /// Where `hit` is, if the strip has it.
    pub fn rect_of(&self, hit: StripHit) -> Option<Rect> {
        self.hits.iter().find(|(h, _)| *h == hit).map(|(_, r)| *r)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        self.hits.iter().map(|(_, r)| *r).collect()
    }
}

/// How wide `text` is in `size`, guessed when there is no font.
fn width(text: &str, size: f32) -> f32 {
    measure(text, size).unwrap_or(text.chars().count() as f32 * size * 0.55)
}

/// `text` broken at spaces into lines `room` wide at most, `most` of them:
/// the last cut with "…" when there is more.
fn wrapped(text: &str, room: f32, size: f32, most: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = match line.is_empty() {
            true => word.to_owned(),
            false => format!("{line} {word}"),
        };
        if width(&next, size) > room && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > most {
        lines.truncate(most);
        if let Some(last) = lines.last_mut() {
            last.push('\u{2026}');
        }
    }
    lines
}

/// The width the strip's row needs at least: its buttons, and room for the
/// criterion's id.
fn least_width(view: &CriteriaStripView) -> i32 {
    let buttons: i32 = (view.buttons.iter())
        .map(|(label, _)| button_width(label) + GAP)
        .sum();
    let place = width(&view.place, SMALL_FONT_SIZE).ceil() as i32 + DOT_ROOM;
    2 * INSET
        + 2 * (ROW_BUTTON + 2)
        + GROUP_GAP
        + place
        + GROUP_GAP
        + buttons
        + GROUP_GAP
        + button_width("Page")
        + GAP
        + ROW_BUTTON
}

/// The chip the strip folds to, at the bottom right of `pane`.
fn chip_rect(view: &CriteriaStripView) -> Rect {
    let w = DOT_ROOM + width(&view.place, SMALL_FONT_SIZE).ceil() as i32 + CHIP_ICON + 2 * INSET;
    let pane = view.pane;
    let w = w.min(pane.size.w - 2 * MARGIN).max(0);
    Rectangle::new(
        (
            pane.loc.x + pane.size.w - MARGIN - w,
            pane.loc.y + pane.size.h - MARGIN - CHIP,
        )
            .into(),
        (w, CHIP).into(),
    )
}

/// Whether the strip shows folded: as asked, or as its pane is too narrow
/// for its row.
pub fn strip_folded(view: &CriteriaStripView) -> bool {
    view.folded || view.pane.size.w - 2 * MARGIN < least_width(view)
}

/// Where the strip's parts go over its pane.
pub fn criteria_strip_layout(view: &CriteriaStripView) -> StripLayout {
    let pane = view.pane;
    if strip_folded(view) {
        let chip = chip_rect(view);
        return StripLayout {
            panel: chip,
            hits: vec![(StripHit::Chip, chip)],
            title: chip,
            ..StripLayout::default()
        };
    }
    let w = (pane.size.w - 2 * MARGIN).min(MAX_WIDTH);
    let inner = w - 2 * INSET;
    let room = inner as f32;
    // The lines above the row: its steps when open, what went wrong.
    let mut texts: Vec<(String, bool)> = Vec::new();
    if view.open || view.field.is_some() {
        for step in &view.steps {
            for line in wrapped(step, room, SMALL_FONT_SIZE, STEP_LINES) {
                texts.push((line, false));
            }
        }
    }
    if view.open && view.field.is_none() && !view.note.is_empty() {
        let note = format!("What went wrong: {}", view.note);
        for line in wrapped(&note, room, SMALL_FONT_SIZE, STEP_LINES) {
            texts.push((line, true));
        }
    }
    // The note's field holds its lines, as a card's on the page does.
    let field_lines =
        (view.field.as_ref()).map(|line| field_lines_height(line.text.split('\n').count()));
    let field_h = field_lines.map_or(0, |h| h + GAP);
    let lines_h = texts.len() as i32 * LINE;
    let top_room = match lines_h + field_h {
        0 => 0,
        h => h + INSET,
    };
    let h = ROW + top_room;
    // Never taller than the pane: the steps go before the row does.
    let h = h.min(pane.size.h - 2 * MARGIN).max(ROW);
    let panel = Rectangle::new(
        (
            pane.loc.x + (pane.size.w - w) / 2,
            pane.loc.y + pane.size.h - MARGIN - h,
        )
            .into(),
        (w, h).into(),
    );
    let (x0, mut y) = (panel.loc.x + INSET, panel.loc.y + INSET);
    let bottom_row = panel.loc.y + h - ROW;
    let mut lines = Vec::new();
    for (text, dim) in texts {
        if y + LINE > bottom_row - field_h {
            break;
        }
        lines.push((
            text,
            Rectangle::new((x0, y).into(), (inner, LINE).into()),
            dim,
        ));
        y += LINE;
    }
    let mut hits = Vec::new();
    let mut field = None;
    let mut shots = None;
    if let Some(fh) = field_lines {
        let fy = bottom_row - fh - GAP / 2;
        // The buttons and the count by its last line, over the row.
        let last = fy + fh - QUIT_BUTTON;
        let done_w = button_width("Done");
        let shot_w = button_width("Screenshot");
        let bx = |w: i32, right: i32| {
            Rectangle::new(
                (right - w, last + (QUIT_BUTTON - ROW_BUTTON) / 2).into(),
                (w, ROW_BUTTON).into(),
            )
        };
        let end = panel.loc.x + w - INSET;
        let done = bx(done_w, end);
        let shot = bx(shot_w, done.loc.x - GAP);
        let fw = shot.loc.x - GROUP_GAP - x0;
        let label = match view.shots.is_empty() {
            true => 0,
            false => (width(&view.shots, SMALL_FONT_SIZE).ceil() as i32 + GAP).min(fw / 3),
        };
        let f = Rectangle::new((x0, fy).into(), ((fw - label).max(0), fh).into());
        if label > 0 {
            shots = Some(Rectangle::new(
                (f.loc.x + f.size.w + GAP, last).into(),
                (label - GAP, QUIT_BUTTON).into(),
            ));
        }
        hits.push((StripHit::Done, done));
        hits.push((StripHit::Shot, shot));
        hits.push((StripHit::Field, f));
        field = Some(f);
    }
    // The row: ‹ › at the left, the fold button and Page at the right, the
    // criterion's buttons before them, the title in what is left.
    let cy = bottom_row + (ROW - ROW_BUTTON) / 2;
    let square = |x: i32| Rectangle::new((x, cy).into(), (ROW_BUTTON, ROW_BUTTON).into());
    let prev = square(x0);
    let next = square(x0 + ROW_BUTTON + 2);
    if view.steps_through {
        hits.push((StripHit::Prev, prev));
        hits.push((StripHit::Next, next));
    }
    let mut right = panel.loc.x + w - INSET;
    let fold = square(right - ROW_BUTTON);
    hits.push((StripHit::Fold, fold));
    right = fold.loc.x - GAP;
    let page_w = button_width("Page");
    let page = Rectangle::new((right - page_w, cy).into(), (page_w, ROW_BUTTON).into());
    hits.push((StripHit::Page, page));
    right = page.loc.x - GROUP_GAP;
    let mut buttons = Vec::new();
    for (i, (label, _)) in view.buttons.iter().enumerate().rev() {
        let bw = button_width(label);
        let rect = Rectangle::new((right - bw, cy).into(), (bw, ROW_BUTTON).into());
        buttons.push((StripHit::Button(i), rect));
        right = rect.loc.x - GAP;
    }
    buttons.reverse();
    hits.extend(buttons);
    let tx = next.loc.x + ROW_BUTTON + GROUP_GAP;
    let title = Rectangle::new(
        (tx, bottom_row).into(),
        ((right - GROUP_GAP + GAP - tx).max(0), ROW).into(),
    );
    hits.push((StripHit::Title, title));
    StripLayout {
        panel,
        hits,
        title,
        lines,
        field,
        shots,
    }
}

/// Where the note's text goes in the strip showing `view`, while one is
/// written: for the caret a click puts.
pub fn criteria_strip_field(view: &CriteriaStripView, scale: f64) -> Option<FieldSpot> {
    let line = view.field.clone()?;
    let area = criteria_strip_layout(view).field?;
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line,
    })
}

/// A filled dot `d` across, or a ring when `ring`.
fn dot(ring: bool, x: f32, y: f32, d: f32) -> f32 {
    let r = d / 2.0;
    let dist = x.hypot(y) - r;
    if ring { dist.abs() - d / 7.0 } else { dist }
}

impl Painter {
    /// The strip showing `view`, laid out as `layout`, in a margin of
    /// `SHADOW` holding its drop shadow.
    pub(super) fn criteria_strip(
        &self,
        view: &CriteriaStripView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = criteria_strip_layout(view);
        let size = layout.panel.size;
        let radius = match strip_folded(view) {
            true => CHIP as f32 / 2.0,
            false => POPUP_RADIUS,
        };
        let mut c = self.popup(size, radius, scale, p);
        // Its own coordinates: the panel `SHADOW` in from the top left.
        let shift = Point::from((SHADOW, SHADOW)) - layout.panel.loc;
        let here = |r: Rect| Rectangle::new(r.loc + shift, r.size);
        let lit = |hit: StripHit| view.hover == Some(hit);
        if view.keys {
            let panel = here(layout.panel);
            let (x, y, w, h) = rect_px(panel, scale);
            let b = (scale.round() as f32).max(1.0) * FOCUS_RING;
            c.rounded(x, y, w, h, radius * scale as f32, p.accent);
            c.rounded(
                x + b,
                y + b,
                w - 2.0 * b,
                h - 2.0 * b,
                radius * scale as f32 - b,
                p.elevated,
            );
        }
        let s = scale as f32;
        let d = super::DOT as f32 * s * 1.3;
        let tag = view.tag;
        let mark = |c: &mut Canvas, at: Rect| {
            let spot = Rectangle::new(
                (at.loc.x, at.loc.y + (at.size.h - DOT_ROOM) / 2).into(),
                (DOT_ROOM - 4, DOT_ROOM).into(),
            );
            icon_sdf(c, spot, scale, tag.color(p), |x, y| {
                dot(tag == StripTag::NotRun, x, y, d)
            });
        };
        if strip_folded(view) {
            let chip = here(layout.panel);
            if lit(StripHit::Chip) {
                let (x, y, w, h) = rect_px(chip, scale);
                c.rounded(x, y, w, h, radius * s, p.element);
            }
            let inner = Rectangle::new(
                (chip.loc.x + INSET, chip.loc.y).into(),
                (chip.size.w - 2 * INSET, chip.size.h).into(),
            );
            mark(&mut c, inner);
            let text_x = (inner.loc.x + DOT_ROOM) as f32 * s;
            let icon = Rectangle::new(
                (inner.loc.x + inner.size.w - CHIP_ICON, inner.loc.y).into(),
                (CHIP_ICON, inner.size.h).into(),
            );
            self.text(
                &mut c,
                &view.place,
                SMALL_FONT_SIZE,
                text_x,
                icon.loc.x as f32 * s,
                band(inner, scale),
                scale,
                p.text,
            );
            // Pointing up: it unfolds upwards.
            icon_sdf(&mut c, icon, scale, p.muted, |x, y| chevron_down(x, -y, s));
            return c.into_buffer(false);
        }
        for (text, rect, dim) in &layout.lines {
            let rect = here(*rect);
            let (x, _, w, _) = rect_px(rect, scale);
            let color = if *dim { p.muted } else { p.text };
            self.text(
                &mut c,
                text,
                SMALL_FONT_SIZE,
                x,
                x + w,
                band(rect, scale),
                scale,
                color,
            );
        }
        if let (Some(field), Some(line)) = (layout.field, &view.field) {
            self.field_of_lines(
                &mut c,
                line,
                "What went wrong",
                here(field),
                view.keys,
                scale,
                p,
            );
        }
        if let Some(rect) = layout.shots {
            let rect = here(rect);
            let (x, _, w, _) = rect_px(rect, scale);
            self.text(
                &mut c,
                &view.shots,
                SMALL_FONT_SIZE,
                x,
                x + w,
                band(rect, scale),
                scale,
                p.muted,
            );
        }
        for (hit, rect) in &layout.hits {
            let rect = here(*rect);
            match *hit {
                StripHit::Prev | StripHit::Next | StripHit::Fold => {
                    if lit(*hit) {
                        let (x, y, w, h) = rect_px(rect, scale);
                        c.rounded(x, y, w, h, BUTTON_RADIUS * s, p.element);
                    }
                    let ink = if lit(*hit) { p.text } else { p.muted };
                    match *hit {
                        StripHit::Fold => {
                            icon_sdf(&mut c, rect, scale, ink, |x, y| chevron_down(x, y, s));
                        }
                        side => {
                            let left = side == StripHit::Prev;
                            icon_sdf(&mut c, rect, scale, ink, |x, y| chevron_side(left, x, y, s));
                        }
                    }
                }
                StripHit::Button(i) => {
                    let (label, style) = &view.buttons[i];
                    self.button(&mut c, label, rect, (lit(*hit), false), *style, scale, p);
                }
                StripHit::Page => {
                    self.button(
                        &mut c,
                        "Page",
                        rect,
                        (lit(*hit), false),
                        ButtonStyle::Quiet,
                        scale,
                        p,
                    );
                }
                StripHit::Shot => {
                    self.button(
                        &mut c,
                        "Screenshot",
                        rect,
                        (lit(*hit), false),
                        ButtonStyle::Plain,
                        scale,
                        p,
                    );
                }
                StripHit::Done => {
                    self.button(
                        &mut c,
                        "Done",
                        rect,
                        (lit(*hit), false),
                        ButtonStyle::Primary,
                        scale,
                        p,
                    );
                }
                StripHit::Title => {
                    if lit(*hit) {
                        let (x, y, w, h) = rect_px(rect, scale);
                        let inset = 4.0 * s;
                        c.rounded(
                            x - inset,
                            y + inset,
                            w + inset,
                            h - 2.0 * inset,
                            BUTTON_RADIUS * s,
                            p.element,
                        );
                    }
                    mark(&mut c, rect);
                    let max = (rect.loc.x + rect.size.w) as f32 * s;
                    let row = band(rect, scale);
                    let mut x = (rect.loc.x + DOT_ROOM) as f32 * s;
                    x = self.text(
                        &mut c,
                        &view.place,
                        SMALL_FONT_SIZE,
                        x,
                        max,
                        row,
                        scale,
                        p.muted,
                    );
                    x += 8.0 * s;
                    x = self.text(&mut c, &view.title, FONT_SIZE, x, max, row, scale, p.text);
                    if !view.state.is_empty() {
                        x += 8.0 * s;
                        self.text(
                            &mut c,
                            &view.state,
                            SMALL_FONT_SIZE,
                            x,
                            max,
                            row,
                            scale,
                            p.muted,
                        );
                    }
                }
                StripHit::Field | StripHit::Chip | StripHit::Panel => {}
            }
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn view(pane_w: i32) -> CriteriaStripView {
        CriteriaStripView {
            pane: Rectangle::new((200, 40).into(), (pane_w, 700).into()),
            folded: false,
            open: false,
            place: "AC-3 \u{b7} 3/7".into(),
            title: "Rows save on edit".into(),
            tag: StripTag::NotRun,
            state: String::new(),
            steps: vec![
                "Given two rows".into(),
                "When I edit both".into(),
                "Then both change".into(),
            ],
            note: String::new(),
            field: None,
            shots: String::new(),
            buttons: vec![
                ("Pass".into(), ButtonStyle::Plain),
                ("Fail\u{2026}".into(), ButtonStyle::Plain),
            ],
            steps_through: true,
            hover: None,
            keys: false,
        }
    }

    #[test]
    fn the_strip_sits_at_its_panes_bottom_with_its_buttons_in_one_row() {
        let v = view(1200);
        let layout = criteria_strip_layout(&v);
        let pane = v.pane;
        assert_eq!(layout.panel.size.w, MAX_WIDTH);
        assert_eq!(
            layout.panel.loc.y + layout.panel.size.h,
            pane.loc.y + pane.size.h - MARGIN
        );
        assert_eq!(layout.panel.size.h, ROW);
        let row = |hit| layout.rect_of(hit).unwrap();
        let pass = row(StripHit::Button(0));
        let fail = row(StripHit::Button(1));
        assert!(row(StripHit::Prev).loc.x < row(StripHit::Next).loc.x);
        assert!(row(StripHit::Title).loc.x + row(StripHit::Title).size.w <= pass.loc.x);
        assert!(pass.loc.x < fail.loc.x && fail.loc.x < row(StripHit::Page).loc.x);
        assert!(row(StripHit::Page).loc.x < row(StripHit::Fold).loc.x);
        for (_, r) in &layout.hits {
            assert!(layout.panel.contains_rect(*r), "{r:?}");
        }
        let mid = pass.to_f64().loc + Point::from((2.0, 2.0));
        assert_eq!(layout.hit(mid), Some(StripHit::Button(0)));
        assert_eq!(layout.hit(Point::from((0.0, 0.0))), None);
    }

    #[test]
    fn open_it_shows_the_steps_and_writing_the_field_over_the_row() {
        let mut v = view(1200);
        v.open = true;
        let layout = criteria_strip_layout(&v);
        assert_eq!(layout.lines.len(), 3);
        assert!(layout.field.is_none());
        v.field = Some(Line::default());
        v.shots = "1 screenshot".into();
        let layout = criteria_strip_layout(&v);
        let field = layout.field.unwrap();
        let row = layout.rect_of(StripHit::Title).unwrap();
        assert!(field.loc.y + field.size.h <= row.loc.y);
        assert!(layout.lines.last().unwrap().1.loc.y + LINE <= field.loc.y);
        let shot = layout.rect_of(StripHit::Shot).unwrap();
        let done = layout.rect_of(StripHit::Done).unwrap();
        assert!(field.loc.x + field.size.w < layout.shots.unwrap().loc.x);
        assert!(layout.shots.unwrap().loc.x < shot.loc.x && shot.loc.x < done.loc.x);
        assert!(criteria_strip_field(&v, 1.0).is_some());
    }

    #[test]
    fn the_note_grows_a_line_at_a_time_with_its_buttons_by_the_last() {
        let mut v = view(1200);
        let line = |text: &str| Line {
            text: text.into(),
            caret: text.len(),
            anchor: text.len(),
        };
        v.field = Some(line("one"));
        let one = criteria_strip_layout(&v);
        v.field = Some(line("one\ntwo\nthree"));
        let three = criteria_strip_layout(&v);
        let (f1, f3) = (one.field.unwrap(), three.field.unwrap());
        assert_eq!(f1.size.h, field_lines_height(1));
        assert_eq!(f3.size.h, field_lines_height(3));
        assert_eq!(three.panel.size.h - one.panel.size.h, f3.size.h - f1.size.h);
        let row = three.rect_of(StripHit::Title).unwrap();
        assert!(f3.loc.y + f3.size.h <= row.loc.y);
        let done = three.rect_of(StripHit::Done).unwrap();
        assert!(done.loc.y > f3.loc.y + f3.size.h - QUIT_BUTTON);
        assert!(done.loc.y + done.size.h <= f3.loc.y + f3.size.h);
    }

    #[test]
    fn folded_or_too_narrow_it_is_a_chip_in_the_corner() {
        let mut v = view(1200);
        v.folded = true;
        let layout = criteria_strip_layout(&v);
        assert_eq!(layout.hits.len(), 1);
        assert_eq!(layout.hits[0].0, StripHit::Chip);
        let pane = v.pane;
        assert_eq!(
            layout.panel.loc.x + layout.panel.size.w,
            pane.loc.x + pane.size.w - MARGIN
        );
        let narrow = view(360);
        assert!(strip_folded(&narrow));
        assert_eq!(criteria_strip_layout(&narrow).hits[0].0, StripHit::Chip);
    }

    #[test]
    fn a_low_pane_keeps_the_row_and_drops_steps() {
        let mut v = view(1200);
        v.pane.size.h = ROW + 2 * MARGIN + LINE;
        v.open = true;
        let layout = criteria_strip_layout(&v);
        assert!(layout.lines.len() <= 1);
        assert!(layout.rect_of(StripHit::Title).is_some());
        assert!(v.pane.contains_rect(layout.panel));
    }

    #[test]
    fn long_steps_wrap_and_are_cut() {
        let long = "word ".repeat(200);
        let lines = wrapped(&long, 200.0, SMALL_FONT_SIZE, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('\u{2026}'));
        assert_eq!(wrapped("short", 200.0, SMALL_FONT_SIZE, 2), ["short"]);
    }
}
