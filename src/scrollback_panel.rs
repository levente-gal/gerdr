//! The scrollback viewer's share of gerdr (see `scrollback`): opening it
//! over a terminal pane with all herdr keeps of the pane's output, placing
//! it, and taking the keys and the pointer while it is open.

use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Rectangle},
};

use crate::{
    Gerdr,
    chrome::{self, Rect, ScrollbackHit, ScrollbackRow, ScrollbackView, SearchPart},
    feature::{fix_round, setup, store},
    herdr,
    input::{MULTI_CLICK, MULTI_CLICK_SLOP, Overlay},
    key_table,
    scroll::Motion,
    scrollback::{self, Fit, Key, Outcome, Scrollback, Source},
    toast::ToastHit,
    tooltip::Tooltip,
};

/// A pane smaller than this either way has the viewer over the whole app
/// area instead, to have room to read in.
const MIN_WIDTH: i32 = 420;
const MIN_HEIGHT: i32 = 240;
/// How far the viewer stands in from its pane's edges, so its own show.
const INSET: i32 = 4;
/// How often a log shown is looked at for what it gained.
const FOLLOW: Duration = Duration::from_millis(250);

/// The keys F1 lists that are no rows of `scrollback::KEYS`: the caret's
/// and the pages' (`Scrollback::motion`), before them.
const MOTION_KEYS: &[(&str, &str)] = &[
    ("Arrows / H J K L", "Move the caret"),
    (
        "W / B / E",
        "Go to the next word, the word before, the word's end",
    ),
    (
        "0 / ^ / $",
        "Go to the line's start, its first character, its end",
    ),
    ("G / Home", "Go to the top"),
    ("Shift+G / End", "Go to the bottom"),
    ("PgUp / PgDn", "Go a page up or down"),
    ("Space", "Go a page down"),
    ("Ctrl+U / Ctrl+D", "Go half a page up or down"),
    ("Shift+Arrows", "Select as the caret goes"),
];

/// What else F1 lists, after the keys: `*` and `#` (`scrollback_key`)
/// and the pointer's ways.
const MORE_KEYS: &[(&str, &str)] = &[
    (
        "* / #",
        "Find the selection or the word at the caret, on or back",
    ),
    (
        "Drag / Shift+Click",
        "Select with the pointer, or reach the selection there",
    ),
    ("Double Click / Triple Click", "Select a word, or a line"),
];

/// Every key the viewer takes, and the pointer's ways, as F1 lists them.
fn keys_sheet() -> Vec<(String, String)> {
    let listed = |rows: &[(&str, &str)]| {
        (rows.iter())
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect::<Vec<_>>()
    };
    let mut sheet = listed(MOTION_KEYS);
    sheet.extend(key_table::joined(key_table::f1_rows(scrollback::KEYS)));
    sheet.extend(listed(MORE_KEYS));
    sheet
}

/// The viewer's key hints, each a key it takes and what it does: the
/// search's while it is open, and while its field has the keyboard only
/// those; `v`'s while it is on; the selection's while there is one. Esc
/// says what it does now (see `Scrollback::key`). F1 and Esc are the
/// last two, kept when the footer is too narrow for all.
fn hints(viewer: &Scrollback) -> Vec<(String, String)> {
    if viewer.keys_open() {
        let hints = key_table::hints(scrollback::SHEET_KEYS, &(), |_| "close keys".to_owned());
        return key_table::joined(hints);
    }
    let ctx = viewer.key_ctx(true);
    let label = |key: &Key| {
        match key {
            Key::Find { back: false } => "next",
            Key::Find { back: true } => "previous",
            Key::ToggleFind if ctx.find == Some(true) => "to the text",
            Key::ToggleFind => "to the search",
            Key::Copy | Key::Yank => "copy",
            Key::CopyClose => "copy and close",
            Key::Close => "close",
            Key::Visual(_) => "select",
            Key::SelectAll => "select all",
            Key::OpenFind => "find",
            Key::Reload => "reload",
            Key::Keys => "keys",
            Key::ClearFind => "clear",
            Key::CloseFind => "close search",
            Key::ClearSelection if ctx.visual => "stop selecting",
            Key::ClearSelection => "clear",
        }
        .to_owned()
    };
    // Esc and Q, both closing, are one choice: "Esc / Q".
    key_table::joined(key_table::hints(scrollback::KEYS, &ctx, label))
}

