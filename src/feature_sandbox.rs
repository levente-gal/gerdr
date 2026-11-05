//! A feature's sandbox from gerdr's side: the Manual test page's buttons, what
//! runs in its tab, and what goes when it goes. `feature::sandbox_run` does
//! the work, off the UI thread.
//!
//! A terminal of a sandbox is a plain one whose shell entered the namespace,
//! so a command typed there or started again stays inside it. It isn't run
//! again after a server restart, when its shell would be outside
//! (`PaneRecord::restart_command`). A web page of the feature's workspace on
//! `localhost` opens inside too (`Gerdr::sandbox_for`), and so reaches this
//! feature's app at the address the app expects.
//!
//! Start asks first which of the project's commands run, in the restart
//! dialog (see `restart`). What is checked stays with the feature
//! (`Feature::sandbox_picks`), not in the project file, and Restart and Reset
//! run the same without asking. A command the feature hasn't been asked about
//! runs, unless the project's line for it is a comment.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    Gerdr,
    chrome::CommandsTab,
    feature::{
        self, Feature, PerProject,
        sandbox_run::{self, SandboxRun},
        store,
    },
    features::{Busy, Done, Job},
    menu::MenuAction,
    prepare::{Prep, Tools, background_command, spawn_prepare},
};

/// What a tab of a sandbox's terminals is called.
const TAB_LABEL: &str = "Sandbox";

/// What stopping a sandbox is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopFor {
    /// It stops, its data kept.
    Stop,
    /// All of it goes.
    Remove,
    /// It starts again on the same data.
    Restart,
    /// It starts again on a new copy of main's data.
    Reset,
}

impl StopFor {
    fn removes(self) -> bool {
        matches!(self, Self::Remove | Self::Reset)
    }

    fn starts_again(self) -> bool {
        matches!(self, Self::Restart | Self::Reset)
    }
}

/// Feature `slug` of `project`'s sandbox folder.
pub fn sandbox_dir(project: &str, slug: &str) -> PathBuf {
    store::dir(None, project, slug).join("sandbox")
}

/// The folder of repository `name` of the project at `root`, outside any
/// feature: that of a single repository is the root itself.
fn main_folder(root: &Path, name: &str) -> Option<PathBuf> {
    let inside = root.join(name);
    if inside.is_dir() {
        return Some(inside);
    }
    (root.file_name()?.to_string_lossy() == name).then(|| root.to_owned())
}

/// A command a sandbox can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLine {
    /// Its repository's `feature::repo_key`.
    pub repo: String,
    /// Its repository's folder name.
    pub name: String,
    /// The feature holds that repository: it runs from its worktree.
    pub held: bool,
    /// Where it runs.
    pub dir: PathBuf,
    /// The project's words for it, without a comment's `#`.
    pub words: String,
    /// What runs in `dir`.
    pub command: String,
    /// Whether it runs: as picked, else unless its line is a comment.
    pub on: bool,
    /// It runs before the others, not in a tab: `workflow.prepare`.
    pub first: bool,
}

impl SandboxLine {
    /// Where it runs and when, as the dialog asking about it says.
    pub fn place(&self) -> String {
        let mut place = self.name.clone();
        if !self.held {
            place.push_str(" \u{b7} from main");
        }
        if self.first {
            place.push_str(" \u{b7} runs first");
        }
        place
    }
}

/// The commands `lists` give, each with the folder it runs in: the feature's
/// worktree for a repository it holds, else the repository's main folder, so a
/// project's other services run too, unchanged. What names a main folder then
/// names the worktree. Those not to run are there too, `on` saying so.
pub fn lines_of(
    feature: &Feature,
    lists: &PerProject<BTreeMap<String, Vec<String>>>,
    first: bool,
) -> Vec<SandboxLine> {
    let mut out = Vec::new();
    let mut add = |root: &Path, project: Option<&str>, list: &BTreeMap<String, Vec<String>>| {
        for (name, commands) in list {
            let held =
                (feature.repos.iter()).find(|r| r.name == *name && r.project.as_deref() == project);
            let dir = held
                .map(|r| r.worktree.clone())
                .or_else(|| main_folder(root, name));
            let Some(dir) = dir else {
                tracing::warn!("no folder for repository {name}: its commands don't run");
                continue;
            };
            let repo = feature::repo_key(project, name);
            let picks = feature.sandbox_picks.get(&repo);
            for command in commands {
                let command = command.trim();
                let words = command.trim_start_matches('#').trim();
                if words.is_empty() {
                    continue;
                }
                let picked = picks.and_then(|p| p.get(words)).copied();
                out.push(SandboxLine {
                    repo: repo.clone(),
                    name: name.clone(),
                    held: held.is_some(),
                    dir: dir.clone(),
                    words: words.to_owned(),
                    command: sandbox_run::in_worktrees(words, &feature.repos),
                    on: picked.unwrap_or(!command.starts_with('#')),
                    first,
                });
            }
        }
    };
    add(&feature.root, None, &lists.home);
    for link in &feature.links {
        if let Some(list) = lists.links.get(&link.project) {
            add(&link.root, Some(&link.project), list);
        }
    }
    out
}

