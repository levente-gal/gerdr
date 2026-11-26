//! Claude Code.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Parser, PlanReply, Store, common::files::SubFiles};
use crate::{
    ai_work::{
        AgentKind, Chat, context,
        context::Context,
        locate::{self, Found, Homes, Span},
        usage::{self, Message},
    },
    chat::{
        Source, commands,
        commands::Command,
        live, mode,
        parse::{self, Reader},
        source,
    },
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
        Box::<parse::claude::Claude>::default()
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

    /// What Claude Code's subagents wrote since the last read, each in its
    /// own file beside the chat's (`<chat>/subagents/agent-<id>.jsonl`), the
    /// call that started it named in its `.meta.json`. Only those whose
    /// call was read: their work has a place to show.
    fn subagents(
        &self,
        chat: &Source,
        reader: &mut Reader,
        files: &mut SubFiles,
        chunk: u64,
    ) -> std::io::Result<()> {
        let dir = chat.path.with_extension("").join("subagents");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(agent) = (path.file_name().and_then(|n| n.to_str()))
                .and_then(|n| n.strip_prefix("agent-")?.strip_suffix(".jsonl"))
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(call) = started_by(&mut files.noted, &path) else {
                continue;
            };
            if !reader.knows_call(&call) {
                continue;
            }
            if let Err(err) = files.read(reader, &path, &agent, &call, chunk) {
                // One subagent unread leaves the chat as it is.
                tracing::warn!("agent chat: can't read {}: {err}", path.display());
            }
        }
        Ok(())
    }

    fn model_in_chat(&self, reader: &Reader) -> Option<String> {
        reader.model()
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

/// The call that started the subagent of file `path`, as its meta file
/// says (kept in `noted`); asked again until it says.
fn started_by(noted: &mut HashMap<PathBuf, String>, path: &Path) -> Option<String> {
    if let Some(call) = noted.get(path) {
        return Some(call.clone());
    }
    let meta = std::fs::read(path.with_extension("meta.json")).ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&meta).ok()?;
    let call = meta["toolUseId"].as_str()?.to_owned();
    noted.insert(path.to_owned(), call.clone());
    Some(call)
}
