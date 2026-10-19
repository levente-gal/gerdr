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
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, DetailTab, ReposView, Tone},
    git::{GitStatus, InProgress},
    git_ops::{self, Failed, Failure, Head, Integrate, OpResult, Pull, Ran, Verdict},
    repos::{
        Batch, BatchKind, Ctx, Job, Outcome, Panel, RepoEvent, RepoItem, RepoRun, Repos, RunState,
    },
    toast::Level,
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
    /// What a feature starts from and merges in: `origin/main`, else a
    /// local `main`.
    base: Option<String>,
    default: Option<String>,
    op: Option<InProgress>,
    feature: Option<String>,
    /// For an undo: where the batch undone found it, and left it.
    undo: Option<(Head, Head)>,
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

/// The branch a feature starts from and takes new commits from.
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
        BatchKind::Branch(name) | BatchKind::Join(name) => {
            step("switching…");
            git_ops::switch_or_create(dir, name)
        }
        BatchKind::StartFeature(name) => {
            let base = need(&task.base, "no default branch")?;
            step("starting…");
            git_ops::start_from(dir, name, &base)
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
        BatchKind::Integrate(how) => {
            let base = need(&task.base, "no default branch")?;
            let log = match task.remote {
                Some(_) => {
                    step("fetching…");
                    git_ops::fetch_all(dir)?
                }
                None => Vec::new(),
            };
            step(match how {
                Integrate::Merge => "merging…",
                Integrate::Rebase => "rebasing…",
            });
            chain(log, git_ops::integrate(dir, &base, *how))
        }
        BatchKind::Park(feature) => {
            let default = need(&task.default, "no default branch")?;
            step("parking…");
            git_ops::park(dir, feature, &default)
        }
        BatchKind::Resume(feature) => {
            step("resuming…");
            git_ops::resume(dir, feature)
        }
        BatchKind::MarkResolved => {
            let mut log = Vec::new();
            for file in &task.job.files {
                log = chain(log, git_ops::mark_resolved(dir, file))?;
            }
            Ok(log)
        }
        // Without an operation stopped half-way, it is a resume's stash.
        BatchKind::Continue => {
            step("continuing…");
            match task.op {
                Some(op) => git_ops::continue_op(dir, op),
                None => git_ops::finish_stash(dir, &need(&task.feature, "no feature")?),
            }
        }
        BatchKind::Abort => {
            step("aborting…");
            match task.op {
                Some(op) => git_ops::abort_op(dir, op),
                None => git_ops::abort_stash(dir, &need(&task.feature, "no feature")?),
            }
        }
        BatchKind::Undo => {
            let Some((before, after)) = &task.undo else {
                return Err(blocked("nothing to undo"));
            };
            step("undoing…");
            git_ops::undo(dir, before, after)
        }
    }
}

