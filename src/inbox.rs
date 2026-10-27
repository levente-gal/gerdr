//! The inbox (prefix, i; the sidebar's "+" menu; a click on its "Needs
//! you" heading): one place to go through what wants the user across the
//! shown project, a section per kind: agents blocked, done or at work; Git
//! needing a look (changes, commits to push or pull, stashes, a merge
//! stopped half-way, worktrees left behind); GitHub's failed CI and pull
//! requests asking for review; and todos (gerdr's own, and the GitHub and
//! Linear issues assigned to the user).
//!
//! Typing filters the list. Up/Down select, Enter goes to the item,
//! Delete dismisses it (until it changes), Ctrl+S snoozes it for an hour,
//! Ctrl+Shift+S until tomorrow morning, Ctrl+H lists the hidden ones
//! (Delete restores one). Ctrl+N writes a todo, Ctrl+E edits one (Tab
//! cycles the workspace it is linked to); Delete marks a todo done,
//! Shift+Delete deletes it. Ctrl+O opens an item's link, Ctrl+C copies it,
//! Ctrl+R reads the issues again. Esc clears the filter, then closes.
//!
//! This module is the model: the items (built by `inbox_sources`), what
//! of them shows (`visible`, `order`), and the panel's keys, clicks and
//! view. `inbox_panel` wires it into gerdr; `chrome::inbox` draws it.

#![allow(dead_code)]

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
};

use crate::{
    agents::AgentKey,
    chrome::{self, InboxHit, InboxView},
    ci::CiState,
    config,
    herdr::AgentStatus,
    inbox_store::{InboxFile, TodoId, Triage},
    issues::{IssueSource, IssuesRead, IssuesState},
    text_field::TextField,
};

/// What colour an item is: the chrome's own, as it has nothing to
/// convert.
pub type Tone = chrome::InboxTone;

/// An item's stable id, the key of its triage: `agent:<AgentKey>`,
/// `git:<dir>`, `worktree:<path>`, `ci:<host/slug>:<subject>`,
/// `gh:<host/slug>#<n>`, `linear:<uuid>`, `todo:<TodoId>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub String);

/// The list's sections, in the order they show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Agents,
    Git,
    GitHub,
    Todos,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Agents,
        Section::Git,
        Section::GitHub,
        Section::Todos,
    ];

    /// Its label's title.
    pub fn title(self) -> &'static str {
        match self {
            Section::Agents => "Agents",
            Section::Git => "Git",
            Section::GitHub => "GitHub",
            Section::Todos => "Todos",
        }
    }
}

/// Something in the inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub id: ItemId,
    pub section: Section,
    /// What it is now, in short: a change brings a dismissed or snoozed
    /// item back (see `Triage`).
    pub fingerprint: String,
    /// The workspace it belongs to, by index into `Gerdr::workspaces`;
    /// `None` for the whole project (an issue, a todo without one).
    pub workspace: Option<usize>,
    pub title: String,
    /// Under the title: `workspace · status`.
    pub subtitle: String,
    pub tone: Tone,
    /// How long it has been as it is; `None` when unknown.
    pub age: Option<Duration>,
    /// Where Enter goes.
    pub target: Target,
    /// What can be done to it, on its row and in its detail, in order.
    pub actions: Vec<ItemAction>,
    pub detail: Detail,
}

/// Where going to an item leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Show the agent (`Gerdr::focus_agent`).
    Agent(AgentKey),
    /// Switch to the workspace (`Gerdr::switch_workspace`).
    Workspace(usize),
    /// The worktrees popup, this worktree selected
    /// (`Gerdr::open_worktrees`).
    Worktree(PathBuf),
    /// A page shown in a tab of the workspace (`Gerdr::open_page`).
    Page { workspace: usize, url: String },
    /// A page of no workspace: in the active one.
    Link(String),
    /// Edit the todo (its workspace, if linked, is switched to).
    Todo(TodoId),
}

/// What can be done to an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemAction {
    /// Enter.
    GoTo,
    /// For an hour: Ctrl+S.
    Snooze,
    /// Until tomorrow morning: Ctrl+Shift+S.
    SnoozeLong,
    /// Until it changes: Delete.
    Dismiss,
    /// A hidden item, listed again: Delete.
    Restore,
    /// A todo: Delete.
    Done,
    /// A todo: Ctrl+E.
    Edit,
    /// A todo, for good: Shift+Delete.
    Delete,
    /// Its link, in a tab: Ctrl+O.
    OpenLink,
    /// Its link, to the clipboard: Ctrl+C.
    CopyLink,
}