/// What the viewer shows of log `path`, and how long the file is: a log
/// not there yet is empty, as its setup makes it once it starts.
fn read_log(path: &Path) -> std::io::Result<(String, u64)> {
    match std::fs::read(path) {
        Ok(raw) => Ok((setup::shown_log(&raw), raw.len() as u64)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), 0)),
        Err(err) => Err(err),
    }
}

/// A pane's output as read off the event loop: its text, how many rows up
/// from the bottom its view stands, and its colours (see `read_styled`).
type PaneRead = Result<(String, u64, String), String>;

/// All herdr at `socket` keeps of pane `pane_id`'s output, and its colours.
fn read_pane(socket: PathBuf, pane_id: &str) -> PaneRead {
    let mut herdr = herdr::Herdr::on_socket(socket);
    let (text, from_bottom) = herdr.pane_scrollback(pane_id)?;
    let styled = read_styled(&mut herdr, pane_id, &text);
    Ok((text, from_bottom, styled))
}

/// The end of pane `pane_id`'s output as its terminal drew it, as
/// many lines as `text` (all of it, read plainly) has at most, for the
/// viewer's colours: none, the text drawn plainly, when herdr can't
/// give it. Refused that many, it is asked for the 1000 it is known
/// to give.
fn read_styled(herdr: &mut herdr::Herdr, pane_id: &str, text: &str) -> String {
    let lines = text.lines().count().max(1);
    let read = herdr
        .pane_styled(pane_id, lines)
        .or_else(|err| match lines > 1000 {
            true => herdr.pane_styled(pane_id, 1000),
            false => Err(err),
        });
    read.unwrap_or_else(|err| {
        tracing::debug!("colours of the scrollback of {pane_id}: {err}");
        String::new()
    })
}

/// `rect` less `by` on every side.
fn inset(rect: Rect, by: i32) -> Rect {
    Rectangle::new(
        (rect.loc.x + by, rect.loc.y + by).into(),
        ((rect.size.w - 2 * by).max(1), (rect.size.h - 2 * by).max(1)).into(),
    )
}

impl Gerdr {
    /// Open the viewer over the focused pane, when it is a terminal.
    pub fn open_focused_scrollback(&mut self) {
        let focused = self.tab().and_then(|tab| {
            let id = tab.focused_id()?;
            tab.herdr_pane(id).map(|_| id)
        });
        match focused {
            Some(id) => self.open_scrollback(id),
            None => self.show_notice("Not a terminal pane".into()),
        }
    }

    /// Open the viewer over terminal pane `id` of the active tab, with all
    /// its pane keeps of its output, standing where the pane's view does.
    pub fn open_scrollback(&mut self, id: u32) {
        let Some(pane_id) = self
            .tab()
            .and_then(|tab| tab.herdr_pane(id))
            .map(|p| p.pane_id.clone())
        else {
            return self.show_notice("Not a terminal pane".into());
        };
        self.focus_pane(|_| Some(id));
        if let Err(err) = self.read_scrollback(&pane_id, true) {
            tracing::warn!("scrollback of {pane_id}: {err}");
            return self.show_error(format!("Couldn't read the pane's output: {err}"));
        }
        let label = self.scrollback_label(&pane_id);
        self.close_for_scrollback();
        // Empty until the read comes back (see `scrollback_read`).
        let mut viewer = Scrollback::new(id, pane_id, label, "", 0);
        if let Some(fit) = self.scrollback_fit_for(id) {
            viewer.fit(fit);
        }
        self.overlays.scrollback = Some(viewer);
        // It takes the pointer from the terminal under it.
        self.refresh_pointer_focus();
    }

    /// Read pane `pane_id`'s output off the event loop, for the viewer
    /// `opening` over it or reading it again.
    fn read_scrollback(&mut self, pane_id: &str, opening: bool) -> Result<(), String> {
        let socket = (self.session.herdr.as_ref())
            .map(|h| h.socket().to_path_buf())
            .ok_or("no herdr session")?;
        let (tx, rx) = channel::channel();
        let id = pane_id.to_owned();
        (self.loop_handle)
            .insert_source(rx, move |event, _, state| {
                if let channel::Event::Msg(read) = event {
                    state.scrollback_read(&id, opening, read);
                }
            })
            .map_err(|err| err.to_string())?;
        let id = pane_id.to_owned();
        thread::Builder::new()
            .name("scrollback".into())
            .spawn(move || {
                // The loop is gone only as gerdr quits.
                let _ = tx.send(read_pane(socket, &id));
            })
            .map_err(|err| err.to_string())?;
        self.overlays.scrollback_reading = Some(pane_id.to_owned());
        Ok(())
    }

