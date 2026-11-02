//! The inbox's items, gathered from what gerdr already shows: the
//! feature gates waiting for the user, the agent cards, the workspaces' Git status and worktrees, the CI cards, the
//! issues and tickets read for the inbox and its own todos. Pure: everything comes in
//! as `Sources`, so tests build them by hand.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    agents::AgentKey,
    chrome::{AgentView, WorkspaceView, status_label, status_meaning},
    ci::{self, CiState, Group, RepoCi, Subject},
    cursor_agents::CursorChat,
    feature::Stage,
    feature_agents::Gate,
    git::{GitStatus, InProgress, Worktree},
    herdr::{AgentInfo, AgentStatus},
    inbox::{Detail, InboxItem, ItemAction, ItemId, PaneText, Section, Target, Tone},
    inbox_store::InboxFile,
    issues::{Issue, IssueSource, IssuesRead, IssuesState, TicketRepo},
    services::ErrorGroup,
    t3_agents::T3Thread,
    test_runs::TestRun,
};

/// What the items are built from.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    /// Each workspace as the sidebar shows it, by index.
    pub workspaces: &'a [WorkspaceView],
    /// Each workspace's folder (`Workspace::dir`), by index: which
    /// repository it is in when its Git status has no root.
    pub dirs: &'a [PathBuf],
    /// Each workspace's record id (`Workspace::record`), by index: what a
    /// todo is linked by.
    pub records: &'a [String],
    /// The active workspace: what an item shared by several workspaces (a
    /// repository's) belongs to, when it is one of them.
    pub active: Option<usize>,
    /// Each workspace's agent cards, in sidebar order (`agent_views`).
    pub agents: &'a [Vec<AgentView>],
    /// What herdr says of its agents (`Gerdr::agents`): their
    /// `state_change_seq`.
    pub herdr_agents: &'a [AgentInfo],
    /// Cursor's chats shown as cards (`Gerdr::cursor_chats`).
    pub cursor_chats: &'a [CursorChat],
    /// T3 Code's threads shown as cards (`T3::threads`).
    pub t3_threads: &'a [T3Thread],
    /// The test runs shown as cards (`Gerdr::test_runs`).
    pub test_runs: &'a [TestRun],
    /// Since when each agent has been as it is (`Gerdr::agent_since`).
    pub agent_since: &'a HashMap<AgentKey, (AgentStatus, Instant)>,
    /// The CI of the project's repositories, by root (`Gerdr::ci`).
    pub ci: &'a HashMap<PathBuf, RepoCi>,
    pub issues: &'a IssuesState,
    /// The shown project's ticket repositories (`Project::tickets`).
    pub tickets: &'a [TicketRepo],
    /// The shown project's inbox file, once loaded.
    pub store: Option<&'a InboxFile>,
    /// Agent panes' last lines, by herdr pane id.
    pub pane_text: &'a HashMap<String, PaneText>,
    /// The services' errors, the newest first.
    pub service_errors: &'a [ServiceItem],
    /// The services' errors the user ignores, one per project and
    /// fingerprint.
    pub ignored_errors: &'a [ServiceItem],
    /// The feature gates waiting for the user, over every project.
    pub gates: &'a [GateItem],
    pub now: Instant,
    /// Now, in seconds since the epoch.
    pub now_secs: u64,
}

/// A service's error, with what the inbox says of where it is.
#[derive(Clone, Debug)]
pub struct ServiceItem {
    pub group: ErrorGroup,
    /// The workspace its pane is in, by index: none for another
    /// project's.
    pub workspace: Option<usize>,
    /// Its workspace's name, else its project's.
    pub place: String,
    /// What its pane's bar calls the service.
    pub service: String,
}

/// A feature gate waiting for the user, with its project's name.
#[derive(Clone, Debug)]
pub struct GateItem {
    pub gate: Gate,
    /// Its project's name, as the project chooser shows it.
    pub project: String,
}

/// What every item but a todo can have done to it.
const TRIAGE: [ItemAction; 4] = [
    ItemAction::GoTo,
    ItemAction::Snooze,
    ItemAction::SnoozeLong,
    ItemAction::Dismiss,
];

/// The same, for an item with a link.
const LINKED: [ItemAction; 6] = [
    ItemAction::GoTo,
    ItemAction::Snooze,
    ItemAction::SnoozeLong,
    ItemAction::Dismiss,
    ItemAction::OpenLink,
    ItemAction::CopyLink,
];

/// What a todo can have done to it.
const TODO: [ItemAction; 5] = [
    ItemAction::GoTo,
    ItemAction::Edit,
    ItemAction::Done,
    ItemAction::Delete,
    ItemAction::Snooze,
];

impl<'a> Sources<'a> {
    /// Workspace `w`'s name.
    fn name(&self, w: usize) -> String {
        self.workspaces
            .get(w)
            .map_or_else(String::new, |ws| ws.name.clone())
    }

    /// Which of `shared`, workspaces of one repository, its item belongs
    /// to: the active one, else the first.
    fn pick(&self, shared: &[usize]) -> usize {
        self.active
            .filter(|a| shared.contains(a))
            .unwrap_or(shared[0])
    }

    /// The top folder of workspace `w`'s worktree: its Git status's root,
    /// else its folder; `None` outside a repository.
    fn root(&self, w: usize) -> Option<PathBuf> {
        let git = self.workspaces.get(w)?.git.as_ref()?;
        git.root.clone().or_else(|| self.dirs.get(w).cloned())
    }

    /// The workspaces in the worktree at `root` (the CI's key).
    fn in_repo(&self, root: &Path) -> Vec<usize> {
        (0..self.workspaces.len())
            .filter(|w| self.root(*w).as_deref() == Some(root))
            .collect()
    }

    /// The project's GitHub repositories, each once (by `ci::key`, in
    /// order): its CI as read for the first of its worktrees, and the
    /// workspaces in any of them.
    fn github(&self) -> Vec<(String, &'a RepoCi, Vec<usize>)> {
        let mut roots: Vec<_> = self.ci.iter().collect();
        roots.sort_by(|a, b| a.0.cmp(b.0));
        let mut repos: Vec<(String, &RepoCi, Vec<usize>)> = Vec::new();
        for (root, repo) in roots {
            let shared = self.in_repo(root);
            if shared.is_empty() {
                continue;
            }
            let key = ci::key(&repo.host, &repo.slug);
            match repos.iter_mut().find(|(k, ..)| *k == key) {
                Some((_, _, all)) => all.extend(shared),
                None => repos.push((key, repo, shared)),
            }
        }
        repos.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, _, shared) in &mut repos {
            shared.sort_unstable();
        }
        repos
    }
}

