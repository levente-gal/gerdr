//! The changes a patch makes, file by file: Codex's and OpenCode's
//! `apply_patch` text, and the unified diffs OpenCode keeps of them.

use crate::chat::{DiffLine, DiffSide};

/// The files `text` changes, in order, each with its lines. It is Codex's
/// patch form (`*** Begin Patch`, `*** Update File: <path>`, `@@`, lines
/// starting with ` `, `-`, `+`) or a unified diff (`Index:` or
/// `diff --git`, `---`/`+++`, `@@`). A file deleted has no lines; one
/// moved is named by where it went.
pub fn files(text: &str) -> Vec<(String, Vec<DiffLine>)> {
    let mut files: Vec<(String, Vec<DiffLine>)> = Vec::new();
    let mut lines = text.lines().peekable();
    // Whether a new file's header was just read: a `---`/`+++` pair after
    // it names the same file.
    let mut fresh = false;
    while let Some(line) = lines.next() {
        let named = (line.strip_prefix("*** Add File: "))
            .or(line.strip_prefix("*** Update File: "))
            .or(line.strip_prefix("*** Delete File: "))
            .or(line.strip_prefix("Index: "))
            .map(str::to_owned)
            .or_else(|| {
                let git = line.strip_prefix("diff --git ")?;
                Some(git.rsplit_once(" b/").map_or(git, |(_, b)| b).to_owned())
            });
        if let Some(path) = named {
            files.push((path.trim().to_owned(), Vec::new()));
            fresh = true;
            continue;
        }
        if let Some(to) = line.strip_prefix("*** Move to: ") {
            if let Some((path, _)) = files.last_mut() {
                *path = to.trim().to_owned();
            }
            continue;
        }
        if line.starts_with("*** ") || line.starts_with("===") || line.starts_with("\\ ") {
            continue;
        }
        if let Some(old) = line.strip_prefix("--- ")
            && let Some(new) = lines.peek().and_then(|next| next.strip_prefix("+++ "))
        {
            lines.next();
            let path = [new, old]
                .into_iter()
                .map(|p| p.split('\t').next().unwrap_or(p).trim())
                .find(|p| *p != "/dev/null")
                .map(|p| p.strip_prefix("b/").or(p.strip_prefix("a/")).unwrap_or(p));
            if !fresh && let Some(path) = path {
                files.push((path.to_owned(), Vec::new()));
            }
            fresh = false;
            continue;
        }
        if line.starts_with("@@") {
            fresh = false;
            continue;
        }
        let Some((_, diff)) = files.last_mut() else {
            continue;
        };
        let (side, text) = match line.as_bytes().first() {
            Some(b'+') => (DiffSide::Added, &line[1..]),
            Some(b'-') => (DiffSide::Removed, &line[1..]),
            Some(b' ') => (DiffSide::Context, &line[1..]),
            _ => (DiffSide::Context, line),
        };
        fresh = false;
        diff.push(DiffLine {
            side,
            text: text.to_owned(),
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides(diff: &[DiffLine]) -> Vec<(DiffSide, &str)> {
        diff.iter().map(|l| (l.side, l.text.as_str())).collect()
    }

    #[test]
    fn a_codex_patch_reads_file_by_file() {
        let text = "*** Begin Patch\n\
                    *** Update File: src/a.rs\n\
                    @@ fn main\n \
                    keep\n\
                    -old\n\
                    +new\n\
                    *** Add File: notes.md\n\
                    +# Notes\n\
                    *** Delete File: gone.txt\n\
                    *** Update File: src/b.rs\n\
                    *** Move to: src/c.rs\n\
                    @@\n\
                    -x\n\
                    +y\n\
                    *** End Patch";
        let files = files(text);
        let names: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["src/a.rs", "notes.md", "gone.txt", "src/c.rs"]);
        use DiffSide::*;
        assert_eq!(
            sides(&files[0].1),
            [(Context, "keep"), (Removed, "old"), (Added, "new")]
        );
        assert_eq!(sides(&files[1].1), [(Added, "# Notes")]);
        assert!(files[2].1.is_empty());
        assert_eq!(sides(&files[3].1), [(Removed, "x"), (Added, "y")]);
    }

    #[test]
    fn a_unified_diff_reads_with_or_without_an_index_line() {
        let text = "Index: /w/config.toml\n\
                    ===================================================================\n\
                    --- /w/config.toml\n\
                    +++ /w/config.toml\n\
                    @@ -1,2 +1,2 @@\n \
                    [tools]\n\
                    -bat = \"1\"\n\
                    +bat = \"2\"\n\
                    \\ No newline at end of file\n\
                    diff --git a/src/x.rs b/src/x.rs\n\
                    --- a/src/x.rs\n\
                    +++ b/src/x.rs\n\
                    @@ -1 +1 @@\n\
                    --- a removed line that starts with dashes\n\
                    +ok\n\
                    --- /dev/null\n\
                    +++ b/new.rs\n\
                    @@ -0,0 +1 @@\n\
                    +fn new() {}";
        let files = files(text);
        let names: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["/w/config.toml", "src/x.rs", "new.rs"]);
        use DiffSide::*;
        assert_eq!(
            sides(&files[0].1),
            [
                (Context, "[tools]"),
                (Removed, "bat = \"1\""),
                (Added, "bat = \"2\"")
            ]
        );
        assert_eq!(
            sides(&files[1].1),
            [
                (Removed, "-- a removed line that starts with dashes"),
                (Added, "ok")
            ]
        );
        assert_eq!(sides(&files[2].1), [(Added, "fn new() {}")]);
    }
}
