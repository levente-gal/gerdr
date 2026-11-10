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
//!   feature's folder is new (checked on 2026-11-04). In a folder of several
//!   repositories each worktree is a `--add-dir` too (`Feature::agent_dirs_for`).
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Feature, Stage};

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

    /// Whether gerdr can start it on a model: `--model`, checked for these
    /// two (a second one wins over one in the `[agents]` command).
    pub fn takes_model(self) -> bool {
        matches!(self, Flavor::Claude | Flavor::Cursor)
    }

    /// Whether gerdr can know its chat's id, to go back to the chat.
    pub fn resumes(self) -> bool {
        matches!(self, Flavor::Claude | Flavor::Cursor)
    }

    /// The line that compacts its chat, if gerdr knows one. Cursor calls it
    /// `/summarize` (read from its command list on 2026-11-05); a T3 Code
    /// thread takes none from outside.
    pub fn compact(self) -> Option<&'static str> {
        match self {
            Flavor::Claude => Some(
                "/compact Keep what the feature is, its plan, the decisions made and the code's state; drop tool output",
            ),
            Flavor::Codex => Some("/compact"),
            Flavor::Cursor => Some("/summarize"),
            Flavor::T3 | Flavor::Other => None,
        }
    }
}

/// The program `command` runs: past `env`, `exec` and the variables it sets.
pub(crate) fn program(command: &str) -> &str {
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

/// The final plan, Plan review's own file.
pub const FINAL_PLAN: &str = "plan-final.md";

/// What the agent of `feature`'s `stage` needs now: `needs`, but Plan
/// review's reviewer writes its suggestions until the final plan is due.
pub fn run_needs(feature: &super::Feature, stage: Stage) -> Option<Needs> {
    let need = needs(stage)?;
    // A pull request's review has no brief or plan of its own to go by.
    if stage == Stage::AiReview && feature.pull().is_some() {
        return Some(Needs {
            template: "pr-review",
            ..need
        });
    }
    let writing = feature.plan_review.as_ref().is_some_and(|r| r.writing);
    if stage == Stage::PlanReview && !writing {
        return Some(Needs {
            artifact: Some(super::plan_review::FILE),
            template: "plan-review",
            ..need
        });
    }
    Some(need)
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
        // Its own file is the final plan: what is reviewed, approved and
        // read by Build. `run_needs` says what its reviewer writes first.
        Stage::PlanReview => Some(Needs {
            read_only: true,
            artifact: Some(FINAL_PLAN),
            template: "plan-final",
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

/// The shell command that opens `base` (an `[agents]` command such as
/// `claude`) for the prompt in `prompt`, allowed into `dirs` too. The prompt
/// isn't on it: the agent waits, so the user can pick its model in its own
/// view, until told to read the file (see `Gerdr::send_stage_prompt`).
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
    line
}

/// `base` started on `model`, for a CLI that can be told one.
pub fn with_model(base: &str, model: Option<&str>) -> String {
    match model.filter(|_| Flavor::of(base).takes_model()) {
        Some(model) => format!(
            "{} --model {}",
            base.trim(),
            crate::desktop::shell_quote(model)
        ),
        None => base.trim().to_owned(),
    }
}

/// The command `line` told `text` as it starts: it is its first message, with
/// no waiting for the CLI to be ready. A CLI gerdr knows gets `--` before it,
/// so `--add-dir` can't take it for a folder; another gets it as its last
/// argument and nothing else.
pub fn with_prompt(line: &str, text: &str) -> String {
    let text = crate::desktop::shell_quote(text);
    match Flavor::of(line).adds_dirs() {
        true => format!("{line} -- {text}"),
        false => format!("{line} {text}"),
    }
}

/// What `run`'s chat is resumed with. Claude's chat keeps its model (tried on
/// 2.1.318), and one changed there with `/model` must stay. Whether Cursor's
/// does couldn't be told from its chat, so it is told again.
pub fn resume_base(run: &StageRun) -> String {
    match Flavor::of(&run.base) {
        Flavor::Cursor => with_model(&run.base, run.asked_model.as_deref()),
        _ => run.base.clone(),
    }
}

/// The shell command that goes back to chat `session` of `base`, in the
/// folder it ran in, allowed into `dirs` too.
pub fn resume_line(base: &str, session: &str, dirs: &[PathBuf]) -> String {
    let id = crate::desktop::shell_quote(session);
    format!("{} --resume {id}", unattended(base, dirs))
}

/// Where the script of Cursor's CLI is in `argv`, what its process shows. Its
/// launcher ends in `exec -a "$0" node --use-system-ca …/index.js "$@"`, so
/// node's flags and the script come before what the CLI was told, and the CLI
/// refuses them (`unknown option '--use-system-ca'`).
fn launcher_script(argv: &[&str]) -> Option<usize> {
    let name = argv.first()?.rsplit('/').next()?;
    let at = argv.iter().skip(1).position(|w| !w.starts_with('-'))? + 1;
    let ours = name == "cursor-agent" || (name == "agent" && argv[at].contains("/cursor-agent/"));
    (ours && argv[at].ends_with("/index.js")).then_some(at)
}

/// `line`, a saved command, with no launcher's words in it, as `rerun_argv`
/// now captures it; `None` if it has none. Older saves hold them.
pub fn without_launcher(line: &str) -> Option<String> {
    let words = crate::projects::split_command(line);
    let argv: Vec<&str> = words.iter().map(String::as_str).collect();
    launcher_script(&argv)?;
    let quoted: Vec<_> = (rerun_argv(&argv).iter())
        .map(|word| crate::desktop::shell_quote(word))
        .collect();
    Some(quoted.join(" "))
}

/// How to run `argv`, what a pane is seen running, again: a chat started
/// with an id of its own goes back to that chat. Claude refuses an id already
/// used, and the prompt would be sent a second time. Cursor's runs without
/// what its launcher adds (see `launcher_script`).
pub fn rerun_argv<'a>(argv: &[&'a str]) -> Vec<&'a str> {
    let told: Vec<&'a str>;
    let argv = match launcher_script(argv) {
        Some(script) => {
            told = [&argv[..1], &argv[script + 1..]].concat();
            &told[..]
        }
        None => argv,
    };
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
    /// The model it last used, as its chat names it ("grok-4.7"), once read
    /// (`read_models`). Kept here as the chat may be cleaned up later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model it was picked to start on, if one was: `model` says what
    /// it went on to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_model: Option<String>,
    /// The herdr pane it runs in, while known: `<machine id>/<pane>` on
    /// another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// The id of the machine it runs on, if not this one: a chat stays on the
    /// machine of its run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
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
    /// A Build agent stopped because the plan doesn't hold: it wrote a
    /// `Blocked:` line in its build log (see `artifacts::blocked`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Blocked>,
    /// Its agent waits for the prompt, which the user sends once they
    /// picked its model. Never for one in T3 Code: that one has its draft.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

impl StageRun {
    /// Whether its agent is still around: a pane, or a place in T3 Code.
    pub fn live(&self) -> bool {
        self.pane.is_some() || self.t3.is_some()
    }

    /// Whether its agent is there, waiting for the user to send its prompt.
    pub fn waits_to_send(&self) -> bool {
        self.held && self.live()
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
        // A chat its prompt never reached has nothing to go back to.
        !self.held && !self.base.is_empty() && Flavor::of(&self.base).resumes()
    }

    /// Its chat's id, as its command kept it. None until the command ran.
    pub fn session(&self) -> Option<String> {
        let id = std::fs::read_to_string(session_file(&self.prompt)).ok()?;
        Some(id.trim().to_owned()).filter(|id| !id.is_empty())
    }
}

/// Why a Build agent stopped short of its milestone, and since when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub why: String,
    pub at: u64,
}

