//! The repositories panel in gerdr (see `crate::repos`): what it lists, the
//! threads running its batches and the checks before them, the diffs and
//! logs it shows, and what its answers do: open conflicts in the
//! repository's Zed, run a command that wants a login in a terminal, show
//! the workspace where a branch is out already.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    time::{Duration, SystemTime},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point, Size},
};

use crate::menu::{MenuAction, MenuItem, MenuSpec};
use crate::{
    Drag, Gerdr,
    chrome::{
        self, Density, DetailTab, Hint, RepoMark, ReposButton, ReposHit, ReposView, SearchPart,
        Tone, TreeLine, TreeTool,
    },
    desktop,
    diff_edit::{self, Cut, Done, FileEdit, Put},
    diff_view::Position,
    git::{Change, GitStatus, InProgress},
    git_ops::{self, Failed, Failure, Head, Integrate, OpResult, Pull, Ran, Turn, Verdict},
    repos::{
        Batch, BatchKind, Ctx, Found, Job, MarkAt, MarkCommits, Outcome, Panel, RepoEvent,
        RepoItem, RepoRun, Repos, RunState,
    },
    review,
    toast::Level,
    tooltip::Tooltip,
};

/// How many repositories a batch works in at once.
const BATCH_THREADS: usize = 4;
/// How many commits a mark's tooltip lists.
const MARK_COMMITS: usize = 5;
/// How many cuts made from the diff can be undone.
const CUTS: usize = 100;

/// A batch that took this long says it is done in a toast, panel open
/// or not.
const SAY_DONE_AFTER: Duration = Duration::from_secs(5);

/// Opening the panel fetches the repositories last fetched longer ago.
const FETCH_AGE: Duration = Duration::from_secs(5 * 60);

/// What a batch's thread needs for its part in one repository.
struct Task {
    batch: u64,
    kind: BatchKind,
    job: Job,
    remote: Option<String>,
    /// What merges and rebases bring in: `origin/main`, else a local
    /// `main`.
    base: Option<String>,
    op: Option<InProgress>,
    /// For an undo: where the batch undone found it, and left it, and
    /// whether to keep what its commits changed (it committed).
    undo: Option<(Head, Head, bool)>,
    /// The git folder the repository's worktrees share: those of one
    /// repository take turns.
    common: Option<PathBuf>,
}

/// A batch's tasks waiting, and the repositories being worked in, by
/// shared git folder: a repository's worktrees take turns, since at once
/// they'd fight over its refs, its stash and its fetches.
struct Queue {
    tasks: VecDeque<Task>,
    busy: Vec<PathBuf>,
}

impl Queue {
    /// The next task whose repository is free, waiting while every task
    /// left is in a busy one; `None` once none are left.
    fn next(queue: &Mutex<Queue>) -> Option<Task> {
        loop {
            {
                let mut q = queue.lock().ok()?;
                if q.tasks.is_empty() {
                    return None;
                }
                let free = q
                    .tasks
                    .iter()
                    .position(|t| t.common.as_ref().is_none_or(|c| !q.busy.contains(c)));
                if let Some(k) = free {
                    let task = q.tasks.remove(k)?;
                    q.busy.extend(task.common.clone());
                    return Some(task);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `task` is done: its repository is free again.
    fn done(queue: &Mutex<Queue>, task: &Task) {
        if let (Ok(mut q), Some(common)) = (queue.lock(), &task.common) {
            q.busy.retain(|c| c != common);
        }
    }
}

/// Not tried, and why.
fn blocked(why: &str) -> Failed {
    Failed {
        failure: Failure::Blocked(why.to_owned()),
        why: why.to_owned(),
        log: Vec::new(),
    }
}

/// `then` after the commands in `log`: both in the log, whatever `then`
/// came to.
fn chain(log: Vec<Ran>, then: OpResult) -> OpResult {
    match then {
        Ok(more) => Ok([log, more].concat()),
        Err(mut failed) => {
            failed.log = [log, failed.log].concat();
            Err(failed)
        }
    }
}

/// The branch new commits are merged or rebased in from.
fn base_of(status: &GitStatus) -> Option<String> {
    let default = status.default_branch.as_ref()?;
    Some(match &status.remote {
        Some(remote) => format!("{remote}/{default}"),
        None => default.clone(),
    })
}

/// What `op` is called in a sentence: "a merge".
fn op_name(op: InProgress) -> &'static str {
    match op {
        InProgress::Merge => "a merge",
        InProgress::Rebase { .. } => "a rebase",
        InProgress::CherryPick => "a cherry-pick",
        InProgress::Revert => "a revert",
    }
}

/// Do `task`'s part in its repository, telling `step` what it moves on to.
fn perform(task: &Task, step: &dyn Fn(&str)) -> OpResult {
    let dir = task.job.root.as_path();
    let need = |v: &Option<String>, why: &str| v.clone().ok_or_else(|| blocked(why));
    match &task.kind {
        BatchKind::Branch(name) => {
            step("switching…");
            git_ops::switch_or_create(dir, name)
        }
        BatchKind::Switch(names) => {
            let name = git_ops::branch_named(dir, names, task.remote.as_deref())
                .ok_or_else(|| blocked("no such branch here"))?;
            step("switching…");
            git_ops::switch_or_create(dir, &name)
        }
        BatchKind::DeleteBranch { names, force } => {
            let local = local_names(dir, names);
            if local.is_empty() {
                return Err(blocked("no such local branch here"));
            }
            step("deleting…");
            let mut log = Vec::new();
            for name in local {
                log = chain(log, git_ops::delete_branch(dir, name, *force))?;
            }
            Ok(log)
        }
        // A checked repository with nothing to commit is only pushed.
        BatchKind::Commit { push: true, .. } if task.job.files.is_empty() => {
            step("pushing…");
            git_ops::push(dir, task.remote.as_deref(), false)
        }
        BatchKind::Commit { msg, push } => {
            step("committing…");
            let log = git_ops::commit_paths(dir, &task.job.files, msg)?;
            if !push {
                return Ok(log);
            }
            step("pushing…");
            chain(log, git_ops::push(dir, task.remote.as_deref(), false))
        }
        BatchKind::Amend { msg, .. } => {
            step("amending…");
            git_ops::amend_paths(dir, &task.job.files, msg.as_deref())
        }
        BatchKind::Push { force } => {
            step("pushing…");
            git_ops::push(dir, task.remote.as_deref(), *force)
        }
        BatchKind::Fetch => {
            step("fetching…");
            git_ops::fetch_all(dir)
        }
        BatchKind::Pull(how) => {
            step("pulling…");
            git_ops::pull(dir, *how)
        }
        BatchKind::Integrate { how, from } => {
            if from.is_none() {
                need(&task.base, "no default branch")?;
            }
            let log = match task.remote {
                Some(_) => {
                    step("fetching…");
                    git_ops::fetch_all(dir)?
                }
                None => Vec::new(),
            };
            // A branch picked is found after the fetch, which may bring it.
            let base = match from {
                None => task.base.clone(),
                Some(names) => git_ops::source_of(dir, names, task.remote.as_deref()),
            };
            let Some(base) = base else {
                return chain(log, Err(blocked("no such branch here")));
            };
            step(match how {
                Integrate::Merge => "merging…",
                Integrate::Rebase => "rebasing…",
            });
            chain(log, git_ops::integrate(dir, &base, *how))
        }
        BatchKind::MarkResolved => {
            let mut log = Vec::new();
            for file in &task.job.files {
                log = chain(log, git_ops::mark_resolved(dir, file))?;
            }
            Ok(log)
        }
        // Without an operation stopped half-way, conflicts are left to
        // the user.
        BatchKind::Continue => {
            step("continuing…");
            match task.op {
                Some(op) => git_ops::continue_op(dir, op),
                None => Err(blocked("nothing stopped half-way here")),
            }
        }
        BatchKind::Abort => {
            step("aborting…");
            match task.op {
                Some(op) => git_ops::abort_op(dir, op),
                None => Err(blocked("nothing stopped half-way here")),
            }
        }
        BatchKind::Undo => {
            let Some((before, after, soft)) = &task.undo else {
                return Err(blocked("nothing to undo"));
            };
            step("undoing…");
            git_ops::undo(dir, before, after, *soft)
        }
        BatchKind::Stash => {
            step("stashing…");
            git_ops::stash_push(dir)
        }
        BatchKind::StashPop(n) => {
            step("popping the stash…");
            git_ops::stash_pop(dir, *n)
        }
        BatchKind::StashDrop(n) => {
            step("dropping the stash…");
            git_ops::stash_drop(dir, *n)
        }
        BatchKind::Discard => {
            step("discarding…");
            git_ops::discard(dir, &task.job.files)
        }
    }
}

/// What the check before `kind` finds in the repository at `root`.
fn verdict(kind: &BatchKind, root: &Path, status: &GitStatus) -> Verdict {
    if let Some(op) = status.op {
        return Verdict::Blocked(format!("in the middle of {}", op_name(op)));
    }
    match kind {
        BatchKind::Switch(names) => {
            match git_ops::branch_named(root, names, status.remote.as_deref()) {
                Some(name) => verdict(&BatchKind::Branch(name), root, status),
                None => Verdict::Blocked("no such branch here".into()),
            }
        }
        BatchKind::Integrate { how, from } => {
            let base = match from {
                None => base_of(status).ok_or("no default branch"),
                Some(names) => git_ops::source_of(root, names, status.remote.as_deref())
                    .ok_or("no such branch here"),
            };
            let base = match base {
                Ok(base) => base,
                Err(why) => return Verdict::Blocked(why.into()),
            };
            if status.head.is_none() {
                return Verdict::Blocked("no commits yet".into());
            }
            if *how == Integrate::Rebase && tracked_changes(status) {
                return Verdict::Blocked(UNCOMMITTED.into());
            }
            git_ops::predict_merge(root, "HEAD", &base)
        }
        BatchKind::Pull(how) => {
            let Some(upstream) = &status.upstream else {
                return Verdict::Blocked("no upstream".into());
            };
            if *how == Pull::Rebase && tracked_changes(status) {
                return Verdict::Blocked(UNCOMMITTED.into());
            }
            git_ops::predict_merge(root, "HEAD", upstream)
        }
        BatchKind::Branch(name) => {
            let local = format!("refs/heads/{name}");
            let target = if git_ops::has_ref(root, &local) {
                Some(name.clone())
            } else {
                status
                    .remote
                    .as_ref()
                    .map(|remote| format!("{remote}/{name}"))
                    .filter(|t| git_ops::has_ref(root, &format!("refs/remotes/{t}")))
            };
            // A new branch from HEAD takes the changes as they are.
            let Some(target) = target else {
                return Verdict::Clean;
            };
            let dirty: Vec<String> = status.changes.iter().map(|c| c.path.clone()).collect();
            let blockers = git_ops::switch_blockers(root, &target, &dirty);
            if blockers.is_empty() {
                return Verdict::Clean;
            }
            Verdict::Blocked(format!(
                "{} differ on {target}: commit or stash them first",
                files(&blockers)
            ))
        }
        BatchKind::DeleteBranch { names, .. } => {
            let local = local_names(root, names);
            if local.is_empty() {
                return Verdict::Blocked("no such local branch here".into());
            }
            if status.branch.as_ref().is_some_and(|b| names.contains(b)) {
                return Verdict::Blocked("checked out here: switch away first".into());
            }
            let lost = local
                .iter()
                .map(|name| git_ops::unmerged_commits(root, name))
                .sum::<Option<usize>>();
            match lost {
                Some(0) => Verdict::Clean,
                Some(n) => Verdict::Unmerged(n),
                None => Verdict::Blocked("can't tell what it would lose".into()),
            }
        }
        _ => Verdict::Clean,
    }
}

/// Of `names`, those the repository at `dir` has as local branches.
fn local_names<'a>(dir: &Path, names: &'a [String]) -> Vec<&'a String> {
    names
        .iter()
        .filter(|name| git_ops::has_ref(dir, &format!("refs/heads/{name}")))
        .collect()
}

/// Why a rebase isn't tried: git won't over uncommitted changes, and the
/// panel doesn't stash them around it (see `git_ops::pull`).
const UNCOMMITTED: &str = "uncommitted changes: commit or stash them first";

/// Whether a tracked file is changed: untracked ones don't stop a rebase.
fn tracked_changes(status: &GitStatus) -> bool {
    status.changes.iter().any(|c| c.code != '?')
}

/// Whether the check before `kind` fetches first, to judge against what
/// the batch will bring in.
fn fetches_first(kind: &BatchKind) -> bool {
    matches!(kind, BatchKind::Integrate { .. } | BatchKind::Pull(_))
}

/// Of `roots` with their shared git folders, one per repository, with
/// what its turn is taken by (`git_ops::Turn`).
fn one_per_repository(roots: Vec<(PathBuf, Option<PathBuf>)>) -> Vec<(PathBuf, PathBuf)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (root, common) in roots {
        if common.as_ref().is_some_and(|c| seen.contains(c)) {
            continue;
        }
        seen.extend(common.clone());
        let key = common.unwrap_or_else(|| root.clone());
        out.push((root, key));
    }
    out
}

