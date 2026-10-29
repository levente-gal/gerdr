//! What an agent's card shows it is: its maker's logo (see
//! `assets/agents`), a generic agent for one without, a flask for a test
//! run. The logos are drawn in their own colours; a part drawn in the
//! text's colour (`currentColor`) is tinted as text is.

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
    /// An agent without a logo here: drawn as a robot's head.
    Generic,
    /// A test run (see `test_runs`): drawn as a flask.
    Test,
}

/// The logos, in `AgentIcon`'s order, as `assets/agents` has them.
const LOGOS: [&[u8]; 15] = [
    include_bytes!("../assets/agents/claude-color.svg"),
    include_bytes!("../assets/agents/codex-color.svg"),
    include_bytes!("../assets/agents/gemini-color.svg"),
    include_bytes!("../assets/agents/cursor.svg"),
    include_bytes!("../assets/agents/opencode.svg"),
    include_bytes!("../assets/agents/githubcopilot.svg"),
    include_bytes!("../assets/agents/amp-color.svg"),
    include_bytes!("../assets/agents/qwen-color.svg"),
    include_bytes!("../assets/agents/kimi-color.svg"),
    include_bytes!("../assets/agents/grok.svg"),
    include_bytes!("../assets/agents/kiro-color.svg"),
    include_bytes!("../assets/agents/cline.svg"),
    include_bytes!("../assets/agents/devin-color.svg"),
    include_bytes!("../assets/agents/antigravity-color.svg"),
    include_bytes!("../assets/agents/mastra.svg"),
];

/// A logo as it is drawn: the part in the text's colour, tinted, then the
/// part in colours of its own over it. Either may be missing, not both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPicture {
    pub mono: Option<AppIcon>,
    pub color: Option<AppIcon>,
}

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
    pub fn picture(self) -> Option<&'static AgentPicture> {
        static PICTURES: [OnceLock<Option<AgentPicture>>; LOGOS.len()] =
            [const { OnceLock::new() }; LOGOS.len()];
        let i = self as usize;
        let data = LOGOS.get(i)?;
        PICTURES[i].get_or_init(|| picture(data)).as_ref()
    }
}

/// The two parts of the logo `data`: its `currentColor` fills alone, and
/// its other fills alone. lobe-icons gives each path a fill of its own (or
/// the root one), so blanking the others' leaves each part.
fn picture(data: &[u8]) -> Option<AgentPicture> {
    let svg = std::str::from_utf8(data).ok()?;
    let mono = blank_fills(svg, |fill| fill != "currentColor");
    let color = blank_fills(svg, |fill| fill == "currentColor");
    let layer = |svg: String, symbolic: bool| {
        AppIcon::from_svg(svg.as_bytes(), symbolic).filter(|icon| !icon.is_empty())
    };
    let picture = AgentPicture {
        mono: layer(mono, true),
        color: layer(color, false),
    };
    (picture.mono.is_some() || picture.color.is_some()).then_some(picture)
}

/// `svg` with each `fill="…"` that `blank` picks set to `none`.
fn blank_fills(svg: &str, blank: impl Fn(&str) -> bool) -> String {
    const FILL: &str = "fill=\"";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find(FILL) {
        let (before, after) = rest.split_at(at + FILL.len());
        out.push_str(before);
        let Some(end) = after.find('"') else {
            rest = after;
            break;
        };
        let fill = &after[..end];
        out.push_str(if blank(fill) { "none" } else { fill });
        rest = &after[end..];
    }
    out.push_str(rest);
    out
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
    fn every_logo_reads_in_its_parts() {
        assert_eq!(logos().count(), LOGOS.len());
        for icon in logos() {
            let picture = icon.picture().unwrap_or_else(|| panic!("{icon:?} reads"));
            let layers = [&picture.mono, &picture.color];
            for layer in layers.into_iter().flatten() {
                let (w, h, rgba) = layer.scaled(16);
                assert_eq!((w, h), (16, 16), "{icon:?}");
                assert!(rgba.chunks(4).any(|p| p[3] > 200), "{icon:?} draws");
            }
        }
        assert_eq!(AgentIcon::Generic.picture(), None);
        assert_eq!(AgentIcon::Test.picture(), None);
    }

    #[test]
    fn one_colour_logos_are_tinted_and_coloured_ones_are_not() {
        use AgentIcon::*;
        for icon in [Cursor, OpenCode, Copilot, Grok, Cline, Mastra] {
            let picture = icon.picture().unwrap();
            assert!(picture.mono.is_some() && picture.color.is_none(), "{icon:?}");
        }
        for icon in [Claude, Codex, Gemini, Amp, Qwen, Kiro, Devin, Antigravity] {
            let picture = icon.picture().unwrap();
            assert!(picture.mono.is_none() && picture.color.is_some(), "{icon:?}");
        }
        // Kimi's "K" is in the text's colour, its dot in blue.
        let kimi = Kimi.picture().unwrap();
        assert!(kimi.mono.is_some() && kimi.color.is_some());
    }

    #[test]
    fn no_coloured_part_is_mostly_black() {
        // A `currentColor` left in a coloured part would draw black, gone
        // on a dark theme.
        for icon in logos() {
            let Some(color) = &icon.picture().unwrap().color else {
                continue;
            };
            let (_, _, rgba) = color.scaled(16);
            let drawn: Vec<_> = rgba.chunks(4).filter(|p| p[3] > 128).collect();
            let black = drawn.iter().filter(|p| p[..3].iter().all(|c| *c < 40)).count();
            assert!(black * 10 < drawn.len(), "{icon:?}");
        }
    }
}
