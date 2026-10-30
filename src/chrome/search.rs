//! A search over a list of text, as the repos panel's diff and the
//! scrollback viewer have one: a field in a title row, how many places it
//! found, and buttons going to the place before or the next and closing
//! it. The places found are tinted where they show, the one gone to more,
//! and ticks on the list's scrollbar say where they all are.

use std::ops::Range;

use super::{
    BUTTON_RADIUS, Canvas, FIELD_GAP, FONT_SIZE, Painter, Palette, Rect, Rgb, SMALL_FONT_SIZE,
    band, chevron_down, cross, field_text, icon_sdf, magnifier, mix, physical, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size};

/// Room for how many it found, and between its buttons.
const SEARCH_COUNT: i32 = 84;
const SEARCH_GAP: i32 = 2;
/// How much of the search's colour tints a place found, and the one gone
/// to; how high a tick on the scrollbar is.
const FOUND_TINT: f32 = 0.35;
const FOUND_CURRENT_TINT: f32 = 0.75;
/// How much of the accent the caret's block is, on the ground.
const CARET_TINT: f32 = 0.6;
/// How much of the accent is in the ground under what is selected.
const CHOSEN_TINT: f32 = 0.4;
const SEARCH_TICK: i32 = 2;

/// The search: what is typed, what it says while empty, and how many
/// places it found (`3 of 17`, `No matches`; empty with nothing typed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchBar {
    pub query: crate::text_field::Line,
    pub placeholder: String,
    pub count: String,
    /// It found some.
    pub found: bool,
    /// It has the keyboard.
    pub focused: bool,
    /// It finds what is typed only as a whole word (vim's `*`).
    pub whole: bool,
}

/// A part of the search: the button opening it (while closed), its
/// field, and its buttons going to the place before or the next, and
/// closing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchPart {
    Open,
    Field,
    Prev,
    Next,
    Close,
}

/// What the field says at its right end while the search finds whole
/// words.
const WHOLE_TAG: &str = "Whole word";

/// What the search's count says: nothing with nothing `typed`, else
/// which of the `n` places found it is at (by index), or how many.
pub fn search_count(typed: bool, n: usize, current: Option<usize>) -> String {
    match current {
        _ if !typed => String::new(),
        _ if n == 0 => "No matches".into(),
        Some(i) => format!("{} of {n}", i + 1),
        None => format!("{n} found"),
    }
}

/// Where the open search's parts go in a row from `left` to `right`,
/// `y` down and `h` high: its field, then its buttons going to the place
/// before and the next and closing it, square; and where its count goes,
/// between the field and the buttons.
pub(super) fn search_spots(
    left: i32,
    right: i32,
    y: i32,
    h: i32,
) -> (Vec<(SearchPart, Rect)>, Rect) {
    let mut parts = Vec::new();
    let mut right = right;
    for part in [SearchPart::Close, SearchPart::Next, SearchPart::Prev] {
        right -= h;
        parts.push((part, Rectangle::new((right, y).into(), (h, h).into())));
        right -= SEARCH_GAP;
    }
    right -= SEARCH_COUNT - SEARCH_GAP;
    let count = Rectangle::new((right, y).into(), (SEARCH_COUNT, h).into());
    right -= FIELD_GAP;
    let field = Rectangle::new((left, y).into(), ((right - left).max(0), h).into());
    parts.push((SearchPart::Field, field));
    parts.reverse();
    (parts, count)
}

/// Those of the places `found` inside `range` (char indices), from its
/// start.
pub(super) fn found_within(
    found: &[(Range<usize>, bool)],
    range: Range<usize>,
) -> Vec<(Range<usize>, bool)> {
    found
        .iter()
        .filter(|(r, _)| r.start >= range.start && r.end <= range.end)
        .map(|(r, on)| (r.start - range.start..r.end - range.start, *on))
        .collect()
}

/// `range` (char indices) in parts one after another, each with whether
/// it is in one of the places `found`.
pub(super) fn found_split(
    range: Range<usize>,
    found: &[(Range<usize>, bool)],
) -> Vec<(Range<usize>, bool)> {
    let mut parts = Vec::new();
    let mut at = range.start;
    for (f, _) in found {
        let (start, end) = (f.start.max(at), f.end.min(range.end));
        if start >= end {
            continue;
        }
        if at < start {
            parts.push((at..start, false));
        }
        parts.push((start..end, true));
        at = end;
    }
    if at < range.end {
        parts.push((at..range.end, false));
    }
    parts
}

