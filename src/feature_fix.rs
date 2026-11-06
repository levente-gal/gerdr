//! Fix rounds, gerdr's side (see `feature::fix_round`): the Build agent's
//! chat compacted, the plan asked for and watched, sent back, and the fixes
//! made once the user says so. Whatever sends work to a done Build agent
//! comes in through `Gerdr::send_to_build`.
//!
//! An agent that can't be reached is not a dead end: a fresh Build agent
//! takes the round on, as the plan and what it fixes are in files.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    Gerdr,
    confirm::Then,
    doc_review::DocFile,
    feature::{
        self, Feature, Stage, Status,
        agents::Flavor,
        fix_round::{self, Compact, FixPhase, FixRound, Fixes, GiveBack, Plan, VerifyFix},
        store,
    },
    feature_agents::{stamp, vars},
    feature_build::FRESH_BUILD_AGENT,
    herdr::AgentStatus,
    prompts, review,
};

/// What gerdr keeps of an open round while it watches it; not saved: a
/// round seen first after a restart starts its watch afresh.
#[derive(Clone, Debug)]
pub(crate) struct FixWatch {
    /// The agent was seen working since it was last told.
    worked: bool,
    /// When it was last told.
    since: Instant,
    /// The plan's stamp when its check last refused it.
    refused: Option<crate::feature_agents::Stamp>,
    /// Since when an agent making fixes showed no work and left the build
    /// log alone.
    rest: Instant,
    /// The build log's stamp when last read.
    log: Option<crate::feature_agents::Stamp>,
    /// The fixes left when it was last told, or first seen.
    left: Option<usize>,
    /// How many times it was told to go on with no more ticked since.
    told: u32,
}

impl Default for FixWatch {
    fn default() -> Self {
        FixWatch {
            worked: false,
            since: Instant::now(),
            refused: None,
            rest: Instant::now(),
            log: None,
            left: None,
            told: 0,
        }
    }
}

impl Gerdr {
    /// Why feature's fix work can't go to its Build agent now. None: it can.
    pub(crate) fn fix_round_refusal(&self, feature: &Feature) -> Option<String> {
        let title = &feature.title;
        if feature.fix_round.is_some() {
            return Some(format!(
                "\u{201c}{title}\u{201d} has a fix plan open: make the fixes or drop it first"
            ));
        }
        if self.build_agent_busy(feature) {
            return Some(format!(
                "The Build agent of \u{201c}{title}\u{201d} is busy: try again once it rests"
            ));
        }
        None
    }

    /// Whether feature's Build agent is at work or asks something: a line
    /// typed now would land in the middle of it.
    fn build_agent_busy(&self, feature: &Feature) -> bool {
        (feature.runs.get(&Stage::Build))
            .filter(|run| !run.held)
            .and_then(|run| run.agent_key())
            .is_some_and(|key| {
                matches!(
                    self.agent_key_state(&key).0,
                    Some(AgentStatus::Working | AgentStatus::Blocked)
                )
            })
    }

