//! Google's Antigravity CLI (`agy`).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Integration, ModelSwitch, Store};
use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{self, Found, Homes, Span},
    },
    chat::{commands, commands::Command, live, menu::Menu, mode, model_menu, source},
    scrollback::Styled,
};

pub struct Antigravity;

impl Integration for Antigravity {
    fn kind(&self) -> AgentKind {
        AgentKind::Antigravity
    }

    fn label(&self) -> &'static str {
        "Antigravity"
    }

    fn herdr_target(&self) -> Option<&'static str> {
        Some("antigravity-cli")
    }

    fn store(&self) -> Option<Store> {
        Some(Store::Lines)
    }

    fn guesses(&self) -> bool {
        true
    }

    fn find(
        &self,
        homes: &Homes,
        chat: &Chat,
        id: Option<&str>,
        claimed: &HashSet<String>,
        span: Span,
    ) -> Result<Option<Found>, String> {
        Ok(match id {
            Some(id) => locate::antigravity_by_id(&homes.antigravity, id),
            None => locate::antigravity_by_time(&homes.antigravity, chat, claimed, span),
        })
    }

    fn transcript_at(&self, path: &str, homes: &Homes) -> Option<PathBuf> {
        source::antigravity_transcript(path, homes)
    }

    fn transcripts_under(&self, homes: &Homes) -> Vec<PathBuf> {
        vec![homes.antigravity.join("brain")]
    }

    /// A transcript is in `brain/<id>/.system_generated/logs/`.
    fn id_in_path<'a>(&self, stem: &'a str, path: &'a Path) -> &'a str {
        (path.ancestors().nth(3))
            .and_then(|dir| dir.file_name()?.to_str())
            .unwrap_or(stem)
    }

    fn modes(&self) -> &'static [&'static str] {
        &["Auto", "Plan", "Default", "Edits"]
    }

    fn mode_shown(&self, rows: &[&str]) -> Option<String> {
        mode::antigravity_shown(rows)
    }

    fn model_switch(&self) -> ModelSwitch {
        ModelSwitch::OwnMenu
    }

    fn model_menu(&self, rows: &[(String, Styled)]) -> Option<Menu> {
        model_menu::antigravity(rows)
    }

    fn model_on_screen<'a>(&self, rows: &[&'a str]) -> Option<&'a str> {
        model_menu::antigravity_now(rows)
    }

    fn builtins(&self) -> &'static [(&'static str, &'static str)] {
        commands::ANTIGRAVITY
    }

    fn disk_commands(&self, cwd: &Path, homes: &Homes, _config: &Path, found: &mut Vec<Command>) {
        commands::antigravity_disk(cwd, homes, found);
    }

    fn hook_command(&self) -> Option<&'static str> {
        Some(live::antigravity::HOOK_COMMAND)
    }

    fn run_hook(&self) {
        live::antigravity::run_hook();
    }

    fn install_live(&self, home: &Path) -> Vec<Result<Option<live::Changed>, String>> {
        if !home.join(".gemini/config").is_dir() {
            return Vec::new();
        }
        vec![
            live::hook_command(live::antigravity::HOOK_COMMAND).and_then(|command| {
                live::install(&home.join(".gemini/config/hooks.json"), |text| {
                    live::antigravity::merged(text, &command)
                })
            }),
        ]
    }
}
