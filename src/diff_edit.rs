//! Cuts made from the repositories panel's diff, as vim's deletes make
//! them: whole lines, or chars on to a place maybe lines on, out of a
//! file of the working tree. A file isn't cut once it no longer holds
//! the lines its diff showed; what it held before is kept, to put back.
//! A restore puts changed lines back as the diff's base had them.

use std::path::{Path, PathBuf};

use crate::diff::{self, FileDiff, LineKind};
use crate::find;

/// A place in a file: a line, by `diff::Line::number` (from 1), and a
/// char of it as the diff shows it, tabs as four spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spot {
    pub line: usize,
    pub col: usize,
}

/// What is cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cut {
    /// These lines, whole, by number; those past the file's end are
    /// none.
    Lines(Vec<usize>),
    /// From one place up to the other, without it: the lines between go,
    /// and those two are joined. Never a line's break at its end.
    Chars(Spot, Spot),
    /// Changes put back as they were: the added lines `drop` go, by
    /// number, and each run of removed lines in `put` comes back over
    /// the line its number is (past the last, at the end).
    Restore {
        drop: Vec<usize>,
        put: Vec<(usize, Vec<String>)>,
    },
}

impl Cut {
    /// How many lines a restore puts back or takes away.
    pub fn restored(&self) -> Option<usize> {
        match self {
            Cut::Restore { drop, put } => {
                Some(drop.len() + put.iter().map(|(_, l)| l.len()).sum::<usize>())
            }
            _ => None,
        }
    }
}

/// A cut out of file `path` of the repository at `root`, made as its
/// diff showed `shown`: the lines it has of the file as it is now, by
/// number. The caret was at `caret`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdit {
    pub root: PathBuf,
    pub path: String,
    pub cut: Cut,
    pub shown: Vec<(usize, String)>,
    pub caret: Spot,
}

/// What was yanked or deleted last, to paste: whole lines, each with its
/// break, or chars.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub lines: bool,
}

/// A paste into file `path` of the repository at `root`, at `at` (the
/// caret), `count` times; before it, or after. `shown` as
/// `FileEdit::shown`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Put {
    pub root: PathBuf,
    pub path: String,
    pub shown: Vec<(usize, String)>,
    pub at: Spot,
    pub before: bool,
    pub count: usize,
}

/// A cut made: the file's text before it and after, and where the caret
/// was and went, to undo and do it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub root: PathBuf,
    pub path: String,
    pub before: String,
    pub after: String,
    pub caret: Spot,
    pub then: Spot,
}

/// The lines `file`'s diff shows of it as it is now (not the removed
/// ones), by number, as `FileEdit::shown`.
pub fn shown(file: &FileDiff) -> Vec<(usize, String)> {
    let lines = file.hunks.iter().flat_map(|h| &h.lines);
    lines
        .filter(|l| l.kind != LineKind::Removed)
        .map(|l| (l.number, l.text.clone()))
        .collect()
}

/// The restore putting back the changed lines of `file` that `picked`
/// takes (by hunk and line): its added lines go, its removed lines come
/// back where they were; and the first line it changes. `None` when it
/// takes no changed line.
pub fn restore_cut(file: &FileDiff, picked: impl Fn(usize, usize) -> bool) -> Option<(Cut, usize)> {
    let mut drop = Vec::new();
    let mut put: Vec<(usize, Vec<String>)> = Vec::new();
    for (h, hunk) in file.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            if line.kind == LineKind::Context || !picked(h, l) {
                continue;
            }
            if line.kind == LineKind::Added {
                drop.push(line.number);
                continue;
            }
            // A removed line's number is the line now after it.
            match put.last_mut() {
                Some((n, lines)) if *n == line.number => lines.push(line.raw().to_owned()),
                _ => put.push((line.number, vec![line.raw().to_owned()])),
            }
        }
    }
    let first = drop
        .first()
        .copied()
        .into_iter()
        .chain(put.first().map(|p| p.0))
        .min()?;
    Some((Cut::Restore { drop, put }, first))
}

/// `text`'s lines, each with its break.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A line without its break.
fn body(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(line) => line.strip_suffix('\r').unwrap_or(line),
        None => line,
    }
}

