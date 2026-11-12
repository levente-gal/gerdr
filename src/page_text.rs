//! Text on the feature tab's page, selected with the pointer and copied,
//! as in the scrollback viewer: a press on it and a drag select from
//! char to char, a double click selects a word, and Ctrl+C or `y` copies
//! what is selected. On Manual test's page `*` and `#` search the
//! criteria for it (with nothing selected, for the word under the
//! pointer, as a word of its own), and go to the place found after it or
//! before it, as vim's do.

use smithay::input::keyboard::{Keysym, ModifiersState};
use smithay::utils::{Logical, Point};
use std::time::Instant;

use crate::{
    Gerdr, chrome,
    chrome::{FeatureTabHit, TabArea, TabColumn, TextPick, TextSpot},
    key_table::{self, Binding},
    scrollback::Grain,
};

/// What `grain` takes of `text` (the line at `at`) at `at`: the place
/// itself, the word or the gap between words there, or the whole line.
fn span(text: &str, at: TextSpot, grain: Grain) -> (TextSpot, TextSpot) {
    let spot = |ch| TextSpot { line: at.line, ch };
    let cs: Vec<char> = text.chars().collect();
    match grain {
        Grain::Char => (at, at),
        Grain::Line => (spot(0), spot(cs.len())),
        Grain::Word => {
            let Some(&c) = cs.get(at.ch).or(cs.last()) else {
                return (at, at);
            };
            let i = at.ch.min(cs.len() - 1);
            let word = |d: char| d.is_alphanumeric() || d == '_';
            let same = |d: char| word(d) == word(c) && d.is_whitespace() == c.is_whitespace();
            let start = (0..i).rev().find(|&j| !same(cs[j])).map_or(0, |j| j + 1);
            let end = (i..cs.len()).find(|&j| !same(cs[j])).unwrap_or(cs.len());
            (spot(start), spot(end))
        }
    }
}

/// A selection's anchor and cursor, from the span its press took (`home`)
/// to the span the pointer is on: that one's end after it, else its start.
fn reach(home: (TextSpot, TextSpot), to: (TextSpot, TextSpot)) -> (TextSpot, TextSpot) {
    if to.0 < home.0 {
        (home.1, to.0)
    } else {
        (home.0, to.1.max(home.1))
    }
}

/// What a selection copied says it copied.
fn copied_note(text: &str) -> String {
    match text.lines().count() {
        n @ 2.. => format!("Copied {n} lines"),
        _ => "Copied 1 line".into(),
    }
}

/// What a key does to the text selected on the page (`PICK_KEYS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickKey {
    Copy,
    /// Let it go.
    Unselect,
    /// Search the criteria for it, going to the place found after it, or
    /// (`true`) before it.
    Star(bool),
}

/// The page as the keys of its text read it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PickCtx {
    /// Text is selected.
    pub picked: bool,
    /// Manual test's criteria show, to search.
    pub criteria: bool,
}

/// The keys of the text selected on the feature tab's page; `*` and `#`
/// search the criteria with nothing selected too.
pub(crate) const PICK_KEYS: &[Binding<PickCtx, PickKey>] = &[
    Binding {
        chord: "Ctrl+C",
        does: |c| c.picked.then_some(PickKey::Copy),
        footer: true,
        f1: "copy the text selected with the pointer",
    },
    Binding {
        chord: "y",
        does: |c| c.picked.then_some(PickKey::Copy),
        footer: true,
        f1: "copy it, as Ctrl+C",
    },
    Binding {
        chord: "*",
        does: |c| c.criteria.then_some(PickKey::Star(false)),
        footer: true,
        f1: "search for the text selected, else the word under the pointer",
    },
    Binding {
        chord: "#",
        does: |c| c.criteria.then_some(PickKey::Star(true)),
        footer: true,
        f1: "the same, going to the place found before it",
    },
    Binding {
        chord: "Esc",
        does: |c| c.picked.then_some(PickKey::Unselect),
        footer: true,
        f1: "let the selection go",
    },
];

