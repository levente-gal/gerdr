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
//! Closing an agent's tab doesn't lose its chat: gerdr can start it again.
//! Claude takes a chat id of our choosing and Cursor makes one with
//! `create-chat`, so the command itself writes the id beside the prompt
//! (`session_file`), without gerdr waiting on a tool to start a tab. Both
//! go back to it with `--resume <id>`, from the same folder (checked on
//! 2026-11-04). Codex can't be told an id, so its tab stays a new agent.
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
    /// T3 Code, whose threads start in its app rather than in a terminal
    /// (see `Gerdr::t3_stage_draft`).
    T3,
    /// Another CLI: given the prompt as its last argument, no other flag.
    Other,
}

impl Flavor {
    /// The flavor of `command`, by its program's name.
    /// `env`, `exec` and `VAR=value` before it are skipped.
    pub fn of(command: &str) -> Flavor {
        let program = program(command);
        match program.rsplit('/').next().unwrap_or(program) {
            "claude" => Flavor::Claude,
            "codex" => Flavor::Codex,
            "cursor-agent" => Flavor::Cursor,
            crate::cli_agents::T3 => Flavor::T3,
            _ => Flavor::Other,
        }
    }

    /// Whether it asks to trust a new folder unless told not to.
    pub fn asks_trust(self) -> bool {
        self == Flavor::Cursor
    }

    /// Whether it takes `--add-dir`.
    pub fn adds_dirs(self) -> bool {
        !matches!(self, Flavor::Other | Flavor::T3)
    }

    /// Whether gerdr can know its chat's id, to go back to the chat.
    pub fn resumes(self) -> bool {
        matches!(self, Flavor::Claude | Flavor::Cursor)
    }
}

/// The program `command` runs: past `env`, `exec` and the variables it sets.
fn program(command: &str) -> &str {
    command
        .split_whitespace()
        .find(|word| !matches!(*word, "env" | "exec") && !word.contains('='))
        .unwrap_or("")
        .trim_matches(|c| c == '\'' || c == '"')
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
///
/// The command keeps its chat's id in `session_file(prompt)`, for a CLI that
/// can go back to it.
pub fn command_line(base: &str, prompt: &Path, dirs: &[PathBuf]) -> String {
    use crate::desktop::shell_quote;
    let mut line = unattended(base, dirs);
    let id_file = shell_quote(&session_file(prompt).to_string_lossy());
    match Flavor::of(base) {
        Flavor::Claude => line.push_str(&format!(
            " --session-id \"$(tee {id_file} </proc/sys/kernel/random/uuid)\""
        )),
        Flavor::Cursor => line.push_str(&format!(
            " --resume \"$({} create-chat | tee {id_file})\"",
            program(base)
        )),
        Flavor::Codex | Flavor::T3 | Flavor::Other => {}
    }
    if Flavor::of(base).adds_dirs() {
        line.push_str(" --");
    }
    line.push_str(&format!(
        " \"$(cat {})\"",
        shell_quote(&prompt.to_string_lossy())
    ));
    line
}

/// The shell command that goes back to chat `session` of `base`, in the
/// folder it ran in, allowed into `dirs` too.
pub fn resume_line(base: &str, session: &str, dirs: &[PathBuf]) -> String {
    let id = crate::desktop::shell_quote(session);
    format!("{} --resume {id}", unattended(base, dirs))
}

/// How to run `argv`, what a pane is seen running, again: a chat started
/// with an id of its own goes back to that chat. Claude refuses an id already
/// used, and the prompt would be sent a second time.
pub fn rerun_argv<'a>(argv: &[&'a str]) -> Vec<&'a str> {
    let same = || argv.to_vec();
    if !argv.first().is_some_and(|a| Flavor::of(a).resumes()) {
        return same();
    }
    let Some(flag) = (argv.iter()).position(|w| matches!(*w, "--session-id" | "--resume")) else {
        return same();
    };
    let end = argv[flag..]
        .iter()
        .position(|w| *w == "--")
        .map_or(argv.len(), |i| flag + i);
    let mut out = argv[..end].to_vec();
    out[flag] = "--resume";
    out
}

/// Where a run started on `prompt` keeps its chat's id: beside it.
pub fn session_file(prompt: &Path) -> PathBuf {
    prompt.with_extension("session")
}

