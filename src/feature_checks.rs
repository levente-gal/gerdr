//! Verify's checks in terminals: a tab of panes in the feature's workspace,
//! each check in its own, all at once but for those of one folder, which take
//! turns (see `feature::verify` for what runs in them, and how it ends). A
//! terminal stays when its check ended, and the
//! next run of the check uses it again while it is at its prompt.
//!
//! The panes go on without gerdr: a run still under way when gerdr starts is
//! waited for again (`adopt_checks`), as what each check leaves is in files.
//!
//! Placed on another machine, a run's terminals are there, and opened from the
//! thread that waits for them (`machine_checks`). They show here only on
//! request (`show_check`), as an agent there does.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    Expected, Gerdr, Tab,
    feature::{
        Feature, Status, store,
        verify::{self, Begun, CheckRun, Outcome, Scope, Turn, Unread, Waited},
    },
    features::{Busy, Done, Job},
    herdr::Herdr,
    layout::{Dir, Node},
    prepare::{self, Prep},
    projects::VenvSetting,
};

/// What a tab of checks is called.
const TAB_LABEL: &str = "Checks";
/// How many terminals share a tab; more open another. Past this they get too
/// small to follow.
const TAB_PANES: usize = 6;
/// How many times in a row herdr must fail to know a pane before it counts
/// as closed.
const GONE_AFTER: u32 = 3;

/// What a new tab of `n` terminals does to lay them out: for each one after
/// the first, the one it splits off, how, and the share that one keeps. They
/// end up in a grid, of equal sizes.
fn grid(n: usize) -> Vec<(usize, Dir, f64)> {
    let columns = (1..=n).find(|c| c * c >= n).unwrap_or(1);
    let rows = |c: usize| n / columns + usize::from(c < n % columns);
    let mut splits = Vec::with_capacity(n.saturating_sub(1));
    let mut heads = vec![0];
    for c in 1..columns {
        let share = 1.0 / (columns - c + 1) as f64;
        splits.push((heads[c - 1], Dir::Right, share));
        heads.push(c);
    }
    let mut next = columns;
    for (c, head) in heads.iter().enumerate() {
        let mut last = *head;
        for r in 1..rows(c) {
            splits.push((last, Dir::Down, 1.0 / (rows(c) - r + 1) as f64));
            last = next;
            next += 1;
        }
    }
    splits
}

/// herdr, as `verify::watch` needs it, on a connection of its own.
pub(crate) struct HerdrPanes {
    pub(crate) herdr: Herdr,
    pub(crate) missed: HashMap<String, u32>,
    /// The server last didn't answer: told to the log once.
    pub(crate) busy: bool,
    /// It is reached over a link to another machine: no connection only says
    /// that machine is out of reach.
    pub(crate) over_link: bool,
    /// The checks' whole logs are kept with the AI work.
    pub(crate) keep_logs: bool,
}

impl HerdrPanes {
    /// Whether `err`, of a call about a pane, says that pane is gone.
    fn lost(&self, err: &str) -> bool {
        match self.over_link {
            true => Herdr::says_no_pane(err),
            false => self.herdr.pane_lost(err),
        }
    }
}

impl verify::Panes for HerdrPanes {
    fn gone(&mut self, pane: &str) -> bool {
        let Err(err) = self.herdr.process_info(pane) else {
            self.missed.remove(pane);
            self.busy = false;
            return false;
        };
        // A machine under load stalls the server for a minute or more. Its
        // terminals go on, and a check counted as closed gets killed.
        if !self.lost(&err) {
            if !self.busy {
                tracing::warn!("herdr didn't answer about the checks' pane {pane}: {err}");
                self.busy = true;
            }
            return false;
        }
        let missed = self.missed.entry(pane.to_owned()).or_default();
        *missed += 1;
        if *missed < GONE_AFTER {
            return false;
        }
        tracing::warn!("the checks' pane {pane} is gone: {err}");
        true
    }

    fn text(&mut self, pane: &str) -> Result<String, Unread> {
        let read = self
            .herdr
            .pane_read(pane, "recent_unwrapped", verify::READ_LINES);
        read.map_err(|err| match self.lost(&err) {
            true => Unread::Gone(err),
            false => Unread::Busy(err),
        })
    }

    fn interrupt(&mut self, pane: &str) {
        if let Err(err) = self.herdr.send_keys(pane, &["ctrl+c"]) {
            // It's killed after a moment all the same.
            tracing::warn!("stopping the check in pane {pane}: {err}");
        }
    }

