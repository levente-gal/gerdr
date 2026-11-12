//! Small text helpers for what programs print.

/// The last line of `text` with something on it, trimmed.
pub fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

/// The last `n` lines of `text` with something on them, in order.
pub fn last_lines(text: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_skip_blanks() {
        let text = "a\n\n  b \nc\n\n \n";
        assert_eq!(last_line(text), Some("c"));
        assert_eq!(last_line(" \n"), None);
        assert_eq!(last_lines(text, 2), ["  b ", "c"]);
        assert_eq!(last_lines(text, 9), ["a", "  b ", "c"]);
    }
}