/// Whether `text` holds the lines `shown` says it does, as a diff shows
/// them.
fn holds(text: &str, shown: &[(usize, String)]) -> bool {
    let lines = lines(text);
    shown.iter().all(|(n, line)| {
        let at = n.checked_sub(1).and_then(|i| lines.get(i));
        at.is_some_and(|l| diff::expand(body(l)) == *line)
    })
}

/// The byte of `line` where its char shown at `col` starts, tabs shown
/// as four; its end, past its last.
fn byte_at(line: &str, col: usize) -> usize {
    let mut shown = 0;
    for (b, c) in line.char_indices() {
        shown += if c == '\t' { 4 } else { 1 };
        if col < shown {
            return b;
        }
    }
    line.len()
}

/// The column Zed opens `line` at for its char shown at `col` (tabs shown
/// as four): from 1, a tab one char.
pub fn zed_column(line: &str, col: usize) -> usize {
    line[..byte_at(line, col)].chars().count() + 1
}

/// The byte of `line` the chars shown before `col` end at: past a tab
/// `col` is within.
fn end_at(line: &str, col: usize) -> usize {
    let Some(last) = col.checked_sub(1) else {
        return 0;
    };
    let b = byte_at(line, last);
    b + line[b..].chars().next().map_or(0, char::len_utf8)
}

/// Where line `n` of `text` starts, by byte.
fn line_start(lines: &[&str], n: usize) -> usize {
    lines[..n - 1].iter().map(|l| l.len()).sum()
}

/// `text` with `cut` cut out of it, and where the caret goes: where the
/// chars were, or on the first char of the line after the lines (the
/// last line, past the end).
pub fn cut(text: &str, cut: &Cut) -> Result<(String, Spot), &'static str> {
    let all = lines(text);
    match cut {
        Cut::Lines(numbers) => {
            let gone: Vec<usize> = numbers
                .iter()
                .copied()
                .filter(|&n| n >= 1 && n <= all.len())
                .collect();
            let Some(&first) = gone.iter().min() else {
                return Err("Those lines aren't in the file");
            };
            let mut kept: Vec<&str> = (1..=all.len())
                .filter(|n| !gone.contains(n))
                .map(|n| all[n - 1])
                .collect();
            let mut after: String = kept.concat();
            // The file ended without a break: so it still does.
            let last_gone = gone.contains(&all.len());
            if last_gone && !text.ends_with('\n') && !kept.is_empty() {
                let end = kept.len() - 1;
                kept[end] = body(kept[end]);
                after = kept.concat();
            }
            let line = first.min(kept.len().max(1));
            let col = kept
                .get(line - 1)
                .map_or(0, |l| find::first_char(&diff::expand(body(l))));
            Ok((after, Spot { line, col }))
        }
        Cut::Chars(from, to) => {
            let (from, to) = if (from.line, from.col) <= (to.line, to.col) {
                (from, to)
            } else {
                (to, from)
            };
            let (start, end) = char_span(&all, from, to)?;
            let after = [&text[..start], &text[end..]].concat();
            let line = lines(&after)
                .get(from.line - 1)
                .map_or("", |l| body(l))
                .to_owned();
            let len = diff::expand(&line).chars().count();
            let col = from.col.min(len.saturating_sub(1));
            Ok((
                after,
                Spot {
                    line: from.line,
                    col,
                },
            ))
        }
        Cut::Restore { drop, put } => restore(text, &all, drop, put),
    }
}

