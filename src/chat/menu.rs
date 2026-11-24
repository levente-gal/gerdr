//! A menu an agent shows while it waits on the user (Claude Code's
//! questions, its approvals), read from the pane's screen rows so the
//! waiting panel can show it as gerdr's own: Claude writes a question to
//! its transcript only once it is answered.

use crate::scrollback::Styled;

/// What the menu asks and the choices it offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    /// The questions' tabs, when it asks more than one.
    pub tabs: Vec<Tab>,
    /// What it asks, a paragraph a line, bold where the screen has it bold.
    pub ask: Vec<(String, bool)>,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub label: String,
    /// Answered (☒), or a step that is no question (✔ Submit).
    pub done: bool,
    /// The one asked now: drawn on a ground of its own.
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Its number, the key that picks it.
    pub n: u8,
    pub label: String,
    /// The rows under it that say more, joined.
    pub detail: String,
    /// The one the menu's arrow is on.
    pub picked: bool,
}

/// The most rows above the choices read as what the menu asks.
const ASK_ROWS: usize = 12;
/// The most rows between two choices: more, and they are no one menu.
const CHOICE_GAP: usize = 8;
/// How far short of the screen's widest row its text wraps: the agent
/// keeps a column or two clear at the right.
const WRAP_SLACK: usize = 4;

/// The menu at the bottom of `rows`, if they end in one: numbered choices
/// from 1 up, one of them under the menu's arrow (❯), so a numbered list
/// the agent wrote is no menu.
pub fn menu(rows: &[(String, Styled)]) -> Option<Menu> {
    let width = rows.iter().map(|(t, _)| t.chars().count()).max()?;
    let mut run: Vec<(usize, Numbered)> = Vec::new();
    for (i, (text, _)) in rows.iter().enumerate() {
        let Some(row) = numbered(text) else {
            continue;
        };
        let follows = run
            .last()
            .is_some_and(|(at, prev)| row.n == prev.n + 1 && i - at <= CHOICE_GAP);
        if row.n == 1 {
            run = vec![(i, row)];
        } else if follows {
            run.push((i, row));
        } else {
            run.clear();
        }
    }
    if run.len() < 2 || !run.iter().any(|(_, row)| row.picked) {
        return None;
    }
    let ends: Vec<usize> = (run.iter().skip(1).map(|(at, _)| *at))
        .chain([rows.len()])
        .collect();
    let choices = (run.iter().zip(ends))
        .map(|((at, row), end)| {
            let under: Vec<&str> = (rows[at + 1..end].iter())
                .map(|(text, _)| text.as_str())
                .take_while(|text| indent(text) >= row.col && !text.trim().is_empty())
                .map(str::trim)
                .collect();
            Choice {
                n: row.n,
                label: row.label.clone(),
                detail: under.join(" "),
                picked: row.picked,
            }
        })
        .collect();

    // What it asks: the rows above its first choice, up to a rule or the
    // questions' tabs.
    let first = run[0].0;
    let mut top = first;
    while top > 0 && first - top < ASK_ROWS {
        let text = &rows[top - 1].0;
        if is_rule(text) || is_tabs(text) {
            break;
        }
        top -= 1;
    }
    let ask = joined(
        rows[top..first]
            .iter()
            .map(|(text, styles)| (text.as_str(), bold(text, styles))),
        width,
    );
    let tabs = (top.checked_sub(1))
        .map(|i| &rows[i])
        .filter(|(text, _)| is_tabs(text))
        .map(|(text, styles)| tabs_of(text, styles))
        .unwrap_or_default();
    Some(Menu { tabs, ask, choices })
}

/// A choice's row: "❯ 2. Label" or "  2. Label".
struct Numbered {
    n: u8,
    label: String,
    picked: bool,
    /// Where its label starts, in characters: the rows that say more about
    /// it start there too.
    col: usize,
}

fn numbered(text: &str) -> Option<Numbered> {
    let rest = text.trim_start();
    let (picked, rest) = match rest.strip_prefix(['❯', '›', '>']) {
        Some(after) => (true, after.trim_start()),
        None => (false, rest),
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let n: u8 = rest.get(..digits)?.parse().ok()?;
    let label = rest[digits..].strip_prefix(". ")?.trim();
    if n == 0 || label.is_empty() {
        return None;
    }
    let col = text.chars().count() - rest[digits + 2..].trim_start().chars().count();
    Some(Numbered {
        n,
        label: label.to_owned(),
        picked,
        col,
    })
}

fn indent(text: &str) -> usize {
    text.chars().take_while(|c| c.is_whitespace()).count()
}

/// A row that is a line across: the menu's edge.
fn is_rule(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '━' | '═' | '╌'))
}

/// The marks of the questions' tabs: not yet answered, answered, a step.
const TAB_MARKS: [char; 5] = ['☐', '☒', '☑', '✔', '✓'];

/// A row of the questions' tabs: "←  ☒ What  ☐ How  ✔ Submit  →".
fn is_tabs(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with('←') || t.ends_with('→')) && t.contains(TAB_MARKS)
}

fn tabs_of(text: &str, styles: &Styled) -> Vec<Tab> {
    let grounded = |i: usize| styles.iter().any(|(r, s)| s.ground && r.contains(&i));
    let mut tabs: Vec<Tab> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        if TAB_MARKS.contains(&c) {
            tabs.push(Tab {
                label: String::new(),
                done: c != '☐',
                current: grounded(i),
            });
        } else if let Some(tab) = tabs.last_mut()
            && c != '→'
        {
            tab.label.push(c);
            tab.current |= !c.is_whitespace() && grounded(i);
        }
    }
    for tab in &mut tabs {
        tab.label = tab.label.trim().to_owned();
    }
    tabs.retain(|tab| !tab.label.is_empty());
    tabs
}

