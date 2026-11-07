//! What the stats panel shows, read from the index: the features, the
//! agents by stage and model, the checks, and one feature in detail.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    AgentKind,
    index::{stage_name, stage_of},
};
use crate::feature::Stage;

/// The panel's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    #[default]
    Features,
    Agents,
    Checks,
}

/// What is asked of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub tab: Tab,
    /// The project, `None` for all of them.
    pub project: Option<String>,
    /// The feature (project, slug) drilled into, on the Features tab.
    pub detail: Option<(String, String)>,
}

/// What the index holds for a `Scope`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub scope: Scope,
    pub features: Vec<FeatureRow>,
    pub agents: Vec<AgentRow>,
    pub checks: Vec<CheckRow>,
    pub detail: Option<Detail>,
    /// AI work is kept, but the index isn't drawn from it yet.
    pub building: bool,
}

/// A feature, on the Features tab. Times are in seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureRow {
    pub project: String,
    pub slug: String,
    pub title: String,
    pub status: String,
    /// When it was made, since the epoch.
    pub created: u64,
    /// How long its agents worked.
    pub ai_secs: u64,
    pub runs: u32,
    pub fix_rounds: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub nudges: u32,
    pub send_backs: u32,
}

/// A stage's agent and model, on the Agents tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentRow {
    pub stage: Option<Stage>,
    pub agent: String,
    pub model: String,
    pub runs: u32,
    pub median_secs: Option<u64>,
    pub tokens_per_run: Option<u64>,
    /// Of its runs, those nudged.
    pub nudge_rate: f32,
    /// Of its runs, those sent back.
    pub send_back_rate: f32,
    /// The AI review's findings on its work, by severity.
    pub findings: BTreeMap<String, u32>,
    pub fixes: u32,
    /// Of its Build runs, those Verify passed after.
    pub verify_after_build: Option<f32>,
}

/// A repository's check command, on the Checks tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckRow {
    pub repo: String,
    pub command: String,
    pub runs: u32,
    pub failure_rate: f32,
    pub median_secs: Option<u64>,
    /// When it last failed, since the epoch.
    pub last_failure: Option<u64>,
    /// What it printed then.
    pub last_log: Option<PathBuf>,
}

/// One feature in detail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    pub project: String,
    pub slug: String,
    pub title: String,
    pub stages: Vec<StageSpan>,
    pub runs: Vec<RunRow>,
    pub fix_rounds: Vec<FixRow>,
    pub verify_runs: Vec<VerifyRow>,
}

/// When a stage began and ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageSpan {
    pub stage: Stage,
    pub started: Option<u64>,
    pub ended: Option<u64>,
}

/// A run of a stage's agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub stage: Stage,
    pub n: u32,
    pub agent: AgentKind,
    pub model: String,
    pub started: u64,
    pub ended: Option<u64>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub nudges: u32,
    pub send_backs: u32,
    /// Its chat's kept transcript, if any.
    pub transcript: Option<PathBuf>,
}

/// A fix round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixRow {
    pub n: u32,
    pub opened: Option<u64>,
    pub closed: Option<u64>,
    pub fixes: u32,
}

/// A Verify run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyRow {
    /// `v{n}@{started}`.
    pub run: String,
    pub started: Option<u64>,
    pub ended: Option<u64>,
    pub passed: u32,
    pub failed: u32,
}

/// What the index holds for `scope`: the rows of its tab, and the feature
/// drilled into.
pub fn query(conn: &Connection, scope: &Scope) -> Result<View, String> {
    let project = scope.project.as_deref();
    let read = || -> rusqlite::Result<View> {
        let mut view = View {
            scope: scope.clone(),
            ..View::default()
        };
        match scope.tab {
            Tab::Features => view.features = features(conn, project)?,
            Tab::Agents => view.agents = agents(conn, project)?,
            Tab::Checks => view.checks = checks(conn, project)?,
        }
        if let Some((project, slug)) = &scope.detail {
            view.detail = detail(conn, project, slug)?;
        }
        Ok(view)
    };
    read().map_err(|err| format!("reading the AI work index: {err}"))
}

/// What rows are asked for: those of a project, or of one feature, or all.
/// `?1` and `?2` in a query's `WHERE`.
const OF: &str = "(?1 IS NULL OR project = ?1) AND (?2 IS NULL OR slug = ?2)";

/// Milliseconds since the epoch, in seconds.
fn secs(ms: i64) -> u64 {
    (ms.max(0) / 1000) as u64
}

/// The middle of `values`, the mean of the two middle ones for an even
/// count.
fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    let mid = values.len() / 2;
    match values.len() {
        0 => None,
        n if n % 2 == 1 => Some(values[mid]),
        _ => Some((values[mid - 1] + values[mid]) / 2),
    }
}

/// What tells a run's stage and number within its feature.
type RunKey = (String, String, String, u32);

/// A run, with what was told of it afterwards.
#[derive(Clone, Debug, Default)]
struct Run {
    project: String,
    slug: String,
    stage: String,
    n: u32,
    started: i64,
    agent: String,
    model: String,
    ready: Option<i64>,
    ended: Option<i64>,
    held: bool,
    nudged: bool,
    sent_back: u32,
    /// What changed and what was typed into it: when, and whether its
    /// agent went to work then (`Some(true)`) or stopped (`Some(false)`).
    marks: Vec<(i64, Option<bool>)>,
    nudge_marks: u32,
    nudge_sends: u32,
    back_marks: u32,
    back_sends: u32,
    tokens_in: u64,
    tokens_out: u64,
    has_usage: bool,
    /// The model each of its main chat's models wrote most with.
    usage_models: BTreeMap<String, u64>,
    /// The chats whose usage went to it.
    chats: Vec<(String, String)>,
    transcript: Option<PathBuf>,
    kind: Option<AgentKind>,
    /// When its chat last said anything, in ms.
    last_message: Option<i64>,
}