    /// Show what a read of pane `pane_id` gave in the viewer, opening or
    /// reading it again. A viewer closed since, or over something else
    /// now, drops it: nothing waits for it any more.
    fn scrollback_read(&mut self, pane_id: &str, opening: bool, read: PaneRead) {
        let pane = match self.overlays.scrollback.as_ref().map(|v| &v.source) {
            Some(Source::Pane { pane, pane_id: p }) if p == pane_id => *pane,
            _ => return,
        };
        if self.overlays.scrollback_reading.as_deref() != Some(pane_id) {
            return;
        }
        self.overlays.scrollback_reading = None;
        let (text, from_bottom, styled) = match read {
            Ok(read) => read,
            Err(err) if opening => {
                tracing::warn!("scrollback of {pane_id}: {err}");
                self.close_scrollback();
                return self.show_error(format!("Couldn't read the pane's output: {err}"));
            }
            Err(err) => {
                tracing::warn!("scrollback of {pane_id}: {err}");
                return self.show_error(format!("Couldn't read the pane's output again: {err}"));
            }
        };
        if opening {
            // Its errors are in view now.
            self.see_service_errors(pane_id);
            let label = self
                .overlays
                .scrollback
                .take()
                .map(|v| v.label)
                .unwrap_or_default();
            let mut viewer = Scrollback::new(pane, pane_id.to_owned(), label, &text, from_bottom);
            viewer.set_styles(&styled);
            if let Some(fit) = self.scrollback_fit_for(pane) {
                viewer.fit(fit);
            }
            self.overlays.scrollback = Some(viewer);
            return;
        }
        let Some((_, fit)) = self.scrollback_place() else {
            return;
        };
        let label = self.scrollback_label(pane_id);
        if let Some(viewer) = self.overlays.scrollback.as_mut() {
            viewer.label = label;
            viewer.reload(&text, fit);
            viewer.set_styles(&styled);
            let n = viewer.line_count();
            let s = if n == 1 { "" } else { "s" };
            self.show_success(format!("Snapshot read again: {n} line{s}"));
        }
    }

    /// What the scrollback of `pane_id` is titled by: what runs there, else
    /// the pane's folder. Not the last command once it ended: a shell's own
    /// loops and builtins start no process, so it may be long past.
    fn scrollback_label(&self, pane_id: &str) -> String {
        let content = self.pane_bar_content(pane_id);
        if content.status.runs() {
            return content.label;
        }
        (self.pane_state(pane_id).and_then(|p| p.cwd.as_deref()))
            .map(|cwd| crate::paths::tilde(cwd, crate::home_dir().as_deref()))
            .unwrap_or(content.label)
    }

    /// What the viewer takes the place of goes.
    fn close_for_scrollback(&mut self) {
        // Another file in the viewer: the stats panel isn't what is under it.
        self.overlays.ai_stats_behind = None;
        self.overlays.switcher = None;
        self.overlays.project_menu = None;
        self.worktrees.close();
        self.repos.close();
        self.inbox.close();
        self.stop_key_repeat();
        self.close_menu();
        self.hover.tooltip.hide();
        self.chrome.drop_tooltip();
    }