/// An agent card's fingerprint: its status and herdr's count of its
/// state changes (a Cursor chat's or T3 Code thread's last update, a test
/// run's process), so
/// it changes whenever the agent moves on. `None` for what is not an agent
/// (a CI card).
pub fn agent_fingerprint(
    view: &AgentView,
    herdr_agents: &[AgentInfo],
    cursor_chats: &[CursorChat],
    t3_threads: &[T3Thread],
    test_runs: &[TestRun],
) -> Option<String> {
    let status = status_label(view.status);
    match &view.key {
        AgentKey::Herdr(pane_id) => {
            let info = herdr_agents.iter().find(|a| &a.pane_id == pane_id);
            let seq = info.map_or(0, |a| a.state_change_seq);
            Some(format!("{status}#{seq}"))
        }
        AgentKey::Cursor(chat) => {
            let chat = cursor_chats.iter().find(|c| &c.row.id == chat);
            let updated = chat.map_or(0, |c| c.row.updated_ms);
            Some(format!("{status}#{updated}"))
        }
        AgentKey::T3(thread) => {
            let thread = t3_threads.iter().find(|t| &t.row.id == thread);
            let updated = thread.map_or("", |t| t.row.updated.as_str());
            Some(format!("{status}#{updated}"))
        }
        AgentKey::Test(pane_id) => {
            let run = test_runs.iter().find(|r| &r.pane_id == pane_id);
            let pid = run.map_or(0, |r| r.pid);
            Some(format!("{status}#{pid}"))
        }
        AgentKey::Ci(_) => None,
    }
}

/// The agents blocked, done or at work, in every workspace.
pub fn agent_items(src: &Sources) -> Vec<InboxItem> {
    let mut items = Vec::new();
    let mut listed = HashSet::new();
    for (w, cards) in src.agents.iter().enumerate() {
        let workspace = src.name(w);
        for view in cards {
            let tone = match view.status {
                AgentStatus::Blocked => Tone::Blocked,
                AgentStatus::Done => Tone::Done,
                AgentStatus::Working => Tone::Working,
                AgentStatus::Idle | AgentStatus::Unknown => continue,
            };
            if !listed.insert(view.key.clone()) {
                continue;
            }
            let Some(fingerprint) = agent_fingerprint(
                view,
                src.herdr_agents,
                src.cursor_chats,
                src.t3_threads,
                src.test_runs,
            ) else {
                continue;
            };
            let pane = match &view.key {
                AgentKey::Herdr(pane_id) | AgentKey::Test(pane_id) => {
                    src.pane_text.get(pane_id).map(|p| p.text.clone())
                }
                _ => None,
            };
            let age = src
                .agent_since
                .get(&view.key)
                .filter(|(s, _)| *s == view.status)
                .map(|(_, at)| src.now.saturating_duration_since(*at));
            let doing = view.title().unwrap_or(status_meaning(view.status));
            items.push(InboxItem {
                id: ItemId(format!("agent:{}", view.key)),
                section: Section::Agents,
                fingerprint,
                workspace: Some(w),
                title: view.heading().to_owned(),
                subtitle: format!("{workspace} · {doing}"),
                tone,
                age,
                target: Target::Agent(view.key.clone()),
                actions: TRIAGE.to_vec(),
                detail: Detail::Agent {
                    status: view.status,
                    icon: view.icon,
                    test: view.test,
                    model: view.model.clone(),
                    workspace: workspace.clone(),
                    pane,
                },
            });
        }
    }
    items
}

/// The workspace folders and worktrees needing a look.
pub fn git_items(src: &Sources) -> Vec<InboxItem> {
    // The workspaces by the worktree they are in, in order: one item each.
    let mut repos: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for w in 0..src.workspaces.len() {
        let Some(root) = src.root(w) else { continue };
        match repos.iter_mut().find(|(r, _)| *r == root) {
            Some((_, shared)) => shared.push(w),
            None => repos.push((root, vec![w])),
        }
    }
    let mut items = Vec::new();
    let mut listed: HashSet<PathBuf> = HashSet::new();
    for (root, shared) in &repos {
        let w = src.pick(shared);
        let view = &src.workspaces[w];
        let Some(git) = view.git.as_ref().filter(|g| needs_look(g)) else {
            continue;
        };
        listed.insert(root.clone());
        let folder = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        let branch = git.branch.clone().unwrap_or_else(|| "detached HEAD".into());
        // The rest it needs a look for is marked after (see `Detail::Git`).
        let mut subtitle = format!("{} · {branch}", view.name);
        if let Some(op) = git.op {
            subtitle = format!("{subtitle} · {}", op_label(op));
        }
        items.push(InboxItem {
            id: ItemId(format!("git:{}", root.display())),
            section: Section::Repos,
            fingerprint: git_fingerprint(git),
            workspace: Some(w),
            title: folder,
            subtitle,
            tone: Tone::Attention,
            age: None,
            target: Target::Workspace(w),
            actions: TRIAGE.to_vec(),
            detail: Detail::Git {
                branch: git.branch.clone(),
                upstream: git.upstream.clone(),
                dirty: git.dirty,
                ahead: git.ahead,
                behind: git.behind,
                stashes: git.stashes,
                op: git.op.map(op_label),
                changes: git
                    .changes
                    .iter()
                    .map(|c| (c.code, c.path.clone()))
                    .collect(),
                worktrees: view.needing_attention().iter().map(|t| t.name()).collect(),
            },
        });
    }
    // The other worktrees needing a look, each once, under a workspace of
    // their repository; not those already listed as a workspace's folder.
    let mut worktrees: Vec<(&Worktree, Vec<usize>)> = Vec::new();
    for (w, view) in src.workspaces.iter().enumerate() {
        for wt in view.needing_attention() {
            if listed.contains(&wt.path) {
                continue;
            }
            match worktrees.iter_mut().find(|(t, _)| t.path == wt.path) {
                Some((_, shared)) => shared.push(w),
                None => worktrees.push((wt, vec![w])),
            }
        }
    }
    for (wt, shared) in worktrees {
        let w = src.pick(&shared);
        // Its changes are marked after (see `Detail::Git`).
        let subtitle = match wt.attention_tag() {
            Some(tag) => format!("{} · worktree · {tag}", src.name(w)),
            None => format!("{} · worktree", src.name(w)),
        };
        items.push(InboxItem {
            id: ItemId(format!("worktree:{}", wt.path.display())),
            section: Section::Repos,
            fingerprint: format!(
                "{}:{}:{}",
                wt.tag(),
                wt.dirty,
                wt.head.as_deref().unwrap_or_default()
            ),
            workspace: Some(w),
            title: wt.name(),
            subtitle,
            tone: Tone::Attention,
            age: None,
            target: Target::Worktree(wt.path.clone()),
            actions: TRIAGE.to_vec(),
            detail: Detail::Git {
                branch: wt.branch.clone(),
                upstream: None,
                dirty: wt.dirty,
                ahead: 0,
                behind: 0,
                stashes: 0,
                op: None,
                changes: Vec::new(),
                worktrees: Vec::new(),
            },
        });
    }
    items
}

