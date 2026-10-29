//! The repositories panel in gerdr (see `crate::repos`): what it lists, the
//! threads running its batches and the checks before them, the diffs and
//! logs it shows, and what its answers do: open conflicts in the
//! repository's Zed, run a command that wants a login in a terminal, show
//! the workspace where a branch is out already.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
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
    chrome::{self, Density, DetailTab, Hint, ReposButton, ReposHit, ReposView, Tone, TreeLine},
    git::{Change, GitStatus, InProgress},
    git_ops::{self, Failed, Failure, Head, Integrate, OpResult, Pull, Ran, Turn, Verdict},
    repos::{
        Batch, BatchKind, Ctx, Found, Job, Outcome, Panel, RepoEvent, RepoItem, RepoRun, Repos,
        RunState,
    },
    review,
    toast::Level,
    tooltip::Tooltip,
};

/// How many repositories a batch works in at once.
const BATCH_THREADS: usize = 4;

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
        BatchKind::StashPop => {
            step("popping the stash…");
            git_ops::stash_pop(dir)
        }
        BatchKind::StashDrop => {
            step("dropping the stash…");
            git_ops::stash_drop(dir)
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

/// How many of the repositories of `statuses` (one per root, however many
/// checkouts share it) have uncommitted changes or are stopped half-way (a
/// merge, rebase or the like, or a conflicted file), and how many are
/// stopped.
fn changed_repos<'a>(statuses: impl Iterator<Item = &'a GitStatus>) -> (usize, usize) {
    let mut roots: Vec<&Path> = Vec::new();
    let (mut changed, mut stuck) = (0, 0);
    for status in statuses {
        let Some(root) = status.root.as_deref() else {
            continue;
        };
        if roots.contains(&root) {
            continue;
        }
        roots.push(root);
        let stopped = status.op.is_some() || status.changes.iter().any(Change::conflicted);
        changed += (status.dirty || stopped) as usize;
        stuck += stopped as usize;
    }
    (changed, stuck)
}