/// The footer's hints for text selected on the page, `criteria` showing
/// or not: "Ctrl+C / y" copy, "* / #" search it, Esc unselect.
pub(crate) fn pick_hints(criteria: bool) -> Vec<(String, String)> {
    let ctx = PickCtx {
        picked: true,
        criteria,
    };
    let label = |k: &PickKey| match k {
        PickKey::Copy => "copy",
        PickKey::Unselect => "unselect",
        PickKey::Star(_) => "search it",
    };
    paired(key_table::hints(PICK_KEYS, &ctx, |k| label(k).to_owned()))
}

/// `hints` with each run of keys doing the same, by their words, as one
/// hint of their keys ("Ctrl+C / y"), as the footer lists them.
pub(crate) fn paired(hints: Vec<(&'static str, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (key, what) in hints {
        match out.last_mut() {
            Some((keys, said)) if *said == what => {
                keys.push_str(" / ");
                keys.push_str(key);
            }
            _ => out.push((key.to_owned(), what)),
        }
    }
    out
}

impl Gerdr {
    /// The text selected on the feature tab's page, while the page still
    /// says what it did when selected.
    pub(crate) fn picked_page_text(&self) -> Option<String> {
        self.features.panel.as_ref()?.pick.as_ref()?;
        let (view, layout) = self.feature_tab_parts()?;
        layout.picked_text(&view)
    }

    /// Whether `pos` is over text on the feature tab's page that a press
    /// selects from: not over something that acts.
    pub(crate) fn on_page_text(&self, pos: Point<f64, Logical>) -> bool {
        let Some((view, layout)) = self.feature_tab_parts() else {
            return false;
        };
        let hit = layout.hit(&view, pos.to_i32_round());
        matches!(hit, Some(FeatureTabHit::Column(TabColumn::Page) | FeatureTabHit::Finding(_)))
            && layout.text_at(&view, pos.to_i32_round(), false).is_some()
    }

    /// A press on the feature tab at `pos`, after it did what it does:
    /// on the page's text, it starts selecting there, by chars; a second
    /// in a row by words and a third by lines, from the one it is on; with
    /// Shift, what was selected reaches there instead. Anywhere else, what
    /// was selected goes.
    pub(crate) fn press_page_text(&mut self, pos: Point<f64, Logical>) {
        let spot = self.on_page_text(pos).then(|| self.feature_tab_parts()).flatten();
        let spot = spot.and_then(|(view, layout)| {
            let at = layout.text_at(&view, pos.to_i32_round(), false)?;
            Some((at, layout.line_text(at.line)?.to_owned()))
        });
        let shift = self.seat.get_keyboard().is_some_and(|k| k.modifier_state().shift);
        let count = self.press_count.max(1);
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let had = panel.pick.take();
        (panel.picking, panel.pick_edge) = (false, None);
        let Some((at, text)) = spot else {
            panel.pick_by = None;
            if had.is_some() {
                self.sync_feature_page();
            }
            return;
        };
        panel.picking = true;
        match (shift, had, panel.pick_by) {
            (true, Some(mut pick), Some((grain, home))) => {
                pick.said[1] = text.clone();
                (pick.anchor, pick.cursor) = reach(home, span(&text, at, grain));
                panel.pick = Some(pick);
            }
            _ => {
                let grain = match count {
                    1 => Grain::Char,
                    2 => Grain::Word,
                    _ => Grain::Line,
                };
                let home = span(&text, at, grain);
                panel.pick_by = Some((grain, home));
                panel.pick = Some(TextPick {
                    anchor: home.0,
                    cursor: home.1,
                    said: [text.clone(), text],
                });
            }
        }
        self.sync_feature_page();
    }

