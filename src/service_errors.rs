//! Errors in the output of the services gerdr runs (web servers, workers):
//! which lines are errors, how a traceback groups into one, what is the
//! same error again, and where reading left off. Pure: `service_watch`
//! feeds it the panes' text.

use serde::{Deserialize, Serialize};

/// The regex herdr's `pane.output_matched` rings on: the same lines
/// `is_error` takes for errors.
pub const HERDR_PATTERN: &str = r#"Traceback \(most recent call last\)|panicked at|Internal Server Error| raised unexpected|Unhandled (exception|rejection)|UnhandledPromiseRejection|\b(ERROR|CRITICAL|FATAL)\b|" 5[0-9][0-9] "#;

/// What starts an error, anywhere in a line.
const MARKS: [&str; 7] = [
    "Traceback (most recent call last)",
    "panicked at",
    "Internal Server Error",
    " raised unexpected",
    "Unhandled exception",
    "Unhandled rejection",
    "UnhandledPromiseRejection",
];

/// Log levels that start an error, as whole words.
const LEVELS: [&str; 3] = ["ERROR", "CRITICAL", "FATAL"];

/// The most lines an error keeps.
const MAX_LINES: usize = 200;
/// How many lines an anchor hashes.
const ANCHOR_LINES: usize = 4;
/// The longest title.
const TITLE_MAX: usize = 160;

/// Where reading a pane's output left off: hashes of its last lines, and
/// of the lines before its last one (should the last have grown since,
/// being written when it was read).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub tail: u64,
    pub prev: u64,
}

/// An error a service printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    /// What makes it the same error again: its kind and where it came
    /// from, numbers left out.
    pub fingerprint: String,
    /// Its line in short: a traceback's exception, else its first line.
    pub title: String,
    /// All of it, as printed.
    pub lines: Vec<String>,
}

/// What a read of a pane's output found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// The errors past the anchor, in order.
    pub errors: Vec<ServiceError>,
    /// Where this read leaves off; `None` for no output.
    pub anchor: Option<Anchor>,
    /// Whether an error line (or the command, should it look like one) is
    /// among the last `window` lines: herdr won't ring again until none is.
    pub dirty: bool,
    /// Whether the command's line is among the last `window` lines.
    pub command_shown: bool,
}

/// Whether `line` starts an error.
pub fn is_error(line: &str) -> bool {
    MARKS.iter().any(|m| line.contains(m))
        || LEVELS.iter().any(|w| has_word(line, w))
        || server_error_status(line)
}

/// Whether `word` is in `line` as a whole word.
fn has_word(line: &str, word: &str) -> bool {
    let bytes = line.as_bytes();
    let word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    line.match_indices(word).any(|(at, _)| {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + word.len()).copied();
        !before.is_some_and(word_char) && !after.is_some_and(word_char)
    })
}

/// Whether `line` is an access log's line of a 5xx answer: `" 500 `.
fn server_error_status(line: &str) -> bool {
    line.match_indices("\" 5").any(|(at, _)| {
        let rest = &line.as_bytes()[at + 3..];
        rest.len() >= 3 && rest[0].is_ascii_digit() && rest[1].is_ascii_digit() && rest[2] == b' '
    })
}

/// Whether `line` is the shell's line of `command` being run: the prompt,
/// then the command.
pub fn is_command_line(line: &str, command: &str) -> bool {
    let command = command.trim();
    !command.is_empty() && line.trim_end().ends_with(command)
}

/// Read `text`, a pane's last lines, for the errors after `anchor` (all
/// of them without one), leaving out the line of `command`. `window` is
/// how many lines herdr watches.
pub fn scan(text: &str, command: &str, anchor: Option<&Anchor>, window: usize) -> Scan {
    let lines = split(text);
    let n = lines.len();
    if n == 0 {
        return Scan::default();
    }
    let start = anchor.map_or(0, |a| resume_at(&lines, a));
    let tail_from = n.saturating_sub(window);
    let mut scan = Scan {
        anchor: Some(anchor_of(&lines)),
        ..Scan::default()
    };
    for line in &lines[tail_from..] {
        let command_line = is_command_line(line, command);
        scan.command_shown |= command_line;
        scan.dirty |= is_error(line);
    }
    let mut i = 0;
    while i < n {
        if !is_error(lines[i]) || is_command_line(lines[i], command) {
            i += 1;
            continue;
        }
        let end = block_end(&lines, i);
        // One begun before the anchor was read already, all or part.
        if i >= start {
            scan.errors.push(error_of(&lines[i..end]));
        }
        i = end;
    }
    scan
}