impl Run {
    /// When it ended, as far as can be told: told so, else ready, else
    /// stopped or blocked, else its chat's last message. `None` for one
    /// nothing tells the end of.
    fn end(&self) -> Option<i64> {
        let rested = (self.marks.iter())
            .filter(|(_, work)| *work == Some(false))
            .map(|(at, _)| *at)
            .max();
        self.ended.or(self.ready).or(rested).or(self.last_message)
    }

    /// Its agent, by one name whichever wrote it: the record's picker name
    /// (`Claude Code`) and a backfill's (`claude`) are the same agent.
    fn agent_key(&self) -> String {
        match (AgentKind::of(&self.agent), self.kind) {
            (AgentKind::Unknown, Some(kind)) if self.agent.is_empty() => kind.name().to_owned(),
            (AgentKind::Unknown, _) => self.agent.clone(),
            (kind, _) => kind.name().to_owned(),
        }
    }

    fn key(&self) -> RunKey {
        (
            self.project.clone(),
            self.slug.clone(),
            self.stage.clone(),
            self.n,
        )
    }

    fn nudges(&self) -> u32 {
        (self.nudge_marks.max(self.nudge_sends)).max(u32::from(self.nudged))
    }

    fn send_backs(&self) -> u32 {
        (self.back_marks.max(self.back_sends)).max(self.sent_back)
    }

    /// How long its agent worked, in ms: from its start, or from its
    /// prompt for one that waited for it, to each time it rested, and again
    /// from each time it was told more. One still at work counts to its end,
    /// or to the last that was heard of it.
    fn work_ms(&self) -> i64 {
        let mut marks = self.marks.clone();
        marks.extend(self.ready.map(|at| (at, Some(false))));
        marks.sort_by_key(|(at, _)| *at);
        let mut since = (!self.held).then_some(self.started);
        let mut total = 0;
        let mut last = self.started;
        for (at, work) in marks {
            last = last.max(at);
            match work {
                Some(true) => {
                    since.get_or_insert(at);
                }
                Some(false) => {
                    if let Some(from) = since.take() {
                        total += (at - from).max(0);
                    }
                }
                None => {}
            }
        }
        if let Some(from) = since {
            let heard = last.max(self.last_message.unwrap_or(last));
            total += (self.ended.unwrap_or(heard) - from).max(0);
        }
        total
    }

    fn model(&self) -> String {
        match self.model.is_empty() {
            false => self.model.clone(),
            true => (self.usage_models.iter())
                .max_by_key(|(model, out)| (**out, std::cmp::Reverse((*model).clone())))
                .map(|(model, _)| model.clone())
                .unwrap_or_default(),
        }
    }
}

/// Runs by their stage and number, each list by start.
struct Runs {
    all: Vec<Run>,
    by_key: HashMap<RunKey, Vec<usize>>,
}

impl Runs {
    /// The run of `key` going at `at`: the latest begun by then, else the
    /// first.
    fn at(&self, key: &RunKey, at: i64) -> Option<usize> {
        let list = self.by_key.get(key)?;
        (list.iter().rev())
            .find(|i| self.all[**i].started <= at)
            .or(list.first())
            .copied()
    }

    /// The run of `project`/`slug`'s `stage` going at `at`, whatever its
    /// number: the latest begun by then, else the first.
    fn of_stage_at(
        &self,
        project: &str,
        slug: &str,
        stage: &str,
        at: Option<i64>,
    ) -> Option<usize> {
        let mut of: Vec<usize> = (0..self.all.len())
            .filter(|i| {
                let r = &self.all[*i];
                r.project == project && r.slug == slug && r.stage == stage
            })
            .collect();
        of.sort_by_key(|i| self.all[*i].started);
        let before = at.and_then(|at| {
            (of.iter().rev())
                .find(|i| self.all[**i].started <= at)
                .copied()
        });
        before.or(of.first().copied())
    }
}

