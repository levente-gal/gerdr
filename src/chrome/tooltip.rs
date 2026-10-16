//! The tooltip over the thing under the pointer: a small popup below it
//! (above it near the bottom) with what it is and the key doing the same,
//! and under that, for a list of worktrees, a line for each.

use super::{
    BRANCH_ICON_GAP, BRANCH_ICON_WIDTH, Canvas, DETAIL_FONT_SIZE, GIT_MARK_GAP, GIT_MARK_WIDTH,
    Painter, Palette, Rect, SHADOW, SMALL_FONT_SIZE, band, git_branch, icon_sdf, physical, rect_px,
};
use crate::git::GitStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size};

/// A tooltip: this high, its text this far in from its ends, the keycap
/// this far after the text, this far from what it is about, and at least
/// this far in from the output's edges.
pub const TOOLTIP_HEIGHT: i32 = 28;
const TOOLTIP_PADDING: i32 = 10;
const TOOLTIP_KEY_GAP: i32 = 8;
const TOOLTIP_GAP: i32 = 6;
const TOOLTIP_MARGIN: i32 = 4;
const TOOLTIP_RADIUS: f32 = 6.0;
/// A line of the list under the text, the room after the list, and the
/// gap between a line's name and where it is.
const TOOLTIP_ROW: i32 = 20;
const TOOLTIP_ROWS_BOTTOM: i32 = 6;
const TOOLTIP_COLUMN_GAP: i32 = 16;

/// A line of a tooltip's list: a worktree's name, a dot when it has
/// changes, a word on its state ("merged", "gone"), and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooltipRow {
    pub name: String,
    pub dirty: bool,
    pub tag: Option<String>,
    pub path: String,
}

/// What the tooltip shows: its text, the key doing the same, and what it
/// is about (window coordinates), to sit below or above it.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipView {
    pub text: String,
    pub key: Option<String>,
    pub anchor: Rect,
    /// The list under the text; empty for a plain tooltip.
    pub rows: Vec<TooltipRow>,
}

impl TooltipView {
    /// How high it is: its text's line, and its list under it.
    pub fn height(&self) -> i32 {
        match self.rows.len() as i32 {
            0 => TOOLTIP_HEIGHT,
            n => TOOLTIP_HEIGHT + n * TOOLTIP_ROW + TOOLTIP_ROWS_BOTTOM,
        }
    }
}

/// Where a tooltip of `size` about `anchor` goes on an output of `output`:
/// centred under the anchor, above it when there is no room below, and
/// kept on the output.
pub fn tooltip_layout(anchor: Rect, size: Size<i32, Logical>, output: Size<i32, Logical>) -> Rect {
    let (w, h) = (size.w, size.h);
    let below = anchor.loc.y + anchor.size.h + TOOLTIP_GAP;
    let y = if below + h + TOOLTIP_MARGIN > output.h {
        (anchor.loc.y - TOOLTIP_GAP - h).max(TOOLTIP_MARGIN)
    } else {
        below
    };
    let x = (anchor.loc.x + (anchor.size.w - w) / 2)
        .min(output.w - TOOLTIP_MARGIN - w)
        .max(TOOLTIP_MARGIN);
    Rectangle::new((x, y).into(), size)
}

impl Painter {
    /// How wide the tooltip showing `view` is, at most `max`.
    pub(super) fn tooltip_width(&self, view: &TooltipView, max: i32, scale: f64) -> i32 {
        let mut w = self.text_width(&view.text, SMALL_FONT_SIZE, scale);
        if let Some(key) = &view.key {
            w += TOOLTIP_KEY_GAP as f32 * scale as f32 + self.keycaps_width(key, scale);
        }
        if !view.rows.is_empty() {
            let (names, paths) = self.row_columns(&view.rows, scale);
            let gap = if paths > 0.0 {
                TOOLTIP_COLUMN_GAP as f32 * scale as f32
            } else {
                0.0
            };
            w = w.max(names + gap + paths);
        }
        let w = (w / scale as f32).ceil() as i32 + 2 * TOOLTIP_PADDING;
        w.min(max).max(TOOLTIP_HEIGHT)
    }

