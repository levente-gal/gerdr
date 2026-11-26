//! Claude Code (`claude`). Its chats are JSON lines under each config
//! folder's `projects/<folder>/<id>.jsonl`, its subagents' in files beside
//! them. Its live word comes from its hooks in `~/.claude/settings.json`,
//! which run `gerdr claude-hook`.

mod commands;
pub(crate) mod find;
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
    Integration, ModelSwitch, Parser, SidePanel, Store,
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
        ModelSwitch::Typed { cli: "claude" }
    }

    fn offers(&self, _cursor: &[Offer]) -> Vec<Offer> {
        crate::stage_models::claude_offers()
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
}
