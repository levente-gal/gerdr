//! What Claude Code's screen shows: its mode under its reply box, and what
//! it suggests saying next in it.

use crate::scrollback::Styled;

/// The mode Claude Code's `rows` (as `chat::mode::shown` gives them) show.
pub(crate) fn mode_shown(rows: &[&str]) -> Option<String> {
    rows.iter().take(3).find_map(|row| {
        [
            ("plan mode on", "Plan"),
            ("accept edits on", "Edits"),
            ("auto mode on", "Auto"),
            ("manual mode on", "Manual"),
            ("bypass permissions on", "Bypass"),
        ]
        .into_iter()
        .find_map(|(says, mode)| row.contains(says).then(|| mode.to_owned()))
    })
}

/// What Claude Code suggests saying next, as its screen shows it: the
/// last `❯` line, its reply box, holding dim text only (what the user
/// types there is plain).
pub(crate) fn suggestion(rows: &[(String, Styled)]) -> Option<String> {
    let (text, styles) = rows.iter().rev().find(|(text, _)| text.starts_with('❯'))?;
    let chars: Vec<char> = text.chars().collect();
    let start = (chars.iter())
        .take_while(|c| **c == '❯' || c.is_whitespace())
        .count();
    let dim = |i: usize| (styles.iter()).any(|(at, style)| at.contains(&i) && style.dim);
    let words: String = chars[start..]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned();
    let ghost = (start..chars.len()).all(|i| chars[i].is_whitespace() || dim(i));
    (ghost && !words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggestion_is_the_dim_text_of_the_reply_line_only() {
        let read = |ansi: &str| suggestion(&crate::scrollback::parse_ansi(ansi));
        // As Claude Code 2.1.332 drew it under herdr: the chat's own `❯`
        // lines above, then its reply box.
        let rule = "\x1b[0m\x1b[38;2;138;138;138m────\x1b[0m\r\n";
        let screen = format!(
            "❯ Create hello.txt\r\n● Done.\r\n{rule}❯\u{a0}\x1b[0m\x1b[2mcommit this\x1b[0m\r\n{rule}  \x1b[0m\x1b[38;2;150;108;30m⏵⏵ auto mode on\x1b[0m\n"
        );
        assert_eq!(read(&screen).as_deref(), Some("commit this"));
        // Typed by the user: plain. Nothing in the box: none.
        assert_eq!(read("❯\u{a0}com\r\n"), None);
        assert_eq!(read("❯\u{a0}\r\n"), None);
        assert_eq!(read("❯ \x1b[2mcom\x1b[0mmit\r\n"), None);
    }
}