/// `fetch --all` in `root` in its repository's turn (`key`), waiting for
/// what runs there first.
fn fetch_in_turn(root: &Path, key: &Path) -> OpResult {
    let _turn = Turn::take(key, "a fetch");
    git_ops::fetch_all(root)
}

/// A few file names, then how many more.
fn files(paths: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut text = paths
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOWN {
        text.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    text
}

/// What the repositories button shows of the project's repositories, each
/// counted once however many checkouts share it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepoCounts {
    /// Repositories with uncommitted changes or stopped half-way (a merge,
    /// rebase or the like, or a conflicted file): the button's badge.
    pub changed: usize,
    /// Of those, the ones stopped half-way: they turn the badge red.
    pub stuck: usize,
    /// Repositories with commits to push to their upstream: the ↑ mark.
    pub ahead: usize,
    /// Repositories with commits to pull from their upstream: the ↓ mark.
    pub behind: usize,
}

/// What the repositories button shows of the repositories of `statuses`
/// (see `RepoCounts`).
fn changed_repos<'a>(statuses: impl Iterator<Item = &'a GitStatus>) -> RepoCounts {
    let mut roots: Vec<&Path> = Vec::new();
    let mut counts = RepoCounts::default();
    for status in statuses {
        let Some(root) = status.root.as_deref() else {
            continue;
        };
        if roots.contains(&root) {
            continue;
        }
        roots.push(root);
        let stopped = status.op.is_some() || status.changes.iter().any(Change::conflicted);
        counts.changed += (status.dirty || stopped) as usize;
        counts.stuck += stopped as usize;
        let tracked = status.upstream.is_some();
        counts.ahead += (tracked && status.ahead > 0) as usize;
        counts.behind += (tracked && status.behind > 0) as usize;
    }
    counts
}

/// `n` repositories, in words.
fn repo_count(n: usize) -> String {
    match n {
        1 => "1 repo".into(),
        n => format!("{n} repos"),
    }
}

/// The repositories button's tooltip, saying what `counts` counts.
pub(crate) fn repos_tip(counts: &RepoCounts) -> String {
    let mut parts = Vec::new();
    if counts.changed > 0 {
        parts.push(format!("{} with changes", counts.changed));
    }
    if counts.stuck > 0 {
        parts.push(format!("{} stopped half-way", counts.stuck));
    }
    if counts.ahead > 0 {
        parts.push(format!("{} to push", repo_count(counts.ahead)));
    }
    if counts.behind > 0 {
        parts.push(format!("{} to pull", repo_count(counts.behind)));
    }
    match parts.is_empty() {
        true => "Repositories".to_owned(),
        false => format!("Repositories: {}", parts.join(", ")),
    }
}

/// `n` lines, in words.
fn lines(n: usize) -> String {
    match n {
        1 => "1 line".into(),
        n => format!("{n} lines"),
    }
}

/// What `edit` does to its file, as said once done: `Deleted 2 lines from
/// src/a.rs:40`.
fn cut_said(edit: &FileEdit) -> String {
    let path = &edit.path;
    match &edit.cut {
        Cut::Lines(numbers) => {
            let first = numbers.iter().min().copied().unwrap_or(edit.caret.line);
            format!("Deleted {} from {path}:{first}", lines(numbers.len()))
        }
        Cut::Chars(from, to) if from.line == to.line => {
            let n = to.col.saturating_sub(from.col);
            let chars = if n == 1 { "char" } else { "chars" };
            format!("Deleted {n} {chars} from {path}:{}", from.line)
        }
        Cut::Chars(from, to) => format!(
            "Deleted from {path}:{} to line {}, joining them",
            from.line, to.line
        ),
        Cut::Restore { drop, put } => {
            let n = edit.cut.restored().unwrap_or(0);
            let first = (drop.iter().copied())
                .chain(put.iter().map(|(at, _)| *at))
                .min()
                .unwrap_or(edit.caret.line);
            format!("Restored {} in {path}:{first}", lines(n))
        }
    }
}

/// What pasting `register` as `put` does, as said once done: `Pasted 2
/// lines into src/a.rs:41`.
fn paste_said(put: &Put, register: &diff_edit::Register) -> String {
    let path = &put.path;
    let line = match (register.lines, put.before) {
        (true, false) => put.at.line + 1,
        _ => put.at.line,
    };
    if register.lines {
        let n = register.text.lines().count() * put.count.max(1);
        format!("Pasted {} into {path}:{line}", lines(n))
    } else {
        let n = register.text.chars().count() * put.count.max(1);
        let chars = if n == 1 { "char" } else { "chars" };
        format!("Pasted {n} {chars} into {path}:{line}")
    }
}

fn level(tone: Tone) -> Level {
    match tone {
        Tone::Success => Level::Success,
        Tone::Warning => Level::Warning,
        Tone::Error => Level::Error,
        Tone::Muted | Tone::Normal => Level::Info,
    }
}

