//! The key help panel shown while the prefix is armed, or pinned open
//! with the Shortcuts action: then it carries a title and a way out.

use super::{
    BUTTON_RADIUS, CAP_GAP, CAP_PADDING, FONT_SIZE, HELP_COLUMN_GAP, HELP_KEY_GAP, HELP_MARGIN,
    HELP_PADDING, HELP_RADIUS, HELP_ROW_HEIGHT, HELP_TITLE_HEIGHT, KEY_FONT_SIZE, Painter, Palette,
    Rect, SHADOW, SMALL_FONT_SIZE, rect_px,
};
use crate::Action;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The pinned panel's title line, above the prefix line.
const HELP_PINNED_TITLE: i32 = 30;
/// The gap between the prefix keycap and the words after it, and between
/// the Esc keycap and its word.
const HELP_WORD_GAP: i32 = 10;
const HELP_ESC_GAP: i32 = 6;

/// How far a clickable row or key reaches past what it shows, sideways.
const HELP_SPOT_PAD: i32 = 6;
const HELP_KEY_PAD: i32 = 3;
/// How far the ground under a hovered row or key keeps from the row's
/// top and bottom.
const HELP_GROUND_INSET: i32 = 2;

/// What the help panel shows: the prefix, and the keys to press after it
/// as (keys, action). Alternative keys are split by " / ".
#[derive(Debug, PartialEq, Eq)]
pub struct Help {
    pub prefix: String,
    pub rows: Vec<(String, String)>,
    /// Per row, per key (as split by " / "), what a click on it does;
    /// `None` for a key that stands for several (`1–9`) or for one a
    /// click can't press.
    pub presses: Vec<Vec<Option<HelpPress>>>,
}

/// What a click on a key in the help panel does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpPress {
    Action(Action),
    /// Put the panel away.
    Close,
}

/// What the pointer is over in the help panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpHit {
    /// A row whose keys all do the same, by index into `Help::rows`.
    Row(usize),
    /// A key of a row whose keys do different things (`n / p`): row, key.
    Key(usize, usize),
    /// The pinned panel's way out.
    Close,
    /// Inside the panel, on nothing to click.
    Panel,
}

impl Help {
    /// The key for the action the panel calls `label`, the `nth` of its
    /// alternatives, with the prefix before it: "Ctrl+B Shift+T". None
    /// for an action the panel does not list, or fewer alternatives.
    pub fn key_for(&self, label: &str, nth: usize) -> Option<String> {
        let (keys, _) = self.rows.iter().find(|(_, l)| l == label)?;
        let key = keys.split(" / ").nth(nth)?;
        Some(format!("{} {key}", self.prefix))
    }

    /// What a click on `hit` does, if anything.
    pub fn press(&self, hit: HelpHit) -> Option<HelpPress> {
        let at = |row: usize, key: usize| *self.presses.get(row)?.get(key)?;
        match hit {
            HelpHit::Row(row) => at(row, 0),
            HelpHit::Key(row, key) => at(row, key),
            HelpHit::Close => Some(HelpPress::Close),
            HelpHit::Panel => None,
        }
    }

    /// Whether row `row`'s keys all do one thing, so the row is clicked
    /// as a whole.
    fn one_press(&self, row: usize) -> bool {
        self.presses.get(row).is_some_and(|keys| {
            keys.first()
                .is_some_and(|first| first.is_some() && keys.iter().all(|k| k == first))
        })
    }
}