/// `text` (its lines `all`) with the lines `drop` gone and those `put`
/// back, as `Cut::Restore`; and where the caret goes, on the first line
/// put back or after those gone. Lines put back end as the file's do.
fn restore(
    text: &str,
    all: &[&str],
    drop: &[usize],
    put: &[(usize, Vec<String>)],
) -> Result<(String, Spot), &'static str> {
    let fits = drop.iter().all(|&n| n >= 1 && n <= all.len())
        && put.iter().all(|&(n, _)| n >= 1 && n <= all.len() + 1);
    if !fits || (drop.is_empty() && put.iter().all(|(_, l)| l.is_empty())) {
        return Err("Those lines aren't in the file");
    }
    let brk = match all.first().is_some_and(|l| l.ends_with("\r\n")) {
        true => "\r\n",
        false => "\n",
    };
    let mut kept: Vec<String> = Vec::new();
    let mut first = None;
    for n in 1..=all.len() + 1 {
        for (_, lines) in put.iter().filter(|(at, _)| *at == n) {
            first.get_or_insert(kept.len() + 1);
            kept.extend(lines.iter().map(|l| format!("{l}{brk}")));
        }
        let Some(line) = all.get(n - 1) else {
            break;
        };
        if drop.contains(&n) {
            first.get_or_insert(kept.len() + 1);
            continue;
        }
        match line.ends_with('\n') {
            true => kept.push(line.to_string()),
            false => kept.push(format!("{line}{brk}")),
        }
    }
    let mut after = kept.concat();
    // The file ended without a break: so it still does.
    if !text.is_empty() && !text.ends_with('\n') {
        let end = after.strip_suffix('\n').unwrap_or(&after);
        let end = end.strip_suffix('\r').unwrap_or(end).len();
        after.truncate(end);
    }
    let line = first.unwrap_or(1).min(kept.len().max(1));
    let col = kept
        .get(line - 1)
        .map_or(0, |l| find::first_char(&diff::expand(body(l))));
    Ok((after, Spot { line, col }))
}

/// Where the chars from `from` up to `to` (in order) are in the text of
/// lines `all`, by byte.
fn char_span(all: &[&str], from: &Spot, to: &Spot) -> Result<(usize, usize), &'static str> {
    let ok = |n: usize| n >= 1 && n <= all.len();
    if !(ok(from.line) && ok(to.line)) {
        return Err("Those lines aren't in the file");
    }
    let start = line_start(all, from.line) + byte_at(body(all[from.line - 1]), from.col);
    let end = line_start(all, to.line) + end_at(body(all[to.line - 1]), to.col);
    if start >= end {
        return Err("Nothing there");
    }
    Ok((start, end))
}

/// What `cut` takes of `text`: whole lines each with its break, or the
/// chars.
pub fn taken(text: &str, cut: &Cut) -> Result<Register, &'static str> {
    let all = lines(text);
    match cut {
        Cut::Lines(numbers) => {
            let mut taken: String = (1..=all.len())
                .filter(|n| numbers.contains(n))
                .map(|n| all[n - 1])
                .collect();
            if taken.is_empty() {
                return Err("Those lines aren't in the file");
            }
            if !taken.ends_with('\n') {
                taken.push('\n');
            }
            Ok(Register {
                text: taken,
                lines: true,
            })
        }
        Cut::Chars(from, to) => {
            let (from, to) = if (from.line, from.col) <= (to.line, to.col) {
                (from, to)
            } else {
                (to, from)
            };
            let (start, end) = char_span(&all, from, to)?;
            Ok(Register {
                text: text[start..end].to_owned(),
                lines: false,
            })
        }
        // Nothing to paste: what was there is the diff's base.
        Cut::Restore { .. } => Ok(Register::default()),
    }
}

/// `text` with `register` put in at `put.at`, `put.count` times: whole
/// lines under its line (over it, `before`), else chars after its char
/// (before it); and where the caret goes, on the first line put or the
/// last char.
pub fn put_in(text: &str, put: &Put, register: &Register) -> Result<(String, Spot), &'static str> {
    let all = lines(text);
    let n = put.at.line;
    if register.text.is_empty() {
        return Err("Nothing yanked or deleted yet");
    }
    if n < 1 || n > all.len().max(1) {
        return Err("That line isn't in the file");
    }
    let more = register.text.repeat(put.count.max(1));
    if register.lines {
        let at = if put.before { n - 1 } else { n.min(all.len()) };
        let byte = line_start(&all, at + 1);
        // After a last line without a break: the break goes first.
        let (gap, more) = match at == all.len() && !text.is_empty() && !text.ends_with('\n') {
            true => ("\n", more.strip_suffix('\n').unwrap_or(&more)),
            false => ("", more.as_str()),
        };
        let after = [&text[..byte], gap, more, &text[byte..]].concat();
        let first = lines(more)
            .first()
            .map_or(0, |l| find::first_char(&diff::expand(body(l))));
        return Ok((
            after,
            Spot {
                line: at + 1,
                col: first,
            },
        ));
    }
    let line = all.get(n - 1).map_or("", |l| body(l));
    let b = match put.before {
        true => byte_at(line, put.at.col),
        false => end_at(line, put.at.col + 1),
    };
    let byte = all
        .get(..n - 1)
        .map_or(0, |a| a.iter().map(|l| l.len()).sum())
        + b;
    let after = [&text[..byte], &more, &text[byte..]].concat();
    let shown = |t: &str| diff::expand(t).chars().count();
    let col = match more.contains('\n') {
        true => shown(&line[..b]),
        false => (shown(&line[..b]) + shown(&more)).saturating_sub(1),
    };
    Ok((after, Spot { line: n, col }))
}

