//! The inbox (prefix, i; the sidebar's "+" menu; a click on its "Needs
//! you" heading): one place to go through what wants the user across the
//! shown project, a section per kind: agents blocked, done or at work; Git
//! needing a look (changes, commits to push or pull, stashes, a merge
//! stopped half-way, worktrees left behind); GitHub's failed CI and pull
//! requests asking for review; the tickets assigned to the user in the
//! project's ticket repositories, in their board's order; and todos
//! (gerdr's own, and the GitHub and Linear issues assigned to the user).
//!
//! Typing filters the list. Up/Down select, Enter goes to the item,
//! Delete dismisses it (until it changes), Ctrl+S snoozes it for an hour,
//! Ctrl+Shift+S until tomorrow morning, Ctrl+H lists the hidden ones
//! (Delete restores one). Ctrl+N writes a todo, Ctrl+E edits one (Tab
//! cycles the workspace it is linked to); Ctrl+T adds a ticket repository
//! (`owner/repo`) to the shown project; Delete marks a todo done,
//! Shift+Delete deletes it. Ctrl+O opens an item's link, Ctrl+C copies it,
//! Ctrl+R reads the issues again. Tab lists the active workspace's items
//! only, or every workspace's again. Esc clears the filter, then goes back
//! from the hidden ones, then closes.
//!
//! This module is the model: the items (built by `inbox_sources`), what
//! of them shows (`visible`, `order`), and the panel's keys, clicks and
//! view. `inbox_panel` wires it into gerdr; `chrome::inbox` draws it.

use std::{
    cmp::Reverse,
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
    chrome::{self, InboxCompose, InboxDetailView, InboxHit, InboxLine, InboxRow, InboxView},
    ci::CiState,
    config, fuzzy,
    herdr::AgentStatus,
    inbox_store::{InboxFile, TodoId, Triage},
    issues::{IssueSource, IssuesRead, IssuesState, TicketRepo},
    text_field::{Edit, TextField},
    wheel::Wheel,
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
    Tickets,
    Todos,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Agents,
        Section::Git,
        Section::GitHub,
        Section::Tickets,
        Section::Todos,
    ];

    /// Its label's title.
    pub fn title(self) -> &'static str {
        match self {
            Section::Agents => "Agents",
            Section::Git => "Git",
            Section::GitHub => "GitHub",
            Section::Tickets => "Tickets",
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
    /// Add this repository (`owner/repo` or `host/owner/repo`) to the
    /// shown project's ticket repositories.
    AddTickets(String),
    /// Read the issues again.
    Refresh,
    /// List the dismissed or snoozed item again.
    Restore(ItemId),
    /// Say this.
    Notice(String),
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
    hides(triage, &item.id.0, &item.fingerprint, now)
}

/// Whether `triage` hides the item `id` at `now` (seconds) while its
/// fingerprint is `fingerprint`: what `is_hidden` asks, for those (the
/// sidebar's) that have no `InboxItem` at hand.
pub fn hides(triage: &BTreeMap<String, Triage>, id: &str, fingerprint: &str, now: u64) -> bool {
    triage
        .get(id)
        .is_some_and(|t| t.fingerprint == fingerprint && t.until.is_none_or(|until| now < until))
}

/// The items of `items` to list at `now` (seconds): those not hidden by
/// `triage`, or (`show_hidden`) only those hidden.
pub fn visible(
    items: &[InboxItem],
    triage: &BTreeMap<String, Triage>,
    now: u64,
    show_hidden: bool,
) -> Vec<InboxItem> {
    items
        .iter()
        .filter(|item| is_hidden(item, triage, now) == show_hidden)
        .cloned()
        .collect()
}

/// Whether the tab strip's inbox button counts `item`: an agent blocked or
/// done, a pull request asking for review, a failed CI of `main`, `master`,
/// `dev` or `develop`.
pub fn counted(item: &InboxItem) -> bool {
    match &item.detail {
        Detail::Agent { status, .. } => {
            matches!(status, AgentStatus::Blocked | AgentStatus::Done)
        }
        Detail::Ci { subject, state, .. } => {
            item.tone == Tone::Review
                || (*state == CiState::Failed && crate::ci::LONG_LIVED.contains(&subject.as_str()))
        }
        _ => false,
    }
}

/// `items` in list order: by section, then the most urgent first, then the
/// oldest.
pub fn order(items: &[InboxItem]) -> Vec<InboxItem> {
    let mut items = items.to_vec();
    // Stable: the rest keep the order they were collected in.
    items.sort_by_key(|item| match item.section {
        Section::Agents => (item.section, Reverse(urgency(item)), Reverse(item.age)),
        section => (section, Reverse(0), Reverse(None)),
    });
    items
}

/// How pressing an agent's item is: blocked, then done, then at work.
fn urgency(item: &InboxItem) -> u8 {
    match &item.detail {
        Detail::Agent { status, .. } => chrome::urgency(*status),
        _ => match item.tone {
            Tone::Blocked => chrome::urgency(AgentStatus::Blocked),
            Tone::Done => chrome::urgency(AgentStatus::Done),
            Tone::Working => chrome::urgency(AgentStatus::Working),
            _ => 0,
        },
    }
}

/// What can be done to `item` as listed: a hidden one (listed with Ctrl+H)
/// is restored rather than dismissed or snoozed again.
fn actions(item: &InboxItem, hidden: bool) -> Vec<ItemAction> {
    use ItemAction as A;
    if !hidden {
        return item.actions.clone();
    }
    let mut actions: Vec<_> = item
        .actions
        .iter()
        .copied()
        .filter(|a| {
            !matches!(
                a,
                A::Snooze | A::SnoozeLong | A::Dismiss | A::Done | A::Restore
            )
        })
        .collect();
    let at = actions
        .iter()
        .position(|a| *a == A::GoTo)
        .map_or(0, |k| k + 1);
    actions.insert(at, A::Restore);
    actions
}

/// The actions an item's row has buttons for, of its `actions`: the
/// rest are in its detail.
fn row_actions(actions: &[ItemAction]) -> Vec<ItemAction> {
    use ItemAction as A;
    actions
        .iter()
        .copied()
        .filter(|a| matches!(a, A::GoTo | A::Snooze | A::Dismiss | A::Done | A::Restore))
        .collect()
}

/// What Delete does, of `actions`: restores a hidden item, marks a todo
/// done, else dismisses.
fn delete_action(actions: &[ItemAction]) -> Option<ItemAction> {
    use ItemAction as A;
    [A::Restore, A::Done, A::Dismiss]
        .into_iter()
        .find(|a| actions.contains(a))
}

/// How long `age` is, in its largest unit: `45s`, `4m`, `2h`, `3d`.
pub fn age_text(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// How many of a pane's last lines an agent's detail shows.
const PANE_LINES: usize = 20;

/// The last `n` lines of `text`, without the blank ones at its end.
fn last_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |k| k + 1);
    lines[end.saturating_sub(n)..end]
        .iter()
        .map(|l| (*l).to_owned())
        .collect()
}

/// The word before a check of state `state` in a CI detail: a word, as
/// the monospace font has no check marks.
fn check_word(state: CiState) -> &'static str {
    match state {
        CiState::Failed => "failed",
        CiState::Passed => "passed",
        CiState::Running => "running",
        CiState::Unknown => "pending",
    }
}

fn ci_text(state: CiState) -> &'static str {
    match state {
        CiState::Failed => "Failed",
        CiState::Passed => "Passed",
        CiState::Running => "Running",
        CiState::Unknown => "Unknown",
    }
}

