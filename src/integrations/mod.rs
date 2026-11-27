//! What gerdr knows of each agent CLI, behind one trait: where its chats
//! are, how its screen shows its mode and model, its commands, its live
//! hooks. Each agent has its own `impl Integration`; `of` gives the one of
//! a kind. A method's default means the agent doesn't have that.

use std::{
    borrow::Cow,
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use self::common::{build::Builder, files::SubFiles, install::Changed};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::{Context, tail_of},
        locate::{self, Found, Homes, Span},
        usage::Message,
    },
    chat::{Source, commands::Command, menu::Menu, parse::Reader},
    herdr::Herdr,
    scrollback::Styled,
    stage_models::Offer,
};

pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub mod common;
pub(crate) mod cursor;
pub(crate) mod opencode;
mod pi;
mod plain;
pub(crate) mod t3;

/// Where an agent keeps a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// A transcript of lines that only grow.
    Lines,
    /// Rows of a database, read whole.
    Database,
}

/// How a chat switches its agent's model, or its effort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSwitch {
    /// It can't.
    None,
    /// gerdr types `/model <id>` (`/effort <level>`).
    Typed,
    /// A line of the agent's opens a menu of its own, mirrored in the chat.
    OwnMenu,
}

impl ModelSwitch {
    /// gerdr types the line that switches it.
    pub fn typed(self) -> bool {
        matches!(self, Self::Typed)
    }
}

/// What a model is picked for, as a picker of it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickedFor {
    /// A feature stage's next start.
    Stage,
    /// An agent started now.
    Start,
    /// A chat's agent, switched as it runs.
    Chat,
}

/// An agent's `/btw` side panel as its screen draws it: the marks
/// `chat::btw` knows it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidePanel {
    /// What its footer says, on its first row: where the panel ends.
    pub footers: &'static [&'static str],
    /// The marks of its frame, left of its rows.
    pub frame: &'static str,
    /// What its question row ends with, past the question.
    pub question_end: Option<&'static str>,
    /// The marks at the end of the rows past which it scrolls.
    pub scroll_marks: &'static [char],
    /// It spins a braille spinner while it answers.
    pub braille: bool,
    /// The marks it spins before a word that goes on (`✽ Answering…`),
    /// which a bullet doesn't.
    pub stars: &'static str,
    /// What its footer says when the answer is cut, and the keys that show
    /// the rest.
    pub expand: Option<(&'static str, &'static [&'static str])>,
}

/// What a chat of an agent offers, all in one: what the UI reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// gerdr reads its chats.
    pub chat: bool,
    /// Earlier messages are read on ask (the chat isn't read whole).
    pub earlier: bool,
    /// It has modes to step through.
    pub modes: bool,
    pub model: ModelSwitch,
    /// How its reasoning effort is switched.
    pub effort: ModelSwitch,
    /// It takes side questions (`/btw`).
    pub side: bool,
    /// Its screen suggests what to say next.
    pub suggests: bool,
    /// Its chat says how full its context is.
    pub context: bool,
}