/// Cut `edit` out of its file, unless it no longer holds what the diff
/// showed of it; and what it took.
pub fn apply(edit: &FileEdit) -> Result<(Done, Register), String> {
    let full = edit.root.join(&edit.path);
    let before = read_shown(&full, &edit.path, &edit.shown)?;
    let register = taken(&before, &edit.cut).map_err(String::from)?;
    let (after, then) = cut(&before, &edit.cut).map_err(String::from)?;
    std::fs::write(&full, &after).map_err(|e| format!("{}: {e}", edit.path))?;
    let done = Done {
        root: edit.root.clone(),
        path: edit.path.clone(),
        before,
        after,
        caret: edit.caret,
        then,
    };
    Ok((done, register))
}

/// What `edit` would cut out of its file, unless it no longer holds what
/// the diff showed of it.
pub fn yank(edit: &FileEdit) -> Result<Register, String> {
    let text = read_shown(&edit.root.join(&edit.path), &edit.path, &edit.shown)?;
    taken(&text, &edit.cut).map_err(String::from)
}

/// Put `register` in the file `put` is in, unless it no longer holds what
/// the diff showed of it.
pub fn paste(put: &Put, register: &Register) -> Result<Done, String> {
    let full = put.root.join(&put.path);
    let before = read_shown(&full, &put.path, &put.shown)?;
    let (after, then) = put_in(&before, put, register).map_err(String::from)?;
    std::fs::write(&full, &after).map_err(|e| format!("{}: {e}", put.path))?;
    Ok(Done {
        root: put.root.clone(),
        path: put.path.clone(),
        before,
        after,
        caret: put.at,
        then,
    })
}

/// The text of the file at `full` (`path` in its repository), if it still
/// holds the lines `shown`.
fn read_shown(full: &Path, path: &str, shown: &[(usize, String)]) -> Result<String, String> {
    let text = read(full)?;
    if !holds(&text, shown) {
        return Err(format!("{path} changed since the diff was read"));
    }
    Ok(text)
}

/// Put `done`'s file back as it was before it (`back`), or cut it again,
/// unless it changed since; where the caret goes.
pub fn redo(done: &Done, back: bool) -> Result<Spot, String> {
    let full = done.root.join(&done.path);
    let (now, then, caret) = match back {
        true => (&done.after, &done.before, done.caret),
        false => (&done.before, &done.after, done.then),
    };
    if read(&full)? != *now {
        return Err(format!("{} changed since", done.path));
    }
    std::fs::write(&full, then).map_err(|e| format!("{}: {e}", done.path))?;
    Ok(caret)
}

