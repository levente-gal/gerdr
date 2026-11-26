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

use self::common::{build::Builder, files::SubFiles};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::{Context, tail_of},
        locate::{self, Found, Homes, Span},
        usage::Message,
    },
    chat::{Source, commands::Command, live::Changed, menu::Menu, parse::Reader},
    scrollback::Styled,
    stage_models::Offer,
};

mod antigravity;
mod claude;
mod codex;
pub mod common;
mod cursor;
mod opencode;
mod pi;
mod plain;
mod t3;

/// Where an agent keeps a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// A transcript of lines that only grow.
    Lines,
    /// Rows of a database, read whole.
    Database,
}

/// How a chat switches its agent's model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSwitch {
    /// It can't.
    None,
    /// gerdr types `/model <id>`; `cli` is the command the stage picker
    /// names the agent by.
    Typed { cli: &'static str },
    /// `/model` opens a menu of the agent's own, mirrored in the chat.
    OwnMenu,
}

/// How a plan review goes back to the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReply {
    /// A line naming the review's file.
    Line,
    /// A deny of the agent's plan ask, with the comments.
    DenyAsk,
}

/// What a chat of an agent offers, all in one: what the UI reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // fields read by the UI from wave 2 on (contract)
pub struct Caps {
    /// gerdr reads its chats.
    pub chat: bool,
    /// Earlier messages are read on ask (the chat isn't read whole).
    pub earlier: bool,
    /// It has modes to step through.
    pub modes: bool,
    pub model: ModelSwitch,
    /// It takes side questions (`/btw`).
    pub side: bool,
    /// Its screen suggests what to say next.
    pub suggests: bool,
    /// Its asks can be allowed or denied from the chat.
    pub asks: bool,
    /// Its questions show as cards of their own.
    pub questions: bool,
    pub plan_reply: PlanReply,
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
    /// The models a chat offers to type; `cursor`: Cursor's, as last read.
    fn offers(&self, _cursor: &[Offer]) -> Vec<Offer> {
        Vec::new()
    }
    /// Its own model menu, as its screen `rows` show it at their bottom.
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
    fn plan_reply(&self) -> PlanReply {
        PlanReply::Line
    }
    /// Whether its screen suggests what to say next.
    fn suggests(&self) -> bool {
        false
    }
    /// Whether its asks can be allowed or denied from the chat.
    fn asks(&self) -> bool {
        false
    }
    /// Whether its questions show as cards of their own.
    fn questions(&self) -> bool {
        false
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

    /// What its chat offers, from the methods above.
    fn caps(&self) -> Caps {
        Caps {
            chat: self.store().is_some(),
            earlier: self.store() == Some(Store::Lines),
            modes: !self.modes().is_empty(),
            model: self.model_switch(),
            side: self.builtins().iter().any(|(name, _)| *name == "btw"),
            suggests: self.suggests(),
            asks: self.asks(),
            questions: self.questions(),
            plan_reply: self.plan_reply(),
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

/// Every agent, in `AgentKind`'s order.
pub const ALL: [&dyn Integration; 8] = [
    &claude::Claude,
    &cursor::Cursor,
    &codex::Codex,
    &opencode::OpenCode,
    &t3::T3,
    &pi::Pi,
    &antigravity::Antigravity,
    &plain::Plain,
];

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
        assert_eq!(claude.plan_reply, PlanReply::DenyAsk);
        assert!(matches!(claude.model, ModelSwitch::Typed { cli: "claude" }));
        assert_eq!(caps(AgentKind::Codex).model, ModelSwitch::OwnMenu);
        assert!(!caps(AgentKind::Codex).side && !caps(AgentKind::Pi).modes);
        assert!(!caps(AgentKind::OpenCode).earlier && caps(AgentKind::OpenCode).asks);
        for kind in [AgentKind::T3, AgentKind::Unknown] {
            let none = caps(kind);
            assert!(!none.chat && !none.modes && !none.side && !none.context);
            assert_eq!(none.model, ModelSwitch::None);
        }
    }
}