/// One agent CLI, as gerdr knows it.
pub trait Integration: Sync {
    fn kind(&self) -> AgentKind;
    /// How it is called here: "Claude Code".
    fn label(&self) -> &'static str;
    /// herdr's name for its integration; none for an agent herdr doesn't
    /// know.
    fn herdr_target(&self) -> Option<&'static str> {
        Some(self.kind().name())
    }

    // Finding its chats.

    /// Where it keeps a chat; none: gerdr doesn't read its chats.
    fn store(&self) -> Option<Store> {
        None
    }
    /// Whether its chat in a pane can be found by folder and start time
    /// when herdr names none.
    fn guesses(&self) -> bool {
        false
    }
    /// Whether a run's chat whose id wasn't kept is still found, by folder
    /// and time.
    fn backfill_by_time(&self) -> bool {
        false
    }
    /// `chat` among `homes` (`locate::find_in`): by `id` (plain), else by
    /// folder and time in `span`, not one of `claimed`. An error: a
    /// database that couldn't be read.
    fn find(
        &self,
        _homes: &Homes,
        _chat: &Chat,
        _id: Option<&str>,
        _claimed: &HashSet<String>,
        _span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(None)
    }
    /// The chat of an agent in a pane, as `find`; some agents are found by
    /// time only there.
    fn find_pane(
        &self,
        homes: &Homes,
        chat: &Chat,
        claimed: &HashSet<String>,
        span: Span,
    ) -> Result<Option<Found>, String> {
        locate::find_in(homes, chat, claimed, span)
    }
    /// Herdr's path to its transcript, `path`, as it really is, if it is
    /// one of its own.
    fn transcript_at(&self, _path: &str, _homes: &Homes) -> Option<PathBuf> {
        None
    }
    /// The folders where its transcripts are written: a path named there
    /// that isn't yet is a chat not written yet.
    fn transcripts_under(&self, _homes: &Homes) -> Vec<PathBuf> {
        Vec::new()
    }
    /// The chat id in its transcript's `path`, whose file stem is `stem`.
    fn id_in_path<'a>(&self, stem: &'a str, _path: &'a Path) -> &'a str {
        stem
    }
    /// The shell lines that list its chat `chat` (`id`, plain) on another
    /// machine (`locate::script`); none for what isn't kept from there.
    fn remote_script(&self, _chat: &Chat, _id: Option<&str>) -> Option<String> {
        None
    }
    /// Whether its script there lists chats to pick one by time from.
    fn remote_guesses(&self) -> bool {
        false
    }
    /// Whether its chat pulled from there is a `store.db`, not a
    /// transcript.
    fn staged_store(&self) -> bool {
        false
    }

    // Reading its chats.

    /// A reader of its chat's lines or rows, with what it keeps of its own.
    fn parser(&self) -> Box<dyn Parser> {
        Box::new(plain::Plain)
    }
    /// What of its database chat was read, to read what changed since;
    /// none for a chat that isn't in a database.
    fn db(&self) -> Option<Box<dyn DbChat>> {
        None
    }
    /// Feed `reader` what the subagents of chat `chat` wrote since the
    /// last read, in files of their own (`files`, read from their last
    /// `chunk` bytes first).
    fn subagents(
        &self,
        _chat: &Source,
        _reader: &mut Reader,
        _files: &mut SubFiles,
        _chunk: u64,
    ) -> std::io::Result<()> {
        Ok(())
    }
    /// The model its chat is on, as `reader` read it or its CLI says.
    fn model_in_chat(&self, _reader: &Reader) -> Option<String> {
        None
    }
    /// The model its chat is on with no reader: by the end of its
    /// transcript `chat`, or as its CLI says.
    fn model_now(&self, _chat: Option<&Path>) -> Option<String> {
        None
    }
    /// The effort its chat is on, as `reader` read it.
    fn effort_in_chat(&self, reader: &Reader) -> Option<String> {
        reader.effort()
    }
    /// The effort its chat is on with no reader, by the end of its
    /// transcript `chat`.
    fn effort_now(&self, _chat: Option<&Path>) -> Option<String> {
        None
    }
    /// The transcript of its chat in herdr pane `pane`, read for the
    /// pane's words when herdr keeps it no scrollback (the alternate
    /// screen).
    fn pane_transcript(&self, _herdr: &mut Herdr, _pane: &str) -> Option<PathBuf> {
        None
    }
    /// Whether its chats say how full its context is.
    fn has_context(&self) -> bool {
        false
    }
    /// The context of its chat whose transcript ends with `tail`.
    fn context_in(&self, _tail: &[u8]) -> Option<Context> {
        None
    }
    /// The context of its chat kept at `path`. Reads the file.
    fn context(&self, path: &Path) -> Option<Context> {
        self.context_in(&tail_of(path)?)
    }
    /// The messages of its transcript `raw`, for their tokens.
    fn usage(&self, _raw: &[u8]) -> Vec<Message> {
        Vec::new()
    }

    // Its screen.

    /// The modes it steps through on Shift+Tab, in its order.
    fn modes(&self) -> &'static [&'static str] {
        &[]
    }
    /// The mode its screen shows; `rows` are its last ones that aren't
    /// empty, trimmed, last first.
    fn mode_shown(&self, _rows: &[&str]) -> Option<String> {
        None
    }
    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::None
    }
    /// The models a chat or a picker offers; `listed`: those its CLI or
    /// app listed, as last read.
    fn offers(&self, _listed: &[Offer]) -> Vec<Offer> {
        Vec::new()
    }
    /// The row a chat offers first, to put it back on its own choice of
    /// model; not one of `offers`, picked by none.
    fn own_model(&self) -> Option<Offer> {
        None
    }
    /// Whether the models it offers are read from its CLI (`offers`' list).
    fn models_read(&self) -> bool {
        false
    }
    fn effort_switch(&self) -> ModelSwitch {
        ModelSwitch::None
    }
    /// The efforts a chat offers, for one switched by a typed line.
    fn efforts(&self) -> &'static [&'static str] {
        &[]
    }
    /// The line that opens its own effort menu, and whether that is its
    /// model menu, the effort its second step (Enter on the current model).
    fn effort_line(&self) -> Option<(&'static str, bool)> {
        None
    }
    /// The effort its screen says it is on; `rows` as `model_on_screen`.
    fn effort_on_screen<'a>(&self, _rows: &[&'a str]) -> Option<&'a str> {
        None
    }
    /// Its own model or effort menu, as its screen `rows` show it at their
    /// bottom.
    fn model_menu(&self, _rows: &[(String, Styled)]) -> Option<Menu> {
        None
    }
    /// The model its screen says it is on; `rows` its last ones that aren't
    /// empty, trimmed, last first.
    fn model_on_screen<'a>(&self, _rows: &[&'a str]) -> Option<&'a str> {
        None
    }
    /// The menu it waits on under a plan, where Enter builds it: its
    /// question, its choice to keep planning, the key for that choice.
    fn plan_menu(&self) -> Option<(&'static str, &'static str, &'static str)> {
        None
    }
    /// Whether its screen suggests what to say next.
    fn suggests(&self) -> bool {
        false
    }
    /// What its screen `rows` suggest saying next.
    fn suggestion(&self, _rows: &[(String, Styled)]) -> Option<String> {
        None
    }
    /// Its `/btw` side panel, as its screen draws it; none: it takes no
    /// side questions.
    fn side_panel(&self) -> Option<SidePanel> {
        None
    }

    // Its commands.

    /// Its built-in commands: name, what it does.
    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
    /// Its commands on disk, for folder `cwd`, into `found`.
    fn disk_commands(
        &self,
        _cwd: &Path,
        _homes: &Homes,
        _config: &Path,
        _found: &mut Vec<Command>,
    ) {
    }

    // Live hooks.

    /// The subcommand its hooks run gerdr with.
    fn hook_command(&self) -> Option<&'static str> {
        None
    }
    /// Run as its hook: read the event, send it on, and end.
    fn run_hook(&self) {}
    /// Add gerdr's hooks to it under home folder `home`, if it is
    /// installed: each file changed, or why not.
    fn install_live(&self, _home: &Path) -> Vec<Result<Option<Changed>, String>> {
        Vec::new()
    }

    // Starting it (`feature::agents`, `cli_agents`).

    /// The program that starts it, which is also its `[agents]` key
    /// (`claude`); none for one gerdr doesn't start.
    fn command(&self) -> Option<&'static str> {
        None
    }
    /// Whether it starts in its own app rather than in a terminal: its
    /// threads are drafts there (T3 Code).
    fn in_app(&self) -> bool {
        false
    }
    /// Whether it asks to trust a new folder unless told `--trust`.
    fn asks_trust(&self) -> bool {
        false
    }
    /// Whether it takes `--add-dir`, and its first message after `--`: it
    /// can be told what to do as it starts. One that doesn't gets a message
    /// as its last argument and nothing else.
    fn adds_dirs(&self) -> bool {
        false
    }
    /// Whether it is told, besides a feature's folders, each repository of
    /// a folder that holds several and the main checkouts of the others
    /// (`Feature::agent_dirs_for`): one that sees a root's git branch and
    /// status only in a root that is a repository, and doesn't search roots
    /// nested in another.
    fn adds_roots(&self) -> bool {
        false
    }
    /// Its command `line` allowed into `dirs`, none of them empty, for one
    /// that `adds_dirs`.
    fn add_dirs(&self, line: String, dirs: &[PathBuf]) -> String {
        common::launch::add_dirs(line, dirs)
    }
    /// Whether gerdr can start it on a model, with `--model` (a second one
    /// wins over one in the `[agents]` command).
    fn takes_model(&self) -> bool {
        false
    }
    /// Whether a stage's model can be picked for it.
    fn picks_model(&self) -> bool {
        self.takes_model()
    }
    /// Whether a feature's stages are offered to it: one gerdr can go back
    /// to the chat of, or that starts in its app.
    fn runs_stages(&self) -> bool {
        self.resumes() || self.in_app()
    }
    /// Whether its chat, gone back to, is told its model again: one that
    /// isn't keeps its own.
    fn retells_model(&self) -> bool {
        false
    }
    /// Whether gerdr can know its chat's id, to go back to the chat with
    /// `--resume <id>`.
    fn resumes(&self) -> bool {
        false
    }
    /// What its command `base` gets to keep its new chat's id in file
    /// `id_file` (shell-quoted), for one that `resumes`.
    fn new_chat(&self, _base: &str, _id_file: &str) -> Option<String> {
        None
    }
    /// Where the script its launcher runs is in `argv`, what its process
    /// shows, when the launcher's own words come before what it was told.
    fn launcher_script(&self, _argv: &[&str]) -> Option<usize> {
        None
    }
    /// Whether `command`, whose program is called `name` (no `.exe`), runs
    /// it under another name than `command`'s.
    fn runs_as(&self, _name: &str, _command: &str) -> bool {
        false
    }
    /// The model its chat `id` ran on, as kept under `homes`. Reads files.
    fn run_model(&self, _homes: &Homes, _id: &str) -> Option<String> {
        None
    }
    /// Read the models its CLI `base` lists. Blocks: run off the event
    /// loop, for one whose models are read (`models_read`).
    fn list_models(&self, _base: &str) -> Result<Vec<Offer>, String> {
        Err("it lists none".to_owned())
    }
    /// What a picker of its model says while it reads them, with none read
    /// yet; `failed`: the last read failed.
    fn reading_hint(&self, failed: bool) -> &'static str {
        match failed {
            false => "Reading its models…",
            true => "Couldn't read its models: type a model's id…",
        }
    }
    /// What a picker of its model says over its list, for a model picked
    /// for `of`; none: the picker's own words.
    fn model_hint(&self, _of: PickedFor) -> Option<&'static str> {
        None
    }

    /// What its chat offers, from the methods above.
    fn caps(&self) -> Caps {
        Caps {
            chat: self.store().is_some(),
            earlier: self.store() == Some(Store::Lines),
            modes: !self.modes().is_empty(),
            model: self.model_switch(),
            effort: self.effort_switch(),
            side: self.side_panel().is_some(),
            suggests: self.suggests(),
            context: self.has_context(),
        }
    }
}

