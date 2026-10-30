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
//!
//! A right press on a region opens its context menu, which then takes
//! every press and key (the arrows walk it, Enter picks, Esc closes). A
//! double click on a workspace row or a tab renames it: the keys go to
//! the name until Enter keeps it or Esc drops it. Esc otherwise closes,
//! in this order, the rename, the menu, the tooltip and the pinned help.
//! The pointer is a hand over whatever can be clicked.

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
    Action, Drag, Gerdr, REORDER_START, REPEAT_DELAY, REPEAT_RATE, Reorder, Reorderable, checkout,
    chrome::{self, BarHit, FieldSpot, HelpHit},
    confirm, inbox,
    launch::Place,
    layout::Dir,
    menu::MenuAction,
    project_menu, quit,
    rename::RenameTarget,
    repos, restart, scrollback,
    switcher::Target,
    text_field::{self, TextField},
    toast::ToastHit,
    welcome, worktrees,
};

/// What a press or the pointer lands on in gerdr's own chrome, as far as
/// the press dispatch here cares: a workspace row or a tab (a double
/// click renames them), something else to click, or nothing.
/// The layouts of the regions under the pointer, as far as they were
/// worked out for one event (see `Gerdr::regions_at`).
#[derive(Default)]
pub struct Regions {
    pub sidebar: Option<chrome::SidebarLayout>,
    pub strip: Option<chrome::TabStripView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionHit {
    Nothing,
    /// A button, a card, a menu item: the pointer is a hand over it.
    Control,
    /// The row of workspace `w`.
    Workspace(usize),
    /// The tab `t` of the active workspace.
    Tab(usize),
}

pub(crate) const BTN_LEFT: u32 = 0x110;
pub(crate) const BTN_RIGHT: u32 = 0x111;
pub(crate) const BTN_MIDDLE: u32 = 0x112;
/// Presses closer together than this, in time and place, count as a
/// double (or triple) click.
pub(crate) const MULTI_CLICK: Duration = Duration::from_millis(400);
pub(crate) const MULTI_CLICK_SLOP: f64 = 4.0;

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
    /// The restart dialog asked for this.
    Restart(restart::Outcome),
    /// The checkout dialog asked for this.
    Checkout(checkout::Outcome),
    /// The confirm dialog asked for this.
    Confirm(confirm::Outcome),
    /// The worktrees popup asked for this.
    Worktrees(worktrees::Outcome),
    /// The repositories panel asked for this.
    Repos(repos::Outcome),
    /// The inbox panel asked for this.
    Inbox(inbox::Outcome),
    /// The scrollback viewer asked for this.
    Scrollback(scrollback::Outcome),
}

/// An overlay with text fields, which takes pastes (see `text_field`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Welcome,
    ProjectMenu,
    Worktrees,
    Repos,
    Inbox,
    /// The scrollback viewer, whose search has a field.
    Scrollback,
    /// The switcher of this serial: not one opened after it.
    Switcher(u64),
}

/// Whether `action`, picked with the prefix, works over the welcome screen:
/// only what acts on gerdr itself rather than on the project behind (the
/// welcome screen is already open).
fn over_welcome(action: &Action) -> bool {
    matches!(action, Action::ToggleTheme | Action::Quit)
}

/// Whether `action`, picked with the prefix, works over the project menu:
/// what acts on gerdr itself, and the key that opened the menu, to close it.
/// The same goes over the worktrees popup and the repositories and inbox
/// panels; the inbox's key opens it over the others, and closes it.
fn over_project_menu(action: &Action) -> bool {
    matches!(
        action,
        Action::ToggleTheme | Action::Quit | Action::Projects | Action::Inbox
    )
}

/// Whether `action`, picked with the prefix, works over the scrollback
/// viewer: as over the project menu, and the viewer's key, to close it.
fn over_scrollback(action: &Action) -> bool {
    over_project_menu(action) || *action == Action::Scrollback
}

