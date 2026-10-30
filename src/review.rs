//! Review comments on the repositories panel's diff, for an AI agent: each
//! on a run of a file's diff lines, kept with a copy of those lines, so it
//! finds them again as the diff changes under it (and says so when it
//! can't). The panel copies them all as one Markdown text to paste into an
//! agent. Kept per project in `~/.local/share/gerdr/reviews/<id>.json`,
//! written atomically (see `projects::write_atomic`); a file that can't be
//! read reads as empty, and is set aside. Those on worktrees' diffs (see
//! `worktree_diff`) are kept by worktree in `reviews/worktrees.json`, and
//! dropped once the worktree is gone, on another branch, left alone for
//! `WORKTREE_DAYS`, or past the `WORKTREE_MOST` saved last.

use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::diff::{FileDiff, LineKind};

/// The file's format version.
pub const VERSION: u32 = 1;

/// How long a worktree's review is kept after it was last saved.
pub const WORKTREE_DAYS: u64 = 30;
/// How many worktrees' reviews are kept at most: those saved last.
pub const WORKTREE_MOST: usize = 50;

/// A diff line as a comment quotes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quoted {
    pub kind: LineKind,
    /// `diff::Line::number`.
    pub number: usize,
    pub text: String,
}

impl Quoted {
    pub fn of(line: &crate::diff::Line) -> Quoted {
        Quoted {
            kind: line.kind,
            number: line.number,
            text: line.text.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// The repository's top folder.
    pub root: PathBuf,
    /// From `root`.
    pub path: String,
    /// The lines it is on, as they were when it was written: never empty.
    pub lines: Vec<Quoted>,
    /// What it says; lines broken with `\n`.
    pub text: String,
}

/// A project's review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Review {
    pub version: u32,
    pub comments: Vec<Comment>,
}

impl Default for Review {
    fn default() -> Self {
        Review {
            version: VERSION,
            comments: Vec::new(),
        }
    }
}

impl Review {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }
}

/// Where in `file` the last line `comment` is on is now, by hunk and line:
/// the same line where it was, else the same line nearest it; `None` when
/// it is gone.
pub fn locate(file: &FileDiff, comment: &Comment) -> Option<(usize, usize)> {
    find(file, comment.lines.last()?)
}

/// Where in `file` line `quoted` is now, as `locate` finds it.
pub fn find(file: &FileDiff, quoted: &Quoted) -> Option<(usize, usize)> {
    let last = quoted;
    let same = |l: &crate::diff::Line| l.kind == last.kind && l.text == last.text;
    let all = file.hunks.iter().enumerate().flat_map(|(h, hunk)| {
        hunk.lines
            .iter()
            .enumerate()
            .map(move |(l, line)| (h, l, line))
    });
    all.filter(|(_, _, line)| same(line))
        .min_by_key(|(_, _, line)| line.number.abs_diff(last.number))
        .map(|(h, l, _)| (h, l))
}

/// `text` broken into rows of at most `cols` characters, as byte ranges:
/// at its line breaks (left out of the rows), and where a line is longer,
/// after the last space that fits, else within the word. Always a row,
/// empty for no text.
pub fn wrap(text: &str, cols: usize) -> Vec<std::ops::Range<usize>> {
    let cols = cols.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        let mut from = start;
        loop {
            let rest = &text[from..end];
            let Some((cut, _)) = rest.char_indices().nth(cols) else {
                rows.push(from..end);
                break;
            };
            // After the last space that fits, else at the edge.
            let at = rest[..cut]
                .char_indices()
                .rev()
                .find(|(_, c)| *c == ' ')
                .map_or(cut, |(i, _)| i + 1);
            let at = if at == 0 { cut } else { at };
            rows.push(from..from + at);
            from += at;
        }
        start = end + 1;
    }
    rows
}

/// The lines `comment` is on, as `path:12` or `path:12-14`.
pub fn place(comment: &Comment) -> String {
    let numbers = comment.lines.iter().map(|q| q.number);
    let (from, to) = (numbers.clone().min(), numbers.max());
    match (from, to) {
        (Some(from), Some(to)) if from != to => format!("{}:{from}-{to}", comment.path),
        (Some(from), _) => format!("{}:{from}", comment.path),
        _ => comment.path.clone(),
    }
}