/// Whether a workspace's folder at `git` needs a look: changes, commits
/// to push or pull, stashes, or an operation stopped half-way.
fn needs_look(git: &GitStatus) -> bool {
    git.dirty || git.ahead > 0 || git.behind > 0 || git.stashes > 0 || git.op.is_some()
}

/// What `op` is, as going on: `merging`, `rebasing 2/5`.
pub fn op_label(op: InProgress) -> String {
    match op {
        InProgress::Merge => "merging".into(),
        InProgress::Rebase { step, total } => format!("rebasing {step}/{total}"),
        InProgress::CherryPick => "cherry-picking".into(),
        InProgress::Revert => "reverting".into(),
    }
}

/// A Git item's fingerprint: what it is checked out at and what it needs
/// a look for, hashed. Kept across runs (the triage is saved), so the hash
/// is FNV-1a, not std's seeded one.
fn git_fingerprint(git: &GitStatus) -> String {
    let mut text = format!(
        "{}|{}|{}|{}|{}|{}|{:?}",
        git.head.as_deref().unwrap_or_default(),
        git.branch.as_deref().unwrap_or_default(),
        git.dirty,
        git.ahead,
        git.behind,
        git.stashes,
        git.op,
    );
    for change in &git.changes {
        text.push('|');
        text.push(change.code);
        text.push_str(&change.path);
    }
    format!("{:016x}", fnv1a(text.as_bytes()))
}

/// The 64-bit FNV-1a hash of `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The failed CI rows and the pull requests asking for review: what keeps
/// a GitHub card from being quiet (`CiView::quiet`). A pull request's go
/// under Pull requests, a branch's under Repos. A repository whose CI
/// couldn't be read says so under Pull requests.
pub fn ci_items(src: &Sources) -> Vec<InboxItem> {
    let mut items = Vec::new();
    for (key, repo, shared) in src.github() {
        let w = src.pick(&shared);
        let rows = repo
            .rows
            .iter()
            .filter(|r| r.state == CiState::Failed || r.group == Group::Review);
        for row in rows {
            let subject = ci::subject_label(&row.subject);
            let review = row.group == Group::Review;
            let what = match (review, row.state == CiState::Failed) {
                (true, true) => "review asked, CI failed",
                (true, false) => "review asked",
                (false, _) => "CI failed",
            };
            items.push(InboxItem {
                id: ItemId(format!("ci:{key}:{subject}")),
                section: match row.subject {
                    Subject::Pr(_) => Section::PullRequests,
                    Subject::Branch(_) => Section::Repos,
                },
                fingerprint: format!("{:?}:{}", row.state, row.link),
                workspace: Some(w),
                title: row.title.clone().unwrap_or_else(|| subject.clone()),
                subtitle: format!("{} · {subject} · {what}", src.name(w)),
                tone: if review { Tone::Review } else { Tone::Failed },
                age: None,
                target: Target::Page {
                    workspace: w,
                    url: row.link.clone(),
                },
                actions: LINKED.to_vec(),
                detail: Detail::Ci {
                    title: row.title.clone(),
                    subject,
                    state: row.state,
                    checks: row
                        .checks
                        .iter()
                        .map(|c| (c.name.clone(), c.state, c.link.clone()))
                        .collect(),
                    link: row.link.clone(),
                },
            });
        }
        if let Some(why) = &repo.error {
            let title = format!("Couldn't load the pull requests of {key}");
            let id = format!("unread:ci:{key}");
            items.push(unread_item(id, Section::PullRequests, title, why));
        }
    }
    items
}

/// The GitHub and Linear issues assigned to the user: GitHub's of the
/// project's repositories only, but its ticket repositories (their issues
/// are its tickets).
pub fn issue_items(src: &Sources) -> Vec<InboxItem> {
    let mut items = Vec::new();
    let repos = src.github();
    let tickets: HashSet<String> = src.tickets.iter().map(TicketRepo::key).collect();
    for (key, read) in &src.issues.last.github {
        let Ok(issues) = read else { continue };
        if tickets.contains(key) {
            continue;
        }
        let Some((_, _, shared)) = repos.iter().find(|(k, ..)| k == key) else {
            continue;
        };
        let w = src.pick(shared);
        for issue in issues {
            let target = Target::Page {
                workspace: w,
                url: issue.url.clone(),
            };
            items.push(issue_item(issue, Some(w), &src.name(w), target));
        }
    }
    if let Some(Ok(issues)) = &src.issues.last.linear {
        for issue in issues {
            items.push(issue_item(issue, None, "", Target::Link(issue.url.clone())));
        }
    }
    items
}

/// A line in `section` saying `title` couldn't be read, and `why`; going
/// to it reads it again.
fn unread_item(id: String, section: Section, title: String, why: &str) -> InboxItem {
    InboxItem {
        id: ItemId(id),
        section,
        fingerprint: why.to_owned(),
        workspace: None,
        title,
        subtitle: why.to_owned(),
        tone: Tone::Failed,
        age: None,
        target: Target::Refresh,
        actions: TRIAGE.to_vec(),
        detail: Detail::Unread {
            why: why.to_owned(),
        },
    }
}

/// A line per source of tickets that couldn't be read and has nothing
/// read before to show: the project's repositories' GitHub issues, its
/// ticket repositories' tickets, Linear's issues. It says why; going to it
/// reads them again.
pub fn unread_items(src: &Sources) -> Vec<InboxItem> {
    let unread = |id, title, why: &String| unread_item(id, Section::Tickets, title, why);
    let mut items = Vec::new();
    let repos = src.github();
    let tickets: HashSet<String> = src.tickets.iter().map(TicketRepo::key).collect();
    for (key, read) in &src.issues.last.github {
        let Err(why) = read else { continue };
        if tickets.contains(key) || !repos.iter().any(|(k, ..)| k == key) {
            continue;
        }
        let title = format!("Couldn't load the issues of {key}");
        items.push(unread(format!("unread:gh:{key}"), title, why));
    }
    for repo in src.tickets {
        let key = repo.key();
        let read = src.issues.last.tickets.iter().find(|(k, _)| *k == key);
        if let Some((_, Err(why))) = read {
            let title = format!("Couldn't load the tickets of {key}");
            items.push(unread(format!("unread:tickets:{key}"), title, why));
        }
    }
    if let Some(Err(why)) = &src.issues.last.linear {
        let title = "Couldn't load Linear's issues".to_owned();
        items.push(unread("unread:linear".to_owned(), title, why));
    }
    items
}

