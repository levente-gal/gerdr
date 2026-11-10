//! Forward host-window input to the focused client, except gerdr's own
//! shortcuts, clicks on the sidebar, the tab strip and the pane bars, drags of the dividers
//! (the sidebar edge and the gap between split panes), and drags of the
//! workspace rows and tabs to reorder them.
//!
//! Shortcuts follow herdr: press the prefix (Ctrl+B), release it, then one key
//! (see `keys` for the defaults, and how to change them). The prefix twice
//! sends it to the app; Esc cancels, and a key bound to nothing says so.
//! While the prefix is armed, a help panel lists the keys.
//!
//! The popups, panels and dialogs stack in one order (see `overlay`): the
//! one on top at the pointer takes the press, the hover and the cursor
//! shape, and while a dialog, the welcome screen or a panel is open, every
//! key, click and scroll goes to it. Over the welcome screen the prefix
//! still switches light/dark and quits; over a panel it also opens the
//! other panels and the help, and the key that opened the panel closes it.
//!
//! The sidebar header's project name opens the project menu, or the welcome
//! screen while no project is shown. A click on a terminal pane's bar
//! focuses the pane, and its buttons restart or stop what runs in it.
//!
//! A right press on a region opens its context menu, which then takes
//! every press and key (the arrows walk it, Enter picks, Esc closes; the
//! prefix closes it and goes on to be armed, any other key closes it and
//! goes nowhere). A double click on a workspace
//! row or a tab renames it: the keys go to the name until Enter keeps it
//! or Esc drops it. Esc otherwise takes down the tooltip and the pinned
//! help and still reaches the app; a press off the pinned help closes it
//! too. The pointer is a hand over whatever can be clicked, and an open
//! hand over what can be dragged.

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, CursorIcon, MotionEvent},
    },
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{
    Action, Drag, Gerdr, REORDER_START, REPEAT_DELAY, REPEAT_RATE, Reorder, Reorderable, checkout,
    chrome::{self, BarHit, FieldSpot, HelpHit},
    confirm, inbox,
    launch::Place,
    layout::Dir,
    menu::MenuAction,
    overlay, project_menu, quit,
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
/// The mouse's back and forward (side) buttons.
pub(crate) const BTN_SIDE: u32 = 0x113;
pub(crate) const BTN_EXTRA: u32 = 0x114;
/// Presses closer together than this, in time and place, count as a
/// double (or triple) click.
pub(crate) const MULTI_CLICK: Duration = Duration::from_millis(400);
pub(crate) const MULTI_CLICK_SLOP: f64 = 4.0;

thread_local! {
    /// Whether Ctrl, held down as the prefix was armed, may be held from
    /// it still: it is, until Ctrl goes down again (see `after_prefix`).
    static CTRL_FROM_PREFIX: Cell<bool> = const { Cell::new(false) };
}

/// The prefix was armed with `mods` held: Ctrl among them may stay held
/// for the key after it.
fn arm_prefix(mods: &ModifiersState) {
    CTRL_FROM_PREFIX.set(mods.ctrl);
}

/// The action the key `sym` pressed after the prefix with `mods` held
/// picks in `keymap`, and the modifiers it is taken with. Ctrl that may
/// be held from the prefix (`carried`) picks the key alone where that
/// has an action, and Ctrl with the key only where it alone has none;
/// Ctrl pressed again after the prefix picks Ctrl with the key first.
fn after_prefix(
    keymap: &crate::keys::Keymap,
    mods: &ModifiersState,
    sym: Keysym,
    carried: bool,
) -> (ModifiersState, Option<Action>) {
    if carried && mods.ctrl {
        let alone = ModifiersState {
            ctrl: false,
            ..*mods
        };
        if let Some(action) = keymap.action(&alone, sym) {
            return (alone, Some(action));
        }
        return (alone, keymap.action(mods, sym));
    }
    (*mods, keymap.action(mods, sym))
}

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
    /// The stats panel on the AI work asked for this.
    AiStats(crate::stats_panel::Outcome),
    /// The switcher's key, pressed after the prefix over it, closes it.
    CloseSwitcher,
    /// The tasks panel asked for this.
    Tasks(crate::tasks::Outcome),
    /// The machines panel asked for this.
    Machines(crate::machines_panel::Outcome),
    /// The new feature dialog asked for this.
    NewFeature(crate::features::DialogOutcome),
    FindingDialog(crate::finding_dialog::FindingDialogOutcome),
    /// The name and email dialog asked for this.
    Identity(crate::git_identity::IdentityOutcome),
    /// The dialog connecting a machine asked for this.
    Machine(crate::machines::MachineOutcome),
    /// The project commands dialog asked for this.
    Commands(crate::project_commands::CommandsOutcome),
    /// The feature panel asked for this.
    Features(crate::features::PanelOutcome),
}

/// An overlay with text fields, which takes pastes (see `text_field`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Welcome,
    ProjectMenu,
    Worktrees,
    Repos,
    Inbox,
    /// The stats panel on the AI work, with its search.
    AiStats,
    /// The scrollback viewer, whose search has a field.
    Scrollback,
    /// The switcher of this serial: not one opened after it.
    Switcher(u64),
    /// The background tasks, which has no field: only its key hints come
    /// this way.
    Tasks,
    /// The machines panel: no field yet, only its key hints.
    MachinesPanel,
    /// The new feature dialog, with its title field.
    NewFeature,
    FindingDialog,
    /// The name and email dialog, with its two fields.
    Identity,
    /// The dialog connecting a machine, with its two fields.
    Machine,
    /// The project commands dialog, with a field per repository.
    Commands,
    /// The feature tab: its key hints come this way, and what is typed into
    /// a criterion's note while one is written.
    Features,
    /// The criteria strip over a sandbox's app, once given the keys: what is
    /// typed into the note being written there.
    Criteria,
}

/// Whether `action`, picked with the prefix, works over the welcome screen:
/// only what acts on gerdr itself rather than on the project behind (the
/// welcome screen is already open).
fn over_welcome(action: &Action) -> bool {
    matches!(action, Action::ToggleTheme | Action::Quit)
}

