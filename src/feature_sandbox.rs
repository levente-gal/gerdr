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
//! A sandbox runs on the machine writing its feature, or on the one its page
//! picked (`SandboxRun::machine`); a gerdr elsewhere opens its app, looks at its
//! terminals and stops it from there (see `machine_sandbox`).
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

use serde::{Deserialize, Serialize};

use crate::{
    Gerdr,
    chrome::CommandsTab,
    feature::{
        self, Feature, PerProject, Stage,
        sandbox::Engine,
        sandbox_pages,
        sandbox_run::{self, Host, SandboxRun},
        store,
    },
    features::{Busy, Done, Job},
    machine_work::Placed,
    menu::MenuAction,
    prepare::{Prep, Tools, background_command, spawn_prepare},
};

/// What a tab of a sandbox's terminals is called.
const TAB_LABEL: &str = "Sandbox";

/// What stopping a sandbox is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub(crate) fn removes(self) -> bool {
        matches!(self, Self::Remove | Self::Reset)
    }

    pub(crate) fn starts_again(self) -> bool {
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
            // This machine's settings may name a repository by its folder here,
            // which needn't be its name on the machine that made the feature.
            let held = (feature.repos.iter()).find(|r| {
                r.project.as_deref() == project
                    && (r.name == *name || r.main.file_name().is_some_and(|f| f == name.as_str()))
            });
            let dir = held
                .map(|r| r.worktree.clone())
                .or_else(|| main_folder(root, name));
            let Some(dir) = dir else {
                tracing::warn!("no folder for repository {name}: its commands don't run");
                continue;
            };
            let name = held.map_or(name, |r| &r.name);
            let repo = feature::repo_key(project, name);
            let picks = feature.sandbox_picks.get(&repo);
            for command in commands {
                let command = command.trim();
                let words = command.trim_start_matches('#').trim();
                // Listed under both names, a command runs once.
                if words.is_empty() || out.iter().any(|l: &SandboxLine| l.dir == dir && l.words == words) {
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

/// Where page `page` of `run`'s app is, with `ports` listening in it: as its
/// record says, once that port listens, else as `set` says (the project's
/// `sandbox_url`, for a sandbox started before pages were kept), else what
/// looks most like an app.
pub(crate) fn sandbox_url(
    run: &SandboxRun,
    page: usize,
    ports: &[u16],
    set: Option<&str>,
) -> Option<String> {
    if let (None, Some(set)) = (run.pages.get(page), set) {
        return Some(set.to_owned());
    }
    let guess = |ports: &[u16]| sandbox_run::app_port(ports).map(|port| format!("http://localhost:{port}"));
    match run.pages.get(page) {
        // The other pages' ports are theirs: a dev server's is among the rest.
        Some(page) if page.guess => {
            let theirs: Vec<u16> = (run.pages.iter())
                .filter(|p| !p.guess)
                .filter_map(|p| local_port(&p.url))
                .collect();
            let rest: Vec<u16> = (ports.iter()).copied().filter(|p| !theirs.contains(p)).collect();
            guess(&rest)
        }
        // A port no one listens on yet would show the browser's error page.
        Some(page) => match local_port(&page.url) {
            Some(port) if !ports.contains(&port) => None,
            _ => Some(page.url.clone()),
        },
        None => guess(ports),
    }
}

/// What the user hears when page `page` of `run`'s app has nothing listening.
pub(crate) fn nothing_listens(run: &SandboxRun, page: usize) -> String {
    match run.pages.get(page) {
        Some(page) if page.guess => format!("Nothing listens in the sandbox yet, for {}", page.name),
        Some(page) => match local_port(&page.url) {
            Some(port) => format!(
                "Nothing listens on port {port} in the sandbox yet, for {}",
                page.name
            ),
            None => format!("Nothing listens in the sandbox yet, for {}", page.name),
        },
        None => "Nothing listens in the sandbox yet".to_owned(),
    }
}

/// Whether `command` opens a page on this machine's `localhost`, which means
/// the sandbox's own inside it. `*.localhost` isn't: T3 Code's pages are named
/// that way, and its server runs outside.
pub(crate) fn is_local_page(command: &str) -> bool {
    host_and_port(command).is_some_and(|(host, _)| matches!(host, "localhost" | "127.0.0.1" | "::1"))
}

/// The port a page on `localhost` is at.
pub(crate) fn local_port(command: &str) -> Option<u16> {
    let (host, port) = host_and_port(command)?;
    matches!(host, "localhost" | "127.0.0.1" | "::1").then_some(port)?
}

/// The host of a page `command` opens, and its port: `None` when what it says of
/// that is no port.
fn host_and_port(command: &str) -> Option<(&str, Option<u16>)> {
    let command = command.trim();
    let (rest, usual) = match command.strip_prefix("http://") {
        Some(rest) => (rest, 80),
        None => (command.strip_prefix("https://")?, 443),
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (host, port) = match host_port.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']').unwrap_or((v6, ""));
            (host, after.strip_prefix(':'))
        }
        None => match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        },
    };
    Some((host, port.map_or(Some(usual), |p| p.parse().ok())))
}

