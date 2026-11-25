//! An agent's modes: Claude Code's plan mode and its permission modes,
//! OpenCode's Build and Plan agents, Cursor's and Codex's Plan, and so on.
//! Which ones it has, which one its screen shows, and stepping it to the
//! one picked in the chat or on the phone with the key each of these
//! CLIs steps with.

use std::time::{Duration, Instant};

use crate::{ai_work::AgentKind, herdr::Herdr};

/// The key every agent CLI here goes to its next mode with, as herdr names
/// it (`pane.send_keys`).
pub const NEXT_KEY: &str = "shift+tab";

/// How long a press may take to show on the agent's screen.
const PRESS_TAKES: Duration = Duration::from_secs(2);
/// How often the screen is read for the mode while switching: a press
/// showed within 150 ms in each agent tried.
pub const READ_EVERY: Duration = Duration::from_millis(150);
/// How many of a pane's last rows are read for its mode.
const SCREEN_ROWS: usize = 40;

// shortcut: the modes and their words on screen are as the CLIs were here
// (Claude Code 2.1.332, OpenCode 2.3.0, Cursor 2026.11.22, Codex 0.171.0,
// Antigravity CLI 1.5.15); refresh them with `commands.rs`'s lists.
/// The modes agent `kind` steps through on `NEXT_KEY`, in its order; none
/// for one without (pi). Claude's Auto shows only where its account has it,
/// and OpenCode's own agents of the user's come after Plan.
pub fn modes(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &["Auto", "Manual", "Edits", "Plan"],
        AgentKind::OpenCode | AgentKind::Codex => &["Build", "Plan"],
        AgentKind::Cursor => &["Agent", "Plan", "Debug", "Ask"],
        AgentKind::Antigravity => &["Auto", "Plan", "Default", "Edits"],
        _ => &[],
    }
}

/// The mode agent `kind`'s screen shows on its last `rows`, by `modes`'s
/// name, or by the agent's own word for one not there (an OpenCode agent of
/// the user's, Claude's bypass). None when the screen doesn't say.
pub fn shown(kind: AgentKind, rows: &[&str]) -> Option<String> {
    let rows: Vec<&str> = (rows.iter().rev())
        .map(|row| row.trim())
        .filter(|row| !row.is_empty())
        .take(8)
        .collect();
    let last = |n: usize| rows.iter().take(n).copied();
    let cycle = "(shift+tab to cycle)";
    let word = match kind {
        AgentKind::Claude => last(3).find_map(|row| {
            [
                ("plan mode on", "Plan"),
                ("accept edits on", "Edits"),
                ("auto mode on", "Auto"),
                ("manual mode on", "Manual"),
                ("bypass permissions on", "Bypass"),
            ]
            .into_iter()
            .find_map(|(says, mode)| row.contains(says).then(|| mode.to_owned()))
        })?,
        // The agent's name heads the line under the prompt: "Build · model".
        AgentKind::OpenCode => last(8).find_map(|row| {
            let (name, _) = row.strip_prefix('┃')?.split_once(" · ")?;
            Some(name.trim().to_owned()).filter(|name| !name.is_empty())
        })?,
        // Its default says nothing.
        AgentKind::Cursor => last(4)
            .find_map(|row| Some(row.split_once(cycle)?.0.trim().to_owned()))
            .unwrap_or_else(|| "Agent".into()),
        AgentKind::Codex => last(3)
            .find_map(|row| {
                let said = row.split_once(cycle)?.0.trim().strip_suffix(" mode")?;
                Some(said.rsplit("  ").next()?.trim().to_owned())
            })
            .unwrap_or_else(|| "Build".into()),
        // "auto · model · effort" at the right of its last row; its
        // default leaves the model first.
        AgentKind::Antigravity => {
            let row = last(2).find(|row| row.contains(" · "))?;
            let said = row.split_once(" · ")?.0.split_whitespace().last()?;
            match said {
                "auto" => "Auto",
                "plan" => "Plan",
                "accept-edits" => "Edits",
                _ => "Default",
            }
            .to_owned()
        }
        _ => return None,
    };
    Some(word)
}