/// `review` as Markdown for an agent: per repository (named by `label`,
/// with its folder) its files' comments, each with the lines it is on as
/// a diff. Those whose lines `outdated` says are gone say so. The
/// repositories come in `order`'s order, then any others; the comments by
/// path and line.
pub fn markdown(
    review: &Review,
    order: &[PathBuf],
    label: impl Fn(&Path) -> String,
    outdated: impl Fn(&Comment) -> bool,
) -> String {
    let mut by_root: BTreeMap<(usize, &Path), Vec<&Comment>> = BTreeMap::new();
    for c in &review.comments {
        let rank = order
            .iter()
            .position(|r| *r == c.root)
            .unwrap_or(order.len());
        by_root.entry((rank, &c.root)).or_default().push(c);
    }
    let n = review.comments.len();
    let mut out = format!(
        "Review comments on uncommitted changes ({n} comment{}). \
         Each quotes the diff lines it is about; address each one.\n",
        if n == 1 { "" } else { "s" }
    );
    for ((_, root), mut comments) in by_root {
        comments.sort_by_key(|c| (&c.path, c.lines.first().map(|q| q.number)));
        out.push_str(&format!("\n## {} ({})\n", label(root), root.display()));
        for c in comments {
            let stale = if outdated(c) {
                " (the code changed since)"
            } else {
                ""
            };
            out.push_str(&format!("\n### `{}`{stale}\n\n```diff\n", place(c)));
            for q in &c.lines {
                let sign = match q.kind {
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                    LineKind::Context => ' ',
                };
                out.push_str(&format!("{sign}{}\n", q.text));
            }
            out.push_str("```\n\n");
            out.push_str(c.text.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Project `project`'s file under `root` (`None`: gerdr's data folder,
/// `projects::data_dir`).
pub fn file(root: Option<&Path>, project: &str) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("reviews").join(format!("{project}.json"))
}

/// Project `project`'s review, as saved under `root`; empty when there is
/// none or it can't be read. One that can't be read is set aside as
/// `<id>.json.bad-<unix time>`, so the next save doesn't lose it.
pub fn load(root: Option<&Path>, project: &str) -> Review {
    let path = file(root, project);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Review::default(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path);
            return Review::default();
        }
    };
    serde_json::from_slice(&raw).unwrap_or_else(|err| {
        tracing::warn!("reading {}: {err}", path.display());
        crate::projects::set_aside(&path);
        Review::default()
    })
}

/// Save project `project`'s review under `root`, atomically; an empty one
/// removes the file.
pub fn save(root: Option<&Path>, project: &str, review: &Review) -> std::io::Result<()> {
    let path = file(root, project);
    if review.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(review).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&path, &bytes)
}

/// The reviews written on worktrees' diffs, by worktree folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktrees {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub worktrees: BTreeMap<PathBuf, Kept>,
}

impl Default for Worktrees {
    fn default() -> Self {
        Worktrees {
            version: VERSION,
            worktrees: BTreeMap::new(),
        }
    }
}

/// A worktree's review, with the branch it was on and when it was saved
/// (seconds since the Unix epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub branch: Option<String>,
    pub saved: u64,
    pub review: Review,
}

impl Worktrees {
    /// The review of the worktree at `path`, on `branch`: none kept for
    /// another branch (the folder used again for another worktree).
    pub fn get(&self, path: &Path, branch: Option<&str>) -> Review {
        match self.worktrees.get(path) {
            Some(kept) if kept.branch.as_deref() == branch => kept.review.clone(),
            _ => Review::default(),
        }
    }

    /// Keep `review` for the worktree at `path`, on `branch`, saved at
    /// `now`; an empty one drops what was kept.
    pub fn put(&mut self, path: &Path, branch: Option<&str>, review: &Review, now: u64) {
        if review.is_empty() {
            self.worktrees.remove(path);
            return;
        }
        let kept = Kept {
            branch: branch.map(str::to_owned),
            saved: now,
            review: review.clone(),
        };
        self.worktrees.insert(path.to_path_buf(), kept);
    }

    /// Drop the review of the worktree at `path`, deleted; whether there
    /// was one.
    pub fn forget(&mut self, path: &Path) -> bool {
        self.worktrees.remove(path).is_some()
    }