/// Whether anything in `lists` can run.
fn has_commands(lists: &PerProject<BTreeMap<String, Vec<String>>>) -> bool {
    std::iter::once(&lists.home)
        .chain(lists.links.values())
        .flat_map(|list| list.values().flatten())
        .any(|command| !command.trim().is_empty())
}

/// Whether `command` opens a page on this machine's `localhost`, which means
/// the sandbox's own inside it. `*.localhost` isn't: T3 Code's pages are named
/// that way, and its server runs outside.
fn is_local_page(command: &str) -> bool {
    let command = command.trim();
    let Some(rest) = (command.strip_prefix("http://")).or_else(|| command.strip_prefix("https://"))
    else {
        return false;
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host_port.split(':').next().unwrap_or(host_port),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

impl Gerdr {
    /// The sandbox running for a page `command` opened from folder `dir`: the
    /// namespace to open it in and the browser profile for it.
    pub(crate) fn sandbox_for(&self, command: &str, dir: &Path) -> Option<(i32, PathBuf)> {
        if !is_local_page(command) {
            return None;
        }
        let same = |a: &Path, b: &Path| a == b || a.canonicalize().ok() == b.canonicalize().ok();
        let feature = (self.features.all.values().flatten())
            .find(|f| same(&f.dir, dir) && f.sandbox.as_ref().is_some_and(SandboxRun::alive))?;
        let holder = feature.sandbox.as_ref()?.holder.as_ref()?.pid;
        let profile = sandbox_dir(&feature.project, &feature.slug).join("browser");
        Some((holder, profile))
    }

    /// Whether `feature`'s sandbox can start now; if not, the user hears why.
    /// With no run commands the dialog for them opens: nothing would run.
    fn can_start_sandbox(&mut self, feature: &Feature) -> bool {
        let key = (feature.project.clone(), feature.slug.clone());
        if self.features.build.sandbox_stops.contains_key(&key) {
            self.show_warning(format!(
                "The sandbox of \u{201c}{}\u{201d} is being started or stopped",
                feature.title
            ));
            return false;
        }
        if feature.sandbox.as_ref().is_some_and(SandboxRun::alive) {
            self.show_warning(format!(
                "The sandbox of \u{201c}{}\u{201d} is already running",
                feature.title
            ));
            return false;
        }
        let runs = self.projects.workflows(feature, |w| w.run.clone());
        if !has_commands(&runs) {
            self.show_warning("Say what a sandbox runs first".into());
            self.open_project_commands(&key.0, Some(&key.1), CommandsTab::Run);
            return false;
        }
        true
    }

    /// Ask which commands feature `slug`'s sandbox runs, then start it. What
    /// runs first is listed first, as it runs.
    pub(crate) fn ask_start_sandbox(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if !self.can_start_sandbox(&feature) {
            return;
        }
        let prepares = self.projects.workflows(&feature, |w| w.prepare.clone());
        let runs = self.projects.workflows(&feature, |w| w.run.clone());
        let mut lines = lines_of(&feature, &prepares, true);
        lines.extend(lines_of(&feature, &runs, false));
        if lines.is_empty() {
            return self.start_sandbox(project, slug);
        }
        self.open_sandbox_picks(project, slug, lines);
    }

    /// The commands of feature `slug`'s sandbox were checked or not: keep
    /// that with the feature, and start it. A pick of a command not listed
    /// now stays: that command may come back.
    pub(crate) fn sandbox_picked(
        &mut self,
        project: &str,
        slug: &str,
        picks: &[(SandboxLine, bool)],
    ) {
        if let Some(f) = self.feature_mut(project, slug) {
            for (line, on) in picks {
                (f.sandbox_picks.entry(line.repo.clone()).or_default())
                    .insert(line.words.clone(), *on);
            }
            self.save_feature(project, slug);
        }
        self.start_sandbox(project, slug);
    }

    /// Start feature `slug`'s sandbox, off the thread, with the commands its
    /// record says.
    pub(crate) fn start_sandbox(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if !self.can_start_sandbox(&feature) {
            return;
        }
        let key = (project.to_owned(), slug.to_owned());
        let prepares = self.projects.workflows(&feature, |w| w.prepare.clone());
        let venv = self.feature_venv(project, slug);
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        let envs = self.projects.workflows(&feature, |w| w.sandbox_env.clone());
        for link in envs.links.values() {
            env.extend(link.clone());
        }
        env.extend(envs.home.clone());
        let taken: Vec<u16> = (self.features.all.values().flatten())
            .filter(|f| !(f.project == project && f.slug == slug))
            .filter_map(|f| f.sandbox.as_ref())
            .flat_map(|run| run.forwards.iter().map(|f| f.host_port))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        self.features
            .build
            .sandbox_stops
            .insert(key, Arc::clone(&stop));
        let title = feature.title.clone();
        // A failure an earlier start put in the problems list goes: this run
        // fixes it or reports its own.
        if let Some(why) = feature.sandbox.as_ref().and_then(|r| r.failed.as_deref()) {
            crate::problems::forget(PROBLEMS, &failed_text(&title, why));
        }
        forget_out_of_reach(&feature);
        if let Some(f) = self.feature_mut(project, slug) {
            let now = feature::now();
            // Its panes go with the run they belonged to: none outlive a stop.
            f.sandbox = Some(SandboxRun {
                started: now,
                ..SandboxRun::default()
            });
            f.log(now, "Sandbox started");
            self.save_feature(project, slug);
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Sandbox,
            repo: None,
            what: format!("Starting the sandbox of \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |tx| {
            let tools = Tools::detect();
            let prepare: Vec<sandbox_run::PrepareStep> = lines_of(&feature, &prepares, true)
                .into_iter()
                .filter(|line| line.on)
                .map(|line| sandbox_run::PrepareStep {
                    line: background_command(&line.command, &line.dir, &feature.dir, &venv, &tools),
                    dir: line.dir,
                    shown: line.command,
                })
                .collect();
            let mut roots = vec![feature.root.clone()];
            roots.extend(feature.links.iter().map(|l| l.root.clone()));
            let env: Vec<(String, String)> = env.into_iter().collect();
            let dir = sandbox_dir(&project, &slug);
            let job = sandbox_run::Job {
                project: &project,
                slug: &slug,
                repos: &feature.repos,
                roots: &roots,
                dir: &dir,
                prepare: &prepare,
                env: &env,
                taken: &taken,
                stop: &stop,
            };
            let mut progress = |what: String| {
                // It fails only once gerdr quits: nothing is left to show it.
                let _ = tx.send(Done::Progress {
                    project: project.clone(),
                    slug: slug.clone(),
                    job: Job::Sandbox,
                    what: format!("Sandbox of \u{201c}{title}\u{201d}: {what}"),
                });
            };
            let run = sandbox_run::start(&job, &mut progress);
            Done::Sandbox {
                stopped: stop.load(Ordering::Relaxed),
                project,
                slug,
                run,
            }
        });
    }

    /// Starting feature `slug`'s sandbox ended: its commands open, or what
    /// went wrong is said and what it had started goes.
    pub(crate) fn sandbox_started(
        &mut self,
        project: &str,
        slug: &str,
        run: SandboxRun,
        stopped: bool,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.sandbox_stops.remove(&key);
        let Some(feature) = self.feature(project, slug) else {
            // The feature went meanwhile, and nothing else knows of these.
            let dir = sandbox_dir(project, slug);
            std::thread::spawn(move || {
                if let Err(why) = sandbox_run::stop(&run, &dir, true) {
                    tracing::error!("removing the sandbox of a removed feature: {why}");
                }
            });
            return;
        };
        let title = feature.title.clone();
        if let Some(why) = run.failed.clone() {
            // Off the thread: it waits for what ran inside to end.
            let pieces = run.clone();
            std::thread::spawn(move || pieces.kill_network());
            if let Some(f) = self.feature_mut(project, slug) {
                f.sandbox = Some(run);
            }
            self.save_feature(project, slug);
            if stopped {
                // What it got started goes too: asked for, it isn't a failure.
                return self.finish_stop(project, slug, StopFor::Stop);
            }
            let text = failed_text(&title, &why);
            crate::problems::report(PROBLEMS, text.clone());
            return self.show_error(text);
        }
        if !run.outward.is_empty() {
            self.show_warning(format!(
                "This sandbox can reach real services: {}. Set workflow.sandbox_env in the project file to change that",
                run.outward.join(", ")
            ));
        }
        if !run.out_of_reach.is_empty() {
            let text = out_of_reach_problem(&title, &run.out_of_reach);
            crate::problems::report(PROBLEMS, text.clone());
            self.show_warning(text);
        }
        let holder = run.holder.as_ref().map(|h| h.pid);
        if let Some(f) = self.feature_mut(project, slug) {
            f.sandbox = Some(run);
        }
        self.save_feature(project, slug);
        let (Some(holder), Some(feature)) = (holder, self.feature(project, slug).cloned()) else {
            return;
        };
        self.open_sandbox_terminals(project, slug, &feature, holder);
    }

    /// Open a terminal inside the sandbox for each of the project's run
    /// commands, together in a tab of their own, as Verify's checks are. It
    /// stays behind what the user is at: starting takes a while.
    fn open_sandbox_terminals(
        &mut self,
        project: &str,
        slug: &str,
        feature: &Feature,
        holder: i32,
    ) {
        let runs = self.projects.workflows(feature, |w| w.run.clone());
        let envs = self.projects.workflows(feature, |w| w.sandbox_env.clone());
        let mut env: Vec<(String, String)> = envs
            .links
            .values()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env.extend(envs.home.iter().map(|(k, v)| (k.clone(), v.clone())));
        let lines: Vec<_> = lines_of(feature, &runs, false)
            .into_iter()
            .filter(|line| line.on)
            .collect();
        let dirs: Vec<PathBuf> = lines.iter().map(|line| line.dir.clone()).collect();
        let panes = if lines.is_empty() {
            Vec::new()
        } else {
            let Some(panes) = self.in_feature_workspace(project, slug, |gerdr, w| {
                gerdr.open_terminal_tabs(w, TAB_LABEL, &dirs)
            }) else {
                return;
            };
            panes
        };
        let venv = self.feature_venv(project, slug);
        let preps = (panes.iter().zip(&lines)).map(|(pane, line)| {
            let command = sandbox_run::enter_line(holder, slug, &env, &line.command);
            let run = Some((line.dir.clone(), Some(command)));
            Prep::new_pane(
                pane.clone(),
                line.dir.clone(),
                feature.dir.clone(),
                venv.clone(),
                run,
            )
        });
        let preps: Vec<Prep> = preps.collect();
        if let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) {
            spawn_prepare(socket, preps);
        }
        let missing: Vec<String> = (lines.iter().skip(panes.len()))
            .map(|line| format!("`{}`", line.command))
            .collect();
        if let Some(f) = self.feature_mut(project, slug)
            && let Some(run) = f.sandbox.as_mut()
        {
            run.panes = panes;
        }
        self.save_feature(project, slug);
        if missing.is_empty() {
            self.show_success(format!(
                "The sandbox of \u{201c}{}\u{201d} is running",
                feature.title
            ));
        } else {
            self.show_error(format!(
                "Couldn't open a terminal for {}",
                missing.join(", ")
            ));
        }
    }

    /// Open the sandbox's app, in the feature's workspace.
    pub(crate) fn open_sandbox_app(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(run) = feature.sandbox.as_ref().filter(|r| r.alive()) else {
            return self.show_warning("The sandbox isn't running".into());
        };
        let set = (self.projects.project(project)).and_then(|p| p.workflow.sandbox_url.clone());
        let url = set.or_else(|| {
            sandbox_run::app_port(&run.listening()).map(|port| format!("http://localhost:{port}"))
        });
        match url {
            Some(url) => self.open_feature_page(project, slug, url),
            None => self.show_warning("Nothing listens in the sandbox yet".into()),
        }
    }

    fn ask_sandbox(
        &mut self,
        title: String,
        body: String,
        go: &'static str,
        project: &str,
        slug: &str,
        what: StopFor,
    ) {
        let then = crate::confirm::Then::StopSandbox {
            project: project.to_owned(),
            slug: slug.to_owned(),
            what,
        };
        self.ask(title, body, go, then);
    }

    /// Ask before removing what sandboxes of gone features left.
    pub(crate) fn ask_remove_sandbox_leftovers(&mut self, stacks: Vec<String>) {
        let body = format!("{}\nTheir containers and data are lost.", stacks.join("\n"));
        self.ask(
            "Remove what these sandboxes left?".into(),
            body,
            "Remove",
            crate::confirm::Then::RemoveSandboxLeftovers(stacks),
        );
    }

    /// The Stop button: stops at once what is still starting, else asks, as
    /// it ends what runs.
    pub(crate) fn ask_stop_sandbox(&mut self, project: &str, slug: &str) {
        let key = (project.to_owned(), slug.to_owned());
        if let Some(stop) = self.features.build.sandbox_stops.get(&key) {
            stop.store(true, Ordering::Relaxed);
            return;
        }
        let Some(title) = self.feature(project, slug).map(|f| f.title.clone()) else {
            return;
        };
        self.ask_sandbox(
            format!("Stop the sandbox of \u{201c}{title}\u{201d}?"),
            "Its terminals close and its containers stop.\nIts data stays for the next start."
                .into(),
            "Stop",
            project,
            slug,
            StopFor::Stop,
        );
    }

    /// The Reset data button: all that was done on the copy goes.
    pub(crate) fn ask_reset_sandbox(&mut self, project: &str, slug: &str) {
        let Some(title) = self.feature(project, slug).map(|f| f.title.clone()) else {
            return;
        };
        self.ask_sandbox(
            format!("Reset the data of \u{201c}{title}\u{201d}'s sandbox?"),
            "What was done on its copy of the data is lost.\nMain's data is copied again.".into(),
            "Reset data",
            project,
            slug,
            StopFor::Reset,
        );
    }

    /// The Remove button: its containers and data go.
    pub(crate) fn ask_remove_sandbox(&mut self, project: &str, slug: &str) {
        let Some(title) = self.feature(project, slug).map(|f| f.title.clone()) else {
            return;
        };
        self.ask_sandbox(
            format!("Remove the sandbox of \u{201c}{title}\u{201d}?"),
            "Its containers and its copy of the data are lost.".into(),
            "Remove",
            project,
            slug,
            StopFor::Remove,
        );
    }

    /// Stop feature `slug`'s sandbox for `what`, as asked: its terminals close, then
    /// the rest goes off the thread.
    pub(crate) fn finish_stop(&mut self, project: &str, slug: &str, what: StopFor) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let Some(run) = feature.sandbox.clone() else {
            return self.sandbox_stopped(project, slug, what, Ok(()));
        };
        let key = (project.to_owned(), slug.to_owned());
        if self.features.build.sandbox_stops.contains_key(&key) {
            return self.show_warning(format!(
                "The sandbox of \u{201c}{title}\u{201d} is being started or stopped"
            ));
        }
        self.features
            .build
            .sandbox_stops
            .insert(key, Arc::new(AtomicBool::new(false)));
        for pane in &run.panes {
            self.close_herdr_pane(pane);
        }
        let dir = sandbox_dir(project, slug);
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Sandbox,
            repo: None,
            what: format!("Stopping the sandbox of \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::SandboxStopped {
            result: sandbox_run::stop(&run, &dir, what.removes()),
            project,
            slug,
            what,
        });
    }

    /// Stopping feature `slug`'s sandbox ended.
    pub(crate) fn sandbox_stopped(
        &mut self,
        project: &str,
        slug: &str,
        what: StopFor,
        result: Result<(), String>,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.sandbox_stops.remove(&key);
        if let Err(why) = result {
            tracing::warn!("stopping the sandbox of {slug}: {why}");
            return self.show_error(format!("The sandbox didn't stop cleanly: {why}"));
        }
        if let Some(feature) = self.feature(project, slug) {
            forget_out_of_reach(feature);
        }
        if let Some(f) = self.feature_mut(project, slug) {
            if what.removes() {
                f.sandbox = None;
            } else if let Some(run) = f.sandbox.as_mut() {
                run.stopped = true;
                run.panes.clear();
                run.holder = None;
                run.pasta = None;
            }
            self.save_feature(project, slug);
        }
        if what.starts_again() {
            self.start_sandbox(project, slug);
        } else if what.removes() {
            self.show_success("Sandbox removed".into());
        } else {
            self.show_success("Sandbox stopped, its data kept".into());
        }
    }

    /// Look for containers of sandboxes whose feature is gone, off the thread.
    pub(crate) fn look_for_sandbox_leftovers(&mut self) {
        self.feature_work(None, |_| {
            let never = AtomicBool::new(false);
            let found = sandbox_run::labelled(&never).unwrap_or_else(|why| {
                // Mostly docker not running: nothing to say to one who doesn't use it.
                tracing::debug!("looking for sandboxes left behind: {why}");
                Vec::new()
            });
            let stacks = found
                .into_iter()
                .filter(|(feature, _)| {
                    feature.split_once('/').is_some_and(|(project, slug)| {
                        !store::dir(None, project, slug)
                            .join("feature.json")
                            .exists()
                    })
                })
                .map(|(_, stack)| stack)
                .collect();
            Done::SandboxLeftovers { stacks }
        });
    }

    /// What `look_for_sandbox_leftovers` found: a toast offers to remove it.
    pub(crate) fn sandbox_leftovers(&mut self, stacks: Vec<String>) {
        if stacks.is_empty() {
            return;
        }
        let text = format!(
            "{} left from features that are gone: {}",
            crate::features::plural(stacks.len(), "sandbox"),
            stacks.join(", ")
        );
        tracing::warn!("{text}");
        self.toast_with(
            text,
            crate::toast::Level::Warning,
            Some((
                "Remove…".to_owned(),
                MenuAction::AskRemoveSandboxLeftovers(stacks),
            )),
        );
    }

    /// Remove what sandboxes of gone features left, as asked.
    pub(crate) fn remove_sandbox_leftovers(&mut self, stacks: Vec<String>) {
        self.feature_work(None, move |_| {
            let mut problems = Vec::new();
            for stack in &stacks {
                if let Err(why) = sandbox_run::remove_stack(stack) {
                    problems.push(format!("{stack}: {why}"));
                }
            }
            Done::SandboxLeftoversRemoved { problems }
        });
    }

    /// `remove_sandbox_leftovers` ended.
    pub(crate) fn sandbox_leftovers_removed(&mut self, problems: Vec<String>) {
        if problems.is_empty() {
            self.show_success("Removed what sandboxes left".into());
        } else {
            self.show_error(format!("Couldn't remove: {}", problems.join("; ")));
        }
    }
}

