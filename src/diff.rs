//! A diff read from git, told apart once: per file its hunks and their
//! lines, and within a removed line and the added line it became, the
//! words that changed. The repositories panel shows every file of every
//! repository this way, one after another.

use std::ops::Range;

/// How many lines of one file's diff are kept; the rest are counted.
pub const FILE_LINES: usize = 2000;
/// A removed and an added line this long or longer aren't compared word
/// by word.
const WORD_LINE: usize = 500;
/// The most tokens times tokens a pair of lines is compared over.
const WORD_WORK: usize = 40_000;
/// How much of a pair of lines must be alike for its changed words to
/// show: less, and the lines are simply other lines.
const WORD_ALIKE: f32 = 0.3;

/// Char ranges into a line.
type Words = Vec<Range<usize>>;

/// One file's changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    /// From the repository's top folder.
    pub path: String,
    /// What a rename was called before.
    pub orig: Option<String>,
    /// `git::Change::code`.
    pub code: char,
    pub hunks: Vec<Hunk>,
    /// What isn't a hunk: `Binary file`, how many more lines there were,
    /// an untracked folder's files, why git couldn't say.
    pub notes: Vec<String>,
    /// Lines added and removed, all of them, kept or not.
    pub added: usize,
    pub removed: usize,
}

/// A run of changed lines with the lines around them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    /// What git says the lines are in, after the second `@@`: often the
    /// function's first line.
    pub context: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// Without its `+`, `-` or space, tabs as four spaces.
    pub text: String,
    /// Where it is in the file as it is now; a removed line, where it was.
    pub number: usize,
    /// The words changed from the line it pairs with, by char index
    /// into `text`; empty when it pairs with none, or is all changed.
    pub words: Words,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineKind {
    #[default]
    Context,
    Added,
    Removed,
}

/// The files of `git diff`'s `text`, in its order. A file's header runs
/// from its `diff --git` to its first hunk, so a removed line starting
/// with `--` isn't taken for one.
pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<Section> = Vec::new();
    for raw in text.lines() {
        if raw.starts_with("diff --git ") || raw.starts_with("diff --cc ") {
            files.push(Section::new(raw));
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        file.line(raw);
    }
    files.into_iter().map(Section::finish).collect()
}

/// A file as it is read.
struct Section {
    diff: FileDiff,
    /// The paths the header names, as found.
    header: Option<String>,
    minus: Option<String>,
    plus: Option<String>,
    renamed: Option<String>,
    /// Inside a hunk (past the header).
    in_hunk: bool,
    /// The next line's number in the file as it is now.
    number: usize,
    /// Lines kept, and lines past `FILE_LINES` dropped.
    kept: usize,
    dropped: usize,
}

impl Section {
    fn new(header: &str) -> Section {
        Section {
            diff: FileDiff::default(),
            header: header_path(header),
            minus: None,
            plus: None,
            renamed: None,
            in_hunk: false,
            number: 1,
            kept: 0,
            dropped: 0,
        }
    }