/// What the detail pane shows of `item`, which `actions` can be done to,
/// at `now` (seconds).
fn detail_view(item: &InboxItem, actions: &[ItemAction], now: u64) -> InboxDetailView {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut field = |label: &str, value: String| fields.push((label.to_owned(), value));
    let mut block_title = None;
    let mut block = Vec::new();
    // A pane's last lines: what an agent asks is at the end.
    let block_tail = matches!(item.detail, Detail::Agent { .. });
    match &item.detail {
        Detail::Agent {
            status,
            model,
            workspace,
            pane,
        } => {
            field("Status", chrome::status_meaning(*status).to_owned());
            if let Some(age) = item.age {
                field("For", age_text(age));
            }
            if let Some(model) = model {
                field("Model", model.clone());
            }
            field("Workspace", workspace.clone());
            if let Some(pane) = pane {
                block_title = Some("Last output".to_owned());
                block = last_lines(pane, PANE_LINES);
            }
        }
        Detail::Git {
            branch,
            upstream,
            ahead,
            behind,
            stashes,
            op,
            changes,
            worktrees,
        } => {
            field(
                "Branch",
                branch.clone().unwrap_or_else(|| "detached".to_owned()),
            );
            if let Some(upstream) = upstream {
                field("Upstream", upstream.clone());
            }
            if *ahead > 0 {
                field("Ahead", format!("{ahead} to push"));
            }
            if *behind > 0 {
                field("Behind", format!("{behind} to pull"));
            }
            if *stashes > 0 {
                field("Stashes", stashes.to_string());
            }
            if let Some(op) = op {
                field("In progress", op.clone());
            }
            if !worktrees.is_empty() {
                field("Worktrees", worktrees.join(", "));
            }
            if !changes.is_empty() {
                block_title = Some(format!("Changes ({})", changes.len()));
                block = changes
                    .iter()
                    .map(|(code, path)| format!("{code} {path}"))
                    .collect();
            }
        }
        Detail::Ci {
            title,
            subject,
            state,
            checks,
            link,
        } => {
            field("Subject", subject.clone());
            if let Some(title) = title {
                field("Title", title.clone());
            }
            field("State", ci_text(*state).to_owned());
            field("Link", link.clone());
            if !checks.is_empty() {
                // The failed ones first: what to look at.
                let mut checks: Vec<_> = checks.iter().collect();
                checks.sort_by_key(|(_, state, _)| *state != CiState::Failed);
                block_title = Some("Checks".to_owned());
                block = checks
                    .into_iter()
                    .map(|(name, state, _)| format!("{:<7} {name}", check_word(*state)))
                    .collect();
            }
        }
        Detail::Issue {
            source,
            key,
            title,
            state,
            labels,
            place,
            url,
            updated,
        } => {
            let source = match source {
                IssueSource::GitHub => "GitHub",
                IssueSource::Linear => "Linear",
            };
            field("Source", source.to_owned());
            field("Key", key.clone());
            field("State", state.clone());
            if !labels.is_empty() {
                field("Labels", labels.join(", "));
            }
            if !place.is_empty() {
                field("In", place.clone());
            }
            if !updated.is_empty() {
                field("Updated", updated.clone());
            }
            field("Link", url.clone());
            if title != &item.title {
                block_title = Some("Title".to_owned());
                block = vec![title.clone()];
            }
        }
        Detail::Todo {
            text,
            workspace,
            created,
        } => {
            field(
                "Workspace",
                workspace.clone().unwrap_or_else(|| "None".to_owned()),
            );
            field("Written", crate::projects::ago(now, *created));
            block_title = Some("Todo".to_owned());
            block = text.lines().map(str::to_owned).collect();
        }
    }
    InboxDetailView {
        title: item.title.clone(),
        tone: item.tone,
        fields,
        block_title,
        block,
        block_tail,
        buttons: actions.iter().map(|a| a.button()).collect(),
    }
}

/// An action's footer hint: its key, as the other panels' footers write
/// it, and what it does.
fn action_hint(action: ItemAction) -> (&'static str, &'static str) {
    use ItemAction as A;
    match action {
        A::GoTo => ("Enter", "go to"),
        A::Snooze => ("Ctrl+S", "snooze 1h"),
        A::SnoozeLong => ("Ctrl+Shift+S", "until tomorrow"),
        A::Dismiss => ("Del", "dismiss"),
        A::Restore => ("Del", "restore"),
        A::Done => ("Del", "done"),
        A::Edit => ("Ctrl+E", "edit"),
        A::Delete => ("Shift+Del", "delete"),
        A::OpenLink => ("Ctrl+O", "open link"),
        A::CopyLink => ("Ctrl+C", "copy link"),
    }
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
#[derive(Debug)]
pub struct Panel {
    /// What the list is filtered by.
    query: TextField,
    /// The todo (Ctrl+N, Ctrl+E) or ticket repository (Ctrl+T) being
    /// written, while one is.
    compose: Option<Compose>,
    hover: Option<InboxHit>,
    /// The section to select the first item of, on opening.
    focus: Option<Section>,
    /// The selected item; `None` until one is (the panel just opened, the
    /// list empty).
    selected: Option<ItemId>,
    /// Where the selected item was in the list: what is selected once it
    /// is gone.
    selected_at: usize,
    /// The first row in view, by index into the list's rows.
    first: usize,
    /// The wheel's turn not yet a whole row.
    wheel: Wheel,
    /// Every workspace's items, not just the active one's.
    all: bool,
    /// Only the hidden items are listed (Ctrl+H).
    show_hidden: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            query: TextField::default(),
            compose: None,
            hover: None,
            focus: None,
            selected: None,
            selected_at: 0,
            first: 0,
            wheel: Wheel::default(),
            all: true,
            show_hidden: false,
        }
    }
}

/// A todo, or a ticket repository, being written.
#[derive(Debug)]
struct Compose {
    text: TextField,
    /// A ticket repository to add, not a todo: no id, no workspace.
    tickets: bool,
    /// The todo edited; `None` for a new one.
    id: Option<TodoId>,
    /// The workspace it is linked to, by index into `Listing::workspaces`.
    workspace: Option<usize>,
}

/// A row of the list: a section's label, or an item by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// With how many items it lists, and how many more are hidden.
    Label(Section, usize, usize),
    Item(usize),
}

/// What the panel lists of a `Listing`: the items, filtered and in order,
/// and the rows, a section's label before its items.
struct Listed {
    items: Vec<InboxItem>,
    rows: Vec<Entry>,
}

impl Listed {
    /// The row item `k` is on.
    fn row(&self, k: usize) -> Option<usize> {
        self.rows.iter().position(|e| *e == Entry::Item(k))
    }
}