/// What the check before `kind` finds in the repository at `root`.
fn verdict(kind: &BatchKind, root: &Path, status: &GitStatus) -> Verdict {
    if let Some(op) = status.op {
        return Verdict::Blocked(format!("in the middle of {}", op_name(op)));
    }
    match kind {
        BatchKind::Integrate(_) => {
            let Some(base) = base_of(status) else {
                return Verdict::Blocked("no default branch".into());
            };
            if status.head.is_none() {
                return Verdict::Blocked("no commits yet".into());
            }
            // Against what the batch will merge, not what was fetched last.
            if status.remote.is_some() {
                let _ = git_ops::fetch_all(root);
            }
            git_ops::predict_merge(root, "HEAD", &base)
        }
        BatchKind::Pull(_) => {
            let Some(upstream) = &status.upstream else {
                return Verdict::Blocked("no upstream".into());
            };
            let _ = git_ops::fetch_all(root);
            git_ops::predict_merge(root, "HEAD", upstream)
        }
        BatchKind::Branch(name) | BatchKind::Join(name) | BatchKind::StartFeature(name) => {
            let local = format!("refs/heads/{name}");
            let target = if git_ops::has_ref(root, &local) {
                Some(name.clone())
            } else if matches!(kind, BatchKind::StartFeature(_)) {
                base_of(status)
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
                "{} differ on {target}: park them first",
                files(&blockers)
            ))
        }
        _ => Verdict::Clean,
    }
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

    /// The shown project's feature branch.
    fn repos_feature(&self) -> Option<String> {
        let id = self.project.as_deref()?;
        self.projects.features.get(id).cloned()
    }

    fn repos_page(&self) -> (usize, usize) {
        chrome::repos_page(self.output_size())
    }

    /// Open the repositories panel: as it was left, for the same project.
    pub fn open_repos(&mut self) {
        self.switcher = None;
        self.project_menu = None;
        self.worktrees.close();
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.repos.items = self.repo_items();
        let panel = match self.repos.kept.take() {
            Some((project, panel)) if project == self.project => panel,
            _ => Panel::new(&self.repos.items),
        };
        self.repos.panel = Some(panel);
        self.repos_refresh();
        self.repos_fetch();
        self.refresh_git();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
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
        let roots: Vec<PathBuf> = self
            .repos
            .items
            .iter()
            .filter(|i| stale(&i.status))
            .map(|i| i.root.clone())
            .collect();
        if roots.is_empty() {
            return;
        }
        let Some(tx) = self.repos_sender() else {
            return;
        };
        std::thread::spawn(move || {
            std::thread::scope(|scope| {
                for root in &roots {
                    scope.spawn(move || git_ops::fetch_all(root));
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
        let feature = self.repos_feature();
        let page = self.repos_page();
        let Repos {
            panel,
            batch,
            items,
            ..
        } = &mut self.repos;
        if let Some(panel) = panel.as_mut() {
            let ctx = Ctx {
                items,
                batch: batch.as_ref(),
                feature: feature.as_deref(),
                page,
                now: SystemTime::now(),
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

    /// Read the diff or log the selection wants, off the event loop, unless
    /// it is being read.
    fn repos_detail(&mut self) {
        let feature = self.repos_feature();
        let page = self.repos_page();
        let Some(panel) = self.repos.panel.as_ref() else {
            return;
        };
        let ctx = Ctx {
            items: &self.repos.items,
            batch: self.repos.batch.as_ref(),
            feature: feature.as_deref(),
            page,
            now: SystemTime::now(),
        };
        let Some(key) = panel.detail_wanted(&ctx) else {
            return;
        };
        if self.repos.reading.as_ref() == Some(&key) {
            return;
        }
        let change = key.file.as_ref().and_then(|file| {
            let item = self.repos.items.iter().find(|i| i.root == key.root)?;
            item.status
                .changes
                .iter()
                .find(|c| &c.path == file)
                .cloned()
        });
        let Some(tx) = self.repos_sender() else {
            return;
        };
        self.repos.reading = Some(key.clone());
        std::thread::spawn(move || {
            let text = match (key.tab, &change) {
                (DetailTab::Diff, Some(change)) => git_ops::diff(&key.root, change),
                (DetailTab::Diff, None) => String::new(),
                (DetailTab::Log, _) => git_ops::log_ahead(&key.root),
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
        let feature = self.repos_feature();
        let page = self.repos_page();
        let Repos {
            panel,
            batch,
            items,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            feature: feature.as_deref(),
            page,
            now: SystemTime::now(),
        };
        let outcome = panel.as_mut()?.key(mods, sym, typed, &ctx);
        self.repos_detail();
        outcome
    }

    /// A left click while the panel is open: outside it closes it.
    pub fn repos_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, self.output_size());
        let Some(hit) = chrome::repos_hit(&layout, pos) else {
            return self.close_repos();
        };
        let feature = self.repos_feature();
        let page = self.repos_page();
        let Repos {
            panel,
            batch,
            items,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            feature: feature.as_deref(),
            page,
            now: SystemTime::now(),
        };
        let outcome = panel.as_mut().and_then(|p| p.click(hit, &ctx));
        self.repos_detail();
        if let Some(outcome) = outcome {
            self.apply_repos(outcome);
        }
    }

    /// The pointer moved while the panel is open.
    pub fn repos_hover(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, self.output_size());
        let hit = chrome::repos_hit(&layout, pos);
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel turned `rows` rows (down positive) over the panel: the
    /// detail under the pointer scrolls, else the tree.
    pub fn repos_scroll(&mut self, rows: f64) {
        let Some(view) = self.repos_view() else {
            return;
        };
        let layout = chrome::repos_layout(&view, self.output_size());
        let pos = self.seat.get_pointer().unwrap().current_location();
        let over_detail = layout.detail.to_f64().contains(pos);
        let feature = self.repos_feature();
        let page = self.repos_page();
        let Repos {
            panel,
            batch,
            items,
            ..
        } = &mut self.repos;
        let ctx = Ctx {
            items,
            batch: batch.as_ref(),
            feature: feature.as_deref(),
            page,
            now: SystemTime::now(),
        };
        if let Some(panel) = panel.as_mut() {
            panel.scroll(rows, over_detail, &ctx);
        }
    }

    /// What the panel shows, while it is open.
    pub fn repos_view(&self) -> Option<ReposView> {
        let panel = self.repos.panel.as_ref()?;
        let feature = self.repos_feature();
        let ctx = Ctx {
            items: &self.repos.items,
            batch: self.repos.batch.as_ref(),
            feature: feature.as_deref(),
            page: self.repos_page(),
            now: SystemTime::now(),
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
                    .map(|r| r.job.clone())
                    .collect();
                let kind = batch.kind.clone();
                self.start_batch(kind, jobs);
            }
            Outcome::OpenInZed { root, paths } => self.repos_open_in_zed(&root, &paths),
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
        let switching = matches!(
            kind,
            BatchKind::Branch(_) | BatchKind::Join(_) | BatchKind::StartFeature(_)
        );
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
        if let Some(panel) = self.repos.panel.as_mut() {
            panel.error = Some(format!("Checking {}…", kind.label().to_lowercase()));
        }
        std::thread::spawn(move || {
            let mut verdicts = found;
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

    /// Run `kind` over `jobs`, a few repositories at a time, off the event
    /// loop; each one's progress and result come back as `RepoEvent`s.
    fn start_batch(&mut self, kind: BatchKind, jobs: Vec<Job>) {
        if jobs.is_empty() || self.repos.batch.as_ref().is_some_and(|b| !b.finished()) {
            return;
        }
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let id = self.repos.next_id;
        self.repos.next_id += 1;
        let feature = self.repos_feature();
        let previous = self.repos.batch.take();
        let mut tasks = VecDeque::new();
        let mut runs = Vec::new();
        for job in jobs {
            let Some(item) = self.repos.items.iter().find(|i| i.root == job.root) else {
                continue;
            };
            let undo = match kind {
                BatchKind::Undo => previous
                    .as_ref()
                    .and_then(|b| b.run(&job.root))
                    .and_then(|r| Some((r.before.clone()?, r.after.clone()?))),
                _ => None,
            };
            let status = &item.status;
            tasks.push_back(Task {
                batch: id,
                kind: kind.clone(),
                job: job.clone(),
                remote: status.remote.clone(),
                base: base_of(status),
                default: status.default_branch.clone(),
                op: status.op,
                feature: feature.clone(),
                undo,
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
        let queue = Arc::new(Mutex::new(tasks));
        for _ in 0..threads {
            let (queue, tx) = (queue.clone(), tx.clone());
            std::thread::spawn(move || {
                loop {
                    let task = queue.lock().ok().and_then(|mut q| q.pop_front());
                    let Some(task) = task else {
                        break;
                    };
                    let root = task.job.root.clone();
                    let before = git_ops::head(&root);
                    let step = |what: &str| {
                        let _ = tx.send(RepoEvent::Step {
                            batch: task.batch,
                            root: root.clone(),
                            what: what.to_owned(),
                        });
                    };
                    let result = perform(&task, &step);
                    let after = git_ops::head(&root);
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
            RepoEvent::Fetched => self.refresh_git(),
        }
    }

    /// The batch is done in every repository: remember the feature it
    /// branched to, empty the field it used, ask which way to pull those
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
        let kind = batch.kind.clone();
        match &kind {
            BatchKind::Branch(name) | BatchKind::StartFeature(name) if done => {
                if let Some(project) = self.project.clone() {
                    self.projects.features.insert(project, name.clone());
                    self.projects.save();
                }
                if let Some(panel) = self.repos.panel.as_mut() {
                    panel.clear_field();
                }
            }
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
