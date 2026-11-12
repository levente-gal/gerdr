//! The one way lines reach a feature's stage agents: its prompt, comments
//! sent back, nudges, fix rounds. Each goes through `tell_stage_agent`, so
//! what was typed can be kept with the feature's AI work (see `ai_work`).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    Gerdr,
    agents::AgentKey,
    ai_work::{self, BlobRef, Event, Line, SendKind, Via},
    feature::{Feature, Stage, agents::StageRun, store},
    feature_build::{AGENT_QUIT, TAB_GONE},
    machine_work::split_pane,
};

/// A line typed into a stage's agent, as the AI work keeps it. It goes
/// along to where it can still fail later: another machine, T3 Code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Told {
    pub project: String,
    pub slug: String,
    pub stage: Stage,
    /// The run of the stage it went to.
    pub n: u32,
    pub kind: SendKind,
    /// The line as gerdr has it, its paths as they read here.
    pub text: String,
}

impl Gerdr {
    /// `told` didn't reach the agent `via`: `why` (see `Live::told_failed`).
    pub(crate) fn told_failed(&self, told: &Told, via: Via, why: &str) {
        self.ai_work.told_failed(told, via, why);
    }

    /// Type `line` into the agent of `stage` of feature `slug`, for `kind`.
    /// It goes as it does today: a nudge straight to where the agent is; a
    /// prompt to its tab while it waits for one; anything else only once
    /// the prompt was sent, and on Build to its T3 Code thread too.
    pub(crate) fn tell_stage_agent(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        kind: SendKind,
        line: &str,
    ) -> Result<(), String> {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return Err("the feature is gone".into());
        };
        self.tell_agent(&feature, stage, kind, line)
    }

    /// Send `line` to the Build agent of `feature`, for `kind`, if it is
    /// there.
    pub(crate) fn tell_build_agent(
        &mut self,
        feature: &Feature,
        kind: SendKind,
        line: &str,
    ) -> Result<(), String> {
        self.tell_agent(feature, Stage::Build, kind, line)
    }

    /// Type `line` into `stage`'s agent of `feature` as it is now, and keep
    /// it with the feature's AI work.
    fn tell_agent(
        &mut self,
        feature: &Feature,
        stage: Stage,
        kind: SendKind,
        line: &str,
    ) -> Result<(), String> {
        let run = feature.runs.get(&stage);
        let told = Told {
            project: feature.project.clone(),
            slug: feature.slug.clone(),
            stage,
            n: run.map_or(0, |r| r.n),
            kind,
            text: line.to_owned(),
        };
        let sent = self.send_told(run, &told);
        self.ai_work.keep_told(&told, via(run, stage, kind), &sent);
        sent
    }

    /// Type `told` into the agent of `run`.
    fn send_told(&mut self, run: Option<&StageRun>, told: &Told) -> Result<(), String> {
        let (stage, kind) = (told.stage, told.kind);
        if kind == SendKind::Nudge {
            return match run.and_then(StageRun::agent_key) {
                Some(AgentKey::T3(id)) => self.t3_send(&id, told),
                Some(AgentKey::Herdr(pane)) if split_pane(&pane).is_some() => {
                    self.tell_machine_pane(&pane, told)
                }
                Some(AgentKey::Herdr(pane)) => match self.session.herdr.as_mut() {
                    Some(herdr) => herdr.send_agent_line(&pane, &told.text),
                    None => Err("herdr isn't connected".into()),
                },
                _ => Err(TAB_GONE.into()),
            };
        }
        if kind != SendKind::Start && run.is_some_and(|r| r.held) {
            return Err("its prompt wasn't sent yet".into());
        }
        if stage == Stage::Build
            && kind != SendKind::Start
            && let Some(run) = run.filter(|r| r.t3.is_some())
        {
            let id = run
                .t3_thread()
                .ok_or_else(|| String::from("its T3 Code draft wasn't sent yet"))?;
            if !self.t3_thread_state(id).1 {
                return Err("its T3 Code thread is gone".into());
            }
            return self.t3_send(id, told);
        }
        let pane = run
            .and_then(|r| r.pane.clone())
            .filter(|p| self.pane_there(p))
            .ok_or_else(|| String::from(TAB_GONE))?;
        self.tell_agent_pane(&pane, told)
    }

    /// Type `told` into `pane`, where a stage's agent ran, if something
    /// still runs there: a pane back at its shell's prompt would run the
    /// line as a command.
    fn tell_agent_pane(&mut self, pane: &str, told: &Told) -> Result<(), String> {
        if split_pane(pane).is_some() {
            return self.tell_machine_pane(pane, told);
        }
        let Some(herdr) = self.session.herdr.as_mut() else {
            return Err("herdr isn't connected".into());
        };
        if crate::prepare::step(&herdr.process_info(pane)) != crate::prepare::Step::Busy {
            return Err(AGENT_QUIT.into());
        }
        herdr.send_agent_line(pane, &told.text)
    }
}