impl Painter {
    /// The search's `parts` (see `search_spots`): its button while `bar`
    /// is closed; open, its field, its buttons and, in `count`, how many
    /// it found. `hover` is the part under the pointer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn search_bar(
        &self,
        c: &mut Canvas,
        bar: Option<&SearchBar>,
        parts: &[(SearchPart, Rect)],
        count: Option<Rect>,
        hover: Option<SearchPart>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        for &(part, rect) in parts {
            let hovered = hover == Some(part);
            if part == SearchPart::Field {
                let Some(bar) = bar else { continue };
                let (x, y, w, h) = rect_px(rect, scale);
                let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
                let (edge, fill) = if bar.focused {
                    (p.accent, p.element)
                } else if hovered {
                    (p.border, mix(p.element, p.selected, 0.4))
                } else {
                    (p.border, mix(p.elevated, p.element, 0.6))
                };
                c.rounded(x, y, w, h, r, edge);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                let (text_x, mut text_end) = field_text(rect, scale);
                // A whole word: a tag at the right end says so.
                if bar.whole {
                    let tw = self.text_width(WHOLE_TAG, SMALL_FONT_SIZE, scale);
                    let pad = (6.0 * s).round();
                    let (tx, tend) = (text_end - tw - pad, text_end + pad / 2.0);
                    if tx - pad > text_x {
                        let (ty, th) = (y + 4.0 * b, h - 8.0 * b);
                        let tag = mix(fill, p.accent, 0.25);
                        c.rounded(tx - pad / 2.0, ty, tend - tx, th, r - b, tag);
                        let band = band(rect, scale);
                        self.text(
                            c,
                            WHOLE_TAG,
                            SMALL_FONT_SIZE,
                            tx,
                            tend,
                            band,
                            scale,
                            p.muted,
                        );
                        text_end = tx - pad;
                    }
                }
                self.edit_line(
                    c,
                    &bar.query,
                    &bar.placeholder,
                    (text_x, text_end),
                    band(rect, scale),
                    bar.focused,
                    FONT_SIZE,
                    scale,
                    p,
                );
                continue;
            }
            // Going on or back needs places found.
            let enabled = match part {
                SearchPart::Prev | SearchPart::Next => bar.is_some_and(|b| b.found),
                _ => true,
            };
            let (x, y, w, h) = rect_px(rect, scale);
            let b = ((scale.round() as i32).max(1)) as f32;
            let r = BUTTON_RADIUS * s;
            let fill = if hovered && enabled {
                mix(p.element, p.selected, 0.5)
            } else {
                p.element
            };
            c.rounded(x, y, w, h, r, p.border);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
            let color = match enabled {
                true if hovered => p.text,
                true => p.muted,
                false => p.placeholder,
            };
            icon_sdf(c, rect, scale, color, |x, y| match part {
                SearchPart::Open => magnifier(x, y, 0.9 * s),
                SearchPart::Prev => chevron_down(x, -y, 1.2 * s),
                SearchPart::Next => chevron_down(x, y, 1.2 * s),
                _ => cross(x, y, 0.8 * s),
            });
        }
        if let (Some(bar), Some(rect)) = (bar, count) {
            let color = if bar.found { p.muted } else { p.error };
            self.centered(c, &bar.count, SMALL_FONT_SIZE, rect, scale, color);
        }
    }

    /// The places the search found, as ticks across a scrollbar `track`
    /// (in buffer coordinates) on `ground`: the one gone to taller, in
    /// the accent.
    pub(super) fn search_ticks(
        &self,
        c: &mut Canvas,
        ticks: &[(f64, bool)],
        track: Rect,
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let (x, y, w, h) = rect_px(track, scale);
        let th = (SEARCH_TICK as f64 * scale).round().max(1.0) as f32;
        let found = mix(ground, p.warning, FOUND_CURRENT_TINT);
        let mut current = None;
        let mut last = None;
        for &(at, on) in ticks {
            let ty = (y + (h - th) * at as f32).round();
            if on {
                current = Some(ty);
            }
            // Ticks on one pixel row are one.
            if last != Some(ty) {
                c.rounded(x, ty, w, th, 0.0, found);
                last = Some(ty);
            }
        }
        if let Some(ty) = current {
            let ty = (ty - th / 2.0).clamp(y, y + h - 2.0 * th);
            c.rounded(x, ty, w, 2.0 * th, th / 2.0, p.accent);
        }
    }

    /// The ticks of `search_ticks` in a buffer of their own, the track's
    /// size: over the scrollbar's thumb, they show on it too.
    pub(super) fn search_ticks_buffer(
        &self,
        ticks: &[(f64, bool)],
        size: Size<i32, Logical>,
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let track = Rectangle::new((0, 0).into(), size);
        self.search_ticks(&mut c, ticks, track, ground, scale, p);
        c.into_buffer(false)
    }

    /// Tint the places the search `found` in `text` (by char index) on
    /// `ground`, its first character at `x` and each measured by `width`
    /// of the text before it, cut at `end`: the one gone to more.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn found_tints(
        &self,
        c: &mut Canvas,
        text: &str,
        found: &[(Range<usize>, bool)],
        (x, end): (f32, f32),
        (y, h): (i32, i32),
        width: impl Fn(&str) -> f32,
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let border = ((scale.round() as i32).max(1)) as f32;
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(b, _)| b);
        for (range, current) in found {
            let x0 = x + width(&text[..byte(range.start)]);
            if x0 >= end {
                break;
            }
            let x1 = (x + width(&text[..byte(range.end)])).min(end);
            let tint = if *current {
                FOUND_CURRENT_TINT
            } else {
                FOUND_TINT
            };
            let (ty, th) = (y as f32 + border, h as f32 - 2.0 * border);
            let color = mix(ground, p.warning, tint);
            c.rounded(x0, ty, x1 - x0, th, 2.0 * scale as f32, color);
        }
    }

    /// Underline `word` of `text` (by char index), the word at the caret
    /// that `*` searches for, placed as `found_tints` places its tints:
    /// at the bottom of the row band `(y, h)`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn word_underline(
        &self,
        c: &mut Canvas,
        text: &str,
        word: &Range<usize>,
        (x, end): (f32, f32),
        (y, h): (i32, i32),
        width: impl Fn(&str) -> f32,
        scale: f64,
        p: &Palette,
    ) {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(b, _)| b);
        let x0 = (x + width(&text[..byte(word.start)])).round();
        let x1 = (x + width(&text[..byte(word.end)])).min(end).round();
        if x0 >= x1 {
            return;
        }
        let thick = (scale.round() as i32).max(1);
        let under = y + h - 2 * thick;
        c.fill(x0 as i32, under, (x1 - x0) as i32, thick, p.muted);
    }

    /// Characters `chars` of `text` selected: a band under them, placed
    /// as `found_tints` places its tints; a space's width for none, as on
    /// an empty line.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn chosen_tint(
        &self,
        c: &mut Canvas,
        text: &str,
        chars: &Range<usize>,
        (x, end): (f32, f32),
        (y, h): (i32, i32),
        width: impl Fn(&str) -> f32,
        scale: f64,
        p: &Palette,
    ) {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(b, _)| b);
        let x0 = x + width(&text[..byte(chars.start)]);
        let mut x1 = x + width(&text[..byte(chars.end)]);
        if x1 <= x0 {
            x1 = x0 + width(" ");
        }
        let (x0, x1) = (x0.round(), x1.min(end).round());
        if x0 >= x1 {
            return;
        }
        let border = ((scale.round() as i32).max(1)) as f32;
        let (ty, th) = (y as f32 + border, h as f32 - 2.0 * border);
        c.rounded(
            x0,
            ty,
            x1 - x0,
            th,
            scale as f32,
            mix(p.elevated, p.accent, CHOSEN_TINT),
        );
    }

    /// The caret: a block over character `ch` of `text` (past its end, a
    /// space's width), placed as `found_tints` places its tints, in the
    /// accent on `ground`, the text drawn over it after.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn caret_block(
        &self,
        c: &mut Canvas,
        text: &str,
        ch: usize,
        (x, end): (f32, f32),
        (y, h): (i32, i32),
        width: impl Fn(&str) -> f32,
        ground: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let byte = |n: usize| text.char_indices().nth(n).map(|(b, _)| b);
        let x0 = x + width(&text[..byte(ch).unwrap_or(text.len())]);
        let x1 = match byte(ch + 1).or(byte(ch).map(|_| text.len())) {
            Some(b) => x + width(&text[..b]),
            None => x0 + width(" "),
        };
        let (x0, x1) = (x0.round(), x1.min(end).round());
        if x0 >= x1 {
            return;
        }
        let border = ((scale.round() as i32).max(1)) as f32;
        let (ty, th) = (y as f32 + border, h as f32 - 2.0 * border);
        let color = mix(ground, p.accent, CARET_TINT);
        c.rounded(x0, ty, x1 - x0, th, scale as f32, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_splits_at_the_places_found() {
        let found = [(2..4, false), (6..9, true)];
        let parts = found_split(0..10, &found);
        let want = [
            (0..2, false),
            (2..4, true),
            (4..6, false),
            (6..9, true),
            (9..10, false),
        ];
        assert_eq!(parts, want);
        // A run inside a place, and one reaching into it.
        assert_eq!(found_split(7..8, &found), [(7..8, true)]);
        assert_eq!(found_split(3..5, &found), [(3..4, true), (4..5, false)]);
        assert_eq!(found_split(0..3, &[]), [(0..3, false)]);
    }

    #[test]
    fn the_count_says_where_it_is_or_that_nothing_is_found() {
        assert_eq!(search_count(false, 0, None), "");
        assert_eq!(search_count(true, 0, None), "No matches");
        assert_eq!(search_count(true, 17, Some(2)), "3 of 17");
        assert_eq!(search_count(true, 4, None), "4 found");
    }

    #[test]
    fn the_open_search_fills_its_row_buttons_at_the_right() {
        let (parts, count) = search_spots(10, 400, 5, 28);
        let order: Vec<SearchPart> = parts.iter().map(|p| p.0).collect();
        use SearchPart::*;
        assert_eq!(order, [Field, Prev, Next, Close]);
        assert_eq!(parts[0].1.loc.x, 10);
        assert_eq!(parts[3].1.loc.x + parts[3].1.size.w, 400);
        assert!(parts[0].1.loc.x + parts[0].1.size.w < count.loc.x);
        assert!(count.loc.x + count.size.w <= parts[1].1.loc.x);
    }
}