fn drag_cursor(drag: &Drag) -> CursorIcon {
    match drag {
        Drag::Sidebar
        | Drag::ReposSplit
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
                        // A key moves what an overlay shows from under
                        // its tooltip.
                        let overlay = state.welcome.is_some()
                            || state.restart.is_some()
                            || state.checkout.is_some()
                            || state.confirm.is_some()
                            || state.project_menu.is_some()
                            || state.worktrees.is_open()
                            || state.repos.is_open()
                            || state.inbox.is_open()
                            || state.scrollback.is_some()
                            || state.switcher.is_some();
                        if overlay {
                            state.tooltip.hide();
                        }
                        if state.quit.is_some() {
                            state.suppressed_keys.push(code);
                            let out = state.quit_key(mods, sym, handle.modified_sym());
                            return FilterResult::Intercept(out.map(Intercepted::Quit));
                        }
                        if state.restart.is_some() {
                            state.suppressed_keys.push(code);
                            let out = state.restart_key(mods, sym);
                            return FilterResult::Intercept(out.map(Intercepted::Restart));
                        }
                        if state.checkout.is_some() {
                            state.suppressed_keys.push(code);
                            let out = state.checkout_key(mods, sym);
                            return FilterResult::Intercept(out.map(Intercepted::Checkout));
                        }
                        if state.confirm.is_some() {
                            state.suppressed_keys.push(code);
                            let out = state.confirm_key(mods, sym);
                            return FilterResult::Intercept(out.map(Intercepted::Confirm));
                        }
                        // A menu over the repositories panel takes its
                        // keys; any other closes it and goes on.
                        if state.repos.is_open()
                            && state.context_menu.is_some()
                            && state.menu_key(sym)
                        {
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(None);
                        }
                        // A name being typed takes every key, repeating
                        // what its field takes while held.
                        if state.rename.is_some() {
                            state.suppressed_keys.push(code);
                            let typed = handle.modified_sym();
                            text_field::took_key();
                            state.rename_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(None);
                        }
                        // A paste into an overlay's text field: the
                        // clipboard's text comes back as a key for it.
                        if !state.prefix_armed
                            && text_field::is_paste(mods, sym)
                            && let Some(overlay) = state.text_overlay().filter(|o| {
                                *o != Overlay::Scrollback
                                    || state.scrollback.as_ref().is_some_and(|s| s.find_focused())
                            })
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
                        if state.worktrees.is_open() {
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
                            let out = state.worktrees_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out.map(Intercepted::Worktrees));
                        }
                        if state.repos.is_open() {
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
                            let out = state.repos_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out.map(Intercepted::Repos));
                        }
                        if state.inbox.is_open() {
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
                            let out = state.inbox_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out.map(Intercepted::Inbox));
                        }
                        if state.scrollback.is_some() {
                            state.suppressed_keys.push(code);
                            if state.prefix_armed {
                                state.prefix_armed = false;
                                let action = state.keymap.action(mods, sym).filter(over_scrollback);
                                return FilterResult::Intercept(action.map(Intercepted::Action));
                            }
                            if state.keymap.is_prefix(mods, sym) {
                                state.prefix_armed = true;
                                return FilterResult::Intercept(None);
                            }
                            let typed = handle.modified_sym();
                            text_field::took_key();
                            let out = state.scrollback_key(mods, sym, typed);
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out.map(Intercepted::Scrollback));
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
                        // The menu's keys; any other closes it and goes on.
                        if state.context_menu.is_some() && state.menu_key(sym) {
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
                        // Esc takes down the tooltip (one still waiting
                        // to show just goes, and the key goes on), then
                        // the pinned help.
                        if sym == Keysym::Escape && state.tooltip.is_up() {
                            let shown = state.tooltip.view().is_some();
                            state.tooltip.hide();
                            if shown {
                                state.suppressed_keys.push(code);
                                return FilterResult::Intercept(None);
                            }
                        }
                        if sym == Keysym::Escape && state.help_pinned {
                            state.help_pinned = false;
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
                self.pointer_moved(pos, event.time_msec());
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let state = event.state();
                let location = pointer.current_location();

                if state == ButtonState::Released {
                    self.field_drag = false;
                    self.repos_rows_drag = false;
                    self.scrollback_release();
                    if self.thumb_drag.take().is_some() {
                        return;
                    }
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
                    // A pane let go without moving was a click on its bar.
                    if reorder.moving {
                        match reorder.what {
                            Reorderable::Pane(id) => self.drop_pane(id),
                            what => self.reorder_herdr(what),
                        }
                    }
                    self.refresh_pointer_focus();
                    return;
                }
                // An overlay takes every release too, but one ending a press a
                // client got before the overlay opened: that client is still
                // grabbing the pointer until it sees the release.
                if state == ButtonState::Released
                    && (self.quit.is_some()
                        || self.restart.is_some()
                        || self.checkout.is_some()
                        || self.confirm.is_some()
                        || self.welcome.is_some()
                        || self.project_menu.is_some()
                        || self.worktrees.is_open()
                        || self.repos.is_open()
                        || self.inbox.is_open()
                        || self.scrollback.is_some())
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
                // A left press on a scrollbar holds its thumb, flung or not.
                if state == ButtonState::Pressed
                    && !pointer.is_grabbed()
                    && event.button_code() == BTN_LEFT
                    && self.press_thumb(location)
                {
                    self.tooltip.hide();
                    self.chrome.drop_tooltip();
                    if self.rename.is_some() {
                        self.commit_rename();
                    }
                    return;
                }
                // A press while a list is flung stops it where it is, and
                // does nothing else: what is under the pointer was moving.
                if state == ButtonState::Pressed && !pointer.is_grabbed() && self.stop_flings() {
                    return;
                }
                if state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let button = event.button_code();
                    let taken = self.press(location, button);
                    self.last_press = Some((Instant::now(), location, button));
                    if taken {
                        return;
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
                // The wheel over an open context menu closes it, as a
                // click outside would, and scrolls nothing.
                if self.context_menu.is_some() {
                    self.close_menu();
                    return;
                }
                // Nothing scrolls behind an overlay; the open panel's list
                // moves instead (see `scroll`).
                let motion = self.scroll_motion::<I>(&event);
                if self.project_menu.is_some()
                    || self.switcher.is_some()
                    || self.worktrees.is_open()
                    || self.repos.is_open()
                    || self.inbox.is_open()
                    || self.scrollback.is_some()
                {
                    if let Some(motion) = motion {
                        if self.project_menu.is_some() {
                            self.project_menu_scroll(motion);
                        } else if self.worktrees.is_open() {
                            self.worktrees_scroll(motion);
                        } else if self.repos.is_open() {
                            self.repos_scroll(motion);
                        } else if self.inbox.is_open() {
                            self.inbox_scroll(motion);
                        } else if self.scrollback.is_some() {
                            self.scrollback_scroll(motion);
                        } else {
                            self.switcher_scroll(motion);
                        }
                    }
                    return;
                }
                if self.quit.is_some()
                    || self.restart.is_some()
                    || self.checkout.is_some()
                    || self.confirm.is_some()
                    || self.welcome.is_some()
                {
                    return;
                }
                // Over the sidebar, its rows scroll.
                if self.pointer.x < self.sidebar_width as f64 {
                    if let Some(motion) = motion {
                        self.sidebar_scroll(motion);
                    }
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

    /// `button` went down at `location` with no client grabbing the
    /// pointer: an overlay or a menu takes it, else the region under it
    /// (the sidebar, the tab strip, a pane bar: a right press asks the
    /// region for its menu). Whether gerdr took the press; else it goes on
    /// to the client under it.
    fn press(&mut self, location: Point<f64, Logical>, button: u32) -> bool {
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        // A press in the name being typed moves its caret; anywhere else
        // keeps the name.
        if self.rename.is_some() {
            if self.rename_press(location, button) {
                return true;
            }
            self.commit_rename();
        }
        // An overlay takes every press; only the left button does
        // anything there.
        if self.quit.is_some() {
            if button == BTN_LEFT {
                self.quit_click(location);
            }
            return true;
        }
        if self.restart.is_some() {
            if button == BTN_LEFT {
                self.restart_click(location);
            }
            return true;
        }
        if self.checkout.is_some() {
            if button == BTN_LEFT {
                self.checkout_click(location);
            }
            return true;
        }
        if self.confirm.is_some() {
            if button == BTN_LEFT {
                self.confirm_click(location);
            }
            return true;
        }
        if self.welcome.is_some() {
            if button == BTN_LEFT {
                self.welcome_click(location);
                self.press_field(location);
            }
            return true;
        }
        if self.project_menu.is_some() {
            if button == BTN_LEFT {
                self.project_menu_click(location);
                self.press_field(location);
            }
            return true;
        }
        if self.worktrees.is_open() {
            if button == BTN_LEFT {
                self.worktrees_click(location);
                self.press_field(location);
            }
            return true;
        }
        if self.repos.is_open() {
            if self.context_menu.is_some() {
                self.menu_press(location);
                return true;
            }
            match button {
                BTN_LEFT => {
                    self.repos_click(location);
                    self.press_field(location);
                }
                BTN_RIGHT => self.repos_right_click(location),
                _ => {}
            }
            return true;
        }
        if self.inbox.is_open() {
            if button == BTN_LEFT {
                self.inbox_click(location);
                self.press_field(location);
            }
            return true;
        }
        if self.scrollback.is_some() {
            if button == BTN_LEFT {
                self.scrollback_click(location);
                self.press_field(location);
            }
            return true;
        }
        if self.switcher.is_some() {
            self.switcher_click(location);
            if button == BTN_LEFT {
                self.press_field(location);
            }
            return true;
        }
        if self.context_menu.is_some() {
            self.menu_press(location);
            return true;
        }
        // The help panel: a click on a key does what the key does after
        // the prefix; it takes every other press on it too.
        if let Some(hit) = self.help_hit(location) {
            if button == BTN_LEFT
                && let Some(press) = self.keymap.help().press(hit)
            {
                self.help_press(press, location);
            }
            return true;
        }
        // The toasts: a click on one dismisses it, on its button does
        // what the button says.
        if button == BTN_LEFT
            && let Some(hit) = self.toast_press(location)
        {
            if let ToastHit::Action(action) = hit {
                self.menu_action(action);
            }
            return true;
        }
        // The button floating over a zoomed pane unzooms it.
        if let Some(id) = self.unzoom_button_at(location) {
            if button == BTN_LEFT {
                self.zoom_pane(id);
            }
            return true;
        }
        if button == BTN_LEFT
            && let Some(drag) = self.divider_at(location)
        {
            self.drag = Some(drag);
            return true;
        }
        // A second click on a workspace row or a tab renames it.
        if button == BTN_LEFT && self.is_double(location) {
            let regions = self.regions_at(location);
            let target = match self.region_hit(location, &regions) {
                RegionHit::Workspace(w) => Some(RenameTarget::Workspace(w)),
                RegionHit::Tab(t) => Some(RenameTarget::Tab(self.active_workspace, t)),
                RegionHit::Control | RegionHit::Nothing => None,
            };
            if let Some(target) = target {
                self.reorder = None;
                self.pane_drop = None;
                self.start_rename(target);
                return true;
            }
        }
        let (x, y) = (location.x as i32, location.y as i32);
        if x < self.sidebar_width {
            match button {
                BTN_RIGHT => {
                    if let Some(spec) = self.sidebar_menu(location) {
                        self.open_menu(spec);
                    }
                }
                _ => self.sidebar_press(x, y, button),
            }
            return true;
        }
        if y < self.tab_strip_height() {
            match button {
                BTN_RIGHT => {
                    if let Some(spec) = self.tab_strip_menu(location) {
                        self.open_menu(spec);
                    }
                }
                _ => self.tab_strip_press(x - self.sidebar_width, y, button),
            }
            return true;
        }
        if let Some((id, hit)) = self.pane_bar_at(location) {
            match button {
                // A second click on its label zooms the pane, or unzooms it.
                BTN_LEFT if hit == BarHit::Label && self.is_double(location) => {
                    self.zoom_pane(id);
                }
                BTN_LEFT => {
                    self.pane_bar_click(id, hit);
                    // Held by its label, the pane can be dragged elsewhere
                    // (see `pane_drag`); let go where it was, it was a click.
                    if hit == BarHit::Label {
                        self.hold(Reorderable::Pane(id));
                    }
                }
                BTN_RIGHT => {
                    if let Some(spec) = self.pane_bar_menu(location) {
                        self.open_menu(spec);
                    }
                }
                _ => {}
            }
            return true;
        }
        // The Retry and Close buttons of a placeholder whose window never
        // came, or went.
        if button == BTN_LEFT
            && let Some((pane, hit)) = self.placeholder_button_at(location)
        {
            self.menu_action(match hit {
                chrome::PlaceholderButton::Retry => MenuAction::RetryExpected(pane),
                chrome::PlaceholderButton::Close => MenuAction::CloseExpected(pane),
            });
            return true;
        }
        // Click to focus a pane.
        let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
        if clicked.is_some() {
            self.focus(clicked);
        }
        false
    }

    /// The pane whose placeholder's button is at `pos`, and which button,
    /// if one is.
    fn placeholder_button_at(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(u32, chrome::PlaceholderButton)> {
        let tab = self.tab()?;
        tab.expected_rects(self.content_rect())
            .into_iter()
            .find_map(|(expected, rect)| {
                let hit = chrome::placeholder_layout(rect, expected.timed_out).button_at(pos)?;
                Some((expected.pane, hit))
            })
    }

    /// The layout of the region under `pos` (the sidebar's or the tab
    /// strip's, whichever it is in), worked out once so a motion event's
    /// hover, tooltip and cursor shape share it.
    pub fn regions_at(&self, pos: Point<f64, Logical>) -> Regions {
        let mut regions = Regions::default();
        if pos.x < self.sidebar_width as f64 {
            regions.sidebar = Some(self.sidebar_layout());
        } else if pos.y < self.tab_strip_height() as f64 {
            regions.strip = Some(self.tab_strip_view());
        }
        regions
    }

    /// What of gerdr's chrome is at `pos`, as the press dispatch and the
    /// pointer's shape care: a workspace row, a tab, something else to
    /// click, or nothing. The regions' own layouts say where things are:
    /// `regions` holds the one under `pos` when it was worked out already.
    pub fn region_hit(&self, pos: Point<f64, Logical>, regions: &Regions) -> RegionHit {
        let output = self.output_size();
        // Over an overlay, only its own controls count.
        if self.quit.is_some() {
            let quit = self.quit_layout();
            let mut controls = quit.buttons.iter().chain(&quit.rows);
            return if controls.any(|rect| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if self.restart.is_some() {
            let controls = self.restart_controls();
            return if controls.iter().any(|rect| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if self.checkout.is_some() {
            let controls = self.checkout_controls();
            return if controls.iter().any(|rect| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if self.confirm.is_some() {
            let controls = self.confirm_controls();
            return if controls.iter().any(|rect| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if self.welcome.is_some() || self.project_menu.is_some() {
            return RegionHit::Nothing;
        }
        if let Some(view) = self.repos_view() {
            if let Some(menu) = &self.context_menu {
                return if menu.item_at(pos).is_some() {
                    RegionHit::Control
                } else {
                    RegionHit::Nothing
                };
            }
            let layout = chrome::repos_layout(&view, output);
            let mut controls = layout
                .lines
                .iter()
                .map(|(line, ..)| *line)
                .chain(layout.buttons.iter().map(|(_, r)| *r))
                .chain(layout.detail_tabs.iter().map(|(_, r)| *r))
                .chain([layout.branch])
                .chain(layout.more)
                .chain(layout.branch_menu.iter().flat_map(|(_, rows)| rows.clone()));
            return if controls.any(|r| r.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if let Some(view) = self.inbox_view() {
            let layout = chrome::inbox_layout(&view, output);
            let hit = chrome::inbox_hit(&view, &layout, pos);
            return match hit {
                None | Some(chrome::InboxHit::Panel | chrome::InboxHit::Query) => {
                    RegionHit::Nothing
                }
                Some(_) => RegionHit::Control,
            };
        }
        if let Some(view) = self.worktrees_view() {
            let layout = chrome::worktrees_layout(&view, output);
            let mut rows = layout.entries.iter();
            return if rows.any(|(rect, _)| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if let Some(switcher) = self.switcher_view() {
            let layout = chrome::switcher_geometry(&switcher, self.content_rect());
            let mut hits = layout.rows.iter().chain(&layout.chips);
            return if hits.any(|rect| rect.to_f64().contains(pos)) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        if let Some(menu) = &self.context_menu {
            return if menu.item_at(pos).is_some() {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        let inside = |rect: &chrome::Rect| rect.to_f64().contains(pos);
        // The sidebar: the header and its "+", the rows and the cards.
        if pos.x < self.sidebar_width as f64 {
            let hit = match &regions.sidebar {
                Some(layout) => layout.hit(pos),
                None => self.sidebar_layout().hit(pos),
            };
            return match hit {
                Some(chrome::SidebarHit::Row(w) | chrome::SidebarHit::Name(w)) => {
                    RegionHit::Workspace(w)
                }
                Some(_) => RegionHit::Control,
                None => RegionHit::Nothing,
            };
        }
        // The tab strip: the tabs, the "+", the chips and the theme switch.
        if pos.y < self.tab_strip_height() as f64 {
            let hit = match &regions.strip {
                Some(view) => self.tab_strip_hit_at(view, pos),
                None => self.tab_strip_hit_at(&self.tab_strip_view(), pos),
            };
            return match hit {
                Some(chrome::TabHit::Tab(t)) => RegionHit::Tab(t),
                Some(_) => RegionHit::Control,
                None => RegionHit::Nothing,
            };
        }
        // Over the apps: the toasts, the pane bars' buttons, a placeholder's,
        // the one unzooming a zoomed pane.
        let views = self.toast_views();
        let toasts = chrome::toast_layout(&views, self.toast_rect());
        if toasts
            .iter()
            .zip(&views)
            .any(|(toast, view)| !view.leaving && inside(&toast.panel))
        {
            return RegionHit::Control;
        }
        if matches!(self.pane_bar_at(pos), Some((_, BarHit::Button(_))))
            || self.placeholder_button_at(pos).is_some()
            || self.unzoom_button_at(pos).is_some()
        {
            return RegionHit::Control;
        }
        RegionHit::Nothing
    }

    /// Whether a press at `pos` now, after the last one, makes a double
    /// click: soon after it, close to it, and with the left button.
    pub fn is_double(&self, pos: Point<f64, Logical>) -> bool {
        self.last_press.is_some_and(|(when, from, button)| {
            button == BTN_LEFT
                && when.elapsed() < MULTI_CLICK
                && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                && (from.y - pos.y).abs() < MULTI_CLICK_SLOP
        })
    }

    /// Start holding the active workspace's row, the active tab or a pane,
    /// where the pointer is.
    pub(crate) fn hold(&mut self, what: Reorderable) {
        let from = self.seat.get_pointer().unwrap().current_location();
        self.reorder = Some(Reorder {
            what,
            from,
            moving: false,
        });
    }

    /// The pointer moved to `pos` (output coordinates) at `time`: drags
    /// follow it, what is under it lights up, and its cursor shape and the
    /// client it is over are found. Also run in place when what is under
    /// it changed without it moving.
    pub fn pointer_moved(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.pointer = pos;
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
        if self.repos_rows_drag {
            self.repos_drag_rows(pos);
        }
        if self.scrollback_dragging() {
            self.scrollback_drag(pos);
        }
        if self.thumb_drag.is_some() {
            self.drag_thumb(pos);
        }
        // An overlay takes the hover from everything under it.
        let overlay = self.quit.is_some()
            || self.restart.is_some()
            || self.checkout.is_some()
            || self.confirm.is_some()
            || self.welcome.is_some()
            || self.project_menu.is_some()
            || self.worktrees.is_open()
            || self.repos.is_open()
            || self.inbox.is_open()
            || self.scrollback.is_some();
        // An overlay's icons have tooltips of their own.
        let mut overlay_tip = None;
        if self.quit.is_some() {
            self.quit_hover(pos);
        } else if self.restart.is_some() {
            self.restart_hover(pos);
        } else if self.checkout.is_some() {
            self.checkout_hover(pos);
        } else if self.confirm.is_some() {
            self.confirm_hover(pos);
        } else if self.welcome.is_some() {
            overlay_tip = self.welcome_hover(pos);
        } else if self.project_menu.is_some() {
            overlay_tip = self.project_menu_hover(pos);
        } else if self.worktrees.is_open() {
            overlay_tip = self.worktrees_hover(pos);
        } else if self.context_menu.is_some() {
            self.menu_motion(pos);
        } else if self.repos.is_open() {
            overlay_tip = self.repos_hover(pos);
        } else if self.inbox.is_open() {
            overlay_tip = self.inbox_hover(pos);
        } else if self.scrollback.is_some() {
            overlay_tip = self.scrollback_hover(pos);
        } else if self.switcher.is_some() {
            overlay_tip = self.switcher_motion(pos);
        }
        // The help panel, behind all those.
        let popup = overlay || self.switcher.is_some() || self.context_menu.is_some();
        self.help_hover = if popup { None } else { self.help_hit(pos) };
        // The region under the pointer lights up, with nothing open
        // over it, and may offer a tooltip. Its layout is worked
        // out once for the event.
        let covered = popup || self.help_hover.is_some();
        self.header_hover = false;
        let regions = self.regions_at(pos);
        // The strip of agents closes off the sidebar (see
        // `track_agent_strip`).
        if covered || regions.sidebar.is_none() {
            self.agent_strip = None;
        }
        let tip = if covered {
            overlay_tip
        } else if let Some(layout) = &regions.sidebar {
            self.sidebar_motion(layout, pos)
        } else if let Some(view) = &regions.strip {
            self.tab_strip_motion(view, pos)
        } else {
            self.pane_bar_motion(pos)
        };
        // So do the buttons of a terminal pane's bar.
        let free = !covered && self.drag.is_none() && self.reorder.is_none();
        self.pane_bar_hover(free.then_some(pos));
        let offered = free || covered;
        self.offer_tooltip(tip.filter(|_| offered), Instant::now());
        let under = self.pointer_target(pos);
        // Over gerdr's chrome or a divider: gerdr's cursor shape, not a client's.
        if under.is_none() {
            let over_field = (overlay || self.switcher.is_some())
                && self
                    .field_spot()
                    .is_some_and(|field| field.area.to_f64().contains(pos))
                || self.on_scrollback_text(pos);
            let icon = if over_field {
                CursorIcon::Text
            } else if self.reorder.is_some_and(|r| r.moving) {
                CursorIcon::Grabbing
            } else if let Some(drag) = self.drag.clone().or_else(|| self.divider_at(pos)) {
                drag_cursor(&drag)
            } else if self.region_hit(pos, &regions) != RegionHit::Nothing
                || self.help_hover.is_some_and(|hit| hit != HelpHit::Panel)
            {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
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
                time,
            },
        );
        pointer.frame(self);
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
            Intercepted::Restart(outcome) => self.apply_restart(outcome),
            Intercepted::Checkout(outcome) => self.apply_checkout(outcome),
            Intercepted::Confirm(outcome) => self.apply_confirm(outcome),
            Intercepted::Worktrees(outcome) => self.apply_worktrees(outcome),
            Intercepted::Repos(outcome) => self.apply_repos(outcome),
            Intercepted::Inbox(outcome) => self.apply_inbox(outcome),
            Intercepted::Scrollback(outcome) => self.apply_scrollback(outcome),
        }
    }

    /// The overlay with text fields that has the keyboard, if any: the quit
    /// dialog, on top, has none.
    fn text_overlay(&self) -> Option<Overlay> {
        if self.quit.is_some()
            || self.restart.is_some()
            || self.checkout.is_some()
            || self.confirm.is_some()
        {
            None
        } else if self.welcome.is_some() {
            Some(Overlay::Welcome)
        } else if self.project_menu.is_some() {
            Some(Overlay::ProjectMenu)
        } else if self.worktrees.is_open() {
            Some(Overlay::Worktrees)
        } else if self.repos.is_open() {
            Some(Overlay::Repos)
        } else if self.inbox.is_open() {
            Some(Overlay::Inbox)
        } else if self.scrollback.is_some() {
            Some(Overlay::Scrollback)
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
            Overlay::Worktrees => self
                .worktrees_key(mods, sym, typed)
                .map(Intercepted::Worktrees),
            Overlay::Repos => self.repos_key(mods, sym, typed).map(Intercepted::Repos),
            Overlay::Inbox => self.inbox_key(mods, sym, typed).map(Intercepted::Inbox),
            Overlay::Scrollback => self
                .scrollback_key(mods, sym, typed)
                .map(Intercepted::Scrollback),
            Overlay::Switcher(_) => self
                .switcher_key(mods, sym, typed)
                .map(|(target, place)| Intercepted::Switch(target, place)),
        }
    }

    /// A click at `pos` on a key hint of overlay `into` showing `keys`:
    /// that key pressed, through the overlay's own key handling. What the
    /// pointer is over is found again, as the overlay may have changed
    /// under it.
    pub fn press_hint(&mut self, into: Overlay, keys: &str, pos: Point<f64, Logical>) {
        let Some((mods, sym)) = crate::keys::hint_key(keys) else {
            return;
        };
        if let Some(out) = self.overlay_key(into, &mods, sym, sym) {
            self.apply_intercepted(out);
        }
        self.copy_from_field();
        let _ = match self.text_overlay() {
            Some(Overlay::Welcome) => self.welcome_hover(pos),
            Some(Overlay::ProjectMenu) => self.project_menu_hover(pos),
            Some(Overlay::Worktrees) => self.worktrees_hover(pos),
            Some(Overlay::Repos) => self.repos_hover(pos),
            Some(Overlay::Inbox) => self.inbox_hover(pos),
            Some(Overlay::Scrollback) => self.scrollback_hover(pos),
            Some(Overlay::Switcher(_)) => self.switcher_motion(pos),
            None => None,
        };
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
            Overlay::Worktrees => Some(chrome::worktrees_field(
                &self.worktrees_view()?,
                size,
                scale,
            )),
            Overlay::Repos => Some(chrome::repos_field(&self.repos_view()?, size, scale)),
            Overlay::Inbox => Some(chrome::inbox_field(&self.inbox_view()?, size, scale)),
            Overlay::Scrollback => chrome::scrollback_field(&self.scrollback_view()?, scale),
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
            Overlay::Worktrees => self.worktrees.field_mut(),
            Overlay::Repos => self.repos.field_mut(),
            Overlay::Inbox => self.inbox.field_mut(),
            Overlay::Scrollback => self.scrollback.as_mut()?.field_mut(),
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
        if self.rename.is_some() {
            self.repeat_rename_key(code, mods, sym, typed, took);
            return;
        }
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

    /// Repeat `code`, held while a name is typed, for as long as the
    /// name's field takes it (`took`: it did once) and the rename runs.
    fn repeat_rename_key(
        &mut self,
        code: Keycode,
        mods: ModifiersState,
        sym: Keysym,
        typed: Keysym,
        took: bool,
    ) {
        if !took || text_field::is_paste(&mods, sym) {
            return;
        }
        let interval = Duration::from_millis(1000 / u64::from(REPEAT_RATE));
        let timer = Timer::from_duration(Duration::from_millis(REPEAT_DELAY.into()));
        let repeat = self.loop_handle.insert_source(timer, move |_, _, state| {
            if state.rename.is_none() {
                state.key_repeat = None;
                return TimeoutAction::Drop;
            }
            text_field::took_key();
            state.rename_key(&mods, sym, typed);
            if !text_field::took_key() {
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
        // The inbox opens over it, and its key closes the inbox again.
        assert!(over_project_menu(&Action::Inbox));
        assert!(!over_project_menu(&Action::LastProject));
        assert!(!over_project_menu(&Action::NewTab));
        assert!(!over_project_menu(&Action::Switcher));
    }
}