    /// Drop the reviews of worktrees no longer there (by `exists`), those
    /// saved more than `WORKTREE_DAYS` before `now`, and past the
    /// `WORKTREE_MOST` saved last; whether any went.
    pub fn prune(&mut self, now: u64, exists: impl Fn(&Path) -> bool) -> bool {
        let before = self.worktrees.len();
        let oldest = now.saturating_sub(WORKTREE_DAYS * 24 * 60 * 60);
        self.worktrees
            .retain(|path, kept| kept.saved >= oldest && !kept.review.is_empty() && exists(path));
        if self.worktrees.len() > WORKTREE_MOST {
            let mut by_age: Vec<(u64, PathBuf)> = (self.worktrees.iter())
                .map(|(path, kept)| (kept.saved, path.clone()))
                .collect();
            by_age.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            for (_, path) in by_age.into_iter().skip(WORKTREE_MOST) {
                self.worktrees.remove(&path);
            }
        }
        self.worktrees.len() != before
    }
}

/// Where the worktrees' reviews are kept, under `root` (gerdr's data
/// folder by default).
pub fn worktrees_file(root: Option<&Path>) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("reviews").join("worktrees.json")
}

/// The worktrees' reviews, as saved under `root`; none when there are
/// none or they can't be read (then set aside, as `load` does).
pub fn load_worktrees(root: Option<&Path>) -> Worktrees {
    let path = worktrees_file(root);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Worktrees::default(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            crate::projects::set_aside(&path);
            return Worktrees::default();
        }
    };
    serde_json::from_slice(&raw).unwrap_or_else(|err| {
        tracing::warn!("reading {}: {err}", path.display());
        crate::projects::set_aside(&path);
        Worktrees::default()
    })
}

/// Save the worktrees' reviews under `root`; with none, the file goes.
pub fn save_worktrees(root: Option<&Path>, reviews: &Worktrees) -> std::io::Result<()> {
    let path = worktrees_file(root);
    if reviews.worktrees.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(reviews).map_err(std::io::Error::other)?;
    crate::projects::write_atomic(&path, &bytes)
}