impl ItemAction {
    /// Its button's label.
    pub fn label(self) -> &'static str {
        match self {
            ItemAction::GoTo => "Go to",
            ItemAction::Snooze => "Snooze",
            ItemAction::SnoozeLong => "Tomorrow",
            ItemAction::Dismiss => "Dismiss",
            ItemAction::Restore => "Restore",
            ItemAction::Done => "Done",
            ItemAction::Edit => "Edit",
            ItemAction::Delete => "Delete",
            ItemAction::OpenLink => "Open",
            ItemAction::CopyLink => "Copy link",
        }
    }

    /// The key that does it, as the hints show it.
    pub fn key(self) -> &'static str {
        match self {
            ItemAction::GoTo => "Enter",
            ItemAction::Snooze => "Ctrl+S",
            ItemAction::SnoozeLong => "Ctrl+Shift+S",
            ItemAction::Dismiss | ItemAction::Restore | ItemAction::Done => "Delete",
            ItemAction::Edit => "Ctrl+E",
            ItemAction::Delete => "Shift+Delete",
            ItemAction::OpenLink => "Ctrl+O",
            ItemAction::CopyLink => "Ctrl+C",
        }
    }

    /// Its button, for the chrome.
    pub fn button(self) -> chrome::InboxButton {
        chrome::InboxButton {
            label: self.label().to_owned(),
            key: self.key().to_owned(),
        }
    }
}

/// What the detail pane shows of an item, by kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Agent {
        status: AgentStatus,
        model: Option<String>,
        /// Its workspace's name.
        workspace: String,
        /// The last lines of its pane, once read (see `Inbox::pane_text`).
        pane: Option<String>,
    },
    Git {
        branch: Option<String>,
        upstream: Option<String>,
        ahead: u32,
        behind: u32,
        stashes: u32,
        /// What it is in the middle of: `merging`, `rebasing 2/5`.
        op: Option<String>,
        /// The changed files, as (`git::Change::code`, path).
        changes: Vec<(char, String)>,
        /// The worktrees needing a look, by name.
        worktrees: Vec<String>,
    },
    Ci {
        /// A pull request's title.
        title: Option<String>,
        /// `#12`, or a branch's name.
        subject: String,
        state: CiState,
        /// Its checks: name, state, page.
        checks: Vec<(String, CiState, Option<String>)>,
        link: String,
    },
    Issue {
        source: IssueSource,
        key: String,
        title: String,
        state: String,
        labels: Vec<String>,
        place: String,
        url: String,
        updated: String,
    },
    Todo {
        text: String,
        /// Its workspace's name, if linked.
        workspace: Option<String>,
        created: u64,
    },
}

/// What the panel asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Go to the item (see `Target`).
    GoTo(ItemId),
    /// Do this to the item.
    Act(ItemId, ItemAction),
    /// Save the todo written: a new one (`id: None`) or an edit, linked to
    /// the workspace at this index.
    SaveTodo {
        id: Option<TodoId>,
        text: String,
        workspace: Option<usize>,
    },
    /// Read the issues again.
    Refresh,
    /// List the dismissed or snoozed item again.
    Restore(ItemId),
}

/// What the panel lists and acts on, gathered afresh for each event.
#[derive(Clone, Copy, Debug)]
pub struct Listing<'a> {
    /// Every item collected (see `inbox_sources::collect`), hidden ones
    /// too, in any order.
    pub items: &'a [InboxItem],
    /// How items were triaged, by id (`InboxFile::triage`).
    pub triage: &'a BTreeMap<String, Triage>,
    /// Now, in seconds since the epoch.
    pub now: u64,
    /// The active workspace, for the panel's "this workspace" scope.
    pub active: Option<usize>,
    /// The workspaces' names, by index: what a todo is linked to.
    pub workspaces: &'a [String],
    /// How many rows the list shows (`chrome::inbox_page`).
    pub page: usize,
    /// The shown project's name, for the title.
    pub project: Option<&'a str>,
}

/// Whether `item` is hidden by `triage` at `now` (seconds): dismissed or
/// snoozed while its fingerprint is the same, and the snooze not out yet.
pub fn is_hidden(item: &InboxItem, triage: &BTreeMap<String, Triage>, now: u64) -> bool {
    let _ = (item, triage, now);
    false
}

