//! The key help panel shown while the prefix is armed.

use super::{
    Canvas, FONT_SIZE, HELP_COLUMN_GAP, HELP_KEY_GAP, HELP_PADDING, HELP_RADIUS, HELP_ROW_HEIGHT,
    HELP_TITLE_HEIGHT, Painter, Palette, SHADOW, SMALL_FONT_SIZE, physical,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Size};

/// What the help panel shows: the prefix, and the keys to press after it
/// as (keys, action). Alternative keys are split by " / ".
#[derive(Debug, PartialEq, Eq)]
pub struct Help {
    pub prefix: String,
    pub rows: Vec<(String, String)>,
}

/// Help panel geometry, in logical pixels.
#[derive(PartialEq)]
pub(super) struct HelpLayout {
    pub(super) size: Size<i32, Logical>,
    pub(super) rows: i32,
    pub(super) key_width: i32,
    pub(super) column_width: i32,
}

impl Painter {
    /// Size the help panel: two columns when they fit in `max_width`, else one.
    pub(super) fn help_layout(&self, help: &Help, max_width: i32, scale: f64) -> HelpLayout {
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
        let h = HELP_TITLE_HEIGHT + rows * HELP_ROW_HEIGHT + HELP_PADDING;
        HelpLayout {
            size: (w, h).into(),
            rows,
            key_width,
            column_width,
        }
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
        let title_band = (
            oy + px(HELP_PADDING / 2),
            px(HELP_TITLE_HEIGHT - HELP_PADDING / 2),
        );
        let after = self.keycaps(&mut c, &help.prefix, pad as f32, title_band, scale, p);
        self.text(
            &mut c,
            "then press a key",
            SMALL_FONT_SIZE,
            after + px(10) as f32,
            max_x as f32,
            title_band,
            scale,
            p.muted,
        );
        c.fill(
            ox + border,
            oy + px(HELP_TITLE_HEIGHT) - border,
            px(layout.size.w) - 2 * border,
            border,
            p.border_variant,
        );

        for (i, (keys, action)) in help.rows.iter().enumerate() {
            let (column, row) = (i as i32 / layout.rows, i as i32 % layout.rows);
            let x = SHADOW + HELP_PADDING + column * (layout.column_width + HELP_COLUMN_GAP);
            let y = SHADOW + HELP_TITLE_HEIGHT + row * HELP_ROW_HEIGHT;
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