    fn keep_log(&mut self, pane: &str) -> Option<String> {
        if !self.keep_logs {
            return None;
        }
        match self.herdr.pane_scrollback(pane) {
            Ok((text, _)) => Some(text),
            Err(err) => {
                // Its last lines are in the record all the same.
                tracing::warn!("reading all the check's pane {pane} kept: {err}");
                None
            }
        }
    }
}

/// Keep what check `i` of Verify run `run` printed, `scrollback` all its
/// terminal kept, in AI work folder `dir`, with how it ended.
fn keep_check_log(dir: &Path, run: &str, i: usize, outcome: &Outcome, scrollback: &str) {
    use crate::ai_work::{Event, Line, observe, write};
    let kept = write::check_log(dir, scrollback, verify::HEAD).and_then(|(log, truncated)| {
        let event = Event::CheckEnded {
            run: run.to_owned(),
            key: observe::check_key(&outcome.repo, &outcome.command),
            i,
            outcome: serde_json::to_value(outcome).unwrap_or_default(),
            log: Some(log),
            truncated,
        };
        write::append(dir, &Line::now(event))
    });
    // Each check's own: another kept doesn't clear it.
    let at = format!(
        "{}#{}",
        dir.display(),
        observe::check_key(&outcome.repo, &outcome.command)
    );
    match kept {
        Ok(()) => crate::problems::clear_at("ai-work", &at),
        Err(err) => crate::problems::report_at(
            "ai-work",
            &at,
            format!("Couldn't keep the log of {}: {err}", outcome.command),
        ),
    }
}

/// The outcome of a check that couldn't even start, because `why`.
pub(crate) fn not_started(row: &CheckRun, why: String) -> Outcome {
    Outcome {
        repo: row.repo.clone(),
        command: row.command.clone(),
        error: Some(why),
        interrupted: true,
        ..Outcome::default()
    }
}

impl Gerdr {
    /// The venv setting of feature `slug`'s workspace, as its terminals have
    /// it: the one found, with no workspace yet to say otherwise.
    pub(crate) fn feature_venv(&self, project: &str, slug: &str) -> VenvSetting {
        (self.feature_workspace(project, slug))
            .and_then(|w| self.workspaces[w].herdr_id())
            .and_then(|id| self.herdr_record(id))
            .map(|record| record.venv.clone())
            .unwrap_or_default()
    }

    /// The code of run `n` of feature `slug`'s checks was read as `begun`:
    /// they start, in their terminals.
    pub(crate) fn checks_begun(&mut self, project: &str, slug: &str, n: u32, begun: Begun) {
        let key = (project.to_owned(), slug.to_owned());
        let scope = self.features.build.scopes.remove(&key).unwrap_or_default();
        let feature = self.feature(project, slug).cloned();
        let run = (feature.as_ref())
            .and_then(|f| f.verify.as_ref())
            .filter(|v| v.n == n);
        let (Some(feature), Some(run)) = (feature.as_ref(), run) else {
            // The feature or the run went meanwhile: nothing to run.
            self.features.build.verifying.remove(&key);
            return self.unbusy(project, slug, Job::Verify);
        };
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        let checks = verify::checks(feature, &config);
        let holds = |repo: &str| run.holds(&begun, repo);
        let mut rows = verify::plan(&checks, &run.checks, &holds, &scope, n);
        // On the same code the user asked for just these: on changed code,
        // why the others don't run is worth a word.
        let same = run.patch.as_deref() == Some(begun.patch.as_str());
        let stands = rows.iter().filter(|row| row.outcome.is_some()).count();
        let total = rows.len();
        let dir = verify::run_dir(&store::dir(None, project, slug), n);
        // On another machine what opens them can't wait: it is told in the
        // thread that waits for them (`checks_there`).
        let there = run.machine.is_some();
        if !there {
            self.open_check_terminals(project, slug, feature, &mut rows, &dir);
        }
        if let Some(f) = self.feature_mut(project, slug)
            && let Some(run) = f.verify.as_mut()
        {
            run.checks = rows;
            run.patch = Some(begun.patch).filter(|p| !p.is_empty());
            run.heads = begun.heads;
            run.opened = !there;
        }
        if stands > 0 && !same {
            let why = "keep their results: their code didn't change";
            let title = &feature.title;
            self.show_notice(format!(
                "{stands} of {total} checks of \u{201c}{title}\u{201d} {why}"
            ));
            if let Some(f) = self.feature_mut(project, slug) {
                let line = format!("Verify run {n}: {stands} of {total} checks {why}");
                f.log(crate::feature::now(), line);
            }
        }
        self.save_feature(project, slug);
        self.watch_checks(project, slug, None);
        self.sync_check_cards();
    }