    fn line(&mut self, raw: &str) {
        if let Some(rest) = raw.strip_prefix("@@") {
            self.in_hunk = true;
            let (numbers, context) = rest.split_once("@@").unwrap_or((rest, ""));
            self.number = numbers
                .split_whitespace()
                .find_map(|n| n.strip_prefix('+'))
                .and_then(|n| n.split(',').next()?.parse().ok())
                .unwrap_or(1);
            if self.kept < FILE_LINES {
                self.diff.hunks.push(Hunk {
                    context: expand(context.trim()),
                    lines: Vec::new(),
                });
            }
            return;
        }
        if !self.in_hunk {
            self.header_line(raw);
            return;
        }
        let (kind, text) = match raw.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &raw[1..]),
            Some(b'-') => (LineKind::Removed, &raw[1..]),
            Some(b' ') => (LineKind::Context, &raw[1..]),
            None => (LineKind::Context, ""),
            // `\ No newline at end of file`.
            Some(b'\\') => return,
            _ => {
                // `… N more lines`, from a cut.
                self.diff.notes.push(raw.to_string());
                return;
            }
        };
        match kind {
            LineKind::Added => self.diff.added += 1,
            LineKind::Removed => self.diff.removed += 1,
            LineKind::Context => {}
        }
        let number = self.number;
        if kind != LineKind::Removed {
            self.number += 1;
        }
        if self.kept >= FILE_LINES {
            self.dropped += 1;
            return;
        }
        self.kept += 1;
        if let Some(hunk) = self.diff.hunks.last_mut() {
            hunk.lines.push(Line {
                kind,
                text: expand(text),
                number,
                words: Vec::new(),
            });
        }
    }

    /// A line of the file's header: where its paths are found.
    fn header_line(&mut self, raw: &str) {
        if let Some(path) = raw.strip_prefix("--- ") {
            self.minus = side_path(path, "a/");
        } else if let Some(path) = raw.strip_prefix("+++ ") {
            self.plus = side_path(path, "b/");
        } else if let Some(path) = raw
            .strip_prefix("rename to ")
            .or_else(|| raw.strip_prefix("copy to "))
        {
            self.renamed = Some(unquote(path));
        } else if let Some(path) = raw
            .strip_prefix("rename from ")
            .or_else(|| raw.strip_prefix("copy from "))
        {
            self.diff.orig = Some(unquote(path));
        } else if raw.starts_with("Binary files ") {
            self.diff.notes.push("Binary file".into());
        }
    }

    fn finish(mut self) -> FileDiff {
        self.diff.path = self
            .renamed
            .or(self.plus)
            .or(self.minus)
            .or(self.header)
            .unwrap_or_default();
        if self.dropped > 0 {
            let more = format!("… {} more lines", self.dropped);
            self.diff.notes.insert(0, more);
        }
        for hunk in &mut self.diff.hunks {
            pair_words(&mut hunk.lines);
        }
        self.diff
    }
}

/// `text` with its tabs as four spaces, as the panel draws them.
fn expand(text: &str) -> String {
    text.replace('\t', "    ")
}

/// The path of a `---` or `+++` line, without its `a/` or `b/`; `None`
/// for `/dev/null`.
fn side_path(path: &str, side: &str) -> Option<String> {
    let path = unquote(path.trim_end_matches('\t'));
    (path != "/dev/null").then(|| path.strip_prefix(side).unwrap_or(&path).to_string())
}

/// The path after `b/` in a `diff --git a/x b/y` line. Unquoted and with a
/// space in it, it is told apart when both sides are the same.
fn header_path(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("diff --git ")
        .or_else(|| line.strip_prefix("diff --cc "))?;
    if line.starts_with("diff --cc ") {
        return Some(unquote(rest));
    }
    if let Some(open) = rest.strip_suffix('"') {
        let start = open.rfind(" \"")? + 1;
        let path = unquote(&rest[start..]);
        return Some(path.strip_prefix("b/").unwrap_or(&path).to_string());
    }
    // `a/P b/P`: P is as long as half of what is left.
    let n = rest.len().checked_sub(5)? / 2;
    if rest.len() == 2 * n + 5
        && rest.is_char_boundary(2 + n)
        && rest[2..2 + n] == rest[n + 5..]
        && rest[2 + n..].starts_with(" b/")
    {
        return Some(rest[n + 5..].to_string());
    }
    let at = rest.rfind(" b/")?;
    Some(rest[at + 3..].to_string())
}

/// A path as git writes it: in double quotes with C escapes when it has
/// odd characters, else as it is.
pub fn unquote(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.to_string();
    };
    let mut bytes = Vec::new();
    let mut chars = inner.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(d @ b'0'..=b'7') => {
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&d @ b'0'..=b'7') => {
                            v = v * 8 + (d - b'0') as u32;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(v as u8);
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Each run of removed lines followed by added lines paired one to one,
/// and the words changed between each pair marked.
fn pair_words(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        let removed = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let added = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        let pairs = (added - removed).min(i - added);
        for k in 0..pairs {
            let (old, new) = (removed + k, added + k);
            if let Some((a, b)) = changed_words(&lines[old].text, &lines[new].text) {
                lines[old].words = a;
                lines[new].words = b;
            }
        }
    }
}

