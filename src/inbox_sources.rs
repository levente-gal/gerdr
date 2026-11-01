//! The inbox's items, gathered from what gerdr already shows: the agent
//! cards, the workspaces' Git status and worktrees, the CI cards, the
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
    git::{GitStatus, InProgress, Worktree},
    herdr::{AgentInfo, AgentStatus},
    inbox::{Detail, InboxItem, ItemAction, ItemId, PaneText, Section, Target, Tone},
    inbox_store::InboxFile,
    issues::{Issue, IssueSource, IssuesRead, IssuesState, TicketRepo},
    services::ErrorGroup,
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
/// state changes (a Cursor chat's last update, a test run's process), so
/// it changes whenever the agent moves on. `None` for what is not an agent
/// (a CI card).
pub fn agent_fingerprint(
    view: &AgentView,
    herdr_agents: &[AgentInfo],
    cursor_chats: &[CursorChat],
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
            let Some(fingerprint) =
                agent_fingerprint(view, src.herdr_agents, src.cursor_chats, src.test_runs)
            else {
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
        items.push(InboxItem {
            id: ItemId(format!("git:{}", root.display())),
            section: Section::Repos,
            fingerprint: git_fingerprint(git),
            workspace: Some(w),
            title: folder,
            subtitle: format!("{} · {branch} · {}", view.name, git_summary(git)),
            tone: Tone::Attention,
            age: None,
            target: Target::Workspace(w),
            actions: TRIAGE.to_vec(),
            detail: Detail::Git {
                branch: git.branch.clone(),
                upstream: git.upstream.clone(),
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
        let why = match (wt.attention_tag(), wt.dirty) {
            (Some(tag), true) => format!("{tag}, changes"),
            (Some(tag), false) => tag.to_owned(),
            (None, _) => "changes".to_owned(),
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
            subtitle: format!("{} · worktree · {why}", src.name(w)),
            tone: Tone::Attention,
            age: None,
            target: Target::Worktree(wt.path.clone()),
            actions: TRIAGE.to_vec(),
            detail: Detail::Git {
                branch: wt.branch.clone(),
                upstream: None,
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

/// What `git` needs a look for, in short: `3 changes, 2 to push`.
fn git_summary(git: &GitStatus) -> String {
    let count = |n: usize, word: &str, plural: &str| match n {
        1 => format!("1 {word}"),
        n => format!("{n} {plural}"),
    };
    let mut parts = Vec::new();
    if git.dirty {
        parts.push(count(git.changes.len().max(1), "change", "changes"));
    }
    if git.ahead > 0 {
        parts.push(format!("{} to push", git.ahead));
    }
    if git.behind > 0 {
        parts.push(format!("{} to pull", git.behind));
    }
    if git.stashes > 0 {
        parts.push(count(git.stashes as usize, "stash", "stashes"));
    }
    parts.extend(git.op.map(op_label));
    parts.join(", ")
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
/// under Pull requests, a branch's under Repos.
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

/// The services' errors, each with how many times it came, and the
/// services stopped.
pub fn service_items(src: &Sources) -> Vec<InboxItem> {
    src.service_errors
        .iter()
        .map(|item| {
            let group = &item.group;
            let times = match group.count {
                1 => String::new(),
                n => format!(" · ×{n}"),
            };
            InboxItem {
                id: ItemId(format!(
                    "service:{}|{}|{}",
                    group.project, group.pane_id, group.fingerprint
                )),
                section: Section::Services,
                // Back from a dismissal when it comes again.
                fingerprint: group.count.to_string(),
                workspace: item.workspace,
                title: group.title.clone(),
                subtitle: format!("{} · {}{times}", item.place, item.service),
                tone: Tone::Failed,
                age: Some(Duration::from_secs(
                    src.now_secs.saturating_sub(group.last_secs),
                )),
                target: Target::ServiceError {
                    project: group.project.clone(),
                    pane_id: group.pane_id.clone(),
                    fingerprint: group.fingerprint.clone(),
                },
                actions: TRIAGE.to_vec(),
                detail: Detail::Service {
                    service: item.service.clone(),
                    place: item.place.clone(),
                    stopped: group.is_stop(),
                    count: group.count,
                    first: group.first_secs,
                    lines: group.lines.clone(),
                },
            }
        })
        .collect()
}

/// Every item, each once.
pub fn collect(src: &Sources) -> Vec<InboxItem> {
    let mut items = agent_items(src);
    items.extend(service_items(src));
    items.extend(git_items(src));
    items.extend(ci_items(src));
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
                test_runs: &[],
                agent_since: &self.agent_since,
                ci: &self.ci,
                issues: &self.issues,
                tickets: &self.tickets,
                store: self.store.as_ref(),
                pane_text: &self.pane_text,
                service_errors: &[],
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
        let print = agent_fingerprint(&f.agents[0][0], &f.herdr_agents, &f.cursor_chats, &[]);
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
