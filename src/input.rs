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

use std::time::{Duration, Instant};

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, CursorIcon, MotionEvent},
    },
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{
    Action, Drag, Gerdr, REORDER_START, REPEAT_DELAY, REPEAT_RATE, Reorder, Reorderable,
    WorkspaceKind,
    chrome::{self, FieldSpot, TAB_STRIP_HEIGHT},
    launch::Place,
    layout::Dir,
    project_menu, quit,
    switcher::Target,
    text_field::{self, TextField},
    welcome,
};

pub(crate) const BTN_LEFT: u32 = 0x110;
/// Presses closer together than this, in time and place, count as a
/// double (or triple) click.
const MULTI_CLICK: Duration = Duration::from_millis(400);
const MULTI_CLICK_SLOP: f64 = 4.0;
pub(crate) const BTN_MIDDLE: u32 = 0x112;

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

/// An overlay with text fields, which takes pastes (see `text_field`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Welcome,
    ProjectMenu,
    /// The switcher of this serial: not one opened after it.
    Switcher(u64),
}

/// How far a touchpad scrolls the project menu's or the switcher's list by
/// a row, in pixels.
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
                            if state.key_repeat.is_some_and(|(held, _)| held == code) {
                                state.stop_key_repeat();
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
                        // A paste into an overlay's text field: the
                        // clipboard's text comes back as a key for it.
                        if !state.prefix_armed
                            && text_field::is_paste(mods, sym)
                            && let Some(overlay) = state.text_overlay()
                        {
                            state.suppressed_keys.push(code);
                            state.paste_clipboard(overlay);
                            return FilterResult::Intercept(None);
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
                            let typed = handle.modified_sym();
                            text_field::took_key();
                            let out = state.welcome_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
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
                            let typed = handle.modified_sym();
                            text_field::took_key();
                            let out = state.project_menu_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out.map(Intercepted::ProjectMenu));
                        }
                        if state.switcher.is_some() {
                            state.suppressed_keys.push(code);
                            let typed = handle.modified_sym();
                            let picked = state.switcher_key(mods, sym, typed);
                            state.repeat_key(code, *mods, sym, typed, true);
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
                if let Some(Some(intercepted)) = action {
                    self.apply_intercepted(intercepted);
                }
                self.copy_from_field();
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
                if self.field_drag {
                    self.drag_field(pos);
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
                // The sidebar lights up under the pointer, with nothing open
                // over it.
                let covered = overlay || self.switcher.is_some() || self.terminal_menu;
                self.header_hover = false;
                if !covered {
                    self.sidebar_motion(pos);
                }
                // So do the buttons of a terminal pane's bar.
                let free = !covered && self.drag.is_none() && self.reorder.is_none();
                self.pane_bar_hover(free.then_some(pos));
                let under = self.pointer_target(pos);
                // Over gerdr's chrome or a divider: gerdr's cursor shape, not a client's.
                if under.is_none() {
                    let over_field = (overlay || self.switcher.is_some())
                        && self
                            .field_spot()
                            .is_some_and(|field| field.area.to_f64().contains(pos));
                    let icon = if over_field {
                        CursorIcon::Text
                    } else if overlay {
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

                if state == ButtonState::Released {
                    self.field_drag = false;
                }
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
                            self.press_field(location);
                        }
                        return;
                    }
                    if self.project_menu.is_some() {
                        if event.button_code() == BTN_LEFT {
                            self.project_menu_click(location);
                            self.press_field(location);
                        }
                        return;
                    }
                    if self.switcher.is_some() {
                        self.switcher_click(location);
                        if event.button_code() == BTN_LEFT {
                            self.press_field(location);
                        }
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
                        self.sidebar_press(x, y, event.button_code());
                        return;
                    }
                    if y < TAB_STRIP_HEIGHT {
                        self.tab_strip_press(x - self.sidebar_width, y, event.button_code());
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
                // Nothing scrolls behind an overlay; the project menu's and
                // the switcher's lists move a row per notch of the wheel, or
                // per row's height scrolled on a touchpad.
                if self.project_menu.is_some() || self.switcher.is_some() {
                    let rows = event
                        .amount_v120(Axis::Vertical)
                        .map(|v120| v120 / 120.0)
                        .or_else(|| event.amount(Axis::Vertical).map(|px| px / MENU_SCROLL_ROW))
                        .unwrap_or(0.0);
                    if rows != 0.0 {
                        if self.project_menu.is_some() {
                            self.project_menu_scroll(rows);
                        } else {
                            self.switcher_scroll(rows);
                        }
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

    /// Start holding the active workspace's row or the active tab, where the pointer is.
    pub(crate) fn hold(&mut self, what: Reorderable) {
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

    /// Do what a key gerdr kept from the apps asked for.
    fn apply_intercepted(&mut self, intercepted: Intercepted) {
        match intercepted {
            Intercepted::Action(action) => self.run_action(action),
            Intercepted::Switch(target, place) => self.switch_to(target, place),
            Intercepted::Welcome(outcome) => self.apply_welcome(outcome),
            Intercepted::ProjectMenu(outcome) => self.apply_project_menu(outcome),
            Intercepted::Quit(outcome) => self.apply_quit(outcome),
        }
    }

    /// The overlay with text fields that has the keyboard, if any: the quit
    /// dialog, on top, has none.
    fn text_overlay(&self) -> Option<Overlay> {
        if self.quit.is_some() {
            None
        } else if self.welcome.is_some() {
            Some(Overlay::Welcome)
        } else if self.project_menu.is_some() {
            Some(Overlay::ProjectMenu)
        } else {
            self.switcher.as_ref().map(|s| Overlay::Switcher(s.serial))
        }
    }

    /// The clipboard's text read for a paste into overlay `into`, if it
    /// still has the keyboard: typed into its focused field as the key
    /// `text_field::PASTE`, through its own key handling.
    pub fn paste_text(&mut self, into: Overlay, text: &str) {
        if self.text_overlay() != Some(into) {
            return;
        }
        let mods = ModifiersState::default();
        let out = text_field::pasting(text, |key| self.overlay_key(into, &mods, key, key));
        if let Some(out) = out {
            self.apply_intercepted(out);
        }
    }

    /// A key for overlay `into`, through its own key handling.
    fn overlay_key(
        &mut self,
        into: Overlay,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Intercepted> {
        match into {
            Overlay::Welcome => self.welcome_key(mods, sym, typed).map(Intercepted::Welcome),
            Overlay::ProjectMenu => self
                .project_menu_key(mods, sym, typed)
                .map(Intercepted::ProjectMenu),
            Overlay::Switcher(_) => self
                .switcher_key(mods, sym, typed)
                .map(|(target, place)| Intercepted::Switch(target, place)),
        }
    }

    /// Where the text field with the keyboard is, in the overlay that has
    /// it, and what it shows.
    fn field_spot(&self) -> Option<FieldSpot> {
        let (size, scale) = (self.output_size(), self.scale());
        match self.text_overlay()? {
            Overlay::Welcome => chrome::welcome_field(&self.welcome_view()?, size, scale),
            Overlay::ProjectMenu => {
                chrome::project_menu_field(&self.project_menu_view()?, size, scale)
            }
            Overlay::Switcher(_) => Some(chrome::switcher_field(
                &self.switcher_view()?,
                self.content_rect(),
                scale,
            )),
        }
    }

    /// The text field with the keyboard, in the overlay that has it.
    fn field_mut(&mut self) -> Option<&mut TextField> {
        match self.text_overlay()? {
            Overlay::Welcome => self.welcome.as_mut()?.field_mut(),
            Overlay::ProjectMenu => self.project_menu.as_mut()?.field_mut(),
            Overlay::Switcher(_) => Some(self.switcher.as_mut()?.field_mut()),
        }
    }

    /// A left press at `pos` over an overlay, once the overlay took it (a
    /// click on a field focuses it): in the text field with the keyboard,
    /// it puts the caret there (Shift: the selection reaches there), and
    /// holding it drags a selection; a second press in a row selects a
    /// word, a third all of it.
    fn press_field(&mut self, pos: Point<f64, Logical>) {
        let Some(field) = self
            .field_spot()
            .filter(|field| field.area.to_f64().contains(pos))
        else {
            self.field_clicks = None;
            return;
        };
        let at = self.chrome.caret_at(&field, pos.x, self.scale());
        let now = Instant::now();
        let count = match self.field_clicks {
            Some((when, from, count))
                if now.duration_since(when) < MULTI_CLICK
                    && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                    && (from.y - pos.y).abs() < MULTI_CLICK_SLOP =>
            {
                count + 1
            }
            _ => 1,
        };
        self.field_clicks = Some((now, pos, count));
        let extend = self.seat.get_keyboard().unwrap().modifier_state().shift;
        if let Some(field) = self.field_mut() {
            field.click(at, extend, count);
        }
        self.field_drag = count == 1;
    }

    /// The pointer moved to `pos` holding a press in a text field: the
    /// selection reaches the character there.
    fn drag_field(&mut self, pos: Point<f64, Logical>) {
        let Some(field) = self.field_spot() else {
            self.field_drag = false;
            return;
        };
        let at = self.chrome.caret_at(&field, pos.x, self.scale());
        if let Some(field) = self.field_mut() {
            field.drag_to(at);
        }
    }

    /// Give the clipboard what a text field copied, if one did.
    fn copy_from_field(&mut self) {
        if let Some(text) = text_field::take_copied() {
            self.copy_text(text);
        }
    }

    /// Repeat a key held over an overlay, as long as it is held, the way
    /// the apps' keyboards do: the host sends gerdr only the press and the
    /// release. `took` is whether a text field took it. In the switcher any
    /// key but Esc and Enter repeats; over the welcome screen and the
    /// project menu, where keys act on projects (Delete forgets one), only
    /// the arrows and what a text field takes, until it takes it no more.
    pub fn repeat_key(
        &mut self,
        code: Keycode,
        mods: ModifiersState,
        sym: Keysym,
        typed: Keysym,
        took: bool,
    ) {
        self.stop_key_repeat();
        let Some(overlay) = self.text_overlay() else {
            return;
        };
        let switcher = matches!(overlay, Overlay::Switcher(_));
        let moves = matches!(
            sym,
            Keysym::Up | Keysym::Down | Keysym::KP_Up | Keysym::KP_Down
        );
        let repeats = if switcher {
            !matches!(sym, Keysym::Escape | Keysym::Return | Keysym::KP_Enter)
        } else {
            took || moves
        };
        if !repeats || text_field::is_paste(&mods, sym) {
            return;
        }
        let interval = Duration::from_millis(1000 / u64::from(REPEAT_RATE));
        let timer = Timer::from_duration(Duration::from_millis(REPEAT_DELAY.into()));
        let repeat = self.loop_handle.insert_source(timer, move |_, _, state| {
            if state.text_overlay() != Some(overlay) {
                state.key_repeat = None;
                return TimeoutAction::Drop;
            }
            text_field::took_key();
            let out = state.overlay_key(overlay, &mods, sym, typed);
            let took = text_field::took_key();
            if let Some(out) = out {
                state.apply_intercepted(out);
            }
            state.copy_from_field();
            if !(switcher || took || moves) {
                state.key_repeat = None;
                return TimeoutAction::Drop;
            }
            TimeoutAction::ToDuration(interval)
        });
        self.key_repeat = repeat.ok().map(|token| (code, token));
    }

    /// Stop repeating the key held over an overlay.
    pub fn stop_key_repeat(&mut self) {
        if let Some((_, token)) = self.key_repeat.take() {
            self.loop_handle.remove(token);
        }
    }

    /// Release every key still down, as the host window loses focus: winit
    /// drops the releases that follow, so a key held while leaving (Super
    /// of a host shortcut) would stay down for gerdr and its apps. A held
    /// Super makes Backspace Super+Backspace, which clears Claude's input,
    /// and keeps the prefix from matching.
    pub fn release_held_keys(&mut self) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let time = self.start_time.elapsed().as_millis() as u32;
        for code in keyboard.pressed_keys() {
            keyboard.input::<(), _>(
                self,
                code,
                KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                time,
                |state, _, _| {
                    // A key gerdr kept from the apps has no press to undo there.
                    if let Some(i) = state.suppressed_keys.iter().position(|k| *k == code) {
                        state.suppressed_keys.remove(i);
                        return FilterResult::Intercept(());
                    }
                    FilterResult::Forward
                },
            );
        }
        self.suppressed_keys.clear();
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
