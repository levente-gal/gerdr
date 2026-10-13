//! Forward host-window input to the focused client, except gerdr's own
//! shortcuts, clicks on the sidebar, the tab strip and the pane bars, drags of the dividers
//! (the sidebar edge and the gap between split panes), and drags of the
//! workspace rows and tabs to reorder them.
//!
//! Shortcuts follow herdr: press the prefix (Ctrl+B), release it, then one key
//! (see `keys` for the defaults, and how to change them). The prefix twice
//! sends it to the app; Esc or any other key cancels. While the prefix is
//! armed, a help panel lists the keys. While the quit dialog, the welcome
//! screen, the project menu or the switcher is open, every key, click and
//! scroll goes to it; over the welcome screen and the project menu, the
//! prefix still switches light/dark and quits, and over the project menu it
//! also closes the menu again.
//!
//! The sidebar header's project name opens the project menu, or the welcome
//! screen while no project is shown. A click on a terminal pane's bar
//! focuses the pane, and its buttons restart or stop what runs in it.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, CursorIcon, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::{
    Action, Drag, Gerdr, REORDER_START, Reorder, Reorderable, WorkspaceKind,
    chrome::{self, TAB_STRIP_HEIGHT},
    home_dir,
    launch::Place,
    layout::Dir,
    project_menu, quit,
    switcher::Target,
    welcome,
};

const BTN_LEFT: u32 = 0x110;
const BTN_MIDDLE: u32 = 0x112;

/// What a key gerdr kept from the apps does, once the keyboard is free again.
enum Intercepted {
    Action(Action),
    /// The switcher picked this, to launch in that place if it is an app.
    Switch(Target, Place),
    /// The welcome screen asked for this.
    Welcome(welcome::Outcome),
    /// The project menu asked for this.
    ProjectMenu(project_menu::Outcome),
    /// The quit dialog asked for this.
    Quit(quit::Outcome),
}

/// How far a touchpad scrolls the project menu's list by a row, in pixels.
const MENU_SCROLL_ROW: f64 = 40.0;

/// Whether `action`, picked with the prefix, works over the welcome screen:
/// only what acts on gerdr itself rather than on the project behind (the
/// welcome screen is already open).
fn over_welcome(action: &Action) -> bool {
    matches!(action, Action::ToggleTheme | Action::Quit)
}

/// Whether `action`, picked with the prefix, works over the project menu:
/// what acts on gerdr itself, and the key that opened the menu, to close it.
fn over_project_menu(action: &Action) -> bool {
    matches!(
        action,
        Action::ToggleTheme | Action::Quit | Action::Projects
    )
}

fn drag_cursor(drag: &Drag) -> CursorIcon {
    match drag {
        Drag::Sidebar
        | Drag::Split {
            dir: Dir::Right, ..
        } => CursorIcon::ColResize,
        Drag::Split { dir: Dir::Down, .. } => CursorIcon::RowResize,
    }
}