/// The mode agent `kind` shows on `ansi`, a pane's text with its escape
/// sequences as herdr reads it.
pub fn shown_in(kind: AgentKind, ansi: &str) -> Option<String> {
    let rows = crate::scrollback::parse_ansi(ansi);
    let texts: Vec<&str> = rows.iter().map(|(text, _)| text.as_str()).collect();
    shown(kind, &texts)
}

/// The mode agent `kind` in herdr pane `pane` shows now.
pub fn read(herdr: &mut Herdr, pane: &str, kind: AgentKind) -> Result<Option<String>, String> {
    let text = herdr.pane_styled(pane, SCREEN_ROWS)?;
    // An app on the alternate screen has no scrollback: its screen.
    let text = match text.trim().is_empty() {
        true => herdr.pane_screen_styled(pane, SCREEN_ROWS)?,
        false => text,
    };
    Ok(shown_in(kind, &text))
}

/// An agent to switch the mode of: what it is in, and its mode key.
pub trait Switched {
    fn shown(&mut self) -> Result<Option<String>, String>;
    fn press(&mut self) -> Result<(), String>;
}

/// Agent `kind` in herdr pane `pane`.
pub struct InPane<'a> {
    pub herdr: &'a mut Herdr,
    pub pane: &'a str,
    pub kind: AgentKind,
}

impl Switched for InPane<'_> {
    fn shown(&mut self) -> Result<Option<String>, String> {
        read(self.herdr, self.pane, self.kind)
    }

    fn press(&mut self) -> Result<(), String> {
        self.herdr.send_keys(self.pane, &[NEXT_KEY])
    }
}

/// Switch `agent`, of `n` modes, to mode `want`: a press of its mode key at
/// a time, each once the last one showed (read `every` so often), till its
/// screen shows `want`. It begins from the mode its screen shows, else from
/// `was`, the one shown to the user. Blocks till then; why not, if it
/// doesn't get there.
pub fn switch(
    agent: &mut impl Switched,
    n: usize,
    was: Option<&str>,
    want: &str,
    every: Duration,
) -> Result<(), String> {
    let from = agent.shown()?.or_else(|| was.map(str::to_owned));
    if from.as_deref() == Some(want) {
        return Ok(());
    }
    agent.press()?;
    let mut goal = Goal::new(want, from.as_deref(), n, Instant::now());
    let mut last = from;
    loop {
        std::thread::sleep(every);
        let now = agent.shown()?;
        if now.is_some() {
            last.clone_from(&now);
        }
        match goal.step(now.as_deref(), Instant::now()) {
            Step::Done => return Ok(()),
            Step::Wait => {}
            Step::Press => agent.press()?,
            Step::GiveUp => {
                return Err(match last {
                    Some(last) => format!("it stayed in {last}"),
                    None => "its screen doesn't show its mode".to_owned(),
                });
            }
            Step::NotThere => {
                return Err("it doesn't offer it here, so it stays as it was".to_owned());
            }
        }
    }
}

/// The modes a chat offers: its agent's, and the one its screen shows when
/// that is none of them, last.
pub fn offered(kind: AgentKind, now: Option<&str>) -> Vec<String> {
    let mut all: Vec<String> = modes(kind).iter().map(|m| (*m).to_owned()).collect();
    if let Some(now) = now.filter(|now| !all.iter().any(|m| m == now)) {
        all.push(now.to_owned());
    }
    all
}

/// A mode picked, while gerdr steps its agent there: one press of
/// `NEXT_KEY` at a time, each once the last one showed.
#[derive(Clone, Debug, PartialEq)]
struct Goal {
    pub want: String,
    /// The mode it began from: back there, it went round without it.
    start: Option<String>,
    /// The mode shown when the last press went.
    from: Option<String>,
    /// How many more presses it may take.
    left: usize,
    at: Instant,
}

/// What stepping to a goal does next, as the screen now shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Done,
    Press,
    /// Read the screen again: the last press hasn't shown yet.
    Wait,
    /// Its press didn't show in time, or it pressed as often as it may.
    GiveUp,
    /// It went round and back to where it began: the agent doesn't offer
    /// the mode (Claude's Auto on an account without it).
    NotThere,
}