/// How much of a transcript's end is read first for its model: most of a
/// long chat needn't be read to find its last answer.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

/// The model of the last answer in a Claude Code transcript's `text`. Lines
/// it wrote itself name `<synthetic>`, and subagents' their own.
fn last_claude_model(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .filter(|l| l.contains("\"model\""))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "assistant" && v["isSidechain"] != true)
        .find_map(|v| {
            let model = v["message"]["model"].as_str()?;
            (!model.is_empty() && model != "<synthetic>").then(|| model.to_owned())
        })
}

/// The model of Claude Code chat `id`, from its transcript under `projects`
/// (one folder per working directory). None without an answer yet.
fn claude_model(projects: &Path, id: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let name = format!("{id}.jsonl");
    let path = (std::fs::read_dir(projects).ok()?.flatten())
        .map(|dir| dir.path().join(&name))
        .find(|path| path.is_file())?;
    let mut file = std::fs::File::open(&path).ok()?;
    let from = file.metadata().ok()?.len().saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    // A line cut by the seek isn't JSON: it is passed over.
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    last_claude_model(&String::from_utf8_lossy(&tail)).or_else(|| {
        let whole = (from > 0).then(|| std::fs::read(&path).ok())??;
        last_claude_model(&String::from_utf8_lossy(&whole))
    })
}