/// The tickets assigned to the user in the project's ticket
/// repositories, each repository's in the order read (its board's).
pub fn ticket_items(src: &Sources) -> Vec<InboxItem> {
    let mut items = Vec::new();
    for repo in src.tickets {
        let key = repo.key();
        let read = src.issues.last.tickets.iter().find(|(k, _)| *k == key);
        let Some((_, Ok(tickets))) = read else {
            continue;
        };
        for ticket in tickets {
            items.push(issue_item(
                ticket,
                None,
                "",
                Target::Link(ticket.url.clone()),
            ));
        }
    }
    items
}

/// The item of `issue`, of workspace `w` (named `name`), going to
/// `target`.
fn issue_item(issue: &Issue, w: Option<usize>, name: &str, target: Target) -> InboxItem {
    let id = match issue.source {
        IssueSource::GitHub => format!("gh:{}", issue.id),
        IssueSource::Linear => format!("linear:{}", issue.id),
    };
    let subtitle = [name, &issue.key, &issue.place, &issue.state]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    InboxItem {
        id: ItemId(id),
        section: Section::Tickets,
        fingerprint: issue.updated.clone(),
        workspace: w,
        title: issue.title.clone(),
        subtitle,
        tone: Tone::Info,
        age: None,
        target,
        actions: LINKED.to_vec(),
        detail: Detail::Issue {
            source: issue.source,
            key: issue.key.clone(),
            title: issue.title.clone(),
            state: issue.state.clone(),
            labels: issue.labels.clone(),
            place: issue.place.clone(),
            url: issue.url.clone(),
            updated: issue.updated.clone(),
        },
    }
}

/// gerdr's own todos not done yet.
pub fn todo_items(src: &Sources) -> Vec<InboxItem> {
    let Some(store) = src.store else {
        return Vec::new();
    };
    let open = store.todos.iter().filter(|t| t.done.is_none());
    open.map(|todo| {
        let w = todo
            .workspace
            .as_ref()
            .and_then(|id| src.records.iter().position(|r| r == id));
        let name = w.map(|w| src.name(w));
        InboxItem {
            id: ItemId(format!("todo:{}", todo.id.0)),
            section: Section::Tickets,
            fingerprint: String::new(),
            workspace: w,
            title: todo.text.lines().next().unwrap_or_default().to_owned(),
            subtitle: name
                .as_ref()
                .map_or_else(|| "Todo".to_owned(), |n| format!("{n} · todo")),
            tone: Tone::Info,
            age: Some(Duration::from_secs(
                src.now_secs.saturating_sub(todo.created),
            )),
            target: Target::Todo(todo.id.clone()),
            actions: TODO.to_vec(),
            detail: Detail::Todo {
                text: todo.text.clone(),
                workspace: name,
                created: todo.created,
            },
        }
    })
    .collect()
}

/// The inbox item of project `project`'s error `fingerprint` in pane
/// `pane_id`.
pub fn service_item_id(project: &str, pane_id: &str, fingerprint: &str) -> ItemId {
    ItemId(format!("service:{project}|{pane_id}|{fingerprint}"))
}

/// What an error a service printed (a stop) can have done to it.
const SERVICE: [ItemAction; 5] = [
    ItemAction::GoTo,
    ItemAction::Snooze,
    ItemAction::SnoozeLong,
    ItemAction::Dismiss,
    ItemAction::Ignore,
];

/// What an ignored error can have done to it.
const IGNORED: [ItemAction; 3] = [ItemAction::GoTo, ItemAction::Unignore, ItemAction::Forget];

/// `item`'s inbox item, ignored (`ignored`) or not.
fn service_item(item: &ServiceItem, now_secs: u64, ignored: bool) -> InboxItem {
    let group = &item.group;
    let times = match group.count {
        1 => String::new(),
        n => format!(" · ×{n}"),
    };
    let (section, actions) = match ignored {
        true => (Section::Ignored, IGNORED.to_vec()),
        // Every stop shares a fingerprint: ignoring one would ignore all.
        false if group.is_stop() => (Section::Services, TRIAGE.to_vec()),
        false => (Section::Services, SERVICE.to_vec()),
    };
    InboxItem {
        id: match ignored {
            true => ignored_item_id(&group.project, &group.fingerprint),
            false => service_item_id(&group.project, &group.pane_id, &group.fingerprint),
        },
        section,
        // Back from a dismissal when it comes again.
        fingerprint: group.count.to_string(),
        workspace: item.workspace,
        title: group.title.clone(),
        subtitle: format!("{} · {}{times}", item.place, item.service),
        tone: Tone::Failed,
        age: Some(Duration::from_secs(
            now_secs.saturating_sub(group.last_secs),
        )),
        target: Target::ServiceError {
            project: group.project.clone(),
            pane_id: group.pane_id.clone(),
            fingerprint: group.fingerprint.clone(),
        },
        actions,
        detail: Detail::Service {
            service: item.service.clone(),
            place: item.place.clone(),
            stopped: group.is_stop(),
            count: group.count,
            first: group.first_secs,
            lines: group.lines.clone(),
            seen: group.seen,
            ignored,
        },
    }
}

/// The inbox item of project `project`'s ignored error `fingerprint`, of
/// whichever pane.
pub fn ignored_item_id(project: &str, fingerprint: &str) -> ItemId {
    ItemId(format!("ignored:{project}|{fingerprint}"))
}

/// The services' errors, each with how many times it came, and the
/// services stopped.
pub fn service_items(src: &Sources) -> Vec<InboxItem> {
    src.service_errors
        .iter()
        .map(|item| service_item(item, src.now_secs, false))
        .collect()
}

/// The services' errors the user ignores, each once.
pub fn ignored_items(src: &Sources) -> Vec<InboxItem> {
    src.ignored_errors
        .iter()
        .map(|item| service_item(item, src.now_secs, true))
        .collect()
}

/// The inbox item of feature `slug`'s gate in project `project`: one per
/// feature, whichever stage waits.
pub fn gate_item_id(project: &str, slug: &str) -> ItemId {
    ItemId(format!("gate:{project}|{slug}"))
}

/// What a gate of `stage` asks the user to review: the file's kind, for
/// the title, and its name.
fn gate_file(stage: Stage) -> (&'static str, Option<&'static str>) {
    match stage {
        Stage::Discover => ("Brief", Some("brief.md")),
        Stage::Plan => ("Plan", Some("plan.md")),
        stage => (stage.label(), None),
    }
}