    /// How wide the list's columns are, in physical pixels: the widest
    /// name with its icon, dot and word, and the widest place.
    fn row_columns(&self, rows: &[TooltipRow], scale: f64) -> (f32, f32) {
        let s = scale as f32;
        let icon = ((BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32 * s).round();
        let dot = ((GIT_MARK_WIDTH + GIT_MARK_GAP) as f32 * s).round();
        let names = rows.iter().map(|row| {
            let mut w = icon + self.text_width(&row.name, SMALL_FONT_SIZE, scale);
            if row.dirty {
                w += dot;
            }
            if let Some(tag) = &row.tag {
                w += self.text_width(&format!("  {tag}"), DETAIL_FONT_SIZE, scale);
            }
            w
        });
        let paths = rows
            .iter()
            .map(|row| self.text_width(&row.path, DETAIL_FONT_SIZE, scale));
        (names.fold(0.0, f32::max), paths.fold(0.0, f32::max))
    }

    /// The tooltip `w` wide, inside a margin of `SHADOW` holding its drop
    /// shadow: its text, then its key as a keycap.
    pub(super) fn tooltip(
        &self,
        view: &TooltipView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = Size::from((w, view.height()));
        let outer = Size::from((w + 2 * SHADOW, view.height() + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let s = scale as f32;
        c.popup(size, TOOLTIP_RADIUS, scale, p);
        let panel = Rectangle::new((SHADOW, SHADOW).into(), size);
        let (x, _, pw, _) = rect_px(panel, scale);
        let pad = TOOLTIP_PADDING as f32 * s;
        let text_line = Rectangle::new(panel.loc, (w, TOOLTIP_HEIGHT).into());
        let tooltip_band = band(text_line, scale);
        let mut right = x + pw - pad;
        if let Some(key) = &view.key {
            let key_w = self.keycaps_width(key, scale);
            let key_x = (right - key_w).max(x + pad);
            self.keycaps(&mut c, key, key_x, tooltip_band, scale, p);
            right = key_x - TOOLTIP_KEY_GAP as f32 * s;
        }
        self.text(
            &mut c,
            &view.text,
            SMALL_FONT_SIZE,
            x + pad,
            right,
            tooltip_band,
            scale,
            p.text,
        );
        self.tooltip_rows(&mut c, &view.rows, panel, scale, p);
        c.into_buffer(false)
    }

    /// The list under a tooltip's text in `panel`: per line, a branch
    /// icon, the name, a dot when it has changes and a muted word on its
    /// state, then where it is, in a column of its own.
    fn tooltip_rows(
        &self,
        c: &mut Canvas,
        rows: &[TooltipRow],
        panel: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let (names, _) = self.row_columns(rows, scale);
        let left = panel.loc.x + TOOLTIP_PADDING;
        let right = px(panel.loc.x + panel.size.w - TOOLTIP_PADDING) as f32;
        let path_x = px(left) as f32 + names + TOOLTIP_COLUMN_GAP as f32 * s;
        for (k, row) in rows.iter().enumerate() {
            let y = panel.loc.y + TOOLTIP_HEIGHT + k as i32 * TOOLTIP_ROW;
            let line = Rectangle::new((left, y).into(), (panel.size.w, TOOLTIP_ROW).into());
            let line_band = band(line, scale);
            let icon = Rectangle::new((left, y).into(), (BRANCH_ICON_WIDTH, TOOLTIP_ROW).into());
            icon_sdf(c, icon, scale, p.placeholder, |x, y| {
                git_branch(x, y, 0.75 * s)
            });
            let name_x = px(left + BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32;
            let mut x = self.text(
                c,
                &row.name,
                SMALL_FONT_SIZE,
                name_x,
                path_x,
                line_band,
                scale,
                p.text,
            );
            let dirty = GitStatus {
                dirty: row.dirty,
                ..GitStatus::default()
            };
            x = self.git_marks(c, &dirty, x, path_x, line_band, scale, p);
            if let Some(tag) = &row.tag {
                self.text(
                    c,
                    &format!("  {tag}"),
                    DETAIL_FONT_SIZE,
                    x,
                    path_x,
                    line_band,
                    scale,
                    p.muted,
                );
            }
            self.text(
                c,
                &row.path,
                DETAIL_FONT_SIZE,
                path_x,
                right,
                line_band,
                scale,
                p.placeholder,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TOOLTIP_GAP, TOOLTIP_HEIGHT, TOOLTIP_MARGIN, TOOLTIP_ROW, TOOLTIP_ROWS_BOTTOM, TooltipRow,
        TooltipView, tooltip_layout,
    };
    use smithay::utils::{Logical, Size};

    fn size(w: i32) -> Size<i32, Logical> {
        (w, TOOLTIP_HEIGHT).into()
    }
    use crate::chrome::Rect;

    #[test]
    fn the_tooltip_sits_under_its_anchor_and_stays_on_the_output() {
        let output = (800, 600).into();
        let anchor = Rect::new((100, 10).into(), (28, 28).into());
        let tip = tooltip_layout(anchor, size(120), output);
        assert_eq!(tip.loc.y, 10 + 28 + TOOLTIP_GAP);
        assert_eq!(tip.loc.x + tip.size.w / 2, 100 + 14);
        assert_eq!(tip.size, (120, TOOLTIP_HEIGHT).into());
        // Near the bottom it goes above; near the edges it moves in.
        let low = Rect::new((790, 580).into(), (10, 20).into());
        let tip = tooltip_layout(low, size(120), output);
        assert_eq!(tip.loc.y + TOOLTIP_HEIGHT + TOOLTIP_GAP, 580);
        assert_eq!(tip.loc.x + tip.size.w, 800 - TOOLTIP_MARGIN);
        let left = Rect::new((0, 0).into(), (10, 20).into());
        assert_eq!(
            tooltip_layout(left, size(120), output).loc.x,
            TOOLTIP_MARGIN
        );
    }

    #[test]
    fn a_list_makes_it_taller_and_still_fits_above() {
        let row = TooltipRow {
            name: "pane-move".into(),
            dirty: false,
            tag: Some("merged".into()),
            path: "~/gerdr/.claude/worktrees/pane-move".into(),
        };
        let anchor = Rect::new((100, 560).into(), (30, 15).into());
        let mut view = TooltipView {
            text: "1 other worktree".into(),
            key: None,
            anchor,
            rows: Vec::new(),
        };
        assert_eq!(view.height(), TOOLTIP_HEIGHT);
        view.rows = vec![row; 3];
        let h = TOOLTIP_HEIGHT + 3 * TOOLTIP_ROW + TOOLTIP_ROWS_BOTTOM;
        assert_eq!(view.height(), h);
        // No room below: it goes above, whole.
        let tip = tooltip_layout(anchor, (200, h).into(), (800, 600).into());
        assert_eq!(tip.loc.y + h + TOOLTIP_GAP, 560);
    }
}