/// `hex` as the text it spells.
fn from_hex(hex: &str) -> Option<String> {
    let bytes = hex.as_bytes().chunks(2).map(|pair| {
        let pair = std::str::from_utf8(pair).ok()?;
        u8::from_str_radix(pair, 16).ok()
    });
    String::from_utf8(bytes.collect::<Option<Vec<u8>>>()?).ok()
}

/// The model of Cursor CLI chat `id`, from its store under `chats` (one
/// folder per working directory). None for a chat that never answered.
fn cursor_model(chats: &Path, id: &str) -> Option<String> {
    let db = (std::fs::read_dir(chats).ok()?.flatten())
        .map(|dir| dir.path().join(id).join("store.db"))
        .find(|db| db.is_file())?;
    let read = || -> rusqlite::Result<String> {
        crate::cursor_agents::open_read_only(&db)?.query_row(
            "SELECT value FROM meta WHERE key = '0'",
            [],
            |row| row.get(0),
        )
    };
    // Asked again at every read of the feature while it runs, and its agent's
    // name shows meanwhile: not worth a toast, nor a line each time.
    let meta = read()
        .inspect_err(|err| tracing::debug!("reading {}: {err}", db.display()))
        .ok()?;
    let meta: serde_json::Value = serde_json::from_str(&from_hex(&meta)?).ok()?;
    let model = meta["lastUsedModel"].as_str()?;
    (!model.is_empty()).then(|| model.to_owned())
}

/// Where the agents keep their chats.
struct ChatDirs {
    claude: PathBuf,
    cursor: PathBuf,
}

impl ChatDirs {
    fn here() -> Option<Self> {
        let home = crate::home_dir()?;
        let claude = (std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".claude"));
        Some(Self {
            claude: claude.join("projects"),
            cursor: home.join(".cursor").join("chats"),
        })
    }
}

/// The models found for `feature`'s runs, by stage, with the run each read
/// was of. Reads files and databases, so not on the UI thread. A done stage
/// with its model kept isn't read again. A chat that can't be read has no
/// model and says nothing: it is asked again at every read of the feature,
/// and its agent's name shows in the model's place.
pub fn read_models(feature: &Feature) -> BTreeMap<Stage, (u32, String)> {
    ChatDirs::here().map_or_else(BTreeMap::new, |dirs| models_in(feature, &dirs))
}

