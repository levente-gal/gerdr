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
        fix_round::{self, Compact, FixPhase, FixRound, GiveBack, Plan},
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
}

impl Default for FixWatch {
    fn default() -> Self {
        FixWatch {
            worked: false,
            since: Instant::now(),
            refused: None,
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
        let data = store::dir(None, project, slug);
        if let Err(why) = archive_plan(&data, feature.fix_rounds) {
            tracing::warn!("keeping the last fix plan of {project}/{slug}: {why}");
            self.show_error(format!("Couldn't keep the last fix plan: {why}"));
            return false;
        }
        let compact = self.compact_line(&feature);
        let n = feature.fix_rounds + 1;
        let now = feature::now();
        let phase = match compact {
            Some(_) => FixPhase::Compacting,
            None => FixPhase::Planning,
        };
        let Some(f) = self.feature_mut(project, slug) else {
            return false;
        };
        f.fix_rounds = n;
        f.fix_round = Some(FixRound {
            n,
            what: what.to_owned(),
            input,
            stage: feature.current().unwrap_or(Stage::Build),
            phase,
            sent_back: 0,
            asked: now,
            ready: None,
            stalled: None,
            give_back: GiveBack::Nothing,
        });
        f.log(now, format!("Fix round {n} started: {what}"));
        self.save_feature(project, slug);
        self.features
            .build
            .fix_watches
            .insert(fix_key(project, slug), FixWatch::default());
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
            FixPhase::Fixing => {
                if !self.fixing_watched(project, slug) {
                    match run.and_then(|r| r.agent_key()) {
                        // Seen first after a restart.
                        Some(agent) => self.watch_fixing_found(project, slug, agent),
                        None => self.end_fix_round(project, slug, false),
                    }
                }
            }
            FixPhase::Compacting => {
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
                    Compact::Go => self.ask_fix_plan(project, slug),
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
        let missing = match std::fs::read_to_string(path) {
            Ok(text) => fix_round::missing(&text),
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
        let Some(round) = feature.fix_round.clone().filter(|r| r.before_fixing()) else {
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

    /// The user approved the plan of feature `slug`: the agent makes the
    /// fixes. Whether it went; a refusal says why and changes nothing.
    pub(crate) fn make_fixes(&mut self, project: &str, slug: &str) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let title = &feature.title;
        let waiting = (feature.fix_round.as_ref()).is_some_and(|r| r.phase == FixPhase::Ready);
        if !waiting {
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
        let lacks = fix_round::missing(&plan);
        if !lacks.is_empty() {
            let lacks = lacks.join(", ");
            self.show_warning(format!("The fix plan needs {lacks}"));
            return false;
        }
        let shown = path.display().to_string();
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Build);
        values.insert("fix_plan.path".into(), shown.clone());
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
        if let Some(round) = f.fix_round.as_mut() {
            round.phase = FixPhase::Fixing;
        }
        f.log(now, "Fix plan approved: fixes sent to the Build agent");
        let agent = f.runs.get(&Stage::Build).and_then(|r| r.agent_key());
        self.save_feature(project, slug);
        if let Some(agent) = agent {
            self.watch_fixing(project, slug, agent);
        }
        self.show_success(format!("Sent the fixes of \u{201c}{title}\u{201d} to {to}"));
        true
    }

    /// Fixing is over on feature `slug`: its round, if it was making them,
    /// ends.
    pub(crate) fn fix_round_fixed(&mut self, project: &str, slug: &str) {
        let fixing = self
            .feature(project, slug)
            .and_then(|f| f.fix_round.as_ref())
            .is_some_and(|r| r.phase == FixPhase::Fixing);
        if fixing {
            self.end_fix_round(project, slug, false);
        }
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
            true => f.log(now, format!("Fix round {n} dropped")),
            false => f.log(now, format!("Fix round {n} over")),
        }
        self.save_feature(project, slug);
        (self.features.build.fix_watches).remove(&fix_key(project, slug));
        self.doc_done(project, slug, DocFile::FixPlan);
        if back {
            self.give_back(project, slug, round.give_back);
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
            self.show_build_agent(project, slug);
        }
    }
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