/// What an agent's chat reader keeps of its own; what all keep is in
/// `Builder`. Each method's default reads nothing.
pub trait Parser: Send {
    /// Read transcript line `line`.
    fn line(&mut self, _b: &mut Builder, _line: &Value) {}
    /// Read a database chat's rows, all of them in order.
    fn rows(&mut self, _b: &mut Builder, _rows: &mut dyn Iterator<Item = &Value>) {}
    /// Read line `line` of a subagent, in the subagent's own reader.
    fn sub_line(&mut self, _b: &mut Builder, _line: &Value) {}
    /// Whole lines of subagent `agent`'s own transcript; `call` started it.
    fn subagent(&mut self, _b: &mut Builder, _agent: &str, _call: &str, _lines: &[u8]) {}
    /// The model the chat is on, as its lines read so far last said.
    fn model(&self) -> Option<String> {
        None
    }
    /// The effort the chat is on, as its lines read so far last said.
    fn effort(&self) -> Option<String> {
        None
    }
    /// The conversations results named as subagents that aren't taken as
    /// such yet, in order.
    fn named_subagents(&self, _b: &Builder) -> Vec<String> {
        Vec::new()
    }
    /// How many subagents asked for have no conversation yet, of calls
    /// whose result came and named none.
    fn unfound_subagents(&self) -> usize {
        0
    }
    /// The conversations taken as subagents.
    fn subagents(&self) -> Vec<String> {
        Vec::new()
    }
    /// Take conversation `child` as a subagent, its transcript's first line
    /// `first`; whether it was taken.
    fn take_subagent(&mut self, _b: &mut Builder, _child: &str, _first: &str) -> bool {
        false
    }
}