fn models_in(feature: &Feature, dirs: &ChatDirs) -> BTreeMap<Stage, (u32, String)> {
    (feature.runs.iter())
        .filter(|(stage, run)| run.model.is_none() || !feature.is_done(**stage))
        .filter_map(|(stage, run)| {
            let model = match Flavor::of(&run.base) {
                Flavor::Claude => claude_model(&dirs.claude, &run.session()?),
                Flavor::Cursor => cursor_model(&dirs.cursor, &run.session()?),
                // Its thread names it when bound; the others can't be told
                // a chat id to look for.
                Flavor::T3 | Flavor::Codex | Flavor::Other => None,
            }?;
            Some((*stage, (run.n, model)))
        })
        .collect()
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

    /// A transcript line of an answer by `model`.
    fn answer(model: &str, sidechain: bool) -> String {
        format!(
            "{{\"type\":\"assistant\",\"isSidechain\":{sidechain},\"message\":{{\"model\":\"{model}\"}}}}\n"
        )
    }

    /// A folder of its own for test `name`, gone with the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let id = std::process::id();
            let dir = std::env::temp_dir().join(format!("gerdr-model-{name}-{id}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A Claude Code chat `id` of `text`, in a folder of a working directory.
    fn transcript(name: &str, id: &str, text: &str) -> Scratch {
        let dir = Scratch::new(name);
        let cwd = dir.path().join("-home-me-x");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join(format!("{id}.jsonl")), text).unwrap();
        dir
    }

    #[test]
    fn a_claude_chat_names_the_model_of_its_last_real_answer() {
        let text = [
            answer("claude-sonnet-5-5", false),
            answer("claude-opus-5-5", false),
            answer("claude-haiku-5-5", true),
            answer("<synthetic>", false),
            "{\"type\":\"user\",\"model\":\"nope\"}\n".to_owned(),
        ]
        .concat();
        let dir = transcript("claude", "c1", &text);
        assert_eq!(
            claude_model(dir.path(), "c1").as_deref(),
            Some("claude-opus-5-5")
        );
        assert_eq!(claude_model(dir.path(), "other"), None);
        assert_eq!(claude_model(&dir.path().join("none"), "c1"), None);
    }

    #[test]
    fn a_claude_answer_older_than_the_tail_is_still_found() {
        let filler = "{\"type\":\"user\"}\n".repeat(TRANSCRIPT_TAIL as usize / 8);
        let dir = transcript("tail", "c1", &(answer("claude-opus-5-5", false) + &filler));
        assert_eq!(
            claude_model(dir.path(), "c1").as_deref(),
            Some("claude-opus-5-5")
        );
    }

    /// A Cursor chat `id` whose first meta row is `meta`, as hex; none
    /// without one.
    fn cursor_chat(name: &str, id: &str, meta: Option<&str>) -> Scratch {
        let dir = Scratch::new(name);
        let chat = dir.path().join("hash-of-cwd").join(id);
        std::fs::create_dir_all(&chat).unwrap();
        let db = rusqlite::Connection::open(chat.join("store.db")).unwrap();
        db.execute("CREATE TABLE meta (key TEXT, value TEXT)", [])
            .unwrap();
        if let Some(meta) = meta {
            let hex: String = meta.bytes().map(|b| format!("{b:02x}")).collect();
            db.execute("INSERT INTO meta VALUES ('0', ?1)", [hex])
                .unwrap();
        }
        dir
    }

    #[test]
    fn a_cursor_chat_names_its_last_used_model() {
        let dir = cursor_chat("cursor", "k1", Some("{\"lastUsedModel\":\"grok-4.7\"}"));
        assert_eq!(cursor_model(dir.path(), "k1").as_deref(), Some("grok-4.7"));
        assert_eq!(cursor_model(dir.path(), "other"), None);
    }

    #[test]
    fn a_cursor_chat_without_a_model_names_none() {
        let empty = cursor_chat("empty", "k1", None);
        assert_eq!(cursor_model(empty.path(), "k1"), None);
        let unused = cursor_chat("unused", "k1", Some("{\"lastUsedModel\":\"\"}"));
        assert_eq!(cursor_model(unused.path(), "k1"), None);
        let broken = cursor_chat("broken", "k1", Some("not json"));
        assert_eq!(cursor_model(broken.path(), "k1"), None);
    }

    #[test]
    fn models_are_read_by_flavor_and_once_for_a_done_stage() {
        let chats = cursor_chat("chats", "k1", Some("{\"lastUsedModel\":\"grok-4.7\"}"));
        let data = Scratch::new("data");
        let prompt = data.path().join("plan-1.md");
        std::fs::write(session_file(&prompt), "k1\n").unwrap();
        let dirs = ChatDirs {
            claude: data.path().join("none"),
            cursor: chats.path().to_owned(),
        };
        let run = |base: &str, model: Option<&str>| StageRun {
            n: 3,
            base: base.to_owned(),
            model: model.map(str::to_owned),
            prompt: prompt.clone(),
            ..StageRun::default()
        };
        let mut f = crate::feature::tests::feature("a", 1);
        f.runs.insert(Stage::Plan, run("cursor-agent", None));
        f.runs.insert(Stage::Build, run("codex", None));
        let found = models_in(&f, &dirs);
        assert_eq!(found.get(&Stage::Plan), Some(&(3, "grok-4.7".to_owned())));
        assert_eq!(found.get(&Stage::Build), None);
        // One still running is read again: it may be switched.
        f.runs.insert(Stage::Plan, run("cursor-agent", Some("old")));
        assert_eq!(models_in(&f, &dirs).len(), 1);
        // Done, the model kept stands.
        let mark = crate::feature::Mark {
            at: 1,
            by: "you".to_owned(),
            note: String::new(),
            patch: None,
        };
        f.marks.insert(Stage::Plan, mark);
        assert!(models_in(&f, &dirs).is_empty());
    }

    #[test]
    fn a_run_saved_before_models_were_kept_still_loads() {
        let run: StageRun = serde_json::from_str(
            r#"{"n":1,"agent":"Cursor","command":"c","prompt":"/p.md","template":"t","started":1}"#,
        )
        .unwrap();
        assert_eq!(run.model, None);
    }

    #[test]
    fn a_chat_is_compacted_by_its_own_clis_line() {
        let compact = |command: &str| Flavor::of(command).compact().map(|l| l.split(' ').next());
        assert_eq!(compact("claude"), Some(Some("/compact")));
        assert_eq!(compact("codex"), Some(Some("/compact")));
        assert_eq!(compact("cursor-agent"), Some(Some("/summarize")));
        assert_eq!(compact("t3"), None);
        assert_eq!(compact("aider"), None);
    }

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
        let id = "'/d/my feature/prompts/plan-1.session'";
        assert_eq!(
            command_line("claude", prompt, &dirs),
            format!(
                "claude --add-dir '/d/my feature' \
                 --session-id \"$(tee {id} </proc/sys/kernel/random/uuid)\""
            )
        );
        assert_eq!(
            command_line("cursor-agent", prompt, &dirs),
            format!(
                "cursor-agent --trust --add-dir '/d/my feature' \
                 --resume \"$(cursor-agent create-chat | tee {id})\""
            )
        );
        assert_eq!(
            command_line("codex", prompt, &dirs),
            "codex --add-dir '/d/my feature'"
        );
        assert_eq!(command_line("aider --yes", prompt, &dirs), "aider --yes");
        assert_eq!(
            command_line("claude", Path::new("/p.md"), &[]),
            "claude --session-id \"$(tee /p.session </proc/sys/kernel/random/uuid)\""
        );
    }

    #[test]
    fn a_picked_model_goes_on_the_command_of_a_cli_that_takes_one() {
        assert_eq!(with_model("claude", Some("opus")), "claude --model opus");
        assert_eq!(
            with_model(" env A=1 cursor-agent ", Some("x[context=1m, fast=false]")),
            "env A=1 cursor-agent --model 'x[context=1m, fast=false]'"
        );
        assert_eq!(with_model("claude", None), "claude");
        assert_eq!(with_model("codex", Some("o3")), "codex");
        assert_eq!(with_model("aider --yes", Some("o3")), "aider --yes");
        // One in the `[agents]` command comes first, so the pick wins.
        assert_eq!(
            with_model("claude --model haiku", Some("opus")),
            "claude --model haiku --model opus"
        );
        // Who reads the command still knows the agent, and its chat's id
        // comes after the model, which a rerun keeps.
        let line = command_line(&with_model("claude", Some("opus")), Path::new("/p.md"), &[]);
        assert_eq!(Flavor::of(&line), Flavor::Claude);
        let argv: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(rerun_argv(&argv)[..3], ["claude", "--model", "opus"]);
    }

    #[test]
    fn a_resumed_chat_is_told_its_model_only_where_it_may_lose_it() {
        let run = |base: &str| StageRun {
            base: base.to_owned(),
            asked_model: Some("opus".into()),
            ..StageRun::default()
        };
        assert_eq!(resume_base(&run("claude")), "claude");
        assert_eq!(
            resume_base(&run("cursor-agent")),
            "cursor-agent --model opus"
        );
    }

    #[test]
    fn a_prompt_on_the_command_follows_dashes_only_for_a_cli_gerdr_knows() {
        let text = "Read /p.md and follow it.";
        let quoted = "'Read /p.md and follow it.'";
        assert_eq!(
            with_prompt("codex --add-dir /d", text),
            format!("codex --add-dir /d -- {quoted}")
        );
        assert_eq!(
            with_prompt("aider --yes", text),
            format!("aider --yes {quoted}")
        );
    }

    #[test]
    fn a_prompt_on_the_command_is_not_sent_again_by_a_rerun() {
        let line = with_prompt(
            "claude --model opus --session-id abc",
            "Read /p.md and follow it.",
        );
        assert_eq!(
            line,
            "claude --model opus --session-id abc -- 'Read /p.md and follow it.'"
        );
        assert_eq!(
            rerun_argv(&[
                "claude",
                "--model",
                "opus",
                "--session-id",
                "abc",
                "--",
                "Read /p.md"
            ]),
            ["claude", "--model", "opus", "--resume", "abc"]
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
    fn cursors_launcher_words_are_not_run_again() {
        let script = "/h/.local/share/cursor-agent/versions/1/index.js";
        assert_eq!(
            rerun_argv(&[
                "/h/bin/cursor-agent",
                "--use-system-ca",
                script,
                "--trust",
                "--add-dir",
                "/d",
                "--resume",
                "id",
            ]),
            [
                "/h/bin/cursor-agent",
                "--trust",
                "--add-dir",
                "/d",
                "--resume",
                "id"
            ]
        );
        // With no flag for node, by its other name, and with nothing told.
        assert_eq!(
            rerun_argv(&["cursor-agent", script, "--resume", "id", "--", "Do it"]),
            ["cursor-agent", "--resume", "id"]
        );
        assert_eq!(
            rerun_argv(&["/h/bin/agent", "--use-system-ca", script]),
            ["/h/bin/agent"]
        );
        // Someone else's script, or a flag's value: as it was.
        for argv in [
            &["node", "--use-system-ca", script][..],
            &["agent", "--use-system-ca", "/srv/app/index.js"],
            &["cursor-agent", "--trust", "--add-dir", "/d"],
        ] {
            assert_eq!(rerun_argv(argv), argv);
        }
    }

    #[test]
    fn a_saved_line_loses_cursors_launcher_words() {
        let line = "/h/bin/cursor-agent --use-system-ca '/h/my data/index.js' --add-dir '/d/a b' --resume id";
        assert_eq!(
            without_launcher(line).as_deref(),
            Some("/h/bin/cursor-agent --add-dir '/d/a b' --resume id")
        );
        for line in ["cursor-agent --trust --resume id", "npm run dev", ""] {
            assert_eq!(without_launcher(line), None);
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
    fn a_held_run_has_no_chat_to_go_back_to_and_says_if_it_can_be_sent() {
        let run = StageRun {
            base: "claude".into(),
            pane: Some("p1".into()),
            held: true,
            ..StageRun::default()
        };
        assert!(!run.resumes());
        assert!(run.waits_to_send());
        let closed = StageRun {
            pane: None,
            ..run.clone()
        };
        assert!(!closed.waits_to_send());
        let sent = StageRun { held: false, ..run };
        assert!(sent.resumes());
        assert!(!sent.waits_to_send());
    }

    #[test]
    fn a_run_saved_before_holding_was_already_sent() {
        let json = r#"{"n":1,"agent":"Claude Code","command":"claude","prompt":"/p.md","template":"builtin:plan@1","started":5}"#;
        let run: StageRun = serde_json::from_str(json).unwrap();
        assert!(!run.held);
        // And one sent saves as it was.
        assert!(!serde_json::to_string(&run).unwrap().contains("held"));
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