/// The runs of `project` (all with `None`), or of feature `slug` of it,
/// with what their changes, sends, usage and chats tell.
fn load_runs(
    conn: &Connection,
    project: Option<&str>,
    slug: Option<&str>,
) -> rusqlite::Result<Runs> {
    let mut all = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, stage, n, started, agent, model, ready, ended, held,
                nudged, sent_back
             FROM runs WHERE {OF} ORDER BY project, slug, stage, n, started"
        ))?;
        let rows = stmt.query_map(params![project, slug], |r| {
            Ok(Run {
                project: r.get(0)?,
                slug: r.get(1)?,
                stage: r.get(2)?,
                n: r.get(3)?,
                started: r.get(4)?,
                agent: r.get(5)?,
                model: r.get(6)?,
                ready: r.get(7)?,
                ended: r.get(8)?,
                held: r.get(9)?,
                nudged: r.get(10)?,
                sent_back: r.get(11)?,
                ..Run::default()
            })
        })?;
        for row in rows {
            all.push(row?);
        }
    }
    let mut by_key: HashMap<RunKey, Vec<usize>> = HashMap::new();
    for (i, run) in all.iter().enumerate() {
        by_key.entry(run.key()).or_default().push(i);
    }
    let mut runs = Runs { all, by_key };

    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, stage, n, at, change FROM run_changes WHERE {OF}"
        ))?;
        let mut rows = stmt.query(params![project, slug])?;
        while let Some(r) = rows.next()? {
            let key: RunKey = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let at: i64 = r.get(4)?;
            let change: String = r.get(5)?;
            let Some(i) = runs.at(&key, at) else { continue };
            let run = &mut runs.all[i];
            if change == "ready" {
                run.ready = run.ready.max(Some(at));
            }
            let work = match change.as_str() {
                "prompt_sent" | "nudged" | "sent_back" | "unblocked" => Some(true),
                "ready" | "stop" | "blocked" | "pane_lost" => Some(false),
                _ => None,
            };
            run.marks.push((at, work));
            match change.as_str() {
                "nudged" => run.nudge_marks += 1,
                "sent_back" => run.back_marks += 1,
                _ => {}
            }
        }
    }
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, stage, n, at, kind FROM sends
             WHERE {OF} AND failed IS NULL"
        ))?;
        let mut rows = stmt.query(params![project, slug])?;
        while let Some(r) = rows.next()? {
            let key: RunKey = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let at: i64 = r.get(4)?;
            let kind: String = r.get(5)?;
            let Some(i) = runs.at(&key, at) else { continue };
            let run = &mut runs.all[i];
            run.marks.push((at, Some(true)));
            match kind.as_str() {
                "nudge" => run.nudge_sends += 1,
                "send_back" | "plan_review_back" => run.back_sends += 1,
                _ => {}
            }
        }
    }
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, stage, n, started, agent, id, model, sidechain,
                sum(input + cache_create + cache_read), sum(output), max(last_at)
             FROM usage WHERE {OF}
             GROUP BY project, slug, stage, n, started, agent, id, model, sidechain"
        ))?;
        let mut rows = stmt.query(params![project, slug])?;
        while let Some(r) = rows.next()? {
            let key: RunKey = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let started: i64 = r.get(4)?;
            let found = (runs.by_key.get(&key).into_iter().flatten())
                .find(|i| runs.all[**i].started == started)
                .copied();
            let Some(i) = found else { continue };
            let run = &mut runs.all[i];
            let chat = (r.get::<_, String>(5)?, r.get::<_, String>(6)?);
            let model: String = r.get(7)?;
            let sidechain: bool = r.get(8)?;
            let (input, output): (i64, i64) = (r.get(9)?, r.get(10)?);
            run.tokens_in += input.max(0) as u64;
            run.tokens_out += output.max(0) as u64;
            run.has_usage = true;
            let last: i64 = r.get(11)?;
            if last > 0 {
                run.last_message = run.last_message.max(Some(last));
            }
            if !sidechain {
                *run.usage_models.entry(model).or_default() += output.max(0) as u64;
            }
            if !run.chats.contains(&chat) {
                run.chats.push(chat);
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT s.project, s.slug, sr.stage, sr.n, s.agent, s.id, s.transcript
             FROM sessions s JOIN session_runs sr ON sr.project = s.project
                AND sr.slug = s.slug AND sr.agent = s.agent AND sr.id = s.id
             WHERE (?1 IS NULL OR s.project = ?1) AND (?2 IS NULL OR s.slug = ?2)
             ORDER BY s.id",
        )?;
        let mut rows = stmt.query(params![project, slug])?;
        let mut chats = Vec::new();
        while let Some(r) = rows.next()? {
            let key: RunKey = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let chat: (String, String) = (r.get(4)?, r.get(5)?);
            let transcript: Option<String> = r.get(6)?;
            chats.push((key, chat, transcript));
        }
        // A chat whose usage went to the run first, then any of its number.
        for whose in [true, false] {
            for (key, chat, transcript) in &chats {
                for i in runs.by_key.get(key).into_iter().flatten() {
                    let run = &mut runs.all[*i];
                    let fits = match whose {
                        true => run.chats.contains(chat),
                        false => run.chats.is_empty(),
                    };
                    if run.kind.is_none() && fits {
                        run.kind = serde_json::from_value(chat.0.clone().into()).ok();
                        run.transcript = transcript.as_ref().map(PathBuf::from);
                    }
                }
            }
        }
    }
    Ok(runs)
}

/// The Features tab: every feature of `project`, the newest first.
fn features(conn: &Connection, project: Option<&str>) -> rusqlite::Result<Vec<FeatureRow>> {
    let runs = load_runs(conn, project, None)?;
    let mut rounds: HashMap<(String, String), u32> = HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, count(*) FROM fix_rounds WHERE {OF} GROUP BY project, slug"
        ))?;
        let mut rows = stmt.query(params![project, None::<&str>])?;
        while let Some(r) = rows.next()? {
            rounds.insert((r.get(0)?, r.get(1)?), r.get(2)?);
        }
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT project, slug, title, status, created FROM features WHERE {OF}
         ORDER BY coalesce(created, 0) DESC, project, slug"
    ))?;
    let mut rows = stmt.query(params![project, None::<&str>])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let (project, slug): (String, String) = (r.get(0)?, r.get(1)?);
        let title: String = r.get(2)?;
        let mut row = FeatureRow {
            title: if title.is_empty() {
                slug.clone()
            } else {
                title
            },
            status: r.get(3)?,
            created: secs(r.get::<_, Option<i64>>(4)?.unwrap_or(0)),
            fix_rounds: rounds
                .get(&(project.clone(), slug.clone()))
                .copied()
                .unwrap_or(0),
            ..FeatureRow::default()
        };
        let mut work = 0;
        for run in (runs.all.iter()).filter(|r| r.project == project && r.slug == slug) {
            row.runs += 1;
            work += run.work_ms();
            row.tokens_in += run.tokens_in;
            row.tokens_out += run.tokens_out;
            row.nudges += run.nudges();
            row.send_backs += run.send_backs();
        }
        row.ai_secs = secs(work);
        row.project = project;
        row.slug = slug;
        out.push(row);
    }
    Ok(out)
}