    /// Open the viewer on the last run of feature `slug`'s setup log, over
    /// the app area, at its end: it follows the log while it stays there.
    /// Show round `n`'s fix plan of feature `slug`: the latest in its tab,
    /// where it is reviewed, an earlier one put aside in the viewer, only to
    /// read and copy from.
    pub fn open_round_plan(&mut self, project: &str, slug: &str, n: u32) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if n == feature.fix_rounds {
            return self.open_doc_tab(project, slug, crate::doc_review::DocFile::FixPlan);
        }
        let title = feature.title.clone();
        let label = fix_round::label(n);
        let path = fix_round::archive_path(&store::dir(None, project, slug), n);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                return self.show_error(format!("Couldn't read the plan of {label}: {err}"));
            }
        };
        self.close_for_scrollback();
        let what = format!("Fix plan of {label}");
        self.overlays.scrollback = Some(Scrollback::of(Source::File { what }, title, &text, 0));
        if let Some((_, fit)) = self.scrollback_place()
            && let Some(viewer) = self.overlays.scrollback.as_mut()
        {
            viewer.fit(fit);
        }
        self.refresh_pointer_focus();
    }

    pub fn open_setup_log(&mut self, project: &str, slug: &str) {
        let Some(title) = self.feature(project, slug).map(|f| f.title.clone()) else {
            return;
        };
        let path = setup::log_path(&store::dir(None, project, slug));
        let (text, read) = match read_log(&path) {
            Ok(log) => log,
            Err(err) => {
                tracing::warn!("{}: {err}", path.display());
                return self.show_error(format!("Couldn't read the setup log: {err}"));
            }
        };
        self.close_for_scrollback();
        let source = Source::SetupLog {
            project: project.to_owned(),
            slug: slug.to_owned(),
            path,
            read,
        };
        let mut viewer = Scrollback::of(source, title, &text, 0);
        viewer.set_styles(&setup::log_styles(&text));
        self.overlays.scrollback = Some(viewer);
        if let Some((_, fit)) = self.scrollback_place()
            && let Some(viewer) = self.overlays.scrollback.as_mut()
        {
            viewer.fit(fit);
        }
        // It takes the pointer from the terminal under it.
        self.refresh_pointer_focus();
        let follow = |_, _: &mut (), state: &mut Gerdr| match state.follow_log() {
            true => TimeoutAction::ToDuration(FOLLOW),
            false => TimeoutAction::Drop,
        };
        if let Err(err) = (self.loop_handle).insert_source(Timer::from_duration(FOLLOW), follow) {
            tracing::warn!("following the setup log: {err}");
        }
    }

    /// Show what the log the viewer shows gained. A selection stops it, as
    /// reading drops the selection: it catches up once that goes. Whether
    /// the viewer still shows a log.
    fn follow_log(&mut self) -> bool {
        let Some(viewer) = self.overlays.scrollback.as_ref() else {
            return false;
        };
        let Source::SetupLog { path, read, .. } = &viewer.source else {
            return false;
        };
        if viewer.has_selection() || viewer.dragging() || viewer.selecting() {
            return true;
        }
        let len = std::fs::metadata(path).map_or(0, |m| m.len());
        if len != *read {
            self.reread_log();
        }
        true
    }

    /// Read the log the viewer shows again; whether it could. Why not goes to
    /// the log: the viewer still shows what it read.
    fn reread_log(&mut self) -> bool {
        let Some((_, fit)) = self.scrollback_place() else {
            return false;
        };
        let Some(viewer) = self.overlays.scrollback.as_mut() else {
            return false;
        };
        let Source::SetupLog { path, read, .. } = &mut viewer.source else {
            return false;
        };
        let (text, len) = match read_log(path) {
            Ok(log) => log,
            Err(err) => {
                tracing::warn!("{}: {err}", path.display());
                return false;
            }
        };
        *read = len;
        viewer.reload(&text, fit);
        viewer.set_styles(&setup::log_styles(&text));
        true
    }

    /// Read the viewer's pane again, for what it printed since the viewer
    /// opened (see `Scrollback::reload`), or its log.
    pub fn reload_scrollback(&mut self) {
        let Some(source) = self.overlays.scrollback.as_ref().map(|v| v.source.clone()) else {
            return;
        };
        let Source::Pane { pane_id, .. } = source else {
            if self.reread_log()
                && let Some(n) = self
                    .overlays
                    .scrollback
                    .as_ref()
                    .map(Scrollback::line_count)
            {
                let s = if n == 1 { "" } else { "s" };
                self.show_success(format!("Log read again: {n} line{s}"));
            } else {
                self.show_error("Couldn't read the setup log again".into());
            }
            return;
        };
        // One read at a time: the note says it reads.
        if self.overlays.scrollback_reading.is_some() {
            return;
        }
        if let Err(err) = self.read_scrollback(&pane_id, false) {
            tracing::warn!("scrollback of {pane_id}: {err}");
            self.show_error(format!("Couldn't read the pane's output again: {err}"));
        }
    }

    /// Close the viewer.
    pub fn close_scrollback(&mut self) {
        self.overlays.scrollback = None;
        self.overlays.scrollback_reading = None;
        // Opened from the stats panel, the viewer goes back to it.
        if let Some(panel) = self.overlays.ai_stats_behind.take() {
            self.overlays.ai_stats = Some(panel);
            self.ai_stats_refresh();
        }
        // Gone, the terminal under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// Where the viewer over pane `id` goes: over the pane in the active
    /// tab, its bar too, or over the app area when the pane is too small
    /// to read in (or, while they are listed, to list its keys in);
    /// `None` while the pane doesn't show.
    fn scrollback_panel_for(&self, id: u32) -> Option<Rect> {
        let area = self.content_rect();
        let (_, rect) = self
            .tab()?
            .shown_rects(area)
            .into_iter()
            .find(|(pane, _)| *pane == id)?;
        let keys = self
            .overlays
            .scrollback
            .as_ref()
            .is_some_and(Scrollback::keys_open);
        let small = rect.size.w < MIN_WIDTH || rect.size.h < MIN_HEIGHT;
        let room = if small
            || (keys && !chrome::scrollback_keys_fit(inset(rect, INSET), keys_sheet().len()))
        {
            area
        } else {
            rect
        };
        Some(inset(room, INSET))
    }

    pub(crate) fn scrollback_fit_for(&self, id: u32) -> Option<Fit> {
        self.scrollback_panel_for(id).map(chrome::scrollback_fit)
    }

    /// Where the open viewer goes, and how its text fits there.
    fn scrollback_place(&self) -> Option<(Rect, Fit)> {
        let panel = match self.overlays.scrollback.as_ref()?.pane() {
            Some(pane) => self.scrollback_panel_for(pane)?,
            None => inset(self.content_rect(), INSET),
        };
        Some((panel, chrome::scrollback_fit(panel)))
    }

    /// What the viewer shows, while it is open over a pane that shows.
    pub fn scrollback_view(&self) -> Option<ScrollbackView> {
        let viewer = self.overlays.scrollback.as_ref()?;
        let (panel, _) = self.scrollback_place()?;
        let first = viewer.first();
        let end = (first + chrome::scrollback_shown(panel)).min(viewer.row_count());
        // The keys listed show in place of the text.
        let (end, keys) = match viewer.keys_open() {
            true => (first, keys_sheet()),
            false => (end, Vec::new()),
        };
        let rows = (first..end)
            .map(|i| ScrollbackRow {
                text: viewer.row_text(i).to_owned(),
                selected: viewer.row_selection(i),
                found: viewer.row_found(i),
                word: viewer.row_word(i),
                caret: viewer.row_caret(i),
                styles: viewer.row_styles(i),
            })
            .collect();
        let note = match viewer.selected_lines() {
            Some(1) => "1 line selected".to_owned(),
            Some(n) => format!("{n} lines selected"),
            None if viewer.line_count() == 1 => "1 line".to_owned(),
            None => format!("{} lines", viewer.line_count()),
        };
        let (title, note) = match &viewer.source {
            Source::Pane { pane_id, .. }
                if self.overlays.scrollback_reading.as_ref() == Some(pane_id) =>
            {
                (
                    format!("Scrollback snapshot · {}", viewer.label),
                    "reading".to_owned(),
                )
            }
            Source::Pane { .. } => (format!("Scrollback snapshot · {}", viewer.label), note),
            Source::File { what } => (format!("{what} · {}", viewer.label), note),
            Source::SetupLog { project, slug, .. } => {
                let state = match self.setting_up(project, slug) {
                    true => "running",
                    false => "last run",
                };
                (
                    format!("Setup log · {}", viewer.label),
                    format!("{state} · {note}"),
                )
            }
        };
        Some(ScrollbackView {
            panel,
            title,
            note,
            rows,
            first,
            part: viewer.part(),
            overscroll: viewer.overscroll(),
            total: if keys.is_empty() {
                viewer.row_count()
            } else {
                0
            },
            hints: hints(viewer),
            hover: viewer.hover,
            search: viewer.find_bar(),
            ticks: if keys.is_empty() {
                viewer.find_ticks()
            } else {
                Vec::new()
            },
            keys,
        })
    }

    /// A key while the viewer is open, `typed` with Shift and the layout
    /// applied. `*` (`#` back) searches for the selection, or the word at
    /// the caret, while the search's field doesn't have the keyboard.
    pub fn scrollback_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let (_, fit) = self.scrollback_place()?;
        let star = match typed {
            Keysym::asterisk | Keysym::KP_Multiply => Some(false),
            Keysym::numbersign => Some(true),
            _ => None,
        };
        let others = mods.ctrl || mods.alt || mods.logo;
        let viewer = self.overlays.scrollback.as_ref()?;
        if let Some(back) = star.filter(|_| !others)
            && !viewer.find_focused()
            && !viewer.keys_open()
        {
            if !self.overlays.scrollback.as_mut()?.star(back, fit) {
                self.show_notice("No word at the caret: move it to one, or select text".into());
            }
            return None;
        }
        self.overlays
            .scrollback
            .as_mut()?
            .key(mods, sym, typed, Instant::now(), fit)
    }

    /// Do what the viewer asked for.
    pub fn apply_scrollback(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_scrollback(),
            Outcome::Copy { text, lines, close } => {
                self.copy_text(text);
                let s = if lines == 1 { "" } else { "s" };
                self.show_success(format!("Copied {lines} line{s}"));
                if close {
                    self.close_scrollback();
                }
            }
            Outcome::Reload => self.reload_scrollback(),
            Outcome::NothingSelected => {
                self.show_notice(
                    "Nothing selected: drag over the text, or press V and move the caret".into(),
                );
            }
        }
    }

    /// A left press while the viewer is open: on the text it starts a
    /// selection (a word on a double click, the line on a triple one, and
    /// with Shift the selection reaches there), the keyboard going to the
    /// text from the search; on the search, what its part does; outside
    /// the viewer it closes it, but on a toast presses that.
    pub fn scrollback_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.scrollback_view() else {
            return;
        };
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let hit = chrome::scrollback_hit(&view, &layout, pos);
        // The keys listed, a click on the viewer but on a hint puts the
        // text back.
        if !view.keys.is_empty() && matches!(hit, Some(h) if !matches!(h, ScrollbackHit::Hint(_))) {
            if let Some(viewer) = self.overlays.scrollback.as_mut() {
                viewer.show_keys(false);
            }
            return;
        }
        if !matches!(hit, Some(ScrollbackHit::Search(_)))
            && let Some(viewer) = self.overlays.scrollback.as_mut()
        {
            viewer.blur_find();
        }
        match hit {
            Some(ScrollbackHit::Search(part)) => {
                let Some((_, fit)) = self.scrollback_place() else {
                    return;
                };
                if let Some(viewer) = self.overlays.scrollback.as_mut() {
                    viewer.find_click(part, fit);
                }
            }
            Some(ScrollbackHit::Text) => {
                let (row, col) = chrome::scrollback_point(&view, &layout, pos);
                let (_, under) = chrome::scrollback_char(&view, &layout, pos);
                let extend = self.keyboard().modifier_state().shift;
                let Some(viewer) = self.overlays.scrollback.as_mut() else {
                    return;
                };
                let now = Instant::now();
                let count = match viewer.clicks {
                    Some((when, from, count))
                        if now.duration_since(when) < MULTI_CLICK
                            && (from.x - pos.x).abs() < MULTI_CLICK_SLOP
                            && (from.y - pos.y).abs() < MULTI_CLICK_SLOP =>
                    {
                        count % 3 + 1
                    }
                    _ => 1,
                };
                viewer.clicks = Some((now, pos, count));
                viewer.press(row, col, extend, count);
                if !extend {
                    viewer.caret_to(row, under);
                }
            }
            // A key hint: its key (the first of a choice), pressed.
            Some(ScrollbackHit::Hint(i)) => {
                let keys = &view.hints[i].0;
                let key = keys.split(" / ").next().unwrap_or(keys);
                self.press_hint(Overlay::Scrollback, key, pos);
            }
            Some(ScrollbackHit::Panel) => {}
            None => match self.toast_press(pos) {
                Some(ToastHit::Action(action)) => self.menu_action(action),
                Some(_) => {}
                None => self.close_scrollback(),
            },
        }
    }

    /// The pointer moved to `pos` holding a press on the text: the
    /// selection reaches the character there. Past the text's top or
    /// bottom, it reaches the row at that edge, which the list scrolls
    /// on under (see `scrollback_scroll_tick`).
    pub fn scrollback_drag(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.scrollback_view() else {
            return;
        };
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let text = layout.text;
        let (top, bottom) = (text.loc.y as f64, (text.loc.y + text.size.h) as f64 - 1.0);
        let at = Point::from((pos.x, pos.y.clamp(top, bottom)));
        let (row, col) = chrome::scrollback_point(&view, &layout, at);
        if let Some(viewer) = self.overlays.scrollback.as_mut() {
            viewer.drag_to(row, col);
        }
    }

    /// Whether a press on the viewer's text is held.
    pub fn scrollback_dragging(&self) -> bool {
        self.overlays
            .scrollback
            .as_ref()
            .is_some_and(Scrollback::dragging)
    }

    /// The left button went up.
    pub fn scrollback_release(&mut self) {
        if let Some(viewer) = self.overlays.scrollback.as_mut() {
            viewer.release();
        }
    }

    /// The pointer moved while the viewer is open: its key hints and the
    /// search's buttons light up, the buttons with a tooltip.
    pub fn scrollback_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.scrollback_view()?;
        let layout = chrome::scrollback_layout(view.panel, &view.hints);
        let hit = chrome::scrollback_hit(&view, &layout, pos);
        self.overlays.scrollback.as_mut()?.hover =
            hit.filter(|hit| matches!(hit, ScrollbackHit::Hint(_) | ScrollbackHit::Search(_)));
        let Some(ScrollbackHit::Search(part)) = hit else {
            return None;
        };
        let (parts, _) = chrome::scrollback_search(&view, &layout);
        let (_, rect) = parts.iter().find(|(p, _)| *p == part)?;
        let (text, key) = match part {
            SearchPart::Open => ("Find in the scrollback", "/"),
            SearchPart::Prev => ("The place found before", "Shift+Enter"),
            SearchPart::Next => ("The next place found", "Enter"),
            SearchPart::Close => ("Close the search", "Esc"),
            SearchPart::Field => return None,
        };
        Some(Tooltip::new(text, *rect).key(key))
    }

    /// Whether `pos` is on the viewer's text, where the pointer is a text
    /// cursor.
    pub fn on_scrollback_text(&self, pos: Point<f64, Logical>) -> bool {
        self.scrollback_view().is_some_and(|view| {
            if !view.keys.is_empty() {
                return false;
            }
            let layout = chrome::scrollback_layout(view.panel, &view.hints);
            chrome::scrollback_hit(&view, &layout, pos) == Some(ScrollbackHit::Text)
        })
    }

    /// The wheel or the touchpad moved the viewer's text, unless its keys
    /// are listed over it.
    pub fn scrollback_scroll(&mut self, motion: Motion) {
        let Some((_, fit)) = self.scrollback_place() else {
            return;
        };
        if let Some(viewer) = self.overlays.scrollback.as_mut().filter(|v| !v.keys_open()) {
            viewer.scroll(motion, Instant::now(), fit);
        }
    }

    /// Stand the viewer's text at row `pos`, as its scrollbar's thumb held
    /// there puts it.
    pub fn scrollback_scroll_to(&mut self, pos: f64) {
        let Some((_, fit)) = self.scrollback_place() else {
            return;
        };
        if let Some(viewer) = self.overlays.scrollback.as_mut() {
            viewer.scroll_to(pos, fit);
        }
    }

    /// Lay the viewer out again for where its pane is now and move its
    /// text on to `now`: on its own, and while a drag is held past its top
    /// or bottom, the selection going on with it. Its pane gone from view,
    /// it closes. Whether its text still moves.
    pub fn scrollback_scroll_tick(&mut self, now: Instant) -> bool {
        if self.overlays.scrollback.is_none() {
            return false;
        }
        let Some((panel, fit)) = self.scrollback_place() else {
            self.close_scrollback();
            return false;
        };
        let edge = if self.scrollback_dragging() {
            chrome::scrollback_edge(&chrome::scrollback_layout(panel, &[]), self.mouse.pos)
        } else {
            0.0
        };
        let Some(viewer) = self.overlays.scrollback.as_mut() else {
            return false;
        };
        viewer.fit(fit);
        let before = (viewer.first(), viewer.part());
        let moving = viewer.tick(now, fit, edge);
        // Moved at all, the last step of a glide too.
        if viewer.dragging() && (viewer.first(), viewer.part()) != before {
            self.scrollback_drag(self.mouse.pos);
        }
        moving
    }
}
