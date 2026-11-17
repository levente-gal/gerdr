//! Research agents: the questions Discover's agent asks in `research.md`
//! that it can't settle from the code (how an outside API behaves, what a
//! library supports). gerdr starts one agent per new question, a few at a
//! time, each in a tab of the feature's workspace with the agent picked for
//! Discover. Each writes its answer to `research-R<n>.md`; once it is there
//! and the agent at rest, gerdr closes its tab, logs it and tells
//! Discover's agent. The brief cites an answer as `(R1)`.

use std::time::{Duration, Instant};

use crate::{
    Gerdr, agents,
    ai_work::SendKind,
    feature::{
        self, Feature, Stage, Status,
        agents::{ResearchRun, allowed_into, with_prompt},
        artifacts::{self, Ask},
        store,
    },
    feature_agents::{stamp, vars, write_prompt},
    herdr::AgentStatus,
    prompts,
};

/// How many research agents of a feature run at once: the rest wait.
pub const AT_ONCE: usize = 3;

/// How long a research agent's tab must stay done, or gone, before gerdr
/// takes it so: herdr may only be catching up.
const SETTLE: Duration = Duration::from_secs(5);

/// The name a research agent's tab goes by: "Claude Code · Research R1".
pub(crate) fn tab_label(agent: &str, id: u32) -> String {
    format!("{agent} \u{b7} Research R{id}")
}

/// The research runs `asks` adds to `feature`: those of new numbers, in
/// order, waiting for their turn.
pub(crate) fn new_runs(feature: &Feature, asks: &[Ask]) -> Vec<ResearchRun> {
    (asks.iter())
        .filter(|ask| !feature.research.iter().any(|r| r.id == ask.id))
        .map(|ask| ResearchRun {
            id: ask.id,
            question: ask.question.clone(),
            why: ask.why.clone(),
            ..ResearchRun::default()
        })
        .collect()
}

/// The ids of `feature`'s research that may start now: waiting ones, as
/// many as `AT_ONCE` leaves room for, oldest first.
pub(crate) fn may_start(feature: &Feature) -> Vec<u32> {
    let running = feature.research.iter().filter(|r| r.running()).count();
    (feature.research.iter())
        .filter(|r| r.waits())
        .take(AT_ONCE.saturating_sub(running))
        .map(|r| r.id)
        .collect()
}

/// What a running research agent comes to, as last seen: `answered` its
/// answer written since it started, `alive` its tab there, `status` what
/// herdr says it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// Still at it, or about to be.
    Going,
    /// Its answer is there and it does nothing more.
    Answered,
    /// Its tab is gone without an answer.
    Gone,
}

/// What `answered`, `alive` and `status` say of a running research agent.
pub(crate) fn seen(answered: bool, alive: bool, status: Option<AgentStatus>) -> Seen {
    match (answered, alive) {
        (true, false) => Seen::Answered,
        (true, true) if status != Some(AgentStatus::Working) => Seen::Answered,
        (false, false) => Seen::Gone,
        _ => Seen::Going,
    }
}

/// Why the brief can't be approved yet: research still to come in.
pub(crate) fn unanswered(feature: &Feature) -> Option<String> {
    let run = feature.research.iter().find(|r| r.running() || r.waits())?;
    Some(match run.running() {
        true => format!(
            "Research R{} is still running: wait for it, or close its tab",
            run.id
        ),
        false => format!("Research R{} waits to start: wait for it", run.id),
    })
}

impl Gerdr {
    /// Look at the research of every open feature of `project` at Discover,
    /// written here: start what was newly asked, as room allows, and take
    /// in what was answered or stopped.
    pub(crate) fn research_tick(&mut self, project: &str) {
        let Some(all) = self.features.all.get(project) else {
            return;
        };
        let slugs: Vec<String> = (all.iter())
            .filter(|f| f.status == Status::Open && f.current() == Some(Stage::Discover))
            // Its agent on another machine writes its questions there.
            .filter(|f| (f.runs.get(&Stage::Discover)).is_some_and(|r| r.machine.is_none()))
            .map(|f| f.slug.clone())
            .collect();
        for slug in slugs {
            self.research_asked(project, &slug);
            self.research_watch(project, &slug);
            let starts = self
                .feature(project, &slug)
                .map(may_start)
                .unwrap_or_default();
            for id in starts {
                self.start_research(project, &slug, id);
            }
        }
    }

