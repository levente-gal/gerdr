//! Side questions (`/btw`): asked from the chat or the phone, answered by
//! the agent in a panel of its own that its transcript never holds (Claude
//! Code 2.1.332, Cursor CLI 2026.11.22, OpenCode 2.3.0 and Antigravity CLI
//! 1.5.17, probed 2026-11-25). gerdr reads that panel off the pane's screen
//! (`panel`), presses what shows the rest of it (`Follow`), closes it once
//! it holds the whole answer, and keeps it for this run: rows of the chat
//! (`with_sides`) and a card on the phone.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Serialize;

use super::Item;
use crate::herdr::AgentSession;
use crate::md_blocks::{self, DocBlock, Inline, Styled};

/// A side question and what came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub question: String,
    /// When it was asked, in Unix ms: where its rows go among the chat's.
    pub at: u64,
    /// The answer's lines as the agent drew them, read so far.
    pub lines: Vec<String>,
    pub state: SideState,
    /// The agent's session it was asked in, as gerdr knew it: a new one
    /// (`/clear`, `/new`, `/resume`) leaves it behind.
    pub session: Option<AgentSession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "why")]
pub enum SideState {
    /// Sent: its panel hasn't shown yet.
    Asked,
    /// Its panel shows: answering, or being read.
    Answering,
    Answered,
    Failed(String),
}

impl Side {
    pub fn new(question: &str, at: u64) -> Self {
        Self {
            question: question.to_owned(),
            at,
            lines: Vec::new(),
            state: SideState::Asked,
            session: None,
        }
    }

    /// Still being asked or read.
    pub fn going(&self) -> bool {
        matches!(self.state, SideState::Asked | SideState::Answering)
    }
}

/// The question of `line`, as typed to an agent: `/btw` and what follows.
pub fn question(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("/btw")?;
    let q = rest.trim();
    (rest.starts_with(char::is_whitespace) && !q.is_empty()).then_some(q)
}

/// An agent's side panel as its screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    /// What its `/btw` row says, as far as it shows.
    pub question: String,
    /// The answer's rows in sight, without the panel's frame.
    pub lines: Vec<String>,
    /// A spinner instead of an answer.
    pub working: bool,
    /// Its footer says ↑/↓ scroll it.
    pub scrolls: bool,
    /// Its footer says Tab shows the rest (Antigravity).
    pub cut: bool,
}

/// What starts each agent's footer: how to close it, OpenCode's how to
/// copy, and Antigravity's how to see more, a row above its close on a
/// narrow pane.
const FOOTERS: [&str; 6] = [
    "to close",
    "to dismiss",
    "copy c",
    "to scroll",
    "Tab to expand",
    "Tab to collapse",
];

/// What an agent draws while it answers, in the answer's place or under
/// what it has so far: a braille spinner (Cursor, Antigravity), or Claude's
/// star with a word that goes on (`✽ Answering…`), which a bullet doesn't.
fn spinner(line: &str) -> bool {
    let Some(c) = line.chars().next() else {
        return false;
    };
    let goes_on = line.ends_with('…') || line.ends_with("...") || line.contains("... (");
    ('\u{2800}'..='\u{28ff}').contains(&c) || ("✻✽✶✢✳·*".contains(c) && goes_on)
}

fn is_rule(row: &str) -> bool {
    let row = row.trim();
    !row.is_empty() && row.chars().all(|c| "─━▔▀═".contains(c))
}