impl Parser for plain::Plain {}

/// What of a database chat was read: read by what changed since.
pub trait DbChat: Send {
    /// Read what changed in database `conn` of chat `id` since the last
    /// read: rows new or written over, and rows gone.
    fn read(&mut self, id: &str, conn: &rusqlite::Connection) -> Result<(), String>;
    /// The rows, as the agent's `Parser::rows` takes them.
    fn rows(&self) -> Vec<Cow<'_, Value>>;
}

/// Every agent, in the order the launcher offers them (`cli_agents`).
pub const ALL: [&dyn Integration; 8] = [
    &claude::Claude,
    &t3::T3,
    &codex::Codex,
    &cursor::Cursor,
    &opencode::OpenCode,
    &pi::Pi,
    &antigravity::Antigravity,
    &plain::Plain,
];

/// The agents a chat of no known agent is looked for as by its id, in
/// order: the names of its files don't say whose it is.
pub const BY_ID: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Cursor, AgentKind::Codex];

/// What a chat of agent `kind` offers.
pub fn caps(kind: AgentKind) -> Caps {
    of(kind).caps()
}

/// The integration of agent `kind`.
pub fn of(kind: AgentKind) -> &'static dyn Integration {
    match kind {
        AgentKind::Claude => &claude::Claude,
        AgentKind::Cursor => &cursor::Cursor,
        AgentKind::Codex => &codex::Codex,
        AgentKind::OpenCode => &opencode::OpenCode,
        AgentKind::T3 => &t3::T3,
        AgentKind::Pi => &pi::Pi,
        AgentKind::Antigravity => &antigravity::Antigravity,
        AgentKind::Unknown => &plain::Plain,
    }
}