impl ai_work::Live {
    /// Keep `told`, gone `via`, with its feature's AI work: what it was and
    /// the files it names, or why it couldn't go.
    fn keep_told(&self, told: &Told, via: Via, sent: &Result<(), String>) {
        let Some(dir) = self.dir(&told.project, &told.slug) else {
            return;
        };
        let event = match sent {
            Ok(()) => {
                let data = store::dir(None, &told.project, &told.slug);
                let files = keep_files(&dir, &files_named(&told.text, &data));
                sent_event(told, via, files)
            }
            Err(why) => failed_event(told, via, why, false),
        };
        self.keep_told_line(told, &dir, event);
    }

    /// `told`, which went at once, didn't reach the agent `via`: `why`.
    pub(crate) fn told_failed(&self, told: &Told, via: Via, why: &str) {
        if let Some(dir) = self.dir(&told.project, &told.slug) {
            self.keep_told_line(told, &dir, failed_event(told, via, why, true));
        }
    }

    /// Append `event`, of `told`, to the AI work in feature folder `dir`,
    /// for the stats soon. One that can't be kept is said as the feature's
    /// problem, as the rest of its AI work says it.
    fn keep_told_line(&self, told: &Told, dir: &Path, event: Event) {
        let at = format!("{}/{}", told.project, told.slug);
        match ai_work::write::append(dir, &Line::now(event)) {
            Ok(()) => {
                crate::problems::clear_at("ai-work", &at);
                self.send(ai_work::thread::Job::Index {
                    project: told.project.clone(),
                    slug: told.slug.clone(),
                });
            }
            Err(err) => crate::problems::report_at(
                "ai-work",
                &at,
                format!(
                    "Couldn't keep a line typed into {}'s agent: {err}",
                    told.stage.label()
                ),
            ),
        }
    }
}

/// How `kind` reaches the agent of `run` of `stage` (see `send_told`).
fn via(run: Option<&StageRun>, stage: Stage, kind: SendKind) -> Via {
    let t3 = match kind {
        SendKind::Nudge => run.and_then(StageRun::t3_thread).is_some(),
        SendKind::Start => false,
        _ => stage == Stage::Build && run.is_some_and(|r| r.t3.is_some()),
    };
    match run.and_then(|r| r.pane.as_deref()) {
        _ if t3 => Via::T3,
        Some(pane) if split_pane(pane).is_some() => Via::Machine,
        _ => Via::Pane,
    }
}

/// `told` went `via`, naming `files`.
fn sent_event(told: &Told, via: Via, files: BTreeMap<String, BlobRef>) -> Event {
    Event::Sent {
        stage: told.stage,
        n: told.n,
        send: told.kind,
        text: told.text.clone(),
        files,
        via,
    }
}

/// `told` couldn't go `via`: `why`; `went`: it was taken at once first.
fn failed_event(told: &Told, via: Via, why: &str, went: bool) -> Event {
    Event::SendFailed {
        stage: told.stage,
        n: told.n,
        send: told.kind,
        text: told.text.clone(),
        via,
        why: why.to_owned(),
        went,
    }
}