/// What one Agents row is summed from.
#[derive(Default)]
struct AgentSum {
    runs: u32,
    spans: Vec<u64>,
    tokens: u64,
    with_usage: u32,
    nudged: u32,
    sent_back: u32,
    findings: BTreeMap<String, u32>,
    fixes: u32,
    verified: u32,
    verify_passed: u32,
}

/// The Agents tab: each stage's agent and model, in stage order.
fn agents(conn: &Connection, project: Option<&str>) -> rusqlite::Result<Vec<AgentRow>> {
    let runs = load_runs(conn, project, None)?;
    // Each run's own sums, then summed by stage, agent and model.
    let mut per_run: Vec<AgentSum> = (runs.all.iter())
        .map(|run| AgentSum {
            runs: 1,
            // One nothing tells the end of says nothing of how long.
            spans: run.end().map(|_| secs(run.work_ms())).into_iter().collect(),
            tokens: run.tokens_in + run.tokens_out,
            with_usage: u32::from(run.has_usage),
            nudged: u32::from(run.nudges() > 0),
            sent_back: u32::from(run.send_backs() > 0),
            ..AgentSum::default()
        })
        .collect();
    let ai_review = stage_name(Stage::AiReview);
    let build = stage_name(Stage::Build);

    // Findings, by the AI review run that was going when they were read.
    {
        let mut stmt = conn.prepare(
            "SELECT f.project, f.slug, f.review, f.severity, f.count, r.at
             FROM findings f LEFT JOIN reviews r ON r.project = f.project
                AND r.slug = f.slug AND r.kind = 'ai' AND r.n = f.review
             WHERE (?1 IS NULL OR f.project = ?1) AND (?2 IS NULL OR f.slug = ?2)",
        )?;
        let mut rows = stmt.query(params![project, None::<&str>])?;
        while let Some(r) = rows.next()? {
            let (p, s): (String, String) = (r.get(0)?, r.get(1)?);
            let review: u32 = r.get(2)?;
            let severity: String = r.get(3)?;
            let count: u32 = r.get(4)?;
            let at: Option<i64> = r.get(5)?;
            let found = match at {
                Some(at) => runs.of_stage_at(&p, &s, &ai_review, Some(at)),
                // Read before reviews were kept: the run of its number.
                None => (runs
                    .by_key
                    .get(&(p.clone(), s.clone(), ai_review.clone(), review)))
                .and_then(|list| list.last().copied())
                .or_else(|| runs.of_stage_at(&p, &s, &ai_review, None)),
            };
            if let Some(i) = found {
                *per_run[i].findings.entry(severity).or_default() += count;
            }
        }
    }
    // Fixes, by the Build run going when the round opened.
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, coalesce(opened, closed), fixes FROM fix_rounds WHERE {OF}"
        ))?;
        let mut rows = stmt.query(params![project, None::<&str>])?;
        while let Some(r) = rows.next()? {
            let (p, s): (String, String) = (r.get(0)?, r.get(1)?);
            let at: Option<i64> = r.get(2)?;
            let fixes: u32 = r.get(3)?;
            if let Some(i) = runs.of_stage_at(&p, &s, &build, at) {
                per_run[i].fixes += fixes;
            }
        }
    }
    // Verify after each Build run: the first that ended with a word,
    // begun after it and before the next.
    {
        let mut verifies: HashMap<(String, String), Vec<(i64, bool)>> = HashMap::new();
        let mut stmt = conn.prepare(&format!(
            "SELECT project, slug, started, passed FROM verify_runs
             WHERE {OF} AND started IS NOT NULL AND passed IS NOT NULL ORDER BY started"
        ))?;
        let mut rows = stmt.query(params![project, None::<&str>])?;
        while let Some(r) = rows.next()? {
            let key = (r.get(0)?, r.get(1)?);
            verifies
                .entry(key)
                .or_default()
                .push((r.get(2)?, r.get(3)?));
        }
        for (i, run) in runs
            .all
            .iter()
            .enumerate()
            .filter(|(_, r)| r.stage == build)
        {
            let next = (runs.all.iter())
                .filter(|o| o.project == run.project && o.slug == run.slug && o.stage == build)
                .map(|o| o.started)
                .filter(|started| *started > run.started)
                .min();
            let key = (run.project.clone(), run.slug.clone());
            let after = (verifies.get(&key).into_iter().flatten())
                .find(|(started, _)| *started >= run.started && next.is_none_or(|n| *started < n));
            if let Some((_, passed)) = after {
                per_run[i].verified += 1;
                per_run[i].verify_passed += u32::from(*passed);
            }
        }
    }

    let mut groups: BTreeMap<(Stage, String, String), AgentSum> = BTreeMap::new();
    for (run, one) in runs.all.iter().zip(per_run) {
        let Some(stage) = stage_of(&run.stage) else {
            continue;
        };
        let sum = groups
            .entry((stage, run.agent_key(), run.model()))
            .or_default();
        sum.runs += one.runs;
        sum.spans.extend(one.spans);
        sum.tokens += one.tokens;
        sum.with_usage += one.with_usage;
        sum.nudged += one.nudged;
        sum.sent_back += one.sent_back;
        for (severity, count) in one.findings {
            *sum.findings.entry(severity).or_default() += count;
        }
        sum.fixes += one.fixes;
        sum.verified += one.verified;
        sum.verify_passed += one.verify_passed;
    }
    let rate = |part: u32, whole: u32| match whole {
        0 => 0.0,
        whole => part as f32 / whole as f32,
    };
    Ok((groups.into_iter())
        .map(|((stage, agent, model), sum)| AgentRow {
            stage: Some(stage),
            agent,
            model,
            runs: sum.runs,
            median_secs: median(sum.spans),
            // Chats that tell no tokens (Cursor's) say nothing of them.
            tokens_per_run: (sum.with_usage > 0 && sum.tokens > 0)
                .then(|| sum.tokens / u64::from(sum.with_usage)),
            nudge_rate: rate(sum.nudged, sum.runs),
            send_back_rate: rate(sum.sent_back, sum.runs),
            findings: sum.findings,
            fixes: sum.fixes,
            verify_after_build: (stage == Stage::Build && sum.verified > 0)
                .then(|| rate(sum.verify_passed, sum.verified)),
        })
        .collect())
}