    /// Whether a press on the page's text is held, selecting.
    pub(crate) fn picking_page_text(&self) -> bool {
        self.features.panel.as_ref().is_some_and(|p| p.picking)
    }

    /// The pointer moved to `pos` holding a press on the page's text: the
    /// selection reaches the char nearest it in view, by what the press
    /// selected by (see `press_page_text`). Past the page's top or bottom
    /// the page scrolls on (`page_text_edge_tick`), not the selection.
    pub(crate) fn drag_page_text(&mut self, pos: Point<f64, Logical>) {
        let Some((view, layout)) = self.feature_tab_parts() else {
            return;
        };
        let mut at: Point<i32, Logical> = pos.to_i32_round();
        if let Some(place) = layout.columns[TabColumn::Page as usize] {
            at.y = at.y.clamp(place.loc.y, place.loc.y + place.size.h - 1);
        }
        let Some(spot) = layout.text_at(&view, at, true) else {
            return;
        };
        let Some(said) = layout.line_text(spot.line).map(str::to_owned) else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        if let Some(pick) = panel.pick.as_mut()
            && let Some((grain, home)) = panel.pick_by
        {
            (pick.anchor, pick.cursor) = reach(home, span(&said, spot, grain));
            pick.said[1] = said;
        }
        self.sync_feature_page();
    }

    /// Every frame: a press selecting the page's text held past the page's
    /// top or bottom scrolls it on that way, moving or not, faster the
    /// further past, and the selection with it. Whether it is past.
    pub(crate) fn page_text_edge_tick(&mut self, now: Instant) -> bool {
        if !self.picking_page_text() {
            return false;
        }
        let Some((_, layout)) = self.feature_tab_parts() else {
            return false;
        };
        let col = TabColumn::Page as usize;
        let room = f64::from(layout.scroll_room(TabColumn::Page));
        let y: f64 = self.pointer.y;
        let edge = layout.columns[col].map_or(0.0, |place| {
            let (top, bottom) = (f64::from(place.loc.y), f64::from(place.loc.y + place.size.h));
            match y {
                y if y < top => y - top,
                y if y > bottom => y - bottom,
                _ => 0.0,
            }
        });
        let Some(panel) = self.features.panel.as_mut() else {
            return false;
        };
        let by = crate::scrolling::edge_steps(&mut panel.pick_edge, now, edge);
        if by != 0 {
            let scroll = &mut panel.scroll[col];
            let to = (scroll.pos() + (by * i64::from(chrome::BODY_LINE)) as f64).clamp(0.0, room);
            if to != scroll.pos() {
                scroll.jump(to);
                self.drag_page_text(self.pointer);
            }
        }
        edge != 0.0
    }

    /// The press selecting the page's text is let go: a click that went
    /// nowhere selects nothing.
    pub(crate) fn release_page_text(&mut self) {
        if let Some(panel) = self.features.panel.as_mut()
            && panel.picking
        {
            (panel.picking, panel.pick_edge) = (false, None);
            if panel.pick.as_ref().is_some_and(|p| p.anchor == p.cursor) {
                (panel.pick, panel.pick_by) = (None, None);
            }
        }
    }