impl Gerdr {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let code = event.key_code();
                let key_state = event.state();
                let action = self.seat.get_keyboard().unwrap().input(
                    self,
                    code,
                    key_state,
                    serial,
                    time,
                    |state, mods, handle| {
                        if key_state == KeyState::Released {
                            if state.switcher_repeat.is_some_and(|(held, _)| held == code) {
                                state.stop_switcher_repeat();
                            }
                            if let Some(i) = state.suppressed_keys.iter().position(|k| *k == code) {
                                state.suppressed_keys.remove(i);
                                return FilterResult::Intercept(None);
                            }
                            return FilterResult::Forward;
                        }
                        let sym = handle
                            .raw_latin_sym_or_raw_current_sym()
                            .unwrap_or(handle.modified_sym());
                        // Modifiers pass through so Shift can be held for the key after the prefix.
                        if sym.is_modifier_key() {
                            return FilterResult::Forward;
                        }
                        if state.quit.is_some() {
                            state.suppressed_keys.push(code);
                            let out = state.quit_key(mods, sym, handle.modified_sym());
                            return FilterResult::Intercept(out.map(Intercepted::Quit));
                        }
                        if state.welcome.is_some() {
                            state.suppressed_keys.push(code);
                            // The prefix works there too, for what needs no project.
                            if state.prefix_armed {
                                state.prefix_armed = false;
                                let action = state.keymap.action(mods, sym).filter(over_welcome);
                                return FilterResult::Intercept(action.map(Intercepted::Action));
                            }
                            if state.keymap.is_prefix(mods, sym) {
                                state.prefix_armed = true;
                                return FilterResult::Intercept(None);
                            }
                            let out = state.welcome_key(mods, sym, handle.modified_sym());
                            return FilterResult::Intercept(out.map(Intercepted::Welcome));
                        }
                        if state.project_menu.is_some() {
                            state.suppressed_keys.push(code);
                            if state.prefix_armed {
                                state.prefix_armed = false;
                                let action =
                                    state.keymap.action(mods, sym).filter(over_project_menu);
                                return FilterResult::Intercept(action.map(Intercepted::Action));
                            }
                            if state.keymap.is_prefix(mods, sym) {
                                state.prefix_armed = true;
                                return FilterResult::Intercept(None);
                            }
                            let out = state.project_menu_key(mods, sym, handle.modified_sym());
                            return FilterResult::Intercept(out.map(Intercepted::ProjectMenu));
                        }
                        if state.switcher.is_some() {
                            state.suppressed_keys.push(code);
                            let typed = handle.modified_sym();
                            let picked = state.switcher_key(mods, sym, typed);
                            state.repeat_switcher_key(code, *mods, sym, typed);
                            return FilterResult::Intercept(
                                picked.map(|(target, place)| Intercepted::Switch(target, place)),
                            );
                        }
                        if state.terminal_menu && sym == Keysym::Escape {
                            state.close_menu();
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(None);
                        }
                        if state.prefix_armed {
                            state.prefix_armed = false;
                            // The prefix twice sends it to the app.
                            if state.keymap.is_prefix(mods, sym) {
                                return FilterResult::Forward;
                            }
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(
                                state.keymap.action(mods, sym).map(Intercepted::Action),
                            );
                        }
                        if state.keymap.is_prefix(mods, sym) {
                            state.prefix_armed = true;
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(None);
                        }
                        FilterResult::Forward
                    },
                );
                match action {
                    Some(Some(Intercepted::Action(action))) => self.run_action(action),
                    Some(Some(Intercepted::Switch(target, place))) => self.switch_to(target, place),
                    Some(Some(Intercepted::Welcome(outcome))) => self.apply_welcome(outcome),
                    Some(Some(Intercepted::ProjectMenu(outcome))) => {
                        self.apply_project_menu(outcome)
                    }
                    Some(Some(Intercepted::Quit(outcome))) => self.apply_quit(outcome),
                    _ => {}
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output_geo = self.space.output_geometry(&self.output).unwrap();
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                if let Some(drag) = self.drag.clone() {
                    self.drag_to(&drag, pos);
                }
                if let Some(reorder) = self.reorder.as_mut() {
                    let moved = pos - reorder.from;
                    reorder.moving |= moved.x.hypot(moved.y) > REORDER_START;
                    if reorder.moving {
                        let what = reorder.what;
                        self.reorder_to(what, pos);
                    }
                }
                // An overlay takes the hover from everything under it.
                let overlay =
                    self.quit.is_some() || self.welcome.is_some() || self.project_menu.is_some();
                if self.quit.is_some() {
                    self.quit_hover(pos);
                } else if self.welcome.is_some() {
                    self.welcome_hover(pos);
                } else if self.project_menu.is_some() {
                    self.project_menu_hover(pos);
                } else if self.terminal_menu {
                    let (_, items) = chrome::terminal_menu(self.sidebar_width);
                    self.menu_hover = items.iter().position(|item| item.to_f64().contains(pos));
                }
                // The header's project name lights up under the pointer, with
                // nothing open over it.
                self.header_hover = !overlay
                    && self.switcher.is_none()
                    && !self.terminal_menu
                    && self.project.is_some()
                    && chrome::sidebar_layout(&[], &[], self.sidebar_width)
                        .header
                        .to_f64()
                        .contains(pos);
                // So do the buttons of a terminal pane's bar.
                let free = !overlay
                    && self.switcher.is_none()
                    && !self.terminal_menu
                    && self.drag.is_none()
                    && self.reorder.is_none();
                self.pane_bar_hover(free.then_some(pos));
                let under = self.pointer_target(pos);
                // Over gerdr's chrome or a divider: gerdr's cursor shape, not a client's.
                if under.is_none() {
                    let icon = if overlay {
                        CursorIcon::Default
                    } else if self.reorder.is_some_and(|r| r.moving) {
                        CursorIcon::Grabbing
                    } else {
                        self.drag
                            .clone()
                            .or_else(|| self.divider_at(pos))
                            .map_or(CursorIcon::Default, |d| drag_cursor(&d))
                    };
                    if pointer.current_focus().is_some() || icon != self.chrome_cursor {
                        self.backend.window().set_cursor_visible(true);
                        self.backend.window().set_cursor(icon);
                        self.chrome_cursor = icon;
                    }
                }
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let state = event.state();
                let location = pointer.current_location();

                if state == ButtonState::Released && self.drag.take().is_some() {
                    // The final size, without Resizing, so every client applies it.
                    self.arrange();
                    // Hand the pointer back to the client under it.
                    self.refresh_pointer_focus();
                    return;
                }
                if state == ButtonState::Released
                    && let Some(reorder) = self.reorder.take()
                {
                    if reorder.moving {
                        self.reorder_herdr(reorder.what);
                    }
                    self.refresh_pointer_focus();
                    return;
                }
                // An overlay takes every release too, but one ending a press a
                // client got before the overlay opened: that client is still
                // grabbing the pointer until it sees the release.
                if state == ButtonState::Released
                    && (self.quit.is_some()
                        || self.welcome.is_some()
                        || self.project_menu.is_some())
                {
                    if pointer.is_grabbed() {
                        pointer.button(
                            self,
                            &ButtonEvent {
                                button: event.button_code(),
                                state,
                                serial,
                                time: event.time_msec(),
                            },
                        );
                        pointer.frame(self);
                        self.refresh_pointer_focus();
                    }
                    return;
                }
                if state == ButtonState::Pressed && !pointer.is_grabbed() {
                    // An overlay takes every press; only the left button does
                    // anything there.
                    if self.quit.is_some() {
                        if event.button_code() == BTN_LEFT {
                            self.quit_click(location);
                        }
                        return;
                    }
                    if self.welcome.is_some() {
                        if event.button_code() == BTN_LEFT {
                            self.welcome_click(location);
                        }
                        return;
                    }
                    if self.project_menu.is_some() {
                        if event.button_code() == BTN_LEFT {
                            self.project_menu_click(location);
                        }
                        return;
                    }
                    if self.switcher.is_some() {
                        self.switcher_click(location);
                        return;
                    }
                    if self.terminal_menu {
                        self.menu_click(location);
                        return;
                    }
                    if event.button_code() == BTN_LEFT
                        && let Some(drag) = self.divider_at(location)
                    {
                        self.drag = Some(drag);
                        return;
                    }
                    let (x, y) = (location.x as i32, location.y as i32);
                    if x < self.sidebar_width {
                        self.sidebar_click(x, y, event.button_code());
                        return;
                    }
                    if y < TAB_STRIP_HEIGHT {
                        self.tab_strip_click(x - self.sidebar_width, y, event.button_code());
                        return;
                    }
                    if let Some((id, hit)) = self.pane_bar_at(location) {
                        if event.button_code() == BTN_LEFT {
                            self.pane_bar_click(id, hit);
                        }
                        return;
                    }
                    // Click to focus a pane.
                    let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
                    if clicked.is_some() {
                        self.focus(clicked);
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button: event.button_code(),
                        state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                // Nothing scrolls behind an overlay; the project menu's list
                // moves a row per notch of the wheel, or per row's height
                // scrolled on a touchpad.
                if self.project_menu.is_some() {
                    let rows = event
                        .amount_v120(Axis::Vertical)
                        .map(|v120| v120 / 120.0)
                        .or_else(|| event.amount(Axis::Vertical).map(|px| px / MENU_SCROLL_ROW))
                        .unwrap_or(0.0);
                    if rows != 0.0 {
                        self.project_menu_scroll(rows);
                    }
                    return;
                }
                if self.quit.is_some() || self.welcome.is_some() {
                    return;
                }
                let source = event.source();
                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let v120 = event.amount_v120(axis);
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| v120.unwrap_or(0.0) * 15.0 / 120.0);
                    if amount != 0.0 {
                        frame = frame.value(axis, amount);
                        if let Some(discrete) = v120 {
                            frame = frame.v120(axis, discrete as i32);
                        }
                    }
                    if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }

    /// Any press while the terminal menu is open: an item runs, the chevron
    /// or anything else closes the menu. The press goes no further.
    fn menu_click(&mut self, location: smithay::utils::Point<f64, smithay::utils::Logical>) {
        let (_, items) = chrome::terminal_menu(self.sidebar_width);
        let item = items
            .iter()
            .position(|item| item.to_f64().contains(location));
        self.close_menu();
        if item == Some(0) {
            self.pick_workspace(WorkspaceKind::Terminal);
        }
    }

    fn close_menu(&mut self) {
        self.terminal_menu = false;
        self.menu_hover = None;
        self.refresh_pointer_focus();
    }

    /// Left click selects a workspace, "×" or middle click closes it; a left
    /// click on an agent card shows its pane. A row held with the left button
    /// can be dragged to another place. The header buttons open a
    /// project, a terminal in home, or the terminal menu; the project's name
    /// left of them opens the project menu (the welcome screen while no
    /// project is shown).
    fn sidebar_click(&mut self, x: i32, y: i32, button: u32) {
        let agents = self.agent_views();
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let layout = chrome::sidebar_layout(&workspaces, &agents, self.sidebar_width);
        let inside =
            |r: &smithay::utils::Rectangle<i32, smithay::utils::Logical>| r.contains((x, y));
        if button == BTN_LEFT {
            if inside(&layout.project) {
                self.pick_workspace(WorkspaceKind::Project);
                return;
            }
            if inside(&layout.terminal) {
                self.open_terminal(home_dir().unwrap_or_default());
                return;
            }
            if inside(&layout.chevron) {
                self.terminal_menu = true;
                self.refresh_pointer_focus();
                return;
            }
            if inside(&layout.header) {
                if self.project.is_some() {
                    self.toggle_project_menu();
                } else {
                    self.open_welcome(true);
                }
                return;
            }
        }
        if button == BTN_LEFT {
            let cards = layout
                .cards
                .iter()
                .zip(&agents)
                .flat_map(|(rects, cards)| rects.iter().zip(cards));
            if let Some((_, card)) = cards.into_iter().find(|(rect, _)| inside(rect)) {
                let pane_id = card.pane_id.clone();
                self.focus_agent(&pane_id);
                return;
            }
        }
        let Some(idx) = layout.rows.iter().position(|(row, _)| inside(row)) else {
            return;
        };
        match button {
            BTN_LEFT if inside(&layout.rows[idx].1) => self.close_workspace(idx),
            BTN_LEFT => {
                self.switch_workspace(idx);
                self.hold(Reorderable::Workspace);
            }
            BTN_MIDDLE => self.close_workspace(idx),
            _ => {}
        }
    }

    /// Left click selects a tab, opens a terminal in a new one with "+", or
    /// the run prompt for an app in a new one with the chevron after it;
    /// "×" or middle click closes a tab.
    /// A tab held with the left button can be dragged to another place.
    /// The button at the right end switches light/dark.
    fn tab_strip_click(&mut self, x: i32, y: i32, button: u32) {
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

    /// Start holding the active workspace's row or the active tab, where the pointer is.
    fn hold(&mut self, what: Reorderable) {
        let from = self.seat.get_pointer().unwrap().current_location();
        self.reorder = Some(Reorder {
            what,
            from,
            moving: false,
        });
    }

    /// Re-run pointer focus after the windows under the cursor changed.
    pub fn refresh_pointer_focus(&mut self) {
        let pointer = self.seat.get_pointer().unwrap();
        let location = pointer.current_location();
        let under = self.pointer_target(location);
        let time = self.start_time.elapsed().as_millis() as u32;
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_the_welcome_screen_the_prefix_only_acts_on_gerdr() {
        assert!(over_welcome(&Action::ToggleTheme));
        assert!(over_welcome(&Action::Quit));
        // Already open, and nothing behind it to act on.
        assert!(!over_welcome(&Action::Projects));
        assert!(!over_welcome(&Action::NewTab));
        assert!(!over_welcome(&Action::Switcher));
    }

    #[test]
    fn over_the_project_menu_the_prefix_acts_on_gerdr_and_closes_it() {
        assert!(over_project_menu(&Action::ToggleTheme));
        assert!(over_project_menu(&Action::Quit));
        // The key that opened the menu closes it.
        assert!(over_project_menu(&Action::Projects));
        assert!(!over_project_menu(&Action::LastProject));
        assert!(!over_project_menu(&Action::NewTab));
        assert!(!over_project_menu(&Action::Switcher));
    }
}