/// The char ranges of `old` and of `new` not in their longest common run
/// of tokens; `None` when they are too long to compare, too unalike for
/// it to help, or the same.
fn changed_words(old: &str, new: &str) -> Option<(Words, Words)> {
    if old.len() >= WORD_LINE || new.len() >= WORD_LINE {
        return None;
    }
    let (a, b) = (tokens(old), tokens(new));
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || n * m > WORD_WORK {
        return None;
    }
    // The longest common subsequence's lengths, from the ends.
    let mut table = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if a[i].1 == b[j].1 {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut same_a, mut same_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i].1 == b[j].1 {
            same_a[i] = true;
            same_b[j] = true;
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let alike = |toks: &[(Range<usize>, &str)], same: &[bool]| -> usize {
        toks.iter()
            .zip(same)
            .filter(|(t, s)| **s && !t.1.trim().is_empty())
            .map(|(t, _)| t.0.len())
            .sum()
    };
    let solid = |toks: &[(Range<usize>, &str)]| -> usize {
        toks.iter()
            .filter(|t| !t.1.trim().is_empty())
            .map(|t| t.0.len())
            .sum()
    };
    let longest = solid(&a).max(solid(&b)).max(1);
    let share = alike(&a, &same_a).max(alike(&b, &same_b)) as f32 / longest as f32;
    if share < WORD_ALIKE {
        return None;
    }
    let (ra, rb) = (ranges(&a, &same_a), ranges(&b, &same_b));
    if ra.is_empty() && rb.is_empty() {
        return None;
    }
    Some((ra, rb))
}

/// `text`'s tokens with their char ranges: runs of letters, digits and
/// `_`, runs of white space, and each other character alone.
fn tokens(text: &str) -> Vec<(Range<usize>, &str)> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut start: Option<(usize, usize, Class)> = None;
    let mut n = 0;
    for (byte, ch) in text.char_indices() {
        let this = class(ch);
        match &start {
            Some((_, _, c)) if *c == this && this != Class::Other => {}
            Some((b, s, _)) => {
                out.push((*s..n, &text[*b..byte]));
                start = Some((byte, n, this));
            }
            None => start = Some((byte, n, this)),
        }
        n += 1;
    }
    if let Some((b, s, _)) = start {
        out.push((s..n, &text[b..]));
    }
    out
}