/// The time now, in seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Hunk, Line};

    fn line(kind: LineKind, number: usize, text: &str) -> Line {
        Line {
            kind,
            text: text.into(),
            number,
            words: Vec::new(),
            syntax: Vec::new(),
        }
    }

    fn file_of(lines: Vec<Line>) -> FileDiff {
        FileDiff {
            path: "src/a.rs".into(),
            code: 'M',
            hunks: vec![Hunk {
                context: String::new(),
                lines,
            }],
            ..FileDiff::default()
        }
    }

    fn comment(lines: &[Line], text: &str) -> Comment {
        Comment {
            root: "/r/api".into(),
            path: "src/a.rs".into(),
            lines: lines.iter().map(Quoted::of).collect(),
            text: text.into(),
        }
    }

    #[test]
    fn a_comment_finds_its_line_where_it_was_moved_or_not_at_all() {
        let ctx = line(LineKind::Context, 3, "fn a() {");
        let added = line(LineKind::Added, 4, "    b();");
        let c = comment(&[ctx.clone(), added.clone()], "why b?");
        let file = file_of(vec![ctx.clone(), added.clone()]);
        assert_eq!(locate(&file, &c), Some((0, 1)));
        // Lines added above move it down; the same text elsewhere is
        // farther off.
        let moved = line(LineKind::Added, 9, "    b();");
        let far = line(LineKind::Added, 40, "    b();");
        let file = file_of(vec![far, line(LineKind::Context, 8, "fn a() {"), moved]);
        assert_eq!(locate(&file, &c), Some((0, 2)));
        // Changed, or no longer added, it is gone.
        let file = file_of(vec![line(LineKind::Context, 4, "    b();")]);
        assert_eq!(locate(&file, &c), None);
    }

    #[test]
    fn copies_as_markdown_by_repository_and_file() {
        let a = line(LineKind::Removed, 7, "old();");
        let b = line(LineKind::Added, 7, "new();");
        let mut first = comment(&[a, b], "Is new right here?\nCheck callers.");
        first.path = "src/z.rs".into();
        let second = comment(&[line(LineKind::Context, 2, "use x;")], "unused");
        let review = Review {
            comments: vec![first, second],
            ..Review::default()
        };
        let text = markdown(&review, &[], |_| "api".into(), |c| c.text == "unused");
        assert_eq!(
            text,
            "Review comments on uncommitted changes (2 comments). \
             Each quotes the diff lines it is about; address each one.\n\
             \n## api (/r/api)\n\
             \n### `src/a.rs:2` (the code changed since)\n\n```diff\n use x;\n```\n\nunused\n\
             \n### `src/z.rs:7`\n\n```diff\n-old();\n+new();\n```\n\n\
             Is new right here?\nCheck callers.\n"
        );
    }

    #[test]
    fn wraps_at_line_breaks_then_spaces_then_anywhere() {
        let rows = |text: &str, cols| -> Vec<String> {
            wrap(text, cols)
                .into_iter()
                .map(|r| text[r].to_owned())
                .collect()
        };
        assert_eq!(rows("", 10), [""]);
        assert_eq!(rows("ab\n\ncd", 10), ["ab", "", "cd"]);
        assert_eq!(rows("one two three", 8), ["one two ", "three"]);
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(rows("éé éé", 3), ["éé ", "éé"]);
    }

    #[test]
    fn saves_and_loads_and_an_empty_one_removes_its_file() {
        let dir = std::env::temp_dir().join(format!("gerdr-review-{}", std::process::id()));
        let review = Review {
            comments: vec![comment(&[line(LineKind::Added, 1, "x")], "y")],
            ..Review::default()
        };
        save(Some(&dir), "web", &review).unwrap();
        assert_eq!(load(Some(&dir), "web"), review);
        save(Some(&dir), "web", &Review::default()).unwrap();
        assert!(!file(Some(&dir), "web").exists());
        assert_eq!(load(Some(&dir), "web"), Review::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktrees_keep_their_review_on_their_branch() {
        let review = Review {
            comments: vec![comment(&[line(LineKind::Added, 1, "x")], "y")],
            ..Review::default()
        };
        let wt = Path::new("/r/.wt/x");
        let mut reviews = Worktrees::default();
        reviews.put(wt, Some("feat/x"), &review, 100);
        assert_eq!(reviews.get(wt, Some("feat/x")), review);
        // The folder used again for another branch: not its comments.
        assert!(reviews.get(wt, Some("feat/y")).is_empty());
        assert!(reviews.get(Path::new("/r/.wt/z"), None).is_empty());
        // Emptied, or deleted, it is dropped.
        reviews.put(wt, Some("feat/x"), &Review::default(), 200);
        assert!(reviews.worktrees.is_empty());
        reviews.put(wt, Some("feat/x"), &review, 200);
        assert!(reviews.forget(wt));
        assert!(!reviews.forget(wt));
    }

    #[test]
    fn worktree_reviews_gone_old_or_too_many_are_pruned() {
        let review = Review {
            comments: vec![comment(&[line(LineKind::Added, 1, "x")], "y")],
            ..Review::default()
        };
        let day = 24 * 60 * 60;
        let now = 100 * day;
        let mut reviews = Worktrees::default();
        reviews.put(Path::new("/gone"), None, &review, now);
        reviews.put(
            Path::new("/old"),
            None,
            &review,
            now - (WORKTREE_DAYS + 1) * day,
        );
        reviews.put(Path::new("/recent"), None, &review, now - day);
        let exists = |p: &Path| p != Path::new("/gone");
        assert!(reviews.prune(now, exists));
        let kept: Vec<&Path> = reviews.worktrees.keys().map(PathBuf::as_path).collect();
        assert_eq!(kept, [Path::new("/recent")]);
        assert!(!reviews.prune(now, exists));
        // Past the most kept, those saved first go.
        for n in 0..WORKTREE_MOST + 5 {
            let path = PathBuf::from(format!("/wt/{n:03}"));
            reviews.put(&path, None, &review, now - 1000 + n as u64);
        }
        assert!(reviews.prune(now, |_| true));
        assert_eq!(reviews.worktrees.len(), WORKTREE_MOST);
        assert!(!reviews.worktrees.contains_key(Path::new("/recent")));
        assert!(!reviews.worktrees.contains_key(Path::new("/wt/004")));
        assert!(reviews.worktrees.contains_key(Path::new("/wt/005")));
    }

    #[test]
    fn worktree_reviews_save_and_load() {
        let dir = std::env::temp_dir().join(format!("gerdr-wt-review-{}", std::process::id()));
        let review = Review {
            comments: vec![comment(&[line(LineKind::Added, 1, "x")], "y")],
            ..Review::default()
        };
        let mut reviews = Worktrees::default();
        reviews.put(Path::new("/r/.wt/x"), Some("x"), &review, 5);
        save_worktrees(Some(&dir), &reviews).unwrap();
        assert_eq!(load_worktrees(Some(&dir)), reviews);
        save_worktrees(Some(&dir), &Worktrees::default()).unwrap();
        assert!(!worktrees_file(Some(&dir)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
