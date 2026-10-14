//! The key help panel shown while the prefix is armed, or pinned open
//! with the Shortcuts action: then it carries a title and a way out.

use super::{
    Canvas, FONT_SIZE, HELP_COLUMN_GAP, HELP_KEY_GAP, HELP_PADDING, HELP_RADIUS, HELP_ROW_HEIGHT,
    HELP_TITLE_HEIGHT, Painter, Palette, SHADOW, SMALL_FONT_SIZE, physical,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Size};

/// The pinned panel's title line, above the prefix line.
const HELP_PINNED_TITLE: i32 = 30;
/// The gap between the prefix keycap and the words after it, and between
/// the Esc keycap and its word.
const HELP_WORD_GAP: i32 = 10;
const HELP_ESC_GAP: i32 = 6;

/// What the help panel shows: the prefix, and the keys to press after it
/// as (keys, action). Alternative keys are split by " / ".
#[derive(Debug, PartialEq, Eq)]
pub struct Help {
    pub prefix: String,
    pub rows: Vec<(String, String)>,
}

/// Help panel geometry, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct HelpLayout {
    pub size: Size<i32, Logical>,
    pub rows: i32,
    pub key_width: i32,
    pub column_width: i32,
    /// Pinned open: a title and an Esc hint over the prefix line.
    pub pinned: bool,
}

impl HelpLayout {
    /// How tall the panel's head is: the prefix line, with the title
    /// above it when pinned.
    fn head(&self) -> i32 {
        if self.pinned {
            HELP_PINNED_TITLE + HELP_TITLE_HEIGHT
        } else {
            HELP_TITLE_HEIGHT
        }
    }
}

impl Painter {
    /// Size the help panel: two columns when they fit in `max_width`, else one.
    pub fn help_layout(&self, help: &Help, max_width: i32, pinned: bool, scale: f64) -> HelpLayout {
        let widest = |widths: &mut dyn Iterator<Item = f32>| {
            // Measured at the output scale, where glyph advances are rounded differently.
            widths
                .map(|w| (w / scale as f32).ceil() as i32 + 1)
                .max()
                .unwrap_or(0)
        };
        let key_width = widest(
            &mut help
                .rows
                .iter()
                .map(|(keys, _)| self.keycaps_width(keys, scale)),
        );
        let action_width = widest(
            &mut help
                .rows
                .iter()
                .map(|(_, action)| self.text_width(action, FONT_SIZE, scale)),
        );
        let column_width = key_width + HELP_KEY_GAP + action_width;
        let columns = if 2 * column_width + HELP_COLUMN_GAP + 2 * HELP_PADDING <= max_width {
            2
        } else {
            1
        };
        let rows = help.rows.len().div_ceil(columns) as i32;
        let w = columns as i32 * column_width
            + (columns as i32 - 1) * HELP_COLUMN_GAP
            + 2 * HELP_PADDING;
        let mut layout = HelpLayout {
            size: (w, 0).into(),
            rows,
            key_width,
            column_width,
            pinned,
        };
        layout.size.h = layout.head() + rows * HELP_ROW_HEIGHT + HELP_PADDING;
        layout
    }

    /// The panel inside a margin of `SHADOW` holding its drop shadow.
    pub(super) fn help_panel(
        &self,
        help: &Help,
        layout: &HelpLayout,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let outer = Size::from((layout.size.w + 2 * SHADOW, layout.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);
        let (ox, oy) = (px(SHADOW), px(SHADOW));
        c.popup(layout.size, HELP_RADIUS, scale, p);

        let (pad, max_x) = (ox + px(HELP_PADDING), ox + px(layout.size.w - HELP_PADDING));
        let mut line_top = HELP_PADDING / 2;
        if layout.pinned {
            // "Shortcuts" with the way out at its right.
            let title_band = (oy + px(line_top), px(HELP_PINNED_TITLE));
            self.text(
                &mut c,
                "Shortcuts",
                FONT_SIZE,
                pad as f32,
                max_x as f32,
                title_band,
                scale,
                p.text,
            );
            let close_w = self.text_width("close", SMALL_FONT_SIZE, scale);
            let close_x = max_x as f32 - close_w;
            self.text(
                &mut c,
                "close",
                SMALL_FONT_SIZE,
                close_x,
                max_x as f32,
                title_band,
                scale,
                p.muted,
            );
            let esc_x = close_x - px(HELP_ESC_GAP) as f32 - self.keycaps_width("Esc", scale);
            self.keycaps(&mut c, "Esc", esc_x, title_band, scale, p);
            line_top += HELP_PINNED_TITLE;
        }
        let title_band = (oy + px(line_top), px(HELP_TITLE_HEIGHT - HELP_PADDING / 2));
        let after = self.keycaps(&mut c, &help.prefix, pad as f32, title_band, scale, p);
        self.text(
            &mut c,
            "then press a key",
            SMALL_FONT_SIZE,
            after + px(HELP_WORD_GAP) as f32,
            max_x as f32,
            title_band,
            scale,
            p.muted,
        );
        let head = layout.head();
        c.fill(
            ox + border,
            oy + px(head) - border,
            px(layout.size.w) - 2 * border,
            border,
            p.border_variant,
        );

        for (i, (keys, action)) in help.rows.iter().enumerate() {
            let (column, row) = (i as i32 / layout.rows, i as i32 % layout.rows);
            let x = SHADOW + HELP_PADDING + column * (layout.column_width + HELP_COLUMN_GAP);
            let y = SHADOW + head + row * HELP_ROW_HEIGHT;
            let band = (px(y), px(HELP_ROW_HEIGHT));
            let column_end = px(x + layout.column_width) as f32;
            self.keycaps(&mut c, keys, px(x) as f32, band, scale, p);
            let action_x = px(x + layout.key_width + HELP_KEY_GAP) as f32;
            self.text(
                &mut c, action, FONT_SIZE, action_x, column_end, band, scale, p.text,
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{HELP_PINNED_TITLE, Help, Painter};

    #[test]
    fn the_pinned_panel_is_taller_by_its_title() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let help = Help {
            prefix: "Ctrl+B".into(),
            rows: vec![("c".into(), "new tab".into()), ("q".into(), "quit".into())],
        };
        let armed = painter.help_layout(&help, 800, false, 1.0);
        let pinned = painter.help_layout(&help, 800, true, 1.0);
        assert_eq!(pinned.size.w, armed.size.w);
        assert_eq!(pinned.size.h, armed.size.h + HELP_PINNED_TITLE);
        assert!(pinned.pinned && !armed.pinned);
    }
}