/// The agent `command` starts, by its program's name (`env`, `exec` and
/// `VAR=value` before it skipped); `Plain` for one gerdr doesn't know.
pub fn by_command(command: &str) -> &'static dyn Integration {
    let program = crate::feature::agents::program(command);
    let name = program.rsplit('/').next().unwrap_or(program);
    (ALL.iter().copied())
        .find(|agent| agent.command() == Some(name))
        .unwrap_or(&plain::Plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_its_own() {
        for (i, integration) in ALL.iter().enumerate() {
            assert_eq!(of(integration.kind()).kind(), integration.kind());
            assert!(ALL[..i].iter().all(|o| o.kind() != integration.kind()));
        }
    }

    #[test]
    fn caps_say_what_each_chat_offers() {
        let caps = |kind| of(kind).caps();
        let claude = caps(AgentKind::Claude);
        assert!(claude.chat && claude.side && claude.suggests && claude.context);
        assert!(matches!(claude.model, ModelSwitch::Typed));
        assert_eq!(caps(AgentKind::Codex).model, ModelSwitch::OwnMenu);
        assert!(!caps(AgentKind::Codex).side && !caps(AgentKind::Pi).modes);
        assert!(!caps(AgentKind::OpenCode).earlier && caps(AgentKind::OpenCode).side);
        for kind in [AgentKind::T3, AgentKind::Unknown] {
            let none = caps(kind);
            assert!(!none.chat && !none.modes && !none.side && !none.context);
            assert_eq!(none.model, ModelSwitch::None);
        }
        // A side panel for each agent whose `/` menu offers `/btw`.
        for agent in ALL {
            let btw = agent.builtins().iter().any(|(name, _)| *name == "btw");
            assert_eq!(agent.caps().side, btw, "{:?}", agent.kind());
        }
    }

    /// The agents with a chat, as `docs/chat-agents.md` names them, in its
    /// order.
    const CHATS: [(AgentKind, &str); 6] = [
        (AgentKind::Claude, "Claude"),
        (AgentKind::Codex, "Codex"),
        (AgentKind::OpenCode, "OpenCode"),
        (AgentKind::Cursor, "Cursor"),
        (AgentKind::Pi, "pi"),
        (AgentKind::Antigravity, "agy"),
    ];

    /// Each field of `caps` as the doc's table says it: a new field won't
    /// build here until it has its row.
    fn caps_rows(caps: Caps) -> [(&'static str, &'static str); 8] {
        let Caps {
            chat,
            earlier,
            modes,
            model,
            effort,
            side,
            suggests,
            context,
        } = caps;
        let yes = |on: bool| if on { "✓" } else { "—" };
        let switch = |by: ModelSwitch| match by {
            ModelSwitch::None => "—",
            ModelSwitch::Typed => "typed",
            ModelSwitch::OwnMenu => "own menu",
        };
        [
            ("`chat`: gerdr reads its chats", yes(chat)),
            ("`earlier`: earlier messages on ask", yes(earlier)),
            ("`modes`: mode switcher", yes(modes)),
            ("`model`: model picker", switch(model)),
            ("`effort`: effort picker", switch(effort)),
            ("`side`: `/btw` side panel", yes(side)),
            ("`suggests`: prompt suggestion", yes(suggests)),
            ("`context`: context size", yes(context)),
        ]
    }

    #[test]
    fn the_chat_doc_says_what_caps_say() {
        const DOC: &str = include_str!("../../docs/chat-agents.md");
        let mut table = String::from("| `Caps` |");
        for (_, name) in CHATS {
            table += &format!(" {name} |");
        }
        table += &format!("\n|---|{}\n", "---|".repeat(CHATS.len()));
        let rows: Vec<_> = CHATS.map(|(kind, _)| caps_rows(caps(kind))).into();
        for (i, (field, _)) in rows[0].iter().enumerate() {
            table += &format!("| {field} |");
            for row in &rows {
                table += &format!(" {} |", row[i].1);
            }
            table += "\n";
        }
        let block = (DOC.split_once("<!-- caps:begin -->\n"))
            .and_then(|(_, rest)| rest.split_once("<!-- caps:end -->"))
            .map(|(block, _)| block);
        assert!(
            block == Some(table.as_str()),
            "docs/chat-agents.md's caps table isn't what the code says; put this \
             between its caps:begin and caps:end lines:\n{table}"
        );
    }

    /// The agents of the doc's launch table: those of a chat, and T3 Code.
    const STARTS: [(AgentKind, &str); 7] = [
        (AgentKind::Claude, "Claude"),
        (AgentKind::Codex, "Codex"),
        (AgentKind::OpenCode, "OpenCode"),
        (AgentKind::Cursor, "Cursor"),
        (AgentKind::Pi, "pi"),
        (AgentKind::Antigravity, "agy"),
        (AgentKind::T3, "T3"),
    ];

    /// How `agent` starts, as the doc's launch table says it.
    fn launch_rows(agent: &dyn Integration) -> Vec<(&'static str, String)> {
        let yes = |on: bool| if on { "✓" } else { "—" }.to_owned();
        let command = agent.command().map_or("—".to_owned(), |c| format!("`{c}`"));
        vec![
            ("`command`: its program and `[agents]` key", command),
            (
                "`in_app`: starts as a draft in its app",
                yes(agent.in_app()),
            ),
            (
                "`adds_dirs`: `--add-dir`, told its first message",
                yes(agent.adds_dirs()),
            ),
            (
                "`adds_roots`: each repository a folder of its own",
                yes(agent.adds_roots()),
            ),
            (
                "`asks_trust`: started with `--trust`",
                yes(agent.asks_trust()),
            ),
            (
                "`takes_model`: started on `--model`",
                yes(agent.takes_model()),
            ),
            (
                "`picks_model`: a stage's model is picked",
                yes(agent.picks_model()),
            ),
            (
                "`models_read`: its CLI lists its models",
                yes(agent.models_read()),
            ),
            ("`resumes`: its chat is gone back to", yes(agent.resumes())),
            (
                "`retells_model`: told its model again then",
                yes(agent.retells_model()),
            ),
            (
                "`runs_stages`: offered for a feature's stages",
                yes(agent.runs_stages()),
            ),
        ]
    }

    #[test]
    fn the_chat_doc_says_how_agents_start() {
        const DOC: &str = include_str!("../../docs/chat-agents.md");
        let mut table = String::from("| Starting it |");
        for (_, name) in STARTS {
            table += &format!(" {name} |");
        }
        table += &format!("\n|---|{}\n", "---|".repeat(STARTS.len()));
        let rows: Vec<_> = STARTS.map(|(kind, _)| launch_rows(of(kind))).into();
        for (i, (method, _)) in rows[0].iter().enumerate() {
            table += &format!("| {method} |");
            for row in &rows {
                table += &format!(" {} |", row[i].1);
            }
            table += "\n";
        }
        let block = (DOC.split_once("<!-- launch:begin -->\n"))
            .and_then(|(_, rest)| rest.split_once("<!-- launch:end -->"))
            .map(|(block, _)| block);
        assert!(
            block == Some(table.as_str()),
            "docs/chat-agents.md's launch table isn't what the code says; put this \
             between its launch:begin and launch:end lines:\n{table}"
        );
    }

    /// The files of the chat's UI, desktop and phone, and the shared
    /// readers of what an agent's screen shows: they read `Caps`, items,
    /// live events, menus and side panels, and name no agent.
    const UI: [(&str, &str); 7] = [
        ("src/chat/view.rs", include_str!("../chat/view.rs")),
        ("src/chat/btw.rs", include_str!("../chat/btw.rs")),
        ("src/chat/menu.rs", include_str!("../chat/menu.rs")),
        (
            "src/chrome/agent_chat.rs",
            include_str!("../chrome/agent_chat.rs"),
        ),
        ("src/phone.rs", include_str!("../phone.rs")),
        ("src/phone_control.rs", include_str!("../phone_control.rs")),
        (
            "assets/phone/app.js",
            include_str!("../../assets/phone/app.js"),
        ),
    ];

    #[test]
    fn the_ui_names_no_agent() {
        let agents = ALL
            .iter()
            .map(|i| i.kind())
            .filter(|k| *k != AgentKind::Unknown);
        let agents: Vec<AgentKind> = agents.collect();
        let variants: Vec<String> = agents.iter().map(|k| format!("AgentKind::{k:?}")).collect();
        let mut names: Vec<String> = agents.iter().map(|k| format!("\"{}\"", k.name())).collect();
        names.push("\"agy\"".into());
        let mut found = Vec::new();
        for (path, text) in UI {
            let rust = path.ends_with(".rs");
            let mut lines = text.lines().enumerate().peekable();
            while let Some((i, line)) = lines.next() {
                // The tests come last, and may name any.
                let tests = lines
                    .peek()
                    .is_some_and(|(_, next)| next.ends_with("mod tests {"));
                if rust && line.starts_with("#[cfg(test)]") && tests {
                    break;
                }
                let code = line.split("//").next().unwrap_or_default();
                let named = (names.iter()).find(|name| code.contains(name.as_str()));
                let named = named.or_else(|| {
                    (variants.iter()).find(|variant| rust && line.contains(variant.as_str()))
                });
                if let Some(named) = named {
                    found.push(format!("{path}:{}: {named}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "agents named in the UI:\n{}",
            found.join("\n")
        );
    }
}
