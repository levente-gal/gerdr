//! The chat in gerdr: flipping a pane between its chat and its screen,
//! the chat's keys, pointer and scrolling, and what it sends to the agent.
//!
//! A pane shows its chat while it was asked to (`PaneChat::shown`) and herdr
//! still finds an agent in it: one that quit leaves its shell, and the
//! screen shows again. The chats of the shown tab are followed by the tail
//! thread; hidden ones (Screen, or a tab out of sight) are not, and are
//! read again once shown (`chats_sync`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use smithay::{
    desktop::Window,
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use super::{ChatFocus, Event, Item, PaneChat, Reading, rows::rows, tail::Tail};
use crate::{
    ChatRows, Gerdr, Pane, Workspace,
    chrome::{self, ChatHit, ChatLayout, ChatView, FieldSpot, WaitKey},
    herdr::{AgentInfo, AgentSession, AgentStatus, Herdr, ProcessInfo},
    key_table::{self, Binding},
    md_blocks::{DocBlock, ItemMark},
    prepare::{self, Step},
    scroll::{Bounds, Motion, NOTCH_PX, Scroll},
    text_field::{self, TextField},
};

/// How many of a pane's last screen rows its waiting panel shows.
const WAITING_ROWS: usize = 14;
/// How often those rows are read again while its agent waits, at most.
const WAITING_EVERY: Duration = Duration::from_secs(1);

impl PaneChat {
    /// A chat asked for the first time: shown, following its bottom, with an
    /// empty reply of lines.
    pub(crate) fn opened() -> Box<Self> {
        Box::new(Self {
            shown: true,
            stick: true,
            draft: TextField::multiline(""),
            ..Default::default()
        })
    }
}

/// Show a pane's chat the first time, then flip it between chat and screen;
/// all else it holds is kept, so the next flip is instant.
fn flip(chat: &mut Option<Box<PaneChat>>) {
    match chat {
        Some(chat) => chat.shown = !chat.shown,
        None => *chat = Some(PaneChat::opened()),
    }
}

/// A key of the chat's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKey {
    Next,
    Previous,
    Fold,
    Top,
    Bottom,
    /// A page up (-1) or down (1).
    Page(i8),
    Copy,
    Compose,
}

/// A key of the chat's reply box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerKey {
    Send,
    Newline,
    /// Back to the list, the reply kept as a draft.
    Back,
    Discard,
}

/// The list's keys (`docs/ux-conventions.md`, the agent chat).
const LIST_KEYS: &[Binding<(), ListKey>] = &[
    Binding {
        chord: "J",
        does: |_| Some(ListKey::Next),
        footer: true,
        f1: "go to the next item",
    },
    Binding {
        chord: "Ctrl+J",
        does: |_| Some(ListKey::Next),
        footer: false,
        f1: "go to the next item",
    },
    key_table::alias("Down", |_| Some(ListKey::Next)),
    Binding {
        chord: "K",
        does: |_| Some(ListKey::Previous),
        footer: true,
        f1: "go to the previous item",
    },
    Binding {
        chord: "Ctrl+K",
        does: |_| Some(ListKey::Previous),
        footer: false,
        f1: "go to the previous item",
    },
    key_table::alias("Up", |_| Some(ListKey::Previous)),
    Binding {
        chord: "Space",
        does: |_| Some(ListKey::Fold),
        footer: true,
        f1: "fold or unfold the item",
    },
    Binding {
        chord: "Enter",
        does: |_| Some(ListKey::Fold),
        footer: false,
        f1: "fold or unfold the item",
    },
    Binding {
        chord: "G",
        does: |_| Some(ListKey::Top),
        footer: false,
        f1: "go to the first item",
    },
    key_table::alias("Home", |_| Some(ListKey::Top)),
    Binding {
        chord: "Shift+G",
        does: |_| Some(ListKey::Bottom),
        footer: false,
        f1: "go to the last item, following new ones",
    },
    key_table::alias("End", |_| Some(ListKey::Bottom)),
    Binding {
        chord: "PgUp",
        does: |_| Some(ListKey::Page(-1)),
        footer: false,
        f1: "scroll a page up",
    },
    Binding {
        chord: "PgDn",
        does: |_| Some(ListKey::Page(1)),
        footer: false,
        f1: "scroll a page down",
    },
    Binding {
        chord: "Y",
        does: |_| Some(ListKey::Copy),
        footer: true,
        f1: "copy the item",
    },
    Binding {
        chord: "Ctrl+C",
        does: |_| Some(ListKey::Copy),
        footer: false,
        f1: "copy the item",
    },
    Binding {
        chord: "I",
        does: |_| Some(ListKey::Compose),
        footer: true,
        f1: "write a reply (or type it at once)",
    },
];

/// The reply box's keys; the rest edit it as any text field.
const COMPOSER_KEYS: &[Binding<(), ComposerKey>] = &[
    Binding {
        chord: "Enter",
        does: |_| Some(ComposerKey::Send),
        footer: true,
        f1: "send the reply to the agent",
    },
    Binding {
        chord: "Shift+Enter",
        does: |_| Some(ComposerKey::Newline),
        footer: true,
        f1: "start a new line",
    },
    Binding {
        chord: "Esc",
        does: |_| Some(ComposerKey::Back),
        footer: true,
        f1: "go back to the list, the reply kept as a draft",
    },
    Binding {
        chord: text_field::DISCARD_KEY,
        does: |_| Some(ComposerKey::Discard),
        footer: true,
        f1: "discard the draft",
    },
];

/// The footer's key hints of a chat whose keys are at `focus` (checklist
/// K3: from the tables the keys are read with). Esc in the list does
/// nothing, so it has none; Esc in the reply box says what becomes of the
/// draft.
pub(crate) fn footer(focus: ChatFocus, draft_empty: bool) -> Vec<(String, String)> {
    match focus {
        ChatFocus::List => key_table::joined(key_table::hints(LIST_KEYS, &(), |k| {
            match k {
                ListKey::Next | ListKey::Previous => "move",
                ListKey::Fold => "fold",
                ListKey::Copy => "copy",
                ListKey::Compose => "reply",
                ListKey::Top | ListKey::Bottom | ListKey::Page(_) => "scroll",
            }
            .to_owned()
        })),
        ChatFocus::Composer => {
            let hints = key_table::hints(COMPOSER_KEYS, &(), |k| {
                match k {
                    ComposerKey::Send => "send",
                    ComposerKey::Newline => "new line",
                    ComposerKey::Back if draft_empty => "back",
                    ComposerKey::Back => "keep draft",
                    ComposerKey::Discard => "discard",
                }
                .to_owned()
            });
            let discard = |chord: &str| chord == text_field::DISCARD_KEY;
            (hints.into_iter())
                .filter(|(chord, _)| !(draft_empty && discard(chord)))
                .map(|(chord, said)| (chord.to_owned(), said))
                .collect()
        }
    }
}