    /// Read feature `slug`'s `research.md` if it changed since last read,
    /// and keep the questions it newly asks.
    fn research_asked(&mut self, project: &str, slug: &str) {
        let data = store::dir(None, project, slug);
        let path = data.join(artifacts::RESEARCH);
        let key = (project.to_owned(), slug.to_owned());
        let Some(now) = stamp(&path) else {
            return;
        };
        // Read once per change, so one that can't be read is said once.
        if self.features.research_read.insert(key, now) == Some(now) {
            return;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                return;
            }
        };
        let asks = artifacts::research_asks(&text);
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let new = new_runs(f, &asks);
        if new.is_empty() {
            return;
        }
        f.research.extend(new);
        self.save_feature(project, slug);
    }

    /// Take in what feature `slug`'s running research agents came to.
    fn research_watch(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let data = store::dir(None, project, slug);
        // One with no tab left to watch (taken over from another machine)
        // stops: Run again starts it here.
        let lost: Vec<u32> = (f.research.iter())
            .filter(|r| r.running() && r.pane.is_none())
            .map(|r| r.id)
            .collect();
        let running: Vec<(u32, String, u64)> = (f.research.iter())
            .filter(|r| r.running())
            .filter_map(|r| Some((r.id, r.pane.clone()?, r.started?)))
            .collect();
        for id in lost {
            self.research_stopped(project, slug, id, "its tab is gone".into());
        }
        let now = Instant::now();
        for (id, pane, started) in running {
            if self.pane_out_of_reach(&pane) || self.pane_paused(&pane) {
                continue;
            }
            let (status, alive) = self.agent_key_state(&agents::AgentKey::Herdr(pane.clone()));
            let path = artifacts::research_path(&data, id);
            let answered = stamp(&path).is_some_and(|(at, _)| at >= started)
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| artifacts::check_research(&text).is_ok());
            let came = seen(answered, alive, status);
            if came == Seen::Going {
                self.features.research_seen.remove(&pane);
                continue;
            }
            let since = *self
                .features
                .research_seen
                .entry(pane.clone())
                .or_insert(now);
            if now.duration_since(since) < SETTLE {
                continue;
            }
            self.features.research_seen.remove(&pane);
            match came {
                Seen::Answered => self.research_answered(project, slug, id, alive.then_some(pane)),
                _ => self.research_stopped(project, slug, id, "its tab closed".into()),
            }
        }
    }

    /// Research `id` of feature `slug` is answered: close its tab (`pane`,
    /// if still there), log it, and tell Discover's agent.
    fn research_answered(&mut self, project: &str, slug: &str, id: u32, pane: Option<String>) {
        if let Some(pane) = &pane {
            self.close_herdr_pane(pane);
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(run) = f.research.iter_mut().find(|r| r.id == id) else {
            return;
        };
        let now = feature::now();
        run.done = Some(now);
        run.pane = None;
        let what = format!("Research R{id} answered: {}", run.question);
        f.log(now, what);
        self.save_feature(project, slug);
        let path = artifacts::research_path(&store::dir(None, project, slug), id);
        let line = format!(
            "Research R{id} is answered in {}. Read it, and cite (R{id}) on the brief's lines it settles.",
            path.display()
        );
        // Not there, it is told by its prompt when it starts again (`vars`).
        if let Err(why) =
            self.tell_stage_agent(project, slug, Stage::Discover, SendKind::Research, &line)
        {
            tracing::info!("not telling Discover's agent of R{id}: {why}");
        }
    }

    /// Research `id` of feature `slug` stopped without an answer: `why`.
    fn research_stopped(&mut self, project: &str, slug: &str, id: u32, why: String) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        let Some(run) = f.research.iter_mut().find(|r| r.id == id) else {
            return;
        };
        run.pane = None;
        run.started.get_or_insert(now);
        run.stopped = Some(why.clone());
        f.log(now, format!("Research R{id} stopped: {why}"));
        self.save_feature(project, slug);
    }

    /// Start the agent of research `id` of feature `slug`, in a new tab of
    /// its workspace behind what shows. One that can't start says why on
    /// its card.
    fn start_research(&mut self, project: &str, slug: &str, id: u32) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(run) = feature.research.iter().find(|r| r.id == id) else {
            return;
        };
        let (agent, base) = match self.discover_agent(&feature) {
            Ok(pick) => pick,
            Err(why) => return self.research_stopped(project, slug, id, why),
        };
        let ask = Ask {
            id,
            question: run.question.clone(),
            why: run.why.clone(),
        };
        let command = match self.research_command(&feature, project, slug, &ask, &base) {
            Ok(command) => command,
            Err(why) => {
                tracing::warn!("{why}");
                self.show_error(why.clone());
                return self.research_stopped(project, slug, id, why);
            }
        };
        let opened = self.open_feature_tab_behind(project, slug, &feature.dir, command);
        let Some(pane) = opened else {
            let why = format!("Couldn't open a tab for {agent}");
            self.show_error(why.clone());
            return self.research_stopped(project, slug, id, why);
        };
        if let Some((w, t)) = self.herdr_pane_place(&pane) {
            self.rename_tab(w, t, tab_label(&agent, id));
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(run) = f.research.iter_mut().find(|r| r.id == id) else {
            return;
        };
        let now = feature::now();
        run.agent.clone_from(&agent);
        run.pane = Some(pane);
        run.started = Some(now);
        f.log(now, format!("Research R{id} started: {agent}"));
        self.save_feature(project, slug);
    }

    /// The command that starts agent CLI `base` on research question `ask`
    /// of `feature`: its prompt written to the feature's data folder.
    fn research_command(
        &self,
        feature: &Feature,
        project: &str,
        slug: &str,
        ask: &Ask,
        base: &str,
    ) -> Result<String, String> {
        let data = store::dir(None, project, slug);
        let names = self.projects.names(feature);
        let mut values = vars(feature, &names, &data, Stage::Discover);
        values.insert("research.id".into(), ask.id.to_string());
        values.insert("research.question".into(), ask.question.clone());
        values.insert("research.why".into(), ask.why.clone().unwrap_or_default());
        let answer = artifacts::research_path(&data, ask.id);
        values.insert("artifact.path".into(), answer.display().to_string());
        let text = prompts::render_builtin("research", &values)
            .map_err(|err| format!("Couldn't write the research prompt: {err}"))?;
        let prompt = write_prompt(&data, &format!("research-R{}.md", ask.id), &text)?;
        let start = self
            .start_line(feature, project, slug, Stage::Discover, &prompt)
            .map_err(|err| format!("Couldn't write the research prompt: {err}"))?;
        let line = allowed_into(base, &feature.agent_dirs_for(base, &data));
        Ok(with_prompt(&line, &start))
    }

    /// Run research `id` of feature `slug` again, from its card: a fresh
    /// agent once there is room.
    pub(crate) fn rerun_research(&mut self, project: &str, slug: &str, id: u32) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(run) = f.research.iter_mut().find(|r| r.id == id) else {
            return;
        };
        if run.running() {
            return self.show_warning(format!("Research R{id} is running already"));
        }
        *run = ResearchRun {
            id,
            question: run.question.clone(),
            why: run.why.clone(),
            ..ResearchRun::default()
        };
        self.save_feature(project, slug);
        if let Some(f) = self.feature(project, slug)
            && may_start(f).contains(&id)
        {
            self.start_research(project, slug, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: u32, started: Option<u64>, done: Option<u64>) -> ResearchRun {
        ResearchRun {
            id,
            question: format!("Q{id}?"),
            started,
            done,
            ..ResearchRun::default()
        }
    }

    #[test]
    fn new_questions_wait_and_start_as_room_allows() {
        let mut f = Feature {
            research: vec![run(1, Some(5), None), run(2, Some(5), Some(9))],
            ..Feature::default()
        };
        let asks = artifacts::research_asks(
            "## R1: Q1?\n## R2: Q2?\n## R3: Q3?\n## R4: Q4?\n## R5: Q5?\n",
        );
        let new = new_runs(&f, &asks);
        assert_eq!(new.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 4, 5]);
        assert!(new.iter().all(ResearchRun::waits));
        f.research.extend(new);
        // R1 runs: two more may.
        assert_eq!(may_start(&f), [3, 4]);
        f.research[0].stopped = Some("its tab closed".into());
        assert_eq!(may_start(&f), [3, 4, 5]);
    }

    #[test]
    fn an_answer_counts_once_its_agent_is_at_rest_or_gone() {
        use AgentStatus::*;
        assert_eq!(seen(true, true, Some(Working)), Seen::Going);
        assert_eq!(seen(true, true, Some(Idle)), Seen::Answered);
        assert_eq!(seen(true, true, None), Seen::Answered);
        assert_eq!(seen(true, false, None), Seen::Answered);
        assert_eq!(seen(false, true, Some(Idle)), Seen::Going);
        assert_eq!(seen(false, false, None), Seen::Gone);
    }

    #[test]
    fn the_brief_waits_for_research_running_or_waiting() {
        let mut f = Feature::default();
        assert_eq!(unanswered(&f), None);
        f.research = vec![run(1, Some(5), Some(9)), run(2, None, None)];
        assert_eq!(
            unanswered(&f),
            Some("Research R2 waits to start: wait for it".into())
        );
        f.research[1].stopped = Some("its tab closed".into());
        assert_eq!(unanswered(&f), None);
        f.research.push(run(3, Some(7), None));
        assert!(unanswered(&f).is_some_and(|w| w.starts_with("Research R3 is still running")));
    }
}