/// The Checks tab: each repository's check commands. Checks cut short by
/// something other than the code say nothing of it, and don't count.
fn checks(conn: &Connection, project: Option<&str>) -> rusqlite::Result<Vec<CheckRow>> {
    #[derive(Default)]
    struct Sum {
        runs: u32,
        failed: u32,
        secs: Vec<u64>,
        last_failure: Option<(i64, Option<String>)>,
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT repo, command, secs, passed, ended, log FROM checks
         WHERE {OF} AND interrupted = 0 ORDER BY coalesce(ended, 0)"
    ))?;
    let mut rows = stmt.query(params![project, None::<&str>])?;
    let mut sums: BTreeMap<(String, String), Sum> = BTreeMap::new();
    while let Some(r) = rows.next()? {
        let sum = sums.entry((r.get(0)?, r.get(1)?)).or_default();
        let secs_ran: Option<i64> = r.get(2)?;
        let passed: Option<bool> = r.get(3)?;
        sum.runs += 1;
        sum.secs.extend(secs_ran.map(|s| s.max(0) as u64));
        if passed != Some(true) {
            sum.failed += 1;
            sum.last_failure = Some((r.get::<_, Option<i64>>(4)?.unwrap_or(0), r.get(5)?));
        }
    }
    Ok((sums.into_iter())
        .map(|((repo, command), sum)| CheckRow {
            repo,
            command,
            runs: sum.runs,
            failure_rate: match sum.runs {
                0 => 0.0,
                runs => sum.failed as f32 / runs as f32,
            },
            median_secs: median(sum.secs),
            last_failure: sum.last_failure.as_ref().map(|(at, _)| secs(*at)),
            last_log: (sum.last_failure)
                .and_then(|(_, log)| log)
                .map(PathBuf::from),
        })
        .collect())
}