/// What a key of the chat asks of gerdr, once the chat took it.
#[derive(Clone, Debug, PartialEq)]
enum ChatDo {
    /// Send the reply to the agent.
    Send,
    /// Give the clipboard this text.
    Copy(String),
    /// Scroll to the top, to the bottom (following new items again), or a
    /// page up or down.
    Top,
    Bottom,
    Page(i8),
}

/// Whether the key typed is a character, to write into the reply box.
fn types(mods: &ModifiersState, typed: Keysym) -> bool {
    !(mods.ctrl || mods.alt || mods.logo) && typed.key_char().is_some_and(|c| !c.is_control())
}

/// A key for `chat` while it has the keys: `sym` the key's own symbol,
/// `typed` what Shift and the layout made of it (checklist K8).
fn chat_key(
    chat: &mut PaneChat,
    mods: &ModifiersState,
    sym: Keysym,
    typed: Keysym,
) -> Option<ChatDo> {
    if chat.focus == ChatFocus::Composer {
        match key_table::press(COMPOSER_KEYS, &(), mods, sym) {
            Some(ComposerKey::Send) => return Some(ChatDo::Send),
            Some(ComposerKey::Newline) => _ = chat.draft.newline(),
            // Never saved nor thrown away (checklist K2).
            Some(ComposerKey::Back) => chat.focus = ChatFocus::List,
            Some(ComposerKey::Discard) => chat.draft.clear(),
            None => _ = chat.draft.key(mods, sym, typed),
        }
        return None;
    }
    // The keys move over the rows as they show: a fold of steps is one.
    let shown = rows(&chat.items, &chat.unfolded);
    let n = shown.len();
    let last = n.checked_sub(1);
    match key_table::press(LIST_KEYS, &(), mods, sym) {
        Some(ListKey::Next) => chat.picked = chat.picked.map_or(last, |i| Some((i + 1).min(n - 1))),
        Some(ListKey::Previous) => {
            chat.picked = chat.picked.map_or(last, |i| Some(i.saturating_sub(1)))
        }
        Some(ListKey::Fold) => {
            let row = chat.picked.and_then(|i| shown.get(i));
            if let Some(id) = row.and_then(|row| row.fold_key(&chat.items))
                && !chat.unfolded.remove(&id)
            {
                chat.unfolded.insert(id);
            }
        }
        Some(ListKey::Top) => {
            chat.picked = (n > 0).then_some(0);
            return Some(ChatDo::Top);
        }
        Some(ListKey::Bottom) => {
            chat.picked = last;
            return Some(ChatDo::Bottom);
        }
        Some(ListKey::Page(by)) => return Some(ChatDo::Page(by)),
        Some(ListKey::Copy) => {
            let row = chat.picked.or(last).and_then(|i| shown.get(i))?;
            let texts: Vec<String> = row.items(&chat.items).iter().map(item_text).collect();
            return Some(ChatDo::Copy(texts.join("\n\n")));
        }
        Some(ListKey::Compose) => chat.focus = ChatFocus::Composer,
        // Any character goes to the reply box, typed into it; a paste too.
        None if sym == text_field::PASTE || types(mods, typed) => {
            chat.focus = ChatFocus::Composer;
            chat.draft.key(mods, sym, typed);
        }
        None => {}
    }
    None
}