    /// Give each of `rows` yet to run a terminal in feature `slug`'s
    /// workspace, one it has at its prompt or a new one, and tell it to run.
    /// One that can't start gets its outcome now.
    fn open_check_terminals(
        &mut self,
        project: &str,
        slug: &str,
        feature: &Feature,
        rows: &mut [CheckRun],
        dir: &Path,
    ) {
        let mut pending: Vec<usize> = (0..rows.len())
            .filter(|i| rows[*i].outcome.is_none())
            .collect();
        if pending.is_empty() {
            return;
        }
        let refuse = |rows: &mut [CheckRun], at: &[usize], why: String| {
            tracing::warn!("checks of {slug}: {why}");
            for i in at {
                rows[*i].outcome = Some(not_started(&rows[*i], why.clone()));
            }
        };
        if let Err(err) = std::fs::create_dir_all(dir) {
            return refuse(
                rows,
                &pending,
                format!("couldn't make {}: {err}", dir.display()),
            );
        }
        for i in std::mem::take(&mut pending) {
            let script = verify::script(dir, i, &rows[i], slug);
            match crate::projects::write_atomic(&verify::script_path(dir, i), script.as_bytes()) {
                Ok(()) => pending.push(i),
                Err(err) => refuse(rows, &[i], format!("couldn't write its script: {err}")),
            }
        }
        let Some((w, _)) = self.ensure_feature_workspace(project, slug) else {
            let why = "its workspace couldn't open".to_owned();
            return refuse(rows, &pending, why);
        };
        let mut preps = Vec::new();
        let venv = self.feature_venv(project, slug);
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return refuse(rows, &pending, "herdr isn't connected".to_owned());
        };
        // A terminal at its prompt runs it again; any other gets a new one,
        // as the user may be at work in it.
        let mut fresh = Vec::new();
        let mut used: Vec<String> = Vec::new();
        for i in pending {
            let pane = rows[i].pane.take().filter(|pane| {
                !used.contains(pane)
                    && self.herdr_pane_place(pane).is_some()
                    && (self.herdr.as_mut()).is_some_and(|herdr| {
                        prepare::step(&herdr.process_info(pane)) == prepare::Step::Send
                    })
            });
            match pane {
                Some(pane) => {
                    used.push(pane.clone());
                    rows[i].pane = Some(pane);
                }
                None => fresh.push(i),
            }
        }
        self.open_new_check_panes(w, rows, &mut fresh);
        for i in fresh {
            let why = "couldn't open a terminal".to_owned();
            rows[i].outcome = Some(not_started(&rows[i], why));
        }
        for (i, row) in rows.iter().enumerate() {
            let Some(pane) = row.pane.clone().filter(|_| row.outcome.is_none()) else {
                continue;
            };
            let script = verify::script_path(dir, i);
            let script = crate::desktop::shell_quote(&script.to_string_lossy());
            preps.push(Prep {
                pane_id: pane,
                pane_cwd: row.dir.clone(),
                ws_dir: feature.dir.clone(),
                venv: venv.clone(),
                captured_env: Vec::new(),
                cd: Some(row.dir.clone()),
                command: Some(format!("sh {script}")),
            });
        }
        prepare::spawn_prepare(socket, preps);
    }

    /// New terminals for `rows` at the indices `fresh`, in the checks' tab
    /// of workspace `w` as far as it has room, else in new tabs, which don't
    /// take the screen. `fresh` keeps those that got none.
    fn open_new_check_panes(&mut self, w: usize, rows: &mut [CheckRun], fresh: &mut Vec<usize>) {
        let held = (self.workspaces[w].tabs.iter().enumerate())
            .rev()
            .find(|(_, tab)| tab.label.as_deref() == Some(TAB_LABEL))
            .map(|(t, tab)| (t, tab.panes.len() + tab.expected.len()))
            .filter(|(_, count)| *count > 0 && *count < TAB_PANES);
        if let Some((t, count)) = held {
            let room = (TAB_PANES - count).min(fresh.len());
            let mut opened = 0;
            for &i in &fresh[..room] {
                if !self.check_pane_beside(w, t, rows, i) {
                    break;
                }
                opened += 1;
            }
            fresh.drain(..opened);
            if opened < room {
                return;
            }
        }
        let dirs: Vec<PathBuf> = fresh.iter().map(|i| rows[*i].dir.clone()).collect();
        let panes = self.open_terminal_tabs(w, TAB_LABEL, &dirs);
        for (i, pane) in fresh.iter().zip(&panes) {
            rows[*i].pane = Some(pane.clone());
        }
        fresh.drain(..panes.len());
    }

    /// A terminal for `rows[i]` split off tab `t`'s last, sharing with it.
    /// Whether it opened.
    fn check_pane_beside(&mut self, w: usize, t: usize, rows: &mut [CheckRun], i: usize) -> bool {
        let tab = &self.workspaces[w].tabs[t];
        let Some(leaf) = tab.layout.as_ref().and_then(|l| l.leaves().last().copied()) else {
            return false;
        };
        let target = (tab.panes.iter())
            .find(|p| p.id == leaf)
            .and_then(|p| p.herdr.as_ref())
            .or_else(|| {
                let expected = tab.expected.iter().find(|e| e.pane == leaf);
                expected.and_then(|e| e.herdr.as_ref())
            })
            .map(|p| p.pane_id.clone());
        let Some(target) = target else {
            return false;
        };
        let split = Some((leaf, target, Dir::Right, 0.5));
        let cwd = rows[i].dir.clone();
        let Some((_, pane)) = self.background_pane(w, Some(t), split, &cwd, TAB_LABEL) else {
            return false;
        };
        rows[i].pane = Some(pane);
        true
    }

    /// New tabs of workspace `w` called `label`, as many as `TAB_PANES` asks
    /// for, with a terminal in each of `dirs`, in a grid. They don't take the
    /// screen. The herdr panes in `dirs`' order, up to the first that couldn't
    /// open.
    pub(crate) fn open_terminal_tabs(
        &mut self,
        w: usize,
        label: &str,
        dirs: &[PathBuf],
    ) -> Vec<String> {
        let mut panes = Vec::with_capacity(dirs.len());
        for dirs in dirs.chunks(TAB_PANES) {
            let opened = self.open_terminal_tab(w, label, dirs);
            let all = opened.len() == dirs.len();
            panes.extend(opened);
            if !all {
                break;
            }
        }
        panes
    }

    /// `open_terminal_tabs`, for what fits in one tab.
    fn open_terminal_tab(&mut self, w: usize, label: &str, dirs: &[PathBuf]) -> Vec<String> {
        let Some(first) = self.background_pane(w, None, None, &dirs[0], label) else {
            return Vec::new();
        };
        let t = self.workspaces[w].tabs.len() - 1;
        let mut opened = vec![first];
        for (k, (from, dir, share)) in grid(dirs.len()).into_iter().enumerate() {
            let (leaf, target) = opened[from].clone();
            let split = Some((leaf, target, dir, share));
            let Some(new) = self.background_pane(w, Some(t), split, &dirs[k + 1], label) else {
                break;
            };
            opened.push(new);
        }
        opened.into_iter().map(|(_, pane)| pane).collect()
    }

    /// Open a terminal in `cwd`, kept in the background: split off a leaf of
    /// tab `t` as `split` says, or in a new tab called `label`. Its leaf and
    /// herdr pane; `None` (logged) when it couldn't.
    fn background_pane(
        &mut self,
        w: usize,
        t: Option<usize>,
        split: Option<(u32, String, Dir, f64)>,
        cwd: &Path,
        label: &str,
    ) -> Option<(u32, String)> {
        let (target, dir) = match &split {
            Some((_, target, dir, _)) => (Some(target.clone()), *dir),
            None => (None, Dir::Right),
        };
        let pane = match self.open_herdr_pane(w, target, dir, cwd)? {
            Ok(pane) => pane,
            Err(err) => {
                tracing::warn!("herdr didn't open a terminal for {label}: {err}");
                return None;
            }
        };
        let Some(pid) = self.attach(&pane, cwd) else {
            if let Some(herdr) = self.herdr.as_mut()
                && let Err(err) = herdr.pane_close(&pane.pane_id)
            {
                tracing::warn!("herdr: {err}");
            }
            return None;
        };
        let id = self.new_pane_id();
        let pane_id = pane.pane_id.clone();
        let expected = Expected::terminal(pid, id, false, Some(pane));
        let tabs = &mut self.workspaces[w].tabs;
        match (t, split) {
            (Some(t), Some((leaf, _, dir, share))) => {
                let tab = &mut tabs[t];
                if let Some(layout) = tab.layout.as_mut() {
                    layout.split(leaf, id, dir, share);
                }
                tab.expected.push(expected);
            }
            _ => tabs.push(Tab {
                layout: Some(Node::Leaf(id)),
                expected: vec![expected],
                label: Some(label.to_owned()),
                ..Default::default()
            }),
        }
        Some((id, pane_id))
    }

    /// Wait off the thread for the checks of feature `slug`'s run that have
    /// not ended, and end it.
    pub(crate) fn watch_checks(&mut self, project: &str, slug: &str, busy: Option<Busy>) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(run) = feature.verify.clone() else {
            return;
        };
        let timeout = (self.projects.project(project))
            .and_then(|p| p.workflow.verify_timeout)
            .map_or(verify::TIMEOUT, std::time::Duration::from_secs);
        let there = match &run.machine {
            Some(id) => match self.machine_remote(id, &feature) {
                Some(remote) => Some(remote),
                // It goes on there: `adopt_checks` waits for it once it is back.
                None if run.opened => {
                    return tracing::info!(
                        "feature {project}/{slug}: its checks' machine isn't up"
                    );
                }
                // Gone before anything started there: nothing will start.
                None => return self.checks_never_opened(project, slug, id, &feature, &run),
            },
            None => None,
        };
        let socket = self.herdr.as_ref().map(|h| h.socket().to_path_buf());
        if there.is_none() && socket.is_none() {
            return;
        }
        let stops: Arc<Vec<AtomicBool>> =
            Arc::new(run.checks.iter().map(|_| AtomicBool::new(false)).collect());
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.stops.insert(key, Arc::clone(&stops));
        let data = store::dir(None, project, slug);
        let kept = self.ai_work.dir(project, slug);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(busy, move |tx| {
            let mut run = run;
            let n = run.n;
            let run_key = crate::ai_work::observe::verify_key(&run);
            verify::prune_runs(&data, n);
            let dir = verify::run_dir(&data, n);
            let begun = Begun {
                patch: run.patch.clone().unwrap_or_default(),
                heads: run.heads.clone(),
            };
            let at = (project.as_str(), slug.as_str(), n);
            let mut ran_on = None;
            if let Some(remote) = &there {
                ran_on = Some(remote.name.clone());
                if !run.opened
                    && let Some(evidence) = remote.open_checks(&feature, at, &mut run, &begun, &tx)
                {
                    let evidence = crate::feature_build::write_evidence(&feature, &data, evidence);
                    return Done::Verified {
                        project,
                        slug,
                        n,
                        evidence,
                    };
                }
            }
            let mut outcomes: Vec<Option<Outcome>> =
                run.checks.iter().map(|c| c.outcome.clone()).collect();
            let waited: Vec<Waited> = (run.checks.iter().enumerate())
                .filter(|(_, c)| c.outcome.is_none())
                .map(|(index, c)| Waited {
                    index,
                    repo: c.repo.clone(),
                    command: c.command.clone(),
                    pane: c
                        .pane
                        .as_deref()
                        .map(|p| crate::machine_work::split_pane(p).map_or(p, |(_, bare)| bare))
                        .unwrap_or_default()
                        .to_owned(),
                })
                .collect();
            let mut panes = HerdrPanes {
                herdr: Herdr::on_socket(match &there {
                    Some(remote) => remote.socket.clone(),
                    None => socket.unwrap_or_default(),
                }),
                missed: HashMap::new(),
                busy: false,
                over_link: there.is_some(),
                keep_logs: kept.is_some(),
            };
            let title = feature.title.clone();
            let total = outcomes.len();
            let turn_tx = tx.clone();
            let (turn_project, turn_slug) = (project.clone(), slug.clone());
            let mut turn = |index, turn: Turn| {
                // As for `CheckEnded` below.
                let _ = turn_tx.send(Done::CheckTurn {
                    project: turn_project.clone(),
                    slug: turn_slug.clone(),
                    n,
                    index,
                    turn,
                });
            };
            let back = there.as_ref().map(|remote| remote.paths.back());
            let mut ended = |index, mut outcome: Outcome| {
                let log = outcome.log.take();
                // What comes from there reads as it does here.
                if let Some(back) = &back {
                    outcome.tail = back.text(&outcome.tail);
                    outcome.excerpt = (outcome.excerpt.iter()).map(|l| back.text(l)).collect();
                }
                if let (Some(dir), Some(log)) = (&kept, log) {
                    let log = match &back {
                        Some(back) => back.text(&log),
                        None => log,
                    };
                    keep_check_log(dir, &run_key, index, &outcome, &log);
                }
                outcomes[index] = Some(outcome.clone());
                let done = outcomes.iter().flatten().count();
                // They fail only once gerdr quits: nothing is left to
                // show them in, and a new gerdr reads what the files say.
                let _ = tx.send(Done::CheckEnded {
                    project: project.clone(),
                    slug: slug.clone(),
                    n,
                    index,
                    outcome,
                });
                let _ = tx.send(Done::Progress {
                    project: project.clone(),
                    slug: slug.clone(),
                    job: Job::Verify,
                    what: format!("Verifying \u{201c}{title}\u{201d}: {done} of {total} done"),
                });
            };
            match &there {
                Some(remote) => {
                    let mut files = remote.files(n);
                    verify::watch_files(
                        waited, &mut files, timeout, &stops, &mut panes, &mut turn, &mut ended,
                    );
                }
                None => verify::watch(
                    waited, &dir, timeout, &stops, &mut panes, &mut turn, &mut ended,
                ),
            }
            let ended = outcomes.into_iter().flatten().collect();
            let mut evidence = verify::finish(&feature, &begun, run.started, ended);
            evidence.ran_on = ran_on;
            if let Some(remote) = &there {
                remote.after_checks(&feature, &begun, &mut evidence);
            }
            let evidence = crate::feature_build::write_evidence(&feature, &data, evidence);
            Done::Verified {
                project,
                slug,
                n,
                evidence,
            }
        });
    }

    /// Machine `id` went before run `run` of feature `slug` could start there:
    /// its checks end now, having said nothing of the code.
    fn checks_never_opened(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        feature: &Feature,
        run: &verify::VerifyRun,
    ) {
        let name = self.machine_name(id);
        let why = format!("couldn't run on {name}: it can't be reached");
        tracing::warn!("{project}/{slug}: {why}");
        let (feature, run) = (feature.clone(), run.clone());
        let data = store::dir(None, project, slug);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| {
            let begun = Begun {
                patch: run.patch.clone().unwrap_or_default(),
                heads: run.heads.clone(),
            };
            let outcomes = (run.checks.iter())
                .map(|row| (row.outcome.clone()).unwrap_or_else(|| not_started(row, why.clone())))
                .collect();
            let mut evidence = verify::finish(&feature, &begun, run.started, outcomes);
            evidence.ran_on = Some(name);
            Done::Verified {
                project,
                slug,
                n: run.n,
                evidence: crate::feature_build::write_evidence(&feature, &data, evidence),
            }
        });
    }

    /// The panes of run `n` of feature `slug`'s checks on another machine
    /// opened, or why not: they can be shown, and a gerdr started later waits
    /// for them.
    pub(crate) fn checks_opened(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        panes: Vec<(usize, Result<String, String>)>,
    ) {
        let run = (self.feature_mut(project, slug))
            .and_then(|f| f.verify.as_mut())
            .filter(|v| v.n == n);
        let Some(run) = run else {
            return;
        };
        run.opened = true;
        for (i, pane) in panes {
            if let (Some(row), Ok(pane)) = (run.checks.get_mut(i), pane) {
                row.pane = Some(pane);
            }
        }
        self.save_feature(project, slug);
        self.sync_check_cards();
    }

    /// Check `index` of run `n` of feature `slug` ended.
    pub(crate) fn check_ended(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        index: usize,
        outcome: Outcome,
    ) {
        let row = (self.feature_mut(project, slug))
            .and_then(|f| f.verify.as_mut())
            .filter(|v| v.n == n)
            .and_then(|v| v.checks.get_mut(index));
        let Some(row) = row else {
            return;
        };
        row.outcome = Some(outcome);
        row.queued = false;
        row.stopping = false;
        self.save_feature(project, slug);
        self.sync_check_cards();
    }

    /// Check `index` of run `n` of feature `slug` has to wait for its folder,
    /// or started.
    pub(crate) fn check_turn(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        index: usize,
        turn: Turn,
    ) {
        let row = (self.feature_mut(project, slug))
            .and_then(|f| f.verify.as_mut())
            .filter(|v| v.n == n)
            .and_then(|v| v.checks.get_mut(index));
        let Some(row) = row else {
            return;
        };
        match turn {
            Turn::Queued => row.queued = true,
            Turn::Began(at) => {
                row.queued = false;
                row.began = Some(at);
            }
        }
        self.save_feature(project, slug);
        self.sync_check_cards();
    }

    /// Wait again for the runs of the shown project's features that were
    /// under way when gerdr last stopped: their terminals went on.
    pub(crate) fn adopt_checks(&mut self, project: &str) {
        let here = self.herdr.is_some() && !self.herdr_tabs.is_empty();
        let Some(all) = self.features.all.get(project) else {
            return;
        };
        let lost: Vec<(String, String)> = all
            .iter()
            .filter(|f| f.status == Status::Open)
            .filter(|f| {
                (f.verify.as_ref()).is_some_and(|v| {
                    let place = match &v.machine {
                        Some(id) => self.work_of(id).is_some(),
                        None => here,
                    };
                    place && v.opened && v.finished.is_none()
                }) && !(self.features.build.verifying)
                    .contains(&(project.to_owned(), f.slug.clone()))
            })
            .map(|f| (f.slug.clone(), f.title.clone()))
            .collect();
        for (slug, title) in lost {
            tracing::info!("feature {project}/{slug}: waiting for its checks again");
            let key = (project.to_owned(), slug.clone());
            self.features.build.verifying.insert(key);
            let busy = Busy {
                project: project.to_owned(),
                slug: slug.clone(),
                job: Job::Verify,
                repo: None,
                what: format!("Verifying \u{201c}{title}\u{201d}"),
            };
            self.watch_checks(project, &slug, Some(busy));
        }
    }

    /// Show the terminal of feature `slug`'s check `index`.
    pub(crate) fn show_check(&mut self, project: &str, slug: &str, index: usize) {
        let pane = (self.feature(project, slug))
            .and_then(|f| f.verify.as_ref())
            .and_then(|v| v.checks.get(index))
            .and_then(|c| c.pane.clone());
        // On another machine what's wrong was said, in that machine's words.
        let there = pane
            .as_deref()
            .is_some_and(|p| crate::machine_work::split_pane(p).is_some());
        if !pane.is_some_and(|pane| self.focus_test_run(&pane)) && !there {
            self.show_warning("That check's terminal isn't open: Run again opens one".into());
        }
    }

    /// Run again feature `slug`'s check `index`. What the others said stands,
    /// where their code is the same.
    pub(crate) fn rerun_check(&mut self, project: &str, slug: &str, index: usize) {
        let row = (self.feature(project, slug))
            .and_then(|f| f.verify.as_ref())
            .and_then(|v| v.checks.get(index))
            .map(CheckRun::key);
        if let Some(key) = row {
            self.run_checks(project, slug, Scope::Only(vec![key]));
        }
    }

    /// Run again the checks of feature `slug` that didn't pass, and those
    /// of a repository that changed since they did.
    pub(crate) fn rerun_failed_checks(&mut self, project: &str, slug: &str) {
        let keys = (self.feature(project, slug))
            .and_then(|f| f.verify.as_ref())
            .map(verify::VerifyRun::not_passed)
            .unwrap_or_default();
        if keys.is_empty() {
            return self.show_warning("No check failed".into());
        }
        self.run_checks(project, slug, Scope::Only(keys));
    }

    /// Stop feature `slug`'s check `index`, or all of them, what they ran
    /// ended as Ctrl+C ends it. They can run again: nothing is lost.
    pub(crate) fn stop_checks(&mut self, project: &str, slug: &str, index: Option<usize>) {
        if !self.halt_checks(project, slug, index) {
            self.show_warning("No check runs now".into());
        }
    }

    /// `stop_checks` that says nothing: whether a run was under way. Its
    /// checks go when what asked for this no longer wants them.
    pub(crate) fn halt_checks(&mut self, project: &str, slug: &str, index: Option<usize>) -> bool {
        let key = (project.to_owned(), slug.to_owned());
        let Some(stops) = self.features.build.stops.get(&key) else {
            return false;
        };
        for (i, stop) in stops.iter().enumerate() {
            if index.is_none_or(|index| index == i) {
                stop.store(true, Ordering::Relaxed);
            }
        }
        // It may take a moment to end: its card says so meanwhile.
        if let Some(run) = self
            .feature_mut(project, slug)
            .and_then(|f| f.verify.as_mut())
        {
            for (i, row) in run.checks.iter_mut().enumerate() {
                if index.is_none_or(|index| index == i) && row.outcome.is_none() {
                    row.stopping = true;
                }
            }
        }
        true
    }
}