/// The side panel among a pane's screen `rows`, if one shows: its last
/// `/btw` row (Claude lists the earlier ones above it) down to its footer.
/// Each row is cut from that row's column: OpenCode's panel floats over
/// the chat, and Antigravity's has a bar at its left.
/// shortcut: columns are counted in chars; a wide char left of a floating
/// panel shifts its rows. Count cells if that shows.
pub fn panel(rows: &[&str]) -> Option<Panel> {
    // A `/btw` row is the panel's own when nothing but its frame is left
    // of it, or a gap (OpenCode's over the chat); an answer that says
    // `/btw` has it mid-text, or right of the panel's column.
    let asks = |row: &&str| {
        let at = row.find("/btw ")?;
        let before = &row[..at];
        let own = before.trim().chars().all(|c| "┃│".contains(c)) || before.ends_with("  ");
        own.then(|| before.chars().count())
    };
    let col = rows.iter().filter_map(asks).min()?;
    let q = rows.iter().rposition(|row| asks(row) == Some(col))?;
    let at = rows[q].find("/btw ")?;
    let asked = rows[q][at + 5..].trim();
    let question = asked.strip_suffix("esc").unwrap_or(asked);
    let footer = (q + 1..rows.len()).find(|&i| FOOTERS.iter().any(|f| rows[i].contains(f)));
    let end = footer
        .or_else(|| (q + 1..rows.len()).find(|&i| is_rule(rows[i])))
        .unwrap_or(rows.len());
    let mut lines: Vec<String> = (rows[q + 1..end].iter())
        .map(|row| {
            let cut: String = row.chars().skip(col).collect();
            // Claude marks the rows past which it scrolls.
            let cut = cut.trim_end().trim_end_matches(['↑', '↓']);
            cut.trim_end().to_owned()
        })
        .collect();
    trimmed(&mut lines);
    // Antigravity draws the answer as it comes, its spinner under it.
    let working = [lines.first(), lines.last()].into_iter().flatten().any(|l| spinner(l));
    // Its rows down to the panel's end: a narrow pane wraps it.
    let footer = footer.map(|i| {
        let rest = rows[i..].iter().take(3).take_while(|row| !is_rule(row));
        rest.copied().collect::<Vec<_>>().join(" ")
    });
    if footer.is_none() && !working {
        return None;
    }
    Some(Panel {
        question: words(question),
        lines,
        working,
        scrolls: footer.as_ref().is_some_and(|f| f.contains("scroll")),
        cut: footer.as_ref().is_some_and(|f| f.contains("Tab to expand")),
    })
}