impl Gerdr {
    /// The project's repositories: one per checkout among its workspaces,
    /// in the sidebar's order, named after the first workspace in each.
    fn repo_items(&self) -> Vec<RepoItem> {
        let mut items: Vec<RepoItem> = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            let Some(status) = self.git.get(&ws.dir) else {
                continue;
            };
            let Some(root) = status.root.clone() else {
                continue;
            };
            if items.iter().any(|i| i.root == root) {
                continue;
            }
            items.push(RepoItem {
                root,
                label: ws.name.clone(),
                workspace: w,
                status: status.clone(),
            });
        }
        items
    }

    /// What the sidebar's repositories button shows of the project's
    /// repositories: its badge and its ↑/↓ marks.
    pub(crate) fn repos_changed(&self) -> RepoCounts {
        let statuses = self
            .workspaces
            .iter()
            .filter_map(|ws| self.git.get(&ws.dir));
        changed_repos(statuses)
    }

    fn repos_page(&self) -> (usize, usize) {
        chrome::repos_page(self.repos_rect(), self.density)
    }

    /// What the panel's views and keys need of the chrome: the density,
    /// the tree's share of the panel, how many characters a review
    /// comment's row of the diff holds, and how many lines a sheet shows.
    fn repos_env(&self) -> (Density, f64, usize, usize) {
        let share = self.repos.tree_share.unwrap_or(chrome::REPOS_TREE_SHARE);
        let cols = chrome::repos_detail_cols(self.repos_rect(), share);
        let sheet = chrome::repos_sheet_rows(self.repos_rect(), false);
        (self.density, share, cols, sheet)
    }

    /// Open the repositories panel: as it was left, for the same project.
    pub fn open_repos(&mut self) {
        self.switcher = None;
        self.project_menu = None;
        self.worktrees.close();
        self.repos.close_worktree();
        self.inbox.close();
        self.scrollback = None;
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.repos.items = self.repo_items();
        let panel = match self.repos.kept.take() {
            Some((project, panel)) if project == self.project => panel,
            _ => {
                let mut panel = Panel::new(&self.repos.items);
                if let Some(id) = &self.project {
                    panel.diff.review = review::load(None, id);
                }
                panel
            }
        };
        self.repos.panel = Some(panel);
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.grouping = self.projects.repo_grouping;
        }
        self.repos_refresh();
        self.repos_fetch();
        self.refresh_git();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The agent the review goes to, by its pane and what it is called:
    /// one in the workspace of a repository commented on (working in that
    /// repository first), else in the workspace shown.
    fn review_agent(&self) -> Option<(String, String)> {
        let panel = self.repos.panel.as_ref()?;
        let mut roots: Vec<&Path> = Vec::new();
        for comment in &panel.diff.review.comments {
            if !roots.contains(&comment.root.as_path()) {
                roots.push(&comment.root);
            }
        }
        let item_of = |root: &&Path| self.repos.items.iter().find(|i| i.root == *root);
        let mut workspaces: Vec<usize> = roots
            .iter()
            .filter_map(item_of)
            .map(|i| i.workspace)
            .collect();
        workspaces.push(self.active_workspace);
        for w in workspaces {
            let Some(id) = self.workspaces.get(w).and_then(|ws| ws.herdr_id()) else {
                continue;
            };
            let here: Vec<&crate::herdr::AgentInfo> = self
                .agents
                .iter()
                .filter(|a| a.workspace_id == id)
                .collect();
            let in_repo = here.iter().find(|a| {
                let dir = a.foreground_cwd.as_ref().or(a.cwd.as_ref());
                dir.is_some_and(|dir| roots.iter().any(|root| dir.starts_with(root)))
            });
            if let Some(agent) = in_repo.or(here.first()) {
                return Some((agent.pane_id.clone(), agent.label().to_owned()));
            }
        }
        None
    }

    /// Send the review (`text`, `count` comments) to its agent, as typed
    /// there and sent with Enter; said either way.
    fn repos_send_review(&mut self, text: &str, count: usize) {
        let what = if count == 1 {
            "1 review comment".to_owned()
        } else {
            format!("{count} review comments")
        };
        let Some((pane, label)) = self.review_agent() else {
            return self
                .show_warning("No agent runs in this workspace: copy the review instead".into());
        };
        let Some(herdr) = self.herdr.as_mut() else {
            return self.show_error("herdr isn't running: copy the review instead".into());
        };
        match herdr.send_line(&pane, text.trim_end()) {
            Ok(()) => self.show_success(format!("Sent {what} to {label}")),
            Err(err) => self.show_error(format!("Couldn't send the review to {label}: {err}")),
        }
    }

    /// Save the panel's review comments with the project, if they changed.
    fn repos_save_review(&mut self) {
        let Some(panel) = self.repos.panel.as_mut().filter(|p| p.diff.review_dirty) else {
            return;
        };
        panel.diff.review_dirty = false;
        let Some(id) = self.project.as_deref() else {
            return;
        };
        if let Err(err) = review::save(None, id, &panel.diff.review) {
            tracing::warn!("saving the review: {err}");
            self.toast(format!("Couldn't save the review: {err}"), Level::Error);
        }
    }

    /// Close the panel, kept as it is for opening again; the edits made
    /// from its diff can no longer be undone.
    /// The diff a vim edit landed in: the open panel's or popup's, else
    /// the file's tab's.
    fn edited_diff(&mut self) -> Option<&mut crate::diff_view::DiffView> {
        match self.repos.is_open() {
            true => self.repos.diff_mut(),
            false => self.wd_mut().map(|wd| &mut wd.diff),
        }
    }

    fn close_repos(&mut self) {
        // Nothing to close in a file's tab.
        if !self.repos.is_open() {
            return;
        }
        // A commit's diff closes with the panel it was opened over.
        let commit = self.wd().is_some_and(|wd| wd.commit.is_some());
        if self.wd().is_some() {
            self.close_worktree_diff();
            if !commit {
                return self.forget_edits();
            }
        }
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
        self.forget_edits();
        self.refresh_pointer_focus();
    }

    /// Fetch, off the event loop, the repositories with a remote not
    /// fetched lately, so what they are behind by is true.
    fn repos_fetch(&mut self) {
        let now = SystemTime::now();
        let stale = |status: &GitStatus| {
            status.remote.is_some()
                && status
                    .fetched
                    .and_then(|t| now.duration_since(t).ok())
                    .is_none_or(|age| age > FETCH_AGE)
        };
        // Worktrees of one repository fetch once: at once they'd fight over
        // its refs' locks.
        let roots = one_per_repository(
            self.repos
                .items
                .iter()
                .filter(|i| stale(&i.status))
                .map(|i| (i.root.clone(), i.status.common_dir.clone()))
                .collect(),
        );
        // One round at a time: opened again meanwhile, the panel waits for it.
        if roots.is_empty() || self.repos.fetches > 0 {
            return;
        }
        let Some(tx) = self.repos_sender() else {
            return;
        };
        self.repos.fetches += 1;
        std::thread::spawn(move || {
            std::thread::scope(|scope| {
                for (root, key) in &roots {
                    scope.spawn(move || fetch_in_turn(root, key));
                }
            });
            let _ = tx.send(RepoEvent::Fetched);
        });
    }

    /// Where the threads send what they came to; set up the first time.
    pub(crate) fn repos_sender(&mut self) -> Option<channel::Sender<RepoEvent>> {
        if self.repos.done.is_none() {
            let (tx, rx) = channel::channel::<RepoEvent>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.repos_event(event);
                }
            });
            match inserted {
                Ok(_) => self.repos.done = Some(tx),
                Err(err) => {
                    tracing::warn!("repositories panel: {err}");
                    return None;
                }
            }
        }
        self.repos.done.clone()
    }

    /// The panel's state brought in line with the last Git status read,
    /// and the detail it wants read.
    fn repos_refresh(&mut self) {
        let agent = self.review_agent().map(|(_, label)| label);
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.agent = agent;
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        if let Some(panel) = panel.as_mut() {
            let ctx = Ctx {
                items,
                batch: batch.as_ref(),
                page,
                now: SystemTime::now(),
                fetching: *fetches > 0,
                same: same_branches,
                top: top_branches,
                density,
                tree_share,
                cols,
                sheet_rows,
                background,
            };
            panel.refresh(&ctx);
        }
        self.repos_detail();
        self.repos_replay();
    }

    /// The keys typed in the diff while it was read again after a cut,
    /// now that it is (or it no longer will be).
    fn repos_replay(&mut self) {
        let Some(panel) = self.repos.panel.as_mut() else {
            return;
        };
        for (mods, sym, typed) in panel.diff.take_held() {
            if let Some(outcome) = self.repos_key(&mods, sym, typed) {
                self.apply_repos(outcome);
            }
        }
    }

    /// A Git status read came in.
    pub fn repos_read(&mut self) {
        self.worktree_diff_read();
        self.refresh_previews();
        if self.repos.panel.is_none() && self.repos.batch.is_none() {
            return;
        }
        self.repos.items = self.repo_items();
        self.repos_refresh();
    }

    /// Read the diffs, log or stashes the panel wants, off the event loop,
    /// unless they are being read: each repository's diff on its own
    /// thread.
    fn repos_detail(&mut self) {
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Some(panel) = self.repos.panel.as_ref() else {
            return;
        };
        let ctx = Ctx {
            items: &self.repos.items,
            batch: self.repos.batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: self.repos.fetches > 0,
            same: &self.repos.same_branches,
            top: &self.repos.top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background: &self.repos.background,
        };
        let diffs = panel.diffs_wanted(&ctx);
        let key = panel
            .detail_wanted(&ctx)
            .filter(|key| self.repos.reading.as_ref() != Some(key));
        let Some(tx) = self.repos_sender() else {
            return;
        };
        for (root, changes, generation) in diffs {
            // One read at a time per repository: the next once it is in.
            if self.repos.reading_diffs.contains_key(&root) {
                continue;
            }
            self.repos.reading_diffs.insert(root.clone(), generation);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let files = git_ops::repo_diff(&root, &changes);
                let _ = tx.send(RepoEvent::Diffs {
                    root,
                    generation,
                    files,
                });
            });
        }
        let Some(key) = key else {
            return;
        };
        self.repos.reading = Some(key.clone());
        std::thread::spawn(move || {
            let (text, oids) = match key.tab {
                DetailTab::Diff => (String::new(), Vec::new()),
                DetailTab::Log => {
                    let lines = git_ops::log_upstream(&key.root).lines();
                    let text = lines.iter().map(|(l, _)| l.as_str());
                    let text = text.collect::<Vec<_>>().join("\n");
                    (text, lines.into_iter().map(|(_, oid)| oid).collect())
                }
                DetailTab::Stashes => (git_ops::stash_list(&key.root), Vec::new()),
            };
            let _ = tx.send(RepoEvent::Detail { key, text, oids });
        });
    }

    /// A key while the panel has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn repos_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        if self.wd().is_some() {
            return self.worktree_diff_key(mods, sym, typed);
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let outcome = panel.as_mut()?.key(mods, sym, typed, &ctx);
        self.repos_detail();
        self.repos_save_review();
        outcome
    }

    /// A left click while the panel is open: outside it closes it. On the
    /// "more" button, a menu of the actions the footer has no room for;
    /// on the line between the tree and the detail, a drag; twice on a
    /// file, it opens in Zed.
    pub fn repos_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.repos_view() else {
            return;
        };
        // The gap beside the file's preview drags their split.
        if let Some(drag) = self.doc_gap_at(pos) {
            self.drag = Some(drag);
            return;
        }
        let layout = chrome::repos_layout(&view, self.repos_rect());
        let Some(hit) = chrome::repos_hit(&layout, pos) else {
            return self.close_repos();
        };
        match hit {
            ReposHit::Mode(i) => {
                if let Some(mode) = self.doc_mode_at(i) {
                    self.set_doc_mode(mode);
                }
                return;
            }
            ReposHit::More => {
                if let Some(more) = layout.more {
                    let hints: Vec<&Hint> =
                        layout.overflow.iter().map(|&i| &view.hints[i]).collect();
                    let mut spec = repos_menu_spec(&hints, None, more);
                    spec.anchor = above(more, &spec, self.output_size());
                    self.open_menu(spec);
                }
                return;
            }
            ReposHit::Splitter => {
                self.drag = Some(Drag::ReposSplit);
                return;
            }
            _ => {}
        }
        // A press on a line puts the caret on the char there.
        let at = chrome::repos_diff_char(&view, self.repos_rect(), pos);
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.diff.pointer_char = at;
        }
        if let Some(wd) = self.wd_mut() {
            wd.diff.pointer_char = at;
        }
        let double = self.is_double(pos);
        self.repos_hit(hit, double);
    }

    /// A click on `hit` in the panel, the second of a double click when
    /// `double`.
    fn repos_hit(&mut self, hit: ReposHit, double: bool) {
        // Shift+click on a diff row reaches the cursor's range there;
        // Ctrl+click opens the code there in Zed.
        let mods = self.seat.get_keyboard().map(|k| k.modifier_state());
        let shift = mods.is_some_and(|m| m.shift);
        let ctrl = mods.is_some_and(|m| m.ctrl && !m.shift);
        if self.wd().is_some() {
            return self.worktree_diff_hit(hit, double, shift, ctrl);
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let mut drags = false;
        let outcome = panel.as_mut().and_then(|p| {
            if let (ReposHit::DiffRow(i), true) = (hit, ctrl) {
                return p.goto_detail_row(i, &ctx);
            }
            if let (ReposHit::DiffRow(i), true, false) = (hit, shift, double) {
                p.extend_detail_row(i, &ctx);
                drags = p.drags_rows(&ctx);
                return None;
            }
            let outcome = p.click(hit, &ctx);
            drags = matches!(hit, ReposHit::DiffRow(_)) && !double && p.drags_rows(&ctx);
            match hit {
                ReposHit::Line(i) if double && p.is_file_line(i, &ctx) => {
                    p.press(ReposButton::OpenInZed, &ctx)
                }
                ReposHit::DiffRow(i) if double => p.open_detail_row(i, &ctx),
                _ => outcome,
            }
        });
        // Held, the press drags the cursor's range on.
        self.repos_rows_drag = drags;
        self.repos_detail();
        self.repos_save_review();
        if let Some(outcome) = outcome {
            self.apply_repos(outcome);
        }
    }

    /// A right click while the panel is open: on a line, it is selected
    /// and a menu of the actions opens; elsewhere in the panel, the menu
    /// alone.
    pub fn repos_right_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.repos_view() else {
            return;
        };
        let output = self.repos_rect();
        let layout = chrome::repos_layout(&view, output);
        let Some(hit) = chrome::repos_hit(&layout, pos) else {
            return;
        };
        if view.sheet.is_some() || view.branch.menu.is_some() {
            return;
        }
        // A worktree's diff: its footer's actions.
        if self.wd().is_some() {
            let at = chrome::Rect::new((pos.x as i32, pos.y as i32).into(), (1, 1).into());
            let hints: Vec<&Hint> = view.hints.iter().collect();
            let spec = repos_menu_spec(&hints, None, at);
            return self.open_menu(spec);
        }
        let line = match hit {
            ReposHit::Line(i)
            | ReposHit::Check(i)
            | ReposHit::Expander(i)
            | ReposHit::Chip(i)
            | ReposHit::Mark(i, _) => Some(i),
            _ => None,
        };
        if let Some(i) = line {
            self.repos_hit(ReposHit::Line(i), false);
        }
        // The actions for what is selected now.
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, output);
        let anchor = line.and_then(|i| layout.lines.get(i)).map_or_else(
            || chrome::Rect::new((pos.x as i32, pos.y as i32).into(), (1, 1).into()),
            |(rect, ..)| *rect,
        );
        let path = view
            .selected
            .and_then(|_| self.repos.panel.as_ref()?.selected_path());
        // On a file or folder, what acts on it first, then its path.
        let on_file =
            |h: &&Hint| matches!(h.button, ReposButton::OpenInZed | ReposButton::DiscardFile);
        let file: bool = line.is_some() && view.hints.iter().any(|h| on_file(&h));
        let spec = if file {
            let (first, rest): (Vec<&Hint>, Vec<&Hint>) = view.hints.iter().partition(on_file);
            let mut spec = repos_menu_spec(&first, path, anchor);
            spec.items
                .extend(repos_menu_spec(&rest, None, anchor).items);
            spec
        } else {
            let hints: Vec<&Hint> = view.hints.iter().collect();
            repos_menu_spec(&hints, path, anchor)
        };
        self.open_menu(spec);
    }

    /// Do `button`, as a menu asks.
    pub fn repos_press(&mut self, button: ReposButton) {
        if self.wd().is_some() {
            return self.worktree_diff_press(button);
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let outcome = panel.as_mut().and_then(|p| p.press(button, &ctx));
        self.repos_detail();
        self.repos_save_review();
        if let Some(outcome) = outcome {
            self.apply_repos(outcome);
        }
    }

    /// The line between the tree and the detail dragged to `pos`.
    pub fn repos_drag_to(&mut self, pos: Point<f64, Logical>) {
        let share = chrome::repos_share_at(pos.x, self.repos_rect());
        self.repos.tree_share = Some(share);
    }

    /// The pointer dragged to `pos` from a press on the diff: the
    /// cursor's range reaches the row under it, the diff scrolling a row
    /// on when it is above or below the rows.
    pub fn repos_drag_rows(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.repos_view() else {
            self.repos_rows_drag = false;
            return;
        };
        let layout = chrome::repos_layout(&view, self.repos_rect());
        let list = layout.detail_list;
        let by = if pos.y < list.loc.y as f64 {
            -1
        } else if pos.y >= (list.loc.y + list.size.h) as f64 {
            1
        } else {
            0
        };
        let row = layout
            .detail_rows
            .iter()
            .position(|r| (r.loc.y as f64..(r.loc.y + r.size.h) as f64).contains(&pos.y));
        if self.wd().is_some() {
            return self.worktree_diff_drag(by, row);
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let Some(p) = panel.as_mut() else {
            return;
        };
        match (by, row) {
            (0, Some(i)) => p.extend_detail_row(i, &ctx),
            (0, None) => {}
            (by, _) => p.drag_past_rows(by, &ctx),
        }
        self.repos_detail();
    }

    /// The first commits to push (`ahead`) or pull of repository `k`, as
    /// a tooltip's lines, `and 3 more` last; read off the event loop when
    /// not read yet at its HEAD and counts, none meanwhile.
    fn mark_commits(&mut self, k: usize, ahead: bool) -> Vec<(String, String)> {
        let Some(item) = self.repos.items.get(k) else {
            return Vec::new();
        };
        let status = &item.status;
        let at: MarkAt = (status.head.clone(), status.ahead, status.behind);
        let count = if ahead { status.ahead } else { status.behind } as usize;
        let root = item.root.clone();
        let key = (root.clone(), ahead);
        match self.repos.mark_commits.get(&key) {
            Some(read) if read.at == at => {
                let mut lines = read.commits.clone().unwrap_or_default();
                if count > lines.len() && !lines.is_empty() {
                    lines.push((String::new(), format!("and {} more", count - lines.len())));
                }
                return lines;
            }
            _ => {}
        }
        let Some(tx) = self.repos_sender() else {
            return Vec::new();
        };
        let reading = MarkCommits {
            at: at.clone(),
            commits: None,
        };
        self.repos.mark_commits.insert(key, reading);
        std::thread::spawn(move || {
            let range = if ahead {
                "@{upstream}..HEAD"
            } else {
                "HEAD..@{upstream}"
            };
            let commits = git_ops::commits_between(&root, range, MARK_COMMITS);
            let _ = tx.send(RepoEvent::MarkCommits {
                root,
                ahead,
                at,
                commits,
            });
        });
        Vec::new()
    }

    /// The pointer moved while the panel is open.
    pub fn repos_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.repos_view()?;
        let size = self.repos_rect();
        if let Some(wd) = self.wd_mut() {
            if wd.scrolling(std::time::Instant::now()) {
                wd.hover = None;
                return None;
            }
            let layout = chrome::repos_layout(&view, size);
            let hit = chrome::repos_hit(&layout, pos);
            wd.hover = hit;
            return match hit? {
                ReposHit::Search(part) => search_tip(&layout, part),
                ReposHit::Mode(i) if view.modes.as_ref().is_some_and(|m| m.off.contains(&i)) => {
                    let part = layout.modes.as_ref()?.1.get(i)?;
                    Some(Tooltip::new(crate::md_preview::NO_EXTENSION, *part))
                }
                _ => None,
            };
        }
        // Nothing while a list moves under the pointer: it is found again
        // once they stop.
        let panel = self.repos.panel.as_mut()?;
        if panel.scrolling(std::time::Instant::now()) {
            panel.hover = None;
            return None;
        }
        let layout = chrome::repos_layout(&view, self.repos_rect());
        let hit = chrome::repos_hit(&layout, pos);
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.hover = hit;
        }
        // Icons say what they are; not over a question or an open menu.
        if view.sheet.is_some() {
            return None;
        }
        let repo = |i: usize| match view.lines.get(i) {
            Some(TreeLine::Repo(repo)) => Some(repo),
            _ => None,
        };
        match hit? {
            ReposHit::Mark(i, mark) => {
                let (.., rect) = layout
                    .marks
                    .iter()
                    .find(|(j, m, _)| (*j, *m) == (i, mark))?;
                let tip = Tooltip::new(&mark.tip(repo(i)?), *rect);
                let ahead = match mark {
                    RepoMark::Ahead => true,
                    RepoMark::Behind => false,
                    _ => return Some(tip),
                };
                let page = self.repos_page().0;
                let panel = self.repos.panel.as_ref()?;
                let k = panel.line_item(i, &self.repos.items, page)?;
                Some(tip.fields(self.mark_commits(k, ahead)))
            }
            ReposHit::Expander(i) => {
                let rect = layout.lines.get(i)?.2?;
                let text = match view.lines.get(i)? {
                    TreeLine::Clean(clean) if clean.open => {
                        "Hide the repositories with nothing to do"
                    }
                    TreeLine::Clean(_) => "Show the repositories with nothing to do",
                    TreeLine::Repo(repo) if repo.expanded? => "Hide its changed files",
                    TreeLine::Dir(dir) if dir.open => "Hide its changed files",
                    TreeLine::Repo(_) | TreeLine::Dir(_) => "Show its changed files",
                    TreeLine::File(_) => return None,
                };
                Some(Tooltip::new(text, rect))
            }
            ReposHit::Tool(tool) => {
                let (_, rect) = layout.tree_tools.iter().find(|(t, _)| *t == tool)?;
                let (text, key) = match tool {
                    TreeTool::Expand => ("Unfold every repository and folder", "Alt+Right"),
                    TreeTool::Collapse => ("Fold every repository and folder", "Alt+Left"),
                    TreeTool::List => ("List the changed files flat, by path", "Alt+F"),
                    TreeTool::Folders => ("List the changed files by folder", "Alt+F"),
                };
                Some(Tooltip::new(text, *rect).key(key))
            }
            ReposHit::Search(part) => search_tip(&layout, part),
            ReposHit::Chip(i) => {
                let (_, rect) = layout.chips.iter().find(|(j, _)| *j == i)?;
                let chip = repo(i)?.chip.as_ref()?;
                Some(Tooltip::new(&chip.tip, *rect))
            }
            // Why an action can't be done now.
            ReposHit::Button(button) if view.branch.menu.is_none() => {
                let hint = view
                    .hints
                    .iter()
                    .chain(view.result.iter().flat_map(|r| &r.buttons))
                    .find(|h| h.button == button)?;
                let why = hint.disabled.as_ref()?;
                let (_, rect) = layout.buttons.iter().find(|(b, _)| *b == button)?;
                Some(Tooltip::new(why, *rect))
            }
            ReposHit::More => {
                let n = layout.overflow.len();
                let text = format!("{n} more actions, with their keys");
                Some(Tooltip::new(&text, layout.more?))
            }
            ReposHit::Splitter => {
                let s = layout.splitter;
                let at =
                    chrome::Rect::new((s.loc.x, pos.y as i32 - 8).into(), (s.size.w, 16).into());
                Some(Tooltip::new(
                    "Drag to give the tree or the diff more room",
                    at,
                ))
            }
            ReposHit::BranchDelete(i) => {
                let (_, rect) = layout.branch_deletes.iter().find(|(j, _)| *j == i)?;
                Some(Tooltip::new("Delete this local branch", *rect).key("Del"))
            }
            ReposHit::BranchButton if view.branch.menu.is_none() => Some(
                Tooltip::new("Switch them to a branch, or create one", layout.branch).key("Ctrl+S"),
            ),
            ReposHit::BranchCreate => {
                let rect = layout.branch_create?;
                let create = view.branch.menu.as_ref()?.create.as_ref()?;
                let tip = match &create.problem {
                    Some(_) => "Fix the name to create the branch",
                    None => "Fetch, then create it; asks first if some have it",
                };
                Some(Tooltip::new(tip, rect).key("Ctrl+Enter"))
            }
            _ => None,
        }
    }

    /// The wheel or the touchpad moved the list under the pointer: the
    /// branch menu's, the detail's, else the tree's. What is under the
    /// pointer is left as it was while it moves.
    pub fn repos_scroll(&mut self, motion: crate::scroll::Motion) {
        if self.wd().is_some() {
            return self.worktree_diff_scroll(motion);
        }
        let now = std::time::Instant::now();
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, self.repos_rect());
        let pos = self.seat.get_pointer().unwrap().current_location();
        let over_detail = layout.detail.to_f64().contains(pos);
        let over_menu = layout
            .branch_menu
            .as_ref()
            .is_some_and(|(menu, _)| menu.to_f64().contains(pos));
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let Some(panel) = panel.as_mut() else {
            return;
        };
        if over_menu {
            panel.scroll_branches(motion, now, &ctx);
        } else {
            panel.scroll(motion, now, over_detail, &ctx);
        }
        if panel.scrolling(now) {
            panel.hover = None;
        }
    }

    /// Stand the detail, or else the tree, at line `pos`, as its
    /// scrollbar's thumb held there puts it.
    pub fn repos_scroll_to(&mut self, pos: f64, detail: bool) {
        if self.wd().is_some() {
            return self.worktree_diff_scroll_to(pos);
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        if let Some(panel) = panel.as_mut() {
            panel.stand_at(pos, detail, &ctx);
        }
    }

    /// Move the repositories panel's lists on to `now` if they move on
    /// their own; whether one still moves (see `Scroll::moving`).
    pub fn repos_scroll_tick(&mut self, now: std::time::Instant) -> bool {
        if self.wd().is_some() {
            return self.worktree_diff_tick(now);
        }
        if !self.repos.panel.as_ref().is_some_and(|p| p.scrolling(now)) {
            return false;
        }
        let page = self.repos_page();
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
            background,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            page,
            now: SystemTime::now(),
            fetching: *fetches > 0,
            same: same_branches,
            top: top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background,
        };
        let Some(panel) = panel.as_mut() else {
            return false;
        };
        let moving = panel.scroll_tick(now, &ctx);
        if moving {
            panel.hover = None;
        }
        moving
    }

    /// What the panel shows, while it is open.
    pub fn repos_view(&self) -> Option<ReposView> {
        if self.wd().is_some() {
            return self.worktree_diff_view();
        }
        let panel = self.repos.panel.as_ref()?;
        let (density, tree_share, cols, sheet_rows) = self.repos_env();
        let ctx = Ctx {
            items: &self.repos.items,
            batch: self.repos.batch.as_ref(),
            page: self.repos_page(),
            now: SystemTime::now(),
            fetching: self.repos.fetches > 0,
            same: &self.repos.same_branches,
            top: &self.repos.top_branches,
            density,
            tree_share,
            cols,
            sheet_rows,
            background: &self.repos.background,
        };
        let title = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map_or("this project", |p| p.name.as_str());
        Some(panel.view(title, &ctx))
    }

    /// Do what the panel asked.
    pub fn apply_repos(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_repos(),
            Outcome::Grouping(grouping) => {
                self.projects.repo_grouping = grouping;
                self.projects.save();
            }
            // Why a key did nothing.
            Outcome::Notice(text) => self.show_warning(text.into()),
            Outcome::Cut(edit) => self.repos_cut(edit),
            Outcome::Yank(edit) => self.repos_yank(edit),
            Outcome::Paste(put) => self.repos_paste(put),
            Outcome::Stop => self.stop_batches(),
            Outcome::OpenCommit { root, oid, line } => self.open_commit_diff(root, oid, line),
            Outcome::Refresh => {
                self.refresh_git();
                self.repos_read();
            }
            Outcome::UndoCut(file) => self.repos_undo_cut(false, file),
            Outcome::RedoCut(file) => self.repos_undo_cut(true, file),
            Outcome::Batch { .. }
            | Outcome::Confirmed { .. }
            | Outcome::CreateBranch { .. }
            | Outcome::RetryFailed
                if self.repos.checking.is_some() =>
            {
                self.repos_refuse("Wait for the check to finish")
            }
            Outcome::CreateBranch { name, jobs } => self.repos_look_for(name, jobs),
            Outcome::Batch { kind, jobs } if kind.checked_first() => self.repos_check(kind, jobs),
            Outcome::Batch { kind, jobs } | Outcome::Confirmed { kind, jobs } => {
                self.start_batch(kind, jobs)
            }
            Outcome::SetIdentity => self.open_identity(),
            // git can't commit without a name and email: asked first, the
            // dialog runs it again once saved.
            Outcome::RetryFailed if self.identity_failed() => self.open_identity(),
            Outcome::RetryFailed => self.retry_failed(),
            Outcome::OpenInZed { root, paths, at } => self.repos_open_in_zed(&root, &paths, at),
            Outcome::Copy { text, count } => {
                self.copy_text(text);
                let what = if count == 1 {
                    "1 review comment".to_owned()
                } else {
                    format!("{count} review comments")
                };
                self.toast(format!("Copied {what} for an AI agent"), Level::Success);
            }
            Outcome::SendReview { text, count } => self.repos_send_review(&text, count),
            Outcome::RunInTerminal(root) => {
                let Some(item) = self.repos.items.iter().find(|i| i.root == root) else {
                    return;
                };
                let w = item.workspace;
                let command = self
                    .repos
                    .batch
                    .as_ref()
                    .and_then(|b| b.run(&root))
                    .and_then(|r| r.log.last())
                    .map(|ran| ran.command.clone());
                let Some(command) = command else {
                    return;
                };
                self.close_repos();
                self.new_tab_running(w, &root, command);
            }
            Outcome::OpenWorkspace(path) => {
                match self
                    .workspaces
                    .iter()
                    .position(|ws| ws.dir.starts_with(&path))
                {
                    Some(w) => {
                        self.close_repos();
                        self.switch_workspace(w);
                    }
                    None => self.toast(
                        format!("{} isn't open as a workspace", path.display()),
                        Level::Warning,
                    ),
                }
            }
        }
    }

    /// Cut `edit` out of its file, kept to paste, the caret to go where
    /// the cut was once the diff is read again; said, with where. A
    /// restore isn't kept to paste.
    fn repos_cut(&mut self, edit: FileEdit) {
        match diff_edit::apply(&edit) {
            Ok((done, taken)) => {
                if edit.cut.restored().is_none() {
                    self.repos.register = taken;
                }
                let said = cut_said(&edit);
                self.repos_edited(done);
                self.show_success(format!("{said} · u to undo"));
            }
            Err(why) => self.show_error(why),
        }
    }

    /// Keep what `edit` would cut, to paste, and put it on the clipboard.
    fn repos_yank(&mut self, edit: FileEdit) {
        match diff_edit::yank(&edit) {
            Ok(taken) => {
                let n = taken.text.lines().count();
                let what = match (taken.lines, n) {
                    (true, 1) => "Yanked 1 line".to_owned(),
                    (true, n) => format!("Yanked {n} lines"),
                    (false, _) => format!("Yanked {} chars", taken.text.chars().count()),
                };
                self.copy_text(taken.text.clone());
                self.repos.register = taken;
                self.show_notice(what);
            }
            Err(why) => self.show_error(why),
        }
    }

    /// Put what was yanked or deleted last in as `put` says; said, with
    /// where.
    fn repos_paste(&mut self, put: Put) {
        match diff_edit::paste(&put, &self.repos.register) {
            Ok(done) => {
                let said = paste_said(&put, &self.repos.register);
                self.repos_edited(done);
                self.show_success(format!("{said} · u to undo"));
            }
            Err(why) => self.show_error(why),
        }
    }

    /// A file was edited from the diff: undone with `u` (its file's edits
    /// done again with `U` no longer), marked in the tree, the caret to go
    /// where the edit left it once the diff is read again.
    fn repos_edited(&mut self, done: Done) {
        if let Some(diff) = self.edited_diff() {
            diff.place_caret(done.root.clone(), done.path.clone(), done.then);
        }
        let root = done.root.clone();
        let file = (done.root.clone(), done.path.clone());
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.edited.insert(file.clone());
        }
        let same = |d: &Done| d.root == file.0 && d.path == file.1;
        self.repos.undone.retain(|d| !same(d));
        self.repos.cuts.push(done);
        if self.repos.cuts.len() > CUTS {
            self.repos.cuts.remove(0);
        }
        self.repos_reread(&root);
    }

    /// A file of the repository at `root` was edited from the diff: its
    /// diff read again straight away, not after Git status is (still read
    /// again, for the tree and the rest).
    fn repos_reread(&mut self, root: &Path) {
        if let Some(wd) = self.wd_mut() {
            wd.again |= wd.reading;
            self.worktree_diff_read();
        } else if let Some(panel) = self.repos.panel.as_mut() {
            panel.diff.reread(root);
            self.repos_detail();
        }
        self.refresh_git();
    }

    /// Undo the last cut in `file` (by root and path; with none, the last
    /// anywhere), or (`again`) do the last undone there again, unless its
    /// file changed since; said, with the file.
    fn repos_undo_cut(&mut self, again: bool, file: Option<(PathBuf, String)>) {
        let stack = match again {
            true => &mut self.repos.undone,
            false => &mut self.repos.cuts,
        };
        let of = |d: &Done| {
            file.as_ref()
                .is_none_or(|(r, p)| d.root == *r && d.path == *p)
        };
        let at = stack.iter().rposition(of);
        let Some(done) = at.map(|at| stack.remove(at)) else {
            let what = if again { "do again" } else { "undo" };
            let none = match &file {
                Some((_, path)) => format!("Nothing to {what} in {path}"),
                None => format!("Nothing to {what}"),
            };
            return self.show_warning(none);
        };
        match diff_edit::redo(&done, !again) {
            Ok(spot) => {
                if let Some(diff) = self.edited_diff() {
                    diff.place_caret(done.root.clone(), done.path.clone(), spot);
                }
                let root = done.root.clone();
                let said = match again {
                    true => format!("Did the edit to {}:{} again", done.path, spot.line),
                    false => format!("Undid the edit to {}:{}", done.path, spot.line),
                };
                match again {
                    true => self.repos.cuts.push(done),
                    false => self.repos.undone.push(done),
                }
                self.show_success(said);
                self.repos_reread(&root);
            }
            Err(why) => self.show_error(why),
        }
    }

    /// The edits made from the diff can't be undone once it closes: they
    /// are let go, and the tree's marks with them.
    fn forget_edits(&mut self) {
        let edits = self.repos.cuts.len();
        self.repos.cuts.clear();
        self.repos.undone.clear();
        if let Some((_, panel)) = self.repos.kept.as_mut() {
            panel.edited.clear();
        }
        if edits > 0 {
            tracing::debug!("let go of {edits} edits made from the diff");
        }
    }

    /// Check before `kind` over `jobs`, off the event loop: run it straight
    /// away when all is clean, else ask about what was found.
    fn repos_check(&mut self, kind: BatchKind, jobs: Vec<Job>) {
        let switching = matches!(kind, BatchKind::Branch(_) | BatchKind::Switch(_));
        // A branch can be out in only one worktree of a repository.
        let mut found = Vec::new();
        let mut probes = Vec::new();
        let mut seen: Vec<(PathBuf, String)> = Vec::new();
        for job in &jobs {
            let Some(item) = self.repos.items.iter().find(|i| i.root == job.root) else {
                continue;
            };
            if let Some(common) = item.status.common_dir.as_ref().filter(|_| switching) {
                if let Some((_, label)) = seen.iter().find(|(c, _)| c == common) {
                    let why = format!("the same repository as {label}, where it will be out");
                    found.push((job.root.clone(), Verdict::Blocked(why)));
                    continue;
                }
                seen.push((common.clone(), item.label.clone()));
            }
            probes.push((job.root.clone(), item.status.clone()));
        }
        // Judged against what the batch will bring in, fetched once per
        // repository.
        let fetches = if fetches_first(&kind) {
            one_per_repository(
                probes
                    .iter()
                    .filter(|(_, s)| s.remote.is_some())
                    .map(|(root, s)| (root.clone(), s.common_dir.clone()))
                    .collect(),
            )
        } else {
            Vec::new()
        };
        // Switching clean checkouts needs no look.
        let clean = probes
            .iter()
            .all(|(_, s)| s.changes.is_empty() && s.op.is_none());
        if switching && found.is_empty() && clean {
            return self.start_batch(kind, jobs);
        }
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let id = self.repos.next_id;
        self.repos.next_id += 1;
        self.repos.checking = Some(id);
        if !fetches.is_empty() {
            self.repos.fetches += 1;
        }
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.error = Some(format!("Checking {}…", kind.label().to_lowercase()));
        }
        std::thread::spawn(move || {
            let mut verdicts = found;
            if !fetches.is_empty() {
                std::thread::scope(|scope| {
                    for (root, key) in &fetches {
                        scope.spawn(move || fetch_in_turn(root, key));
                    }
                });
                let _ = tx.send(RepoEvent::Fetched);
            }
            std::thread::scope(|scope| {
                let kind = &kind;
                let handles: Vec<_> = probes
                    .iter()
                    .map(|(root, status)| scope.spawn(move || verdict(kind, root, status)))
                    .collect();
                for ((root, _), handle) in probes.iter().zip(handles) {
                    let verdict = handle
                        .join()
                        .unwrap_or_else(|_| Verdict::Blocked("the check failed".into()));
                    verdicts.push((root.clone(), verdict));
                }
            });
            let _ = tx.send(RepoEvent::Checked {
                id,
                kind,
                jobs,
                verdicts,
            });
        });
    }

    /// Before creating branch `name` in `jobs`, off the event loop: fetch
    /// (once per repository), then look for a branch by that name in each,
    /// local or on its remote. None has it: it is created; else the panel
    /// asks what to do.
    fn repos_look_for(&mut self, name: String, jobs: Vec<Job>) {
        let probes: Vec<(PathBuf, Option<String>, Option<String>)> = jobs
            .iter()
            .filter_map(|job| {
                let item = self.repos.items.iter().find(|i| i.root == job.root)?;
                let status = &item.status;
                Some((
                    job.root.clone(),
                    status.remote.clone(),
                    status.branch.clone(),
                ))
            })
            .collect();
        let fetches = one_per_repository(
            jobs.iter()
                .filter_map(|job| self.repos.items.iter().find(|i| i.root == job.root))
                .filter(|i| i.status.remote.is_some())
                .map(|i| (i.root.clone(), i.status.common_dir.clone()))
                .collect(),
        );
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let id = self.repos.next_id;
        self.repos.next_id += 1;
        self.repos.checking = Some(id);
        if !fetches.is_empty() {
            self.repos.fetches += 1;
        }
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.error = Some(format!("Fetching to look for {name}…"));
        }
        std::thread::spawn(move || {
            if !fetches.is_empty() {
                std::thread::scope(|scope| {
                    for (root, key) in &fetches {
                        scope.spawn(move || fetch_in_turn(root, key));
                    }
                });
                let _ = tx.send(RepoEvent::Fetched);
            }
            let have = probes
                .iter()
                .filter_map(|(root, remote, on)| {
                    let found = if on.as_deref() == Some(name.as_str()) {
                        Found::On
                    } else if git_ops::has_ref(root, &format!("refs/heads/{name}")) {
                        Found::Local
                    } else {
                        let remote = remote.as_ref()?;
                        let at = format!("refs/remotes/{remote}/{name}");
                        git_ops::has_ref(root, &at).then(|| Found::Remote(remote.clone()))?
                    };
                    Some((root.clone(), found))
                })
                .collect();
            let _ = tx.send(RepoEvent::Found {
                id,
                name,
                jobs,
                have,
            });
        });
    }

    /// Run `kind` over `jobs`, a few repositories at a time, off the event
    /// loop; each one's progress and result come back as `RepoEvent`s.
    fn start_batch(&mut self, kind: BatchKind, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        // Only those repositories wait that a batch still runs in; the
        // others go ahead, that batch going on in the background.
        let running = (self.repos.batch.iter())
            .chain(&self.repos.background)
            .find(|b| jobs.iter().any(|j| b.runs_in(&j.root)));
        if let Some(batch) = running {
            let what = batch.kind.label().to_lowercase();
            return self.repos_refuse(&format!(
                "Wait for the {what} running there (Alt+C stops it)"
            ));
        }
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let id = self.repos.next_id;
        self.repos.next_id += 1;
        let previous = self.repos.batch.take();
        let mut tasks = VecDeque::new();
        let mut runs = Vec::new();
        for job in jobs {
            let Some(item) = self.repos.items.iter().find(|i| i.root == job.root) else {
                continue;
            };
            let undo = match (&kind, &previous) {
                (BatchKind::Undo, Some(undone)) => {
                    let soft = matches!(
                        undone.kind,
                        BatchKind::Commit { .. } | BatchKind::Amend { .. }
                    );
                    undone
                        .run(&job.root)
                        .and_then(|r| Some((r.before.clone()?, r.after.clone()?, soft)))
                }
                _ => None,
            };
            let status = &item.status;
            tasks.push_back(Task {
                batch: id,
                kind: kind.clone(),
                job: job.clone(),
                remote: status.remote.clone(),
                base: base_of(status),
                op: status.op,
                undo,
                common: status.common_dir.clone(),
            });
            runs.push(RepoRun {
                root: job.root.clone(),
                job,
                before: None,
                after: None,
                state: RunState::Queued,
                log: Vec::new(),
                since: None,
            });
        }
        if tasks.is_empty() {
            self.repos.batch = previous;
            return;
        }
        let threads = tasks.len().min(BATCH_THREADS);
        if let Some(previous) = previous.filter(|b| !b.finished()) {
            self.repos.background.push(previous);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.repos.stops.insert(id, stop.clone());
        self.repos.batch = Some(Batch {
            id,
            kind,
            runs,
            started: SystemTime::now(),
            stopping: false,
        });
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.error = None;
        }
        let queue = Arc::new(Mutex::new(Queue {
            tasks,
            busy: Vec::new(),
        }));
        for _ in 0..threads {
            let (queue, tx, stop) = (queue.clone(), tx.clone(), stop.clone());
            std::thread::spawn(move || {
                while let Some(task) = Queue::next(&queue) {
                    let root = task.job.root.clone();
                    let step = |what: &str| {
                        let _ = tx.send(RepoEvent::Step {
                            batch: task.batch,
                            root: root.clone(),
                            what: what.to_owned(),
                        });
                    };
                    // What its commands wait for is said too; stopped,
                    // they are killed.
                    let say = {
                        let (tx, batch, root) = (tx.clone(), task.batch, root.clone());
                        move |what: &str| {
                            let _ = tx.send(RepoEvent::Step {
                                batch,
                                root: root.clone(),
                                what: what.to_owned(),
                            });
                        }
                    };
                    git_ops::watch(Some(git_ops::Watch {
                        stop: stop.clone(),
                        say: Box::new(say),
                    }));
                    // A fetch of the panel's own, or a check's, may run
                    // there still.
                    let key = task.common.clone().unwrap_or_else(|| root.clone());
                    let what = format!("the {}", task.kind.label().to_lowercase());
                    let turn = match Turn::try_take(&key, &what) {
                        Ok(turn) => Some(turn),
                        Err(holder) => {
                            step(&format!("waiting for {holder}"));
                            Turn::take_unless_stopped(&key, &what)
                        }
                    };
                    let stopped = turn.is_none() || stop.load(AtomicOrdering::Relaxed);
                    let before = (!stopped).then(|| git_ops::head(&root)).flatten();
                    let result = match stopped {
                        true => Err(blocked(git_ops::STOPPED)),
                        false => perform(&task, &step),
                    };
                    let after = (!stopped).then(|| git_ops::head(&root)).flatten();
                    git_ops::watch(None);
                    drop(turn);
                    Queue::done(&queue, &task);
                    let _ = tx.send(RepoEvent::Finished {
                        batch: task.batch,
                        root,
                        result,
                        before,
                        after,
                    });
                }
            });
        }
    }

    /// Say under the field why the panel's last ask wasn't done; in a toast
    /// when the panel is closed.
    fn repos_refuse(&mut self, why: &str) {
        match self.repos.panel.as_mut() {
            Some(panel) => panel.error = Some(why.to_owned()),
            None => self.toast(why.to_owned(), Level::Warning),
        }
    }

    /// A batch's run in the repository at `root`, while it is the batch
    /// or runs in the background.
    fn batch_run_mut(&mut self, batch: u64, root: &Path) -> Option<&mut RepoRun> {
        let mut batches = self
            .repos
            .batch
            .iter_mut()
            .chain(&mut self.repos.background);
        let found = batches.find(|b| b.id == batch)?;
        found.runs.iter_mut().find(|r| r.root == root)
    }

    /// Stop every batch running: their commands killed, the rest not run.
    fn stop_batches(&mut self) {
        let running = (self.repos.batch.iter_mut())
            .chain(&mut self.repos.background)
            .filter(|b| !b.finished());
        for batch in running {
            batch.stopping = true;
            if let Some(stop) = self.repos.stops.get(&batch.id) {
                stop.store(true, AtomicOrdering::Relaxed);
            }
        }
    }

    /// A batch running in the background is done: told in a toast, as
    /// the result bar shows another.
    fn background_finished(&mut self, id: u64) {
        let Some(at) = (self.repos.background.iter()).position(|b| b.id == id && b.finished())
        else {
            return;
        };
        let batch = self.repos.background.remove(at);
        self.repos.stops.remove(&id);
        let (summary, tone) = batch.summary();
        self.toast(summary, level(tone));
    }

    /// A thread's news.
    fn repos_event(&mut self, event: RepoEvent) {
        match event {
            RepoEvent::IdentityGuess { serial, guess } => self.identity_guess(serial, guess),
            RepoEvent::IdentitySaved { serial, result } => self.identity_saved(serial, result),
            RepoEvent::Step { batch, root, what } => {
                if let Some(run) = self.batch_run_mut(batch, &root) {
                    run.state = RunState::Running(what);
                    run.since.get_or_insert_with(SystemTime::now);
                }
            }
            RepoEvent::Finished {
                batch,
                root,
                result,
                before,
                after,
            } => {
                if let Some(run) = self.batch_run_mut(batch, &root) {
                    run.before = before;
                    run.after = after;
                    match result {
                        Ok(log) => {
                            run.log = log;
                            run.state = RunState::Done;
                        }
                        Err(failed) => {
                            run.log = failed.log;
                            run.state = match failed.failure {
                                Failure::Blocked(why) => RunState::Skipped(why),
                                failure => RunState::Failed {
                                    failure,
                                    why: failed.why,
                                },
                            };
                        }
                    }
                }
                if self
                    .repos
                    .batch
                    .as_ref()
                    .is_some_and(|b| b.id == batch && b.finished())
                {
                    self.repos.stops.remove(&batch);
                    self.batch_finished();
                }
                self.background_finished(batch);
                self.refresh_git();
            }
            RepoEvent::Checked {
                id,
                kind,
                jobs,
                verdicts,
            } => {
                if self.repos.checking != Some(id) {
                    return;
                }
                self.repos.checking = None;
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.error = None;
                }
                if verdicts.iter().all(|(_, v)| *v == Verdict::Clean) {
                    self.start_batch(kind, jobs);
                } else if let Some(panel) = self.repos.panel.as_mut() {
                    panel.ask_preflight(kind, jobs, verdicts);
                }
            }
            RepoEvent::Found {
                id,
                name,
                jobs,
                have,
            } => {
                if self.repos.checking != Some(id) {
                    return;
                }
                self.repos.checking = None;
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.error = None;
                }
                // The fetch may have brought branches the panel lists.
                self.refresh_git();
                if have.is_empty() {
                    let kind = BatchKind::Branch(name);
                    self.apply_repos(Outcome::Batch { kind, jobs });
                } else if let Some(panel) = self.repos.panel.as_mut() {
                    panel.ask_exists(name, jobs, have);
                }
            }
            RepoEvent::Detail { key, text, oids } => {
                if self.repos.reading.as_ref() == Some(&key) {
                    self.repos.reading = None;
                }
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.set_detail(key, text, oids);
                }
                // The selection may have moved on while it was read.
                self.repos_detail();
            }
            RepoEvent::Diffs {
                root,
                generation,
                files,
            } => {
                self.repos.reading_diffs.remove(&root);
                let page = self.repos_page();
                let (density, tree_share, cols, sheet_rows) = self.repos_env();
                let Repos {
                    panel,
                    batch,
                    items,
                    same_branches,
                    top_branches,
                    fetches,
                    background,
                    ..
                } = &mut self.repos;
                let ctx = Ctx {
                    items,
                    batch: batch.as_ref(),
                    page,
                    now: SystemTime::now(),
                    fetching: *fetches > 0,
                    same: same_branches,
                    top: top_branches,
                    density,
                    tree_share,
                    cols,
                    sheet_rows,
                    background,
                };
                if let Some(panel) = panel.as_mut() {
                    panel.set_diffs(root, generation, files, &ctx);
                }
                // Read again if the status moved on meanwhile.
                self.repos_detail();
                self.repos_replay();
            }
            RepoEvent::Fetched => {
                self.repos.fetches = self.repos.fetches.saturating_sub(1);
                self.refresh_git();
            }
            RepoEvent::WorktreeDiff { root, read } => self.worktree_diff_event(root, read),
            RepoEvent::FeatureDiff(read) => self.feature_diff_event(*read),
            RepoEvent::MarkCommits {
                root,
                ahead,
                at,
                commits,
            } => {
                let read = MarkCommits {
                    at,
                    commits: Some(commits),
                };
                self.repos.mark_commits.insert((root, ahead), read);
                // The tooltip waiting or up on the mark gets them.
                if self.tooltip.is_up()
                    && let Some(tip) = self.repos_hover(self.pointer)
                {
                    self.tooltip.refresh(tip);
                }
            }
        }
    }

    /// Run the panel's batch again where it failed but in conflict: what
    /// committed only pushed again.
    pub(crate) fn retry_failed(&mut self) {
        let Some(batch) = &self.repos.batch else {
            return;
        };
        let jobs = batch
            .runs
            .iter()
            .filter(|r| match &r.state {
                RunState::Failed { failure, .. } => !matches!(failure, Failure::Conflict(_)),
                _ => false,
            })
            .map(|r| {
                // What committed is only pushed again.
                let committed = r
                    .log
                    .iter()
                    .any(|ran| ran.ok && ran.command.contains(" commit "));
                let mut job = r.job.clone();
                if matches!(batch.kind, BatchKind::Commit { push: true, .. }) && committed {
                    job.files.clear();
                }
                job
            })
            .collect();
        let kind = batch.kind.clone();
        self.start_batch(kind, jobs);
    }

    /// The batch is done in every repository: empty the field it used, ask
    /// which way to pull those
    /// that diverged; told in a toast when the panel is closed.
    fn batch_finished(&mut self) {
        let Some(batch) = &self.repos.batch else {
            return;
        };
        let done = batch.runs.iter().any(|r| r.state == RunState::Done);
        let diverged: Vec<PathBuf> = batch
            .runs
            .iter()
            .filter(|r| {
                matches!(
                    r.state,
                    RunState::Failed {
                        failure: Failure::Diverged,
                        ..
                    }
                )
            })
            .map(|r| r.root.clone())
            .collect();
        let (summary, tone) = batch.summary();
        let batch_started = batch.started;
        match batch.kind {
            BatchKind::Commit { .. } | BatchKind::Amend { .. } if done => {
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.clear_field();
                }
            }
            BatchKind::Pull(Pull::FastForward) if !diverged.is_empty() => {
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.ask_diverged(diverged);
                }
            }
            _ => {}
        }
        // Told when the panel is closed, or took long enough that the
        // result bar may not be looked at.
        let long = SystemTime::now()
            .duration_since(batch_started)
            .is_ok_and(|d| d >= SAY_DONE_AFTER);
        let identity = self.identity_failed();
        if identity && self.repos.panel.is_some() {
            // git wants a name and email to commit: asked at once, the
            // batch run again once they are saved.
            self.open_identity();
        } else if identity {
            let set = (
                "Set name and email\u{2026}".to_owned(),
                MenuAction::SetGitIdentity,
            );
            self.toast_with(summary, level(tone), Some(set));
        } else if self.repos.panel.is_none() || long {
            self.toast(summary, level(tone));
        }
    }

    /// Open `paths` of the checkout at `root` (the first at `at`) in the
    /// Zed of the workspace whose checkout it is; in a terminal tab
    /// showing `git status` when that workspace has no Zed. A worktree no
    /// workspace is in opens in a Zed tab on it, made the first time.
    fn repos_open_in_zed(&mut self, root: &Path, paths: &[String], at: Option<Position>) {
        let worktree = self
            .wd()
            .filter(|wd| wd.is(root))
            .map(|wd| (wd.workspace, wd.home));
        let home = match worktree {
            Some((_, home)) => home,
            None => {
                let item = self.repos.items.iter().find(|i| i.root == root);
                item.map(|i| i.workspace)
            }
        };
        let home = home.filter(|&w| w < self.workspaces.len());
        let Some(w) = home.or(worktree.map(|(w, _)| w)) else {
            return;
        };
        let mut files: Vec<PathBuf> = paths.iter().map(|path| root.join(path)).collect();
        if let (Some(first), Some(at)) = (files.first_mut(), at) {
            *first = zed_place(first, at);
        }
        // Off to Zed: not back to the worktrees popup.
        if worktree.is_some() {
            self.worktrees.close();
        }
        self.close_repos();
        if home.is_none() {
            return self.open_in_worktree_zed(w, root, &files);
        }
        if w != self.active_workspace {
            self.switch_workspace(w);
        }
        let zed = self.workspaces[w]
            .tabs
            .iter()
            .enumerate()
            .find_map(|(t, tab)| {
                let pane = tab.panes.iter().find(|p| crate::is_zed(&p.window))?;
                Some((t, pane.window.clone()))
            });
        let Some((t, window)) = zed else {
            return self.new_tab_running(w, root, "git status".into());
        };
        self.switch_tab(t);
        self.focus(Some(window));
        self.zed_open_existing(root, &files);
    }

    /// Open `files` of the worktree at `root` in the Zed on it (launched
    /// `zed -n <root> …`), in whichever workspace; else in a new tab with
    /// one, in workspace `w`.
    fn open_in_worktree_zed(&mut self, w: usize, root: &Path, files: &[PathBuf]) {
        let lead = format!("zed -n {}", desktop::shell_quote(&root.to_string_lossy()));
        let on_root = |command: Option<&str>| {
            command.is_some_and(|c| c == lead || c.starts_with(&format!("{lead} ")))
        };
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs.iter().enumerate().find_map(|(t, tab)| {
                let pane = tab
                    .panes
                    .iter()
                    .find(|p| crate::is_zed(&p.window) && on_root(p.app_command.as_deref()))?;
                Some((w, t, pane.window.clone()))
            })
        });
        let Some((w, t, window)) = found else {
            let quoted: Vec<String> = files
                .iter()
                .map(|f| desktop::shell_quote(&f.to_string_lossy()))
                .collect();
            let command = [lead].into_iter().chain(quoted).collect::<Vec<_>>();
            return self.new_zed_tab(w, command.join(" "));
        };
        if w != self.active_workspace {
            self.switch_workspace(w);
        }
        self.switch_tab(t);
        self.focus(Some(window));
        self.zed_open_existing(root, files);
    }

    /// Hand `files` (`root` with none) to the Zed window used last, the
    /// one just focused.
    fn zed_open_existing(&mut self, root: &Path, files: &[PathBuf]) {
        let mut zed = Command::new("zed");
        zed.arg("--user-data-dir")
            .arg(&self.zed_data)
            .arg("--existing");
        if files.is_empty() {
            zed.arg(root);
        }
        zed.args(files);
        if self.spawn(zed, root, "zed").is_none() {
            self.toast("Zed didn't open the files".into(), Level::Error);
        }
    }
}