/// Whether check `row` passed.
pub(crate) fn check_passed(row: &CheckRun) -> bool {
    row.outcome.as_ref().is_some_and(Outcome::passed)
}

/// Whether `feature`'s Verify run is under way.
pub(crate) fn checks_running(feature: &Feature) -> bool {
    (feature.verify.as_ref()).is_some_and(|v| v.opened && v.finished.is_none())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
    };

    use super::*;
    use crate::feature::verify::Panes;

    /// A server that gives each of `replies` to one request. `pane_lost` only
    /// connects, and gets none.
    fn server(name: &str, replies: Vec<&'static str>) -> PathBuf {
        let socket = std::env::temp_dir().join(format!("gerdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let mut replies = replies.into_iter();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                if !line.is_empty()
                    && let Some(reply) = replies.next()
                {
                    stream.write_all(reply.as_bytes()).unwrap();
                }
            }
        });
        socket
    }

    fn panes_on(socket: &Path) -> HerdrPanes {
        HerdrPanes {
            herdr: Herdr::on_socket(socket.to_path_buf()),
            missed: HashMap::new(),
            busy: false,
            over_link: false,
            keep_logs: false,
        }
    }

    const NOT_FOUND: &str =
        "{\"error\":{\"code\":\"pane_not_found\",\"message\":\"pane not found\"}}\n";

    #[test]
    fn only_the_servers_word_makes_a_terminal_closed() {
        let socket = server("gone", vec![NOT_FOUND, NOT_FOUND, NOT_FOUND]);
        let mut panes = panes_on(&socket);
        assert!(!panes.gone("p"));
        assert!(!panes.gone("p"));
        assert!(panes.gone("p"));
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn a_server_with_trouble_answering_closes_no_terminal() {
        let not_json = "not json\n";
        let socket = server("stall", vec![not_json; 5]);
        let mut panes = panes_on(&socket);
        assert!((0..5).all(|_| !panes.gone("p")));
        assert!(panes.missed.is_empty());
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn no_server_at_all_closes_its_terminals() {
        let socket = std::env::temp_dir().join(format!("gerdr-none-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let mut panes = panes_on(&socket);
        assert!(!panes.gone("p"));
        assert!(!panes.gone("p"));
        assert!(panes.gone("p"));
        assert!(matches!(panes.text("p"), Err(Unread::Gone(_))));
    }

    /// The rows and columns the panes of `grid(n)` end in: it must be
    /// (nearly) square, with equal shares.
    fn shape(n: usize) -> Vec<(usize, Dir, f64)> {
        let splits = grid(n);
        assert_eq!(splits.len(), n - 1);
        for (i, (from, _, _)) in splits.iter().enumerate() {
            assert!(*from <= i, "pane {} splits one not yet there", i + 1);
        }
        splits
    }

    #[test]
    fn the_checks_lay_out_in_a_grid_of_equal_panes() {
        assert_eq!(shape(1), vec![]);
        assert_eq!(shape(2), vec![(0, Dir::Right, 0.5)]);
        assert_eq!(shape(3), vec![(0, Dir::Right, 0.5), (0, Dir::Down, 0.5)]);
        assert_eq!(
            shape(4),
            vec![
                (0, Dir::Right, 0.5),
                (0, Dir::Down, 0.5),
                (1, Dir::Down, 0.5),
            ]
        );
        // Three columns: the first keeps a third, then half of the rest.
        let six = shape(6);
        assert_eq!(six[0], (0, Dir::Right, 1.0 / 3.0));
        assert_eq!(six[1], (1, Dir::Right, 0.5));
        assert_eq!(six[2..].iter().filter(|s| s.1 == Dir::Down).count(), 3);
    }
}