/// The gates waiting for the user, the oldest first, at `now_secs`: each
/// says which file is ready, in which project, and what Enter does.
pub fn gate_items(gates: &[GateItem], now_secs: u64) -> Vec<InboxItem> {
    let mut gates: Vec<&GateItem> = gates.iter().collect();
    gates.sort_by_key(|g| g.gate.since);
    gates
        .into_iter()
        .map(|item| {
            let gate = &item.gate;
            let (kind, file) = gate_file(gate.stage);
            let review = file.map_or_else(|| "review it".to_owned(), |f| format!("review {f}"));
            InboxItem {
                id: gate_item_id(&gate.project, &gate.slug),
                section: Section::Gates,
                // Back from a dismissal when the next stage waits, or
                // this one again after it was sent back.
                fingerprint: format!("{}|{:?}|{}", gate.slug, gate.stage, gate.since),
                workspace: None,
                title: format!("{kind} ready: “{}”", gate.title),
                subtitle: format!("{} · {review}", item.project),
                tone: Tone::Blocked,
                age: Some(Duration::from_secs(now_secs.saturating_sub(gate.since))),
                target: Target::Gate {
                    project: gate.project.clone(),
                    slug: gate.slug.clone(),
                    stage: gate.stage,
                },
                actions: TRIAGE.to_vec(),
                detail: Detail::Gate {
                    project: item.project.clone(),
                    feature: gate.title.clone(),
                    stage: gate.stage,
                    file,
                    since: gate.since,
                },
            }
        })
        .collect()
}

/// Every item, each once.
pub fn collect(src: &Sources) -> Vec<InboxItem> {
    let mut items = gate_items(src.gates, src.now_secs);
    items.extend(agent_items(src));
    items.extend(service_items(src));
    items.extend(ignored_items(src));
    items.extend(git_items(src));
    items.extend(ci_items(src));
    items.extend(unread_items(src));
    items.extend(ticket_items(src));
    items.extend(issue_items(src));
    items.extend(todo_items(src));
    items
}

/// A round of reading the issues, `read`, taken over `last`: a source
/// that failed keeps what it last read, if anything.
pub fn merge_issues(last: &IssuesRead, read: IssuesRead) -> IssuesRead {
    let github = merge_repos(&last.github, read.github);
    let tickets = merge_repos(&last.tickets, read.tickets);
    let linear = read.linear.map(|result| {
        result.or_else(|err| {
            tracing::debug!("Linear's issues: {err}");
            let old = last.linear.as_ref().and_then(|r| r.clone().ok());
            old.ok_or(err)
        })
    });
    IssuesRead {
        github,
        tickets,
        linear,
    }
}

/// Per repository, what was `read`, over what was `last` read when it
/// failed.
type RepoReads = Vec<(String, Result<Vec<Issue>, String>)>;

