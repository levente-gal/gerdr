//! What an agent's card shows it is: its maker's logo (see
//! `assets/agents`), a generic agent for one without, a flask for a test
//! run. The logos are monochrome, tinted as the other icons are.

use std::sync::OnceLock;

use crate::app_icons::AppIcon;
use crate::herdr::AgentInfo;

/// Which icon an agent's card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentIcon {
    Claude,
    Codex,
    Gemini,
    Cursor,
    OpenCode,
    Copilot,
    Amp,
    Qwen,
    Kimi,
    Grok,
    Kiro,
    Cline,
    Devin,
    Antigravity,
    Mastra,
    /// T3 Code, whose threads run the other agents in its own app.
    T3,
    /// An agent without a logo here: drawn as a robot's head.
    Generic,
    /// A test run (see `test_runs`): drawn as a flask.
    Test,
}

/// The logos, in `AgentIcon`'s order, as `assets/agents` has them.
const LOGOS: [&[u8]; 16] = [
    include_bytes!("../assets/agents/claude.svg"),
    include_bytes!("../assets/agents/codex.svg"),
    include_bytes!("../assets/agents/gemini.svg"),
    include_bytes!("../assets/agents/cursor.svg"),
    include_bytes!("../assets/agents/opencode.svg"),
    include_bytes!("../assets/agents/githubcopilot.svg"),
    include_bytes!("../assets/agents/amp.svg"),
    include_bytes!("../assets/agents/qwen.svg"),
    include_bytes!("../assets/agents/kimi.svg"),
    include_bytes!("../assets/agents/grok.svg"),
    include_bytes!("../assets/agents/kiro.svg"),
    include_bytes!("../assets/agents/cline.svg"),
    include_bytes!("../assets/agents/devin.svg"),
    include_bytes!("../assets/agents/antigravity.svg"),
    include_bytes!("../assets/agents/mastra.svg"),
    include_bytes!("../assets/agents/t3.svg"),
];

impl AgentIcon {
    /// The icon of the agent herdr calls `id` (`claude`, `codex`, …), or
    /// names so (`Claude Code`): case, spaces and dashes aside.
    pub fn from_agent(id: &str) -> Self {
        let id: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match id.as_str() {
            "claude" | "claudecode" => Self::Claude,
            "codex" | "openaicodex" | "codexcli" => Self::Codex,
            "gemini" | "geminicli" => Self::Gemini,
            "cursor" | "cursoragent" | "cursorcli" => Self::Cursor,
            "opencode" => Self::OpenCode,
            "copilot" | "githubcopilot" | "copilotcli" => Self::Copilot,
            "amp" | "ampcode" => Self::Amp,
            "qwen" | "qwencode" => Self::Qwen,
            "kimi" | "kimicli" | "kimicode" => Self::Kimi,
            "grok" | "grokcli" => Self::Grok,
            "kiro" | "kirocli" => Self::Kiro,
            "cline" => Self::Cline,
            "devin" => Self::Devin,
            "agy" | "antigravity" | "googleantigravity" => Self::Antigravity,
            "mastra" | "mastracode" => Self::Mastra,
            "t3" | "t3code" => Self::T3,
            _ => Self::Generic,
        }
    }

    /// The icon of the agent herdr reports as `info`: by its id, else by
    /// the name it shows.
    pub fn from_info(info: &AgentInfo) -> Self {
        [&info.agent, &info.display_agent]
            .into_iter()
            .flatten()
            .map(|id| Self::from_agent(id))
            .find(|icon| *icon != Self::Generic)
            .unwrap_or(Self::Generic)
    }

    /// Its logo, read once; none for the icons drawn as shapes.
    pub fn picture(self) -> Option<&'static AppIcon> {
        static PICTURES: [OnceLock<Option<AppIcon>>; LOGOS.len()] =
            [const { OnceLock::new() }; LOGOS.len()];
        let i = self as usize;
        let data = LOGOS.get(i)?;
        PICTURES[i]
            .get_or_init(|| AppIcon::from_svg(data, true))
            .as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentIcon, LOGOS};

    /// Every icon with a logo.
    fn logos() -> impl Iterator<Item = AgentIcon> {
        use AgentIcon::*;
        [
            Claude,
            Codex,
            Gemini,
            Cursor,
            OpenCode,
            Copilot,
            Amp,
            Qwen,
            Kimi,
            Grok,
            Kiro,
            Cline,
            Devin,
            Antigravity,
            Mastra,
            T3,
        ]
        .into_iter()
    }

    #[test]
    fn herdr_ids_and_names_find_their_logo() {
        use AgentIcon::*;
        for (id, icon) in [
            ("claude", Claude),
            ("Claude Code", Claude),
            ("codex", Codex),
            ("gemini", Gemini),
            ("Gemini CLI", Gemini),
            ("cursor", Cursor),
            ("cursor-agent", Cursor),
            ("opencode", OpenCode),
            ("OpenCode", OpenCode),
            ("copilot", Copilot),
            ("GitHub Copilot", Copilot),
            ("amp", Amp),
            ("qwen", Qwen),
            ("kimi", Kimi),
            ("grok", Grok),
            ("kiro", Kiro),
            ("cline", Cline),
            ("devin", Devin),
            ("agy", Antigravity),
            ("mastracode", Mastra),
            ("t3", T3),
            ("T3 Code", T3),
            // No logo of their own here.
            ("droid", Generic),
            ("pi", Generic),
            ("", Generic),
            ("something new", Generic),
        ] {
            assert_eq!(AgentIcon::from_agent(id), icon, "{id:?}");
        }
    }

    #[test]
    fn the_display_name_counts_when_the_id_is_unknown() {
        let mut info: crate::herdr::AgentInfo = serde_json::from_value(serde_json::json!({
            "pane_id": "p1", "terminal_id": "t1", "workspace_id": "w1", "tab_id": "t1",
            "agent": "claude-next", "display_agent": "Claude",
        }))
        .unwrap();
        assert_eq!(AgentIcon::from_info(&info), AgentIcon::Claude);
        info.agent = Some("codex".into());
        assert_eq!(AgentIcon::from_info(&info), AgentIcon::Codex);
        info.agent = None;
        info.display_agent = None;
        assert_eq!(AgentIcon::from_info(&info), AgentIcon::Generic);
    }

    #[test]
    fn every_logo_reads() {
        assert_eq!(logos().count(), LOGOS.len());
        for icon in logos() {
            let picture = icon.picture().unwrap_or_else(|| panic!("{icon:?} reads"));
            let (w, h, rgba) = picture.scaled(16);
            assert_eq!((w, h), (16, 16), "{icon:?}");
            assert!(rgba.chunks(4).any(|p| p[3] > 200), "{icon:?} draws");
        }
        assert_eq!(AgentIcon::Generic.picture(), None);
        assert_eq!(AgentIcon::Test.picture(), None);
    }
}