/// `file` as Zed's command line takes it to open it at `at`: `:line`,
/// then `:column` (see `diff_edit::zed_column`) when its line can be read.
fn zed_place(file: &Path, at: Position) -> PathBuf {
    let text = at
        .col
        .and_then(|col| Some((std::fs::read_to_string(file).ok()?, col)));
    let column = text.and_then(|(text, col)| {
        let line = text.lines().nth(at.line.checked_sub(1)?)?;
        Some(diff_edit::zed_column(line, col))
    });
    let mut place = file.as_os_str().to_owned();
    place.push(format!(":{}", at.line));
    if let Some(column) = column {
        place.push(format!(":{column}"));
    }
    PathBuf::from(place)
}

/// The tip of part `part` of the diff's search, as `layout` has it.
pub(crate) fn search_tip(layout: &chrome::ReposLayout, part: SearchPart) -> Option<Tooltip> {
    let (_, rect) = layout.detail_search.iter().find(|(p, _)| *p == part)?;
    let (text, key) = match part {
        SearchPart::Open => ("Find in the diff", "/"),
        SearchPart::Prev => ("The place found before", "Shift+Enter"),
        SearchPart::Next => ("The next place found", "Enter"),
        SearchPart::Close => ("Close the search", "Esc"),
        SearchPart::Field => return None,
    };
    Some(Tooltip::new(text, *rect).key(key))
}

