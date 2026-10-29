//! The scrollback viewer over a terminal pane (see `crate::scrollback`):
//! a title row, the pane's output in the monospace font on the terminal's
//! own colours, and the key hints under it. Each row is a buffer of its
//! own, so scrolling only moves them.

use super::{
    FONT_SIZE, HELP_RADIUS, Painter, Palette, Rect, SHADOW, SMALL_FONT_SIZE, band,
    footer_hint_spots, hint_at, hint_strs, layers::Part, measure, measure_mono, mix, phys,
    physical, rect_px,
};
use crate::scrollback::Fit;
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
/// The scrollbar at the list's right, the gap beside it, and its thumb's
/// least height.
const SCROLLBAR: i32 = 6;
const SCROLLBAR_GAP: i32 = 4;
const THUMB_MIN: i32 = 24;
/// The title's room from the panel's sides.
const TITLE_INSET: i32 = 14;

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
    /// The hint under the pointer.
    pub hover: Option<usize>,
}

/// A row of the viewer's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackRow {
    pub text: String,
    /// From and to which of its characters it is selected, and whether
    /// the selection goes on past its end (see `Scrollback::row_selection`).
    pub selected: Option<(usize, usize, bool)>,
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
    let hints = footer_hint_spots(&hint_strs(hints), footer);
    ScrollbackLayout {
        panel,
        title,
        list,
        text,
        footer,
        hints,
    }
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
    let hints = hint_strs(&view.hints);
    Some(match hint_at(&hints, &layout.hints, pos) {
        Some(i) => ScrollbackHit::Hint(i),
        None => ScrollbackHit::Panel,
    })
}

/// Which row and character `pos` is at, the row counted in all of them:
/// before the first row or past the last when it is above or below them.
pub fn scrollback_point(
    view: &ScrollbackView,
    layout: &ScrollbackLayout,
    pos: Point<f64, Logical>,
) -> (i64, usize) {
    let text = layout.text;
    let y = pos.y - text.loc.y as f64 + view.part * ROW as f64 - view.overscroll;
    let row = view.first as i64 + (y / ROW as f64).floor() as i64;
    let col = ((pos.x - text.loc.x as f64) / advance()).round().max(0.0) as usize;
    (row, col)
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
    let track = track(layout);
    let page = (layout.text.size.h / ROW).max(1) as usize;
    if view.total <= page || track.size.h <= 0 {
        return None;
    }
    let h = track.size.h as f64;
    let thumb_h = (h * page as f64 / view.total as f64)
        .max((THUMB_MIN as f64).min(h))
        .min(h)
        .round();
    let last = (view.total - page) as f64;
    let pos = (view.first as f64 + view.part).min(last);
    Some(super::Thumb {
        track,
        top: track.loc.y as f64 + (h - thumb_h) * pos / last,
        h: thumb_h,
        last,
    })
}

/// What the frame is drawn from: the view without its rows and where they
/// stand, so scrolling leaves it be.
fn frame_view(view: &ScrollbackView) -> ScrollbackView {
    ScrollbackView {
        rows: Vec::new(),
        first: 0,
        part: 0.0,
        overscroll: 0.0,
        ..view.clone()
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
        self.text(
            &mut c,
            &view.title,
            FONT_SIZE,
            left + inset,
            note_x - inset,
            band(title, scale),
            scale,
            p.text,
        );

        let (x, y, w, h) = rect_px(shift(layout.list), scale);
        c.rounded(x, y, w, h, 4.0 * s, p.terminal_background);

        let hints = hint_strs(&view.hints);
        let spots: Vec<(usize, Rect)> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        self.hints(&mut c, &hints, &spots, view.hover, scale, p);
        c
    }

    /// A row of the text, `w` wide: on the terminal's background, the
    /// selected part of it on the selection's.
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
        c
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

/// The viewer as the chrome shows it, fading in and out (see `Overlay`):
/// its frame, a buffer per row in view and the scrollbar's thumb.
#[derive(Default)]
pub(super) struct ScrollbackLayers {
    overlay: super::Overlay<FrameKey>,
    rows: super::layers::RowCache<(ScrollbackRow, i32, super::Theme, f64)>,
    thumb: super::Cached<(Size<i32, Logical>, super::Theme, f64)>,
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
        let (rows, thumb) = (&mut self.rows, &mut self.thumb);
        let p = theme.palette();
        let more = |_: &FrameKey| {
            let Some(view) = &view else {
                return Vec::new();
            };
            let layout = scrollback_layout(view.panel, &view.hints);
            let mut parts = Vec::new();
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
            })
            .collect();
        v.part = 0.4;
        v.hover = Some(1);
        for (theme, scale) in [
            (Theme::Dark, 1.0),
            (Theme::Light, 1.0),
            (Theme::Dark, 2.0),
            (Theme::Light, 2.0),
        ] {
            let w = ((v.panel.size.w + 2 * SHADOW) as f64 * scale).round() as usize;
            let mut buffer = painter.scrollback(&v, scale, theme.palette());
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
            let file = format!("scrollback_{}_{scale}.ppm", theme.name());
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