/// `base` allowed into `dirs`, needing no one to answer before it starts.
fn unattended(base: &str, dirs: &[PathBuf]) -> String {
    if Flavor::of(base).asks_trust() {
        allowed_into(&format!("{} --trust", base.trim()), dirs)
    } else {
        allowed_into(base, dirs)
    }
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

/// Where a stage's agent runs in T3 Code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum T3Run {
    /// Its prompt waits in a draft in T3 Code's app, for the user to pick
    /// a model and send it.
    Draft,
    /// In a thread, sent from that draft.
    Thread {
        id: String,
        /// The agent T3 Code runs it with, as `ThreadRow::agent` names it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
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
    /// The `[agents]` command behind it, to go back to its chat with; empty
    /// for a run from before gerdr kept chats.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// The herdr pane it runs in, while known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// Where it runs in T3 Code, for one that runs there: gone with the
    /// thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t3: Option<T3Run>,
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

impl StageRun {
    /// Whether its agent is still around: a pane, or a place in T3 Code.
    pub fn live(&self) -> bool {
        self.pane.is_some() || self.t3.is_some()
    }

    /// Its T3 Code thread's id, once it was sent.
    pub fn t3_thread(&self) -> Option<&str> {
        match &self.t3 {
            Some(T3Run::Thread { id, .. }) => Some(id),
            _ => None,
        }
    }

    /// How the rest of gerdr knows its agent, once it has one to know.
    pub fn agent_key(&self) -> Option<crate::agents::AgentKey> {
        use crate::agents::AgentKey;
        match (self.t3_thread(), &self.pane) {
            (Some(id), _) => Some(AgentKey::T3(id.to_owned())),
            (None, Some(pane)) => Some(AgentKey::Herdr(pane.clone())),
            (None, None) => None,
        }
    }

    /// Whether its chat can be gone back to once its tab is closed.
    pub fn resumes(&self) -> bool {
        !self.base.is_empty() && Flavor::of(&self.base).resumes()
    }

    /// Its chat's id, as its command kept it. None until the command ran.
    pub fn session(&self) -> Option<String> {
        let id = std::fs::read_to_string(session_file(&self.prompt)).ok()?;
        Some(id.trim().to_owned()).filter(|id| !id.is_empty())
    }
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
        assert_eq!(Flavor::of("t3"), Flavor::T3);
        assert!(!Flavor::T3.adds_dirs());
        assert!(!Flavor::T3.resumes());
        assert_eq!(Flavor::of("aider"), Flavor::Other);
        assert_eq!(Flavor::of(""), Flavor::Other);
        assert!(Flavor::Cursor.adds_dirs());
        assert!(!Flavor::Other.adds_dirs());
    }

    #[test]
    fn command_lines_per_flavor() {
        let prompt = Path::new("/d/my feature/prompts/plan-1.md");
        let dirs = [PathBuf::from("/d/my feature")];
        let cat = "\"$(cat '/d/my feature/prompts/plan-1.md')\"";
        let id = "'/d/my feature/prompts/plan-1.session'";
        assert_eq!(
            command_line("claude", prompt, &dirs),
            format!(
                "claude --add-dir '/d/my feature' \
                 --session-id \"$(tee {id} </proc/sys/kernel/random/uuid)\" -- {cat}"
            )
        );
        assert_eq!(
            command_line("cursor-agent", prompt, &dirs),
            format!(
                "cursor-agent --trust --add-dir '/d/my feature' \
                 --resume \"$(cursor-agent create-chat | tee {id})\" -- {cat}"
            )
        );
        assert_eq!(
            command_line("codex", prompt, &dirs),
            format!("codex --add-dir '/d/my feature' -- {cat}")
        );
        assert_eq!(
            command_line("aider --yes", prompt, &dirs),
            "aider --yes \"$(cat '/d/my feature/prompts/plan-1.md')\""
        );
        assert_eq!(
            command_line("claude", Path::new("/p.md"), &[]),
            "claude --session-id \"$(tee /p.session </proc/sys/kernel/random/uuid)\" -- \"$(cat /p.md)\""
        );
    }

    #[test]
    fn the_program_stays_first_for_who_reads_the_command() {
        // `same_agent` tells the builder from the reviewer by it.
        let line = command_line("env A=1 /bin/cursor-agent -f", Path::new("/p.md"), &[]);
        assert_eq!(Flavor::of(&line), Flavor::Cursor);
        assert!(line.contains("$(/bin/cursor-agent create-chat | tee /p.session)"));
    }

    #[test]
    fn a_closed_agent_goes_back_to_its_chat_with_the_same_trust_and_dirs() {
        let dirs = [PathBuf::from("/d/my feature")];
        assert_eq!(
            resume_line("claude --model opus", "abc-1", &dirs),
            "claude --model opus --add-dir '/d/my feature' --resume abc-1"
        );
        assert_eq!(
            resume_line("cursor-agent", "abc-1", &dirs),
            "cursor-agent --trust --add-dir '/d/my feature' --resume abc-1"
        );
    }

    #[test]
    fn running_a_chat_again_goes_back_to_it_without_its_prompt() {
        assert_eq!(
            rerun_argv(&[
                "claude",
                "--add-dir",
                "/d",
                "--session-id",
                "id",
                "--",
                "Do it"
            ]),
            ["claude", "--add-dir", "/d", "--resume", "id"]
        );
        assert_eq!(
            rerun_argv(&[
                "/b/cursor-agent",
                "--trust",
                "--resume",
                "id",
                "--",
                "Do it"
            ]),
            ["/b/cursor-agent", "--trust", "--resume", "id"]
        );
        // Already so, or no chat of ours: as it was.
        for argv in [
            &["claude", "--resume", "id"][..],
            &["claude", "--", "Do it"],
            &["claude"],
            &["other", "--resume", "id", "--", "x"],
            &[],
        ] {
            assert_eq!(rerun_argv(argv), argv);
        }
    }

    #[test]
    fn a_run_resumes_when_its_cli_can_and_gerdr_kept_how() {
        let run = |base: &str| StageRun {
            base: base.into(),
            ..StageRun::default()
        };
        assert!(run("claude").resumes());
        assert!(run("/usr/bin/cursor-agent --model x").resumes());
        assert!(!run("codex").resumes());
        assert!(!run("aider").resumes());
        // A run from before gerdr kept chats.
        assert!(!run("").resumes());
    }

    #[test]
    fn the_chat_id_is_read_from_beside_the_prompt() {
        let dir = std::env::temp_dir().join(format!("gerdr-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = StageRun {
            prompt: dir.join("plan-1.md"),
            ..StageRun::default()
        };
        assert_eq!(run.session(), None);
        std::fs::write(dir.join("plan-1.session"), "  ").unwrap();
        assert_eq!(run.session(), None);
        std::fs::write(dir.join("plan-1.session"), "abc-1\n").unwrap();
        assert_eq!(run.session().as_deref(), Some("abc-1"));
        std::fs::remove_dir_all(&dir).unwrap();
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