/// Whether `action`, picked with the prefix, works over a panel: what acts
/// on gerdr itself, the help, and the keys of the panels that open over
/// the others (or in their place). The key that opened a panel closes it
/// (see `Overlay::toggled_by`).
fn over_panel(action: &Action) -> bool {
    matches!(
        action,
        Action::ToggleTheme
            | Action::Quit
            | Action::Shortcuts
            | Action::Projects
            | Action::Inbox
            | Action::AiStats
            | Action::Tasks
            | Action::Machines
            | Action::Features
            | Action::Repos
            | Action::Worktrees
    )
}

/// Whether `action`, picked with the prefix, works over open `over`.
pub(crate) fn works_over(over: overlay::Overlay, action: &Action) -> bool {
    match over {
        overlay::Overlay::Welcome => over_welcome(action),
        // A dialog asks first: nothing else goes on over it.
        o if o.is_dialog() => false,
        // The feature panel and a file's tab are tabs: every key works there,
        // as over the app the criteria strip is on.
        overlay::Overlay::Features | overlay::Overlay::Doc | overlay::Overlay::Criteria => true,
        _ => over_panel(action),
    }
}

/// Whether `action` opens one of the panels.
fn opens_panel(action: &Action) -> bool {
    matches!(
        action,
        Action::Projects
            | Action::Inbox
            | Action::AiStats
            | Action::Repos
            | Action::Worktrees
            | Action::Switcher
            | Action::Scrollback
            | Action::Features
            | Action::Machines
    )
}

/// What closes panel `over`, as its own Esc would.
fn closing(over: overlay::Overlay) -> Option<Intercepted> {
    use crate::overlay::Overlay as O;
    Some(match over {
        O::ProjectMenu => Intercepted::ProjectMenu(project_menu::Outcome::Closed),
        O::Worktrees => Intercepted::Worktrees(worktrees::Outcome::Close),
        O::Repos => Intercepted::Repos(repos::Outcome::Close),
        O::Inbox => Intercepted::Inbox(inbox::Outcome::Close),
        O::AiStats => Intercepted::AiStats(crate::stats_panel::Outcome::Close),
        O::Scrollback => Intercepted::Scrollback(scrollback::Outcome::Close),
        O::Switcher => Intercepted::CloseSwitcher,
        O::Tasks => Intercepted::Tasks(crate::tasks::Outcome::Close),
        O::Machines => Intercepted::Machines(crate::machines_panel::Outcome::Close),
        _ => return None,
    })
}

/// How a notice names the key pressed with `mods` held, like "Shift+Q"
/// (the help panel's way).
fn key_name(mods: &ModifiersState, sym: Keysym) -> String {
    crate::keys::chord_label(mods, sym)
}