/// `lines` without their common indent, nor empty lines at either end.
fn trimmed(lines: &mut Vec<String>) {
    let indent = (lines.iter())
        .filter(|line| !line.is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    for line in lines.iter_mut() {
        line.drain(..indent.min(line.len()));
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let first = lines.iter().position(|line| !line.is_empty());
    lines.drain(..first.unwrap_or(lines.len()));
}

fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Add `window`, the rows in sight after a scroll down, to `read`, the
/// rows read so far, where they overlap. Whether it brought new rows.
/// shortcut: rows that repeat can overlap at the wrong place; a gap (a
/// scroll past the window) is joined as it comes.
fn merge(read: &mut Vec<String>, window: &[String]) -> bool {
    for o in 0..read.len() {
        let tail = &read[o..];
        let n = tail.len().min(window.len());
        if tail[..n] == window[..n] {
            let grew = window.len() > n;
            read.extend_from_slice(&window[n..]);
            return grew;
        }
    }
    read.extend_from_slice(window);
    !window.is_empty()
}

/// What to do in the pane after a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Read again.
    Wait,
    /// Press these keys, then read again.
    Press(&'static [&'static str]),
    /// It holds the whole answer: close the agent's panel (herdr's key
    /// names), and stop.
    Close(&'static str),
    /// Stop: it's answered, or failed.
    Done,
}

/// How many reads in a row must show the same answer before it counts as
/// whole: Cursor draws it as it comes.
const STILL: u8 = 3;
/// How many reads in a row must miss a panel seen before it counts as
/// closed: a frame drawn halfway reads as none.
const GONE: u8 = 3;
/// How long its panel may take to show, at most: OpenCode shows none
/// until it has answered.
pub const SHOW_WAIT: Duration = Duration::from_secs(120);
/// How long a side question is followed, at most.
pub const FOLLOW_MOST: Duration = Duration::from_secs(300);
/// A scroll down, enough rows to move on and few enough to overlap.
const DOWN: &[&str] = &["Down", "Down", "Down", "Down", "Down"];

/// A side question being followed on its pane's screen.
#[derive(Debug)]
pub struct Follow {
    pub side: Side,
    since: Instant,
    /// Reads in a row that changed nothing.
    still: u8,
    seen: bool,
    /// Reads in a row that found no panel since one was seen.
    gone: u8,
    tabbed: bool,
    scrolling: bool,
}

impl Follow {
    pub fn new(side: Side, now: Instant) -> Self {
        Self {
            side,
            since: now,
            still: 0,
            seen: false,
            gone: 0,
            tabbed: false,
            scrolling: false,
        }
    }

    /// Whether `panel` is this question's: its row shows as much of it as
    /// fits, cut with an ellipsis maybe.
    fn ours(&self, panel: &Panel) -> bool {
        let shown = panel.question.trim_end_matches(['…', '.']).trim_end();
        !shown.is_empty() && words(&self.side.question).starts_with(shown)
    }

    /// The rows of `panel` past the question: a long one wraps onto them.
    fn answer(&self, panel: &Panel) -> Vec<String> {
        let all = words(&self.side.question);
        let mut rest = all.strip_prefix(panel.question.as_str()).unwrap_or("").trim();
        let mut lines = panel.lines.as_slice();
        while let Some(line) = lines.first().filter(|_| !rest.is_empty()) {
            let line = words(line);
            if line.is_empty() || !rest.starts_with(&line) {
                break;
            }
            rest = rest[line.len()..].trim_start();
            lines = &lines[1..];
        }
        let mut lines = lines.to_vec();
        trimmed(&mut lines);
        lines
    }

    fn end(&mut self, state: SideState) -> Step {
        self.side.state = state;
        Step::Done
    }

    /// What `panel`, as read from the pane now (none showing), says.
    pub fn took(&mut self, panel: Option<Panel>, now: Instant) -> Step {
        let Some(panel) = panel.filter(|p| self.ours(p)) else {
            if self.seen {
                self.gone += 1;
                if self.gone < GONE {
                    return Step::Wait;
                }
            }
            return match self.seen {
                // Closed in the pane: what was read stays.
                true if !self.side.lines.is_empty() => self.end(SideState::Answered),
                true => self.end(SideState::Failed("its panel closed before it answered".into())),
                false if now - self.since >= SHOW_WAIT => {
                    self.end(SideState::Failed("no answer showed in its pane".into()))
                }
                false => Step::Wait,
            };
        };
        self.seen = true;
        self.gone = 0;
        self.side.state = SideState::Answering;
        if now - self.since >= FOLLOW_MOST {
            self.side.state = SideState::Answered;
            return Step::Close("esc");
        }
        if panel.working {
            return Step::Wait;
        }
        if panel.cut && !self.tabbed {
            self.tabbed = true;
            return Step::Press(&["Tab"]);
        }
        let window = self.answer(&panel);
        let changed = match self.scrolling {
            true => merge(&mut self.side.lines, &window),
            false => std::mem::replace(&mut self.side.lines, window.clone()) != window,
        };
        if changed {
            self.still = 0;
            return match self.scrolling {
                true => Step::Press(DOWN),
                false => Step::Wait,
            };
        }
        self.still += 1;
        if self.scrolling {
            // The end of it: a scroll that brought nothing, read twice.
            return match self.still >= 2 {
                true => self.close(),
                false => Step::Wait,
            };
        }
        if self.still < STILL {
            return Step::Wait;
        }
        match panel.scrolls {
            true => {
                self.scrolling = true;
                self.still = 0;
                Step::Press(DOWN)
            }
            false => self.close(),
        }
    }

    fn close(&mut self) -> Step {
        self.side.state = SideState::Answered;
        Step::Close("esc")
    }
}

/// The id prefix of a side question's rows among a chat's items.
const ID: &str = "side:";

/// `items` with the cards of `sides` in their places (instead of those
/// they had): each before the first item said after it was asked.
pub fn with_sides(items: &Arc<Vec<Item>>, sides: &[Side]) -> Arc<Vec<Item>> {
    let had = items.iter().any(|item| item.id().starts_with(ID));
    if sides.is_empty() && !had {
        return items.clone();
    }
    let mut out: Vec<Item> = (items.iter())
        .filter(|item| !item.id().starts_with(ID))
        .cloned()
        .collect();
    for (n, side) in sides.iter().enumerate().rev() {
        let at = |item: &Item| match item {
            Item::User { at, .. } | Item::Assistant { at, .. } => *at,
            _ => None,
        };
        let place = (out.iter())
            .position(|item| at(item).is_some_and(|t| t > side.at))
            .unwrap_or(out.len());
        out.splice(place..place, side_items(n, side));
    }
    Arc::new(out)
}

/// Drop the side questions asked in another session than `now`, the one
/// the agent holds; whether any went. None goes while one is still being
/// asked or read: its follower writes it back by its place in `sides`.
pub fn drop_left(sides: &mut Vec<Side>, now: Option<&AgentSession>) -> bool {
    let Some(now) = now else {
        return false;
    };
    if sides.iter().any(Side::going) {
        return false;
    }
    let before = sides.len();
    sides.retain(|side| side.session.as_ref().is_none_or(|s| s == now));
    sides.len() != before
}

/// Whether chat item `id` is a side question's card.
pub fn is_side(id: &str) -> bool {
    id.starts_with(ID)
}

/// A side question's card: one item, its question and its answer under a
/// note that the agent's chat won't keep them.
fn side_items(n: usize, side: &Side) -> Vec<Item> {
    let mut question = Styled::default();
    question.push(&side.question, Some(&Inline::Bold));
    let mut blocks = vec![
        DocBlock::Note("Side question · not kept in the agent's chat".into()),
        DocBlock::Para(question),
    ];
    if !side.lines.is_empty() {
        blocks.extend(md_blocks::blocks(&answer_md(&side.lines)));
    }
    match &side.state {
        SideState::Asked | SideState::Answering if side.lines.is_empty() => {
            blocks.push(DocBlock::Note("answering".into()))
        }
        SideState::Failed(why) => blocks.push(DocBlock::Note(format!("Failed: {why}"))),
        _ => {}
    }
    vec![Item::Assistant {
        id: format!("{ID}{n}"),
        at: Some(side.at),
        blocks: Arc::new(blocks),
    }]
}

/// The answer's lines as markdown: each keeps its own line, as drawn, but
/// for those the agent wrapped to its panel's width, joined again: a row
/// so full that the next one's first word wouldn't have fit on it.
/// shortcut: the widest row stands for the wrap width; a reply of short
/// rows only (under `WRAPS`) is taken as unwrapped.
pub fn answer_md(lines: &[String]) -> String {
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    let mut prev: Option<&str> = None;
    // The row before is a list item, or the rest of one: the agent wraps
    // an item's words onto rows set in under its first.
    let mut item = false;
    for line in lines {
        let first = line.split_whitespace().next().unwrap_or("");
        let listed = first.starts_with(['-', '*', '•', '#', '>', '|'])
            || first.trim_end_matches(['.', ')']).parse::<u32>().is_ok();
        let wrapped = prev.is_some_and(|p| {
            !p.is_empty() && p.chars().count() + 1 + first.chars().count() > width
        });
        let set_in = line.starts_with(' ');
        if wrapped && width >= WRAPS && !first.is_empty() && !listed && (!set_in || item) {
            out.push(' ');
            out.push_str(line.trim_start());
        } else {
            if prev.is_some() {
                out.push_str("  \n");
            }
            out.push_str(line);
            item = listed;
        }
        prev = Some(line);
    }
    out
}

/// The least wrap width rows are joined at.
const WRAPS: usize = 40;

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(screen: &str) -> Vec<&str> {
        screen.lines().collect()
    }

    // As each agent drew it in a 110-column tmux pane (2026-11-25).
    const CLAUDE_DONE: &str = "✻ Cooked for 3s · done 11:05 AM
▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
  /btw what is 2+2? answer in two short paragraphs

    2 + 2 is 4.

    This is basic arithmetic: adding two to two gives four.

  c to copy · f to fork · Esc to close";
    const CLAUDE_LOADING: &str = "▔▔▔▔▔▔▔▔
  /btw what is 2+2? answer in two short paragraphs
    ✽ Answering…
  Esc to close";
    const CLAUDE_LONG: &str = "▔▔▔▔▔▔▔▔
  /btw what is 2+2? answer in two short paragraphs
  /btw list the numbers
    one
    two
    three                                                      ↓
  ↑/↓ to scroll · ⇧←/→ to browse · c to copy · f to fork · x to clear history · Esc to close";
    const OPENCODE: &str = "     They whispered in snowfields beneath a pale sun.
     They c   /btw  what is 2+2? answer in two short paragraphs                              esc
     And sh
     The de
     To dri   2 + 2 is 4.
     A king
     Then v   Answer: 4.
     The re
              copy c  scroll j/k ↑/↓
  ┃  Build · GPT-5.6 Sol OpenAI · high";
    const ANTIGRAVITY_LOADING: &str = "> Write a poem.
 ┃ /btw what is 2+2? answer in two short paragraphs
 ┃
 ┃ ⣾  Working... (Enter/Esc to cancel)
──────────────────────────────────────
>";
    const ANTIGRAVITY_CUT: &str = " ┃ /btw list the numbers
 ┃
 ┃   one
 ┃   two
 ┃ ... (truncated) — Tab to expand · ←/→ to navigate · Enter/Esc to dismiss
──────────────────────────────────────";
    const CURSOR: &str = "  16. A heron's patience outlasts the current's boast.
   /btw what is 2+2? answer in two short paragraphs
   2 + 2 is 4.
   That follows ordinary integer arithmetic: two plus two equals four.
 Press any key to dismiss";

    fn lines(of: &[&str]) -> Vec<String> {
        of.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_answer_that_says_btw_is_no_question_row() {
        let screen = "  /btw is this working now?
  /btw does this work?

    It came as a /btw side question.

    /btw shows it on the side.

  ⇧←/→ to browse · c to copy · f to fork · x to clear history · Esc to close";
        let p = panel(&rows(screen)).expect("claude");
        assert_eq!(p.question, "does this work?");
        assert_eq!(
            p.lines,
            lines(&["It came as a /btw side question.", "", "/btw shows it on the side."])
        );
    }

    #[test]
    fn each_agents_panel_reads_as_its_question_and_answer() {
        let p = panel(&rows(CLAUDE_DONE)).expect("claude");
        assert_eq!(p.question, "what is 2+2? answer in two short paragraphs");
        assert_eq!(
            p.lines,
            lines(&["2 + 2 is 4.", "", "This is basic arithmetic: adding two to two gives four."])
        );
        assert!(!p.working && !p.scrolls && !p.cut);

        let p = panel(&rows(OPENCODE)).expect("opencode");
        assert_eq!(p.question, "what is 2+2? answer in two short paragraphs");
        assert_eq!(p.lines, lines(&["2 + 2 is 4.", "", "Answer: 4."]));
        assert!(p.scrolls);

        let p = panel(&rows(CURSOR)).expect("cursor");
        assert_eq!(p.lines[0], "2 + 2 is 4.");
        assert_eq!(p.lines.len(), 2);

        let p = panel(&rows(ANTIGRAVITY_CUT)).expect("antigravity");
        assert_eq!(p.question, "list the numbers");
        assert_eq!(p.lines, lines(&["one", "two"]));
        assert!(p.cut);

        // Its footer wrapped on a narrow pane, after a Tab.
        let narrow = " ┃ /btw list the numbers\n ┃\n ┃   one\n ┃   two\n ┃ ↑/↓ to scroll (1-31 of 40) — Tab to collapse · ←/→ to navigate ·\n ┃ Enter/Esc to dismiss\n────\n>";
        let p = panel(&rows(narrow)).expect("narrow");
        assert_eq!(p.lines, lines(&["one", "two"]));
        assert!(p.scrolls && !p.cut);
    }

    #[test]
    fn a_spinner_is_no_answer_and_claudes_last_question_is_the_one() {
        assert!(panel(&rows(CLAUDE_LOADING)).expect("loading").working);
        assert!(panel(&rows(ANTIGRAVITY_LOADING)).expect("loading").working);
        // Antigravity's answer as it comes, its spinner under it.
        let coming = " ┃ /btw name rivers\n ┃\n ┃   Danube\n ┃   Rhine\n ┃ ⣯  Generating... (Enter/Esc to cancel)\n────\n>";
        assert!(panel(&rows(coming)).expect("coming").working);
        // A bullet is no spinner.
        assert!(!spinner("* one"));
        assert!(!spinner("· two"));
        let p = panel(&rows(CLAUDE_LONG)).expect("long");
        assert_eq!(p.question, "list the numbers");
        // Its scroll mark is no part of the answer.
        assert_eq!(p.lines, lines(&["one", "two", "three"]));
        assert!(p.scrolls);
        // No panel: a screen without one, or a `/btw` in the chat's text.
        assert_eq!(panel(&rows("hello\n> /btw ")), None);
        assert_eq!(panel(&["say /btw now", "and more"]), None);
    }

    #[test]
    fn only_btw_with_words_is_a_side_question() {
        assert_eq!(question(" /btw  what now "), Some("what now"));
        assert_eq!(question("/btw"), None);
        assert_eq!(question("/btwx y"), None);
        assert_eq!(question("hi /btw y"), None);
    }

    #[test]
    fn a_new_session_leaves_its_side_questions_behind() {
        let session = |v: &str| AgentSession {
            source: "herdr".into(),
            agent: "claude".into(),
            kind: "id".into(),
            value: v.into(),
        };
        let asked = |s: Option<&str>, state: SideState| Side {
            session: s.map(session),
            state,
            ..Side::new("q", 1)
        };
        let mut sides = vec![
            asked(Some("a"), SideState::Answered),
            asked(None, SideState::Answered),
        ];
        // Same session, or none known: kept.
        assert!(!drop_left(&mut sides, Some(&session("a"))));
        assert!(!drop_left(&mut sides, None));
        // One still read holds them all back, until it is done.
        sides.push(asked(Some("b"), SideState::Answering));
        assert!(!drop_left(&mut sides, Some(&session("c"))));
        sides[2].state = SideState::Answered;
        assert!(drop_left(&mut sides, Some(&session("c"))));
        assert_eq!(sides, vec![asked(None, SideState::Answered)]);
    }

    #[test]
    fn scrolled_windows_join_where_they_overlap() {
        let mut read = lines(&["a", "b", "c", "d"]);
        assert!(merge(&mut read, &lines(&["c", "d", "e", "f"])));
        assert_eq!(read, lines(&["a", "b", "c", "d", "e", "f"]));
        // The same window again, or one inside: nothing new.
        assert!(!merge(&mut read, &lines(&["d", "e", "f"])));
        assert!(!merge(&mut read, &lines(&["b", "c"])));
        assert_eq!(read.len(), 6);
    }

    fn take(f: &mut Follow, screen: &str, t: Instant) -> Step {
        f.took(panel(&rows(screen)), t)
    }

    #[test]
    fn an_answer_counts_once_it_stays_then_its_panel_closes() {
        let t = Instant::now();
        let side = Side::new("what is 2+2? answer in two short paragraphs", 1);
        let mut f = Follow::new(side, t);
        assert_eq!(f.took(None, t), Step::Wait);
        assert_eq!(f.side.state, SideState::Asked);
        assert_eq!(take(&mut f, CLAUDE_LOADING, t), Step::Wait);
        assert_eq!(f.side.state, SideState::Answering);
        assert_eq!(take(&mut f, CLAUDE_DONE, t), Step::Wait);
        for _ in 1..STILL {
            assert_eq!(take(&mut f, CLAUDE_DONE, t), Step::Wait);
        }
        assert_eq!(take(&mut f, CLAUDE_DONE, t), Step::Close("esc"));
        assert_eq!(f.side.state, SideState::Answered);
        assert_eq!(f.side.lines[0], "2 + 2 is 4.");
    }

    #[test]
    fn a_long_answer_is_scrolled_through_and_read_whole() {
        let t = Instant::now();
        let mut f = Follow::new(Side::new("list the numbers", 1), t);
        for _ in 0..STILL {
            assert_eq!(take(&mut f, CLAUDE_LONG, t), Step::Wait);
        }
        assert_eq!(take(&mut f, CLAUDE_LONG, t), Step::Press(DOWN));
        let next = CLAUDE_LONG.replace("    one\n    two\n", "    two\n").replace(
            "    three                                                      ↓",
            "    three\n    four",
        );
        assert_eq!(take(&mut f, &next, t), Step::Press(DOWN));
        assert_eq!(take(&mut f, &next, t), Step::Wait);
        assert_eq!(take(&mut f, &next, t), Step::Close("esc"));
        assert_eq!(f.side.lines, lines(&["one", "two", "three", "four"]));
    }

    #[test]
    fn antigravity_is_asked_for_all_of_it_first() {
        let t = Instant::now();
        let mut f = Follow::new(Side::new("list the numbers", 1), t);
        assert_eq!(take(&mut f, ANTIGRAVITY_CUT, t), Step::Press(&["Tab"]));
        assert_eq!(take(&mut f, ANTIGRAVITY_CUT, t), Step::Wait);
    }

    #[test]
    fn a_panel_closed_or_never_shown_ends_it() {
        let t = Instant::now();
        let mut f = Follow::new(Side::new("what is 2+2? answer in two short paragraphs", 1), t);
        assert_eq!(take(&mut f, CLAUDE_DONE, t), Step::Wait);
        // A read that misses it once is no close.
        for _ in 1..GONE {
            assert_eq!(f.took(None, t), Step::Wait);
        }
        assert_eq!(f.took(None, t), Step::Done);
        assert_eq!(f.side.state, SideState::Answered);

        let mut f = Follow::new(Side::new("q", 1), t);
        assert_eq!(f.took(None, t + SHOW_WAIT - Duration::from_secs(1)), Step::Wait);
        assert_eq!(f.took(None, t + SHOW_WAIT), Step::Done);
        assert!(matches!(f.side.state, SideState::Failed(_)));
        // Another question's panel is none of its.
        let mut f = Follow::new(Side::new("something else", 1), t);
        assert_eq!(take(&mut f, CLAUDE_DONE, t), Step::Wait);
        assert_eq!(f.side.state, SideState::Asked);
    }

    #[test]
    fn a_long_question_wrapped_onto_the_answers_rows_is_left_out() {
        let screen = "  /btw why is the sky blue and what
  does that say about sunsets
    Light scatters.
  Esc to close";
        let f = Follow::new(Side::new("why is the sky blue and what does that say about sunsets", 1), Instant::now());
        let p = panel(&rows(screen)).expect("panel");
        assert!(f.ours(&p));
        assert_eq!(f.answer(&p), lines(&["Light scatters."]));
    }

    #[test]
    fn rows_the_agent_wrapped_are_joined_again() {
        let wrapped = lines(&[
            "On a clear day, the sky looks blue. Sunlight contains all colours, and",
            "air molecules scatter blue light more.",
            "",
            "- one item",
            "- another",
        ]);
        assert_eq!(
            answer_md(&wrapped),
            "On a clear day, the sky looks blue. Sunlight contains all colours, and \
             air molecules scatter blue light more.  \n  \n- one item  \n- another"
        );
        // An item's words wrapped onto rows set in under it: one item.
        let item = lines(&[
            "- Not restarted: the old parser still runs. It breaks only when an",
            "  answer contains the command word.",
            "- next",
        ]);
        assert_eq!(
            answer_md(&item),
            "- Not restarted: the old parser still runs. It breaks only when an \
             answer contains the command word.  \n- next"
        );
        // Short rows are its own lines, as a list of words is.
        assert_eq!(answer_md(&lines(&["twenty-two", "twenty-three"])), "twenty-two  \ntwenty-three");
    }

    fn user(id: &str, at: u64) -> Item {
        Item::User {
            id: id.into(),
            at: Some(at),
            text: "go".into(),
        }
    }

    #[test]
    fn side_rows_go_where_they_were_asked_and_replace_their_old_ones() {
        let items = Arc::new(vec![user("u1", 10), user("u2", 30)]);
        let mut side = Side::new("why", 20);
        let one = with_sides(&items, std::slice::from_ref(&side));
        let ids: Vec<&str> = one.iter().map(Item::id).collect();
        assert_eq!(ids, ["u1", "side:0", "u2"]);
        side.lines = lines(&["because"]);
        side.state = SideState::Answered;
        let two = with_sides(&one, &[side]);
        let ids: Vec<&str> = two.iter().map(Item::id).collect();
        assert_eq!(ids, ["u1", "side:0", "u2"]);
        // None: the chat as it was, the same list when it had none.
        assert!(Arc::ptr_eq(&with_sides(&items, &[]), &items));
        assert_eq!(with_sides(&two, &[]).len(), 2);
    }
}