/// The text of the file at `path`.
fn read(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} isn't text", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(line: usize, col: usize) -> Spot {
        Spot { line, col }
    }

    #[test]
    fn whole_lines_go_with_their_breaks() {
        let text = "one\n  two\nthree\nfour\n";
        let (after, at) = cut(text, &Cut::Lines(vec![2, 3])).unwrap();
        assert_eq!(after, "one\nfour\n");
        assert_eq!(at, spot(2, 0));
        let (after, at) = cut(text, &Cut::Lines(vec![1])).unwrap();
        assert_eq!(after, "  two\nthree\nfour\n");
        assert_eq!(at, spot(1, 2));
        // Lines apart, as a selection over two hunks takes them.
        let (after, _) = cut(text, &Cut::Lines(vec![1, 4])).unwrap();
        assert_eq!(after, "  two\nthree\n");
        // Past the end: as many as there are, the caret on the last.
        let (after, at) = cut(text, &Cut::Lines(vec![4, 5, 6])).unwrap();
        assert_eq!(after, "one\n  two\nthree\n");
        assert_eq!(at, spot(3, 0));
        assert!(cut(text, &Cut::Lines(vec![9])).is_err());
    }

    #[test]
    fn a_file_without_a_last_break_keeps_without() {
        let (after, _) = cut("a\nb\nc", &Cut::Lines(vec![3])).unwrap();
        assert_eq!(after, "a\nb");
        let (after, _) = cut("a\r\nb", &Cut::Lines(vec![2])).unwrap();
        assert_eq!(after, "a");
        let (after, _) = cut("a\nb\n", &Cut::Lines(vec![1, 2])).unwrap();
        assert_eq!(after, "");
    }

    #[test]
    fn a_restore_puts_changes_back_as_they_were() {
        let restore = |drop: Vec<usize>, put: Vec<(usize, Vec<&str>)>| Cut::Restore {
            drop,
            put: put
                .into_iter()
                .map(|(n, l)| (n, l.into_iter().map(String::from).collect()))
                .collect(),
        };
        // `-a` `+b`: b goes, a comes back over where it was.
        let text = "x\nb\ny\n";
        let (after, at) = cut(text, &restore(vec![2], vec![(2, vec!["a"])])).unwrap();
        assert_eq!(after, "x\na\ny\n");
        assert_eq!(at, spot(2, 0));
        // An added null check goes.
        let text = "f()\nif x == nil {\n  return\n}\ng()\n";
        let (after, at) = cut(text, &restore(vec![2, 3, 4], vec![])).unwrap();
        assert_eq!(after, "f()\ng()\n");
        assert_eq!(at, spot(2, 0));
        // Removed lines come back, at the end too; tabs as they were.
        let (after, _) = cut("a\nc\n", &restore(vec![], vec![(2, vec!["\tb"])])).unwrap();
        assert_eq!(after, "a\n\tb\nc\n");
        let (after, _) = cut("a\n", &restore(vec![], vec![(2, vec!["z"])])).unwrap();
        assert_eq!(after, "a\nz\n");
        // Part of a change: one added line of two.
        let (after, _) = cut("a\nb\nc\n", &restore(vec![3], vec![])).unwrap();
        assert_eq!(after, "a\nb\n");
        // Breaks as the file's, and none at its end when it had none.
        let (after, _) = cut("a\r\nb\r\n", &restore(vec![2], vec![(2, vec!["c"])])).unwrap();
        assert_eq!(after, "a\r\nc\r\n");
        let (after, _) = cut("a\nb", &restore(vec![], vec![(3, vec!["c"])])).unwrap();
        assert_eq!(after, "a\nb\nc");
        let (after, _) = cut("a\n\nb", &restore(vec![3], vec![])).unwrap();
        assert_eq!(after, "a\n");
        assert!(cut("a\n", &restore(vec![5], vec![])).is_err());
        assert!(cut("a\n", &restore(vec![], vec![(3, vec!["z"])])).is_err());
        assert_eq!(
            taken("a\n", &restore(vec![1], vec![])),
            Ok(Register::default())
        );
    }

    #[test]
    fn zed_columns_count_a_tab_as_one_char() {
        assert_eq!(zed_column("let x", 0), 1);
        assert_eq!(zed_column("let x", 4), 5);
        // Within the tab, or after it.
        assert_eq!(zed_column("\tif", 2), 1);
        assert_eq!(zed_column("\tif", 5), 3);
        assert_eq!(zed_column("é = 1", 2), 3);
        // Past its end, after its last.
        assert_eq!(zed_column("ab", 9), 3);
    }

    #[test]
    fn chars_go_by_where_the_diff_shows_them() {
        let text = "let x = 1;\n\tif y {\nz\r\n";
        // `x` and the space after it.
        let (after, at) = cut(text, &Cut::Chars(spot(1, 4), spot(1, 6))).unwrap();
        assert_eq!(after, "let = 1;\n\tif y {\nz\r\n");
        assert_eq!(at, spot(1, 4));
        // The tab shows as four: col 2 is within it, col 4 is `i`.
        let (after, _) = cut(text, &Cut::Chars(spot(2, 2), spot(2, 5))).unwrap();
        assert_eq!(after, "let x = 1;\nf y {\nz\r\n");
        // On to the end of the line, never its break.
        let (after, _) = cut(text, &Cut::Chars(spot(3, 0), spot(3, 99))).unwrap();
        assert_eq!(after, "let x = 1;\n\tif y {\n\r\n");
        // Over lines, joining them; either way round.
        let (after, at) = cut(text, &Cut::Chars(spot(2, 8), spot(1, 8))).unwrap();
        assert_eq!(after, "let x =  {\nz\r\n");
        assert_eq!(at, spot(1, 8));
    }

    #[test]
    fn a_file_is_cut_only_while_it_holds_what_was_shown() {
        let text = "a\n\tb\nc\n";
        let shown = |n: usize, t: &str| vec![(n, t.to_owned())];
        assert!(holds(text, &shown(2, "    b")));
        assert!(!holds(text, &shown(2, "b")));
        assert!(!holds(text, &shown(4, "")));
        let dir = std::env::temp_dir().join(format!("gerdr-diff-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), text).unwrap();
        let edit = |shown: Vec<(usize, String)>| FileEdit {
            root: dir.clone(),
            path: "f".into(),
            cut: Cut::Lines(vec![2]),
            shown,
            caret: spot(2, 4),
        };
        assert!(apply(&edit(shown(2, "b"))).is_err());
        let (done, taken) = apply(&edit(shown(2, "    b"))).unwrap();
        assert_eq!(taken.text, "\tb\n");
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "a\nc\n");
        assert_eq!(redo(&done, true), Ok(spot(2, 4)));
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), text);
        assert_eq!(redo(&done, false), Ok(spot(2, 0)));
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "a\nc\n");
        // Changed since: left alone.
        std::fs::write(dir.join("f"), "other\n").unwrap();
        assert!(redo(&done, true).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "other\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_a_cut_takes_is_kept_to_paste() {
        let text = "a\n\tb c\nd";
        let lines = |t: &str| Register {
            text: t.into(),
            lines: true,
        };
        let chars = |t: &str| Register {
            text: t.into(),
            lines: false,
        };
        assert_eq!(taken(text, &Cut::Lines(vec![2])), Ok(lines("\tb c\n")));
        assert_eq!(taken(text, &Cut::Lines(vec![3, 1])), Ok(lines("a\nd\n")));
        assert_eq!(
            taken(text, &Cut::Chars(spot(2, 4), spot(2, 5))),
            Ok(chars("b"))
        );
        assert_eq!(
            taken(text, &Cut::Chars(spot(2, 6), spot(3, 1))),
            Ok(chars("c\nd"))
        );
    }

    #[test]
    fn a_paste_goes_under_or_over_the_line_or_by_the_char() {
        let text = "one\n  two\nend";
        let put = |line, col, before, count| Put {
            root: PathBuf::new(),
            path: String::new(),
            shown: Vec::new(),
            at: spot(line, col),
            before,
            count,
        };
        let lines = Register {
            text: "  x\n".into(),
            lines: true,
        };
        let (after, at) = put_in(text, &put(1, 0, false, 1), &lines).unwrap();
        assert_eq!((after.as_str(), at), ("one\n  x\n  two\nend", spot(2, 2)));
        let (after, at) = put_in(text, &put(1, 2, true, 2), &lines).unwrap();
        assert_eq!(
            (after.as_str(), at),
            ("  x\n  x\none\n  two\nend", spot(1, 2))
        );
        // Under a last line without a break, and still without one.
        let (after, at) = put_in(text, &put(3, 0, false, 1), &lines).unwrap();
        assert_eq!((after.as_str(), at), ("one\n  two\nend\n  x", spot(4, 2)));
        let chars = Register {
            text: "ab".into(),
            lines: false,
        };
        let (after, at) = put_in(text, &put(1, 1, false, 1), &chars).unwrap();
        assert_eq!((after.as_str(), at), ("onabe\n  two\nend", spot(1, 3)));
        let (after, at) = put_in(text, &put(1, 1, true, 2), &chars).unwrap();
        assert_eq!((after.as_str(), at), ("oababne\n  two\nend", spot(1, 4)));
        // On an empty file and an empty line.
        assert_eq!(put_in("", &put(1, 0, false, 1), &chars).unwrap().0, "ab");
        assert_eq!(
            put_in("\n", &put(1, 0, false, 1), &chars).unwrap().0,
            "ab\n"
        );
        assert!(put_in(text, &put(1, 0, false, 1), &Register::default()).is_err());
    }
}