/// The paths under data folder `data` that `text` names, each once, in
/// the order named. A path ends where the text around it starts: a space,
/// a quote, a closing bracket, or a full stop or colon at its end.
fn files_named(text: &str, data: &Path) -> Vec<PathBuf> {
    let Some(data) = data.to_str().filter(|d| !d.is_empty()) else {
        return Vec::new();
    };
    let ends = |c: char| c.is_whitespace() || "\"'`)]>,;".contains(c);
    let mut found: Vec<PathBuf> = Vec::new();
    for (at, _) in text.match_indices(data) {
        let rest = &text[at..];
        let path = rest[..rest.find(ends).unwrap_or(rest.len())].trim_end_matches(['.', ':']);
        let below = path.len() > data.len() + 1 && path[data.len()..].starts_with('/');
        let path = PathBuf::from(path);
        if below && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// The files of `paths` kept as blobs in feature folder `dir`, by their
/// path. What isn't a file is left out.
fn keep_files(dir: &Path, paths: &[PathBuf]) -> BTreeMap<String, BlobRef> {
    let mut kept = BTreeMap::new();
    for path in paths.iter().filter(|p| p.is_file()) {
        let blob = std::fs::read(path).and_then(|bytes| ai_work::blobs::put(dir, &bytes));
        match blob {
            Ok(blob) => {
                kept.insert(path.display().to_string(), blob);
            }
            Err(err) => tracing::warn!("keeping {} with the AI work: {err}", path.display()),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::agents::T3Run;

    fn told(kind: SendKind) -> Told {
        Told {
            project: "p".into(),
            slug: "s".into(),
            stage: Stage::Build,
            n: 3,
            kind,
            text: format!("{kind:?} line"),
        }
    }

    #[test]
    fn every_kind_is_kept_as_sent_with_its_files() {
        use SendKind::*;
        let kinds = [
            Start,
            SendBack,
            PlanReviewBack,
            Nudge,
            Compact,
            FixPlan,
            FixOn,
            FixGo,
            BuildCheck,
            Continue,
        ];
        for kind in kinds {
            let files = BTreeMap::from([("/d/a.md".to_owned(), BlobRef::of(b"a"))]);
            let event = sent_event(&told(kind), Via::Machine, files.clone());
            assert_eq!(
                event,
                Event::Sent {
                    stage: Stage::Build,
                    n: 3,
                    send: kind,
                    text: format!("{kind:?} line"),
                    files,
                    via: Via::Machine,
                },
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_failure_now_or_later_is_kept_with_why() {
        let event = failed_event(
            &told(SendKind::FixGo),
            Via::T3,
            "T3 Code's server isn't running",
            true,
        );
        assert_eq!(
            event,
            Event::SendFailed {
                stage: Stage::Build,
                n: 3,
                send: SendKind::FixGo,
                text: "FixGo line".into(),
                via: Via::T3,
                why: "T3 Code's server isn't running".into(),
                went: true,
            }
        );
        let line = serde_json::to_string(&Line::now(event)).unwrap();
        assert!(line.contains(r#""kind":"send_failed""#), "{line}");
        assert!(line.contains(r#""send":"fix_go""#), "{line}");
    }

    #[test]
    fn files_named_are_those_under_the_data_folder() {
        let data = Path::new("/data/features/p/s");
        let text = "Read /data/features/p/s/prompts/build-1.md, then `/data/features/p/s/fix-plan.md`. \
                    Not /data/features/p/sx/a.md nor /data/features/p/s nor /elsewhere/b.md; \
                    again /data/features/p/s/prompts/build-1.md:";
        assert_eq!(
            files_named(text, data),
            [
                PathBuf::from("/data/features/p/s/prompts/build-1.md"),
                PathBuf::from("/data/features/p/s/fix-plan.md"),
            ]
        );
        assert!(files_named("no paths here", data).is_empty());
    }

    #[test]
    fn only_files_are_kept() {
        let dir = std::env::temp_dir().join(format!("gerdr-told-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.md");
        std::fs::write(&file, "abc").unwrap();
        let kept = keep_files(&dir, &[file.clone(), dir.join("gone.md"), dir.clone()]);
        assert_eq!(
            kept,
            BTreeMap::from([(file.display().to_string(), BlobRef::of(b"abc"))])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_goes_where_its_agent_is() {
        let pane = |p: &str| StageRun {
            pane: Some(p.into()),
            ..StageRun::default()
        };
        let thread = StageRun {
            t3: Some(T3Run::Thread {
                id: "t".into(),
                agent: None,
            }),
            ..StageRun::default()
        };
        let draft = StageRun {
            t3: Some(T3Run::Draft),
            ..StageRun::default()
        };
        use SendKind::*;
        assert_eq!(via(Some(&pane("p1")), Stage::Plan, SendBack), Via::Pane);
        assert_eq!(via(Some(&pane("m/p1")), Stage::Plan, Start), Via::Machine);
        assert_eq!(via(Some(&pane("m/p1")), Stage::Build, Nudge), Via::Machine);
        assert_eq!(via(Some(&thread), Stage::Build, FixGo), Via::T3);
        assert_eq!(via(Some(&draft), Stage::Build, FixGo), Via::T3);
        assert_eq!(via(Some(&thread), Stage::AiReview, Nudge), Via::T3);
        assert_eq!(via(Some(&draft), Stage::Build, Nudge), Via::Pane);
        assert_eq!(via(Some(&thread), Stage::AiReview, SendBack), Via::Pane);
        assert_eq!(via(None, Stage::Build, Continue), Via::Pane);
    }
}