fn merge_repos(last: &RepoReads, read: RepoReads) -> RepoReads {
    read.into_iter()
        .map(|(key, result)| {
            let result = result.or_else(|err| {
                tracing::debug!("issues of {key}: {err}");
                let old = last.iter().find(|(k, _)| *k == key);
                old.and_then(|(_, r)| r.clone().ok()).ok_or(err)
            });
            (key, result)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WorkspaceKind,
        ci::{Check, CiRow},
        cursor_agents::ChatRow,
        git::Change,
        inbox_store::{Todo, TodoId},
    };

    /// Everything `Sources` borrows, owned.
    #[derive(Default)]
    struct Fixture {
        workspaces: Vec<WorkspaceView>,
        dirs: Vec<PathBuf>,
        records: Vec<String>,
        active: Option<usize>,
        agents: Vec<Vec<AgentView>>,
        herdr_agents: Vec<AgentInfo>,
        cursor_chats: Vec<CursorChat>,
        agent_since: HashMap<AgentKey, (AgentStatus, Instant)>,
        ci: HashMap<PathBuf, RepoCi>,
        issues: IssuesState,
        tickets: Vec<TicketRepo>,
        store: Option<InboxFile>,
        pane_text: HashMap<String, PaneText>,
        gates: Vec<GateItem>,
    }

    impl Fixture {
        /// Workspace `name` in folder `dir`, of the repository at `git`.
        fn workspace(&mut self, name: &str, dir: &str, git: Option<GitStatus>) {
            let mut view = WorkspaceView::new(name.into(), WorkspaceKind::Project, &[]);
            view.git = git;
            self.workspaces.push(view);
            self.dirs.push(dir.into());
            self.records.push(format!("r-{name}"));
            self.agents.push(Vec::new());
        }

        fn sources(&self) -> Sources<'_> {
            Sources {
                workspaces: &self.workspaces,
                dirs: &self.dirs,
                records: &self.records,
                active: self.active,
                agents: &self.agents,
                herdr_agents: &self.herdr_agents,
                cursor_chats: &self.cursor_chats,
                t3_threads: &[],
                test_runs: &[],
                agent_since: &self.agent_since,
                ci: &self.ci,
                issues: &self.issues,
                tickets: &self.tickets,
                store: self.store.as_ref(),
                pane_text: &self.pane_text,
                service_errors: &[],
                ignored_errors: &[],
                gates: &self.gates,
                now: Instant::now(),
                now_secs: 1_000_000,
            }
        }
    }

    /// A repository at `root`, its worktree's top folder.
    fn git(root: &str) -> GitStatus {
        GitStatus {
            branch: Some("main".into()),
            root: Some(root.into()),
            head: Some("abc".into()),
            ..GitStatus::default()
        }
    }

    fn dirty(root: &str) -> GitStatus {
        GitStatus {
            dirty: true,
            changes: vec![Change {
                path: "src/a.rs".into(),
                orig: None,
                code: 'M',
            }],
            ..git(root)
        }
    }

    fn agent(key: AgentKey, status: AgentStatus) -> AgentView {
        AgentView {
            key,
            dir: None,
            status,
            kind: "claude".into(),
            name: None,
            model: Some("opus".into()),
            lines: Vec::new(),
            closable: false,
            subagents: Vec::new(),
            inbox: false,
            test: None,
            icon: crate::agent_icons::AgentIcon::Claude,
        }
    }

    fn herdr_info(pane_id: &str, seq: u64) -> AgentInfo {
        serde_json::from_value(serde_json::json!({
            "pane_id": pane_id,
            "terminal_id": "t1",
            "workspace_id": "w1",
            "tab_id": "tab1",
            "agent": null,
            "display_agent": null,
            "name": null,
            "agent_session": null,
            "cwd": null,
            "foreground_cwd": null,
            "title": null,
            "terminal_title": null,
            "terminal_title_stripped": null,
            "state_change_seq": seq
        }))
        .expect("an agent with defaults")
    }

    fn row(subject: Subject, state: CiState, group: Group) -> CiRow {
        CiRow {
            link: format!("https://github.com/o/r/{}", ci::subject_label(&subject)),
            subject,
            title: None,
            head: None,
            state,
            checks: vec![Check {
                name: "test".into(),
                state,
                link: None,
            }],
            group,
            watched: false,
        }
    }

    fn repo(rows: Vec<CiRow>) -> RepoCi {
        RepoCi {
            slug: "o/r".into(),
            host: "github.com".into(),
            rows,
            other_prs: 0,
            closed: Vec::new(),
            error: None,
        }
    }

    fn ids(items: &[InboxItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.0.as_str()).collect()
    }

    #[test]
    fn agents_needing_a_look_once_each() {
        let mut f = Fixture::default();
        f.workspace("a", "/a", None);
        f.workspace("b", "/b", None);
        let p1 = AgentKey::Herdr("p1".into());
        f.agents[0] = vec![
            agent(p1.clone(), AgentStatus::Blocked),
            agent(AgentKey::Herdr("p2".into()), AgentStatus::Idle),
        ];
        f.agents[1] = vec![
            agent(p1.clone(), AgentStatus::Blocked),
            agent(AgentKey::Cursor("c1".into()), AgentStatus::Done),
        ];
        f.herdr_agents = vec![herdr_info("p1", 7)];
        f.cursor_chats = vec![CursorChat {
            row: ChatRow {
                id: "c1".into(),
                updated_ms: 42,
                ..ChatRow::default()
            },
            status: AgentStatus::Done,
            model: None,
            subagents: Vec::new(),
        }];
        f.pane_text.insert(
            "p1".into(),
            PaneText {
                text: "last line".into(),
                at: Instant::now(),
            },
        );
        let items = agent_items(&f.sources());
        assert_eq!(ids(&items), ["agent:p1", "agent:cursor:c1"]);
        assert_eq!(items[0].fingerprint, "blocked#7");
        assert_eq!(items[0].tone, Tone::Blocked);
        assert_eq!(items[0].workspace, Some(0));
        assert_eq!(items[0].target, Target::Agent(p1));
        let Detail::Agent { pane, model, .. } = &items[0].detail else {
            panic!("an agent's detail");
        };
        assert_eq!(pane.as_deref(), Some("last line"));
        assert_eq!(model.as_deref(), Some("opus"));
        assert_eq!(items[1].fingerprint, "done#42");
        assert_eq!(items[1].workspace, Some(1));
    }

    #[test]
    fn agent_fingerprints_follow_their_state() {
        let mut f = Fixture::default();
        f.workspace("a", "/a", None);
        f.agents[0] = vec![agent(AgentKey::Herdr("p1".into()), AgentStatus::Done)];
        f.herdr_agents = vec![herdr_info("p1", 3)];
        let before = agent_items(&f.sources());
        assert_eq!(before, agent_items(&f.sources()));
        f.herdr_agents = vec![herdr_info("p1", 4)];
        let after = agent_items(&f.sources());
        assert_ne!(before[0].fingerprint, after[0].fingerprint);
        // The sidebar's check of a card agrees with the inbox's item.
        let print = agent_fingerprint(&f.agents[0][0], &f.herdr_agents, &f.cursor_chats, &[], &[]);
        assert_eq!(print.as_ref(), Some(&after[0].fingerprint));
    }

    #[test]
    fn a_repository_shared_by_workspaces_is_one_item() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(dirty("/r")));
        f.workspace("b", "/r/src", Some(dirty("/r")));
        f.workspace("c", "/clean", Some(git("/clean")));
        f.active = Some(1);
        let items = git_items(&f.sources());
        assert_eq!(ids(&items), ["git:/r"]);
        // The active one of them.
        assert_eq!(items[0].workspace, Some(1));
        assert_eq!(items[0].target, Target::Workspace(1));
        let Detail::Git { changes, .. } = &items[0].detail else {
            panic!("a Git detail");
        };
        assert_eq!(changes, &[('M', "src/a.rs".to_owned())]);
    }

    #[test]
    fn git_needs_a_look_for_any_mark() {
        let mut f = Fixture::default();
        f.workspace(
            "ahead",
            "/1",
            Some(GitStatus {
                ahead: 2,
                ..git("/1")
            }),
        );
        f.workspace(
            "behind",
            "/2",
            Some(GitStatus {
                behind: 1,
                ..git("/2")
            }),
        );
        f.workspace(
            "stash",
            "/3",
            Some(GitStatus {
                stashes: 1,
                ..git("/3")
            }),
        );
        let op = Some(InProgress::Rebase { step: 2, total: 5 });
        f.workspace("op", "/4", Some(GitStatus { op, ..git("/4") }));
        f.workspace("none", "/5", None);
        let items = git_items(&f.sources());
        assert_eq!(ids(&items), ["git:/1", "git:/2", "git:/3", "git:/4"]);
        assert!(
            items[3].subtitle.ends_with("rebasing 2/5"),
            "{}",
            items[3].subtitle
        );
        // Marked as the repositories panel marks them, not in words.
        assert!(!items[0].subtitle.contains('↑'), "{}", items[0].subtitle);
        let marks = |i: usize| items[i].marks().unwrap();
        assert_eq!(marks(0).ahead, 2);
        assert_eq!(marks(1).behind, 1);
        assert_eq!(marks(2).stashes, 1);
        assert!(marks(3).stopped);
        assert!(!marks(0).stopped && !marks(0).dirty);
    }

    #[test]
    fn git_fingerprints_are_stable_and_follow_changes() {
        let a = git_fingerprint(&dirty("/r"));
        assert_eq!(a, git_fingerprint(&dirty("/r")));
        let mut more = dirty("/r");
        more.changes.push(Change {
            path: "b".into(),
            orig: None,
            code: '?',
        });
        assert_ne!(a, git_fingerprint(&more));
        let moved = GitStatus {
            head: Some("def".into()),
            ..dirty("/r")
        };
        assert_ne!(a, git_fingerprint(&moved));
        // Not seeded: the same in every run.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn worktrees_needing_a_look_once_each() {
        let worktree = |path: &str, merged: bool, dirty: bool| Worktree {
            path: path.into(),
            head: Some("h".into()),
            branch: Some(path.rsplit('/').next().unwrap_or_default().into()),
            merged,
            dirty,
            ..Worktree::default()
        };
        let wts = vec![
            worktree("/r/.wt/wip", false, false),
            worktree("/r/.wt/done", true, false),
            worktree("/r/.wt/open", true, true),
        ];
        let mut f = Fixture::default();
        let shared = GitStatus {
            worktrees: wts.clone(),
            ..git("/r")
        };
        f.workspace("a", "/r", Some(shared.clone()));
        f.workspace("b", "/r", Some(shared));
        // A workspace in a worktree with changes: listed for its folder.
        f.workspace("c", "/r/.wt/open", Some(dirty("/r/.wt/open")));
        let items = git_items(&f.sources());
        assert_eq!(ids(&items), ["git:/r/.wt/open", "worktree:/r/.wt/wip"]);
        let wip = &items[1];
        assert_eq!(wip.fingerprint, "unmerged:false:h");
        assert_eq!(wip.target, Target::Worktree("/r/.wt/wip".into()));
        assert_eq!(wip.workspace, Some(0));
        assert_eq!(wip.title, "wip");
    }

    #[test]
    fn ci_lists_failures_and_reviews() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.workspace("b", "/r2", Some(git("/r2")));
        let rows = vec![
            row(
                Subject::Branch("main".into()),
                CiState::Failed,
                Group::Branch,
            ),
            row(
                Subject::Branch("dev".into()),
                CiState::Passed,
                Group::Branch,
            ),
            row(Subject::Pr(3), CiState::Running, Group::Mine),
            row(Subject::Pr(4), CiState::Failed, Group::Mine),
            row(Subject::Pr(5), CiState::Passed, Group::Review),
        ];
        f.ci.insert("/r".into(), repo(rows.clone()));
        // Another worktree of the same repository: listed once.
        f.ci.insert("/r2".into(), repo(rows.clone()));
        // A repository no workspace is in.
        f.ci.insert(
            "/gone".into(),
            RepoCi {
                slug: "o/gone".into(),
                ..repo(rows)
            },
        );
        let items = ci_items(&f.sources());
        assert_eq!(
            ids(&items),
            [
                "ci:github.com/o/r:main",
                "ci:github.com/o/r:#4",
                "ci:github.com/o/r:#5"
            ]
        );
        let tones: Vec<_> = items.iter().map(|i| i.tone).collect();
        assert_eq!(tones, [Tone::Failed, Tone::Failed, Tone::Review]);
        assert_eq!(items[0].fingerprint, "Failed:https://github.com/o/r/main");
        assert_eq!(
            items[2].target,
            Target::Page {
                workspace: 0,
                url: "https://github.com/o/r/#5".into()
            }
        );
        assert!(items[0].actions.contains(&ItemAction::OpenLink));
        assert!(items[0].actions.contains(&ItemAction::CopyLink));
    }

    #[test]
    fn the_inbox_button_counts_agents_done_or_blocked_reviews_and_long_lived_failures() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.agents[0] = vec![
            agent(AgentKey::Herdr("p1".into()), AgentStatus::Blocked),
            agent(AgentKey::Herdr("p2".into()), AgentStatus::Done),
            agent(AgentKey::Herdr("p3".into()), AgentStatus::Working),
        ];
        let failed = |name: &str| row(Subject::Branch(name.into()), CiState::Failed, Group::Branch);
        f.ci.insert(
            "/r".into(),
            repo(vec![
                failed("main"),
                failed("master"),
                failed("dev"),
                failed("trunk"),
                row(Subject::Pr(4), CiState::Failed, Group::Mine),
                row(Subject::Pr(5), CiState::Passed, Group::Review),
                row(Subject::Pr(6), CiState::Failed, Group::Review),
            ]),
        );
        let counted: Vec<_> = collect(&f.sources())
            .into_iter()
            .filter(crate::inbox::counted)
            .map(|i| i.id.0)
            .collect();
        assert_eq!(
            counted,
            [
                "agent:p1",
                "agent:p2",
                "ci:github.com/o/r:main",
                "ci:github.com/o/r:master",
                "ci:github.com/o/r:dev",
                "ci:github.com/o/r:#5",
                "ci:github.com/o/r:#6",
            ]
        );
    }

    fn gate_of(project: &str, slug: &str, stage: Stage, since: u64) -> GateItem {
        GateItem {
            gate: Gate {
                project: project.into(),
                slug: slug.into(),
                title: format!("{slug} title"),
                stage,
                since,
            },
            project: format!("{project} name"),
        }
    }

    #[test]
    fn gates_say_what_is_ready_where_the_oldest_first() {
        let gates = [
            gate_of("p2", "export", Stage::Plan, 999_000),
            gate_of("p1", "log-in", Stage::Discover, 990_000),
        ];
        let items = gate_items(&gates, 1_000_000);
        let lines: Vec<_> = items
            .iter()
            .map(|i| (i.title.as_str(), i.subtitle.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                ("Brief ready: “log-in title”", "p1 name · review brief.md"),
                ("Plan ready: “export title”", "p2 name · review plan.md"),
            ]
        );
        let gate = &items[0];
        assert_eq!(gate.id, gate_item_id("p1", "log-in"));
        assert_eq!(gate.section, Section::Gates);
        assert_eq!(gate.tone, Tone::Blocked);
        assert_eq!(gate.workspace, None);
        assert_eq!(gate.age, Some(Duration::from_secs(10_000)));
        assert_eq!(gate.actions, TRIAGE);
        assert_eq!(
            gate.target,
            Target::Gate {
                project: "p1".into(),
                slug: "log-in".into(),
                stage: Stage::Discover,
            }
        );
        // The next stage, or the same one ready again, brings back a
        // dismissed gate: same id, another fingerprint.
        let plan = gate_items(&[gate_of("p1", "log-in", Stage::Plan, 990_000)], 1_000_000);
        let again = gate_items(
            &[gate_of("p1", "log-in", Stage::Discover, 995_000)],
            1_000_000,
        );
        for other in [&plan[0], &again[0]] {
            assert_eq!(other.id, gate.id);
            assert_ne!(other.fingerprint, gate.fingerprint);
        }
    }

    #[test]
    fn gates_come_first_and_count_on_the_inbox_button() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.agents[0] = vec![agent(AgentKey::Herdr("p1".into()), AgentStatus::Blocked)];
        f.gates = vec![gate_of("p", "log-in", Stage::Discover, 999_000)];
        let items = collect(&f.sources());
        assert_eq!(items[0].section, Section::Gates);
        let counted: Vec<_> = items
            .into_iter()
            .filter(crate::inbox::counted)
            .map(|i| i.id.0)
            .collect();
        assert_eq!(counted, ["gate:p|log-in", "agent:p1"]);
    }

    fn issue(source: IssueSource, id: &str) -> Issue {
        Issue {
            source,
            id: id.into(),
            key: "#1".into(),
            title: "Fix it".into(),
            url: format!("https://example.com/{id}"),
            state: "open".into(),
            labels: Vec::new(),
            updated: "2026-10-01T00:00:00Z".into(),
            place: "o/r".into(),
        }
    }

    #[test]
    fn issues_of_the_project_and_linear() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.ci.insert("/r".into(), repo(Vec::new()));
        f.issues.last = IssuesRead {
            github: vec![
                (
                    "github.com/o/r".into(),
                    Ok(vec![issue(IssueSource::GitHub, "github.com/o/r#1")]),
                ),
                (
                    "github.com/o/other".into(),
                    Ok(vec![issue(IssueSource::GitHub, "github.com/o/other#2")]),
                ),
            ],
            linear: Some(Ok(vec![issue(IssueSource::Linear, "uuid")])),
            ..IssuesRead::default()
        };
        let items = issue_items(&f.sources());
        assert_eq!(ids(&items), ["gh:github.com/o/r#1", "linear:uuid"]);
        assert_eq!(items[0].section, Section::Tickets);
        assert_eq!(items[0].fingerprint, "2026-10-01T00:00:00Z");
        assert_eq!(
            items[0].target,
            Target::Page {
                workspace: 0,
                url: "https://example.com/github.com/o/r#1".into()
            }
        );
        assert_eq!(
            items[1].target,
            Target::Link("https://example.com/uuid".into())
        );
        assert_eq!(items[1].workspace, None);
    }

    #[test]
    fn what_couldnt_be_read_says_why_instead_of_staying_empty() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.ci.insert("/r".into(), repo(Vec::new()));
        f.tickets = vec![TicketRepo {
            repo: "o/t".into(),
            project: None,
        }];
        f.issues.last = IssuesRead {
            github: vec![
                ("github.com/o/r".into(), Err("gh is not installed".into())),
                // Not the project's: nothing about it.
                ("github.com/o/other".into(), Err("HTTP 502".into())),
            ],
            tickets: vec![("github.com/o/t".into(), Err("HTTP 404".into()))],
            linear: Some(Err("Linear: Bad key".into())),
        };
        let items = unread_items(&f.sources());
        assert_eq!(
            ids(&items),
            [
                "unread:gh:github.com/o/r",
                "unread:tickets:github.com/o/t",
                "unread:linear"
            ]
        );
        assert_eq!(items[0].title, "Couldn't load the issues of github.com/o/r");
        assert_eq!(items[0].subtitle, "gh is not installed");
        assert!(items.iter().all(|i| i.section == Section::Tickets));
        assert!(items.iter().all(|i| i.target == Target::Refresh));
        // Read, they go.
        f.issues.last = IssuesRead::default();
        assert!(unread_items(&f.sources()).is_empty());
    }

    #[test]
    fn ci_that_couldnt_be_read_says_why_under_pull_requests() {
        let mut f = Fixture::default();
        f.workspace("a", "/r", Some(git("/r")));
        f.ci.insert(
            "/r".into(),
            RepoCi {
                error: Some("gh: not signed in".into()),
                ..repo(Vec::new())
            },
        );
        let items = ci_items(&f.sources());
        assert_eq!(ids(&items), ["unread:ci:github.com/o/r"]);
        assert_eq!(
            items[0].title,
            "Couldn't load the pull requests of github.com/o/r"
        );
        assert_eq!(items[0].subtitle, "gh: not signed in");
        assert_eq!(items[0].section, Section::PullRequests);
        assert_eq!(items[0].target, Target::Refresh);
        // Read again, it goes.
        f.ci.insert("/r".into(), repo(Vec::new()));
        assert!(ci_items(&f.sources()).is_empty());
    }

    #[test]
    fn todos_open_and_linked_by_record() {
        let mut f = Fixture::default();
        f.workspace("a", "/a", None);
        f.workspace("b", "/b", None);
        let todo = |id: &str, workspace: Option<&str>, done: Option<u64>| Todo {
            id: TodoId(id.into()),
            text: format!("{id}\nmore"),
            workspace: workspace.map(str::to_owned),
            created: 999_000,
            done,
        };
        f.store = Some(InboxFile {
            todos: vec![
                todo("t1", Some("r-b"), None),
                todo("t2", None, Some(5)),
                todo("t3", Some("r-gone"), None),
            ],
            ..InboxFile::default()
        });
        let items = todo_items(&f.sources());
        assert_eq!(ids(&items), ["todo:t1", "todo:t3"]);
        assert_eq!(items[0].workspace, Some(1));
        assert_eq!(items[0].title, "t1");
        assert_eq!(items[0].age, Some(Duration::from_secs(1000)));
        assert_eq!(items[0].target, Target::Todo(TodoId("t1".into())));
        assert_eq!(items[1].workspace, None);
        let Detail::Todo { workspace, .. } = &items[0].detail else {
            panic!("a todo's detail");
        };
        assert_eq!(workspace.as_deref(), Some("b"));
    }

    #[test]
    fn a_failed_read_keeps_what_was_read() {
        let old = vec![issue(IssueSource::GitHub, "github.com/o/r#1")];
        let last = IssuesRead {
            github: vec![("github.com/o/r".into(), Ok(old.clone()))],
            tickets: vec![("github.com/o/t".into(), Ok(old.clone()))],
            linear: Some(Ok(Vec::new())),
        };
        let read = IssuesRead {
            github: vec![
                ("github.com/o/r".into(), Err("offline".into())),
                ("github.com/o/new".into(), Err("offline".into())),
            ],
            tickets: vec![("github.com/o/t".into(), Err("offline".into()))],
            linear: Some(Err("offline".into())),
        };
        let merged = merge_issues(&last, read);
        assert_eq!(merged.github[0].1, Ok(old.clone()));
        assert_eq!(merged.github[1].1, Err("offline".into()));
        assert_eq!(merged.tickets[0].1, Ok(old));
        assert_eq!(merged.linear, Some(Ok(Vec::new())));
    }

    #[test]
    fn tickets_keep_their_board_order() {
        let mut f = Fixture::default();
        // The ticket repository is a workspace too: its issues show once,
        // as tickets.
        f.workspace("t", "/t", Some(git("/t")));
        let mut ci = repo(Vec::new());
        ci.slug = "o/t".into();
        f.ci.insert("/t".into(), ci);
        f.tickets = vec![TicketRepo {
            repo: "o/t".into(),
            project: None,
        }];
        let ticket = |n: u64| issue(IssueSource::GitHub, &format!("github.com/o/t#{n}"));
        f.issues.last = IssuesRead {
            github: vec![("github.com/o/t".into(), Ok(vec![ticket(1)]))],
            tickets: vec![("github.com/o/t".into(), Ok(vec![ticket(3), ticket(1)]))],
            ..IssuesRead::default()
        };
        let items = collect(&f.sources());
        assert_eq!(ids(&items), ["gh:github.com/o/t#3", "gh:github.com/o/t#1"]);
        assert!(items.iter().all(|i| i.section == Section::Tickets));
        assert_eq!(items[0].workspace, None);
        assert_eq!(
            items[0].target,
            Target::Link("https://example.com/github.com/o/t#3".into())
        );
        // The list keeps the board's order, not the read's.
        let ordered = crate::inbox::order(&items);
        assert_eq!(
            ids(&ordered),
            ["gh:github.com/o/t#3", "gh:github.com/o/t#1"]
        );
        // A project without the repository shows none of them.
        f.tickets.clear();
        assert!(ticket_items(&f.sources()).is_empty());
    }
}