/// The workspace after `at` (`back`: before) among `n`, then none, round.
fn cycle(at: Option<usize>, n: usize, back: bool) -> Option<usize> {
    let at = at.filter(|&w| w < n);
    match (at, back) {
        _ if n == 0 => None,
        (None, false) => Some(0),
        (None, true) => Some(n - 1),
        (Some(w), false) => (w + 1 < n).then_some(w + 1),
        (Some(w), true) => w.checked_sub(1),
    }
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
        match self.compose.as_mut() {
            Some(compose) => &mut compose.text,
            None => &mut self.query,
        }
    }

    /// Whether the hidden items are listed (Ctrl+H).
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Whether every workspace's items are listed, not just the active
    /// one's.
    pub fn all(&self) -> bool {
        self.all
    }

    /// The selected item, as last settled (see `settle_on`).
    pub fn selected(&self) -> Option<&ItemId> {
        self.selected.as_ref()
    }

    /// Select what the view shows as selected in `list`, while nothing is
    /// (the panel just opened) or what was is gone: so `selected` is that
    /// before any key or click.
    pub fn settle_on(&mut self, list: &Listing) {
        let listed = self.listed(list);
        self.settle(&listed, list.page);
    }

    /// What it lists of `list`: the items not hidden (or only those, with
    /// Ctrl+H), of the scope, matching the query, the best matches first
    /// within each section; with every section's label.
    fn listed(&self, list: &Listing) -> Listed {
        let in_scope = |item: &InboxItem| {
            self.all
                || list.active.is_none()
                || item.workspace.is_none()
                || item.workspace == list.active
        };
        let score = |item: &InboxItem| {
            let text = format!("{} {}", item.title, item.subtitle);
            fuzzy::score(&self.query, &text).map(|(score, _)| score)
        };
        let shown = visible(list.items, list.triage, list.now, self.show_hidden);
        let mut kept: Vec<(i32, InboxItem)> = order(&shown)
            .into_iter()
            .filter(|item| in_scope(item))
            .filter_map(|item| Some((score(&item)?, item)))
            .collect();
        kept.sort_by_key(|(score, item)| (item.section, Reverse(*score)));
        let items: Vec<InboxItem> = kept.into_iter().map(|(_, item)| item).collect();
        // What else the sections hold, hidden.
        let hidden = |section: Section| {
            if self.show_hidden {
                return 0;
            }
            list.items
                .iter()
                .filter(|i| i.section == section && in_scope(i) && score(i).is_some())
                .filter(|i| is_hidden(i, list.triage, list.now))
                .count()
        };
        // A section with only hidden items keeps its label, to say so.
        let mut rows = Vec::new();
        for section in Section::ALL {
            let listed: Vec<usize> = (0..items.len())
                .filter(|&k| items[k].section == section)
                .collect();
            let hidden = hidden(section);
            if listed.is_empty() && hidden == 0 {
                continue;
            }
            rows.push(Entry::Label(section, listed.len(), hidden));
            rows.extend(listed.into_iter().map(Entry::Item));
        }
        Listed { items, rows }
    }

    /// The selected item, by index into `items`: the one selected while
    /// it is listed, else the one that took its place, else (just opened)
    /// the focused section's first, else the first.
    fn current(&self, items: &[InboxItem]) -> Option<usize> {
        let last = items.len().checked_sub(1)?;
        match &self.selected {
            Some(id) => Some(
                items
                    .iter()
                    .position(|i| &i.id == id)
                    .unwrap_or(self.selected_at.min(last)),
            ),
            None => Some(
                self.focus
                    .and_then(|s| items.iter().position(|i| i.section == s))
                    .unwrap_or(0),
            ),
        }
    }

    /// Whether the selected item is the one listed as selected, rather than
    /// one standing in for it.
    fn pinned(&self, items: &[InboxItem]) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|id| items.iter().any(|i| &i.id == id))
    }

    /// The first row in a view of `page` rows: as scrolled, within the
    /// list; moved to show item `k` (and its section's label, above its
    /// first) when `follow`.
    fn first_row(&self, listed: &Listed, k: Option<usize>, page: usize, follow: bool) -> usize {
        let page = page.max(1);
        let mut first = self.first.min(listed.rows.len().saturating_sub(page));
        if let Some(r) = k.and_then(|k| listed.row(k)).filter(|_| follow) {
            let top = match r.checked_sub(1).map(|r| listed.rows[r]) {
                Some(Entry::Label(..)) => r - 1,
                _ => r,
            };
            first = first.min(top);
            if r >= first + page {
                first = r + 1 - page;
            }
        }
        first
    }

    /// Select item `k` of `listed`, scrolling it into a view of `page` rows.
    fn select(&mut self, listed: &Listed, k: usize, page: usize) {
        let Some(item) = listed.items.get(k) else {
            return;
        };
        self.selected = Some(item.id.clone());
        self.selected_at = k;
        let before = self.first_row(listed, None, page, false);
        self.first = self.first_row(listed, Some(k), page, true);
        // Scrolled to it, or selected at the top where it is cut, the list
        // shows whole rows.
        if self.first != before || listed.row(k) == Some(self.first) {
            self.wheel = Wheel::default();
        }
    }

    /// The part of row `first` scrolled past in a view of `page` rows of
    /// `listed`: none where the list shows its end.
    fn part(&self, listed: &Listed, first: usize, page: usize) -> f64 {
        if first + page.max(1) < listed.rows.len() {
            self.wheel.part(first)
        } else {
            0.0
        }
    }

    /// With the selected item gone from `listed` (or none selected yet),
    /// select the one standing in for it.
    fn settle(&mut self, listed: &Listed, page: usize) {
        if !self.pinned(&listed.items)
            && let Some(k) = self.current(&listed.items)
        {
            self.select(listed, k, page);
        }
    }

    /// What it lists changed with the filter, the scope or Ctrl+H: the best
    /// match selected while there is a query, else the selection kept when
    /// still listed, else the first.
    fn refilter(&mut self, list: &Listing) {
        let listed = self.listed(list);
        self.first = 0;
        self.wheel = Wheel::default();
        let kept = listed
            .items
            .iter()
            .position(|i| Some(&i.id) == self.selected.as_ref())
            .filter(|_| self.query.is_empty());
        if let Some(k) = kept.or((!listed.items.is_empty()).then_some(0)) {
            self.select(&listed, k, list.page);
        }
    }

    /// Do `action` to `item` (`hidden`: listed with Ctrl+H), if it can be
    /// done to it.
    fn act(&mut self, item: &InboxItem, action: ItemAction, list: &Listing) -> Option<Outcome> {
        if !actions(item, self.show_hidden).contains(&action) {
            return None;
        }
        let id = item.id.clone();
        Some(match action {
            // A todo of no workspace leads nowhere: written over instead.
            ItemAction::GoTo
                if matches!(item.target, Target::Todo(_)) && item.workspace.is_none() =>
            {
                self.edit(item, list);
                return None;
            }
            ItemAction::GoTo => Outcome::GoTo(id),
            ItemAction::Restore => Outcome::Restore(id),
            ItemAction::Edit => {
                self.edit(item, list);
                return None;
            }
            action => Outcome::Act(id, action),
        })
    }

    /// Write a new todo, linked to the selected item's workspace, else the
    /// active one.
    fn write(&mut self, item: Option<&InboxItem>, list: &Listing) {
        let workspace = item.and_then(|i| i.workspace).or(list.active);
        self.compose = Some(Compose {
            text: TextField::default(),
            tickets: false,
            id: None,
            workspace: workspace.filter(|&w| w < list.workspaces.len()),
        });
    }

    /// Write a ticket repository to add.
    fn write_tickets(&mut self) {
        self.compose = Some(Compose {
            text: TextField::default(),
            tickets: true,
            id: None,
            workspace: None,
        });
    }

    /// Edit todo `item`'s text and workspace.
    fn edit(&mut self, item: &InboxItem, list: &Listing) {
        let id = match &item.target {
            Target::Todo(id) => id.clone(),
            _ => match item.id.0.strip_prefix("todo:") {
                Some(id) => TodoId(id.to_owned()),
                None => return,
            },
        };
        let text = match &item.detail {
            Detail::Todo { text, .. } => text.clone(),
            _ => item.title.clone(),
        };
        self.compose = Some(Compose {
            text: text.into(),
            tickets: false,
            id: Some(id),
            workspace: item.workspace.filter(|&w| w < list.workspaces.len()),
        });
    }

    /// A key while a todo or ticket repository is being written: the
    /// field takes what edits it; Enter saves (an empty one is dropped; a
    /// repository not written as one is said so, and kept to fix), Esc
    /// drops it, Tab and Shift+Tab cycle the workspace a todo is linked to.
    fn compose_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        list: &Listing,
    ) -> Option<Outcome> {
        let compose = self.compose.as_mut()?;
        if compose.text.key(mods, sym, typed) != Edit::Ignored || mods.alt || mods.logo {
            return None;
        }
        match sym {
            Keysym::Escape => self.compose = None,
            Keysym::Return | Keysym::KP_Enter if !mods.ctrl => {
                let compose = self.compose.take()?;
                let text = compose.text.trim().to_owned();
                if compose.tickets && !text.is_empty() {
                    let repo = TicketRepo {
                        repo: text.clone(),
                        project: None,
                    };
                    if repo.place().is_err() {
                        self.compose = Some(compose);
                        return Some(Outcome::Notice(format!(
                            "\"{text}\" is not owner/repo or host/owner/repo"
                        )));
                    }
                    return Some(Outcome::AddTickets(text));
                }
                if !text.is_empty() {
                    return Some(Outcome::SaveTodo {
                        id: compose.id,
                        text,
                        workspace: compose.workspace,
                    });
                }
            }
            Keysym::Tab | Keysym::ISO_Left_Tab if !mods.ctrl && !compose.tickets => {
                let back = mods.shift || sym == Keysym::ISO_Left_Tab;
                compose.workspace = cycle(compose.workspace, list.workspaces.len(), back);
            }
            _ => {}
        }
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
        if self.compose.is_some() {
            return self.compose_key(mods, sym, typed, list);
        }
        match self.query.key(mods, sym, typed) {
            Edit::Ignored => {}
            Edit::Changed => {
                self.refilter(list);
                return None;
            }
            Edit::Moved => return None,
        }
        if mods.alt || mods.logo {
            return None;
        }
        let (ctrl, shift) = (mods.ctrl, mods.shift);
        // What changes the list, rather than act on its selection.
        match sym {
            Keysym::Tab | Keysym::ISO_Left_Tab if !ctrl && list.active.is_some() => {
                self.all = !self.all;
            }
            Keysym::Escape if !self.query.is_empty() => self.query.clear(),
            Keysym::Escape if self.show_hidden => self.show_hidden = false,
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::h | Keysym::H if ctrl => self.show_hidden = !self.show_hidden,
            Keysym::r | Keysym::R if ctrl => return Some(Outcome::Refresh),
            _ => {
                let listed = self.listed(list);
                return self.list_key(ctrl, shift, sym, listed, list);
            }
        }
        self.refilter(list);
        None
    }

    /// A key for the list in `listed`, with Ctrl and Shift as said.
    fn list_key(
        &mut self,
        ctrl: bool,
        shift: bool,
        sym: Keysym,
        listed: Listed,
        list: &Listing,
    ) -> Option<Outcome> {
        let page = list.page;
        self.settle(&listed, page);
        let k = self.current(&listed.items);
        let item = k.map(|k| &listed.items[k]);
        let step = |by: isize| {
            let n = listed.items.len() as isize;
            match k {
                _ if n == 0 => None,
                Some(k) => Some((k as isize + by).clamp(0, n - 1) as usize),
                None => Some(0),
            }
        };
        let by_page = page.max(1) as isize;
        let to = match sym {
            Keysym::Down | Keysym::KP_Down => step(1),
            Keysym::Up | Keysym::KP_Up => step(-1),
            Keysym::Page_Down | Keysym::KP_Page_Down => step(by_page),
            Keysym::Page_Up | Keysym::KP_Page_Up => step(-by_page),
            Keysym::Home | Keysym::KP_Home => step(isize::MIN / 2),
            Keysym::End | Keysym::KP_End => step(isize::MAX / 2),
            _ => None,
        };
        if let Some(to) = to {
            self.select(&listed, to, page);
            return None;
        }
        let action = match sym {
            Keysym::n | Keysym::N if ctrl => {
                self.write(item, list);
                return None;
            }
            Keysym::t | Keysym::T if ctrl => {
                self.write_tickets();
                return None;
            }
            Keysym::Return | Keysym::KP_Enter if !ctrl => ItemAction::GoTo,
            // Shift changes the symbol: match both cases.
            Keysym::s | Keysym::S if ctrl && shift => ItemAction::SnoozeLong,
            Keysym::s | Keysym::S if ctrl => ItemAction::Snooze,
            Keysym::e | Keysym::E if ctrl => ItemAction::Edit,
            Keysym::o | Keysym::O if ctrl => ItemAction::OpenLink,
            Keysym::c | Keysym::C if ctrl => ItemAction::CopyLink,
            Keysym::Delete | Keysym::KP_Delete if !ctrl && shift => ItemAction::Delete,
            Keysym::Delete | Keysym::KP_Delete if !ctrl => {
                delete_action(&actions(item?, self.show_hidden))?
            }
            _ => return None,
        };
        let item = item?.clone();
        self.act(&item, action, list)
    }

    /// A left click on `hit` (not a hint: `inbox_panel` presses those);
    /// what to do, if anything. A click on the selected row goes to it.
    pub fn click(&mut self, hit: InboxHit, list: &Listing) -> Option<Outcome> {
        if self.compose.is_some() {
            return None;
        }
        let page = list.page;
        let listed = self.listed(list);
        self.settle(&listed, page);
        let selected = self.current(&listed.items);
        let first = self.first_row(&listed, selected, page, false);
        // Scrolled part of a row, the row under the page shows too.
        let shown = page.max(1) + usize::from(self.part(&listed, first, page) > 0.0);
        let at = |i: usize| match listed.rows.get(first + i) {
            Some(Entry::Item(k)) if i < shown => Some(*k),
            _ => None,
        };
        let (item, action) = match hit {
            InboxHit::Scope(all) => {
                if list.active.is_some() && self.all != all {
                    self.all = all;
                    self.refilter(list);
                }
                return None;
            }
            InboxHit::Query | InboxHit::Panel | InboxHit::Hint(_) => return None,
            InboxHit::Row(i) => {
                let k = at(i)?;
                if selected != Some(k) {
                    self.select(&listed, k, page);
                    return None;
                }
                (k, ItemAction::GoTo)
            }
            InboxHit::Button(i, b) => {
                let k = at(i)?;
                self.select(&listed, k, page);
                let actions = actions(&listed.items[k], self.show_hidden);
                (k, *row_actions(&actions).get(b)?)
            }
            InboxHit::DetailButton(b) => {
                let k = selected?;
                (k, *actions(&listed.items[k], self.show_hidden).get(b)?)
            }
        };
        let item = listed.items[item].clone();
        self.act(&item, action, list)
    }

    /// What the pointer is over now.
    pub fn hover(&mut self, hit: Option<InboxHit>) {
        self.hover = hit;
    }

    /// The wheel turned `rows` rows (down positive) over the list: whole
    /// rows scroll it, the part of a row left shows it moved by as much.
    pub fn scroll(&mut self, rows: f64, list: &Listing) {
        let listed = self.listed(list);
        self.settle(&listed, list.page);
        let last = listed.rows.len().saturating_sub(list.page.max(1));
        let first = self.first_row(&listed, None, list.page, false);
        let wanted = first as i64 + self.wheel.turn(rows, first);
        self.first = wanted.clamp(0, last as i64) as usize;
        self.wheel.settle(wanted, self.first, last);
    }

    /// The footer's hints, for what is selected (`actions`) over `list`.
    fn hints(&self, actions: &[ItemAction], list: &Listing) -> Vec<(String, String)> {
        let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
        if let Some(compose) = &self.compose {
            let mut hints = vec![hint("Enter", if compose.tickets { "add" } else { "save" })];
            if !compose.tickets && !list.workspaces.is_empty() {
                hints.push(hint("Tab", "link a workspace"));
            }
            hints.push(hint("Esc", "cancel"));
            return hints;
        }
        use ItemAction as A;
        let mut hints: Vec<_> = [
            A::GoTo,
            A::Restore,
            A::Done,
            A::Dismiss,
            A::Snooze,
            A::SnoozeLong,
            A::Edit,
            A::Delete,
            A::OpenLink,
            A::CopyLink,
        ]
        .into_iter()
        .filter(|a| actions.contains(a))
        // Delete does one of them.
        .filter(|a| {
            !matches!(a, A::Restore | A::Done | A::Dismiss) || delete_action(actions) == Some(*a)
        })
        .map(|a| {
            let (k, what) = action_hint(a);
            hint(k, what)
        })
        .collect();
        hints.push(hint("Ctrl+N", "new todo"));
        hints.push(hint("Ctrl+T", "add ticket repo"));
        hints.push(hint(
            "Ctrl+H",
            if self.show_hidden {
                "back to the list"
            } else {
                "hidden ones"
            },
        ));
        hints.push(hint("Ctrl+R", "refresh"));
        if list.active.is_some() {
            let scope = if self.all {
                "this workspace"
            } else {
                "all workspaces"
            };
            hints.push(hint("Tab", scope));
        }
        let esc = if !self.query.is_empty() {
            "clear"
        } else if self.show_hidden {
            "back"
        } else {
            "close"
        };
        hints.push(hint("Esc", esc));
        hints
    }

    /// What the panel shows of `list`.
    pub fn view(&self, list: &Listing) -> InboxView {
        let listed = self.listed(list);
        let page = list.page.max(1);
        let selected = self.current(&listed.items);
        let follow = !self.pinned(&listed.items);
        let first = self.first_row(&listed, selected, page, follow);
        // Scrolled part of a row, the list shows one more under it.
        let part = self.part(&listed, first, page);
        let end = (first + page + usize::from(part > 0.0)).min(listed.rows.len());
        let hovered = match self.hover {
            Some(InboxHit::Row(i) | InboxHit::Button(i, _)) => Some(first + i),
            _ => None,
        };
        let rows = listed.rows[first..end]
            .iter()
            .enumerate()
            .map(|(n, entry)| match *entry {
                Entry::Label(section, count, hidden) => InboxRow::Label {
                    title: section.title().to_owned(),
                    count,
                    hidden,
                },
                Entry::Item(k) => {
                    let item = &listed.items[k];
                    let buttons = if selected == Some(k) || hovered == Some(first + n) {
                        row_actions(&actions(item, self.show_hidden))
                            .into_iter()
                            .map(ItemAction::button)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    InboxRow::Item(InboxLine {
                        id: item.id.0.clone(),
                        title: item.title.clone(),
                        subtitle: item.subtitle.clone(),
                        tone: item.tone,
                        age: item.age.map(age_text).unwrap_or_default(),
                        hidden: self.show_hidden,
                        buttons,
                    })
                }
            })
            .collect();
        let item_actions = selected
            .map(|k| actions(&listed.items[k], self.show_hidden))
            .unwrap_or_default();
        let empty = if !self.query.is_empty() {
            "No match"
        } else if self.show_hidden {
            "Nothing hidden"
        } else {
            "Nothing needs you"
        };
        let mut detail = selected.map(|k| detail_view(&listed.items[k], &item_actions, list.now));
        // A hidden one says why, and till when.
        if let (Some(detail), Some(k)) = (detail.as_mut(), selected)
            && self.show_hidden
            && let Some(triage) = list.triage.get(&listed.items[k].id.0)
        {
            let why = match triage.until {
                Some(until) => {
                    let left = Duration::from_secs(until.saturating_sub(list.now));
                    format!("snoozed, back in {}", age_text(left))
                }
                None => "dismissed until it changes".to_owned(),
            };
            detail.fields.push(("Hidden".to_owned(), why));
        }
        let placeholder = match &self.compose {
            Some(c) if c.tickets => "Ticket repository: owner/repo…",
            Some(_) => "New todo…",
            None if self.show_hidden => "Filter the hidden items…",
            None => "Filter the inbox…",
        };
        InboxView {
            project: list.project.map(str::to_owned),
            all: list.active.map(|_| self.all),
            query: self.query.line(),
            placeholder: placeholder.into(),
            compose: self.compose.as_ref().map(|c| InboxCompose {
                text: c.text.line(),
                workspace: c.workspace.and_then(|w| list.workspaces.get(w).cloned()),
                editing: c.id.is_some(),
                tickets: c.tickets,
            }),
            rows,
            first,
            total: listed.rows.len(),
            selected: selected
                .and_then(|k| listed.row(k))
                .filter(|r| (first..end).contains(r))
                .map(|r| r - first),
            hover: self.hover,
            detail,
            hints: self.hints(&item_actions, list),
            empty: empty.into(),
            show_hidden: self.show_hidden,
            part,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ItemAction as A;

    fn item(id: &str, section: Section, workspace: Option<usize>) -> InboxItem {
        let detail = match section {
            Section::Agents => Detail::Agent {
                status: AgentStatus::Working,
                model: None,
                workspace: "w".into(),
                pane: None,
            },
            _ => Detail::Todo {
                text: format!("{id} text"),
                workspace: None,
                created: 0,
            },
        };
        InboxItem {
            id: ItemId(id.into()),
            section,
            fingerprint: "f1".into(),
            workspace,
            title: id.into(),
            subtitle: "w · status".into(),
            tone: Tone::Info,
            age: None,
            target: Target::Link("https://example.com".into()),
            actions: vec![A::GoTo, A::Snooze, A::SnoozeLong, A::Dismiss],
            detail,
        }
    }

    fn agent(id: &str, status: AgentStatus, age: u64) -> InboxItem {
        let mut item = item(id, Section::Agents, Some(0));
        item.age = Some(Duration::from_secs(age));
        if let Detail::Agent { status: s, .. } = &mut item.detail {
            *s = status;
        }
        item
    }

    fn todo(n: u32, workspace: Option<usize>) -> InboxItem {
        let mut item = item(&format!("todo:t{n}"), Section::Todos, workspace);
        item.target = Target::Todo(TodoId(format!("t{n}")));
        item.actions = vec![A::GoTo, A::Snooze, A::Done, A::Edit, A::Delete];
        item
    }

    const WORKSPACES: [&str; 2] = ["gerdr", "herdr"];

    fn names() -> Vec<String> {
        WORKSPACES.iter().map(|w| (*w).to_owned()).collect()
    }

    fn listing<'a>(
        items: &'a [InboxItem],
        triage: &'a BTreeMap<String, Triage>,
        names: &'a [String],
    ) -> Listing<'a> {
        Listing {
            items,
            triage,
            now: 1000,
            active: Some(0),
            workspaces: names,
            page: 10,
            project: Some("gerdr"),
        }
    }

    fn mods(ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            ..ModifiersState::default()
        }
    }

    fn press(panel: &mut Panel, sym: Keysym, list: &Listing) -> Option<Outcome> {
        panel.key(&ModifiersState::default(), sym, sym, list)
    }

    fn ctrl(panel: &mut Panel, sym: Keysym, list: &Listing) -> Option<Outcome> {
        panel.key(&mods(true, false), sym, sym, list)
    }

    fn type_text(panel: &mut Panel, text: &str, list: &Listing) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            panel.key(&ModifiersState::default(), sym, sym, list);
        }
    }

    fn dismissed(fingerprint: &str, until: Option<u64>) -> Triage {
        Triage {
            fingerprint: fingerprint.into(),
            until,
            seen: 0,
        }
    }

    /// The ids of the items a view lists, in order.
    fn ids(view: &InboxView) -> Vec<String> {
        view.rows
            .iter()
            .filter_map(|r| match r {
                InboxRow::Item(line) => Some(line.id.clone()),
                InboxRow::Label { .. } => None,
            })
            .collect()
    }

    fn selected_id(view: &InboxView) -> Option<String> {
        match &view.rows[view.selected?] {
            InboxRow::Item(line) => Some(line.id.clone()),
            InboxRow::Label { .. } => None,
        }
    }

    #[test]
    fn a_dismissed_item_is_hidden_until_it_changes() {
        let mut it = item("git:/src/r", Section::Git, Some(0));
        let triage = BTreeMap::from([("git:/src/r".to_owned(), dismissed("f1", None))]);
        assert!(is_hidden(&it, &triage, 1000));
        assert!(visible(std::slice::from_ref(&it), &triage, 1000, false).is_empty());
        it.fingerprint = "f2".into();
        assert!(!is_hidden(&it, &triage, 1000));
        assert_eq!(
            visible(std::slice::from_ref(&it), &triage, 1000, false).len(),
            1
        );
        // Nothing triaged, nothing hidden.
        assert!(!is_hidden(&it, &BTreeMap::new(), 1000));
    }

    #[test]
    fn a_snooze_runs_out() {
        let it = item("ci:x", Section::GitHub, None);
        let triage = BTreeMap::from([("ci:x".to_owned(), dismissed("f1", Some(2000)))]);
        assert!(is_hidden(&it, &triage, 1999));
        assert!(!is_hidden(&it, &triage, 2000));
        assert!(!is_hidden(&it, &triage, 5000));
    }

    #[test]
    fn ctrl_h_lists_only_the_hidden_ones_to_restore() {
        let items = vec![
            item("a", Section::Git, Some(0)),
            item("b", Section::Git, Some(0)),
        ];
        let triage = BTreeMap::from([("b".to_owned(), dismissed("f1", None))]);
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(ids(&panel.view(&list)), ["a"]);
        assert_eq!(ctrl(&mut panel, Keysym::h, &list), None);
        assert!(panel.show_hidden());
        let view = panel.view(&list);
        assert!(view.show_hidden);
        assert_eq!(ids(&view), ["b"]);
        let InboxRow::Item(line) = &view.rows[1] else {
            panic!("{:?}", view.rows);
        };
        assert!(line.hidden);
        let labels: Vec<_> = line.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Go to", "Restore"]);
        // Hidden already: no snoozing it again.
        assert_eq!(ctrl(&mut panel, Keysym::s, &list), None);
        assert_eq!(
            press(&mut panel, Keysym::Delete, &list),
            Some(Outcome::Restore(ItemId("b".into())))
        );
        assert!(view.hints.contains(&("Del".into(), "restore".into())));
        let fields = view.detail.map(|d| d.fields).unwrap_or_default();
        assert!(fields.contains(&("Hidden".into(), "dismissed until it changes".into())));
        // Esc goes back to the list first.
        assert_eq!(press(&mut panel, Keysym::Escape, &list), None);
        assert!(!panel.show_hidden());
        assert_eq!(
            press(&mut panel, Keysym::Escape, &list),
            Some(Outcome::Close)
        );
        // With nothing hidden, it says so.
        let none = BTreeMap::new();
        let list = listing(&items, &none, &names);
        ctrl(&mut panel, Keysym::h, &list);
        let view = panel.view(&list);
        assert!(view.rows.is_empty());
        assert_eq!(view.empty, "Nothing hidden");
    }

    #[test]
    fn the_selection_survives_a_reorder_and_a_removal() {
        let mut items = vec![
            item("a", Section::Git, Some(0)),
            item("b", Section::Git, Some(0)),
            item("c", Section::Git, Some(0)),
        ];
        let triage = BTreeMap::new();
        let names = names();
        let mut panel = Panel::new(None);
        let list = listing(&items, &triage, &names);
        press(&mut panel, Keysym::Down, &list);
        assert_eq!(panel.selected(), Some(&ItemId("b".into())));
        items.reverse();
        let list = listing(&items, &triage, &names);
        assert_eq!(selected_id(&panel.view(&list)).as_deref(), Some("b"));
        press(&mut panel, Keysym::Down, &list);
        assert_eq!(panel.selected(), Some(&ItemId("a".into())));
        // Gone, the one taking its place is selected.
        items.retain(|i| i.id.0 != "a");
        items.push(item("d", Section::Git, Some(0)));
        let list = listing(&items, &triage, &names);
        assert_eq!(selected_id(&panel.view(&list)).as_deref(), Some("d"));
        assert_eq!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::GoTo(ItemId("d".into())))
        );
    }

    #[test]
    fn it_opens_on_the_focused_section() {
        let items = vec![
            item("a", Section::Git, Some(0)),
            todo(1, None),
            todo(2, None),
        ];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let panel = Panel::new(Some(Section::Todos));
        assert_eq!(selected_id(&panel.view(&list)).as_deref(), Some("todo:t1"));
        // A section with nothing in it: the first item.
        let panel = Panel::new(Some(Section::Agents));
        assert_eq!(selected_id(&panel.view(&list)).as_deref(), Some("a"));
    }

    #[test]
    fn a_todo_is_written_linked_and_saved() {
        let items = vec![item("a", Section::Git, Some(1))];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(ctrl(&mut panel, Keysym::n, &list), None);
        // Linked to the selected item's workspace.
        let compose = panel.view(&list).compose.unwrap();
        assert_eq!(compose.workspace.as_deref(), Some("herdr"));
        assert!(!compose.editing);
        type_text(&mut panel, "Fix it", &list);
        assert_eq!(&**panel.field_mut(), "Fix it");
        // The filter stays as it was.
        assert!(panel.view(&list).query.is_empty());
        // Tab: none, then the first, round.
        let tab = |panel: &mut Panel, shift: bool| {
            panel.key(&mods(false, shift), Keysym::Tab, Keysym::Tab, &list);
            panel.view(&list).compose.unwrap().workspace
        };
        assert_eq!(tab(&mut panel, false), None);
        assert_eq!(tab(&mut panel, false).as_deref(), Some("gerdr"));
        assert_eq!(tab(&mut panel, true), None);
        assert_eq!(tab(&mut panel, true).as_deref(), Some("herdr"));
        // Keys for the list are the field's, or nothing.
        assert_eq!(press(&mut panel, Keysym::Delete, &list), None);
        assert_eq!(ctrl(&mut panel, Keysym::s, &list), None);
        assert!(panel.view(&list).compose.is_some());
        assert_eq!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::SaveTodo {
                id: None,
                text: "Fix it".into(),
                workspace: Some(1),
            })
        );
        assert!(panel.view(&list).compose.is_none());
        // Empty, Enter drops it; so does Esc, the panel staying open.
        ctrl(&mut panel, Keysym::n, &list);
        type_text(&mut panel, "  ", &list);
        assert_eq!(press(&mut panel, Keysym::Return, &list), None);
        assert!(panel.view(&list).compose.is_none());
        ctrl(&mut panel, Keysym::n, &list);
        type_text(&mut panel, "x", &list);
        assert_eq!(press(&mut panel, Keysym::Escape, &list), None);
        assert!(panel.view(&list).compose.is_none());
        assert_eq!(&**panel.field_mut(), "");
    }

    #[test]
    fn ctrl_t_adds_a_ticket_repository() {
        let items = vec![item("a", Section::Git, Some(1))];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(ctrl(&mut panel, Keysym::t, &list), None);
        let view = panel.view(&list);
        let compose = view.compose.unwrap();
        assert!(compose.tickets);
        assert_eq!(compose.workspace, None);
        assert_eq!(view.placeholder, "Ticket repository: owner/repo…");
        assert!(!view.hints.iter().any(|(k, _)| k == "Tab"));
        // Tab links no workspace.
        press(&mut panel, Keysym::Tab, &list);
        assert_eq!(panel.view(&list).compose.unwrap().workspace, None);
        // Not a repository: said so, and kept to fix.
        type_text(&mut panel, "acme", &list);
        assert!(matches!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::Notice(_))
        ));
        assert!(panel.view(&list).compose.unwrap().tickets);
        type_text(&mut panel, "/tickets ", &list);
        assert_eq!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::AddTickets("acme/tickets".into()))
        );
        assert!(panel.view(&list).compose.is_none());
        // Esc drops it, the panel staying open.
        ctrl(&mut panel, Keysym::t, &list);
        assert_eq!(press(&mut panel, Keysym::Escape, &list), None);
        assert!(panel.view(&list).compose.is_none());
    }

    #[test]
    fn ctrl_e_edits_a_todo() {
        let items = vec![todo(3, Some(0)), item("a", Section::Git, None)];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        // Not a todo: nothing to edit.
        assert_eq!(ctrl(&mut panel, Keysym::e, &list), None);
        assert!(panel.view(&list).compose.is_none());
        press(&mut panel, Keysym::Down, &list);
        assert_eq!(panel.selected(), Some(&ItemId("todo:t3".into())));
        ctrl(&mut panel, Keysym::e, &list);
        let compose = panel.view(&list).compose.unwrap();
        assert!(compose.editing);
        assert_eq!(compose.text, "todo:t3 text");
        assert_eq!(compose.workspace.as_deref(), Some("gerdr"));
        type_text(&mut panel, "!", &list);
        assert_eq!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::SaveTodo {
                id: Some(TodoId("t3".into())),
                text: "todo:t3 text!".into(),
                workspace: Some(0),
            })
        );
    }

    #[test]
    fn enter_on_a_todo_of_no_workspace_edits_it() {
        let items = vec![todo(4, None), todo(5, Some(1))];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(press(&mut panel, Keysym::Return, &list), None);
        let compose = panel.view(&list).compose.unwrap();
        assert!(compose.editing);
        assert_eq!(compose.text, "todo:t4 text");
        press(&mut panel, Keysym::Escape, &list);
        press(&mut panel, Keysym::Down, &list);
        assert_eq!(
            press(&mut panel, Keysym::Return, &list),
            Some(Outcome::GoTo(ItemId("todo:t5".into())))
        );
    }

    #[test]
    fn the_selection_is_settled_before_a_key() {
        let items = vec![
            item("a", Section::Git, None),
            agent("b", AgentStatus::Blocked, 5),
        ];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(Some(Section::Git));
        assert_eq!(panel.selected(), None);
        panel.settle_on(&list);
        assert_eq!(panel.selected(), Some(&ItemId("a".into())));
        // What it shows selected is what is selected.
        assert_eq!(selected_id(&panel.view(&list)).as_deref(), Some("a"));
        // Gone, the one in its place is.
        let rest = vec![items[1].clone()];
        let list = listing(&rest, &triage, &names);
        panel.settle_on(&list);
        assert_eq!(panel.selected(), Some(&ItemId("b".into())));
    }

    #[test]
    fn esc_clears_the_filter_then_closes() {
        let items = vec![
            item("alpha", Section::Git, None),
            item("beta", Section::Git, None),
        ];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        type_text(&mut panel, "bet", &list);
        let view = panel.view(&list);
        assert_eq!(ids(&view), ["beta"]);
        assert_eq!(selected_id(&view).as_deref(), Some("beta"));
        assert!(view.hints.contains(&("Esc".into(), "clear".into())));
        type_text(&mut panel, "zz", &list);
        assert_eq!(panel.view(&list).empty, "No match");
        assert_eq!(press(&mut panel, Keysym::Escape, &list), None);
        let view = panel.view(&list);
        assert_eq!(ids(&view), ["alpha", "beta"]);
        assert!(view.hints.contains(&("Esc".into(), "close".into())));
        assert_eq!(
            press(&mut panel, Keysym::Escape, &list),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn no_key_takes_the_default_prefix() {
        // Ctrl+B arms the prefix before the panel sees it, and the prefix
        // then eats the next key too.
        let items = vec![todo(1, None), item("a", Section::Git, None)];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        for composing in [false, true] {
            let mut panel = Panel::new(None);
            if composing {
                ctrl(&mut panel, Keysym::n, &list);
            }
            let before = panel.view(&list);
            for (m, sym) in [
                (mods(true, false), Keysym::b),
                (mods(true, true), Keysym::B),
            ] {
                assert_eq!(panel.key(&m, sym, sym, &list), None);
            }
            assert_eq!(panel.view(&list), before);
            assert!(!before.hints.iter().any(|(k, _)| k.contains("Ctrl+B")));
        }
    }

    #[test]
    fn only_what_the_item_allows_is_asked_for() {
        let mut gh = item("gh:o/r#1", Section::Todos, None);
        gh.actions = vec![A::GoTo, A::OpenLink, A::CopyLink];
        let items = vec![gh, todo(1, None)];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        let id = ItemId("gh:o/r#1".into());
        assert_eq!(ctrl(&mut panel, Keysym::s, &list), None);
        assert_eq!(press(&mut panel, Keysym::Delete, &list), None);
        assert_eq!(ctrl(&mut panel, Keysym::e, &list), None);
        assert_eq!(
            ctrl(&mut panel, Keysym::o, &list),
            Some(Outcome::Act(id.clone(), A::OpenLink))
        );
        assert_eq!(
            ctrl(&mut panel, Keysym::c, &list),
            Some(Outcome::Act(id, A::CopyLink))
        );
        assert_eq!(ctrl(&mut panel, Keysym::r, &list), Some(Outcome::Refresh));
        // A todo: Delete marks it done, Shift+Delete deletes it.
        press(&mut panel, Keysym::Down, &list);
        let todo = ItemId("todo:t1".into());
        assert_eq!(
            press(&mut panel, Keysym::Delete, &list),
            Some(Outcome::Act(todo.clone(), A::Done))
        );
        assert_eq!(
            panel.key(&mods(false, true), Keysym::Delete, Keysym::Delete, &list),
            Some(Outcome::Act(todo.clone(), A::Delete))
        );
        assert_eq!(
            ctrl(&mut panel, Keysym::s, &list),
            Some(Outcome::Act(todo.clone(), A::Snooze))
        );
        // Not allowed for this one.
        assert_eq!(
            panel.key(&mods(true, true), Keysym::S, Keysym::S, &list),
            None
        );
        assert_eq!(ctrl(&mut panel, Keysym::o, &list), None);
        let view = panel.view(&list);
        let keys: Vec<_> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            keys.contains(&"Del") && !keys.contains(&"Ctrl+O"),
            "{keys:?}"
        );
        let detail = view.detail.unwrap();
        let labels: Vec<_> = detail.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Go to", "Snooze", "Done", "Edit", "Delete"]);
        // Snooze until tomorrow, where allowed.
        let items = vec![item("a", Section::Git, None)];
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(
            panel.key(&mods(true, true), Keysym::S, Keysym::S, &list),
            Some(Outcome::Act(ItemId("a".into()), A::SnoozeLong))
        );
        assert_eq!(
            press(&mut panel, Keysym::Delete, &list),
            Some(Outcome::Act(ItemId("a".into()), A::Dismiss))
        );
    }

    #[test]
    fn clicks_select_go_and_act() {
        let items = vec![item("a", Section::Git, None), item("b", Section::Git, None)];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        // Row 0 is the label.
        assert_eq!(panel.click(InboxHit::Row(0), &list), None);
        assert_eq!(panel.click(InboxHit::Row(2), &list), None);
        assert_eq!(panel.selected(), Some(&ItemId("b".into())));
        assert_eq!(
            panel.click(InboxHit::Row(2), &list),
            Some(Outcome::GoTo(ItemId("b".into())))
        );
        // Row a's second button: Snooze.
        assert_eq!(
            panel.click(InboxHit::Button(1, 1), &list),
            Some(Outcome::Act(ItemId("a".into()), A::Snooze))
        );
        assert_eq!(panel.selected(), Some(&ItemId("a".into())));
        // The detail's third: Tomorrow.
        assert_eq!(
            panel.click(InboxHit::DetailButton(2), &list),
            Some(Outcome::Act(ItemId("a".into()), A::SnoozeLong))
        );
        assert_eq!(panel.click(InboxHit::DetailButton(9), &list), None);
        assert_eq!(panel.view(&list).all, Some(true));
        panel.click(InboxHit::Scope(false), &list);
        assert!(!panel.all());
    }

    #[test]
    fn the_scope_is_the_active_workspace_or_all() {
        let items = vec![
            item("a", Section::Git, Some(0)),
            item("b", Section::Git, Some(1)),
            todo(1, None),
        ];
        let triage = BTreeMap::new();
        let names = names();
        let list = listing(&items, &triage, &names);
        let mut panel = Panel::new(None);
        assert_eq!(ids(&panel.view(&list)), ["a", "b", "todo:t1"]);
        press(&mut panel, Keysym::Tab, &list);
        assert!(!panel.all());
        let view = panel.view(&list);
        assert_eq!(view.all, Some(false));
        assert_eq!(ids(&view), ["a", "todo:t1"]);
        // Without an active workspace, everything, and no toggle.
        let list = Listing {
            active: None,
            ..list
        };
        let view = panel.view(&list);
        assert_eq!(view.all, None);
        assert_eq!(ids(&view).len(), 3);
    }

    #[test]
    fn labels_count_the_listed_and_hidden() {
        let items = vec![
            item("a", Section::Git, None),
            item("b", Section::Git, None),
            item("c", Section::Git, None),
            item("d", Section::GitHub, None),
            item("e", Section::GitHub, None),
        ];
        let triage = BTreeMap::from([
            ("b".to_owned(), dismissed("f1", None)),
            ("e".to_owned(), dismissed("f1", Some(5000))),
            // Changed since: listed.
            ("c".to_owned(), dismissed("f0", None)),
        ]);
        let names = names();
        let list = listing(&items, &triage, &names);
        let view = Panel::new(None).view(&list);
        let labels: Vec<_> = view
            .rows
            .iter()
            .filter_map(|r| match r {
                InboxRow::Label {
                    title,
                    count,
                    hidden,
                } => Some((title.as_str(), *count, *hidden)),
                InboxRow::Item(_) => None,
            })
            .collect();
        assert_eq!(labels, [("Git", 2, 1), ("GitHub", 1, 1)]);
        assert_eq!(view.total, 5);
        // All of a section hidden, its label stays to say so.
        let triage = BTreeMap::from([("d".to_owned(), dismissed("f1", None))]);
        let triage = triage.into_iter().chain(list.triage.clone()).collect();
        let list = listing(&items, &triage, &names);
        let view = Panel::new(None).view(&list);
        assert!(view.rows.iter().any(|r| *r
            == InboxRow::Label {
                title: "GitHub".into(),
                count: 0,
                hidden: 2,
            }));
    }

    #[test]
    fn paging_keeps_the_selection_in_view() {
        let items: Vec<_> = (0..12)
            .map(|n| item(&format!("g{n}"), Section::Git, None))
            .chain((0..3).map(|n| todo(n, None)))
            .collect();
        let triage = BTreeMap::new();
        let names = names();
        let list = Listing {
            page: 5,
            ..listing(&items, &triage, &names)
        };
        let mut panel = Panel::new(None);
        let in_view = |panel: &Panel| {
            let view = panel.view(&list);
            assert!(view.rows.len() <= 5);
            assert_eq!(view.total, 17);
            let id = selected_id(&view).expect("the selection in view");
            // Just opened, nothing is selected for good yet.
            if let Some(selected) = panel.selected() {
                assert_eq!(selected, &ItemId(id));
            }
            view
        };
        let view = in_view(&panel);
        assert_eq!(view.first, 0);
        assert!(matches!(view.rows[0], InboxRow::Label { .. }));
        for _ in 0..7 {
            press(&mut panel, Keysym::Down, &list);
            in_view(&panel);
        }
        press(&mut panel, Keysym::Page_Down, &list);
        assert_eq!(panel.selected(), Some(&ItemId("todo:t0".into())));
        let view = in_view(&panel);
        assert_eq!(view.selected, Some(4));
        assert!(matches!(&view.rows[3], InboxRow::Label { title, .. } if title == "Todos"));
        press(&mut panel, Keysym::End, &list);
        assert_eq!(in_view(&panel).first, 12);
        press(&mut panel, Keysym::Page_Up, &list);
        in_view(&panel);
        // Up to a section's first item, its label comes into view too.
        press(&mut panel, Keysym::Home, &list);
        assert_eq!(in_view(&panel).first, 0);
        // The wheel scrolls, within the list.
        panel.scroll(3.0, &list);
        assert_eq!(panel.view(&list).first, 3);
        panel.scroll(100.0, &list);
        assert_eq!(panel.view(&list).first, 12);
        // Half a row up shows the row above in part.
        panel.scroll(-0.5, &list);
        assert_eq!(panel.view(&list).first, 11);
        assert_eq!(panel.view(&list).part, 0.5);
        panel.scroll(-0.5, &list);
        assert_eq!(panel.view(&list).first, 11);
        assert_eq!(panel.view(&list).part, 0.0);
        // Clicks go by the rows in view.
        let view = panel.view(&list);
        let InboxRow::Item(line) = &view.rows[0] else {
            panic!("{:?}", view.rows);
        };
        assert_eq!(line.id, "g10");
        assert_eq!(panel.click(InboxHit::Row(2), &list), None);
        panel.click(InboxHit::Row(0), &list);
        assert_eq!(panel.selected(), Some(&ItemId("g10".into())));
        assert_eq!(panel.view(&list).first, 11);
    }

    #[test]
    fn a_touchpad_scrolls_by_parts_of_a_row() {
        let items: Vec<_> = (0..12)
            .map(|n| item(&format!("g{n}"), Section::Git, None))
            .chain((0..3).map(|n| todo(n, None)))
            .collect();
        let triage = BTreeMap::new();
        let names = names();
        let list = Listing {
            page: 5,
            ..listing(&items, &triage, &names)
        };
        let mut panel = Panel::new(None);
        let id = |view: &InboxView, i: usize| match &view.rows[i] {
            InboxRow::Item(line) => line.id.clone(),
            row => panic!("{row:?}"),
        };
        panel.scroll(1.5, &list);
        let view = panel.view(&list);
        assert_eq!((view.first, view.part), (1, 0.5));
        // The row shown in part under the page too, and clicked.
        assert_eq!(view.rows.len(), 6);
        assert_eq!(id(&view, 5), "g5");
        panel.click(InboxHit::Row(5), &list);
        assert_eq!(panel.selected(), Some(&ItemId("g5".into())));
        // Selecting it scrolled the list to show it whole.
        let view = panel.view(&list);
        assert_eq!((view.first, view.part, view.rows.len()), (2, 0.0, 5));
        // Up past the top, and down past the end, hold the list whole.
        panel.scroll(-3.25, &list);
        let view = panel.view(&list);
        assert_eq!((view.first, view.part, view.rows.len()), (0, 0.0, 5));
        panel.scroll(100.5, &list);
        let view = panel.view(&list);
        assert_eq!((view.first, view.part, view.rows.len()), (12, 0.0, 5));
        // A key moving the list drops the part.
        press(&mut panel, Keysym::Home, &list);
        panel.scroll(0.5, &list);
        assert_eq!(panel.view(&list).part, 0.5);
        press(&mut panel, Keysym::End, &list);
        assert_eq!(panel.view(&list).part, 0.0);
        // So does selecting the row cut at the top, shown whole then.
        press(&mut panel, Keysym::Home, &list);
        panel.scroll(2.5, &list);
        assert_eq!(id(&panel.view(&list), 0), "g1");
        press(&mut panel, Keysym::Down, &list);
        let view = panel.view(&list);
        assert_eq!(panel.selected(), Some(&ItemId("g1".into())));
        assert_eq!((view.first, view.part, view.selected), (2, 0.0, Some(0)));
    }

    #[test]
    fn agents_go_by_urgency_then_wait() {
        let items = vec![
            item("todo", Section::Todos, None),
            agent("working", AgentStatus::Working, 500),
            item("git", Section::Git, None),
            agent("done-new", AgentStatus::Done, 10),
            agent("blocked", AgentStatus::Blocked, 5),
            agent("done-old", AgentStatus::Done, 300),
            item("ci", Section::GitHub, None),
            item("git2", Section::Git, None),
        ];
        let ids: Vec<_> = order(&items).into_iter().map(|i| i.id.0).collect();
        assert_eq!(
            ids,
            [
                "blocked", "done-old", "done-new", "working", "git", "git2", "ci", "todo"
            ]
        );
    }

    #[test]
    fn the_detail_shows_the_panes_last_lines() {
        let mut it = agent("a", AgentStatus::Blocked, 90);
        let text: String = (1..=30).map(|n| format!("line {n}  \n")).collect();
        if let Detail::Agent { pane, .. } = &mut it.detail {
            *pane = Some(format!("{text}\n\n   \n"));
        }
        let detail = detail_view(&it, &it.actions, 0);
        assert_eq!(detail.block_title.as_deref(), Some("Last output"));
        assert_eq!(detail.block.len(), PANE_LINES);
        assert_eq!(detail.block.first().map(String::as_str), Some("line 11"));
        assert_eq!(detail.block.last().map(String::as_str), Some("line 30"));
        // Its end is what matters: cut at the start when it doesn't fit.
        assert!(detail.block_tail);
        assert!(detail.fields.contains(&("For".into(), "1m".into())));
        let mut git = item("git:/r", Section::Git, Some(0));
        git.detail = Detail::Git {
            branch: Some("main".into()),
            upstream: None,
            ahead: 2,
            behind: 0,
            stashes: 0,
            op: None,
            changes: vec![('M', "src/a.rs".into()), ('?', "new".into())],
            worktrees: Vec::new(),
        };
        let detail = detail_view(&git, &git.actions, 0);
        assert_eq!(detail.block, ["M src/a.rs", "? new"]);
        assert!(!detail.block_tail);
        assert!(
            detail
                .fields
                .contains(&("Ahead".into(), "2 to push".into()))
        );
        assert_eq!(age_text(Duration::from_secs(4 * 3600 + 5)), "4h");
        assert_eq!(age_text(Duration::from_secs(3 * 86400)), "3d");
    }
}
