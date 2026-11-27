//! Claude Code (`claude`). Its chats are JSON lines under each config
//! folder's `projects/<folder>/<id>.jsonl`, its subagents' in files beside
//! them. Its live word comes from its hooks in `~/.claude/settings.json`,
//! which run `gerdr claude-hook`.

mod commands;
pub(crate) mod find;
mod launch;
mod live;
pub(crate) mod parse;
mod screen;
mod stats;
mod subagents;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{
    Integration, ModelSwitch, Parser, PickedFor, SidePanel, Store,
    common::{files::SubFiles, install::Changed},
};
use crate::{
    ai_work::{
        AgentKind, Chat,
        context::{Context, tail_of},
        locate::{Found, Homes, Span},
        usage::Message,
    },
    chat::{Source, commands::Command, parse::Reader},
    herdr::Herdr,
    scrollback::Styled,
    stage_models::Offer,
};

pub struct Claude;

impl Integration for Claude {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn parser(&self) -> Box<dyn Parser> {
        Box::<parse::Claude>::default()
    }

    fn find(
        &self,
        homes: &Homes,
        _chat: &Chat,
        id: Option<&str>,
        _claimed: &HashSet<String>,
        _span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(id.and_then(|id| find::by_id(homes, id)))
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        find::transcript(path, &homes.claude)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        find::projects(&homes.claude)
    }

    fn remote_script(&self, _chat: &Chat, id: Option<&str>) -> Option<String> {
        id.map(find::script)
    }

    fn subagents(
        &self,
        chat: &Source,
        reader: &mut Reader,
        files: &mut SubFiles,
        chunk: u64,
    ) -> std::io::Result<()> {
        subagents::read(chat, reader, files, chunk)
    }

    fn model_in_chat(&self, reader: &Reader) -> Option<String> {
        reader.model()
    }

    fn model_now(&self, chat: Option<&Path>) -> Option<String> {
        parse::last_in(&tail_of(chat?)?)
    }

    fn effort_now(&self, chat: Option<&Path>) -> Option<String> {
        parse::effort_in(&tail_of(chat?)?)
    }

    fn pane_transcript(&self, herdr: &mut Herdr, pane: &str) -> Option<PathBuf> {
        find::claude_chat(herdr, pane)
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context_in(&self, tail: &[u8]) -> Option<Context> {
        stats::context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        stats::usage(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Auto", "Manual", "Edits", "Plan"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        screen::mode_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::Typed
    }

    fn offers(&self, _listed: &[Offer]) -> Vec<Offer> {
        launch::offers()
    }

    fn effort_switch(&self) -> ModelSwitch {
        ModelSwitch::Typed
    }

    /// `/effort`'s levels, the same for each model (2.1.332).
    fn efforts(&self) -> &'static [&'static str] {
        &["low", "medium", "high", "xhigh", "max"]
    }

    /// `/model default`: Claude Code's own choice.
    fn own_model(&self) -> Option<Offer> {
        Some(Offer {
            id: "default".into(),
            name: "Default".into(),
        })
    }

    fn suggests(&self) -> bool {
        true
    }

    fn suggestion(&self, rows: &[(String, Styled)]) -> Option<String> {
        screen::suggestion(rows)
    }

    fn side_panel(&self) -> Option<SidePanel> {
        Some(screen::SIDE_PANEL)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::BUILTINS
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::on_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::run_hook();
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<Changed>, String>> {
        live::install(home)
    }

    fn command(&self) -> Option<&'static str> {
        Some("claude")
    }

    fn adds_dirs(&self) -> bool {
        true
    }

    fn add_dirs(&self, line: String, dirs: &[PathBuf]) -> String {
        launch::add_dirs(line, dirs)
    }

    fn takes_model(&self) -> bool {
        true
    }

    fn takes_effort(&self) -> bool {
        true
    }

    /// It takes a chat id of our choosing (`new_chat`). Its chat, gone back
    /// to, keeps its model (tried on 2.1.318): one changed there with
    /// `/model` must stay.
    fn resumes(&self) -> bool {
        true
    }

    fn new_chat(&self, _base: &str, id_file: &str) -> Option<String> {
        Some(launch::new_chat(id_file))
    }

    fn run_model(&self, homes: &Homes, id: &str) -> Option<String> {
        launch::run_model(homes, id)
    }

    fn model_hint(&self, of: PickedFor) -> Option<&'static str> {
        // Tried on 2.1.325: its `/model` saves the model as the default of
        // new sessions.
        (of == PickedFor::Chat)
            .then_some("Model for this agent; Claude Code may keep it as its default…")
    }
}
