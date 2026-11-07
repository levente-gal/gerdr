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
    Gerdr,
    chrome::{FeatureTabHit, TabArea, TabColumn, TextPick, TextSpot},
    input::{MULTI_CLICK, MULTI_CLICK_SLOP},
};

/// What a selection copied says it copied.
fn copied_note(text: &str) -> String {
    match text.lines().count() {
        n @ 2.. => format!("Copied {n} lines"),
        _ => "Copied 1 line".into(),
    }
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
    /// on the page's text, it starts selecting there; twice, it selects
    /// the word. Anywhere else, what was selected goes.
    pub(crate) fn press_page_text(&mut self, pos: Point<f64, Logical>) {
        let spot = self.on_page_text(pos).then(|| self.feature_tab_parts()).flatten();
        let spot = spot.and_then(|(view, layout)| {
            let at = layout.text_at(&view, pos.to_i32_round(), false)?;
            Some((at, layout.line_text(at.line)?.to_owned()))
        });
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let had = panel.pick.take().is_some();
        panel.picking = false;
        let Some((at, text)) = spot else {
            panel.pick_clicks = None;
            if had {
                self.sync_feature_page();
            }
            return;
        };
        let now = Instant::now();
        let count = match panel.pick_clicks {
            Some((when, from, count))
                if now.duration_since(when) < MULTI_CLICK
                    && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                    && (from.y - pos.y).abs() < MULTI_CLICK_SLOP =>
            {
                count + 1
            }
            _ => 1,
        };
        panel.pick_clicks = Some((now, pos, count));
        let said = [text.clone(), text.clone()];
        panel.pick = match count {
            1 => {
                panel.picking = true;
                Some(TextPick {
                    anchor: at,
                    cursor: at,
                    said,
                })
            }
            _ => crate::find::word_at(&text, at.ch).map(|word| TextPick {
                anchor: TextSpot {
                    line: at.line,
                    ch: word.start,
                },
                cursor: TextSpot {
                    line: at.line,
                    ch: word.end,
                },
                said,
            }),
        };
        self.sync_feature_page();
    }

    /// Whether a press on the page's text is held, selecting.
    pub(crate) fn picking_page_text(&self) -> bool {
        self.features.panel.as_ref().is_some_and(|p| p.picking)
    }

    /// The pointer moved to `pos` holding a press on the page's text: the
    /// selection reaches the char nearest it. Past the page's top or
    /// bottom, the page scrolls that way.
    pub(crate) fn drag_page_text(&mut self, pos: Point<f64, Logical>) {
        let Some((view, layout)) = self.feature_tab_parts() else {
            return;
        };
        let at = pos.to_i32_round();
        let place = layout.columns[TabColumn::Page as usize];
        let spot = layout.text_at(&view, at, true);
        let said = spot.and_then(|s| layout.line_text(s.line)).map(str::to_owned);
        let room = f64::from(layout.scroll_room(TabColumn::Page));
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        if let (Some(spot), Some(said)) = (spot, said)
            && let Some(pick) = panel.pick.as_mut()
        {
            pick.cursor = spot;
            pick.said[1] = said;
        }
        if let Some(place) = place {
            let (top, bottom) = (place.loc.y, place.loc.y + place.size.h);
            let past = match at.y {
                y if y < top => f64::from(y - top),
                y if y > bottom => f64::from(y - bottom),
                _ => 0.0,
            };
            let scroll = &mut panel.scroll[TabColumn::Page as usize];
            let to = (scroll.pos() + past).clamp(0.0, room);
            if past != 0.0 && to != scroll.pos() {
                scroll.jump(to);
            }
        }
        self.sync_feature_page();
    }

    /// The press selecting the page's text is let go: a click that went
    /// nowhere selects nothing.
    pub(crate) fn release_page_text(&mut self) {
        if let Some(panel) = self.features.panel.as_mut()
            && panel.picking
        {
            panel.picking = false;
            if panel.pick.as_ref().is_some_and(|p| p.anchor == p.cursor) {
                panel.pick = None;
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
        let bare = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let ctrl = mods.ctrl && !(mods.alt || mods.logo || mods.shift);
        let had = panel.pick.is_some();
        let picked = self.picked_page_text();
        if let Some(text) = &picked {
            let copy = (ctrl && matches!(sym, Keysym::c | Keysym::C)) || (bare && sym == Keysym::y);
            if copy {
                let note = copied_note(text);
                self.copy_text(text.clone());
                self.show_success(note);
                return true;
            }
            if bare && sym == Keysym::Escape {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.pick = None;
                }
                self.sync_feature_page();
                return true;
            }
        } else if had && let Some(panel) = self.features.panel.as_mut() {
            // Its page changed under it.
            panel.pick = None;
        }
        let star = [sym, typed].iter().any(|k| matches!(*k, Keysym::asterisk | Keysym::KP_Multiply));
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
        self.go_to_match(back, here.is_none());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