/// `text`'s lines, without the blank ones at its end.
fn split(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Where lines `lines` stop being read: hashes of the last
/// `ANCHOR_LINES`, and of those before the last.
fn anchor_of(lines: &[&str]) -> Anchor {
    let n = lines.len();
    Anchor {
        tail: hash_lines(&lines[n.saturating_sub(ANCHOR_LINES)..]),
        prev: hash_lines(&lines[n.saturating_sub(ANCHOR_LINES + 1)..n - 1]),
    }
}

/// The first line after `anchor` in `lines`: after the last place its
/// tail hashes the same, else its lines before the last (which grew
/// since); else all is new (what it hashed scrolled away).
fn resume_at(lines: &[&str], anchor: &Anchor) -> usize {
    let ends = |hash: u64| {
        (1..=lines.len())
            .rev()
            .find(|&end| hash_lines(&lines[end.saturating_sub(ANCHOR_LINES)..end]) == hash)
    };
    // Else the line last read was the last, being written: read it again.
    ends(anchor.tail).or_else(|| ends(anchor.prev)).unwrap_or(0)
}

fn hash_lines(lines: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for line in lines {
        for b in line.trim_end().bytes().chain(*b"\n") {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Where the error begun at line `i` ends: its traceback's lines, the
/// indented ones under it, an access log's line of the 5xx it answered.
fn block_end(lines: &[&str], i: usize) -> usize {
    let mut in_traceback = is_traceback_start(lines[i]);
    let mut j = i + 1;
    while j < lines.len() && j - i < MAX_LINES {
        let line = lines[j];
        let trimmed = line.trim();
        if is_traceback_start(line) || is_chain(trimmed) {
            in_traceback = true;
        } else if line.starts_with([' ', '\t']) && !trimmed.is_empty() {
        } else if in_traceback && !trimmed.is_empty() {
            // The exception's line ends the traceback, unless another is
            // chained to it.
            let chained = lines
                .get(j + 1..)
                .into_iter()
                .flatten()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| is_chain(l.trim()));
            if !chained {
                j += 1;
                break;
            }
            // On through the blank lines to the next.
        } else if trimmed.is_empty() && in_traceback {
        } else {
            break;
        }
        j += 1;
    }
    // The answer it was: Django prints its access line after.
    if j < lines.len() && server_error_status(lines[j]) && !server_error_status(lines[i]) {
        j += 1;
    }
    j
}

fn is_traceback_start(line: &str) -> bool {
    line.contains("Traceback (most recent call last)")
}

/// The line between two chained exceptions.
fn is_chain(trimmed: &str) -> bool {
    trimmed.starts_with("During handling of the above exception")
        || trimmed.starts_with("The above exception was the direct cause")
}

/// The error printed as `lines`.
fn error_of(lines: &[&str]) -> ServiceError {
    let traceback = lines.iter().any(|l| is_traceback_start(l));
    // A traceback's exception: its last line not indented.
    let exception = traceback
        .then(|| {
            lines
                .iter()
                .rev()
                .find(|l| {
                    let t = l.trim();
                    !t.is_empty()
                        && !l.starts_with([' ', '\t'])
                        && !is_traceback_start(l)
                        && !server_error_status(l)
                        && !is_chain(t)
                })
                .copied()
        })
        .flatten();
    let title = exception.unwrap_or(lines[0]).trim();
    let kind = match exception {
        Some(e) => e.split(':').next().unwrap_or(e).trim().to_owned(),
        None => normalize(lines[0]),
    };
    // Where it came from: a traceback's last frame.
    let frame = lines
        .iter()
        .rev()
        .find(|l| l.trim_start().starts_with("File \""))
        .map(|l| l.trim().to_owned())
        .unwrap_or_default();
    ServiceError {
        fingerprint: format!("{:016x}", hash_lines(&[&kind, &frame])),
        title: shorten(title),
        lines: lines.iter().map(|l| (*l).to_owned()).collect(),
    }
}

/// `line` with its numbers and hex ids left out: what stays the same
/// when the same error comes again.
fn normalize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_number = false;
    for ch in line.trim().chars() {
        if ch.is_ascii_digit() {
            if !in_number {
                out.push('#');
            }
            in_number = true;
        } else {
            in_number = false;
            out.push(ch);
        }
    }
    out
}

fn shorten(text: &str) -> String {
    match text.char_indices().nth(TITLE_MAX) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DJANGO: &str = "\
Watching for file changes with StatReloader
[01/Nov/2026 10:00:00] \"GET / HTTP/1.1\" 200 512
Internal Server Error: /boom
Traceback (most recent call last):
  File \"/app/views.py\", line 12, in boom
    raise ValueError(\"bad\")
ValueError: bad
[01/Nov/2026 10:00:01] \"GET /boom HTTP/1.1\" 500 145
[01/Nov/2026 10:00:02] \"GET / HTTP/1.1\" 200 512";

    #[test]
    fn matches_error_lines() {
        assert!(is_error(
            "[2026-11-01 10:00:00,000: ERROR/MainProcess] Task t raised unexpected: X"
        ));
        assert!(is_error("thread 'main' panicked at src/main.rs:1:1:"));
        assert!(is_error("\"GET /x HTTP/1.1\" 502 0"));
        assert!(!is_error("\"GET /x HTTP/1.1\" 200 0"));
        assert!(!is_error("0 ERRORS found"));
        assert!(!is_error("no_ERROR here"));
        assert!(is_error("level ERROR: x"));
    }

    #[test]
    fn django_crash_is_one_error() {
        let scan = scan(DJANGO, "python manage.py runserver", None, 20);
        assert_eq!(scan.errors.len(), 1, "{:?}", scan.errors);
        let e = &scan.errors[0];
        assert_eq!(e.title, "ValueError: bad");
        assert_eq!(e.lines.len(), 6);
        assert!(e.lines[5].contains("500"));
        assert!(scan.dirty);
    }

    #[test]
    fn same_error_same_fingerprint() {
        let a = scan(DJANGO, "", None, 20);
        let other = DJANGO.replace("10:00:01", "11:22:33");
        let b = scan(&other, "", None, 20);
        assert_eq!(a.errors[0].fingerprint, b.errors[0].fingerprint);
        let c = scan("[10:00:01] ERROR job 17 failed", "", None, 20);
        let d = scan("[11:12:13] ERROR job 99 failed", "", None, 20);
        assert_eq!(c.errors[0].fingerprint, d.errors[0].fingerprint);
        assert_ne!(a.errors[0].fingerprint, c.errors[0].fingerprint);
    }

    #[test]
    fn command_line_is_no_error() {
        let text = "❯ celery -A app worker --loglevel=ERROR\nstarted\n";
        let scan = scan(text, "celery -A app worker --loglevel=ERROR", None, 20);
        assert!(scan.errors.is_empty());
        assert!(scan.command_shown);
        // herdr still sees it.
        assert!(scan.dirty);
    }

    #[test]
    fn reads_on_from_the_anchor() {
        let first = "a\nb\nERROR one\nc\nd";
        let s1 = scan(first, "", None, 20);
        assert_eq!(s1.errors.len(), 1);
        let second = format!("{first}\ne\nERROR two\nf");
        let s2 = scan(&second, "", s1.anchor.as_ref(), 20);
        assert_eq!(s2.errors.len(), 1);
        assert_eq!(s2.errors[0].title, "ERROR two");
        let s3 = scan(&second, "", s2.anchor.as_ref(), 20);
        assert!(s3.errors.is_empty());
    }

    #[test]
    fn last_line_grown_is_read_again() {
        let first = "a\nb\nc\nd\nERR";
        let s1 = scan(first, "", None, 20);
        assert!(s1.errors.is_empty());
        let second = "a\nb\nc\nd\nERROR boom\nx";
        let s2 = scan(second, "", s1.anchor.as_ref(), 20);
        assert_eq!(s2.errors.len(), 1);
    }

    #[test]
    fn anchor_scrolled_away_reads_all() {
        let s1 = scan("x1\nx2\nx3\nx4", "", None, 20);
        let s2 = scan("y1\nERROR new\ny2", "", s1.anchor.as_ref(), 20);
        assert_eq!(s2.errors.len(), 1);
    }

    #[test]
    fn traceback_begun_before_anchor_is_not_read_again() {
        let first = "Internal Server Error: /x";
        let s1 = scan(first, "", None, 20);
        assert_eq!(s1.errors.len(), 1);
        let second = "Internal Server Error: /x\nTraceback (most recent call last):\n  File \"a\", line 1\nKeyError: 'k'";
        let s2 = scan(second, "", s1.anchor.as_ref(), 20);
        assert!(s2.errors.is_empty(), "{:?}", s2.errors);
    }

    #[test]
    fn chained_tracebacks_are_one_error() {
        let text = "\
Traceback (most recent call last):
  File \"a\", line 1, in f
KeyError: 'k'

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File \"b\", line 2, in g
RuntimeError: wrapped
next line";
        let s = scan(text, "", None, 20);
        assert_eq!(s.errors.len(), 1, "{:?}", s.errors);
        assert_eq!(s.errors[0].title, "RuntimeError: wrapped");
    }

    #[test]
    fn clean_window_is_not_dirty() {
        let mut text = String::from("ERROR old\n");
        for i in 0..25 {
            text.push_str(&format!("line {i}\n"));
        }
        let s = scan(&text, "", None, 20);
        assert!(!s.dirty);
    }

    #[test]
    fn herdr_pattern_names_the_same_marks() {
        for mark in MARKS.iter().chain(LEVELS.iter()) {
            let plain = mark.trim().replace('(', r"\(").replace(')', r"\)");
            let first = plain.split(' ').next().unwrap();
            assert!(HERDR_PATTERN.contains(first), "{mark}");
        }
    }
}