/// The repositories button's tooltip, with `changed` repositories changed
/// and `stuck` of them stopped half-way.
pub(crate) fn repos_tip(changed: usize, stuck: usize) -> String {
    let mut text = "Repositories".to_owned();
    if changed > 0 {
        text.push_str(&format!(": {changed} with changes"));
    }
    if stuck > 0 {
        text.push_str(&format!(", {stuck} stopped half-way"));
    }
    text
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

    /// How many of the project's repositories have uncommitted changes or
    /// are stopped half-way, and how many are stopped: the sidebar's badge
    /// on the repositories button.
    pub(crate) fn repos_changed(&self) -> (usize, usize) {
        let statuses = self
            .workspaces
            .iter()
            .filter_map(|ws| self.git.get(&ws.dir));
        changed_repos(statuses)
    }

    fn repos_page(&self) -> (usize, usize) {
        chrome::repos_page(self.output_size(), self.density)
    }

    /// What the panel's views and keys need of the chrome: the density,
    /// the tree's share of the panel, and how many characters a review
    /// comment's row of the diff holds.
    fn repos_env(&self) -> (Density, f64, usize) {
        let share = self.repos.tree_share.unwrap_or(chrome::REPOS_TREE_SHARE);
        let cols = chrome::repos_detail_cols(self.output_size(), share);
        (self.density, share, cols)
    }

    /// Open the repositories panel: as it was left, for the same project.
    pub fn open_repos(&mut self) {
        self.switcher = None;
        self.project_menu = None;
        self.worktrees.close();
        self.inbox.close();
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
                    panel.review = review::load(None, id);
                }
                panel
            }
        };
        self.repos.panel = Some(panel);
        self.repos_refresh();
        self.repos_fetch();
        self.refresh_git();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Save the panel's review comments with the project, if they changed.
    fn repos_save_review(&mut self) {
        let Some(panel) = self.repos.panel.as_mut().filter(|p| p.review_dirty) else {
            return;
        };
        panel.review_dirty = false;
        let Some(id) = self.project.as_deref() else {
            return;
        };
        if let Err(err) = review::save(None, id, &panel.review) {
            tracing::warn!("saving the review: {err}");
            self.toast(format!("Couldn't save the review: {err}"), Level::Error);
        }
    }

    /// Close the panel, kept as it is for opening again.
    fn close_repos(&mut self) {
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
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
    fn repos_sender(&mut self) -> Option<channel::Sender<RepoEvent>> {
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
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
            };
            panel.refresh(&ctx);
        }
        self.repos_detail();
    }

    /// A Git status read came in.
    pub fn repos_read(&mut self) {
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
        let (density, tree_share, cols) = self.repos_env();
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
            let text = match key.tab {
                DetailTab::Diff => String::new(),
                DetailTab::Log => git_ops::log_ahead(&key.root),
                DetailTab::Stashes => git_ops::stash_list(&key.root),
            };
            let _ = tx.send(RepoEvent::Detail { key, text });
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
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        let layout = chrome::repos_layout(&view, self.output_size());
        let Some(hit) = chrome::repos_hit(&layout, pos) else {
            return self.close_repos();
        };
        match hit {
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
        let double = self.is_double(pos);
        self.repos_hit(hit, double);
    }

    /// A click on `hit` in the panel, the second of a double click when
    /// `double`.
    fn repos_hit(&mut self, hit: ReposHit, double: bool) {
        // Shift+click on a diff row reaches the cursor's range there.
        let shift = self
            .seat
            .get_keyboard()
            .is_some_and(|k| k.modifier_state().shift);
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        };
        let mut drags = false;
        let outcome = panel.as_mut().and_then(|p| {
            if let (ReposHit::DiffRow(i), true, false) = (hit, shift, double) {
                p.extend_detail_row(i, &ctx);
                drags = p.drags_rows();
                return None;
            }
            let outcome = p.click(hit, &ctx);
            drags = matches!(hit, ReposHit::DiffRow(_)) && !double && p.drags_rows();
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
        let output = self.output_size();
        let layout = chrome::repos_layout(&view, output);
        let Some(hit) = chrome::repos_hit(&layout, pos) else {
            return;
        };
        if view.sheet.is_some() || view.branch.menu.is_some() {
            return;
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
        let path = view.selected.and_then(|i| match view.lines.get(i)? {
            TreeLine::File(file) => {
                let root = self.repos.panel.as_ref()?.selected_root()?;
                Some(root.join(file.path.split(" → ").last().unwrap_or(&file.path)))
            }
            TreeLine::Repo(_) => self.repos.panel.as_ref()?.selected_root(),
        });
        let hints: Vec<&Hint> = view.hints.iter().collect();
        let spec = repos_menu_spec(&hints, path, anchor);
        self.open_menu(spec);
    }

    /// Do `button`, as a menu asks.
    pub fn repos_press(&mut self, button: ReposButton) {
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        let share = chrome::repos_share_at(pos.x, self.output_size());
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
        let layout = chrome::repos_layout(&view, self.output_size());
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
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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

    /// The pointer moved while the panel is open.
    pub fn repos_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.repos_view()?;
        // Nothing while a list moves under the pointer: it is found again
        // once they stop.
        let panel = self.repos.panel.as_mut()?;
        if panel.scrolling(std::time::Instant::now()) {
            panel.hover = None;
            return None;
        }
        let layout = chrome::repos_layout(&view, self.output_size());
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
                Some(Tooltip::new(&mark.tip(repo(i)?), *rect))
            }
            ReposHit::Expander(i) => {
                let rect = layout.lines.get(i)?.2?;
                let text = match repo(i)?.expanded? {
                    true => "Hide its changed files",
                    false => "Show its changed files",
                };
                Some(Tooltip::new(text, rect))
            }
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
        let now = std::time::Instant::now();
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, self.output_size());
        let pos = self.seat.get_pointer().unwrap().current_location();
        let over_detail = layout.detail.to_f64().contains(pos);
        let over_menu = layout
            .branch_menu
            .as_ref()
            .is_some_and(|(menu, _)| menu.to_f64().contains(pos));
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        };
        if let Some(panel) = panel.as_mut() {
            panel.stand_at(pos, detail, &ctx);
        }
    }

    /// Move the repositories panel's lists on to `now` if they move on
    /// their own; whether one still moves (see `Scroll::moving`).
    pub fn repos_scroll_tick(&mut self, now: std::time::Instant) -> bool {
        if !self.repos.panel.as_ref().is_some_and(|p| p.scrolling(now)) {
            return false;
        }
        let page = self.repos_page();
        let (density, tree_share, cols) = self.repos_env();
        let Repos {
            panel,
            batch,
            items,
            same_branches,
            top_branches,
            fetches,
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
        let panel = self.repos.panel.as_ref()?;
        let (density, tree_share, cols) = self.repos_env();
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
            Outcome::RetryFailed => {
                let Some(batch) = &self.repos.batch else {
                    return;
                };
                let jobs = batch
                    .runs
                    .iter()
                    .filter(|r| match &r.state {
                        RunState::Failed { failure, .. } => {
                            !matches!(failure, Failure::Conflict(_))
                        }
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
            Outcome::OpenInZed { root, paths } => self.repos_open_in_zed(&root, &paths),
            Outcome::Copy { text, count } => {
                self.copy_text(text);
                let what = if count == 1 {
                    "1 review comment".to_owned()
                } else {
                    format!("{count} review comments")
                };
                self.toast(format!("Copied {what} for an AI agent"), Level::Success);
            }
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
        if self.repos.batch.as_ref().is_some_and(|b| !b.finished()) {
            return self.repos_refuse("Wait for the batch to finish");
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
                    let soft = matches!(undone.kind, BatchKind::Commit { .. });
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
            });
        }
        if tasks.is_empty() {
            self.repos.batch = previous;
            return;
        }
        let threads = tasks.len().min(BATCH_THREADS);
        self.repos.batch = Some(Batch { id, kind, runs });
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.error = None;
        }
        let queue = Arc::new(Mutex::new(Queue {
            tasks,
            busy: Vec::new(),
        }));
        for _ in 0..threads {
            let (queue, tx) = (queue.clone(), tx.clone());
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
                    // A fetch of the panel's own, or a check's, may run
                    // there still.
                    let key = task.common.clone().unwrap_or_else(|| root.clone());
                    let what = format!("the {}", task.kind.label().to_lowercase());
                    let turn = Turn::try_take(&key, &what).unwrap_or_else(|holder| {
                        step(&format!("waiting for {holder}"));
                        Turn::take(&key, &what)
                    });
                    let before = git_ops::head(&root);
                    let result = perform(&task, &step);
                    let after = git_ops::head(&root);
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

    /// A batch's run in the repository at `root`, while it is the batch.
    fn batch_run_mut(&mut self, batch: u64, root: &Path) -> Option<&mut RepoRun> {
        let current = self.repos.batch.as_mut().filter(|b| b.id == batch)?;
        current.runs.iter_mut().find(|r| r.root == root)
    }

    /// A thread's news.
    fn repos_event(&mut self, event: RepoEvent) {
        match event {
            RepoEvent::Step { batch, root, what } => {
                if let Some(run) = self.batch_run_mut(batch, &root) {
                    run.state = RunState::Running(what);
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
                    self.batch_finished();
                }
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
            RepoEvent::Detail { key, text } => {
                if self.repos.reading.as_ref() == Some(&key) {
                    self.repos.reading = None;
                }
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.set_detail(key, text);
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
                let (density, tree_share, cols) = self.repos_env();
                let Repos {
                    panel,
                    batch,
                    items,
                    same_branches,
                    top_branches,
                    fetches,
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
                };
                if let Some(panel) = panel.as_mut() {
                    panel.set_diffs(root, generation, files, &ctx);
                }
                // Read again if the status moved on meanwhile.
                self.repos_detail();
            }
            RepoEvent::Fetched => {
                self.repos.fetches = self.repos.fetches.saturating_sub(1);
                self.refresh_git();
            }
        }
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
        match batch.kind {
            BatchKind::Commit { .. } if done => {
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
        if self.repos.panel.is_none() {
            self.toast(summary, level(tone));
        }
    }

    /// Open `paths` of the repository at `root` in its workspace's Zed; in
    /// a terminal tab showing `git status` when the workspace has no Zed.
    fn repos_open_in_zed(&mut self, root: &Path, paths: &[String]) {
        let Some(w) = self
            .repos
            .items
            .iter()
            .find(|i| i.root == root)
            .map(|i| i.workspace)
            .filter(|&w| w < self.workspaces.len())
        else {
            return;
        };
        self.close_repos();
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
        // `--existing`: in the Zed window used last, the one just focused.
        let mut zed = Command::new("zed");
        zed.arg("--user-data-dir")
            .arg(&self.zed_data)
            .arg("--existing");
        if paths.is_empty() {
            zed.arg(root);
        }
        for path in paths {
            zed.arg(root.join(path));
        }
        if self.spawn(zed, root, "zed").is_none() {
            self.toast("Zed didn't open the files".into(), Level::Error);
        }
    }
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
    fn changed_repos_count_each_root_once() {
        let statuses = [
            status("/a", true),
            status("/a", true),
            status("/b", false),
            status("/c", true),
            GitStatus::default(),
        ];
        assert_eq!(changed_repos(statuses.iter()), (2, 0));
        assert_eq!(changed_repos(std::iter::empty()), (0, 0));
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
        assert_eq!(changed_repos(statuses.iter()), (1, 1));
        assert_eq!(changed_repos([conflicted].iter()), (1, 1));
    }

    #[test]
    fn the_repos_tip_counts_changed_and_stopped() {
        assert_eq!(repos_tip(0, 0), "Repositories");
        assert_eq!(repos_tip(2, 0), "Repositories: 2 with changes");
        assert_eq!(
            repos_tip(3, 1),
            "Repositories: 3 with changes, 1 stopped half-way"
        );
    }
}