    /// The line compacting feature's Build agent's chat: none for a CLI
    /// gerdr knows no such line of, or an agent that isn't there or has no
    /// chat yet.
    fn compact_line(&self, feature: &Feature) -> Option<&'static str> {
        let run = feature.runs.get(&Stage::Build).filter(|r| !r.held)?;
        let key = run.agent_key()?;
        if !self.agent_key_state(&key).1 {
            return None;
        }
        Flavor::of(&run.base).compact()
    }

    /// Start a fix round on feature `slug`, of `what` in the file `input`;
    /// its refusal was looked at (`fix_round_refusal`). A failure says so.
    pub(crate) fn start_fix_round(
        &mut self,
        project: &str,
        slug: &str,
        what: &str,
        input: PathBuf,
    ) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        if !self.keep_last_plan(&feature) {
            return false;
        }
        let compact = self.compact_line(&feature);
        let phase = match compact {
            Some(_) => FixPhase::Compacting,
            None => FixPhase::Planning,
        };
        if !self.open_fix_round(&feature, what, input, phase) {
            return false;
        }
        let Some(line) = compact else {
            self.ask_fix_plan(project, slug);
            return true;
        };
        match self.tell_build_agent(&feature, line) {
            Ok(()) => {
                let title = &feature.title;
                self.show_success(format!(
                    "The Build agent of \u{201c}{title}\u{201d} compacts its chat, then plans the fixes"
                ));
            }
            Err(why) => {
                tracing::info!("compacting the Build agent of {project}/{slug}: {why}");
                self.ask_fix_plan(project, slug);
            }
        }
        true
    }

    /// Start a fix round on feature `slug` that asks for no plan: gerdr wrote
    /// `plan`, and it waits as an agent's would until `make_unplanned_fixes`.
    /// Nothing is told to the agent yet. A failure says so.
    pub(crate) fn start_unplanned_fix_round(
        &mut self,
        project: &str,
        slug: &str,
        what: &str,
        input: PathBuf,
        plan: &str,
    ) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        if !self.keep_last_plan(&feature) {
            return false;
        }
        let path = fix_round::path(&store::dir(None, project, slug));
        if let Err(err) = std::fs::write(&path, plan) {
            tracing::warn!("writing {}: {err}", path.display());
            self.show_error(format!("Couldn't write {}: {err}", fix_round::FILE));
            return false;
        }
        self.open_fix_round(&feature, what, input, FixPhase::Ready)
    }

    /// Move `feature`'s last plan out of the way of a new round. A failure
    /// says so.
    fn keep_last_plan(&mut self, feature: &Feature) -> bool {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let data = store::dir(None, project, slug);
        match archive_plan(&data, feature.fix_rounds) {
            Ok(()) => true,
            Err(why) => {
                tracing::warn!("keeping the last fix plan of {project}/{slug}: {why}");
                self.show_error(format!("Couldn't keep the last fix plan: {why}"));
                false
            }
        }
    }

    /// Record `feature`'s new round, at `phase`.
    fn open_fix_round(
        &mut self,
        feature: &Feature,
        what: &str,
        input: PathBuf,
        phase: FixPhase,
    ) -> bool {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let n = feature.fix_rounds + 1;
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return false;
        };
        f.fix_rounds = n;
        f.last_fix_round = None;
        f.fix_round = Some(FixRound {
            n,
            what: what.to_owned(),
            input,
            stage: feature.current().unwrap_or(Stage::Build),
            phase,
            sent_back: 0,
            asked: now,
            ready: (phase == FixPhase::Ready).then_some(now),
            stalled: None,
            give_back: GiveBack::Nothing,
            items: Vec::new(),
            done: 0,
        });
        f.log(now, format!("Fix round {n} started: {what}"));
        self.save_feature(project, slug);
        self.features
            .build
            .fix_watches
            .insert(fix_key(project, slug), FixWatch::default());
        true
    }

    /// Say what the open round of feature `slug` gives back if it is
    /// dropped: what its starter took, known once that took it.
    pub(crate) fn fix_round_gives_back(&mut self, project: &str, slug: &str, back: GiveBack) {
        let Some(round) = self
            .feature_mut(project, slug)
            .and_then(|f| f.fix_round.as_mut())
        else {
            return;
        };
        round.give_back = back;
        self.save_feature(project, slug);
    }

    /// Tell the Build agent of feature `slug` to write its round's plan, or
    /// start a fresh one on it.
    pub(crate) fn ask_fix_plan(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(round) = feature.fix_round.clone() else {
            return;
        };
        let data = store::dir(None, project, slug);
        let plan = fix_round::path(&data).display().to_string();
        let input = round.input.display().to_string();
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Build);
        values.insert("fixes.what".into(), round.what.clone());
        values.insert("fixes.path".into(), input.clone());
        values.insert("fix_plan.path".into(), plan.clone());
        let sent = prompts::render_builtin("fix-plan", &values)
            .map_err(|e| e.to_string())
            .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        let title = &feature.title;
        let text = match sent {
            Ok(()) => format!("The Build agent of \u{201c}{title}\u{201d} plans the fixes"),
            Err(why) => {
                tracing::info!("asking {project}/{slug}'s Build agent for a fix plan: {why}");
                let fixes = match std::fs::read_to_string(&round.input) {
                    Ok(fixes) => fixes,
                    Err(err) => {
                        tracing::warn!("reading {input}: {err}");
                        return self.fix_round_stalled(
                            project,
                            slug,
                            format!("Couldn't read {input}: {err}"),
                        );
                    }
                };
                let extra = [
                    ("fixes", fixes),
                    ("fixes.path", input),
                    ("fixes.what", round.what.clone()),
                    ("fix_plan.path", plan),
                ];
                if !self.launch_fresh_build(project, slug, &extra) {
                    // It said why.
                    return self.fix_round_stalled_quietly(
                        project,
                        slug,
                        "the Build agent couldn't start".into(),
                    );
                }
                format!("Planning the fixes of \u{201c}{title}\u{201d} went to {FRESH_BUILD_AGENT}")
            }
        };
        let now = feature::now();
        if let Some(round) = self
            .feature_mut(project, slug)
            .and_then(|f| f.fix_round.as_mut())
        {
            round.phase = FixPhase::Planning;
            round.asked = now;
            round.ready = None;
            round.stalled = None;
        }
        self.save_feature(project, slug);
        self.features
            .build
            .fix_watches
            .insert(fix_key(project, slug), FixWatch::default());
        self.show_success(text);
    }

    /// Why the round of feature `slug` isn't getting on, said once.
    fn fix_round_stalled(&mut self, project: &str, slug: &str, why: String) {
        self.show_warning(why.clone());
        self.fix_round_stalled_quietly(project, slug, why);
    }

    /// The same, already said.
    fn fix_round_stalled_quietly(&mut self, project: &str, slug: &str, why: String) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        f.log(now, format!("Fix plan stalled: {why}"));
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Planning;
            round.stalled = Some(why);
        }
        self.save_feature(project, slug);
    }

    /// Look at the open fix rounds of `project`. Called after each read of
    /// herdr's agents.
    pub(crate) fn fix_rounds_tick(&mut self, project: &str) {
        let Some(all) = self.features.all.get(project) else {
            return;
        };
        let open: Vec<Feature> = (all.iter())
            .filter(|f| f.fix_round.is_some())
            .cloned()
            .collect();
        for feature in open {
            self.fix_round_tick(&feature);
        }
    }

    fn fix_round_tick(&mut self, feature: &Feature) {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let Some(round) = feature.fix_round.as_ref() else {
            return;
        };
        if feature.status != Status::Open {
            return self.end_fix_round(project, slug, false);
        }
        if round.before_fixing() && feature.current() != Some(round.stage) {
            let (title, stage) = (&feature.title, round.stage.label());
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} left {stage}, so its fix plan was dropped"
            ));
            return self.end_fix_round(project, slug, true);
        }
        let run = feature.runs.get(&Stage::Build);
        let key = fix_key(project, slug);
        match round.phase {
            FixPhase::Ready => {}
            FixPhase::Fixing => self.fixes_tick(feature, round),
            FixPhase::Compacting | FixPhase::CompactingToFix => {
                let state = run.and_then(|r| r.agent_key());
                let (status, alive) = match &state {
                    Some(agent) => self.agent_key_state(agent),
                    None => (None, false),
                };
                let watch = self.features.build.fix_watches.entry(key).or_default();
                let since = watch.since.elapsed();
                match fix_round::compact_next(watch.worked, status, alive, since) {
                    Compact::Wait => {}
                    Compact::Worked => watch.worked = true,
                    Compact::Go if round.phase == FixPhase::Compacting => {
                        self.ask_fix_plan(project, slug);
                    }
                    Compact::Go => {
                        let said = "Chat compacted: fixes sent to the Build agent";
                        if !self.send_fixes(project, slug, said) {
                            // It said why: they can go again from the plan.
                            self.fix_plan_waits_again(project, slug);
                        }
                    }
                }
            }
            FixPhase::Planning => {
                // Not at work yet: picking its model isn't planning.
                if run.is_some_and(|r| r.held) {
                    let watch = self.features.build.fix_watches.entry(key).or_default();
                    watch.since = Instant::now();
                    return;
                }
                let agent = run.and_then(|r| r.agent_key());
                let (status, alive) = match &agent {
                    Some(agent) => self.agent_key_state(agent),
                    None => (None, false),
                };
                let data = store::dir(None, project, slug);
                let path = fix_round::path(&data);
                let at = stamp(&path);
                let written = at.is_some_and(|(at, _)| at >= round.asked);
                let watch = self.features.build.fix_watches.entry(key).or_default();
                let since = watch.since.elapsed();
                let refused = watch.refused;
                match fix_round::plan_next(watch.worked, status, alive, since, written) {
                    Plan::Wait => {}
                    Plan::Worked => {
                        watch.worked = true;
                        if round.stalled.is_some() {
                            self.fix_round_going(project, slug);
                        }
                    }
                    Plan::Read if at != refused => self.fix_plan_read(feature, &path, at),
                    Plan::Read => {}
                    Plan::Stopped if round.stalled.is_none() => {
                        let title = &feature.title;
                        self.fix_round_stalled(
                            project,
                            slug,
                            format!(
                                "The Build agent of \u{201c}{title}\u{201d} stopped without writing its fix plan"
                            ),
                        );
                    }
                    Plan::Gone if round.stalled.is_none() => {
                        let title = &feature.title;
                        self.fix_round_stalled(
                            project,
                            slug,
                            format!(
                                "The Build agent of \u{201c}{title}\u{201d} is gone before writing its fix plan"
                            ),
                        );
                    }
                    Plan::Stopped | Plan::Gone => {}
                }
            }
        }
    }

    /// Look at the agent making the fixes of `round`: it goes on until
    /// every one is ticked in the build log, whatever it stops for on the
    /// way, and the round ends then. One that won't is the user's to settle.
    fn fixes_tick(&mut self, feature: &Feature, round: &FixRound) {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let run = feature.runs.get(&Stage::Build);
        let key = fix_key(project, slug);
        // A fresh agent's work starts once its prompt goes.
        if run.is_some_and(|r| r.held) {
            let watch = self.features.build.fix_watches.entry(key).or_default();
            watch.rest = Instant::now();
            return;
        }
        let path = build_log(&store::dir(None, project, slug));
        let log = match std::fs::read_to_string(&path) {
            Ok(log) => log,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                String::new()
            }
        };
        let left = fix_round::left(&log, round.n, &round.items);
        let blocked = feature::artifacts::blocked(&log);
        let at = stamp(&path);
        let (status, alive) = match run.and_then(|r| r.agent_key()) {
            Some(agent) => self.agent_key_state(&agent),
            None => (None, false),
        };
        // Its line isn't seen taken yet: it hasn't rested on it.
        let told_now = (run.and_then(|r| r.pane.as_deref()))
            .is_some_and(|pane| {
                self.machine_line_pending(pane)
                    || self.herdr.as_ref().is_some_and(|h| h.line_pending(pane))
            });
        let watch = self.features.build.fix_watches.entry(key).or_default();
        if watch.log != at || told_now {
            watch.log = at;
            watch.rest = Instant::now();
        }
        if watch.left.is_none_or(|was| left.len() < was) {
            watch.left = Some(left.len());
            watch.told = 0;
        }
        let (rested, told) = (watch.rest.elapsed(), watch.told);
        let done = u32::try_from(round.items.len() - left.len()).unwrap_or(u32::MAX);
        // AI review's page says which of its findings are made by now.
        let made = (self.feature_mut(project, slug))
            .and_then(|f| f.review.as_mut())
            .is_some_and(|review| fix_round::mark_made(review, round, &left));
        if made {
            self.feature_findings_changed(project, slug);
        }
        if done != round.done || made {
            if let Some(round) =
                (self.feature_mut(project, slug)).and_then(|f| f.fix_round.as_mut())
            {
                round.done = done;
            }
            self.save_feature(project, slug);
            // Build's page lists them, ticks and all.
            self.probe_feature(project, slug);
        }
        let title = &feature.title;
        let open = left.join("; ");
        let n = match left.len() {
            1 => "1 fix".to_owned(),
            n => format!("{n} fixes"),
        };
        match fix_round::fixes_next(status, alive, rested, left.len(), blocked.is_some(), told) {
            Fixes::Wait => {}
            Fixes::Worked => {
                let watch = (self.features.build.fix_watches)
                    .entry(fix_key(project, slug))
                    .or_default();
                watch.rest = Instant::now();
                if round.stalled.is_some() {
                    self.fix_round_going(project, slug);
                }
            }
            Fixes::Done => self.fixes_made(feature),
            // Said once: it stays stopped until the user sends them again.
            _ if round.stalled.is_some() => {}
            Fixes::GoOn => self.fixes_go_on(feature, round, &open),
            Fixes::Blocked => self.fixes_stopped(
                feature,
                format!(
                    "The Build agent of \u{201c}{title}\u{201d} is blocked with {n} left: {}",
                    blocked.unwrap_or_default()
                ),
            ),
            Fixes::Stopped => self.fixes_stopped(
                feature,
                format!("The Build agent of \u{201c}{title}\u{201d} stopped with {n} left: {open}"),
            ),
            Fixes::Gone => self.fixes_stopped(
                feature,
                format!("The Build agent of \u{201c}{title}\u{201d} is gone with {n} left: {open}"),
            ),
        }
    }

    /// Tell the agent resting with fixes `open` to go on.
    fn fixes_go_on(&mut self, feature: &Feature, round: &FixRound, open: &str) {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let title = &feature.title;
        let data = store::dir(None, project, slug);
        let names = self.projects.names(feature);
        let mut values = vars(feature, &names, &data, Stage::Build);
        let plan = fix_round::path(&data).display().to_string();
        values.insert("fix_plan.path".into(), plan);
        values.insert("fixes.section".into(), fix_round::heading(round.n));
        values.insert("fixes.left".into(), open.to_owned());
        let sent = prompts::render_builtin("fix-on", &values)
            .map_err(|e| e.to_string())
            .and_then(|line| self.tell_build_agent(feature, line.trim()));
        if let Err(why) = sent {
            tracing::info!("telling {project}/{slug}'s Build agent to go on with fixes: {why}");
            return self.fixes_stopped(
                feature,
                format!(
                    "Couldn't tell the Build agent of \u{201c}{title}\u{201d} to go on with its fixes: {why}"
                ),
            );
        }
        let watch = (self.features.build.fix_watches)
            .entry(fix_key(project, slug))
            .or_default();
        watch.told += 1;
        watch.rest = Instant::now();
        if let Some(f) = self.feature_mut(project, slug) {
            f.log(
                feature::now(),
                format!("Fixes not all made: told to go on with {open}"),
            );
        }
        self.save_feature(project, slug);
    }

    /// The agent won't get on with the round's fixes, as `why` says: the
    /// user is told, once, and can send it again or drop the plan.
    fn fixes_stopped(&mut self, feature: &Feature, why: String) {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let pane = feature.runs.get(&Stage::Build).and_then(|r| r.pane.clone());
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        f.log(feature::now(), format!("Fixes stopped: {why}"));
        if let Some(round) = f.fix_round.as_mut() {
            round.stalled = Some(why.clone());
        }
        self.save_feature(project, slug);
        self.notify_feature(project, slug, pane, &why);
        self.show_warning(why);
    }

    /// Every fix of the round is ticked and its agent rests: the round is
    /// over, boxes its agent added as milestones are gone from the log, and
    /// its code is read.
    fn fixes_made(&mut self, feature: &Feature) {
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let data = store::dir(None, project, slug);
        let names = self.projects.names(feature);
        let values = vars(feature, &names, &data, Stage::Build);
        let path = build_log(&data);
        if let (Some(planned), Ok(log)) = (
            crate::feature_build::planned_milestones(&values),
            std::fs::read_to_string(&path),
        ) {
            let tidy = fix_round::without_added_milestones(&log, planned);
            if tidy != log
                && let Err(err) = std::fs::write(&path, tidy)
            {
                tracing::warn!("writing {}: {err}", path.display());
                self.show_error(format!("Couldn't tidy {}: {err}", path.display()));
            }
        }
        self.end_fix_round(project, slug, false);
        self.probe_feature(project, slug);
        let title = &feature.title;
        self.show_success(format!("Every fix of \u{201c}{title}\u{201d} is made"));
    }

    /// The agent works on a plan again.
    fn fix_round_going(&mut self, project: &str, slug: &str) {
        if let Some(round) = self
            .feature_mut(project, slug)
            .and_then(|f| f.fix_round.as_mut())
        {
            round.stalled = None;
        }
        self.save_feature(project, slug);
    }

    /// Read the plan at `path`, new with stamp `at`: up for review if good.
    fn fix_plan_read(
        &mut self,
        feature: &Feature,
        path: &Path,
        at: Option<crate::feature_agents::Stamp>,
    ) {
        let (project, slug, title) = (
            feature.project.as_str(),
            feature.slug.as_str(),
            &feature.title,
        );
        let ids = feature
            .fix_round
            .as_ref()
            .map(FixRound::ids)
            .unwrap_or_default();
        let missing = match std::fs::read_to_string(path) {
            Ok(text) => fix_round::missing(&text, &ids),
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                vec![format!("to be readable: {err}")]
            }
        };
        if !missing.is_empty() {
            let watch = (self.features.build.fix_watches)
                .entry(fix_key(project, slug))
                .or_default();
            watch.refused = at;
            let lacks = missing.join(", ");
            return self.fix_round_stalled(
                project,
                slug,
                format!("The fix plan of \u{201c}{title}\u{201d} needs {lacks}"),
            );
        }
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Ready;
            round.ready = Some(now);
            round.stalled = None;
        }
        f.log(now, "Fix plan ready for review");
        self.save_feature(project, slug);
        (self.features.build.fix_watches).remove(&fix_key(project, slug));
        self.doc_reread_shown(project, slug, DocFile::FixPlan);
        let text = format!("The fix plan of \u{201c}{title}\u{201d} is ready to review");
        self.notify_feature(project, slug, None, &text);
        self.show_success(text);
    }

    /// Send the plan of feature `slug` back with the user's `comments`.
    pub(crate) fn fix_plan_send_back(&mut self, project: &str, slug: &str, comments: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = &feature.title;
        let last = feature
            .last_fix_round
            .clone()
            .filter(|_| feature.status == Status::Open);
        let open = feature.fix_round.clone();
        let Some(round) = (match open {
            Some(round) => Some(round).filter(FixRound::before_fixing),
            None => last,
        }) else {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} has no fix plan waiting for review"
            ));
        };
        if self.build_agent_busy(&feature) {
            return self.show_warning(format!(
                "The Build agent of \u{201c}{title}\u{201d} is busy: try again once it rests"
            ));
        }
        let data = store::dir(None, project, slug);
        let plan = fix_round::path(&data).display().to_string();
        let back = round.sent_back + 1;
        let file = data
            .join("prompts")
            .join(format!("fix-plan-{}-back-{back}.md", round.n));
        let comments = format!(
            "{comments}\n\nThe user may have edited the file: read it again and keep their edits.\n"
        );
        let shown = file.display().to_string();
        let written = std::fs::create_dir_all(data.join("prompts"))
            .and_then(|()| std::fs::write(&file, &comments));
        if let Err(err) = written {
            tracing::warn!("writing {shown}: {err}");
            return self.show_error(format!("Couldn't write {shown}: {err}"));
        }
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Build);
        values.insert("artifact.path".into(), plan.clone());
        values.insert("comments.path".into(), shown);
        let sent = prompts::render_builtin("send-back", &values)
            .map_err(|e| e.to_string())
            .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        let text = match sent {
            Ok(()) => {
                format!("Sent the fix plan of \u{201c}{title}\u{201d} back to the Build agent")
            }
            Err(why) => {
                tracing::info!("sending the fix plan of {project}/{slug} back: {why}");
                // The plan and the comments say enough without it.
                let fixes = std::fs::read_to_string(&round.input).unwrap_or_else(|err| {
                    tracing::warn!("reading {}: {err}", round.input.display());
                    String::new()
                });
                let fixes = format!(
                    "{fixes}\n\nThe user read the plan already there at that path and sent it back with these comments:\n\n{comments}"
                );
                let extra = [
                    ("fixes", fixes),
                    ("fixes.path", round.input.display().to_string()),
                    ("fixes.what", round.what.clone()),
                    ("fix_plan.path", plan),
                ];
                if !self.launch_fresh_build(project, slug, &extra) {
                    return;
                }
                format!("Sent the fix plan of \u{201c}{title}\u{201d} back to {FRESH_BUILD_AGENT}")
            }
        };
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        f.reopen_fix_round();
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Planning;
            round.sent_back = back;
            round.asked = now;
            round.ready = None;
            round.stalled = None;
        }
        f.log(now, format!("Fix plan sent back ({back})"));
        self.save_feature(project, slug);
        (self.features.build.fix_watches).insert(fix_key(project, slug), FixWatch::default());
        self.doc_done(project, slug, DocFile::FixPlan);
        self.show_success(text);
    }

    /// The user approved the plan of feature `slug`, or asks for the fixes
    /// of its last round again: the agent compacts its chat if it can, as
    /// the plan is in its file, and then makes them. Whether it went; a
    /// refusal says why and changes nothing.
    pub(crate) fn make_fixes(&mut self, project: &str, slug: &str) -> bool {
        self.fixes_go(project, slug, false)
    }

    /// The same for a round with no plan to approve, the one gerdr wrote
    /// waiting (`start_unplanned_fix_round`).
    pub(crate) fn make_unplanned_fixes(&mut self, project: &str, slug: &str) -> bool {
        self.fixes_go(project, slug, true)
    }

    fn fixes_go(&mut self, project: &str, slug: &str, unplanned: bool) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let title = &feature.title;
        let again = feature.fix_round.is_none()
            && feature.status == Status::Open
            && feature.last_fix_round.is_some();
        let waiting = (feature.fix_round.as_ref()).is_some_and(FixRound::can_make_fixes);
        if !waiting && !again {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} has no fix plan waiting for review"
            ));
            return false;
        }
        if self.build_agent_busy(&feature) {
            self.show_warning(format!(
                "The Build agent of \u{201c}{title}\u{201d} is busy: try again once it rests"
            ));
            return false;
        }
        let data = store::dir(None, project, slug);
        let path = fix_round::path(&data);
        let plan = match std::fs::read_to_string(&path) {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                self.show_error(format!("Couldn't read {}: {err}", fix_round::FILE));
                return false;
            }
        };
        let ids = (feature.fix_round.as_ref())
            .or(feature.last_fix_round.as_ref())
            .map(FixRound::ids)
            .unwrap_or_default();
        let lacks = fix_round::missing(&plan, &ids);
        if !lacks.is_empty() {
            let lacks = lacks.join(", ");
            self.show_warning(format!("The fix plan needs {lacks}"));
            return false;
        }
        if let Some(line) = self.compact_line(&feature) {
            match self.tell_build_agent(&feature, line) {
                Ok(()) => {
                    let said = match (again, unplanned) {
                        (true, _) => {
                            "Fixes asked for again: the Build agent compacts its chat first"
                        }
                        (_, true) => "No plan needed: the Build agent compacts its chat first",
                        _ => "Fix plan approved: the Build agent compacts its chat first",
                    };
                    self.fixes_compact(project, slug, said);
                    self.show_success(format!(
                        "The Build agent of \u{201c}{title}\u{201d} compacts its chat, then makes the fixes"
                    ));
                    return true;
                }
                Err(why) => {
                    tracing::info!("compacting the Build agent of {project}/{slug}: {why}");
                }
            }
        }
        let said = match (again, unplanned) {
            (true, _) => "Fix plan's fixes sent to the Build agent again",
            (_, true) => "No plan needed: fixes sent to the Build agent",
            _ => "Fix plan approved: fixes sent to the Build agent",
        };
        self.send_fixes(project, slug, said)
    }

    /// The Build agent of feature `slug` was told to compact before the
    /// fixes: `said` goes in the log.
    fn fixes_compact(&mut self, project: &str, slug: &str, said: &str) {
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        f.reopen_fix_round();
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::CompactingToFix;
            round.stalled = None;
        }
        if let Some(run) = f.runs.get_mut(&Stage::Build) {
            run.stop = None;
        }
        f.log(now, said);
        self.save_feature(project, slug);
        (self.features.build.fix_watches).insert(fix_key(project, slug), FixWatch::default());
    }

    /// Fixes that couldn't go after the compact: the plan waits again.
    fn fix_plan_waits_again(&mut self, project: &str, slug: &str) {
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Ready;
            round.ready = Some(now);
        }
        f.log(now, "Fixes not sent: the fix plan waits again");
        self.save_feature(project, slug);
        (self.features.build.fix_watches).remove(&fix_key(project, slug));
    }

    /// Tell the Build agent of feature `slug`, or a fresh one, to make the
    /// fixes of its plan: `said` goes in the log. Whether it went; if not it
    /// said why and changed nothing.
    fn send_fixes(&mut self, project: &str, slug: &str, said: &str) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let title = &feature.title;
        let data = store::dir(None, project, slug);
        let path = fix_round::path(&data);
        let plan = match std::fs::read_to_string(&path) {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                self.show_error(format!("Couldn't read {}: {err}", fix_round::FILE));
                return false;
            }
        };
        let shown = path.display().to_string();
        // What it must tick goes in the build log first: the round isn't
        // over until it does, whatever the agent makes of the plan.
        let round = (feature.fix_round.as_ref()).or(feature.last_fix_round.as_ref());
        let Some(n) = round.map(|r| r.n) else {
            return false;
        };
        let ids = round.map(FixRound::ids).unwrap_or_default();
        let items = fix_round::items(&plan, &ids);
        if let Err(why) = write_items(&data, n, &items) {
            tracing::warn!("listing the fixes of {project}/{slug}: {why}");
            self.show_error(format!("Couldn't list the fixes in the build log: {why}"));
            return false;
        }
        let section = fix_round::heading(n);
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Build);
        values.insert("fix_plan.path".into(), shown.clone());
        values.insert("fixes.section".into(), section.clone());
        let sent = prompts::render_builtin("fix-go", &values)
            .map_err(|e| e.to_string())
            .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        let mut to = "the Build agent";
        if let Err(why) = sent {
            tracing::info!("telling {project}/{slug}'s Build agent to make the fixes: {why}");
            let extra = [
                ("fixes", plan),
                ("fixes.path", shown),
                ("fixes.what", "fixes of the approved plan".to_owned()),
                ("fixes.section", section),
            ];
            if !self.launch_fresh_build(project, slug, &extra) {
                return false;
            }
            to = FRESH_BUILD_AGENT;
        }
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return false;
        };
        f.reopen_fix_round();
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Fixing;
            round.stalled = None;
            round.items = items;
            round.done = 0;
        }
        if let Some(run) = f.runs.get_mut(&Stage::Build) {
            run.stop = None;
        }
        f.log(now, said);
        let agent = f.runs.get(&Stage::Build).and_then(|r| r.agent_key());
        self.save_feature(project, slug);
        (self.features.build.fix_watches).remove(&fix_key(project, slug));
        if let Some(agent) = agent {
            self.watch_fixing(project, slug, agent);
        }
        // Build's page lists them from now on.
        self.probe_feature(project, slug);
        self.show_success(format!("Sent the fixes of \u{201c}{title}\u{201d} to {to}"));
        true
    }

    /// Ask before dropping the open round of feature `slug`.
    pub(crate) fn ask_drop_fix_plan(&mut self, project: String, slug: String) {
        let body = format!(
            "What it was to fix comes back where it was, and its agent isn't stopped.\n{} stays in its folder until the next round.",
            fix_round::FILE
        );
        self.ask(
            "Drop this fix plan?".to_owned(),
            body,
            "Drop plan",
            Then::DropFixPlan { project, slug },
        );
    }

    /// Drop the open round of feature `slug`, as the user asked.
    pub(crate) fn drop_fix_plan(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if feature.fix_round.is_none() {
            return;
        }
        self.end_fix_round(project, slug, true);
        self.show_success(format!("Dropped the fix plan of \u{201c}{title}\u{201d}"));
    }

    /// End the round of feature `slug`, giving back what it took if `back`.
    fn end_fix_round(&mut self, project: &str, slug: &str, back: bool) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(round) = f.fix_round.take() else {
            return;
        };
        let now = feature::now();
        let n = round.n;
        match back {
            true => {
                f.log(now, format!("Fix round {n} dropped"));
                if let Some(fix) = f.verify_fixes.iter_mut().find(|fix| fix.round == n) {
                    fix.dropped = true;
                }
            }
            false => {
                f.log(now, format!("Fix round {n} over"));
                f.last_fix_round = Some(round.clone());
            }
        }
        self.save_feature(project, slug);
        (self.features.build.fix_watches).remove(&fix_key(project, slug));
        self.doc_done(project, slug, DocFile::FixPlan);
        if back {
            self.drop_unticked(project, slug, n);
            self.give_back(project, slug, round.give_back);
        }
    }

    /// Take dropped round `n`'s fixes not made from the build log: Build's
    /// page would list them as still to do for good.
    fn drop_unticked(&mut self, project: &str, slug: &str, n: u32) {
        let path = build_log(&store::dir(None, project, slug));
        let log = match std::fs::read_to_string(&path) {
            Ok(log) => log,
            // No log, no list: a round dropped before its fixes went.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                self.show_error(format!("Couldn't read {}: {err}", path.display()));
                return;
            }
        };
        let tidy = fix_round::without_unticked(&log, n);
        if tidy != log
            && let Err(err) = std::fs::write(&path, tidy)
        {
            tracing::warn!("writing {}: {err}", path.display());
            self.show_error(format!("Couldn't tidy {}: {err}", path.display()));
        }
    }

    /// Put back what a dropped round took.
    fn give_back(&mut self, project: &str, slug: &str, back: GiveBack) {
        match back {
            GiveBack::Nothing => {}
            GiveBack::Findings { ids, comments } => {
                if let Some(review) = self
                    .feature_mut(project, slug)
                    .and_then(|f| f.review.as_mut())
                {
                    fix_round::findings_back(review, &ids);
                }
                self.save_feature(project, slug);
                if let Some(copy) = comments {
                    self.comments_back(project, slug, &copy);
                }
            }
            GiveBack::Comments { copy } => self.comments_back(project, slug, &copy),
            GiveBack::Manual { checks } => {
                if let Some(test) = self
                    .feature_mut(project, slug)
                    .and_then(|f| f.manual.as_mut())
                {
                    feature::manual::unmark_sent(test, &checks);
                }
                self.save_feature(project, slug);
            }
        }
        self.probe_feature(project, slug);
    }

    /// Put Code review's comments kept at `copy` back with the ones there:
    /// in its file, and in its review if it is open.
    fn comments_back(&mut self, project: &str, slug: &str, copy: &Path) {
        let to = store::dir(None, project, slug).join(crate::feature_diff::COMMENTS);
        let kept = review::load_file(copy);
        let mut now = review::load_file(&to);
        now.comments.extend(kept.comments.iter().cloned());
        match review::save_file(&to, &now) {
            Ok(()) => {
                if let Err(err) = std::fs::remove_file(copy) {
                    tracing::warn!("deleting {}: {err}", copy.display());
                }
                self.code_review_comments_back(project, slug, kept.comments, now.own());
            }
            Err(err) => {
                tracing::warn!("giving back {}: {err}", copy.display());
                self.show_error(format!(
                    "Couldn't put the comments back: they are in {}",
                    copy.display()
                ));
            }
        }
    }

    /// Plan fixes for feature `slug`'s failed Verify run.
    pub(crate) fn plan_verify_fixes(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let Some(failures) = feature.verify.as_ref().and_then(feature::verify::to_fix) else {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} has no failed checks to fix"
            ));
        };
        let run = feature.verify.clone().unwrap_or_default();
        let data = store::dir(None, project, slug);
        let evidence = data.join(feature::verify::EVIDENCE);
        let text = format!(
            "# Verify failures\n\nThe checks of \u{201c}{title}\u{201d} failed:\n\n{}\n\nThe checks and their output are in {}.\n",
            failures
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n"),
            evidence.display()
        );
        if self.send_to_build(project, slug, "Verify failures", text) {
            if let Some(f) = self.feature_mut(project, slug) {
                let agent =
                    (f.runs.get(&Stage::Build)).map_or_else(String::new, |r| r.agent.clone());
                let fix = VerifyFix::new(f.fix_rounds, &run, agent);
                f.verify_fixes.push(fix);
                self.save_feature(project, slug);
            }
            self.show_build_agent(project, slug);
        }
    }
}