/// A menu of `hints`, each with its key and dimmed when it can't be done
/// now, and a line to copy `path`, under `anchor`.
fn repos_menu_spec(hints: &[&Hint], path: Option<PathBuf>, anchor: chrome::Rect) -> MenuSpec {
    let mut items: Vec<MenuItem> = hints
        .iter()
        .map(|hint| {
            let mut item =
                MenuItem::new(&sentence(&hint.label), MenuAction::ReposAction(hint.button))
                    .enabled(hint.disabled.is_none());
            if !hint.key.is_empty() {
                item = item.hint(&hint.key);
            }
            item
        })
        .collect();
    if let Some(path) = path {
        items.push(MenuItem::new("Copy path", MenuAction::CopyPath(path)));
    }
    MenuSpec { anchor, items }
}

/// `label` with its first letter in upper case, as menus write their items
/// (`Push 3`).
fn sentence(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// An anchor that opens `spec`'s menu above `button`, its bottom a gap over
/// the button's top, on an output of `output`: a button at the bottom of
/// the panel has its menu go up, inside the panel.
fn above(button: chrome::Rect, spec: &MenuSpec, output: Size<i32, Logical>) -> chrome::Rect {
    let rows: Vec<chrome::MenuRow> = spec
        .items
        .iter()
        .map(|item| chrome::MenuRow {
            label: item.label.clone(),
            hint: item.hint.clone(),
            enabled: item.enabled,
        })
        .collect();
    // Under a one-pixel anchor at the top: the gap it keeps, and its height.
    let probe = chrome::Rect::new((button.loc.x, 0).into(), (button.size.w, 1).into());
    let (panel, _) = chrome::menu_layout(probe, &rows, output);
    let gap = panel.loc.y - 1;
    let y = button.loc.y - panel.size.h - 2 * gap - 1;
    if y < 0 {
        return button;
    }
    chrome::Rect::new((button.loc.x, y).into(), (button.size.w, 1).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(root: &str, dirty: bool) -> GitStatus {
        GitStatus {
            root: Some(PathBuf::from(root)),
            dirty,
            ..GitStatus::default()
        }
    }

    #[test]
    fn edits_from_the_diff_say_what_they_did_and_where() {
        use crate::diff_edit::{Register, Spot};
        let spot = |line, col| Spot { line, col };
        let edit = |cut| FileEdit {
            root: PathBuf::from("/r"),
            path: "src/a.rs".into(),
            cut,
            shown: Vec::new(),
            caret: spot(40, 0),
        };
        assert_eq!(
            cut_said(&edit(Cut::Lines(vec![40, 41]))),
            "Deleted 2 lines from src/a.rs:40"
        );
        assert_eq!(
            cut_said(&edit(Cut::Chars(spot(7, 2), spot(7, 3)))),
            "Deleted 1 char from src/a.rs:7"
        );
        let restore = Cut::Restore {
            drop: vec![12],
            put: vec![(10, vec!["a".into(), "b".into()])],
        };
        assert_eq!(cut_said(&edit(restore)), "Restored 3 lines in src/a.rs:10");
        let put = Put {
            root: PathBuf::from("/r"),
            path: "src/a.rs".into(),
            shown: Vec::new(),
            at: spot(40, 0),
            before: false,
            count: 2,
        };
        let lines = Register {
            text: "x\n".into(),
            lines: true,
        };
        assert_eq!(paste_said(&put, &lines), "Pasted 2 lines into src/a.rs:41");
        let chars = Register {
            text: "ab".into(),
            lines: false,
        };
        assert_eq!(paste_said(&put, &chars), "Pasted 4 chars into src/a.rs:40");
    }

    #[test]
    fn changed_repos_count_each_root_once() {
        let statuses = [
            status("/a", true),
            status("/a", true),
            status("/b", false),
            status("/c", true),
            GitStatus::default(),
        ];
        let counts = |c: RepoCounts| (c.changed, c.stuck);
        assert_eq!(counts(changed_repos(statuses.iter())), (2, 0));
        assert_eq!(changed_repos(std::iter::empty()), RepoCounts::default());
    }

    #[test]
    fn a_stopped_repo_counts_and_marks_the_badge() {
        let mut merging = status("/a", false);
        merging.op = Some(InProgress::Merge);
        let mut conflicted = status("/b", true);
        conflicted.changes.push(Change {
            path: "f".into(),
            orig: None,
            code: 'U',
        });
        let statuses = [merging, status("/c", false)];
        let counts = |c: RepoCounts| (c.changed, c.stuck);
        assert_eq!(counts(changed_repos(statuses.iter())), (1, 1));
        assert_eq!(counts(changed_repos([conflicted].iter())), (1, 1));
    }

    #[test]
    fn repos_to_push_or_pull_count_each_tracked_root_once() {
        let tracked = |root, ahead, behind| GitStatus {
            upstream: Some("origin/main".into()),
            ahead,
            behind,
            ..status(root, false)
        };
        let untracked = GitStatus {
            ahead: 3,
            ..status("/d", false)
        };
        let statuses = [
            tracked("/a", 2, 0),
            tracked("/a", 2, 0),
            tracked("/b", 1, 4),
            tracked("/c", 0, 0),
            untracked,
        ];
        let counts = changed_repos(statuses.iter());
        assert_eq!((counts.ahead, counts.behind), (2, 1));
        assert_eq!(counts.changed, 0);
    }

    #[test]
    fn the_repos_tip_counts_changed_stopped_and_to_push_or_pull() {
        let tip = |changed, stuck, ahead, behind| {
            repos_tip(&RepoCounts {
                changed,
                stuck,
                ahead,
                behind,
            })
        };
        assert_eq!(tip(0, 0, 0, 0), "Repositories");
        assert_eq!(tip(2, 0, 0, 0), "Repositories: 2 with changes");
        assert_eq!(
            tip(3, 1, 0, 0),
            "Repositories: 3 with changes, 1 stopped half-way"
        );
        assert_eq!(tip(0, 0, 1, 0), "Repositories: 1 repo to push");
        assert_eq!(
            tip(3, 0, 2, 1),
            "Repositories: 3 with changes, 2 repos to push, 1 repo to pull"
        );
    }
}