/// What copying `item` gives: what the user wrote, the agent's words, a
/// tool's input and output, an edit's change.
fn item_text(item: &Item) -> String {
    match item {
        Item::User { text, .. } | Item::Thinking { text, .. } | Item::Note { text, .. } => {
            text.clone()
        }
        Item::Assistant { blocks, .. } => blocks_text(blocks),
        Item::Tool {
            name,
            input,
            output,
            ..
        } => {
            let mut text = format!("{name}\n{input}");
            if let Some(out) = output {
                text.push_str("\n\n");
                text.push_str(&out.text);
            }
            text
        }
        Item::Edit { path, diff, .. } => {
            let mut text = path.clone();
            for line in diff {
                let mark = match line.side {
                    super::DiffSide::Context => ' ',
                    super::DiffSide::Added => '+',
                    super::DiffSide::Removed => '-',
                };
                text.push('\n');
                text.push(mark);
                text.push_str(&line.text);
            }
            text
        }
        Item::Subagent { title, items, .. } => std::iter::once(title.clone())
            .chain(items.iter().map(item_text))
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

/// The agent's words as markdown again, near enough: what the transcript
/// gave is parsed into blocks once, off the UI thread.
fn blocks_text(blocks: &[DocBlock]) -> String {
    let block = |b: &DocBlock| match b {
        DocBlock::Heading { level, text } => {
            format!("{} {}", "#".repeat(usize::from(*level)), text.text)
        }
        DocBlock::Para(text) => text.text.clone(),
        DocBlock::Item { depth, mark, text } => {
            let mark = match mark {
                ItemMark::Bullet => "- ".to_owned(),
                ItemMark::Number(n) => format!("{n}. "),
                ItemMark::Task(done) => format!("- [{}] ", if *done { 'x' } else { ' ' }),
                ItemMark::Under => "  ".to_owned(),
            };
            format!("{}{mark}{}", "  ".repeat(usize::from(*depth)), text.text)
        }
        DocBlock::Code(lines) => format!("```\n{}\n```", lines.join("\n")),
        DocBlock::Quote(text) => format!("> {}", text.text),
        DocBlock::Table(rows) => (rows.iter().map(|r| r.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        DocBlock::Rule => "---".to_owned(),
        DocBlock::Note(text) => text.clone(),
    };
    blocks.iter().map(block).collect::<Vec<_>>().join("\n\n")
}

/// The reply as one line: a line break typed into the pane would send it
/// there and then.
/// shortcut: a reply of lines goes as one; type it with bracketed paste per
/// agent when someone needs the line breaks.
fn one_line(draft: &str) -> String {
    draft.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a line typed into a pane, where herdr says `info` runs, goes to
/// its agent: with its shell in front, it would run as a command.
fn agent_in_front(info: &Result<ProcessInfo, String>) -> Result<(), String> {
    match prepare::step(info) {
        Step::Busy => Ok(()),
        Step::Drop => Err(match info {
            Err(err) => format!("herdr didn't say what runs in the pane: {err}"),
            Ok(_) => "its pane is gone".to_owned(),
        }),
        Step::Wait | Step::Send => Err("its pane's shell is in front, not the agent".to_owned()),
    }
}

/// herdr's name of `key`, for `pane.send_keys` (as the phone presses them).
fn wait_key_name(key: WaitKey) -> String {
    match key {
        WaitKey::Esc => "esc".into(),
        WaitKey::Up => "Up".into(),
        WaitKey::Down => "Down".into(),
        WaitKey::Enter => "Enter".into(),
        WaitKey::Digit(d) => d.to_string(),
    }
}

/// Which panes' chats to follow afresh and which to stop following: `wanted`
/// are those to follow now (shown, in sight), with their agents as herdr has
/// them. One already followed is followed again when herdr says its agent
/// holds another session (`/clear`, `/resume`, a new chat).
fn follow_plan<'a>(
    followed: &HashMap<String, Option<AgentSession>>,
    wanted: &'a [AgentInfo],
) -> (Vec<&'a AgentInfo>, Vec<String>) {
    let follow = (wanted.iter())
        .filter(|a| followed.get(&a.pane_id) != Some(&a.agent_session))
        .collect();
    let stop = (followed.keys())
        .filter(|id| !wanted.iter().any(|a| &a.pane_id == *id))
        .cloned()
        .collect();
    (follow, stop)
}

/// Where a chat stands: its first item, that item's row's top in its list,
/// and how far the list is scrolled.
struct Place {
    first: String,
    y: i32,
    top: f64,
}

/// Take `event` from the tail into its pane's chat, among `chats` (by herdr
/// pane id). Whether a chat took it: one closed meanwhile doesn't.
fn take_event<'a>(
    chats: impl IntoIterator<Item = (&'a str, &'a mut PaneChat)>,
    event: Event,
) -> bool {
    let pane_id = match &event {
        Event::Items { pane_id, .. }
        | Event::Missing { pane_id, .. }
        | Event::Failed { pane_id, .. } => pane_id.clone(),
    };
    let Some((_, chat)) = chats.into_iter().find(|(id, _)| *id == pane_id) else {
        return false;
    };
    match event {
        Event::Items {
            source,
            items,
            earlier,
            ..
        } => {
            let n = rows(&items, &chat.unfolded).len();
            chat.picked = chat.picked.filter(|i| *i < n);
            chat.items = items;
            chat.earlier = earlier;
            chat.reading_earlier = false;
            chat.source = Some(source);
            chat.reading = Reading::Read;
        }
        Event::Missing { why, .. } => chat.reading = Reading::Missing(why),
        Event::Failed { error, .. } => chat.reading = Reading::Failed(error),
    }
    true
}

/// The rows of a pane's screen worth showing: the last `n`, without the
/// empty ones at the bottom.
fn last_rows(mut rows: ChatRows, n: usize) -> ChatRows {
    while rows.last().is_some_and(|(text, _)| text.trim().is_empty()) {
        rows.pop();
    }
    let cut = rows.len().saturating_sub(n);
    rows.split_off(cut)
}

/// Pane `pane_id`'s last screen rows with their colours, read from herdr
/// off the UI thread.
fn screen_rows(herdr: &mut Herdr, pane_id: &str) -> Result<ChatRows, String> {
    let text = herdr.pane_styled(pane_id, WAITING_ROWS)?;
    // An app on the alternate screen has no scrollback: its screen, plainly.
    let text = match text.trim().is_empty() {
        true => herdr.pane_read(pane_id, "visible", WAITING_ROWS)?,
        false => text,
    };
    Ok(last_rows(
        crate::scrollback::parse_ansi(&text),
        WAITING_ROWS,
    ))
}

/// How a chat's list scrolls: in pixels, from its top to as far as its
/// items are higher than it.
fn chat_bounds(layout: &ChatLayout) -> Bounds {
    let view = f64::from(layout.list.size.h);
    Bounds {
        max: (f64::from(layout.content) - view).max(0.0),
        notch: NOTCH_PX,
        unit: 1.0,
        next: 1.0,
        prev: 1.0,
        view,
    }
}

/// The chat of herdr pane `pane_id`, among `workspaces`' panes and those
/// whose window is still to come.
pub(crate) fn chat_mut<'a>(workspaces: &'a mut [Workspace], pane_id: &str) -> Option<&'a mut PaneChat> {
    chats_mut(workspaces).find_map(|(id, chat)| (id == pane_id).then_some(chat))
}

/// Every chat of `workspaces`, by herdr pane id.
fn chats_mut(workspaces: &mut [Workspace]) -> impl Iterator<Item = (&str, &mut PaneChat)> {
    workspaces
        .iter_mut()
        .flat_map(|ws| &mut ws.tabs)
        .flat_map(|tab| {
            let panes = (tab.panes.iter_mut())
                .filter_map(|p| Some((p.herdr.as_ref()?.pane_id.as_str(), p.chat.as_deref_mut()?)));
            let expected = (tab.expected.iter_mut())
                .filter_map(|e| Some((e.herdr.as_ref()?.pane_id.as_str(), e.chat.as_deref_mut()?)));
            panes.chain(expected)
        })
}

impl Gerdr {
    /// herdr's agent in pane `pane_id`, if it found one there.
    pub(crate) fn chat_agent(&self, pane_id: &str) -> Option<&AgentInfo> {
        self.session.agents.iter().find(|a| a.pane_id == pane_id)
    }

    /// Whether `pane` shows its chat in its place: it was asked to, and herdr
    /// still finds its agent. One that quit leaves its shell: the screen.
    pub(crate) fn chat_shown(&self, pane: &Pane) -> bool {
        pane.chat.as_ref().is_some_and(|c| c.shown)
            && (pane.herdr.as_ref()).is_some_and(|h| self.chat_agent(&h.pane_id).is_some())
    }

    /// Whether `window`'s pane shows its chat in its place.
    pub(crate) fn window_shows_chat(&self, window: &Window) -> bool {
        (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .flat_map(|tab| &tab.panes)
            .find(|p| &p.window == window)
            .is_some_and(|p| self.chat_shown(p))
    }

    /// Show the focused agent pane as a chat, or its screen again.
    pub(crate) fn toggle_chat(&mut self) {
        match self.tab().and_then(|tab| tab.focused_id()) {
            Some(id) => self.toggle_pane_chat(id),
            None => self.show_notice("No agent runs in this pane".into()),
        }
    }

    /// Show pane `id` of the active tab as its agent's chat, or its screen
    /// again (the prefix key, its bar's button, its bar's menu). Hiding the
    /// chat never stops the agent. A pane herdr finds no agent in has no
    /// chat: nothing changes.
    pub(crate) fn toggle_pane_chat(&mut self, id: u32) {
        let pane_id = (self.tab())
            .and_then(|tab| tab.herdr_pane(id))
            .map(|p| p.pane_id.clone());
        if !pane_id.is_some_and(|p| self.chat_agent(&p).is_some()) {
            return self.show_notice("No agent runs in this pane".into());
        }
        let (w, t) = match self.workspace() {
            Some(ws) => (self.active_workspace, ws.active_tab),
            None => return,
        };
        let Some(pane) = (self.workspaces[w].tabs[t].panes.iter_mut()).find(|p| p.id == id) else {
            return;
        };
        flip(&mut pane.chat);
        // Its window off screen or back, the chat followed or not.
        self.arrange();
        // The keys go to the chat, or to the window again.
        self.focus_pane(|_| Some(id));
    }

    /// The chats shown and followed brought in line with what shows and with
    /// herdr's agents, after a round of them: a chat whose agent came or
    /// went shows or hides.
    pub(crate) fn chats_refresh(&mut self) {
        let shown: HashSet<String> = (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .flat_map(|tab| &tab.panes)
            .filter(|p| self.chat_shown(p))
            .filter_map(|p| Some(p.herdr.as_ref()?.pane_id.clone()))
            .collect();
        if shown == self.chats.shown {
            return self.chats_sync();
        }
        // `arrange` syncs them.
        self.arrange();
        // The keys go to the chat that came, or to the window again.
        if let Some(window) = self.tab().and_then(|tab| tab.focused.clone()) {
            self.focus(Some(window));
        }
    }

    /// Follow the chats shown in sight, and only those (checklist C6: a
    /// closed pane's, a hidden one's, one in a tab out of sight stop), and
    /// read the screens of those whose agent waits on the user.
    pub(crate) fn chats_sync(&mut self) {
        let mut shown = HashSet::new();
        let mut wanted = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for pane in tab.panes.iter().filter(|p| self.chat_shown(p)) {
                    let Some(agent) =
                        (pane.herdr.as_ref()).and_then(|h| self.chat_agent(&h.pane_id))
                    else {
                        continue;
                    };
                    shown.insert(agent.pane_id.clone());
                    if self.is_visible(w, t) {
                        wanted.push(self.with_live_session(agent));
                    }
                }
            }
        }
        if !shown.is_empty() {
            self.install_live_hooks();
        }
        self.chats.shown = shown;
        let (follow, stop) = follow_plan(&self.chats.followed, &wanted);
        let follow: Vec<AgentInfo> = follow.into_iter().cloned().collect();
        if !(follow.is_empty() && stop.is_empty())
            && self.start_chat_tail()
            && let Some(tail) = self.chats.tail.as_ref()
        {
            for pane_id in stop {
                tail.stop(&pane_id);
                self.chats.followed.remove(&pane_id);
            }
            for agent in follow {
                (self.chats.followed).insert(agent.pane_id.clone(), agent.agent_session.clone());
                tail.follow(agent);
            }
        }
        // What is kept of chats gone with their panes.
        let kept: HashSet<String> = chats_mut(&mut self.workspaces)
            .map(|(id, _)| id.to_owned())
            .collect();
        self.chats.scrolls.retain(|id, _| kept.contains(id));
        self.chats.read_at.retain(|id, _| kept.contains(id));
        self.chats_read_rows(&wanted);
        self.live_release_hidden();
    }

    /// Start the thread following the chats, the first time one shows;
    /// whether it runs.
    fn start_chat_tail(&mut self) -> bool {
        if self.chats.tail.is_none() {
            let (tx, rx) = channel::channel::<Event>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.chat_event(event);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent chats: {err}");
                self.show_error("Couldn't start reading the agents' chats".into());
                return false;
            }
            let socket = (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf());
            self.chats.tail = Some(Tail::spawn(tx, socket));
        }
        true
    }

    /// What the tail read of a pane's chat, or why it couldn't.
    fn chat_event(&mut self, event: Event) {
        // The chat says it too, in its place (checklist E7).
        if let Event::Failed { pane_id, error } = &event {
            tracing::warn!("the chat of pane {pane_id}: {error}");
        }
        let items_of = match &event {
            Event::Items { pane_id, .. } => Some(pane_id.clone()),
            _ => None,
        };
        let place = items_of.as_deref().and_then(|p| self.chat_place_of(p));
        // A pane closed meanwhile: nothing waits for it.
        take_event(chats_mut(&mut self.workspaces), event);
        if let (Some(pane_id), Some(place)) = (items_of, place) {
            self.chat_keep_place(&pane_id, place);
        }
    }

    /// Where pane `pane_id`'s chat stands, to stay there as earlier
    /// messages come in above.
    fn chat_place_of(&self, pane_id: &str) -> Option<Place> {
        let (_, _, view) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)?;
        let first = view.items.first()?.id().to_owned();
        let (y, _) = chrome::chat_item_span(&view, 0)?;
        Some(Place {
            first,
            y,
            top: view.top,
        })
    }

    /// Earlier messages came in above pane `pane_id`'s chat, which stood
    /// at `place`: the same messages stay in view, and the same one picked.
    fn chat_keep_place(&mut self, pane_id: &str, place: Place) {
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)
        else {
            return;
        };
        let Some(i) = (view.items.iter()).position(|item| item.id() == place.first) else {
            return;
        };
        let shown = rows(&view.items, &view.unfolded);
        let Some(r) = shown.iter().position(|row| row.holds(i)) else {
            return;
        };
        let Some((y, _)) = chrome::chat_item_span(&view, r).filter(|(y, _)| *y != place.y) else {
            return;
        };
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        chat.picked = chat
            .picked
            .map(|p| (p + r).min(shown.len().saturating_sub(1)));
        if !chat.stick {
            self.chat_scroll_to(pane_id, Some(place.top + f64::from(y - place.y)));
        }
    }

    /// Ask for the messages before those read of pane `pane_id`'s chat,
    /// once until they come.
    fn chat_earlier(&mut self, pane_id: &str) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        if !chat.earlier || chat.reading_earlier {
            return;
        }
        if let Some(tail) = self.chats.tail.as_ref() {
            chat.reading_earlier = true;
            tail.earlier(pane_id);
        }
    }

    /// Read the screens of the chats in `wanted` whose agent waits on the
    /// user, once a `WAITING_EVERY` at most; the others show none.
    fn chats_read_rows(&mut self, wanted: &[AgentInfo]) {
        let blocked: HashSet<&str> = (wanted.iter())
            .filter(|a| a.agent_status == AgentStatus::Blocked)
            .map(|a| a.pane_id.as_str())
            .collect();
        for (pane_id, chat) in chats_mut(&mut self.workspaces) {
            if !blocked.contains(pane_id) {
                chat.waiting = None;
            }
        }
        self.chats
            .waiting
            .retain(|id, _| blocked.contains(id.as_str()));
        let due: Vec<String> = (blocked.into_iter())
            .filter(|id| !self.chats.reading.contains(*id))
            .filter(|id| {
                (self.chats.read_at.get(*id)).is_none_or(|at| at.elapsed() >= WAITING_EVERY)
            })
            .map(str::to_owned)
            .collect();
        for pane_id in due {
            self.read_rows(pane_id);
        }
    }

    /// Read pane `pane_id`'s screen rows on a thread of its own.
    fn read_rows(&mut self, pane_id: String) {
        let Some(socket) = (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf()) else {
            return;
        };
        let Some(tx) = self.rows_sender() else {
            return;
        };
        self.chats.reading.insert(pane_id.clone());
        self.chats.read_at.insert(pane_id.clone(), Instant::now());
        let id = pane_id.clone();
        let spawned = std::thread::Builder::new()
            .name("chat rows".into())
            .spawn(move || {
                let rows = screen_rows(&mut Herdr::on_socket(socket), &id);
                // The loop is gone only as gerdr quits.
                let _ = tx.send((id, rows));
            });
        if let Err(err) = spawned {
            tracing::warn!("reading the screen of pane {pane_id}: {err}");
            self.chats.reading.remove(&pane_id);
        }
    }

    /// Where the reads of screen rows come back; set up the first time.
    fn rows_sender(&mut self) -> Option<channel::Sender<(String, Result<ChatRows, String>)>> {
        if self.chats.rows_tx.is_none() {
            let (tx, rx) = channel::channel();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((pane_id, rows)) = event {
                    state.take_rows(pane_id, rows);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent chats' screens: {err}");
                return None;
            }
            self.chats.rows_tx = Some(tx);
        }
        self.chats.rows_tx.clone()
    }

    /// Pane `pane_id`'s screen rows as read, for its waiting panel.
    fn take_rows(&mut self, pane_id: String, rows: Result<ChatRows, String>) {
        self.chats.reading.remove(&pane_id);
        // Too late: its agent waits no more, or its chat is gone.
        if !(self.chat_agent(&pane_id)).is_some_and(|a| a.agent_status == AgentStatus::Blocked) {
            return;
        }
        let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
            return;
        };
        match rows {
            Ok(rows) if chat.waiting.as_ref() != Some(&rows) => {
                self.chats.waiting.insert(pane_id, Arc::new(rows.clone()));
                chat.waiting = Some(rows);
            }
            Ok(_) => {}
            // Read again on herdr's next round; a toast a second would
            // drown the rest. The panel keeps the rows it had.
            Err(err) => tracing::warn!("the screen of pane {pane_id}: {err}"),
        }
    }

    /// The focused pane of the active tab while it shows its chat in sight.
    fn focused_chat(&self) -> Option<&Pane> {
        let tab = self.tab()?;
        if tab.review().is_some_and(|review| review.page.is_none()) {
            return None;
        }
        let id = tab.focused_id()?;
        (tab.panes.iter()).find(|p| p.id == id && self.chat_shown(p))
    }

    /// Whether the keys go to the focused pane's chat: it shows one, and
    /// nothing over it takes them. The prefix works as everywhere.
    pub(crate) fn chat_has_keys(&self) -> bool {
        self.overlays.rename.is_none()
            && self.grabbing_overlay().is_none()
            && self.key_overlay().is_none()
            && !self.is_open(crate::overlay::Overlay::Features)
            && self.focused_chat().is_some()
    }

    /// The herdr pane of the focused chat, and the chat.
    fn focused_chat_mut(&mut self) -> Option<(String, &mut PaneChat)> {
        let pane_id = self.focused_chat()?.herdr.as_ref()?.pane_id.clone();
        let chat = chat_mut(&mut self.workspaces, &pane_id)?;
        Some((pane_id, chat))
    }

    /// The reply box of the focused chat, while it has the keys.
    pub(crate) fn chat_field_mut(&mut self) -> Option<&mut TextField> {
        let (_, chat) = self.focused_chat_mut()?;
        (chat.focus == ChatFocus::Composer).then_some(&mut chat.draft)
    }

    /// Where the focused chat's reply box is, while it has the keys.
    pub(crate) fn chat_field_spot(&self) -> Option<FieldSpot> {
        let (_, _, view) = self.chat_places().into_iter().find(|(_, _, v)| v.focused)?;
        (view.focus == ChatFocus::Composer)
            .then(|| chrome::chat_field_spot(&view, self.scale()))
            .flatten()
    }

    /// A key for the focused chat (see `chat_key`).
    pub(crate) fn chat_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) {
        let Some((pane_id, chat)) = self.focused_chat_mut() else {
            return;
        };
        let picked = chat.picked;
        match chat_key(chat, mods, sym, typed) {
            None => self.chat_show_picked(&pane_id, picked),
            Some(ChatDo::Send) => self.chat_send(&pane_id),
            Some(ChatDo::Copy(text)) => {
                let lines = text.lines().count().max(1);
                let s = if lines == 1 { "" } else { "s" };
                self.copy_text(text);
                self.show_success(format!("Copied {lines} line{s}"));
            }
            Some(ChatDo::Top) => self.chat_scroll_to(&pane_id, Some(0.0)),
            Some(ChatDo::Bottom) => self.chat_scroll_to(&pane_id, None),
            Some(ChatDo::Page(by)) => {
                let page = (self.chat_places().into_iter())
                    .find(|(_, p, _)| *p == pane_id)
                    .map(|(_, _, view)| chrome::chat_layout(&view));
                if let Some(layout) = page {
                    let b = chat_bounds(&layout);
                    let scroll = (self.chats.scrolls.entry(pane_id.clone())).or_default();
                    let to = scroll.pos() + f64::from(by) * b.view;
                    scroll.glide_to(to, Instant::now(), b);
                    if let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) {
                        chat.stick = false;
                    }
                }
            }
        }
    }

    /// Stand pane `pane_id`'s chat at `top` pixels down, or at its bottom
    /// following new items (`None`).
    fn chat_scroll_to(&mut self, pane_id: &str, top: Option<f64>) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        chat.stick = top.is_none();
        if let Some(top) = top {
            chat.top = top;
            (self.chats.scrolls.entry(pane_id.to_owned()).or_default()).jump(top);
        }
    }

    /// Send the focused chat's reply to its agent, as if typed in its pane.
    /// A reply refused keeps its draft.
    fn chat_send(&mut self, pane_id: &str) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        let line = one_line(&chat.draft);
        if line.is_empty() {
            return;
        }
        match self.send_to_agent(pane_id, &line) {
            Ok(()) => {
                if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
                    chat.draft.clear();
                    chat.stick = true;
                }
            }
            Err(why) => {
                tracing::warn!("a reply to the agent in pane {pane_id}: {why}");
                self.show_error(format!("Couldn't send the reply: {why}"));
            }
        }
    }

    /// Type `line` into herdr pane `pane_id`, then Enter, if its agent is in
    /// front: never into its shell, where it would run as a command.
    fn send_to_agent(&mut self, pane_id: &str, line: &str) -> Result<(), String> {
        if self.pane_paused(pane_id) {
            return Err("the agent is paused; resume it first".into());
        }
        let herdr = (self.session.herdr.as_mut()).ok_or("herdr isn't connected")?;
        agent_in_front(&herdr.process_info(pane_id))?;
        herdr.send_agent_line(pane_id, line)
    }

    /// Press `key` in pane `pane_id`, from its waiting panel's button: a
    /// digit typed as a menu's choice, without Enter.
    fn chat_wait_key(&mut self, pane_id: &str, key: WaitKey) {
        if self.pane_paused(pane_id) {
            return self.show_error("Couldn't press the key: the agent is paused".into());
        }
        let Some(herdr) = self.session.herdr.as_mut() else {
            return self.show_error("Couldn't press the key: herdr isn't connected".into());
        };
        let name = wait_key_name(key);
        let sent = match key {
            WaitKey::Digit(_) => herdr
                .call(
                    "pane.send_input",
                    json!({ "pane_id": pane_id, "text": name, "keys": [] }),
                )
                .map(drop),
            _ => herdr.send_keys(pane_id, &[&name]),
        };
        match sent {
            // Its screen read again soon, as the menu moves.
            Ok(()) => _ = self.chats.read_at.remove(pane_id),
            Err(err) => {
                tracing::warn!("pressing {name} in pane {pane_id}: {err}");
                self.show_error(format!("Couldn't press {name} in the agent's pane: {err}"));
            }
        }
    }

    /// Open the file edit `i` of pane `pane_id`'s chat changed, in the
    /// workspace's Zed.
    fn chat_open_path(&mut self, pane_id: &str, i: usize) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        let shown = rows(&chat.items, &chat.unfolded);
        let Some(Item::Edit { path, .. }) = shown.get(i).and_then(|row| row.item(&chat.items))
        else {
            return;
        };
        let mut file = PathBuf::from(path);
        if file.is_relative()
            && let Some(cwd) = self.chat_agent(pane_id).and_then(|a| a.cwd.clone())
        {
            file = cwd.join(file);
        }
        self.open_in_zed(&file, crate::launch::Place::Tab, "the file");
    }

    /// The active tab's panes shown as chats: each pane's id, its herdr
    /// pane's, and what its chat is painted from.
    fn chat_places(&self) -> Vec<(u32, String, ChatView)> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        if tab.review().is_some_and(|review| review.page.is_none()) {
            return Vec::new();
        }
        let focused = tab.focused_id().filter(|_| self.chat_has_keys());
        let pointer = Some(self.mouse.pos).filter(|pos| self.overlay_at(*pos).is_none());
        (tab.shown_rects(self.content_rect()).into_iter())
            .filter_map(|(id, rect)| {
                let pane = (tab.panes.iter()).find(|p| p.id == id && self.chat_shown(p))?;
                let pane_id = &pane.herdr.as_ref()?.pane_id;
                let agent = self.chat_agent(pane_id)?;
                let chat = pane.chat.as_ref()?;
                // The agent and its folder; herdr named no chat when it was
                // found by folder and time: it may be the wrong one.
                let mut label = agent.label().to_owned();
                if let Some(name) = (agent.cwd.as_deref())
                    .and_then(|cwd| cwd.file_name())
                    .and_then(|name| name.to_str())
                {
                    label = format!("{label} · {name}");
                }
                if chat.source.as_ref().is_some_and(|source| source.guessed) {
                    label.push_str(" · found by folder and time");
                }
                let mut view = ChatView {
                    rect: tab.window_rect(id, rect),
                    label,
                    cwd: agent.cwd.clone(),
                    status: agent.agent_status,
                    paused: self.pane_paused(pane_id),
                    reading: chat.reading.clone(),
                    items: chat.items.clone(),
                    unfolded: chat.unfolded.clone(),
                    picked: chat.picked,
                    top: chat.top,
                    focus: chat.focus,
                    focused: focused == Some(id),
                    draft: chat.draft.line(),
                    waiting: self.chats.waiting.get(pane_id).cloned(),
                    hints: footer(chat.focus, chat.draft.line().text.is_empty()),
                    earlier: match (chat.earlier, chat.reading_earlier) {
                        (false, _) => None,
                        (true, reading) => Some(reading),
                    },
                    hover: None,
                };
                view.hover = pointer
                    .filter(|pos| view.rect.to_f64().contains(*pos))
                    .and_then(|pos| chrome::chat_layout(&view).hit(&view, pos));
                Some((id, pane_id.clone(), view))
            })
            .collect()
    }

    /// What the chats of the active tab are painted from.
    pub(crate) fn chat_views(&self) -> Vec<ChatView> {
        (self.chat_places().into_iter())
            .map(|(_, _, view)| view)
            .collect()
    }

    /// The chat at `pos`, if one is painted there.
    fn chat_place_at(&self, pos: Point<f64, Logical>) -> Option<(u32, String, ChatView)> {
        (self.chat_places().into_iter()).find(|(_, _, view)| view.rect.to_f64().contains(pos))
    }

    /// `button` went down at `pos` over a chat: its pane takes the focus, and
    /// a left press (only: checklist O2) does what is under it. Whether a
    /// chat was there. A press in the reply box gives it the keys; the caret
    /// is then placed by `field` (the text fields' own press).
    pub(crate) fn chat_press(
        &mut self,
        pos: Point<f64, Logical>,
        button: u32,
        field: impl FnOnce(&mut Self),
    ) -> bool {
        let Some((id, pane_id, view)) = self.chat_place_at(pos) else {
            return false;
        };
        self.focus_pane(|_| Some(id));
        if button != crate::input::BTN_LEFT {
            return true;
        }
        // A key hint does what its key does (checklist K3).
        if let Some(i) = chrome::chat_hint_at(&view, pos) {
            let keys = view.hints[i].0.clone();
            self.press_hint(crate::input::Overlay::Chat, &keys, pos);
            return true;
        }
        let Some(hit) = chrome::chat_layout(&view).hit(&view, pos) else {
            return true;
        };
        let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
            return true;
        };
        match hit {
            ChatHit::Item(i) => {
                chat.picked = Some(i);
                chat.focus = ChatFocus::List;
            }
            ChatHit::Fold(i) => {
                chat.picked = Some(i);
                let row = rows(&chat.items, &chat.unfolded).get(i).cloned();
                if let Some(id) = row.and_then(|row| row.fold_key(&chat.items))
                    && !chat.unfolded.remove(&id)
                {
                    chat.unfolded.insert(id);
                }
            }
            ChatHit::ShowAll(i) => {
                chat.picked = Some(i);
                let shown = rows(&chat.items, &chat.unfolded);
                if let Some(id) = shown.get(i).and_then(|row| row.fold_key(&chat.items)) {
                    chat.unfolded.insert(chrome::whole_key(&id));
                    chat.unfolded.insert(id);
                }
            }
            ChatHit::Path(i) => self.chat_open_path(&pane_id, i),
            ChatHit::Earlier => self.chat_earlier(&pane_id),
            ChatHit::Key(key) => self.chat_wait_key(&pane_id, key),
            ChatHit::Composer => {
                chat.focus = ChatFocus::Composer;
                field(self);
            }
            ChatHit::Send => self.chat_send(&pane_id),
            ChatHit::ShowScreen => self.toggle_pane_chat(id),
            ChatHit::Scrollbar => {
                self.chats.bar_drag = Some(pane_id);
                self.chat_drag(pos);
            }
        }
        true
    }

    /// The pointer moved to `pos` holding a chat's scrollbar: its list
    /// stands where the pointer is along it.
    pub(crate) fn chat_drag(&mut self, pos: Point<f64, Logical>) {
        let Some(pane_id) = self.chats.bar_drag.clone() else {
            return;
        };
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| *p == pane_id)
        else {
            self.chats.bar_drag = None;
            return;
        };
        let layout = chrome::chat_layout(&view);
        let b = chat_bounds(&layout);
        let list = layout.list;
        let along = (pos.y - f64::from(list.loc.y)) / f64::from(list.size.h.max(1));
        let top = along.clamp(0.0, 1.0) * b.max;
        self.chat_scroll_to(&pane_id, (top < b.max).then_some(top));
    }

    /// The list of pane `pane_id`'s chat scrolled for its picked item to
    /// show, when the keys moved it from `was`.
    fn chat_show_picked(&mut self, pane_id: &str, was: Option<usize>) {
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)
        else {
            return;
        };
        let Some(i) = view.picked.filter(|_| view.picked != was) else {
            return;
        };
        let Some((y, h)) = chrome::chat_item_span(&view, i) else {
            return;
        };
        let (y, h, shown) = (
            f64::from(y),
            f64::from(h),
            f64::from(chrome::chat_layout(&view).list.size.h),
        );
        let to = match () {
            _ if y < view.top => y,
            _ if y + h > view.top + shown => (y + h - shown).min(y),
            _ => return,
        };
        self.chat_scroll_to(pane_id, Some(to));
    }

    /// The wheel or fingers moved over a chat at `pos`: its list moves, and
    /// stops following new items once moved up. Whether a chat was there.
    pub(crate) fn chat_scroll(&mut self, pos: Point<f64, Logical>, motion: Option<Motion>) -> bool {
        let Some((_, pane_id, view)) = self.chat_place_at(pos) else {
            return false;
        };
        let Some(motion) = motion else {
            return true;
        };
        let b = chat_bounds(&chrome::chat_layout(&view));
        let scroll =
            (self.chats.scrolls.entry(pane_id.clone())).or_insert_with(|| Scroll::at(view.top));
        scroll.apply(motion, Instant::now(), b);
        let up = match motion {
            Motion::Wheel(n) | Motion::Notches(n) => n < 0.0,
            Motion::Finger { px, .. } => px < 0.0,
            Motion::Lift { .. } => false,
        };
        if up && let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) {
            chat.stick = false;
        }
        true
    }

    /// Move each chat shown on to `now`: one following new items stands at
    /// its bottom; one scrolled back to its bottom follows them again.
    pub(crate) fn chat_scroll_tick(&mut self, now: Instant) {
        let mut at_top = Vec::new();
        for (_, pane_id, view) in self.chat_places() {
            let b = chat_bounds(&chrome::chat_layout(&view));
            let scroll =
                (self.chats.scrolls.entry(pane_id.clone())).or_insert_with(|| Scroll::at(view.top));
            let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
                continue;
            };
            if chat.stick && !scroll.moving(now) {
                scroll.jump(b.max);
            }
            let moving = scroll.tick(now, b);
            if !moving && b.max > 0.0 && scroll.pos() >= b.max - 0.5 {
                chat.stick = true;
            }
            chat.top = scroll.pos().clamp(0.0, b.max);
            // Scrolled up to the first message read: read the ones before.
            if !chat.stick && chat.top < 1.0 && chat.earlier && !chat.reading_earlier {
                at_top.push(pane_id);
            }
        }
        for pane_id in at_top {
            self.chat_earlier(&pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{DiffLine, DiffSide, Source, ToolOut};
    use crate::herdr::Process;

    fn agent(pane: &str, session: Option<&str>) -> AgentInfo {
        let session = session.map(|id| {
            format!(r#","agent_session":{{"source":"herdr:claude","agent":"claude","kind":"id","value":"{id}"}}"#)
        });
        serde_json::from_str(&format!(
            r#"{{"pane_id":"{pane}","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t1","agent":"claude","display_agent":null,"name":null,"cwd":null,"foreground_cwd":null,"title":null,"terminal_title":null,"terminal_title_stripped":null{}}}"#,
            session.unwrap_or_default()
        ))
        .unwrap()
    }

    /// A key as input sends it: `sym` the key's own, `typed` what Shift made
    /// of it (checklist K8).
    fn press(chat: &mut PaneChat, keys: &str, typed: char) -> Option<ChatDo> {
        let (mods, sym) = crate::keys::hint_key(keys).unwrap();
        chat_key(chat, &mods, sym, Keysym::from_char(typed))
    }

    fn user(id: &str, text: &str) -> Item {
        Item::User {
            id: id.into(),
            at: None,
            text: text.into(),
        }
    }

    fn with_items(items: Vec<Item>) -> PaneChat {
        let mut chat = *PaneChat::opened();
        chat.items = Arc::new(items);
        chat
    }

    #[test]
    fn the_first_toggle_shows_the_chat_and_the_next_flip_keeps_it() {
        let mut chat = None;
        flip(&mut chat);
        let shown = chat.as_mut().unwrap();
        assert!(shown.shown && shown.stick);
        assert_eq!(shown.focus, ChatFocus::List);
        shown.draft.set("half a reply");
        shown.unfolded.insert("t1".into());
        shown.top = 120.0;
        flip(&mut chat);
        let hidden = chat.as_ref().unwrap();
        assert!(!hidden.shown);
        assert_eq!(hidden.draft, "half a reply");
        assert!(hidden.unfolded.contains("t1"));
        assert_eq!(hidden.top, 120.0);
        flip(&mut chat);
        assert!(chat.unwrap().shown);
    }

    #[test]
    fn shown_chats_are_followed_hidden_and_closed_ones_stopped() {
        let followed = HashMap::new();
        let wanted = [agent("w1:p1", Some("a"))];
        let (follow, stop) = follow_plan(&followed, &wanted);
        assert_eq!(follow.len(), 1);
        assert!(stop.is_empty());
        let followed = HashMap::from([("w1:p1".to_owned(), wanted[0].agent_session.clone())]);
        assert_eq!(follow_plan(&followed, &wanted), (vec![], vec![]));
        // Another session in the pane: found and read afresh.
        let again = [agent("w1:p1", Some("b"))];
        assert_eq!(follow_plan(&followed, &again).0.len(), 1);
        // Closed, hidden or out of sight: no longer wanted.
        assert_eq!(
            follow_plan(&followed, &[]),
            (vec![], vec!["w1:p1".to_owned()])
        );
    }

    #[test]
    fn what_the_tail_reads_goes_to_its_own_pane() {
        let (mut one, mut two) = (*PaneChat::opened(), *PaneChat::opened());
        one.picked = Some(4);
        let source = Source {
            agent: crate::ai_work::AgentKind::Claude,
            id: "abc".into(),
            path: "/x.jsonl".into(),
            guessed: false,
        };
        let event = Event::Items {
            pane_id: "w1:p2".into(),
            source: source.clone(),
            items: Arc::new(vec![user("u1", "hi")]),
            earlier: true,
        };
        assert!(take_event(
            [("w1:p1", &mut one), ("w1:p2", &mut two)],
            event
        ));
        assert_eq!(two.reading, Reading::Read);
        assert_eq!(two.items.len(), 1);
        assert!(two.earlier && !two.reading_earlier);
        assert_eq!(two.source, Some(source));
        assert_eq!(one.reading, Reading::Finding);
        let missing = Event::Missing {
            pane_id: "w1:p1".into(),
            why: "no chat".into(),
        };
        assert!(take_event([("w1:p1", &mut one)], missing));
        assert_eq!(one.reading, Reading::Missing("no chat".into()));
        let gone = Event::Failed {
            pane_id: "w1:p9".into(),
            error: "x".into(),
        };
        assert!(!take_event([("w1:p1", &mut one)], gone));
    }

    #[test]
    fn list_keys_move_fold_copy_and_type_into_the_reply() {
        let mut chat = with_items(vec![user("u1", "one"), user("u2", "two")]);
        assert_eq!(press(&mut chat, "K", 'k'), None);
        assert_eq!(chat.picked, Some(1));
        press(&mut chat, "Ctrl+K", 'k');
        assert_eq!(chat.picked, Some(0));
        press(&mut chat, "J", 'j');
        assert_eq!(chat.picked, Some(1));
        press(&mut chat, "Space", ' ');
        assert!(chat.unfolded.contains("u2"));
        press(&mut chat, "Enter", '\r');
        assert!(chat.unfolded.is_empty());
        assert_eq!(press(&mut chat, "G", 'g'), Some(ChatDo::Top));
        assert_eq!(chat.picked, Some(0));
        // Shift+G comes as g, Shift held, G typed.
        assert_eq!(press(&mut chat, "Shift+G", 'G'), Some(ChatDo::Bottom));
        assert_eq!(press(&mut chat, "Y", 'y'), Some(ChatDo::Copy("two".into())));
        assert_eq!(chat.focus, ChatFocus::List);
        // A character goes to the reply, typed into it; Shift+J too.
        press(&mut chat, "Shift+J", 'J');
        assert_eq!(chat.focus, ChatFocus::Composer);
        assert_eq!(chat.draft, "J");
    }

    #[test]
    fn the_reply_sends_on_enter_and_esc_keeps_its_draft() {
        let mut chat = with_items(Vec::new());
        press(&mut chat, "I", 'i');
        assert_eq!(chat.focus, ChatFocus::Composer);
        // In the reply, list keys are letters.
        press(&mut chat, "J", 'j');
        press(&mut chat, "K", 'k');
        assert_eq!(chat.draft, "jk");
        press(&mut chat, "Shift+Enter", '\r');
        assert_eq!(chat.draft, "jk\n");
        press(&mut chat, "Esc", '\u{1b}');
        assert_eq!(chat.focus, ChatFocus::List);
        assert_eq!(chat.draft, "jk\n");
        press(&mut chat, "I", 'i');
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
        press(&mut chat, "Ctrl+Shift+Del", '\u{7f}');
        assert_eq!(chat.draft, "");
        assert_eq!(one_line("fix it\n  and test\n"), "fix it and test");
    }

    #[test]
    fn a_reply_never_goes_to_a_shell() {
        let process = |pid: i32, name: &str| Process {
            pid,
            name: name.into(),
            argv: vec![name.into()],
            cmdline: name.into(),
            cwd: None,
        };
        let at_prompt = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(10),
            foreground_processes: vec![process(10, "zsh")],
        };
        assert!(agent_in_front(&Ok(at_prompt)).is_err());
        let agent = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(20),
            foreground_processes: vec![process(20, "claude")],
        };
        assert_eq!(agent_in_front(&Ok(agent)), Ok(()));
        assert!(agent_in_front(&Err("no server".into())).is_err());
    }

    #[test]
    fn a_copied_item_says_what_it_holds() {
        let tool = Item::Tool {
            id: "t".into(),
            name: "Bash".into(),
            gist: "ls".into(),
            input: "ls -la".into(),
            output: Some(ToolOut {
                text: "a\nb".into(),
                failed: false,
                ms: None,
            }),
        };
        assert_eq!(item_text(&tool), "Bash\nls -la\n\na\nb");
        let edit = Item::Edit {
            id: "e".into(),
            path: "src/a.rs".into(),
            diff: vec![
                DiffLine {
                    side: DiffSide::Removed,
                    text: "old".into(),
                },
                DiffLine {
                    side: DiffSide::Added,
                    text: "new".into(),
                },
            ],
            output: None,
        };
        assert_eq!(item_text(&edit), "src/a.rs\n-old\n+new");
        let said = Item::Assistant {
            id: "a".into(),
            at: None,
            blocks: Arc::new(crate::md_blocks::blocks("# Done\n\nIt **works**.\n\n- one")),
        };
        assert_eq!(item_text(&said), "# Done\n\nIt works.\n\n- one");
    }

    #[test]
    fn the_waiting_panel_shows_the_last_rows_but_empty_ones() {
        let rows = |texts: &[&str]| -> ChatRows {
            texts
                .iter()
                .map(|t| ((*t).to_owned(), Vec::new()))
                .collect()
        };
        let got = last_rows(rows(&["a", "b", "c", "", " "]), 2);
        assert_eq!(got, rows(&["b", "c"]));
        assert_eq!(wait_key_name(WaitKey::Esc), "esc");
        assert_eq!(wait_key_name(WaitKey::Digit(2)), "2");
    }

    #[test]
    fn the_chat_keys_agree_with_their_hints() {
        for (chord, _) in key_table::f1_rows(LIST_KEYS)
            .into_iter()
            .chain(key_table::f1_rows(COMPOSER_KEYS))
        {
            assert!(crate::keys::hint_key(chord).is_some(), "{chord}");
        }
    }
}