/// The source of sandboxes' problems on the "!" badge.
pub(crate) const PROBLEMS: &str = "sandbox";

/// What the problems list says of a sandbox that didn't start.
fn failed_text(title: &str, why: &str) -> String {
    format!("The sandbox of \u{201c}{title}\u{201d} didn't start: {why}")
}

/// What the problems list says of a running sandbox with `found` out of reach.
fn out_of_reach_problem(title: &str, found: &[String]) -> String {
    format!(
        "The sandbox of \u{201c}{title}\u{201d} {}",
        sandbox_run::out_of_reach_text(found)
    )
}

/// `feature`'s sandbox stopped or starts again: what it couldn't reach goes
/// from the problems list. A new run reports its own.
fn forget_out_of_reach(feature: &Feature) {
    if let Some(run) = (feature.sandbox.as_ref()).filter(|r| !r.out_of_reach.is_empty()) {
        let text = out_of_reach_problem(&feature.title, &run.out_of_reach);
        crate::problems::forget(PROBLEMS, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pages_are_told_from_others() {
        assert!(is_local_page("http://localhost:4200/x?y"));
        assert!(is_local_page("https://localhost"));
        assert!(is_local_page("http://127.0.0.1:9000"));
        assert!(is_local_page("http://[::1]:3000/a"));
        assert!(!is_local_page("http://proj.localhost:3773"));
        assert!(!is_local_page("https://github.com"));
        assert!(!is_local_page("chromium http://localhost:1"));
        assert!(!is_local_page("zed"));
    }

    fn feature_with(root: &Path) -> Feature {
        let mut repo = crate::features::tests::repo("api", true);
        repo.main = root.join("api");
        repo.worktree = root.join(".features/x/api");
        let mut f = crate::features::tests::feature("x", vec![repo], 0);
        f.root = root.to_owned();
        f
    }

    #[test]
    fn a_repository_held_runs_in_its_worktree_others_in_main() {
        let root = std::env::temp_dir().join(format!("gerdr-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(root.join("web")).unwrap();
        let f = feature_with(&root);
        let api_main = f.repos[0].main.display().to_string();
        let lists = PerProject::all(BTreeMap::from([
            (
                "api".to_owned(),
                vec![
                    format!("{api_main}/.venv/bin/python run.py"),
                    "# celery beat".to_owned(),
                    "  ".to_owned(),
                ],
            ),
            ("web".to_owned(), vec!["npm run dev".to_owned()]),
            ("gone".to_owned(), vec!["nope".to_owned()]),
        ]));
        let lines = lines_of(&f, &lists, false);
        let worktree = f.repos[0].worktree.clone();
        let runs: Vec<_> = (lines.iter().filter(|l| l.on))
            .map(|l| (l.dir.clone(), l.command.clone()))
            .collect();
        assert_eq!(
            runs,
            [
                (
                    worktree.clone(),
                    format!("{}/.venv/bin/python run.py", worktree.display())
                ),
                (root.join("web"), "npm run dev".to_owned()),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// `f`'s lines of a repository `api`, written as `written`.
    fn on_of(f: &Feature, written: &[&str]) -> Vec<(String, bool)> {
        let written = written.iter().map(|&line| line.to_owned()).collect();
        let lists = PerProject::all(BTreeMap::from([("api".to_owned(), written)]));
        (lines_of(f, &lists, false).into_iter())
            .map(|l| (l.words, l.on))
            .collect()
    }

    #[test]
    fn a_comment_is_listed_unchecked_and_a_pick_goes_over_what_the_project_says() {
        let root = std::env::temp_dir().join(format!("gerdr-picks-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut f = feature_with(&root);
        let written = ["runserver", "# celery beat", "#", "npm start"];
        assert_eq!(
            on_of(&f, &written),
            [
                ("runserver".to_owned(), true),
                ("celery beat".to_owned(), false),
                ("npm start".to_owned(), true),
            ]
        );
        let picks = BTreeMap::from([
            ("runserver".to_owned(), false),
            ("celery beat".to_owned(), true),
        ]);
        f.sandbox_picks.insert("api".to_owned(), picks);
        assert_eq!(
            on_of(&f, &written),
            [
                ("runserver".to_owned(), false),
                ("celery beat".to_owned(), true),
                // Not asked about yet: as the project says.
                ("npm start".to_owned(), true),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_line_says_where_and_when_it_runs() {
        let line = SandboxLine {
            repo: "web".to_owned(),
            name: "web".to_owned(),
            held: true,
            dir: PathBuf::new(),
            words: "npm start".to_owned(),
            command: "npm start".to_owned(),
            on: true,
            first: false,
        };
        assert_eq!(line.place(), "web");
        let outside = SandboxLine {
            held: false,
            first: true,
            ..line
        };
        assert_eq!(outside.place(), "web \u{b7} from main \u{b7} runs first");
    }

    #[test]
    fn only_a_comment_or_blank_still_counts_as_said() {
        let some =
            |line: &str| PerProject::all(BTreeMap::from([("a".to_owned(), vec![line.to_owned()])]));
        assert!(has_commands(&some("npm run dev")));
        // A comment says on purpose that nothing runs: containers alone.
        assert!(has_commands(&some("# nothing")));
        assert!(!has_commands(&some("  ")));
        assert!(!has_commands(&PerProject::all(BTreeMap::new())));
    }

    #[test]
    fn a_single_repository_is_its_root() {
        let root = std::env::temp_dir().join(format!("gerdr-single-{}", std::process::id()));
        let single = root.join("api");
        std::fs::create_dir_all(&single).unwrap();
        assert_eq!(main_folder(&single, "api"), Some(single.clone()));
        assert_eq!(main_folder(&root, "api"), Some(single.clone()));
        assert_eq!(main_folder(&root, "web"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