fn drag_cursor(drag: &Drag) -> CursorIcon {
    match drag {
        Drag::Sidebar
        | Drag::ReposSplit
        | Drag::DocSplit
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
                // A key may move a field's caret: its view follows.
                self.field_scroll_due = true;
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
                        // Ctrl pressed again after the prefix is meant for the key after it.
                        if sym.is_modifier_key() {
                            if matches!(sym, Keysym::Control_L | Keysym::Control_R) {
                                CTRL_FROM_PREFIX.set(false);
                            }
                            return FilterResult::Forward;
                        }
                        let top = state.key_overlay();
                        // A key moves what an overlay shows from under
                        // its tooltip.
                        if state.grabbing_overlay().is_some() {
                            state.tooltip.hide();
                            state.drop_agent_strip();
                        }
                        // A dialog takes every key.
                        if let Some(dialog) = top.filter(|o| o.is_dialog()) {
                            state.suppressed_keys.push(code);
                            // The dialogs with fields take pastes, and
                            // repeat what they take while held.
                            let fielded = match dialog {
                                overlay::Overlay::NewFeature => Some(Overlay::NewFeature),
                                overlay::Overlay::FindingDialog => Some(Overlay::FindingDialog),
                                overlay::Overlay::GitIdentity => Some(Overlay::Identity),
                                overlay::Overlay::ConnectMachine => Some(Overlay::Machine),
                                overlay::Overlay::ProjectCommands => Some(Overlay::Commands),
                                _ => None,
                            };
                            if let Some(into) = fielded {
                                state.prefix_armed = false;
                                if text_field::is_paste(mods, sym) {
                                    state.paste_clipboard(into);
                                    return FilterResult::Intercept(None);
                                }
                                let typed = handle.modified_sym();
                                text_field::took_key();
                                let out = state.overlay_key(into, mods, sym, typed);
                                let took = text_field::took_key();
                                state.repeat_key(code, *mods, sym, typed, took);
                                return FilterResult::Intercept(out);
                            }
                            let out = state.dialog_key(dialog, mods, sym, handle.modified_sym());
                            return FilterResult::Intercept(out);
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
                        // The menu takes every key (see `menu_key`): what
                        // isn't one of its own closes it and goes nowhere
                        // else, neither to a panel under it nor to an app.
                        // Only the prefix, or the key after it, closes it and
                        // goes on, to be armed or to pick an action.
                        if top == Some(overlay::Overlay::ContextMenu) {
                            if state.prefix_armed || state.keymap.is_prefix(mods, sym) {
                                state.close_menu();
                            } else {
                                state.menu_key(sym);
                                state.suppressed_keys.push(code);
                                return FilterResult::Intercept(None);
                            }
                        }
                        // A paste into an overlay's text field: the
                        // clipboard's text comes back as a key for it.
                        if !state.prefix_armed
                            && text_field::is_paste(mods, sym)
                            && !state.doc_cycles(mods, sym)
                            && let Some(overlay) = state.text_overlay().filter(|o| {
                                *o != Overlay::Scrollback
                                    || state.scrollback.as_ref().is_some_and(|s| s.find_focused())
                            })
                        {
                            state.suppressed_keys.push(code);
                            state.paste_clipboard(overlay);
                            return FilterResult::Intercept(None);
                        }
                        // The welcome screen and the panels take every key,
                        // repeating what their fields take while held. The
                        // prefix works there too, for what acts on gerdr
                        // itself rather than on what is behind (see
                        // `works_over`), and the key that opened a panel
                        // closes it.
                        if let Some(top) = state.key_overlay() {
                            state.suppressed_keys.push(code);
                            if state.prefix_armed {
                                state.prefix_armed = false;
                                let out = state.prefix_key(Some(top), mods, sym);
                                return FilterResult::Intercept(out);
                            }
                            if state.keymap.is_prefix(mods, sym) {
                                state.prefix_armed = true;
                                arm_prefix(mods);
                                return FilterResult::Intercept(None);
                            }
                            let typed = handle.modified_sym();
                            text_field::took_key();
                            let out = state
                                .text_overlay()
                                .and_then(|into| state.overlay_key(into, mods, sym, typed));
                            let took = text_field::took_key();
                            state.repeat_key(code, *mods, sym, typed, took);
                            return FilterResult::Intercept(out);
                        }
                        if state.prefix_armed {
                            state.prefix_armed = false;
                            // The prefix twice sends it to the app.
                            if state.keymap.is_prefix(mods, sym) {
                                return FilterResult::Forward;
                            }
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(state.prefix_key(None, mods, sym));
                        }
                        if state.keymap.is_prefix(mods, sym) {
                            state.prefix_armed = true;
                            arm_prefix(mods);
                            state.suppressed_keys.push(code);
                            return FilterResult::Intercept(None);
                        }
                        // A file's tab showing its page takes what acts on
                        // the file: the page has all else.
                        if state.review_tab().is_some()
                            && crate::doc_tab::doc_preview_key(mods, sym)
                        {
                            state.suppressed_keys.push(code);
                            let typed = handle.modified_sym();
                            let out = state.overlay_key(Overlay::Repos, mods, sym, typed);
                            return FilterResult::Intercept(out);
                        }
                        // Esc takes down the tooltip and the pinned help,
                        // and still goes on to the app: it may be what
                        // leaves a mode there.
                        if sym == Keysym::Escape {
                            state.tooltip.hide();
                            state.drop_agent_strip();
                            state.help_pinned = false;
                        }
                        // What it interrupts stopped on purpose.
                        if mods.ctrl && matches!(sym, Keysym::c | Keysym::C | Keysym::backslash) {
                            state.note_interrupt();
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
                    self.repos_editor_drag = false;
                    self.scrollback_release();
                    self.release_page_text();
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
                if state == ButtonState::Released && self.grabbing_overlay().is_some() {
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
                    self.press_count = self.count_press(location, button);
                    self.field_scroll_due = true;
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
                let motion = self.scroll_motion::<I>(&event);
                let at = self.overlay_at(self.pointer);
                // The wheel over an open context menu leaves it be; off
                // it, the wheel closes it, as a press outside would, and
                // scrolls nothing.
                if at == Some(overlay::Overlay::ContextMenu) {
                    let over_menu = self
                        .context_menu
                        .as_ref()
                        .is_some_and(|menu| menu.layout().0.to_f64().contains(self.pointer));
                    if !over_menu {
                        self.close_menu();
                    }
                    return;
                }
                // Nothing scrolls behind an overlay; what the one on top
                // lists moves instead (see `overlay_scroll`), if it lists
                // anything.
                // A file's preview beside its review scrolls itself.
                if let Some(top) = self.grabbing_overlay() {
                    if let Some(motion) = motion {
                        self.overlay_scroll(top, motion);
                    }
                    return;
                }
                // Over the feature panel's tab, its repositories move;
                // over a file's tab, its review.
                if let Some(tab @ (overlay::Overlay::Features | overlay::Overlay::Doc)) = at {
                    if let Some(motion) = motion {
                        self.overlay_scroll(tab, motion);
                    }
                    return;
                }
                // Over a toast, the help panel or the footer, nothing
                // moves.
                if at.is_some() || self.on_footer(self.pointer) {
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
        // A press stops the wait for the strip of agents, as for the
        // tooltip: a click on the AI button opens the agent picker, and
        // the strip must not open beside it.
        self.agent_strip_due = None;
        // A press in the name being typed moves its caret; anywhere else
        // keeps the name.
        if self.rename.is_some() {
            if self.rename_press(location, button) {
                return true;
            }
            self.commit_rename();
        }
        let hit = self.overlay_at(location);
        // A press off the pinned help, on what is under it, closes it and
        // goes on.
        if self.help_pinned && hit.is_none_or(|o| overlay::Overlay::Help.is_above(o)) {
            self.help_pinned = false;
        }
        if let Some(hit) = hit {
            self.overlay_press(hit, location, button);
            return true;
        }
        // A press on the app or gerdr's chrome takes the keys from the
        // criteria strip: they go where the press went.
        self.drop_criteria_keys();
        // The mouse's back and forward buttons over gerdr's own parts go
        // back and forward through the tabs shown; a window under the
        // pointer (a browser, Zed) gets them itself.
        let (x, y) = (location.x as i32, location.y as i32);
        if matches!(button, BTN_SIDE | BTN_EXTRA)
            && (x < self.sidebar_width
                || y < self.tab_strip_height()
                || self.on_footer(location)
                || self.pane_bar_at(location).is_some())
        {
            if button == BTN_SIDE {
                self.go_back();
            } else {
                self.go_forward();
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
        // The footer: a task opens the tasks panel.
        if self.on_footer(location) {
            if button == BTN_LEFT {
                self.footer_press(location);
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
        if x < self.sidebar_width {
            match button {
                BTN_RIGHT => {
                    if let Some(spec) = self.sidebar_menu(location) {
                        self.open_menu(spec);
                    }
                }
                _ => self.sidebar_press(x, y, button),
            }
            // What the press opened over the sidebar closes its strip.
            if self.sidebar_covered() {
                self.drop_agent_strip();
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

    /// `button` went down at `pos` on open `overlay`, the one on top
    /// there, which takes it. Only the left button does anything there,
    /// but on the repositories panel, where the right one opens a menu.
    fn overlay_press(&mut self, overlay: overlay::Overlay, pos: Point<f64, Logical>, button: u32) {
        use crate::overlay::Overlay as O;
        match (overlay, button) {
            // A press anywhere while the menu is open goes to it: off it,
            // it closes the menu.
            (O::ContextMenu, _) => return self.menu_press(pos),
            (O::Repos | O::Doc, BTN_RIGHT) => return self.repos_right_click(pos),
            (_, BTN_LEFT) => {}
            _ => return,
        }
        match overlay {
            O::Quit => _ = self.quit_click(pos),
            O::Restart => _ = self.restart_click(pos),
            O::Checkout => _ = self.checkout_click(pos),
            O::Confirm => _ = self.confirm_click(pos),
            O::Tasks => self.tasks_click(pos),
            O::Machines => self.machines_panel_click(pos),
            O::NewFeature => self.new_feature_click(pos),
            O::FindingDialog => self.finding_dialog_click(pos),
            O::GitIdentity => self.identity_click(pos),
            O::ConnectMachine => self.machine_click(pos),
            O::ProjectCommands => self.commands_click(pos),
            O::Features => self.feature_panel_click(pos),
            O::Criteria => self.criteria_strip_click(pos),
            // A click on a toast dismisses it, on its button does what the
            // button says.
            O::Toasts => {
                if let Some(ToastHit::Action(action)) = self.toast_press(pos) {
                    self.menu_action(action);
                }
            }
            // A click on a key of the help does what the key does after
            // the prefix.
            O::Help => {
                let press = self
                    .help_hit(pos)
                    .and_then(|hit| self.keymap.help().press(hit));
                if let Some(press) = press {
                    self.help_press(press, pos);
                }
            }
            O::Welcome => _ = self.welcome_click(pos),
            O::ProjectMenu => self.project_menu_click(pos),
            O::Worktrees => self.worktrees_click(pos),
            O::Repos | O::Doc => self.repos_click(pos),
            O::Inbox => self.inbox_click(pos),
            O::AiStats => self.ai_stats_click(pos),
            O::Scrollback => self.scrollback_click(pos),
            O::Switcher => self.switcher_click(pos),
            O::Tooltip | O::ContextMenu => {}
        }
        // On the feature tab's page, a press on text selects from there.
        if overlay == O::Features {
            self.press_page_text(pos);
        }
        // A click on a field focuses it, and puts the caret there.
        let takes_field = matches!(overlay, O::Features | O::Criteria | O::Doc)
            || overlay.grabs()
                && (!overlay.is_dialog()
                    || matches!(
                        overlay,
                        O::NewFeature | O::GitIdentity | O::ConnectMachine | O::ProjectCommands
                    ));
        if takes_field {
            self.press_field(pos);
        }
    }

    /// Whether a control of open `overlay` is at `pos`: the pointer is a
    /// hand over it.
    fn overlay_control_at(&self, overlay: overlay::Overlay, pos: Point<f64, Logical>) -> bool {
        use crate::overlay::Overlay as O;
        let output = self.output_size();
        let on = |rect: &chrome::Rect| rect.to_f64().contains(pos);
        match overlay {
            O::Quit => {
                let quit = self.quit_layout();
                quit.buttons.iter().chain(&quit.rows).any(on)
            }
            O::Restart => self.restart_controls().iter().any(on),
            O::Checkout => self.checkout_controls().iter().any(on),
            O::Confirm => self.confirm_controls().iter().any(on),
            O::Tasks => self.tasks_controls().iter().any(on),
            O::Machines => self.machines_panel_controls().iter().any(on),
            O::NewFeature => self.new_feature_controls().iter().any(on),
            O::FindingDialog => self.finding_dialog_controls().iter().any(on),
            O::GitIdentity => self.identity_controls().iter().any(on),
            O::ConnectMachine => self.machine_controls().iter().any(on),
            O::ProjectCommands => self.commands_controls().iter().any(on),
            O::Features => self.feature_tab_control_at(pos),
            O::Criteria => !matches!(
                self.criteria_strip_hit(pos),
                None | Some(chrome::StripHit::Panel)
            ),
            O::ContextMenu => self
                .context_menu
                .as_ref()
                .is_some_and(|menu| menu.item_at(pos).is_some()),
            O::Toasts => true,
            O::Repos | O::Doc => self.repos_view().is_some_and(|view| {
                let layout = chrome::repos_layout(&view, self.repos_rect());
                let mut controls = layout
                    .lines
                    .iter()
                    .map(|(line, ..)| *line)
                    .chain(layout.buttons.iter().map(|(_, r)| *r))
                    .chain(layout.detail_tabs.iter().map(|(_, r)| *r))
                    .chain(layout.modes.iter().flat_map(|(_, parts)| parts.clone()))
                    .chain([layout.branch])
                    .chain(layout.more)
                    .chain(layout.branch_menu.iter().flat_map(|(_, rows)| rows.clone()));
                controls.any(|r| on(&r))
            }),
            O::Inbox => self.inbox_view().is_some_and(|view| {
                let layout = chrome::inbox_layout(&view, output);
                let hit = chrome::inbox_hit(&view, &layout, pos);
                !matches!(
                    hit,
                    None | Some(chrome::InboxHit::Panel | chrome::InboxHit::Query)
                )
            }),
            O::AiStats => self.ai_stats_control_at(pos),
            O::Worktrees => self.worktrees_view().is_some_and(|view| {
                let layout = chrome::worktrees_layout(&view, output);
                layout.entries.iter().any(|(rect, _)| on(rect))
            }),
            O::Switcher => self.switcher_view().is_some_and(|switcher| {
                let layout = chrome::switcher_geometry(&switcher, self.content_rect());
                layout.rows.iter().chain(&layout.chips).any(on)
            }),
            // The help's keys light up by its own hover (see `help_hover`).
            O::Tooltip | O::Welcome | O::Help | O::ProjectMenu | O::Scrollback => false,
        }
    }

    /// The client surface the pointer at `pos` is over, if one has it:
    /// none while an overlay takes the pointer there (see
    /// `Gerdr::overlay_at`), nor over gerdr's own chrome. Off an open
    /// context menu the client keeps it, though a press there closes the
    /// menu.
    fn pointer_client(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        match self.overlay_at(pos) {
            Some(overlay::Overlay::ContextMenu) | None => self.pointer_target(pos),
            Some(_) => None,
        }
    }

    /// A key pressed while `dialog` is open, through its own key handling.
    fn dialog_key(
        &mut self,
        dialog: overlay::Overlay,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Intercepted> {
        use crate::overlay::Overlay as O;
        match dialog {
            O::Quit => self.quit_key(mods, sym, typed).map(Intercepted::Quit),
            O::Restart => self.restart_key(mods, sym).map(Intercepted::Restart),
            O::Checkout => self.checkout_key(mods, sym).map(Intercepted::Checkout),
            O::Confirm => self.confirm_key(mods, sym).map(Intercepted::Confirm),
            _ => None,
        }
    }

    /// The key pressed after the prefix, over open `over` (or over the
    /// apps): what it asks for, if it works there (see `after_prefix`).
    /// Esc, and the prefix again over an overlay, cancel; the key that
    /// opened `over` closes it; any other key says why it does nothing.
    fn prefix_key(
        &mut self,
        over: Option<overlay::Overlay>,
        mods: &ModifiersState,
        sym: Keysym,
    ) -> Option<Intercepted> {
        let carried = CTRL_FROM_PREFIX.replace(false);
        if sym == Keysym::Escape || self.keymap.is_prefix(mods, sym) {
            return None;
        }
        let (mods, action) = after_prefix(&self.keymap, mods, sym, carried);
        let keys = format!("{} {}", self.keymap.help().prefix, key_name(&mods, sym));
        let Some(action) = action else {
            self.show_warning(format!("No shortcut on {keys}"));
            return None;
        };
        let Some(over) = over else {
            return Some(Intercepted::Action(action));
        };
        if over.toggled_by() == Some(action)
            && let Some(close) = closing(over)
        {
            return Some(close);
        }
        if !works_over(over, &action) {
            self.show_warning(format!("{keys} doesn't work over {}", over.name()));
            return None;
        }
        // The tasks panel, over the others, makes way for the one asked.
        if over == overlay::Overlay::Tasks && opens_panel(&action) {
            self.close_tasks();
        }
        // So does the machines panel.
        if over == overlay::Overlay::Machines && (opens_panel(&action) || action == Action::Tasks) {
            self.close_machines();
        }
        Some(Intercepted::Action(action))
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
        if self.on_footer(pos) {
            return regions;
        }
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
        // Over an overlay, only its own controls count.
        if let Some(overlay) = self.overlay_at(pos) {
            return if self.overlay_control_at(overlay, pos) {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
        // The footer: its chips and its count.
        if self.on_footer(pos) {
            return if self.footer_hit(pos).is_some() {
                RegionHit::Control
            } else {
                RegionHit::Nothing
            };
        }
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
        // Over the apps: the pane bars' buttons, a placeholder's, the one
        // unzooming a zoomed pane.
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
        if self.repos_rows_drag || self.repos_editor_drag {
            self.repos_drag_rows(pos);
        }
        if self.scrollback_dragging() {
            self.scrollback_drag(pos);
        }
        if self.picking_page_text() {
            self.drag_page_text(pos);
        }
        if self.thumb_drag.is_some() {
            self.drag_thumb(pos);
        }
        // The overlay on top at the pointer takes the hover from
        // everything under it; its icons have tooltips of their own.
        let at = self.overlay_at(pos);
        let overlay_tip = at.and_then(|overlay| self.overlay_hover(overlay, pos));
        if at != Some(overlay::Overlay::Doc)
            && let Some(review) = self.review_tab_mut()
        {
            review.wd.hover = None;
        }
        if at != Some(overlay::Overlay::Criteria) {
            self.criteria_strip.hover = None;
        }
        // The keys of the help panel light up under the pointer.
        self.help_hover = if at == Some(overlay::Overlay::Help) {
            self.help_hit(pos)
        } else {
            None
        };
        // The toast under the pointer waits there; the others go on.
        if !self.toasts.is_empty() {
            let over = (at == Some(overlay::Overlay::Toasts)).then_some(pos);
            self.toast_hover(over);
        }
        // The region under the pointer lights up, with nothing open
        // over it, and may offer a tooltip. Its layout is worked
        // out once for the event.
        let covered = at.is_some();
        self.header_hover = false;
        let regions = self.regions_at(pos);
        // The strip of agents closes off the sidebar (see
        // `track_agent_strip`).
        if covered || regions.sidebar.is_none() {
            self.agent_strip = None;
            self.agent_strip_due = None;
        }
        let on_footer = !covered && self.on_footer(pos);
        if !on_footer {
            self.footer_motion(None);
        }
        let tip = if covered {
            overlay_tip
        } else if on_footer {
            self.footer_motion(Some(pos))
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
        let under = self.pointer_client(pos);
        // Over gerdr's chrome or a divider: gerdr's cursor shape, not a client's.
        if under.is_none() {
            let over_field = at.is_some_and(|o| {
                o.grabs() || matches!(o, overlay::Overlay::Features | overlay::Overlay::Criteria)
            })
                && self
                    .field_spot()
                    .is_some_and(|field| field.area.to_f64().contains(pos))
                || self.on_scrollback_text(pos)
                || at == Some(overlay::Overlay::Features) && self.on_page_text(pos);
            let hit = self.region_hit(pos, &regions);
            // What can be held and dragged: a workspace row, a tab, a
            // pane by its bar's label, a scrollbar's thumb.
            let draggable = || {
                matches!(hit, RegionHit::Workspace(_) | RegionHit::Tab(_))
                    || !covered && matches!(self.pane_bar_at(pos), Some((_, BarHit::Label)))
                    || self.on_thumb(pos)
            };
            let icon = if over_field {
                CursorIcon::Text
            } else if self.reorder.is_some_and(|r| r.moving) || self.thumb_drag.is_some() {
                CursorIcon::Grabbing
            } else if let Some(drag) = self.drag.clone().or_else(|| self.divider_at(pos)) {
                drag_cursor(&drag)
            } else if draggable() {
                CursorIcon::Grab
            } else if hit != RegionHit::Nothing
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
        let under = self.pointer_client(location);
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
            Intercepted::AiStats(outcome) => self.apply_ai_stats(outcome),
            Intercepted::CloseSwitcher => {
                self.switcher = None;
                self.refresh_pointer_focus();
            }
            Intercepted::Tasks(outcome) => self.apply_tasks(outcome),
            Intercepted::Machines(outcome) => self.apply_machines_panel(outcome),
            Intercepted::NewFeature(outcome) => self.apply_new_feature(outcome),
            Intercepted::FindingDialog(outcome) => self.apply_finding_dialog(outcome),
            Intercepted::Identity(outcome) => self.apply_identity(outcome),
            Intercepted::Machine(outcome) => self.apply_machine(outcome),
            Intercepted::Commands(outcome) => self.apply_commands(outcome),
            Intercepted::Features(outcome) => self.apply_feature_panel(outcome),
        }
    }

    /// The overlay with text fields that has the keyboard, if any: the
    /// one on top that grabs it. The dialogs have none.
    fn text_overlay(&self) -> Option<Overlay> {
        use crate::overlay::Overlay as O;
        // The feature panel's tab or a file's tab, with nothing over it.
        let Some(top) = self.grabbing_overlay() else {
            match self.key_overlay() {
                Some(O::Doc) => return Some(Overlay::Repos),
                Some(O::Criteria) => return Some(Overlay::Criteria),
                _ => {}
            }
            return self.is_open(O::Features).then_some(Overlay::Features);
        };
        match top {
            O::Welcome => Some(Overlay::Welcome),
            O::ProjectMenu => Some(Overlay::ProjectMenu),
            O::Worktrees => Some(Overlay::Worktrees),
            O::Repos => Some(Overlay::Repos),
            O::Inbox => Some(Overlay::Inbox),
            O::AiStats => Some(Overlay::AiStats),
            O::Scrollback => Some(Overlay::Scrollback),
            O::Switcher => self.switcher.as_ref().map(|s| Overlay::Switcher(s.serial)),
            O::Tasks => Some(Overlay::Tasks),
            O::Machines => Some(Overlay::MachinesPanel),
            O::NewFeature => Some(Overlay::NewFeature),
            O::FindingDialog => Some(Overlay::FindingDialog),
            O::GitIdentity => Some(Overlay::Identity),
            O::ConnectMachine => Some(Overlay::Machine),
            O::ProjectCommands => Some(Overlay::Commands),
            _ => None,
        }
    }

    /// The clipboard's text read for a paste into overlay `into`, if it
    /// still has the keyboard: typed into its focused field as the key
    /// `text_field::PASTE`, through its own key handling.
    pub fn paste_text(&mut self, into: Overlay, text: &str) {
        if self.text_overlay() != Some(into) {
            return;
        }
        self.field_scroll_due = true;
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
            Overlay::AiStats => self
                .ai_stats_key(mods, sym, typed)
                .map(Intercepted::AiStats),
            Overlay::Scrollback => self
                .scrollback_key(mods, sym, typed)
                .map(Intercepted::Scrollback),
            Overlay::Switcher(_) => self
                .switcher_key(mods, sym, typed)
                .map(|(target, place)| Intercepted::Switch(target, place)),
            Overlay::Tasks => self.tasks_key(mods, sym).map(Intercepted::Tasks),
            Overlay::MachinesPanel => self
                .machines_panel_key(mods, sym)
                .map(Intercepted::Machines),
            Overlay::NewFeature => self
                .new_feature_key(mods, sym, typed)
                .map(Intercepted::NewFeature),
            Overlay::FindingDialog => self
                .finding_dialog_key(mods, sym, typed)
                .map(Intercepted::FindingDialog),
            Overlay::Identity => self
                .identity_key(mods, sym, typed)
                .map(Intercepted::Identity),
            Overlay::Machine => self
                .machine_key(mods, sym, typed)
                .map(Intercepted::Machine),
            Overlay::Commands => self
                .commands_key(mods, sym, typed)
                .map(Intercepted::Commands),
            Overlay::Features => self
                .feature_panel_key(mods, sym, typed)
                .map(Intercepted::Features),
            Overlay::Criteria => {
                self.criteria_strip_key(mods, sym, typed);
                None
            }
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
        if let Some(overlay) = self.overlay_at(pos) {
            let _ = self.overlay_hover(overlay, pos);
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
            Overlay::Worktrees => Some(chrome::worktrees_field(
                &self.worktrees_view()?,
                size,
                scale,
            )),
            Overlay::Repos => Some(chrome::repos_field(
                &self.repos_view()?,
                self.repos_rect(),
                scale,
            )),
            Overlay::Inbox => Some(chrome::inbox_field(&self.inbox_view()?, size, scale)),
            Overlay::AiStats => chrome::stats_field(&self.ai_stats_view()?, size, scale),
            Overlay::Scrollback => chrome::scrollback_field(&self.scrollback_view()?, scale),
            Overlay::Switcher(_) => Some(chrome::switcher_field(
                &self.switcher_view()?,
                self.content_rect(),
                scale,
            )),
            // Its fields hold lines: it places their carets itself.
            Overlay::Tasks | Overlay::MachinesPanel | Overlay::FindingDialog => None,
            // A criterion's note, while one is written, or their search
            // while it has the keys.
            Overlay::Features => {
                // Code review's diff, its search while it has the keys.
                if (self.inline_wd()).is_some_and(|wd| wd.diff.search_focused()) {
                    let view = self.repos_view()?;
                    return Some(chrome::repos_field(&view, self.repos_rect(), scale));
                }
                let panel = self.features.panel.as_ref()?;
                let typing = (panel.search.as_ref()).is_some_and(|s| s.typing);
                if panel.note.is_none() && !typing {
                    return None;
                }
                chrome::feature_tab_field(&self.feature_tab_view()?, self.content_rect(), scale)
            }
            Overlay::Criteria => {
                chrome::criteria_strip_field(&self.criteria_strip_view()?, scale)
            }
            Overlay::NewFeature => Some(chrome::new_feature_field(
                &self.new_feature_view()?,
                size,
                scale,
            )),
            Overlay::Identity => chrome::identity_field(&self.identity_view()?, size, scale),
            Overlay::Machine => chrome::machine_field(&self.machine_view()?, size, scale),
            Overlay::Commands => chrome::commands_field(&self.commands_view()?, size, scale),
        }
    }

    /// The text field with the keyboard as the pointer at `pos` finds it,
    /// and the byte of its text that starts at: a field of lines is the
    /// line there (`held`: a press from it is being dragged out of it).
    fn field_line_at(&self, pos: Point<f64, Logical>, held: bool) -> Option<(FieldSpot, usize)> {
        if self.text_overlay() == Some(Overlay::Commands) {
            let (size, scale) = (self.output_size(), self.scale());
            return chrome::commands_field_at(&self.commands_view()?, size, scale, pos, held);
        }
        let spot = self.field_spot()?;
        // A card's note holds lines, as the criteria strip's does; one line,
        // as Code review's search, is the field as it is.
        if matches!(
            self.text_overlay(),
            Some(Overlay::Features | Overlay::Criteria)
        ) && spot.line.contains('\n')
        {
            return Some(chrome::field_line_spot(
                spot.area,
                spot.area,
                &spot.line,
                pos,
                self.scale(),
            ));
        }
        Some((spot, 0))
    }

    /// The text field with the keyboard, in the overlay that has it.
    fn field_mut(&mut self) -> Option<&mut TextField> {
        match self.text_overlay()? {
            Overlay::Welcome => self.welcome.as_mut()?.field_mut(),
            Overlay::ProjectMenu => self.project_menu.as_mut()?.field_mut(),
            Overlay::Worktrees => self.worktrees.field_mut(),
            // A file's tab holds its own search.
            Overlay::Repos if !self.repos.is_open() => self.wd_mut()?.diff.search_field_mut(),
            Overlay::Repos => self.repos.field_mut(),
            Overlay::Inbox => self.inbox.field_mut(),
            Overlay::AiStats => self.ai_stats_field_mut(),
            Overlay::Scrollback => self.scrollback.as_mut()?.field_mut(),
            Overlay::Switcher(_) => Some(self.switcher.as_mut()?.field_mut()),
            Overlay::Tasks | Overlay::MachinesPanel => None,
            Overlay::Features => {
                if (self.inline_wd()).is_some_and(|wd| wd.diff.search_focused()) {
                    return self.inline_wd_mut()?.diff.search_field_mut();
                }
                let panel = self.features.panel.as_mut()?;
                match &mut panel.note {
                    Some((_, field)) => Some(field),
                    None => (panel.search.as_mut())
                        .filter(|s| s.typing)
                        .map(|s| &mut s.query),
                }
            }
            Overlay::Criteria => self.criteria_field_mut(),
            Overlay::NewFeature => Some(self.features.dialog.as_mut()?.field_mut()),
            Overlay::FindingDialog => self.finding_dialog_field_mut(),
            Overlay::Identity => self.identity_field_mut(),
            Overlay::Machine => self.machine_field_mut(),
            Overlay::Commands => self.commands_field_mut(),
        }
    }

    /// A left press at `pos` over an overlay, once the overlay took it (a
    /// click on a field focuses it): in the text field with the keyboard,
    /// it puts the caret there (Shift: the selection reaches there), and
    /// holding it drags a selection; a second press in a row selects a
    /// word, a third all of it.
    fn press_field(&mut self, pos: Point<f64, Logical>) {
        let Some((field, base)) = self
            .field_line_at(pos, false)
            .filter(|(field, _)| field.area.to_f64().contains(pos))
        else {
            return;
        };
        let at = base + self.chrome.caret_at(&field, pos.x, self.scale());
        let count = self.press_count;
        let extend = self.seat.get_keyboard().unwrap().modifier_state().shift;
        if let Some(field) = self.field_mut() {
            field.click(at, extend, count);
        }
        self.field_drag = count == 1;
    }

    /// How many left presses in a row the one at `pos` is, a moment apart
    /// and in about one place: the second selects a word in a text field,
    /// the third all of it.
    fn count_press(&self, pos: Point<f64, Logical>, button: u32) -> u32 {
        match self.last_press {
            Some((when, from, b))
                if b == button
                    && when.elapsed() < MULTI_CLICK
                    && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                    && (from.y - pos.y).abs() < MULTI_CLICK_SLOP =>
            {
                self.press_count + 1
            }
            _ => 1,
        }
    }

    /// The text field a held press drags in, as the pointer at `pos` finds
    /// it, and the byte of its text that starts at (see `field_line_at`):
    /// the name being typed, else the overlay's field with the keyboard.
    fn held_field_at(&self, pos: Point<f64, Logical>) -> Option<(FieldSpot, usize)> {
        if self.rename.is_some() {
            return self.rename_field_spot().map(|spot| (spot, 0));
        }
        self.field_line_at(pos, true)
    }

    /// The text field a held press drags in: see `held_field_at`.
    fn held_field_mut(&mut self) -> Option<&mut TextField> {
        if self.rename.is_none() {
            return self.field_mut();
        }
        self.rename.as_mut().map(|rename| &mut rename.field)
    }

    /// Whether a held press drags in the finding dialog's field.
    fn holds_finding_field(&self) -> bool {
        self.rename.is_none() && self.text_overlay() == Some(Overlay::FindingDialog)
    }

    /// The pointer moved to `pos` holding a press in a text field: the
    /// selection reaches the character there, of the text in view (held
    /// past it, `field_edge_tick` scrolls).
    fn drag_field(&mut self, pos: Point<f64, Logical>) {
        self.field_scroll_due = true;
        if self.holds_finding_field() {
            return self.finding_dialog_drag(pos);
        }
        let Some((field, base)) = self.held_field_at(pos) else {
            self.field_drag = false;
            return;
        };
        let scale = self.scale();
        let (from, to) = (field.text.0 as f64 / scale, field.text.1 as f64 / scale);
        let x = pos.x.clamp(from, to.max(from));
        let at = base + self.chrome.caret_at(&field, x, scale);
        if let Some(field) = self.held_field_mut() {
            field.drag_to(at);
        }
    }

    /// How far the pointer at `pos` is past the text of the field a press
    /// is held in, in logical pixels: left or above it negative, none
    /// within. A field of lines that doesn't scroll is past only to the
    /// left or right, on the line there.
    fn field_edge(&self, pos: Point<f64, Logical>) -> f64 {
        if self.holds_finding_field() {
            return self.finding_dialog_edge(pos);
        }
        let Some((field, _)) = self.held_field_at(pos) else {
            return 0.0;
        };
        let scale = self.scale();
        let (from, to) = (field.text.0 as f64 / scale, field.text.1 as f64 / scale);
        match pos.x {
            x if x < from => x - from,
            x if x > to => x - to,
            _ => 0.0,
        }
    }

    /// While a press is held in a text field past its text, scroll on to
    /// `now`, the pointer moving or not, as fast as the pointer is far past
    /// (see `scrollback::edge_speed`): the selection goes on a character a
    /// step along the line there, or a row a step in the finding dialog.
    /// Whether it scrolls.
    pub(crate) fn field_edge_tick(&mut self, now: Instant) -> bool {
        let edge = match self.field_drag {
            true => self.field_edge(self.pointer),
            false => 0.0,
        };
        let by = crate::scrolling::edge_steps(&mut self.field_edge, now, edge);
        if by != 0 {
            self.field_scroll_due = true;
            if self.holds_finding_field() {
                self.finding_dialog_past(by);
            } else if let Some((field, base)) = self.held_field_at(self.pointer) {
                let line = &field.line;
                let at = base + text_field::chars_from(line, line.caret.min(line.len()), by);
                if let Some(field) = self.held_field_mut() {
                    field.drag_to(at);
                }
            }
        }
        edge != 0.0
    }

    /// Keep where the view of the field with the keyboard starts as it is
    /// drawn, for the next frame (see `Line::scroll`): after a key, a paste
    /// or a press may have moved its caret (`field_scroll_due`), not every
    /// frame, as the views it takes are not cheap.
    pub(crate) fn keep_field_scroll(&mut self) {
        if !std::mem::take(&mut self.field_scroll_due) {
            return;
        }
        let scale = self.scale();
        if self.rename.is_some() {
            let at = (self.rename_field_spot()).map(|spot| self.chrome.field_first(&spot, scale));
            if let (Some(at), Some(rename)) = (at, self.rename.as_mut()) {
                rename.field.set_scroll(at);
            }
        }
        if self.text_overlay() == Some(Overlay::FindingDialog) {
            return self.finding_dialog_keep_scroll();
        }
        let at = (self.field_spot()).map(|spot| self.chrome.field_first(&spot, scale));
        if let (Some(at), Some(field)) = (at, self.field_mut()) {
            field.set_scroll(at);
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
            state.field_scroll_due = true;
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
            state.field_scroll_due = true;
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
    fn over_a_panel_the_prefix_acts_on_gerdr_and_opens_the_other_panels() {
        assert!(over_panel(&Action::ToggleTheme));
        assert!(over_panel(&Action::Quit));
        assert!(over_panel(&Action::Shortcuts));
        // These open over the others, or in their place.
        assert!(over_panel(&Action::Projects));
        assert!(over_panel(&Action::Inbox));
        assert!(over_panel(&Action::Repos));
        assert!(over_panel(&Action::Worktrees));
        assert!(!over_panel(&Action::LastProject));
        assert!(!over_panel(&Action::NewTab));
        assert!(!over_panel(&Action::Switcher));
    }

    #[test]
    fn the_key_that_opened_a_panel_closes_it() {
        use crate::overlay::Overlay as O;
        for panel in O::ORDER.into_iter().filter(|o| o.is_panel()) {
            assert!(panel.toggled_by().is_some(), "{panel:?}");
            assert!(closing(panel).is_some(), "{panel:?}");
        }
        assert!(closing(O::Welcome).is_none());
        // Over the welcome screen, only what needs no project.
        assert!(works_over(O::Welcome, &Action::Quit));
        assert!(!works_over(O::Welcome, &Action::Repos));
        assert!(works_over(O::Switcher, &Action::Inbox));
    }

    #[test]
    fn ctrl_held_from_the_prefix_picks_the_key_alone_and_pressed_again_with_ctrl() {
        // After Ctrl+B, X and Ctrl+X do different things.
        let (keymap, ..) = crate::config::from_text("[keys]\nnew-tab = \"Ctrl+X\"\n");
        let none = ModifiersState::default();
        let ctrl = ModifiersState {
            ctrl: true,
            ..Default::default()
        };
        let alone = keymap.action(&none, Keysym::x);
        assert!(alone.is_some() && alone != Some(Action::NewTab));
        // Ctrl still down from the prefix: the key alone.
        assert_eq!(after_prefix(&keymap, &ctrl, Keysym::x, true), (none, alone));
        // Ctrl pressed again after the prefix: Ctrl+X.
        assert_eq!(
            after_prefix(&keymap, &ctrl, Keysym::x, false),
            (ctrl, Some(Action::NewTab))
        );
        // Ctrl let go of: the key alone, either way.
        assert_eq!(after_prefix(&keymap, &none, Keysym::x, true).1, alone);
        // A key with an action only with Ctrl takes it, Ctrl held from the
        // prefix or not.
        let config = "[keys]\nnew-tab = \"Ctrl+X\"\nclose-pane = []\n";
        let (keymap, ..) = crate::config::from_text(config);
        assert_eq!(keymap.action(&none, Keysym::x), None);
        assert_eq!(
            after_prefix(&keymap, &ctrl, Keysym::x, true).1,
            Some(Action::NewTab)
        );
    }

    #[test]
    fn notices_name_keys_as_the_help_does() {
        let none = ModifiersState::default();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        assert_eq!(key_name(&none, Keysym::q), "Q");
        assert_eq!(key_name(&shift, Keysym::q), "Shift+Q");
        assert_eq!(key_name(&none, Keysym::Escape), "Esc");
        assert_eq!(key_name(&none, Keysym::F5), "F5");
    }
}