/// The ranges of the tokens not `same`, those next to each other (or with
/// only white space between) joined.
fn ranges(toks: &[(Range<usize>, &str)], same: &[bool]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    // Only white space lies between the last range and here.
    let mut joins = false;
    for (tok, &s) in toks.iter().zip(same) {
        if !s {
            match out.last_mut() {
                Some(last) if joins => last.end = tok.0.end,
                _ => out.push(tok.0.clone()),
            }
            joins = true;
        } else if !tok.1.trim().is_empty() {
            joins = false;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn text(lines: &[&str]) -> String {
        lines.iter().flat_map(|l| [*l, "\n"]).collect()
    }

    #[test]
    fn files_hunks_and_lines() {
        let d = text(&[
            "diff --git a/src/a.rs b/src/a.rs",
            "index 1..2 100644",
            "--- a/src/a.rs",
            "+++ b/src/a.rs",
            "@@ -1,3 +1,3 @@ fn main() {",
            " keep",
            "-let x = 1;",
            "+let x = 2;",
            "\tcontext",
            "@@ -10,2 +10,3 @@",
            " a",
            "+b",
            "--- not a header",
            "\\ No newline at end of file",
            "diff --git a/gone b/gone",
            "deleted file mode 100644",
            "--- a/gone",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-bye",
        ]);
        let files = parse(&d);
        assert_eq!(files.len(), 2);
        let a = &files[0];
        assert_eq!(a.path, "src/a.rs");
        assert_eq!((a.added, a.removed), (2, 2));
        assert_eq!(a.hunks.len(), 2);
        assert_eq!(a.hunks[0].context, "fn main() {");
        let l = &a.hunks[0].lines;
        assert_eq!(l[0].kind, LineKind::Context);
        assert_eq!(
            (l[1].kind, l[1].text.as_str(), l[1].number),
            (LineKind::Removed, "let x = 1;", 2)
        );
        assert_eq!((l[2].kind, l[2].number), (LineKind::Added, 2));
        assert_eq!(l[1].words, [8..9]);
        assert_eq!(l[2].words, [8..9]);
        // A tab-led line is no context line; kept as a note instead.
        assert_eq!(a.notes, ["\tcontext"]);
        let h = &a.hunks[1].lines;
        assert_eq!(h[1].number, 11);
        assert_eq!(
            (h[2].kind, h[2].text.as_str()),
            (LineKind::Removed, "-- not a header")
        );
        assert_eq!(files[1].path, "gone");
        assert_eq!(files[1].removed, 1);
    }

    #[test]
    fn renames_binaries_and_odd_paths() {
        let d = text(&[
            "diff --git a/old name b/new name",
            "similarity index 90%",
            "rename from old name",
            "rename to new name",
            "diff --git a/img.png b/img.png",
            "Binary files a/img.png and b/img.png differ",
            "diff --git \"a/t\\303\\251\\tx\" \"b/t\\303\\251\\tx\"",
            "new file mode 100644",
            "--- /dev/null",
            "+++ \"b/t\\303\\251\\tx\"",
            "@@ -0,0 +1 @@",
            "+hi",
            "diff --git a/my file b/my file",
            "old mode 100644",
            "new mode 100755",
        ]);
        let files = parse(&d);
        assert_eq!(files[0].path, "new name");
        assert_eq!(files[0].orig.as_deref(), Some("old name"));
        assert!(files[0].hunks.is_empty());
        assert_eq!(files[1].path, "img.png");
        assert_eq!(files[1].notes, ["Binary file"]);
        assert_eq!(files[2].path, "té\tx");
        assert_eq!(files[2].added, 1);
        assert_eq!(files[3].path, "my file");
    }

    #[test]
    fn long_files_are_cut() {
        let mut lines = vec![
            "diff --git a/f b/f",
            "--- a/f",
            "+++ b/f",
            "@@ -0,0 +1,2100 @@",
        ];
        let added: Vec<String> = (0..FILE_LINES + 100).map(|n| format!("+{n}")).collect();
        lines.extend(added.iter().map(String::as_str));
        let files = parse(&text(&lines));
        assert_eq!(files[0].added, FILE_LINES + 100);
        assert_eq!(files[0].hunks[0].lines.len(), FILE_LINES);
        assert_eq!(files[0].notes, ["… 100 more lines"]);
    }

    #[test]
    fn words_changed() {
        let w = |a: &str, b: &str| changed_words(a, b);
        assert_eq!(
            w("call(foo, bar)", "call(foo, baz)"),
            Some((vec![10..13], vec![10..13]))
        );
        // Words changed next to each other, white space between, are one.
        assert_eq!(
            w("let a b = 1", "let c d = 1"),
            Some((vec![4..7], vec![4..7]))
        );
        assert_eq!(w("x", "x"), None);
        // Nothing alike: no words, the lines are other lines.
        assert_eq!(w("alpha beta gamma", "one two three"), None);
        let added = w("a = 1", "a = 1 + 2").unwrap();
        assert!(added.0.is_empty());
        assert_eq!(added.1, [5..9]);
        // Pairs only within a run: the first removed with the first added.
        let mut lines: Vec<Line> = [
            (LineKind::Removed, "one 1"),
            (LineKind::Removed, "two 2"),
            (LineKind::Added, "one 9"),
            (LineKind::Context, "x"),
            (LineKind::Added, "two 9"),
        ]
        .into_iter()
        .map(|(kind, text)| Line {
            kind,
            text: text.into(),
            number: 1,
            words: Vec::new(),
        })
        .collect();
        pair_words(&mut lines);
        assert_eq!(lines[0].words, [4..5]);
        assert!(lines[1].words.is_empty());
        assert_eq!(lines[2].words, [4..5]);
        assert!(lines[4].words.is_empty());
    }

    #[test]
    fn unquotes() {
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(unquote("\"a\\\"b\\\\c\\nd\""), "a\"b\\c\nd");
        assert_eq!(unquote("\"\\303\\251\""), "é");
    }
}