impl Goal {
    /// Going to `want` from `now`, the first press just sent at `at`. It
    /// may go round the agent's `n` modes once, and some more of the user's
    /// own (OpenCode's agents).
    fn new(want: &str, now: Option<&str>, n: usize, at: Instant) -> Self {
        Self {
            want: want.to_owned(),
            start: now.map(str::to_owned),
            from: now.map(str::to_owned),
            left: n + 6,
            at,
        }
    }

    /// The screen shows mode `now` (none: it doesn't say) at `at`.
    fn step(&mut self, now: Option<&str>, at: Instant) -> Step {
        if now == Some(self.want.as_str()) {
            return Step::Done;
        }
        if now.is_none() || now == self.from.as_deref() {
            return match at.duration_since(self.at) > PRESS_TAKES {
                true => Step::GiveUp,
                false => Step::Wait,
            };
        }
        if self.start.is_some() && now == self.start.as_deref() {
            return Step::NotThere;
        }
        if self.left == 0 {
            return Step::GiveUp;
        }
        self.left -= 1;
        self.from = now.map(str::to_owned);
        self.at = at;
        Step::Press
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shows(kind: AgentKind, screen: &str) -> Option<String> {
        shown(kind, &screen.lines().collect::<Vec<_>>())
    }

    #[test]
    fn each_agents_screen_says_its_mode() {
        let claude = "❯ Try \"edit <filepath> to…\"\n──────\n  Opus 5.5 · master\n  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents\n\n";
        assert_eq!(shows(AgentKind::Claude, claude).as_deref(), Some("Auto"));
        let plan = "  Opus 5.5 · master\n  ⏸ plan mode on (shift+tab to cycle) · ← for agents";
        assert_eq!(shows(AgentKind::Claude, plan).as_deref(), Some("Plan"));
        assert_eq!(shows(AgentKind::Claude, "  ⏸ manual mode on").as_deref(), Some("Manual"));
        assert_eq!(shows(AgentKind::Claude, "  ⏵⏵ accept edits on").as_deref(), Some("Edits"));
        // A screen without the line doesn't say (a dialog over it).
        assert_eq!(shows(AgentKind::Claude, "❯ 1. Yes\n  2. No"), None);

        let opencode = "  ┃\n  ┃  Ask anything… \"Fix a TODO\"\n  ┃\n  ┃  Plan · GPT-5.6 Sol OpenAI · high\n  ╹▀▀▀▀\n  /…/modeprobe:master  shift+tab agents  ctrl+p commands\n      2.3.0";
        assert_eq!(shows(AgentKind::OpenCode, opencode).as_deref(), Some("Plan"));
        let own = "  ┃  Cavecrew-Builder · GPT-5.6 Sol OpenAI · high\n  ╹▀▀▀▀";
        assert_eq!(shows(AgentKind::OpenCode, own).as_deref(), Some("Cavecrew-Builder"));

        let cursor = " ▀▀▀▀\n  Debug (shift+tab to cycle)\n  Auto            Run Everything\n  /tmp/modeprobe · master";
        assert_eq!(shows(AgentKind::Cursor, cursor).as_deref(), Some("Debug"));
        let agent = "  → Plan, search, build anything\n ▀▀▀▀\n  Auto            Run Everything\n  /tmp/modeprobe · master";
        assert_eq!(shows(AgentKind::Cursor, agent).as_deref(), Some("Agent"));

        let codex = "› Ask Codex to do anything\n  GPT-6.1 Terra medium · /tmp/m\n  ← for agents · ? for shortcuts          Plan mode (shift+tab to cycle)";
        assert_eq!(shows(AgentKind::Codex, codex).as_deref(), Some("Plan"));
        let build = "› Ask Codex to do anything\n  GPT-6.1 Terra medium · /tmp/m\n  ← for agents · ? for shortcuts";
        assert_eq!(shows(AgentKind::Codex, build).as_deref(), Some("Build"));

        let agy = |mode: &str| format!("> \n────\n? for shortcuts            {mode}Gemini 4 Hydrogen · high");
        assert_eq!(shows(AgentKind::Antigravity, &agy("plan · ")).as_deref(), Some("Plan"));
        assert_eq!(shows(AgentKind::Antigravity, &agy("accept-edits · ")).as_deref(), Some("Edits"));
        assert_eq!(shows(AgentKind::Antigravity, &agy("")).as_deref(), Some("Default"));

        assert_eq!(shows(AgentKind::Pi, "anything"), None);
        assert!(modes(AgentKind::Pi).is_empty());
    }

    /// An agent going round `modes` on each press, its screen showing the
    /// new one a read after the press.
    struct Fake {
        modes: Vec<&'static str>,
        at: usize,
        pressed: usize,
        lag: bool,
    }

    impl Switched for Fake {
        fn shown(&mut self) -> Result<Option<String>, String> {
            let at = match std::mem::take(&mut self.lag) {
                true => self.at + self.modes.len() - 1,
                false => self.at,
            };
            Ok(Some(self.modes[at % self.modes.len()].to_owned()))
        }

        fn press(&mut self) -> Result<(), String> {
            self.at += 1;
            self.pressed += 1;
            self.lag = true;
            Ok(())
        }
    }

    fn fake(modes: &[&'static str]) -> Fake {
        Fake {
            modes: modes.to_vec(),
            at: 0,
            pressed: 0,
            lag: false,
        }
    }

    #[test]
    fn switching_presses_till_the_mode_shows_or_says_why_not() {
        let mut cursor = fake(&["Agent", "Plan", "Debug", "Ask"]);
        assert_eq!(switch(&mut cursor, 4, None, "Ask", Duration::ZERO), Ok(()));
        assert_eq!((cursor.pressed, cursor.at), (3, 3));
        // Already there: no press.
        assert_eq!(switch(&mut cursor, 4, None, "Ask", Duration::ZERO), Ok(()));
        assert_eq!(cursor.pressed, 3);
        // Claude without Auto: round once, and back where it was.
        let mut claude = fake(&["Manual", "Edits", "Plan"]);
        let not = switch(&mut claude, 4, None, "Auto", Duration::ZERO);
        assert!(not.unwrap_err().contains("doesn't offer it"));
        assert_eq!(claude.at % 3, 0);
    }

    #[test]
    fn a_mode_of_the_users_own_is_offered_too() {
        assert_eq!(offered(AgentKind::OpenCode, Some("Plan")), ["Build", "Plan"]);
        assert_eq!(offered(AgentKind::OpenCode, Some("Docs")), ["Build", "Plan", "Docs"]);
        assert_eq!(offered(AgentKind::Codex, None), ["Build", "Plan"]);
    }

    #[test]
    fn a_goal_presses_once_each_press_showed_till_its_mode_shows() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // Cursor from Agent to Ask: three presses, the first sent already.
        let mut goal = Goal::new("Ask", Some("Agent"), 4, t0);
        assert_eq!(goal.step(Some("Agent"), ms(100)), Step::Wait);
        assert_eq!(goal.step(None, ms(150)), Step::Wait);
        assert_eq!(goal.step(Some("Plan"), ms(200)), Step::Press);
        assert_eq!(goal.step(Some("Plan"), ms(300)), Step::Wait);
        assert_eq!(goal.step(Some("Debug"), ms(400)), Step::Press);
        assert_eq!(goal.step(Some("Ask"), ms(500)), Step::Done);
        // A press that never shows gives up.
        let mut stuck = Goal::new("Plan", Some("Build"), 2, t0);
        assert_eq!(stuck.step(Some("Build"), ms(2500)), Step::GiveUp);
        // A mode that never comes round: back where it began, it stops.
        let mut never = Goal::new("Auto", Some("Manual"), 3, t0);
        assert_eq!(never.step(Some("Edits"), ms(100)), Step::Press);
        assert_eq!(never.step(Some("Plan"), ms(200)), Step::Press);
        assert_eq!(never.step(Some("Manual"), ms(300)), Step::NotThere);
        // From a mode not known, it stops after its presses.
        let mut blind = Goal::new("Auto", None, 1, t0);
        let presses = (0..20)
            .take_while(|i| {
                let now = if i % 2 == 0 { "Plan" } else { "Manual" };
                blind.step(Some(now), ms(*i as u64)) == Step::Press
            })
            .count();
        assert_eq!(presses, 7);
    }
}