/// Whether row `text` is bold where its words start.
fn bold(text: &str, styles: &Styled) -> bool {
    let start = indent(text);
    styles.iter().any(|(r, s)| s.bold && r.contains(&start))
}

/// `rows` as paragraphs: the rows the screen wrapped (`width` wide) joined
/// again, the others each its own, blank ones left out. A paragraph is
/// bold as its first row.
fn joined<'a>(rows: impl Iterator<Item = (&'a str, bool)>, width: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    // How long the row before was, 0 after a blank one.
    let mut before = 0;
    for (text, bold) in rows {
        let t = text.trim();
        // The screen moved a word down when it didn't fit the row before.
        let first = t.split_whitespace().next().map_or(0, |w| w.chars().count());
        let wrapped = before > 0 && before + 1 + first + WRAP_SLACK > width;
        before = text.trim_end().chars().count();
        match out.last_mut() {
            _ if t.is_empty() => before = 0,
            Some((para, _)) if wrapped => {
                para.push(' ');
                para.push_str(t);
            }
            _ => out.push((t.to_owned(), bold)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollback::parse_ansi;

    /// Claude Code asking its second question of three, as herdr reads the
    /// screen, the current tab on a ground of its own.
    fn asking() -> Vec<(String, Styled)> {
        let rule = "─".repeat(120);
        let text = format!(
            "● Agent \"Find font size\" finished · 53s\n\
             \n  Searched for 1 pattern, ran 2 shell commands\n\
             {rule}\n\
             Planning: /home/u/.claude/plans/x.md\n\
             {rule}\n\
             ←  ☒ What grows  \x1b[48;2;87;105;247m☐ GUI control\x1b[0m  ✔ Submit  →\n\
             \n\
             \x1b[1mgerdr is the Wayland compositor, so the cheapest fix is to scale its output (like a desktop's display scale). That\x1b[0m\n\
             \x1b[1mmakes everything bigger. Which do you want?\x1b[0m\n\
             \n\
             ❯ 1. Switcher rows + keys (Recommended)\n\
             \x20    Switcher rows 'Bigger UI', 'Smaller UI', 'Reset UI size' (steps of 10%), plus Prefix keys (e.g. Prefix + /\n\
             \x20    Prefix -). Saved to config.toml.\n\
             \x20 2. Switcher rows only\n\
             \x20    Only the three switcher rows, no new keys.\n\
             \x20 3. Type something.\n\
             {rule}\n\
             \x20 4. Chat about this\n\
             \n\
             Enter to select · Tab/Arrow keys to navigate · Esc to cancel\n"
        );
        parse_ansi(&text)
    }

    #[test]
    fn a_question_reads_as_its_tabs_ask_and_choices() {
        let m = menu(&asking()).expect("a menu");
        let tabs: Vec<(&str, bool, bool)> = (m.tabs.iter())
            .map(|t| (t.label.as_str(), t.done, t.current))
            .collect();
        assert_eq!(
            tabs,
            [
                ("What grows", true, false),
                ("GUI control", false, true),
                ("Submit", true, false)
            ]
        );
        assert_eq!(m.ask.len(), 1, "{:?}", m.ask);
        assert!(m.ask[0].1, "bold");
        assert!(
            m.ask[0]
                .0
                .ends_with("scale). That makes everything bigger. Which do you want?")
        );
        let choices: Vec<(u8, &str, bool)> = (m.choices.iter())
            .map(|c| (c.n, c.label.as_str(), c.picked))
            .collect();
        assert_eq!(
            choices,
            [
                (1, "Switcher rows + keys (Recommended)", true),
                (2, "Switcher rows only", false),
                (3, "Type something.", false),
                (4, "Chat about this", false),
            ]
        );
        assert!(
            m.choices[0]
                .detail
                .ends_with("Prefix + / Prefix -). Saved to config.toml.")
        );
        assert_eq!(
            m.choices[1].detail,
            "Only the three switcher rows, no new keys."
        );
        assert_eq!(m.choices[2].detail, "");
        assert_eq!(m.choices[3].detail, "");
    }

    #[test]
    fn an_approval_asks_its_lines_apart() {
        let text = "\x1b[1mBash command\x1b[0m\n\n  rm -rf target\n  Remove the build\n\nDo you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No\n\nEsc to cancel\n";
        let m = menu(&parse_ansi(text)).expect("a menu");
        assert!(m.tabs.is_empty());
        let ask: Vec<(&str, bool)> = m.ask.iter().map(|(t, b)| (t.as_str(), *b)).collect();
        assert_eq!(
            ask,
            [
                ("Bash command", true),
                ("rm -rf target", false),
                ("Remove the build", false),
                ("Do you want to proceed?", false),
            ]
        );
        assert_eq!(m.choices.len(), 3);
        assert_eq!(m.choices[2].detail, "");
    }

    #[test]
    fn a_numbered_list_without_the_arrow_is_no_menu() {
        let text = "Steps:\n  1. Build it\n  2. Test it\n";
        assert_eq!(menu(&parse_ansi(text)), None);
        // Choices not counted from 1 aren't one either.
        assert_eq!(menu(&parse_ansi("❯ 2. Two\n  3. Three\n")), None);
        assert_eq!(menu(&[]), None);
    }
}