impl Gerdr {
    /// The sandbox running for a page `command` opened from folder `dir`: the
    /// namespace to open it in and the browser profile for it.
    pub(crate) fn sandbox_for(&self, command: &str, dir: &Path) -> Option<(i32, PathBuf)> {
        if !is_local_page(command) {
            return None;
        }
        let same = |a: &Path, b: &Path| a == b || a.canonicalize().ok() == b.canonicalize().ok();
        // Seen from a machine that runs it for another's feature, it is here too.
        let feature = (self.features.all.values().flatten())
            .chain(self.features.viewed.values().flatten())
            .find(|f| {
                same(&f.dir, dir)
                    && (f.sandbox.as_ref())
                        .is_some_and(|run| !run.on_another_machine() && run.alive())
            })?;
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
        let Some(machine) = self.sandbox_place(&feature) else {
            return;
        };
        if let Some(id) = &machine
            && self.set_up_there_first(&feature, id)
        {
            return;
        }
        let key = (project.to_owned(), slug.to_owned());
        let prepares = self.projects.workflows(&feature, |w| w.prepare.clone());
        let runs = self.projects.workflows(&feature, |w| w.run.clone());
        let (set_pages, set_url) = (self.projects.project(project))
            .map(|p| {
                (
                    p.workflow.sandbox_pages.clone(),
                    p.workflow.sandbox_url.clone(),
                )
            })
            .unwrap_or_default();
        let venv = self.feature_venv(project, slug);
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        let envs = self.projects.workflows(&feature, |w| w.sandbox_env.clone());
        for link in envs.links.values() {
            env.extend(link.clone());
        }
        env.extend(envs.home.clone());
        let ip = crate::sandbox_lan::lan_ip();
        for value in env.values_mut() {
            *value = crate::sandbox_lan::with_lan_ip(value, ip);
        }
        // A port is only taken on the machine that holds it.
        let taken: Vec<u16> = (self.features.all.values().flatten())
            .filter(|f| !(f.project == project && f.slug == slug))
            .filter_map(|f| f.sandbox.as_ref())
            .filter(|run| run.machine == machine)
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
                machine: machine.clone(),
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
        let there = machine.as_ref().and_then(|id| {
            self.sandbox_there_job(&feature, id, &env, taken.clone())
        });
        if machine.is_some() && there.is_none() {
            // The link went this moment: what it would have said comes next.
            return self.sandbox_started(
                project,
                slug,
                SandboxRun {
                    machine,
                    started: feature::now(),
                    finished: Some(feature::now()),
                    failed: Some("that machine can't be reached".to_owned()),
                    ..SandboxRun::default()
                },
                false,
                Some(Vec::new()),
            );
        }
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |tx| {
            let mut progress = |what: String| {
                // It fails only once gerdr quits: nothing is left to show it.
                let _ = tx.send(Done::Progress {
                    project: project.clone(),
                    slug: slug.clone(),
                    job: Job::Sandbox,
                    what: format!("Sandbox of \u{201c}{title}\u{201d}: {what}"),
                });
            };
            // Read from the worktrees here, which a machine running it has a copy of.
            let pages = {
                let lines: Vec<SandboxLine> = (lines_of(&feature, &runs, false).into_iter())
                    .filter(|l| l.on)
                    .collect();
                let ran: Vec<sandbox_pages::Ran> = (lines.iter())
                    .map(|l| sandbox_pages::Ran {
                        dir: &l.dir,
                        command: &l.command,
                    })
                    .collect();
                sandbox_pages::pages(&set_pages, set_url.as_deref(), &ran)
            };
            if let Some(there) = there {
                let (mut run, missing) = there.run(&stop, &mut progress);
                run.pages = pages;
                return Done::Sandbox {
                    stopped: stop.load(Ordering::Relaxed),
                    project,
                    slug,
                    run,
                    opened: Some(missing),
                };
            }
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
                host: &Host::Here,
                project: &project,
                slug: &slug,
                owner: &crate::machine_lease::own_id_made().unwrap_or_default(),
                repos: &feature.repos,
                roots: &roots,
                dir: &dir,
                prepare: &prepare,
                env: &env,
                taken: &taken,
                stop: &stop,
            };
            let mut run = sandbox_run::start(&job, &mut progress);
            run.pages = pages;
            Done::Sandbox {
                stopped: stop.load(Ordering::Relaxed),
                project,
                slug,
                run,
                opened: None,
            }
        });
    }

    /// Where feature's sandbox starts: `None` for here, in the outer `None` when
    /// it can't, which the user has heard. A sandbox that has a record stays on its
    /// machine, for its data is there; removed, it goes where its page says.
    fn sandbox_place(&mut self, feature: &Feature) -> Option<Option<String>> {
        if let Some(run) = &feature.sandbox {
            let Some(id) = &run.machine else {
                return Some(None);
            };
            if self.work_of(id).is_some() {
                return Some(Some(id.clone()));
            }
            let name = self.machine_name(id);
            self.show_warning(format!(
                "Its sandbox's data is on {name}: connect it, or remove the sandbox to start one elsewhere"
            ));
            return None;
        }
        match self.placed(feature, Stage::ManualTest) {
            Placed::Here => Some(None),
            Placed::There(id) => Some(Some(id)),
            Placed::Away(name) => {
                self.show_warning(format!(
                    "{name} can't be reached: connect it, or pick another place for the sandbox"
                ));
                None
            }
        }
    }

    /// Starting feature `slug`'s sandbox ended: its commands open, or what
    /// went wrong is said and what it had started goes. `opened`: its commands
    /// opened where it runs, all but those named.
    pub(crate) fn sandbox_started(
        &mut self,
        project: &str,
        slug: &str,
        run: SandboxRun,
        stopped: bool,
        opened: Option<Vec<String>>,
    ) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.sandbox_stops.remove(&key);
        let host = self.sandbox_host(&run);
        let Some(feature) = self.feature(project, slug) else {
            // The feature went meanwhile, and nothing else knows of these.
            let dir = self.sandbox_dir_of(project, slug, &run);
            let (Ok(host), Some(dir)) = (host, dir) else {
                return tracing::error!("the sandbox of a removed feature stays where it runs");
            };
            std::thread::spawn(move || {
                if let Err(why) = sandbox_run::stop(&run, &dir, true, &host) {
                    tracing::error!("removing the sandbox of a removed feature: {why}");
                }
            });
            return;
        };
        let title = feature.title.clone();
        if let Some(why) = run.failed.clone() {
            // Off the thread: it waits for what ran inside to end.
            let pieces = run.clone();
            if let Ok(host) = host {
                std::thread::spawn(move || {
                    if let Err(why) = pieces.kill_network(&host) {
                        tracing::warn!("a sandbox that failed to start: {why}");
                    }
                });
            }
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
        if !run.not_there.is_empty() {
            let name = run.machine.as_deref().map_or_else(String::new, |id| self.machine_name(id));
            self.show_warning(format!(
                "{} doesn't run on {name}, so the sandbox has no copy of it: start it there first",
                run.not_there.join(", ")
            ));
        }
        let holder = run.holder.as_ref().map(|h| h.pid);
        self.note_sandbox_there(project, slug, &run);
        if let Some(f) = self.feature_mut(project, slug) {
            f.sandbox = Some(run);
        }
        self.save_feature(project, slug);
        if let Some(missing) = opened {
            return self.say_sandbox_opened(&title, &missing);
        }
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
        let ip = crate::sandbox_lan::lan_ip();
        for (_, value) in &mut env {
            *value = crate::sandbox_lan::with_lan_ip(value, ip);
        }
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
        self.say_sandbox_opened(&feature.title, &missing);
    }

    /// A sandbox is up, with all its terminals but those for `missing`.
    fn say_sandbox_opened(&mut self, title: &str, missing: &[String]) {
        if missing.is_empty() {
            self.show_success(format!("The sandbox of \u{201c}{title}\u{201d} is running"));
        } else {
            self.show_error(format!(
                "Couldn't open a terminal for {}",
                missing.join(", ")
            ));
        }
    }

    /// Open page `page` of the sandbox's app (see `SandboxRun::pages`), in the
    /// feature's workspace. `None` opens the one after the last opened.
    pub(crate) fn open_sandbox_app(&mut self, project: &str, slug: &str, page: Option<usize>) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let key = (project.to_owned(), slug.to_owned());
        let count = (feature.sandbox.as_ref()).map_or(0, |run| run.pages.len());
        let page = page.unwrap_or_else(|| {
            let last = self.features.build.sandbox_pages.get(&key).copied();
            last.map_or(1, |last| last + 1) % count.max(1)
        });
        let run = feature.sandbox.clone();
        self.features.build.sandbox_pages.insert(key, page);
        if let Some(id) = (run.as_ref()).and_then(|run| run.machine.clone()) {
            return self.open_seen_sandbox_app(project, slug, &id, page);
        }
        let Some(run) = run.filter(SandboxRun::alive) else {
            return self.show_warning("The sandbox isn't running".into());
        };
        let ports = run.listening();
        match sandbox_url(&run, page, &ports, self.set_sandbox_url(project).as_deref()) {
            Some(url) => {
                self.sandbox_shown(project, slug);
                self.open_feature_page(project, slug, url);
            }
            None => self.show_warning(nothing_listens(&run, page)),
        }
    }

    /// The page the settings of `project` name for its sandbox's app.
    pub(crate) fn set_sandbox_url(&self, project: &str) -> Option<String> {
        (self.projects.project(project)).and_then(|p| p.workflow.sandbox_url.clone())
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
    pub(crate) fn ask_remove_sandbox_leftovers(&mut self, stacks: Vec<LeftStack>) {
        let mut body = format!(
            "{}\nTheir containers and data are lost.",
            self.left_stacks_text(&stacks, "\n")
        );
        if stacks.iter().any(|left| left.from.is_some()) {
            body.push_str(
                "\nAnother gerdr started it: if that gerdr still uses it, its sandbox is gone.",
            );
        }
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
        let writer = feature.writer.clone();
        let Some(run) = feature.sandbox.clone() else {
            return self.sandbox_stopped(project, slug, what, Ok(()));
        };
        let key = (project.to_owned(), slug.to_owned());
        if self.features.build.sandbox_stops.contains_key(&key) {
            return self.show_warning(format!(
                "The sandbox of \u{201c}{title}\u{201d} is being started or stopped"
            ));
        }
        if let Some(writer) = writer {
            return self.stop_from_afar(project, slug, what, &writer);
        }
        let host = match self.sandbox_host(&run) {
            Ok(host) => host,
            Err(name) => {
                return self.show_warning(format!(
                    "{name} can't be reached: the sandbox keeps running there"
                ));
            }
        };
        let Some(dir) = self.sandbox_dir_of(project, slug, &run) else {
            return;
        };
        self.features
            .build
            .sandbox_stops
            .insert(key, Arc::new(AtomicBool::new(false)));
        // The tabs of one kept on its old writer are that gerdr's: it closes them
        // once the sandbox is gone.
        for pane in run.panes.iter().filter(|_| !run.kept) {
            self.close_herdr_pane(pane);
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Sandbox,
            repo: None,
            what: format!("Stopping the sandbox of \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::SandboxStopped {
            result: sandbox_run::stop(&run, &dir, what.removes(), &host),
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
            // On its page too, as the one who asked may be on another machine.
            if let Some(f) = self.feature_mut(project, slug) {
                f.log(feature::now(), format!("Sandbox couldn't stop: {why}"));
            }
            self.save_feature(project, slug);
            return self.show_error(format!("The sandbox didn't stop cleanly: {why}"));
        }
        if let Some(feature) = self.feature(project, slug) {
            forget_out_of_reach(feature);
        }
        if let Some(f) = self.feature_mut(project, slug) {
            if what.starts_again() && what.removes() {
                // It starts again where it was: only its data goes.
                let machine = f.sandbox.take().and_then(|run| run.machine);
                f.sandbox = Some(SandboxRun {
                    machine,
                    stopped: true,
                    ..SandboxRun::default()
                });
            } else if what.removes() {
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
        self.report_left_there();
        // A gerdr that died copying left main's containers paused.
        let paused_in = (self.features.all.values().flatten())
            .filter(|f| (f.sandbox.as_ref()).is_none_or(|run| run.machine.is_none()))
            .map(|f| sandbox_dir(&f.project, &f.slug))
            .collect();
        self.look_for_left(None, Host::Here, paused_in);
    }

    /// Machine `id` is back: look there too.
    pub(crate) fn look_for_sandbox_leftovers_there(&mut self, id: &str) {
        if let Some(work) = self.work_of(id) {
            let host = Host::There(work.reach());
            self.look_for_left(Some(id.to_owned()), host, Vec::new());
        }
    }

    fn look_for_left(&mut self, machine: Option<String>, host: Host, paused_in: Vec<PathBuf>) {
        // A gerdr connected here answers for its own sandboxes.
        let up: Vec<String> = (self.machines.links.values())
            .filter_map(|link| link.work.as_ref().map(|work| work.id.clone()))
            .collect();
        let gerdr_there = (machine.as_deref())
            .and_then(|id| self.work_of(id))
            .map(|work| format!("{}/gerdr", work.info.site.data));
        self.feature_work(None, move |_| {
            let still_paused: Vec<String> = (paused_in.iter())
                .filter_map(|dir| sandbox_run::unpause_left(&host, dir).err())
                .collect();
            let never = AtomicBool::new(false);
            let found = sandbox_run::labelled(&host, &never).unwrap_or_else(|why| {
                // Mostly neither Docker nor Podman running: nothing to say to
                // one who uses neither.
                tracing::debug!("looking for sandboxes left behind: {why}");
                Vec::new()
            });
            // Not knowing who was there lately, no other gerdr's is offered.
            let reached = match (&host, gerdr_there) {
                (Host::There(reach), Some(gerdr)) => {
                    let script = crate::machine_reach::list_script(&gerdr);
                    match reach.script(&script, &never) {
                        Ok(listed) => Some(crate::machine_reach::parse_list(&listed)),
                        Err(why) => {
                            tracing::warn!("reading which gerdrs reached a machine: {why:?}");
                            None
                        }
                    }
                }
                (Host::There(_), None) => None,
                (Host::Here, _) => Some(
                    (crate::machine_reach::read_dir(&crate::machine_reach::dir_here()).into_iter())
                        .map(|(reached, _)| reached)
                        .collect(),
                ),
            };
            // With no id yet, nothing started here said it was this gerdr's.
            let own = crate::machine_views::own_id();
            let others = Others {
                up: &up,
                reached: reached.as_deref(),
                now: crate::machine_reach::now_secs(),
            };
            let at = machine.as_deref();
            let stacks = left_behind(found, at, own.as_deref(), &others, |project, slug| {
                feature_known(project, slug, at)
            });
            Done::SandboxLeftovers {
                machine,
                stacks,
                still_paused,
            }
        });
    }

    /// What a launch's look `here` couldn't unpause, of what gerdrs that died
    /// copying left paused: main's app waits on it meanwhile.
    pub(crate) fn sandbox_still_paused(&mut self, here: bool, still_paused: Vec<String>) {
        if !here {
            return;
        }
        if still_paused.is_empty() {
            crate::problems::clear(PAUSED_PROBLEMS);
            return;
        }
        let text = format!(
            "Containers of your main app stay paused after a sandbox copy was cut short: {}",
            still_paused.join("; ")
        );
        crate::problems::report(PAUSED_PROBLEMS, text.clone());
        self.show_warning(text);
    }

    /// What sandboxes of gone features left, as tasks: a row each, while its
    /// machine is connected, with Remove.
    pub(crate) fn sandbox_left_tasks(&self) -> Vec<crate::tasks::Task> {
        let shown = (self.machines.left.iter()).filter(|(machine, _)| match machine {
            Some(id) => self.work_of(id).is_some(),
            None => true,
        });
        let now = crate::machine_reach::now_secs();
        (shown.flat_map(|(_, stacks)| stacks))
            .map(|left| {
                let slug = (left.feature.split_once('/')).map_or(&*left.feature, |(_, slug)| slug);
                let mut detail = vec![match &left.machine {
                    Some(id) => format!("on {}", self.machine_name(id)),
                    None => "here".to_owned(),
                }];
                detail.push(left.stack.clone());
                let started = match &left.started {
                    day if day.is_empty() => "started".to_owned(),
                    day => format!("started {day}"),
                };
                detail.push(match &left.from {
                    Some(from) => format!("{started} by {}", self.left_from_text(from, now)),
                    None => started,
                });
                crate::tasks::Task {
                    title: format!("Sandbox left: {slug}"),
                    chip: "sandbox left".to_owned(),
                    detail: detail.join(" \u{b7} "),
                    dot: crate::chrome::Dot::Idle,
                    actions: vec![(
                        "Remove\u{2026}",
                        MenuAction::AskRemoveSandboxLeftovers(vec![left.clone()]),
                    )],
                }
            })
            .collect()
    }

    /// Who started a stack another gerdr left, and when it was last there.
    fn left_from_text(&self, from: &LeftFrom, now: u64) -> String {
        let name = if from.id.is_empty() {
            "an older gerdr".to_owned()
        } else if let Some(name) = self.machines.names.get(&from.id) {
            name.clone()
        } else if !from.name.is_empty() {
            from.name.clone()
        } else {
            "another gerdr".to_owned()
        };
        match from.seen {
            Some(at) => format!("{name}, last there {}", crate::projects::ago(now, at)),
            None => name,
        }
    }

    /// `stacks`, each saying where it is if that's not here.
    fn left_stacks_text(&self, stacks: &[LeftStack], between: &str) -> String {
        (stacks.iter())
            .map(|left| match &left.machine {
                Some(id) => format!("{} on {}", left.stack, self.machine_name(id)),
                None => left.stack.clone(),
            })
            .collect::<Vec<_>>()
            .join(between)
    }

    /// What `look_for_sandbox_leftovers` found on `machine` (none: here): each
    /// stays in the tasks panel to be removed. A toast offers this gerdr's own,
    /// once while it runs, however often a machine comes back.
    pub(crate) fn sandbox_leftovers(&mut self, machine: Option<String>, stacks: Vec<LeftStack>) {
        if stacks.is_empty() {
            self.machines.left.remove(&machine);
        } else {
            self.machines.left.insert(machine, stacks.clone());
        }
        let mut stacks: Vec<LeftStack> = stacks.into_iter().filter(|left| left.from.is_none()).collect();
        stacks.retain(|left| self.machines.left_told.insert(left.clone()));
        if stacks.is_empty() {
            return;
        }
        let text = format!(
            "{} left from features that are gone: {}",
            crate::features::plural(stacks.len(), "sandbox"),
            self.left_stacks_text(&stacks, ", ")
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
    pub(crate) fn remove_sandbox_leftovers(&mut self, stacks: Vec<LeftStack>) {
        let jobs: Vec<_> = (stacks.into_iter())
            .map(|left| {
                // If it can't be removed, it is said again when next found.
                self.machines.left_told.remove(&left);
                let (host, dir) = self.left_stack_place(&left);
                // Another gerdr's folder there is its own: only its containers go.
                let dir = dir.filter(|_| left.from.is_none());
                (left, host, dir)
            })
            .collect();
        self.feature_work(None, move |_| {
            let mut problems = Vec::new();
            let mut removed = Vec::new();
            for (left, host, dir) in jobs {
                let result = (host.as_ref())
                    .map_err(|name| format!("{name} can't be reached"))
                    .and_then(|host| {
                        sandbox_run::remove_stack(&left.stack, left.engine, dir.as_deref(), host)
                    });
                match result {
                    Ok(()) => removed.push(left),
                    Err(why) => problems.push(format!("{}: {why}", left.stack)),
                }
            }
            Done::SandboxLeftoversRemoved { removed, problems }
        });
    }

    /// `remove_sandbox_leftovers` ended: what went leaves the tasks panel.
    pub(crate) fn sandbox_leftovers_removed(&mut self, removed: Vec<LeftStack>, problems: Vec<String>) {
        for stacks in self.machines.left.values_mut() {
            stacks.retain(|left| !removed.contains(left));
        }
        self.machines.left.retain(|_, stacks| !stacks.is_empty());
        if problems.is_empty() {
            self.show_success("Removed what sandboxes left".into());
        } else {
            self.show_error(format!("Couldn't remove: {}", problems.join("; ")));
        }
    }
}

/// The source of sandboxes' problems on the "!" badge.
pub(crate) const PROBLEMS: &str = "sandbox";

/// The source of what stays paused after a copy cut short, apart: a
/// sandbox's own problems clear when it starts.
const PAUSED_PROBLEMS: &str = "sandbox-paused";

/// A compose project a sandbox of a feature that's gone left.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LeftStack {
    /// The machine it is on, if not this one.
    pub(crate) machine: Option<String>,
    /// What `sandbox::LABEL` says of it.
    pub(crate) feature: String,
    pub(crate) stack: String,
    /// The day it started, as Docker or Podman says.
    pub(crate) started: String,
    /// What runs it.
    pub(crate) engine: Engine,
    /// The gerdr that started it, if not this one.
    pub(crate) from: Option<LeftFrom>,
}

/// The gerdr that started a stack another one found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LeftFrom {
    /// Its machine id: empty for what started before owners were noted.
    pub(crate) id: String,
    /// What it calls itself, from what it left on that machine.
    pub(crate) name: String,
    /// When it was last there, from the same.
    pub(crate) seen: Option<u64>,
}

/// What tells another gerdr still around from one that is gone.
struct Others<'a> {
    /// The machines connected here.
    up: &'a [String],
    /// The gerdrs that reached the machine looked at; none if that couldn't
    /// be read.
    reached: Option<&'a [crate::machine_reach::Reached]>,
    now: u64,
}

/// Whether this gerdr knows feature `slug` still: its own, one it shows of another
/// machine, or one whose removal waits for `machine`.
fn feature_known(project: &str, slug: &str, machine: Option<&str>) -> bool {
    let data = crate::projects::data_dir();
    let has = |root: Option<&std::path::Path>| {
        store::dir(root, project, slug).join("feature.json").exists()
    };
    has(None)
        // Running here for the machine that writes it now.
        || (machine.is_none()
            && (crate::machine_lease::own_sandbox_dir(project, slug))
                .join(crate::feature::lease::KEPT)
                .exists())
        || (crate::feature::view::kept_ids(&data).iter())
            .any(|id| has(Some(&crate::feature::view::root(&data, id))))
        || machine
            .is_some_and(|id| crate::machine_sandbox::left_file(id, project, slug).exists())
}

/// Of what `found` on `machine` (none: here), what this gerdr can offer to remove:
/// the sandboxes of features it doesn't know. What says it is this gerdr's is its
/// own, and so is what started here before owners were noted. A machine can hold
/// the sandboxes of several gerdrs: another's is offered too, saying whose, unless
/// that gerdr is connected here or was on that machine lately, as it answers for
/// its own then.
fn left_behind(
    found: Vec<sandbox_run::Labelled>,
    machine: Option<&str>,
    own: Option<&str>,
    others: &Others,
    known: impl Fn(&str, &str) -> bool,
) -> Vec<LeftStack> {
    (found.into_iter())
        .filter(|left| {
            (left.feature.split_once('/')).is_some_and(|(project, slug)| !known(project, slug))
        })
        .filter_map(|left| {
            let mine = match left.owner.as_str() {
                "" => machine.is_none(),
                owner => own == Some(owner),
            };
            let from = match mine {
                true => None,
                false => {
                    let reached = others.reached?.iter().find(|r| r.id == left.owner);
                    let around = reached.is_some_and(|r| r.around(others.now));
                    if others.up.contains(&left.owner) || around {
                        return None;
                    }
                    Some(LeftFrom {
                        id: left.owner,
                        name: reached.map(|r| r.name.clone()).unwrap_or_default(),
                        seen: reached.map(|r| r.at),
                    })
                }
            };
            Some(LeftStack {
                machine: machine.map(str::to_owned),
                feature: left.feature,
                stack: left.stack,
                started: left.started,
                engine: left.engine,
                from,
            })
        })
        .collect()
}

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

    fn labelled(feature: &str, owner: &str) -> sandbox_run::Labelled {
        sandbox_run::Labelled {
            feature: feature.into(),
            stack: format!("gerdr-{feature}"),
            owner: owner.into(),
            started: "2026-11-07".into(),
            engine: Engine::Docker,
        }
    }

    const NOW: u64 = 1_000_000_000;

    fn no_others() -> Others<'static> {
        Others {
            up: &[],
            reached: Some(&[]),
            now: NOW,
        }
    }

    /// What is offered of a few stacks on `on`: each feature, and whose it is
    /// if not this gerdr's.
    fn found(on: Option<&str>, known: &[&str], others: &Others) -> Vec<(String, Option<String>)> {
        let all = vec![
            labelled("p/mine", "me"),
            labelled("p/known", "me"),
            labelled("p/other", "them"),
            labelled("p/old", ""),
            labelled("no slash", "me"),
        ];
        left_behind(all, on, Some("me"), others, |project, slug| {
            known.contains(&format!("{project}/{slug}").as_str())
        })
        .into_iter()
        .map(|left| (left.feature, left.from.map(|from| from.id)))
        .collect()
    }

    fn of(feature: &str, from: Option<&str>) -> (String, Option<String>) {
        (feature.to_owned(), from.map(str::to_owned))
    }

    #[test]
    fn a_gone_feature_is_this_gerdrs_only_if_its_sandbox_says_so() {
        assert_eq!(
            found(Some("w"), &["p/known"], &no_others()),
            [of("p/mine", None), of("p/other", Some("them")), of("p/old", Some(""))]
        );
        // Here, what started before owners were noted is this gerdr's.
        assert_eq!(
            found(None, &["p/known"], &no_others()),
            [of("p/mine", None), of("p/other", Some("them")), of("p/old", None)]
        );
        let all = ["p/known", "p/mine", "p/other", "p/old"];
        assert!(found(Some("w"), &all, &no_others()).is_empty());
    }

    #[test]
    fn another_gerdr_still_around_answers_for_its_own() {
        let up = ["them".to_owned()];
        let connected = Others { up: &up, ..no_others() };
        assert_eq!(
            found(Some("w"), &["p/known"], &connected),
            [of("p/mine", None), of("p/old", Some(""))]
        );
        let lately = [crate::machine_reach::Reached {
            id: "them".into(),
            name: "laptop".into(),
            at: NOW - 60,
            ..Default::default()
        }];
        let there = Others { reached: Some(&lately), ..no_others() };
        assert_eq!(found(Some("w"), &["p/known"], &there).len(), 2);
        let long_ago = [crate::machine_reach::Reached {
            at: NOW - crate::machine_reach::AROUND - 1,
            ..lately[0].clone()
        }];
        let gone = Others { reached: Some(&long_ago), ..no_others() };
        let all = vec![labelled("p/other", "them")];
        let left = left_behind(all, Some("w"), Some("me"), &gone, |_, _| false);
        let from = left[0].from.as_ref().unwrap();
        assert_eq!((from.name.as_str(), from.seen), ("laptop", Some(long_ago[0].at)));
        // Not knowing who was there, another gerdr's isn't offered.
        let unsure = Others { reached: None, ..no_others() };
        assert_eq!(found(Some("w"), &["p/known"], &unsure), [of("p/mine", None)]);
    }

    #[test]
    fn a_stack_left_on_a_machine_says_which() {
        let all = vec![labelled("p/mine", "me")];
        let left = left_behind(all, Some("w"), Some("me"), &no_others(), |_, _| false);
        assert_eq!(left[0].machine.as_deref(), Some("w"));
        assert_eq!(left[0].stack, "gerdr-p/mine");
        assert_eq!(left[0].started, "2026-11-07");
    }

    #[test]
    fn with_no_id_of_its_own_a_gerdr_claims_nothing_on_a_machine() {
        let all = vec![labelled("p/mine", "me"), labelled("p/old", "")];
        let left = left_behind(all, Some("w"), None, &no_others(), |_, _| false);
        assert!(left.iter().all(|left| left.from.is_some()));
    }

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

    #[test]
    fn a_guessed_page_opens_what_listens_past_the_other_pages() {
        use crate::feature::sandbox_pages::Page;
        let page = |name: &str, url: &str, guess: bool| Page {
            name: name.into(),
            url: url.into(),
            guess,
        };
        let run = SandboxRun {
            pages: vec![
                page("App", "http://localhost/", true),
                page("Web app", "http://localhost:9000/", false),
                page("Django admin", "http://localhost:9000/admin/", false),
            ],
            ..SandboxRun::default()
        };
        let url = |page, ports: &[u16]| sandbox_url(&run, page, ports, None);
        assert_eq!(url(0, &[9000, 6379, 4200]).as_deref(), Some("http://localhost:4200"));
        assert_eq!(url(2, &[9000, 4200]).as_deref(), Some("http://localhost:9000/admin/"));
        assert_eq!(url(0, &[9000]), None);
        assert_eq!(nothing_listens(&run, 0), "Nothing listens in the sandbox yet, for App");
    }

    #[test]
    fn a_local_page_says_its_port() {
        assert_eq!(local_port("http://localhost:4200/x?y"), Some(4200));
        assert_eq!(local_port("https://localhost"), Some(443));
        assert_eq!(local_port("http://127.0.0.1"), Some(80));
        assert_eq!(local_port("http://[::1]:3000/a"), Some(3000));
        assert_eq!(local_port("http://localhost:nope"), None);
        assert_eq!(local_port("http://proj.localhost:3773"), None);
        assert_eq!(local_port("https://github.com"), None);
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

    #[test]
    fn a_repository_named_by_its_folder_here_runs_in_its_worktree_once() {
        let root = PathBuf::from("/w/AssetTag");
        let mut f = feature_with(&root);
        f.repos[0].main = PathBuf::from("/w/Asset-Track");
        let lists = PerProject::all(BTreeMap::from([
            ("Asset-Track".to_owned(), vec!["runserver".to_owned()]),
            ("api".to_owned(), vec!["runserver".to_owned()]),
        ]));
        let lines = lines_of(&f, &lists, false);
        let runs: Vec<_> = (lines.iter())
            .map(|l| (l.repo.as_str(), l.dir.clone(), l.words.as_str()))
            .collect();
        assert_eq!(runs, [("api", root.join(".features/x/api"), "runserver")]);
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
