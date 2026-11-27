//! The agents that run a feature's stages: which CLI, started how, and what
//! gerdr keeps of each run (`StageRun`).
//!
//! An agent stage (Discover, Plan, Build; later AI review) starts its
//! agent in a new tab of the feature's workspace, in the feature folder,
//! with the stage's prompt rendered to `<feature data dir>/prompts/<stage>-<n>.md`
//! (see `crate::prompts`). The agent is told to write its stage's file
//! (`brief.md`, `plan.md`; see `artifacts`) into the feature data dir,
//! which it is let into (`allowed_into`): the code's worktrees stay clean.
//!
//! The CLIs' flags, as checked on 2026-11-02:
//!
//! - `claude [--settings '{"permissions":{"additionalDirectories":[…]}}'] -- "<prompt>"`:
//!   interactive, prompt first.
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
use crate::integrations::{self, by_command};

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
    if let Some(words) = by_command(base).new_chat(base, &id_file) {
        line.push_str(&words);
    }
    line
}

/// `base` started on `model`, for a CLI that can be told one.
pub fn with_model(base: &str, model: Option<&str>) -> String {
    match model.filter(|_| by_command(base).takes_model()) {
        Some(model) => format!(
            "{} --model {}",
            base.trim(),
            crate::desktop::shell_quote(model)
        ),
        None => base.trim().to_owned(),
    }
}

/// `line` started on `effort`, for a CLI that can be told one.
pub fn with_effort(line: &str, effort: Option<&str>) -> String {
    let agent = by_command(line);
    match effort.filter(|e| agent.takes_effort() && agent.efforts().contains(e)) {
        Some(effort) => format!("{} --effort {effort}", line.trim()),
        None => line.trim().to_owned(),
    }
}

/// The config file's `[effort]`: the effort a stage starts on with none
/// picked on its page, in a feature that takes gerdr's
/// (`Feature::effort_defaults`). An empty one leaves it to the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortDefaults {
    /// Each stage's, where it has its own.
    pub stages: BTreeMap<Stage, String>,
    /// That of a stage without its own.
    pub other: String,
    /// Build's in a fix round, which must get right what went wrong.
    pub fix: String,
}

impl Default for EffortDefaults {
    fn default() -> Self {
        Self {
            stages: BTreeMap::from([
                (Stage::PlanReview, "high".to_owned()),
                (Stage::AiReview, "high".to_owned()),
            ]),
            other: "medium".to_owned(),
            fix: "high".to_owned(),
        }
    }
}

impl EffortDefaults {
    /// `stage`'s, Build's while `fixing` the fix round's.
    pub fn of(&self, stage: Stage, fixing: bool) -> &str {
        match stage {
            Stage::Build if fixing => &self.fix,
            _ => self.stages.get(&stage).unwrap_or(&self.other),
        }
    }
}

static EFFORT_DEFAULTS: std::sync::RwLock<Option<EffortDefaults>> = std::sync::RwLock::new(None);

/// Use `defaults` for every `effort_defaults()` after, as the config file says.
pub fn set_effort_defaults(defaults: EffortDefaults) {
    if let Ok(mut set) = EFFORT_DEFAULTS.write() {
        *set = Some(defaults);
    }
}

/// The efforts stages start on with none picked, as the config file last said.
pub fn effort_defaults() -> EffortDefaults {
    let set = EFFORT_DEFAULTS.read().ok().and_then(|set| set.clone());
    set.unwrap_or_default()
}

/// The command `line` told `text` as it starts: it is its first message, with
/// no waiting for the CLI to be ready. A CLI gerdr knows gets `--` before it,
/// so `--add-dir` can't take it for a folder; another gets it as its last
/// argument and nothing else.
pub fn with_prompt(line: &str, text: &str) -> String {
    let text = crate::desktop::shell_quote(text);
    match by_command(line).adds_dirs() {
        true => format!("{line} -- {text}"),
        false => format!("{line} {text}"),
    }
}

