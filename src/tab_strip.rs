//! Input to the tab strip: presses on the tabs, the "+" button and its
//! menu chevron, and the theme switch (see `chrome::tabs` for what it
//! shows).

use crate::{
    Gerdr, Reorderable, chrome,
    input::{BTN_LEFT, BTN_MIDDLE},
    launch::Place,
};

impl Gerdr {
    /// A press at `(x, y)` in the tab strip (strip coordinates). Left
    /// click selects a tab, opens a terminal in a new one with "+", or the
    /// run prompt for an app in a new one with the chevron after it; "×"
    /// or middle click closes a tab. A tab held with the left button can
    /// be dragged to another place. The button at the right end switches
    /// light/dark.
    pub(crate) fn tab_strip_press(&mut self, x: i32, y: i32, button: u32) {
        let width = self.output_size().w - self.sidebar_width;
        let inside =
            |r: &smithay::utils::Rectangle<i32, smithay::utils::Logical>| r.contains((x, y));
        if button == BTN_LEFT && inside(&chrome::theme_toggle(width)) {
            self.toggle_theme();
            return;
        }
        let count = self.workspace().map_or(0, |ws| ws.tabs.len());
        if count == 0 {
            return;
        }
        let (tabs, plus, chevron) = chrome::tab_layout(count, width);
        let hit = tabs.iter().position(|(tab, _)| inside(tab));
        match (button, hit) {
            (BTN_LEFT, Some(idx)) if inside(&tabs[idx].1) => self.close_tab(idx),
            (BTN_LEFT, Some(idx)) => {
                self.switch_tab(idx);
                self.hold(Reorderable::Tab);
            }
            (BTN_MIDDLE, Some(idx)) => self.close_tab(idx),
            (BTN_LEFT, None) if inside(&plus) => self.new_tab(),
            (BTN_LEFT, None) if inside(&chevron) => self.open_run_prompt(Place::Tab),
            _ => {}
        }
    }
}