    /// A key for the text selected on the feature tab's page: Ctrl+C and
    /// `y` copy it, Esc lets it go; and `*` and `#`, searching the criteria
    /// for it. False when the key isn't one of those.
    pub(crate) fn page_text_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> bool {
        let Some(panel) = self.features.panel.as_ref() else {
            return false;
        };
        let typing = panel.search.as_ref().is_some_and(|s| s.typing);
        if panel.note.is_some() || typing || panel.keys {
            return false;
        }
        let had = panel.pick.is_some();
        let picked = self.picked_page_text();
        if picked.is_none()
            && had
            && let Some(panel) = self.features.panel.as_mut()
        {
            // Its page changed under it.
            panel.pick = None;
        }
        let ctx = PickCtx {
            picked: picked.is_some(),
            criteria: self.criteria_search_shows(),
        };
        match key_table::press(PICK_KEYS, &ctx, mods, sym) {
            Some(PickKey::Copy) => {
                if let Some(text) = &picked {
                    let note = copied_note(text);
                    self.copy_text(text.clone());
                    self.show_success(note);
                }
                return true;
            }
            Some(PickKey::Unselect) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.pick = None;
                }
                self.sync_feature_page();
                return true;
            }
            Some(PickKey::Star(back)) => return self.star_criteria(back, picked),
            None => {}
        }
        // The keypad's `*`, and a `*` or `#` typed with AltGr, which no
        // chord names: matched on what they type.
        let star = [sym, typed]
            .iter()
            .any(|k| matches!(*k, Keysym::asterisk | Keysym::KP_Multiply));
        let hash = [sym, typed].contains(&Keysym::numbersign);
        if (star || hash) && !(mods.ctrl || mods.alt || mods.logo) {
            return self.star_criteria(hash, picked);
        }
        false
    }

    /// `*` (`back`: `#`) on Manual test's page: search the criteria for
    /// `picked`, else for the word under the pointer as a word of its own,
    /// and go to the place found after it (`back`: before it). False off
    /// that page.
    fn star_criteria(&mut self, back: bool, picked: Option<String>) -> bool {
        if !self.criteria_search_shows() {
            return false;
        }
        let Some((view, layout)) = self.feature_tab_parts() else {
            return false;
        };
        let pointer = self.seat.get_pointer().map(|p| p.current_location());
        let from = match (&picked, view.marks.pick.as_ref()) {
            (Some(text), Some(pick)) => {
                let query = text.split_whitespace().collect::<Vec<_>>().join(" ");
                Some((query, false, pick.ends().0))
            }
            _ => pointer.and_then(|pos| {
                let at = layout.text_at(&view, pos.to_i32_round(), false)?;
                let text = layout.line_text(at.line)?;
                let word = crate::find::word_at(text, at.ch)?;
                let query: String = text.chars().skip(word.start).take(word.len()).collect();
                let spot = TextSpot {
                    line: at.line,
                    ch: word.start,
                };
                Some((query, true, spot))
            }),
        };
        let Some((query, whole, spot)) = from.filter(|(q, ..)| !q.is_empty()) else {
            self.show_notice("No word under the pointer: point at one, or select text".into());
            return true;
        };
        let card = layout.texts.get(spot.line).and_then(|t| t.card);
        let said = layout.line_text(spot.line).map(str::to_owned);
        let Some(panel) = self.features.panel.as_mut() else {
            return true;
        };
        let search = panel.search.get_or_insert_with(Default::default);
        search.query.set(&query);
        (search.typing, search.whole, search.at) = (false, whole, None);
        panel.pick = None;
        panel.focus = TabArea::Findings;
        if let Some(card) = card {
            panel.finding = card;
        }
        self.settle_criteria(false);
        self.sync_feature_page();
        // Where it is now that the search shows it: the place it was
        // found from, to go on from.
        let Some((_, layout)) = self.feature_tab_parts() else {
            return true;
        };
        let here = layout.matches.iter().position(|m| {
            m.card == card
                && m.range.start == spot.ch
                && layout.line_text(m.line).map(str::to_owned) == said
        });
        if let Some(panel) = self.features.panel.as_mut()
            && let Some(search) = panel.search.as_mut()
        {
            search.at = here;
        }
        let from = match here {
            Some(_) => crate::manual_test::MatchFrom::Here,
            None => crate::manual_test::MatchFrom::Fresh,
        };
        self.go_to_match(back, from);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_click_drags_on_by_words_and_a_triple_one_by_lines() {
        let at = |line, ch| TextSpot { line, ch };
        let text = "fix the  login_page now";
        // A word, the gap between two, the whole line.
        assert_eq!(span(text, at(0, 5), Grain::Word), (at(0, 4), at(0, 7)));
        assert_eq!(span(text, at(0, 7), Grain::Word), (at(0, 7), at(0, 9)));
        assert_eq!(span(text, at(0, 12), Grain::Word), (at(0, 9), at(0, 19)));
        assert_eq!(span(text, at(0, 99), Grain::Word), (at(0, 20), at(0, 23)));
        assert_eq!(span(text, at(0, 5), Grain::Line), (at(0, 0), at(0, 23)));
        assert_eq!(span(text, at(0, 5), Grain::Char), (at(0, 5), at(0, 5)));
        assert_eq!(span("", at(0, 0), Grain::Word), (at(0, 0), at(0, 0)));
        // "the" double clicked, dragged on to "now": both words whole.
        let home = span(text, at(0, 5), Grain::Word);
        let to = span(text, at(0, 21), Grain::Word);
        assert_eq!(reach(home, to), (at(0, 4), at(0, 23)));
        // Dragged back to "fix": from its start to the end of "the".
        let to = span(text, at(0, 1), Grain::Word);
        assert_eq!(reach(home, to), (at(0, 7), at(0, 0)));
        // Back within "the", it alone.
        assert_eq!(reach(home, home), (at(0, 4), at(0, 7)));
        // By lines, to a line below: the first whole, to the other's end.
        let home = span(text, at(0, 5), Grain::Line);
        assert_eq!(reach(home, span("two", at(2, 1), Grain::Line)), (at(0, 0), at(2, 3)));
        // By chars, as the pointer goes.
        let home = (at(1, 3), at(1, 3));
        assert_eq!(reach(home, (at(0, 2), at(0, 2))), (at(1, 3), at(0, 2)));
        assert_eq!(reach(home, (at(1, 5), at(1, 5))), (at(1, 3), at(1, 5)));
    }

    #[test]
    fn a_copy_says_how_many_lines_it_took() {
        assert_eq!(copied_note("one"), "Copied 1 line");
        assert_eq!(copied_note("one\ntwo"), "Copied 2 lines");
    }

    #[test]
    fn the_footers_hints_press_the_keys_they_name() {
        // Each key of a hint is a keycap of its own.
        let key = |hint: &str| crate::keys::hint_key(hint).map(|(m, k)| (m.ctrl, k));
        assert_eq!(key("Ctrl+C"), Some((true, Keysym::c)));
        assert_eq!(key("y"), Some((false, Keysym::y)));
        assert!(key("*").is_some() && key("#").is_some());
        assert_eq!(key("n"), Some((false, Keysym::n)));
        assert!(key("N").is_some());
    }

    #[test]
    fn the_selections_keys_are_one_table_its_footer_reads() {
        let ctxs: Vec<PickCtx> = [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .map(|(picked, criteria)| PickCtx { picked, criteria })
            .collect();
        key_table::check(PICK_KEYS, &ctxs);
        let said = |hints: Vec<(String, String)>| -> Vec<String> {
            hints.into_iter().map(|(k, w)| format!("{k} {w}")).collect()
        };
        assert_eq!(
            said(pick_hints(true)),
            ["Ctrl+C / y copy", "* / # search it", "Esc unselect"]
        );
        assert_eq!(said(pick_hints(false)), ["Ctrl+C / y copy", "Esc unselect"]);
        // `*` and `#` as a press gives them on a US layout: Shift and the
        // digit's key (K8); with nothing selected they still search.
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        let star = key_table::press(PICK_KEYS, &ctxs[1], &shift, Keysym::_8);
        assert_eq!(star, Some(PickKey::Star(false)));
        let hash = key_table::press(PICK_KEYS, &ctxs[1], &shift, Keysym::_3);
        assert_eq!(hash, Some(PickKey::Star(true)));
        assert_eq!(
            key_table::press(PICK_KEYS, &ctxs[0], &shift, Keysym::_8),
            None
        );
    }
}