/// What `run`'s chat is resumed with: told its model again by an agent
/// that `retells_model`, as it was, and its effort, which lasts a session.
pub fn resume_base(run: &StageRun) -> String {
    let base = match by_command(&run.base).retells_model() {
        true => with_model(&run.base, run.asked_model.as_deref()),
        false => run.base.clone(),
    };
    with_effort(&base, run.asked_effort.as_deref())
}

/// The shell command that goes back to chat `session` of `base`, in the
/// folder it ran in, allowed into `dirs` too.
pub fn resume_line(base: &str, session: &str, dirs: &[PathBuf]) -> String {
    let id = crate::desktop::shell_quote(session);
    format!("{} --resume {id}", unattended(base, dirs))
}

/// Where the script an agent's launcher runs is in `argv`, what its process
/// shows (`Integration::launcher_script`).
fn launcher_script(argv: &[&str]) -> Option<usize> {
    (integrations::ALL.iter()).find_map(|agent| agent.launcher_script(argv))
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
/// used, and the prompt would be sent a second time. One runs without what
/// its launcher adds (see `launcher_script`).
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
    if !argv.first().is_some_and(|a| by_command(a).resumes()) {
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
    if by_command(base).asks_trust() {
        allowed_into(&format!("{} --trust", base.trim()), dirs)
    } else {
        allowed_into(base, dirs)
    }
}

/// The shell command that starts `base` allowed into `dirs` too, for a CLI
/// gerdr knows the flag of (`Integration::add_dirs`): the others reach the
/// feature's files through `layout::DOCS_LINK`.
pub fn allowed_into(base: &str, dirs: &[PathBuf]) -> String {
    let line = base.trim().to_owned();
    let agent = by_command(base);
    match agent.adds_dirs() && !dirs.is_empty() {
        true => agent.add_dirs(line, dirs),
        false => line,
    }
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

/// An agent the user started for the feature with its AI button: told what
/// the feature is, then theirs to direct. Kept so the feature's page shows it
/// while its tab is there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnRun {
    /// Which of the feature's own agents: 1, 2, …
    pub n: u32,
    /// The agent's name, as the picker shows it ("Claude Code").
    pub agent: String,
    /// The herdr pane it runs in, while its tab is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// Unix seconds.
    pub started: u64,
    /// The built-in prompt it was started on for a task ("retro"); None:
    /// the user's to direct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// A question Discover's agent asked to have researched (`research.md`),
/// and the agent gerdr started on it: it writes its answer to
/// `artifacts::research_path`, and is closed once it did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchRun {
    /// Its number: R1, R2, …
    pub id: u32,
    pub question: String,
    /// Why Discover's agent asks, if it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The agent's name, as the picker shows it; empty until one starts.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// The herdr pane it runs in, while its tab is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// When its agent started, unix seconds; none while it waits its turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
    /// When its answer was found written and its agent done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
    /// Why it stopped without an answer: its tab closed, or no agent could
    /// start. Run again clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

impl ResearchRun {
    /// Whether its agent is at work on it, or about to be.
    pub fn running(&self) -> bool {
        self.started.is_some() && self.done.is_none() && self.stopped.is_none()
    }

    /// Whether it waits for a turn to start.
    pub fn waits(&self) -> bool {
        self.started.is_none() && self.done.is_none() && self.stopped.is_none()
    }
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
    /// The effort it was picked to start on, if one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_effort: Option<String>,
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
    /// The template it was rendered from, and the skills it named with
    /// their folder hashes: `builtin:plan@1 skills:tdd@667dbdd`.
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
    /// How full its context was when last read: once its agent is gone,
    /// how full it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<crate::ai_work::context::Context>,
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

    /// Whether its prompt waits for the user to send it: held in its tab,
    /// or a draft in T3 Code. Its work starts once it goes.
    pub fn unsent(&self) -> bool {
        self.held || self.t3 == Some(T3Run::Draft)
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
        !self.held && !self.base.is_empty() && by_command(&self.base).resumes()
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

/// The models found for `feature`'s runs, by stage, with the run each read
/// was of. Reads files and databases, so not on the UI thread. A done stage
/// with its model kept isn't read again. A chat that can't be read has no
/// model and says nothing: it is asked again at every read of the feature,
/// and its agent's name shows in the model's place.
pub fn read_models(feature: &Feature) -> BTreeMap<Stage, (u32, String)> {
    crate::ai_work::locate::Homes::here()
        .map_or_else(BTreeMap::new, |homes| models_in(feature, &homes))
}

fn models_in(
    feature: &Feature,
    homes: &crate::ai_work::locate::Homes,
) -> BTreeMap<Stage, (u32, String)> {
    (feature.runs.iter())
        .filter(|(stage, run)| run.model.is_none() || !feature.is_done(**stage))
        .filter_map(|(stage, run)| {
            // T3 Code's thread names it when bound; the others can't be
            // told a chat id to look for.
            let agent = by_command(&run.base);
            let model = agent
                .resumes()
                .then(|| agent.run_model(homes, &run.session()?))??;
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

/// What one of Build's milestones took, read when the build log was first
/// seen with it ticked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneTokens {
    /// The milestone, as the build log names it.
    pub name: String,
    /// The Build run that ticked it.
    pub run: u32,
    /// How full that agent's context was then.
    pub context: crate::ai_work::context::Context,
    /// How much its context grew for it: since the run's last milestone, or
    /// since the run started. None when more than one was first seen ticked
    /// at once, which can't be told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn models_are_read_by_agent_and_once_for_a_done_stage() {
        let home = Scratch::new("home");
        let homes = crate::ai_work::locate::Homes::under(home.path());
        let meta = Some("{\"lastUsedModel\":\"grok-4.7\"}");
        crate::integrations::cursor::launch::tests::chat(&homes.cursor.join("chats"), "k1", meta);
        let data = Scratch::new("data");
        let prompt = data.path().join("plan-1.md");
        std::fs::write(session_file(&prompt), "k1\n").unwrap();
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
        let found = models_in(&f, &homes);
        assert_eq!(found.get(&Stage::Plan), Some(&(3, "grok-4.7".to_owned())));
        assert_eq!(found.get(&Stage::Build), None);
        // One still running is read again: it may be switched.
        f.runs.insert(Stage::Plan, run("cursor-agent", Some("old")));
        assert_eq!(models_in(&f, &homes).len(), 1);
        // Done, the model kept stands.
        let mark = crate::feature::Mark {
            at: 1,
            by: "you".to_owned(),
            note: String::new(),
            patch: None,
        };
        f.marks.insert(Stage::Plan, mark);
        assert!(models_in(&f, &homes).is_empty());
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
    fn agents_go_by_program_name() {
        use crate::ai_work::AgentKind;
        let kind = |command: &str| by_command(command).kind();
        assert_eq!(kind("claude"), AgentKind::Claude);
        assert_eq!(kind("/usr/bin/claude --model opus"), AgentKind::Claude);
        assert_eq!(kind("env FOO=1 codex"), AgentKind::Codex);
        assert_eq!(kind("cursor-agent"), AgentKind::Cursor);
        assert_eq!(kind("t3"), AgentKind::T3);
        let t3 = by_command("t3");
        assert!(!t3.adds_dirs());
        assert!(!t3.resumes());
        assert!(t3.picks_model() && !t3.takes_model());
        assert_eq!(kind("aider"), AgentKind::Unknown);
        assert_eq!(kind(""), AgentKind::Unknown);
        assert!(by_command("cursor-agent").adds_dirs());
        assert!(!by_command("aider").adds_dirs());
    }

    #[test]
    fn a_t3_draft_is_unsent_until_its_thread_runs() {
        let mut run = StageRun {
            t3: Some(T3Run::Draft),
            ..StageRun::default()
        };
        assert!(run.unsent() && !run.held);
        run.t3 = Some(T3Run::Thread {
            id: "t".into(),
            agent: None,
        });
        assert!(!run.unsent());
        run.t3 = None;
        run.held = true;
        assert!(run.unsent());
    }

    #[test]
    fn command_lines_per_agent() {
        let prompt = Path::new("/d/my feature/prompts/plan-1.md");
        let dirs = [PathBuf::from("/d/my feature")];
        let id = "'/d/my feature/prompts/plan-1.session'";
        assert_eq!(
            command_line("claude", prompt, &dirs),
            format!(
                "claude --settings '{{\"permissions\":{{\"additionalDirectories\":[\"/d/my feature\"]}}}}' \
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
        assert_eq!(by_command(&line).kind(), crate::ai_work::AgentKind::Claude);
        let argv: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(rerun_argv(&argv)[..3], ["claude", "--model", "opus"]);
    }

    #[test]
    fn a_picked_effort_goes_on_the_command_of_a_cli_that_takes_one() {
        let line = with_effort(&with_model("claude", Some("opus")), Some("high"));
        assert_eq!(line, "claude --model opus --effort high");
        assert_eq!(with_effort("claude", None), "claude");
        // A level the CLI doesn't offer, or a CLI that takes none, gets none.
        assert_eq!(with_effort("claude", Some("huge")), "claude");
        assert_eq!(with_effort("cursor-agent", Some("high")), "cursor-agent");
        assert_eq!(with_effort("codex", Some("high")), "codex");
        // A rerun keeps it, as it keeps the model.
        let line = command_line(&line, Path::new("/p.md"), &[]);
        let argv: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            rerun_argv(&argv)[..5],
            ["claude", "--model", "opus", "--effort", "high"]
        );
        // A resumed chat is told it again: it lasts a session.
        let run = StageRun {
            base: "claude".into(),
            asked_effort: Some("max".into()),
            ..StageRun::default()
        };
        assert_eq!(resume_base(&run), "claude --effort max");
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
        assert_eq!(by_command(&line).kind(), crate::ai_work::AgentKind::Cursor);
        assert!(line.contains("$(/bin/cursor-agent create-chat | tee /p.session)"));
    }

    #[test]
    fn a_closed_agent_goes_back_to_its_chat_with_the_same_trust_and_dirs() {
        let dirs = [PathBuf::from("/d/my feature")];
        assert_eq!(
            resume_line("claude --model opus", "abc-1", &dirs),
            "claude --model opus --settings '{\"permissions\":{\"additionalDirectories\":[\"/d/my feature\"]}}' \
             --resume abc-1"
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
        for base in ["codex", "cursor-agent"] {
            assert_eq!(
                allowed_into(base, &dirs),
                format!("{base} --add-dir '/d/my feature' --add-dir /e")
            );
        }
        // Claude gets them by settings: --add-dir would ask to trust each.
        let claude = allowed_into("claude", &dirs);
        assert_eq!(
            claude,
            "claude --settings '{\"permissions\":{\"additionalDirectories\":[\"/d/my feature\",\"/e\"]}}'"
        );
        assert_eq!(
            by_command(&claude).kind(),
            crate::ai_work::AgentKind::Claude
        );
        assert_eq!(allowed_into(&claude, &[]), claude);
        // One with settings of its own keeps --add-dir, told to read their
        // CLAUDE.md, once.
        let own = allowed_into("claude --settings my.json", &dirs);
        assert_eq!(
            own,
            "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 claude --settings my.json \
             --add-dir '/d/my feature' --add-dir /e"
        );
        assert_eq!(allowed_into(&own, &dirs).matches("CLAUDE_MD=").count(), 1);
        for base in ["opencode", "aider --yes", "agy"] {
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
