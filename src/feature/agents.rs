//! The agents that run a feature's stages: which CLI, started how, and what
//! gerdr keeps of each run (`StageRun`).
//!
//! An agent stage (Discover, Plan; later Build and AI review) starts its
//! agent in a new tab of the feature's workspace, in the feature folder,
//! with the stage's prompt rendered to `<feature data dir>/prompts/<stage>-<n>.md`
//! (see `crate::prompts`). The agent is told to write its stage's file
//! (`brief.md`, `plan.md`; see `artifacts`) into the feature data dir,
//! which it is given with `--add-dir`: the code's worktrees stay clean.
//!
//! The CLIs' flags, as checked on 2026-11-02:
//!
//! - `claude [--add-dir <dir>]… "<prompt>"`: interactive, prompt first.
//! - `cursor-agent [--add-dir <dir>]… "<prompt>"`.
//! - `codex [--add-dir <dir>]… "<prompt>"`.
//!
//! A read-only stage (Plan) is told so in its prompt; its plan mode would
//! stop it writing `plan.md`. gerdr checks the code didn't change instead
//! (`Feature::stale` / the patch reading), so read-only holds by check,
//! not by flag.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Stage;

/// Which CLI a command runs, for its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Claude,
    Codex,
    Cursor,
    /// Another CLI: given the prompt as its last argument, no other flag.
    Other,
}

impl Flavor {
    /// The flavor of `command`, by its program's name.
    pub fn of(command: &str) -> Flavor {
        todo!("agents: Flavor::of {command}")
    }

    /// Whether it takes `--add-dir`.
    pub fn adds_dirs(self) -> bool {
        todo!()
    }
}

/// What a stage needs of its agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// It must not change the code.
    pub read_only: bool,
    /// The file it must write, in the feature data dir; `None` for a
    /// stage without one.
    pub artifact: Option<&'static str>,
    /// The built-in template it starts with (`crate::prompts::builtin`).
    pub template: &'static str,
}

/// What `stage` needs of its agent; `None` for a stage no agent runs (yet).
pub fn needs(stage: Stage) -> Option<Needs> {
    todo!("agents: needs {stage:?}")
}

/// The shell command that starts `base` (an `[agents]` command such as
/// `claude`) on the prompt in `prompt`, allowed into `dirs` too.
pub fn command_line(base: &str, prompt: &Path, dirs: &[PathBuf]) -> String {
    todo!("agents: command_line {base} {prompt:?} {dirs:?}")
}

/// One run of a stage's agent, kept in the feature record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageRun {
    /// Which run of the stage: 1, then 2 after a fresh agent.
    pub n: u32,
    /// The agent's name, as the picker shows it ("Claude Code").
    pub agent: String,
    /// The command it was started with.
    pub command: String,
    /// The herdr pane it runs in, while known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// The rendered prompt.
    pub prompt: PathBuf,
    /// The template it was rendered from: `builtin:plan@1`.
    pub template: String,
    /// Unix seconds.
    pub started: u64,
    /// Whether it was nudged once to write its file.
    #[serde(default)]
    pub nudged: bool,
    /// When its file was found valid and the agent done: the gate waits
    /// for the user (Approve or Send back). Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<u64>,
    /// How many times it was sent back.
    #[serde(default)]
    pub sent_back: u32,
}
