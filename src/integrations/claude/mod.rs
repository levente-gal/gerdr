//! Claude Code.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, PlanReply, Store};
use crate::{
    ai_work::{
        AgentKind, Chat, context,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::{self, Message},
    },
    chat::{commands, commands::Command, live, mode, parse::Reader, source},
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

    fn find(
        &self,
        homes: &Homes,
        _chat: &Chat,
        id: Option<&str>,
        _claimed: &HashSet<String>,
        _span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(id.and_then(|id| locate::claude(homes, id)))
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        source::claude_transcript(path, &homes.claude)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        source::claude_projects(&homes.claude)
    }

    fn remote_script(&self, _chat: &Chat, id: Option<&str>) -> Option<String> {
        id.map(locate::claude_script)
    }

    fn model_in_chat(&self, reader: &Reader) -> Option<String> {
        reader.model().map(str::to_owned)
    }

    fn has_context(&self) -> bool {
        true
    }

    fn context_in(&self, tail: &[u8]) -> Option<Context> {
        context::claude_context(tail)
    }

    fn usage(&self, raw: &[u8]) -> Vec<Message> {
        usage::claude(raw)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Auto", "Manual", "Edits", "Plan"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::claude_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::Typed { cli: "claude" }
    }

    fn offers(&self, _cursor: &[Offer]) -> Vec<Offer> {
        crate::stage_models::claude_offers()
    }

    fn plan_reply(&self) -> PlanReply {
        PlanReply::DenyAsk
    }

    fn suggests(&self) -> bool {
        true
    }

    fn asks(&self) -> bool {
        true
    }

    fn questions(&self) -> bool {
        true
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::CLAUDE
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::claude_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::claude::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::claude::run_hook();
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        if !home.join(".claude").is_dir() {
            return Vec::new();
        }
        vec![
            live::hook_command(live::claude::HOOK_COMMAND).and_then(|command| {
                live::install(&home.join(".claude/settings.json"), |text| {
                    live::claude::merged(text, &command)
                })
            }),
        ]
    }
}
