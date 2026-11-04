//! The agents that run a feature's stages: which CLI, started how, and what
//! gerdr keeps of each run (`StageRun`).
//!
//! An agent stage (Discover, Plan, Build; later AI review) starts its
//! agent in a new tab of the feature's workspace, in the feature folder,
//! with the stage's prompt rendered to `<feature data dir>/prompts/<stage>-<n>.md`
//! (see `crate::prompts`). The agent is told to write its stage's file
//! (`brief.md`, `plan.md`; see `artifacts`) into the feature data dir,
//! which it is given with `--add-dir`: the code's worktrees stay clean.
//!
//! The CLIs' flags, as checked on 2026-11-02:
//!
//! - `claude [--add-dir <dir>]… -- "<prompt>"`: interactive, prompt first.
//! - `cursor-agent --trust [--add-dir <dir>]… -- "<prompt>"`: a folder it
//!   doesn't know stops at a trust question before the prompt, and each
//!   feature's folder is new (checked on 2026-11-04).
//! - `codex [--add-dir <dir>]… -- "<prompt>"`.
//!
//! The `--` matters: claude's `--add-dir` takes any number of directories,
//! so without it the prompt is read as one more, and the agent starts with
//! no prompt at all and no complaint (checked on 2026-11-03).
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
    /// `env`, `exec` and `VAR=value` before it are skipped.
    pub fn of(command: &str) -> Flavor {
        let program = command
            .split_whitespace()
            .find(|word| !matches!(*word, "env" | "exec") && !word.contains('='))
            .unwrap_or("");
        let program = program.trim_matches(|c| c == '\'' || c == '"');
        match program.rsplit('/').next().unwrap_or(program) {
            "claude" => Flavor::Claude,
            "codex" => Flavor::Codex,
            "cursor-agent" => Flavor::Cursor,
            _ => Flavor::Other,
        }
    }

    /// Whether it asks to trust a new folder unless told not to.
    pub fn asks_trust(self) -> bool {
        self == Flavor::Cursor
    }

    /// Whether it takes `--add-dir`.
    pub fn adds_dirs(self) -> bool {
        self != Flavor::Other
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

/// What `stage` needs of its agent; `None` for a stage no agent runs.
pub fn needs(stage: Stage) -> Option<Needs> {
    match stage {
        Stage::Discover => Some(Needs {
            read_only: false,
            artifact: Some("brief.md"),
            template: "discover",
        }),
        Stage::Plan => Some(Needs {
            read_only: true,
            artifact: Some("plan.md"),
            template: "plan",
        }),
        Stage::Build => Some(Needs {
            read_only: false,
            artifact: Some("build.md"),
            template: "build",
        }),
        Stage::AiReview => Some(Needs {
            read_only: true,
            artifact: Some(super::review_import::REVIEW),
            template: "ai-review",
        }),
        _ => None,
    }
}

/// The shell command that starts `base` (an `[agents]` command such as
/// `claude`) on the prompt in `prompt`, allowed into `dirs` too.
/// The prompt goes in as `"$(cat '<prompt>')"`, so a long one with quotes
/// and newlines arrives whole. A CLI gerdr knows gets `--` before it, so no
/// flag can take the prompt for its own value, whether gerdr added the flag
/// or the `[agents]` command ends in one.
pub fn command_line(base: &str, prompt: &Path, dirs: &[PathBuf]) -> String {
    use crate::desktop::shell_quote;
    let mut line = if Flavor::of(base).asks_trust() {
        allowed_into(&format!("{} --trust", base.trim()), dirs)
    } else {
        allowed_into(base, dirs)
    };
    if Flavor::of(base).adds_dirs() {
        line.push_str(" --");
    }
    line.push_str(&format!(
        " \"$(cat {})\"",
        shell_quote(&prompt.to_string_lossy())
    ));
    line
}

/// The shell command that starts `base` allowed into `dirs` too, for a CLI
/// gerdr knows the flag of: the others reach the feature's files through
/// `layout::DOCS_LINK`.
pub fn allowed_into(base: &str, dirs: &[PathBuf]) -> String {
    let mut line = base.trim().to_owned();
    if Flavor::of(base).adds_dirs() {
        for dir in dirs {
            line.push_str(" --add-dir ");
            line.push_str(&crate::desktop::shell_quote(&dir.to_string_lossy()));
        }
    }
    line
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
    /// A Build agent stopped after a milestone, waiting for the user to
    /// let it go on (see `Stop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Stop>,
}

/// A Build agent's stop after a milestone: `build.md` ticks `done` of
/// `total` milestones, and the agent waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub done: u32,
    pub total: u32,
    /// Unix seconds.
    pub at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors_go_by_program_name() {
        assert_eq!(Flavor::of("claude"), Flavor::Claude);
        assert_eq!(Flavor::of("/usr/bin/claude --model opus"), Flavor::Claude);
        assert_eq!(Flavor::of("env FOO=1 codex"), Flavor::Codex);
        assert_eq!(Flavor::of("cursor-agent"), Flavor::Cursor);
        assert_eq!(Flavor::of("aider"), Flavor::Other);
        assert_eq!(Flavor::of(""), Flavor::Other);
        assert!(Flavor::Cursor.adds_dirs());
        assert!(!Flavor::Other.adds_dirs());
    }

    #[test]
    fn command_lines_per_flavor() {
        let prompt = Path::new("/d/my feature/prompts/plan-1.md");
        let dirs = [PathBuf::from("/d/my feature")];
        for base in ["claude", "codex", "cursor-agent --trust"] {
            let program = base.split(' ').next().unwrap();
            assert_eq!(
                command_line(program, prompt, &dirs),
                format!(
                    "{base} --add-dir '/d/my feature' -- \"$(cat '/d/my feature/prompts/plan-1.md')\""
                )
            );
        }
        assert_eq!(
            command_line("aider --yes", prompt, &dirs),
            "aider --yes \"$(cat '/d/my feature/prompts/plan-1.md')\""
        );
        assert_eq!(
            command_line("claude", Path::new("/p.md"), &[]),
            "claude -- \"$(cat /p.md)\""
        );
    }

    #[test]
    fn an_agent_started_by_hand_is_allowed_into_the_dirs() {
        let dirs = [PathBuf::from("/d/my feature"), PathBuf::from("/e")];
        for base in ["claude", "codex", "cursor-agent"] {
            assert_eq!(
                allowed_into(base, &dirs),
                format!("{base} --add-dir '/d/my feature' --add-dir /e")
            );
        }
        for base in ["opencode", "aider --yes", "gemini"] {
            assert_eq!(allowed_into(base, &dirs), base);
        }
        assert_eq!(allowed_into(" claude ", &[]), "claude");
    }

    #[test]
    fn discover_plan_and_build_have_agents() {
        assert_eq!(needs(Stage::Discover).unwrap().artifact, Some("brief.md"));
        let plan = needs(Stage::Plan).unwrap();
        assert!(plan.read_only);
        assert_eq!(plan.template, "plan");
        let build = needs(Stage::Build).unwrap();
        assert!(!build.read_only);
        assert_eq!(build.artifact, Some("build.md"));
        assert_eq!(needs(Stage::Verify), None);
        let review = needs(Stage::AiReview).unwrap();
        assert!(review.read_only);
        assert_eq!(review.artifact, Some("review.json"));
        assert_eq!(review.template, "ai-review");
        assert_eq!(needs(Stage::CodeReview), None);
    }
}
