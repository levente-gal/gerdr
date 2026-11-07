//! What the stats panel shows, read from the index: the features, the
//! agents by stage and model, the checks, and one feature in detail.

use std::{collections::BTreeMap, path::PathBuf};

use rusqlite::Connection;

use super::AgentKind;
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

/// What the index holds for `scope`.
pub fn query(conn: &Connection, scope: &Scope) -> Result<View, String> {
    let _ = conn;
    Ok(View {
        scope: scope.clone(),
        ..View::default()
    })
}