/// Where feature data dir `data` has its build log.
fn build_log(data: &Path) -> PathBuf {
    data.join(feature::artifacts::name(Stage::Build).unwrap_or("build.md"))
}

/// Put round `n`'s `items` in the build log under `data`.
fn write_items(data: &Path, n: u32, items: &[String]) -> Result<(), String> {
    let path = build_log(data);
    let log = match std::fs::read_to_string(&path) {
        Ok(log) => log,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.to_string()),
    };
    let with = fix_round::with_items(&log, n, items);
    if with == log {
        return Ok(());
    }
    std::fs::write(&path, with).map_err(|err| err.to_string())
}

/// A feature's key among the watches.
fn fix_key(project: &str, slug: &str) -> (String, String) {
    (project.to_owned(), slug.to_owned())
}

/// Move the plan of round `n` out of the way of the next one, its comments
/// with it: nothing to do when it has none.
fn archive_plan(data: &Path, n: u32) -> Result<(), String> {
    let plan = fix_round::path(data);
    if plan.exists() {
        let to = fix_round::archive_path(data, n);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&plan, &to).map_err(|e| e.to_string())?;
    }
    match std::fs::remove_file(fix_round::comments_path(data)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_plan_makes_way_for_the_next_round() {
        let base = std::env::temp_dir().join(format!("gerdr-fix-plan-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let plan = fix_round::path(&base);
        std::fs::write(&plan, "## Fixes\n- a\n").unwrap();
        std::fs::write(fix_round::comments_path(&base), "{}").unwrap();
        archive_plan(&base, 3).unwrap();
        assert!(!plan.exists());
        assert!(!fix_round::comments_path(&base).exists());
        let kept = std::fs::read_to_string(fix_round::archive_path(&base, 3)).unwrap();
        assert_eq!(kept, "## Fixes\n- a\n");
        // With none there, nothing to do.
        archive_plan(&base, 4).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }
}
