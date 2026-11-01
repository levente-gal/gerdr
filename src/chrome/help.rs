//! The key help panel shown while the prefix is armed, or pinned open
//! with the Shortcuts action: then it carries a title and a way out.

use super::{
    CAP_GAP, CAP_PADDING, FONT_SIZE, HELP_COLUMN_GAP, HELP_KEY_GAP, HELP_MARGIN, HELP_PADDING,
    HELP_ROW_HEIGHT, HELP_TITLE_HEIGHT, KEY_FONT_SIZE, POPUP_RADIUS, Painter, Palette, Rect,
    SHADOW, SMALL_FONT_SIZE, keycap_text,
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
    /// Nothing: the key doesn't work over what has the keys now (a
    /// panel, the welcome screen). Its row is greyed out.
    NotHere,
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

/// How far the keys of a row that doesn't work here fade into the panel.
const HELP_GREY_FADE: f32 = 0.55;

/// The most columns the help panel splits into to fit the height it has.
const HELP_MAX_COLUMNS: usize = 3;

impl Help {
    /// The key for the action the panel calls `label` (in any case, or
    /// as it was called before its labels started with a verb), the
    /// `nth` of its alternatives, with the prefix before it: "Ctrl+B
    /// Shift+T". None for an action the panel does not list, or fewer
    /// alternatives. A key listed without the modifiers it shares with
    /// the first ("Alt+H / J") gets them back.
    pub fn key_for(&self, label: &str, nth: usize) -> Option<String> {
        let label = crate::keys::help_label(label);
        let (keys, _) = self
            .rows
            .iter()
            .find(|(_, l)| l.eq_ignore_ascii_case(label))?;
        self.nth_key(keys, nth)
    }

    /// The key that runs `action`, the `nth` of those that do, with the
    /// prefix before it: "Ctrl+B C". None when fewer keys run it.
    pub fn key_of(&self, action: Action, nth: usize) -> Option<String> {
        let wanted = Some(HelpPress::Action(action));
        self.rows
            .iter()
            .zip(&self.presses)
            .flat_map(|((keys, _), presses)| {
                let count = keys.split(" / ").count();
                (0..count)
                    .filter(move |k| presses.get(*k).copied().flatten() == wanted)
                    .map(move |k| (keys, k))
            })
            .nth(nth)
            .and_then(|(keys, k)| self.nth_key(keys, k))
    }

    /// The `nth` key of a row's `keys`, with the modifiers of the first
    /// it was written without, after the prefix.
    fn nth_key(&self, keys: &str, nth: usize) -> Option<String> {
        let mut all = keys.split(" / ");
        let first = all.clone().next()?;
        let key = all.nth(nth)?;
        // "Ctrl++" is Ctrl and "+": its modifiers run up to the last key.
        let mods = |key: &str| match key.strip_suffix('+') {
            Some(rest) if rest.is_empty() || rest.ends_with('+') => rest.len(),
            _ => key.rfind('+').map_or(0, |i| i + 1),
        };
        let shared = &first[..mods(first)];
        let key = if nth > 0 && mods(key) == 0 && !shared.is_empty() {
            format!("{shared}{key}")
        } else {
            key.to_owned()
        };
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

    /// Whether none of row `row`'s keys work where the prefix was
    /// pressed: it shows greyed out, and takes no click.
    pub fn greyed(&self, row: usize) -> bool {
        self.presses.get(row).is_some_and(|keys| {
            !keys.is_empty() && keys.iter().all(|k| *k == Some(HelpPress::NotHere))
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
    /// Rows in a column.
    pub rows: i32,
    pub key_width: i32,
    pub column_width: i32,
    /// Pinned open: a title and an Esc hint over the prefix line.
    pub pinned: bool,
    /// How many of the help's rows show: all of them, unless even split
    /// into columns they don't fit the height there is. Then the last
    /// place says how many more there are.
    pub shown: usize,
    /// Per row, whether it is greyed out (see `Help::greyed`): a panel
    /// drawn for other rows greyed is another.
    pub greyed: Vec<bool>,
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
    /// Size the help panel: two columns when they fit in `max_width`, else
    /// one, as tall as it needs.
    #[cfg_attr(not(test), allow(dead_code))] // Tests only, once chrome uses `help_layout_within`.
    pub fn help_layout(&self, help: &Help, max_width: i32, pinned: bool, scale: f64) -> HelpLayout {
        self.help_layout_within(help, (max_width, i32::MAX).into(), pinned, scale)
    }

    /// Size the help panel to fit `max` (the app area, less its margins):
    /// two columns when they fit its width, more when that is what it
    /// takes to fit its height, and if even then it is too tall, only the
    /// rows that fit, the last place saying how many more there are.
    pub fn help_layout_within(
        &self,
        help: &Help,
        max: Size<i32, Logical>,
        pinned: bool,
        scale: f64,
    ) -> HelpLayout {
        let max_width = max.w;
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
        let fits_width = |columns: usize| {
            columns as i32 * column_width
                + (columns as i32 - 1) * HELP_COLUMN_GAP
                + 2 * HELP_PADDING
                <= max_width
        };
        let head = if pinned {
            HELP_PINNED_TITLE + HELP_TITLE_HEIGHT
        } else {
            HELP_TITLE_HEIGHT
        };
        // How many rows a column has room for.
        let room = ((max.h.saturating_sub(head + HELP_PADDING)) / HELP_ROW_HEIGHT).max(1) as usize;
        let n = help.rows.len();
        let mut columns = if fits_width(2) { 2 } else { 1 };
        while n.div_ceil(columns) > room && columns < HELP_MAX_COLUMNS && fits_width(columns + 1) {
            columns += 1;
        }
        let rows = n.div_ceil(columns).min(room).max(1);
        let shown = n.min(rows * columns);
        let w = columns as i32 * column_width
            + (columns as i32 - 1) * HELP_COLUMN_GAP
            + 2 * HELP_PADDING;
        let mut layout = HelpLayout {
            size: (w, 0).into(),
            rows: rows as i32,
            key_width,
            column_width,
            pinned,
            // The last place says how many more there are.
            shown: if shown < n { shown - 1 } else { n },
            greyed: (0..n).map(|row| help.greyed(row)).collect(),
        };
        layout.size.h = layout.head() + layout.rows * HELP_ROW_HEIGHT + HELP_PADDING;
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
        for (i, (keys, _)) in help.rows.iter().enumerate().take(layout.shown) {
            if help.greyed(i) {
                continue;
            }
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
                let cap = width(&keycap_text(key), KEY_FONT_SIZE) + 2.0 * CAP_PADDING as f32;
                if presses
                    .and_then(|keys| keys.get(k))
                    .is_some_and(|p| p.is_some_and(|p| p != HelpPress::NotHere))
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
        let mut c = self.popup(layout.size, POPUP_RADIUS, scale, p);
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
            self.row_highlight(&mut c, rect, (false, 1.0), scale, p);
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

        let place = |i: usize| {
            let (column, row) = (i as i32 / layout.rows, i as i32 % layout.rows);
            let x = SHADOW + HELP_PADDING + column * (layout.column_width + HELP_COLUMN_GAP);
            let y = SHADOW + head + row * HELP_ROW_HEIGHT;
            (x, (px(y), px(HELP_ROW_HEIGHT)))
        };
        for (i, (keys, action)) in help.rows.iter().enumerate().take(layout.shown) {
            let (x, band) = place(i);
            let column_end = px(x + layout.column_width) as f32;
            let end = self.keycaps(&mut c, keys, px(x) as f32, band, scale, p);
            // A row that doesn't work here: its keys faded into the
            // panel, its words in the colour of what can't be used.
            let greyed = help.greyed(i);
            if greyed {
                for py in band.0..band.0 + band.1 {
                    for pxl in px(x)..end.ceil() as i32 {
                        c.blend(pxl, py, p.elevated, HELP_GREY_FADE);
                    }
                }
            }
            let colour = if greyed { p.placeholder } else { p.text };
            let action_x = px(x + layout.key_width + HELP_KEY_GAP) as f32;
            self.text(
                &mut c, action, FONT_SIZE, action_x, column_end, band, scale, colour,
            );
        }
        let more = help.rows.len() - layout.shown;
        if more > 0 {
            // Too many to show: the last place says how many are left out.
            let (x, band) = place(layout.shown);
            let column_end = px(x + layout.column_width) as f32;
            let text = format!("{more} more: give the window more room");
            self.text(
                &mut c,
                &text,
                SMALL_FONT_SIZE,
                px(x) as f32,
                column_end,
                band,
                scale,
                p.muted,
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
    fn a_short_area_splits_the_panel_and_then_cuts_it() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let help = Help {
            prefix: "Ctrl+B".into(),
            rows: (0..30).map(|i| ("c".into(), format!("row {i}"))).collect(),
            presses: Vec::new(),
        };
        let tall = painter.help_layout(&help, 4000, false, 1.0);
        assert_eq!((tall.rows, tall.shown), (15, 30));
        // Too short for two columns of 15: three of 10.
        let h = tall.size.h - 5 * super::HELP_ROW_HEIGHT;
        let split = painter.help_layout_within(&help, (4000, h).into(), false, 1.0);
        assert_eq!((split.rows, split.shown), (10, 30));
        assert!(split.size.h <= h && split.size.w > tall.size.w);
        // One column, too short for it all: as many as fit, then a line
        // saying how many more.
        let narrow = tall.column_width + 2 * super::HELP_PADDING;
        let cut = painter.help_layout_within(&help, (narrow, h).into(), false, 1.0);
        assert_eq!(cut.rows, 10);
        assert_eq!(cut.shown, 9);
        assert!(cut.size.h <= h);
    }

    #[test]
    fn keys_are_found_by_label_old_or_new_and_by_action() {
        let help = Help {
            prefix: "Ctrl+B".into(),
            rows: vec![
                ("C".into(), "Open a new tab".into()),
                ("Alt+H / J".into(), "Swap pane left / down".into()),
            ],
            presses: vec![
                vec![Some(HelpPress::Action(Action::NewTab))],
                vec![
                    Some(HelpPress::Action(Action::NextTab)),
                    Some(HelpPress::Action(Action::PrevTab)),
                ],
            ],
        };
        assert_eq!(help.key_for("open a new tab", 0), Some("Ctrl+B C".into()));
        assert_eq!(help.key_for("new tab", 0), Some("Ctrl+B C".into()));
        assert_eq!(help.key_for("new tab", 1), None);
        // The second key gets the modifiers it shares with the first.
        assert_eq!(
            help.key_for("Swap pane left / down", 1),
            Some("Ctrl+B Alt+J".into())
        );
        assert_eq!(help.key_of(Action::NewTab, 0), Some("Ctrl+B C".into()));
        assert_eq!(help.key_of(Action::PrevTab, 0), Some("Ctrl+B Alt+J".into()));
        assert_eq!(help.key_of(Action::Quit, 0), None);
    }

    #[test]
    fn a_greyed_row_takes_no_click_and_draws_another_panel() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let quit = Some(HelpPress::Action(Action::Quit));
        let mut help = Help {
            prefix: "Ctrl+B".into(),
            rows: vec![
                ("c".into(), "new tab".into()),
                ("n / p".into(), "next / previous tab".into()),
                ("q".into(), "quit".into()),
            ],
            presses: vec![
                vec![Some(HelpPress::NotHere)],
                vec![Some(HelpPress::NotHere); 2],
                vec![quit],
            ],
        };
        assert!(help.greyed(0) && help.greyed(1) && !help.greyed(2));
        let layout = painter.help_layout(&help, 800, false, 1.0);
        assert_eq!(layout.greyed, [true, true, false]);
        let spots = painter.help_spots(&help, &layout, (0, 0).into(), 1.0);
        let hits: Vec<HelpHit> = spots.iter().map(|(hit, _)| *hit).collect();
        assert_eq!(hits, [HelpHit::Row(2)]);
        // The same rows working: the same size, but not the same panel.
        help.presses[0] = vec![Some(HelpPress::Action(Action::NewTab))];
        let working = painter.help_layout(&help, 800, false, 1.0);
        assert_eq!(working.size, layout.size);
        assert_ne!(working, layout);
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