/// The items of `items` to list at `now` (seconds): those not hidden by
/// `triage`, or (`show_hidden`) only those hidden.
pub fn visible(
    items: &[InboxItem],
    triage: &BTreeMap<String, Triage>,
    now: u64,
    show_hidden: bool,
) -> Vec<InboxItem> {
    let _ = (triage, now, show_hidden);
    items.to_vec()
}

/// `items` in list order: by section, then the most urgent first, then the
/// oldest.
pub fn order(items: &[InboxItem]) -> Vec<InboxItem> {
    items.to_vec()
}

/// A pane's last lines as last read, for an agent's detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneText {
    pub text: String,
    pub at: Instant,
}

/// The inbox: the panel, while open, and what it lists that isn't read
/// elsewhere (the issues, the store, the panes' text). All its state lives
/// here, so `inbox_panel` needs nothing else of `Gerdr`'s.
#[derive(Default)]
pub struct Inbox {
    /// The panel, while open.
    pub panel: Option<Panel>,
    /// Linear's key and teams, from the config file and the environment;
    /// `None`: Linear isn't read.
    pub linear: Option<config::Linear>,
    /// The issues assigned to the user, as last read.
    pub issues: IssuesState,
    /// Hands a round of reading the issues to the event loop (see
    /// `Gerdr::watch_issues`).
    pub issues_tx: Option<channel::Sender<IssuesRead>>,
    /// The shown project's inbox file, by project id, as loaded.
    pub store: Option<(String, InboxFile)>,
    /// Agent panes' last lines, by herdr pane id.
    pub pane_text: HashMap<String, PaneText>,
    /// The panes whose text is being read.
    pub pending: HashSet<String>,
    /// Hands a pane's text (pane id, its text or why not) to the event loop
    /// (see `Gerdr::request_pane_text`).
    pub pane_tx: Option<channel::Sender<(String, Result<String, String>)>>,
}

impl Inbox {
    pub fn is_open(&self) -> bool {
        self.panel.is_some()
    }

    pub fn close(&mut self) {
        self.panel = None;
    }

    /// The field with the keyboard (the filter, or the todo being
    /// written), while the panel is open.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.panel.as_mut().map(Panel::field_mut)
    }
}

/// The panel's state: what is selected (by id, as the list changes under
/// it), scrolled to, filtered by, written.
#[derive(Debug, Default)]
pub struct Panel {
    /// What the list is filtered by.
    query: TextField,
    hover: Option<InboxHit>,
    /// The section to select the first item of, on opening.
    focus: Option<Section>,
}

impl Panel {
    /// A panel opening on `focus`'s first item, else the first.
    pub fn new(focus: Option<Section>) -> Self {
        Panel {
            focus,
            ..Panel::default()
        }
    }

    /// The field with the keyboard: the filter, or the todo being written.
    pub fn field_mut(&mut self) -> &mut TextField {
        &mut self.query
    }

    /// Whether the hidden items are listed (Ctrl+H).
    pub fn show_hidden(&self) -> bool {
        false
    }

    /// Whether every workspace's items are listed, not just the active
    /// one's.
    pub fn all(&self) -> bool {
        true
    }

    /// The selected item.
    pub fn selected(&self) -> Option<&ItemId> {
        None
    }

    /// A key, over `list`; what to do, if anything. `sym` is the key's own
    /// symbol, `typed` the one with Shift and the layout applied. The
    /// field takes the keys that edit it first.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        list: &Listing,
    ) -> Option<Outcome> {
        let _ = (mods, typed, list);
        (sym == Keysym::Escape).then_some(Outcome::Close)
    }

    /// A left click on `hit` (not a hint: `inbox_panel` presses those);
    /// what to do, if anything.
    pub fn click(&mut self, hit: InboxHit, list: &Listing) -> Option<Outcome> {
        let _ = (hit, list);
        None
    }

    /// What the pointer is over now.
    pub fn hover(&mut self, hit: Option<InboxHit>) {
        self.hover = hit;
    }

    /// The wheel turned `rows` rows (down positive) over the list.
    pub fn scroll(&mut self, rows: f64, list: &Listing) {
        let _ = (rows, list);
    }

    /// What the panel shows of `list`.
    pub fn view(&self, list: &Listing) -> InboxView {
        InboxView {
            project: list.project.map(str::to_owned),
            query: self.query.line(),
            placeholder: "Filter the inbox…".into(),
            hover: self.hover,
            empty: "Nothing needs you".into(),
            ..InboxView::default()
        }
    }
}