/// Where the help panel of `layout` goes in the app `area`: centred at
/// its bottom.
pub fn help_rect(layout: &HelpLayout, area: Rect) -> Rect {
    Rectangle::new(
        (
            area.loc.x + (area.size.w - layout.size.w) / 2,
            (area.loc.y + area.size.h - layout.size.h - HELP_MARGIN).max(area.loc.y),
        )
            .into(),
        layout.size,
    )
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

    /// Where the help panel of `layout` with its top left at `origin`
    /// takes clicks: a row whose keys all do one thing as a whole, else
    /// each key that does something; on the pinned panel, its way out.
    pub(super) fn help_spots(
        &self,
        help: &Help,
        layout: &HelpLayout,
        origin: Point<i32, Logical>,
        scale: f64,
    ) -> Vec<(HelpHit, Rect)> {
        let s = scale as f32;
        let width = |text: &str, size: f32| self.text_width(text, size, scale) / s;
        let mut spots = Vec::new();
        if layout.pinned {
            let max_x = origin.x + layout.size.w - HELP_PADDING;
            let close_w = width("close", SMALL_FONT_SIZE);
            let esc_w = self.keycaps_width("Esc", scale) / s;
            let esc_x = (max_x as f32 - close_w - HELP_ESC_GAP as f32 - esc_w).floor() as i32;
            let rect = Rectangle::new(
                (esc_x - HELP_KEY_PAD, origin.y + HELP_PADDING / 2).into(),
                (max_x - esc_x + 2 * HELP_KEY_PAD, HELP_PINNED_TITLE).into(),
            );
            spots.push((HelpHit::Close, rect));
        }
        let head = layout.head();
        for (i, (keys, _)) in help.rows.iter().enumerate() {
            let (column, row) = (i as i32 / layout.rows, i as i32 % layout.rows);
            let x = origin.x + HELP_PADDING + column * (layout.column_width + HELP_COLUMN_GAP);
            let y = origin.y + head + row * HELP_ROW_HEIGHT;
            if help.one_press(i) {
                let rect = Rectangle::new(
                    (x - HELP_SPOT_PAD, y).into(),
                    (layout.column_width + 2 * HELP_SPOT_PAD, HELP_ROW_HEIGHT).into(),
                );
                spots.push((HelpHit::Row(i), rect));
                continue;
            }
            let presses = help.presses.get(i);
            let sep = width("/", KEY_FONT_SIZE) + 2.0 * CAP_GAP as f32;
            let mut pen = x as f32;
            for (k, key) in keys.split(" / ").enumerate() {
                let cap = width(key, KEY_FONT_SIZE) + 2.0 * CAP_PADDING as f32;
                if presses
                    .and_then(|keys| keys.get(k))
                    .is_some_and(|p| p.is_some())
                {
                    let left = pen.floor() as i32 - HELP_KEY_PAD;
                    let right = (pen + cap).ceil() as i32 + HELP_KEY_PAD;
                    let rect =
                        Rectangle::new((left, y).into(), (right - left, HELP_ROW_HEIGHT).into());
                    spots.push((HelpHit::Key(i, k), rect));
                }
                pen += cap + sep;
            }
        }
        spots
    }

    /// The panel inside a margin of `SHADOW` holding its drop shadow, what
    /// the pointer is on (`hover`) on a lighter ground.
    pub(super) fn help_panel(
        &self,
        help: &Help,
        layout: &HelpLayout,
        hover: Option<HelpHit>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = self.popup(layout.size, HELP_RADIUS, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);
        let (ox, oy) = (px(SHADOW), px(SHADOW));
        let ground = self
            .help_spots(help, layout, (SHADOW, SHADOW).into(), scale)
            .into_iter()
            .find(|(hit, _)| Some(*hit) == hover);
        if let Some((_, rect)) = ground {
            let inset = HELP_GROUND_INSET;
            let rect = Rectangle::new(
                (rect.loc.x, rect.loc.y + inset).into(),
                (rect.size.w, rect.size.h - 2 * inset).into(),
            );
            let (x, y, w, h) = rect_px(rect, scale);
            c.rounded(x, y, w, h, BUTTON_RADIUS * scale as f32, p.selected);
        }

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
    use super::{HELP_PINNED_TITLE, Help, HelpHit, HelpPress, Painter};
    use crate::Action;

    #[test]
    fn the_pinned_panel_is_taller_by_its_title() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let help = Help {
            prefix: "Ctrl+B".into(),
            rows: vec![("c".into(), "new tab".into()), ("q".into(), "quit".into())],
            presses: Vec::new(),
        };
        let armed = painter.help_layout(&help, 800, false, 1.0);
        let pinned = painter.help_layout(&help, 800, true, 1.0);
        assert_eq!(pinned.size.w, armed.size.w);
        assert_eq!(pinned.size.h, armed.size.h + HELP_PINNED_TITLE);
        assert!(pinned.pinned && !armed.pinned);
    }

    #[test]
    fn rows_doing_one_thing_click_whole_and_others_by_key() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let new_tab = Some(HelpPress::Action(Action::NewTab));
        let help = Help {
            prefix: "Ctrl+B".into(),
            rows: vec![
                ("c / t".into(), "new tab".into()),
                ("n / p".into(), "next / previous tab".into()),
                ("1–9".into(), "go to tab".into()),
                ("Esc".into(), "cancel".into()),
            ],
            presses: vec![
                vec![new_tab, new_tab],
                vec![
                    Some(HelpPress::Action(Action::NextTab)),
                    Some(HelpPress::Action(Action::PrevTab)),
                ],
                vec![None],
                vec![Some(HelpPress::Close)],
            ],
        };
        let layout = painter.help_layout(&help, 300, true, 1.0);
        let spots = painter.help_spots(&help, &layout, (100, 200).into(), 1.0);
        let hits: Vec<HelpHit> = spots.iter().map(|(hit, _)| *hit).collect();
        assert_eq!(
            hits,
            [
                HelpHit::Close,
                HelpHit::Row(0),
                HelpHit::Key(1, 0),
                HelpHit::Key(1, 1),
                HelpHit::Row(3),
            ]
        );
        // The two keys of a row side by side, apart.
        let (n, p) = (spots[2].1, spots[3].1);
        assert!(n.loc.x + n.size.w <= p.loc.x, "{n:?} {p:?}");
        assert_eq!(
            help.press(HelpHit::Key(1, 1)),
            Some(HelpPress::Action(Action::PrevTab))
        );
        assert_eq!(help.press(HelpHit::Row(0)), new_tab);
        assert_eq!(help.press(HelpHit::Close), Some(HelpPress::Close));
        assert_eq!(help.press(HelpHit::Panel), None);
        // Everything inside the panel.
        let panel = smithay::utils::Rectangle::new((100, 200).into(), layout.size);
        for (hit, rect) in &spots {
            assert!(panel.contains_rect(*rect), "{hit:?} {rect:?} {panel:?}");
        }
    }
}