/// Feature `slug` of `project` in detail, if the index has it.
fn detail(conn: &Connection, project: &str, slug: &str) -> rusqlite::Result<Option<Detail>> {
    let title: Option<String> = conn
        .query_row(
            "SELECT title FROM features WHERE project = ?1 AND slug = ?2",
            params![project, slug],
            |r| r.get(0),
        )
        .optional()?;
    let Some(title) = title else {
        return Ok(None);
    };
    let of = params![Some(project), Some(slug)];
    let runs = load_runs(conn, Some(project), Some(slug))?;
    let mut sorted: Vec<&Run> = runs.all.iter().collect();
    sorted.sort_by_key(|r| (r.started, r.stage.clone(), r.n));
    let run_rows: Vec<RunRow> = (sorted.iter())
        .filter_map(|run| {
            Some(RunRow {
                stage: stage_of(&run.stage)?,
                n: run.n,
                agent: run.kind.unwrap_or_else(|| AgentKind::of(&run.agent)),
                model: run.model(),
                started: secs(run.started),
                ended: run.end().map(secs),
                tokens_in: run.tokens_in,
                tokens_out: run.tokens_out,
                nudges: run.nudges(),
                send_backs: run.send_backs(),
                transcript: run.transcript.clone(),
            })
        })
        .collect();

    let mut verify_rows = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT v.run, v.started, v.ended,
                (SELECT count(*) FROM checks c WHERE c.project = v.project
                    AND c.slug = v.slug AND c.run = v.run AND c.passed = 1),
                (SELECT count(*) FROM checks c WHERE c.project = v.project
                    AND c.slug = v.slug AND c.run = v.run AND c.passed = 0
                    AND c.interrupted = 0)
             FROM verify_runs v WHERE {OF} ORDER BY coalesce(v.started, 0), v.run"
        ))?;
        let mut rows = stmt.query(of)?;
        while let Some(r) = rows.next()? {
            verify_rows.push(VerifyRow {
                run: r.get(0)?,
                started: r.get::<_, Option<i64>>(1)?.map(secs),
                ended: r.get::<_, Option<i64>>(2)?.map(secs),
                passed: r.get(3)?,
                failed: r.get(4)?,
            });
        }
    }

    let mut marked: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT stage, marked FROM stages WHERE {OF} AND marked IS NOT NULL"
        ))?;
        let mut rows = stmt.query(of)?;
        while let Some(r) = rows.next()? {
            marked.insert(r.get(0)?, r.get(1)?);
        }
    }
    let mut stages = Vec::new();
    for stage in Stage::ALL {
        let name = stage_name(stage);
        let of_stage = || runs.all.iter().filter(|r| r.stage == name);
        let mut started = of_stage().map(|r| r.started).min().map(secs);
        let mut ended = of_stage()
            .filter_map(|r| r.ready.max(r.ended))
            .max()
            .map(secs);
        if stage == Stage::Verify {
            let first = verify_rows.iter().filter_map(|v| v.started).min();
            started = [started, first].into_iter().flatten().min();
            let last = verify_rows.iter().filter_map(|v| v.ended).max();
            ended = ended.max(last);
        }
        if let Some(at) = marked.get(&name) {
            ended = Some(secs(*at));
        }
        if started.is_some() || ended.is_some() {
            stages.push(StageSpan {
                stage,
                started,
                ended,
            });
        }
    }

    let mut fix_rows = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT n, opened, closed, fixes FROM fix_rounds WHERE {OF} ORDER BY n"
        ))?;
        let mut rows = stmt.query(of)?;
        while let Some(r) = rows.next()? {
            fix_rows.push(FixRow {
                n: r.get(0)?,
                opened: r.get::<_, Option<i64>>(1)?.map(secs),
                closed: r.get::<_, Option<i64>>(2)?.map(secs),
                fixes: r.get(3)?,
            });
        }
    }

    Ok(Some(Detail {
        project: project.to_owned(),
        slug: slug.to_owned(),
        title: if title.is_empty() {
            slug.to_owned()
        } else {
            title
        },
        stages,
        runs: run_rows,
        fix_rounds: fix_rows,
        verify_runs: verify_rows,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;
    use crate::ai_work::{
        BlobRef, EndWhy, Event, Line, RunChange, SendKind, StartHow, Via,
        index::{self, testing::*},
    };

    fn run(n: u32, agent: &str, model: &str, started: u64) -> Value {
        json!({"n": n, "agent": agent, "model": model, "started": started, "prompt": "/x",
            "template": "t", "command": "c"})
    }

    fn started(at: u64, stage: Stage, n: u32, run: Value) -> Line {
        line(
            at,
            Event::RunStarted {
                stage,
                n,
                run,
                prompt: None,
                how: StartHow::Fresh,
            },
        )
    }

    fn changed(at: u64, stage: Stage, n: u32, change: RunChange) -> Line {
        line(at, Event::RunChanged { stage, n, change })
    }

    fn sent(at: u64, stage: Stage, n: u32, send: SendKind) -> Line {
        line(
            at,
            Event::Sent {
                stage,
                n,
                send,
                text: "go".into(),
                files: BTreeMap::new(),
                via: Via::Pane,
            },
        )
    }

    fn check(
        at: u64,
        run: &str,
        i: usize,
        repo: &str,
        command: &str,
        exit: Option<i32>,
        secs: u64,
    ) -> Line {
        let mut outcome = json!({"repo": repo, "command": command, "secs": secs});
        match exit {
            Some(exit) => outcome["exit"] = json!(exit),
            None => outcome["interrupted"] = json!(true),
        }
        line(
            at,
            Event::CheckEnded {
                run: run.into(),
                key: format!("{repo} {command}"),
                i,
                outcome,
                log: Some(BlobRef::of(format!("{run}{i}").as_bytes())),
                truncated: false,
            },
        )
    }

    fn verify(at: u64, run: &str, started: u64, passed: Option<bool>) -> Line {
        let verify = json!({"n": 1, "started": started, "finished": at, "passed": passed});
        match passed {
            None => line(
                at,
                Event::VerifyStarted {
                    run: run.into(),
                    verify,
                },
            ),
            Some(_) => line(
                at,
                Event::VerifyEnded {
                    run: run.into(),
                    verify,
                },
            ),
        }
    }

    /// Two features: "Alpha" (p/a) through Plan, two Builds, a fix round,
    /// two Verify runs and an AI review; and q/b, one Build and a Verify.
    fn fixture(name: &str) -> (PathBuf, Connection) {
        let root = root(name);
        let a = root.join("p").join("a");
        let b = root.join("q").join("b");
        let claude = "Claude Code";
        append(
            &a,
            "here",
            &[
                started(1000, Stage::Plan, 1, run(1, claude, "claude-opus", 1000)),
                sent(1000, Stage::Plan, 1, SendKind::Start),
                changed(1050, Stage::Plan, 1, RunChange::Nudged),
                sent(1050, Stage::Plan, 1, SendKind::Nudge),
                changed(1100, Stage::Plan, 1, RunChange::Ready),
                changed(1200, Stage::Plan, 1, RunChange::SentBack),
                sent(1200, Stage::Plan, 1, SendKind::SendBack),
                changed(1250, Stage::Plan, 1, RunChange::Ready),
                line(
                    1300,
                    Event::Marked {
                        stage: Stage::Plan,
                        mark: json!({"at": 1300, "by": "you"}),
                    },
                ),
                started(2000, Stage::Build, 1, run(1, "Codex", "gpt-5", 2000)),
                verify(2350, "v1@2350", 2350, None),
                check(2390, "v1@2350", 0, "api", "cargo test", Some(1), 30),
                check(2391, "v1@2350", 1, "api", "cargo clippy", Some(0), 10),
                verify(2400, "v1@2350", 2350, Some(false)),
            ],
        );
        // Another machine wrote what came next.
        let mut ended = run(1, "Codex", "gpt-5", 2000);
        ended["ready"] = json!(2300);
        append(
            &a,
            "m2",
            &[
                line(
                    2999,
                    Event::RunEnded {
                        stage: Stage::Build,
                        n: 1,
                        why: EndWhy::Replaced,
                        run: ended,
                    },
                ),
                started(3000, Stage::Build, 2, run(2, "Codex", "gpt-5", 3000)),
                line(
                    3100,
                    Event::FixRoundOpened {
                        n: 1,
                        round: json!({"n": 1, "stage": "ai-review", "items": [], "done": 0}),
                    },
                ),
                line(
                    3150,
                    Event::FixRoundClosed {
                        n: 1,
                        round: json!({"n": 1, "stage": "ai-review", "items": ["x", "y"], "done": 2}),
                    },
                ),
                changed(3200, Stage::Build, 2, RunChange::Ready),
                verify(3300, "v2@3300", 3300, None),
                check(3320, "v2@3300", 0, "api", "cargo test", Some(0), 20),
                check(3330, "v2@3300", 1, "api", "cargo clippy", Some(0), 12),
                check(3340, "v2@3300", 2, "web", "npm test", None, 99),
                verify(3400, "v2@3300", 3300, Some(true)),
                started(
                    4000,
                    Stage::AiReview,
                    1,
                    run(1, claude, "claude-sonnet", 4000),
                ),
                changed(4060, Stage::AiReview, 1, RunChange::Ready),
                line(
                    4060,
                    Event::ReviewRead {
                        n: 1,
                        findings: 3,
                        review: None,
                    },
                ),
                line(
                    4070,
                    Event::Transcript {
                        agent: AgentKind::Claude,
                        session: "s1".into(),
                        runs: vec![(Stage::Plan, 1)],
                        raw_bytes: 1,
                        complete: true,
                    },
                ),
            ],
        );
        std::fs::write(
            a.join("record.json"),
            json!({"title": "Alpha", "status": "open", "created": 1000,
                "review": {"n": 1, "findings": [
                    {"id": "r1-1", "severity": "high"}, {"id": "r1-2", "severity": "medium"},
                    {"id": "r1-3", "severity": "medium"}, {"id": "u1", "severity": "high"}]}})
            .to_string(),
        )
        .unwrap();
        // 1010 s is 00:16:50; 1210 s is 00:20:10.
        let msg = |id: &str, at: &str, input: u64, create: u64, read: u64, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"1970-01-01T{at}Z","message":{{"id":"{id}","model":"claude-opus","usage":{{"input_tokens":{input},"cache_creation_input_tokens":{create},"cache_read_input_tokens":{read},"output_tokens":{out}}}}}}}"#
            )
        };
        let raw = [
            msg("m1", "00:16:50", 5, 10, 100, 20),
            msg("m1", "00:16:51", 5, 10, 100, 20),
            msg("m2", "00:20:10", 5, 0, 200, 30),
        ]
        .join("\n");
        chat(&a, "claude", "s1", "[]", &raw);

        let mut ended = run(1, "Codex", "gpt-5", 5000);
        ended["ready"] = json!(5100);
        append(
            &b,
            "here",
            &[
                started(5000, Stage::Build, 1, run(1, "Codex", "gpt-5", 5000)),
                line(
                    5150,
                    Event::RunEnded {
                        stage: Stage::Build,
                        n: 1,
                        why: EndWhy::FeatureClosed,
                        run: ended,
                    },
                ),
                verify(5200, "v1@5200", 5200, None),
                check(5240, "v1@5200", 0, "api", "cargo test", Some(0), 40),
                verify(5250, "v1@5200", 5200, Some(true)),
            ],
        );
        std::fs::write(
            b.join("record.json"),
            json!({"title": "", "status": "done", "created": 4900}).to_string(),
        )
        .unwrap();

        let mut conn = index::open(&root).unwrap();
        index::ingest(&mut conn, &index::dirs(&root)).unwrap();
        drop(conn);
        let conn = index::open_read(&root).unwrap();
        (root, conn)
    }

    fn scope(tab: Tab, project: Option<&str>) -> Scope {
        Scope {
            tab,
            project: project.map(str::to_owned),
            detail: None,
        }
    }

    #[test]
    fn features_tab_sums_each_features_runs() {
        let (root, conn) = fixture("features");
        let view = query(&conn, &scope(Tab::Features, None)).unwrap();
        assert!(view.agents.is_empty() && view.checks.is_empty());
        let alpha = FeatureRow {
            project: "p".into(),
            slug: "a".into(),
            title: "Alpha".into(),
            status: "open".into(),
            created: 1000,
            // Plan 100 + 50 after it was sent back, Builds 300 and 200,
            // AI review 60.
            ai_secs: 710,
            runs: 4,
            fix_rounds: 1,
            tokens_in: 320,
            tokens_out: 50,
            nudges: 1,
            send_backs: 1,
        };
        let b = FeatureRow {
            project: "q".into(),
            slug: "b".into(),
            title: "b".into(),
            status: "done".into(),
            created: 4900,
            ai_secs: 100,
            runs: 1,
            ..FeatureRow::default()
        };
        assert_eq!(view.features, vec![b, alpha.clone()]);
        let only = query(&conn, &scope(Tab::Features, Some("p"))).unwrap();
        assert_eq!(only.features, vec![alpha]);
        let none = query(&conn, &scope(Tab::Features, Some("zz"))).unwrap();
        assert!(none.features.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn agents_tab_goes_by_stage_agent_and_model() {
        let (root, conn) = fixture("agents");
        let view = query(&conn, &scope(Tab::Agents, None)).unwrap();
        let plan = AgentRow {
            stage: Some(Stage::Plan),
            agent: "claude".into(),
            model: "claude-opus".into(),
            runs: 1,
            median_secs: Some(150),
            tokens_per_run: Some(370),
            nudge_rate: 1.0,
            send_back_rate: 1.0,
            ..AgentRow::default()
        };
        let build = AgentRow {
            stage: Some(Stage::Build),
            agent: "codex".into(),
            model: "gpt-5".into(),
            runs: 3,
            median_secs: Some(200),
            fixes: 2,
            // Alpha's first Build failed Verify, its second and b's passed.
            verify_after_build: Some(2.0 / 3.0),
            ..AgentRow::default()
        };
        let review = AgentRow {
            stage: Some(Stage::AiReview),
            agent: "claude".into(),
            model: "claude-sonnet".into(),
            runs: 1,
            median_secs: Some(60),
            findings: [("high".to_owned(), 1), ("medium".to_owned(), 2)].into(),
            ..AgentRow::default()
        };
        assert_eq!(view.agents, vec![plan, build.clone(), review]);
        let q = query(&conn, &scope(Tab::Agents, Some("q"))).unwrap();
        assert_eq!(
            q.agents,
            vec![AgentRow {
                runs: 1,
                median_secs: Some(100),
                fixes: 0,
                verify_after_build: Some(1.0),
                ..build
            }]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checks_tab_goes_by_repository_and_command() {
        let (root, conn) = fixture("checks");
        let view = query(&conn, &scope(Tab::Checks, None)).unwrap();
        let log = |run: &str, i: usize| {
            BlobRef::of(format!("{run}{i}").as_bytes()).path(&root.join("p").join("a"))
        };
        assert_eq!(
            view.checks,
            vec![
                CheckRow {
                    repo: "api".into(),
                    command: "cargo clippy".into(),
                    runs: 2,
                    failure_rate: 0.0,
                    median_secs: Some(11),
                    last_failure: None,
                    last_log: None,
                },
                CheckRow {
                    repo: "api".into(),
                    command: "cargo test".into(),
                    runs: 3,
                    failure_rate: 1.0 / 3.0,
                    median_secs: Some(30),
                    last_failure: Some(2390),
                    last_log: Some(log("v1@2350", 0)),
                },
            ]
        );
        let q = query(&conn, &scope(Tab::Checks, Some("q"))).unwrap();
        assert_eq!(q.checks.len(), 1);
        assert_eq!(q.checks[0].runs, 1);
        assert_eq!(q.checks[0].failure_rate, 0.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_feature_in_detail() {
        let (root, conn) = fixture("detail");
        let asked = Scope {
            detail: Some(("p".into(), "a".into())),
            ..scope(Tab::Features, Some("p"))
        };
        let detail = query(&conn, &asked).unwrap().detail.unwrap();
        assert_eq!(detail.title, "Alpha");
        let span = |stage, started, ended| StageSpan {
            stage,
            started,
            ended,
        };
        assert_eq!(
            detail.stages,
            vec![
                span(Stage::Plan, Some(1000), Some(1300)),
                span(Stage::Build, Some(2000), Some(3200)),
                span(Stage::Verify, Some(2350), Some(3400)),
                span(Stage::AiReview, Some(4000), Some(4060)),
            ]
        );
        type Seen = (Stage, u32, AgentKind, u64, Option<u64>, u64, u32);
        let runs: Vec<Seen> = (detail.runs.iter())
            .map(|r| {
                (
                    r.stage,
                    r.n,
                    r.agent,
                    r.started,
                    r.ended,
                    r.tokens_in,
                    r.send_backs,
                )
            })
            .collect();
        assert_eq!(
            runs,
            vec![
                // With no end told, it was ready, or its agent stopped.
                (Stage::Plan, 1, AgentKind::Claude, 1000, Some(1250), 320, 1),
                (Stage::Build, 1, AgentKind::Codex, 2000, Some(2999), 0, 0),
                (Stage::Build, 2, AgentKind::Codex, 3000, Some(3200), 0, 0),
                (
                    Stage::AiReview,
                    1,
                    AgentKind::Claude,
                    4000,
                    Some(4060),
                    0,
                    0
                ),
            ]
        );
        let transcript = detail.runs[0].transcript.as_deref().unwrap();
        assert!(transcript.ends_with(Path::new("p/a/sessions/claude-s1/transcript.jsonl.zst")));
        assert_eq!(detail.runs[1].transcript, None);
        assert_eq!(
            detail.fix_rounds,
            vec![FixRow {
                n: 1,
                opened: Some(3100),
                closed: Some(3150),
                fixes: 2,
            }]
        );
        assert_eq!(
            detail.verify_runs,
            vec![
                VerifyRow {
                    run: "v1@2350".into(),
                    started: Some(2350),
                    ended: Some(2400),
                    passed: 1,
                    failed: 1,
                },
                VerifyRow {
                    run: "v2@3300".into(),
                    started: Some(3300),
                    ended: Some(3400),
                    passed: 2,
                    failed: 0,
                },
            ]
        );
        let unknown = Scope {
            detail: Some(("p".into(), "zz".into())),
            ..asked
        };
        assert_eq!(query(&conn, &unknown).unwrap().detail, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A backfill names the agent as its folder does, the record as the
    /// picker does: one agent all the same. A run nothing tells the end
    /// of says nothing of how long runs take.
    #[test]
    fn an_agent_is_one_row_whoever_named_it() {
        let root = root("agent-names");
        let dir = root.join("p").join("a");
        append(
            &dir,
            "here",
            &[
                started(1000, Stage::Plan, 1, run(1, "claude", "opus", 1000)),
                changed(1100, Stage::Plan, 1, RunChange::Ready),
                started(2000, Stage::Plan, 2, run(2, "Claude Code", "opus", 2000)),
                changed(2300, Stage::Plan, 2, RunChange::Stop),
                started(3000, Stage::Plan, 3, run(3, "Claude Code", "opus", 3000)),
            ],
        );
        let mut conn = index::open(&root).unwrap();
        index::ingest(&mut conn, std::slice::from_ref(&dir)).unwrap();
        let view = query(&conn, &scope(Tab::Agents, None)).unwrap();
        let [row] = view.agents.as_slice() else {
            panic!("{:?}", view.agents)
        };
        assert_eq!((row.agent.as_str(), row.runs), ("claude", 3));
        // 100 s and 300 s: the third, never ended, isn't a 0 s.
        assert_eq!(row.median_secs, Some(200));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_index_has_no_rows() {
        let root = root("empty");
        let conn = index::open(&root).unwrap();
        for tab in [Tab::Features, Tab::Agents, Tab::Checks] {
            let view = query(&conn, &scope(tab, None)).unwrap();
            assert_eq!(
                view,
                View {
                    scope: scope(tab, None),
                    ..View::default()
                }
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn medians_take_the_middle() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3, 1, 2]), Some(2));
        assert_eq!(median(vec![4, 1, 2, 3]), Some(2));
    }
}
