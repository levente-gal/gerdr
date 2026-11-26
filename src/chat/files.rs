//! Files attached to a reply in an agent's chat, on the desktop or the
//! phone: a terminal agent reads a file from its path, so the reply carries
//! the attached files' paths after its words. They come from the `@` menu
//! (the files under the agent's folder), the file picker, files copied in a
//! file manager and pasted, or the phone, whose files gerdr saves first.
//! shortcut: files sent from the phone are never pruned; add pruning by age
//! if the folder grows.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::mpsc,
};

use super::commands::Command;
use crate::text_field::Line;

/// The most files the `@` menu lists: past it, the rest are left out.
const MOST: usize = 50_000;

/// The `@word` being typed at the caret of `line`, for the `@` menu: where
/// its `@` is, and what follows it. None when the caret isn't in one, or
/// the `@` follows a letter (an email address).
pub fn query(line: &Line) -> Option<(usize, &str)> {
    if line.caret != line.anchor {
        return None;
    }
    let before = line.text.get(..line.caret)?;
    let at = before.rfind(|c: char| c.is_whitespace() || c == '@')?;
    let after_at = before[at..].strip_prefix('@')?;
    let starts_word = before[..at]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    starts_word.then_some((at, after_at))
}

/// `line` with the `@word` from byte `at` to the caret taken out, and where
/// the caret goes.
pub fn without_query(line: &Line, at: usize) -> (String, usize) {
    let text = format!("{}{}", &line.text[..at], &line.text[line.caret..]);
    (text, at)
}

/// The files under `root`, as paths from it, for the `@` menu: git's own
/// and its untracked ones not ignored in a repository, else those found
/// walking it, hidden folders left out.
pub fn list(root: &Path) -> Vec<Command> {
    let git = Process::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output();
    let mut paths: Vec<String> = match git {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .take(MOST)
            .map(str::to_owned)
            .collect(),
        _ => walk(root),
    };
    // A file in conflict is listed once per stage.
    paths.dedup();
    paths
        .into_iter()
        .map(|name| Command {
            name,
            ..Default::default()
        })
        .collect()
}

/// The files under `root`, hidden folders left out, at most `MOST`.
fn walk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && !hidden => dirs.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Ok(rel) = path.strip_prefix(root) {
                        found.push(rel.to_string_lossy().into_owned());
                    }
                    if found.len() >= MOST {
                        return found;
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// List the files under `root` on a thread of its own (it may run git):
/// they come on the channel given back.
pub fn load(root: PathBuf) -> mpsc::Receiver<Vec<Command>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("chat files".into())
        .spawn(move || _ = tx.send(list(&root)));
    if let Err(err) = spawned {
        tracing::warn!("listing the files for the @ menu: {err}");
    }
    rx
}

/// The reply sent for `words` with `attached` files: the words, then each
/// file's path; one with a space is quoted.
pub fn reply(words: &str, attached: &[PathBuf]) -> String {
    let paths = attached.iter().map(|p| {
        let p = p.display().to_string();
        match p.contains(char::is_whitespace) {
            true => format!("\"{p}\""),
            false => p,
        }
    });
    std::iter::once(words.trim().to_owned())
        .chain(paths)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an attached file's chip says: its path from `cwd` when it is under
/// it, else its name.
pub fn label(path: &Path, cwd: Option<&Path>) -> String {
    let rel = cwd.and_then(|cwd| path.strip_prefix(cwd).ok());
    rel.or_else(|| path.file_name().map(Path::new))
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The local files a `text/uri-list` names (a file manager's copied
/// files), in order; its other URIs are left out.
pub fn uri_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // A host may stand before the path: `file://host/path`.
        .filter_map(|l| l.find('/').map(|i| &l[i..]))
        .map(|p| PathBuf::from(unescape(p)))
        .collect()
}

/// `text` with its `%XX` escapes decoded.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The folder of files sent from the phone.
pub fn dir() -> PathBuf {
    crate::projects::data_dir().join("chat-files")
}

/// Save `bytes`, sent from the phone as a file named `name`, to a new file
/// in the sent files' folder, named by the time and its name; where it went.
pub fn save(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    save_in(&dir(), name, bytes)
}

/// `name` made safe for a file name: its last part only, no spaces (its
/// path goes in a reply as one word), at most 80 characters.
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let safe: String = last
        .chars()
        .map(|c| match c.is_whitespace() || c.is_control() {
            true => '-',
            false => c,
        })
        .take(80)
        .collect();
    match safe.trim_start_matches('.') {
        "" => "file".to_owned(),
        _ => safe,
    }
}

fn save_in(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let name = safe_name(name);
    // Named by the time; two in the same millisecond take the next one.
    let mut at = crate::ai_work::now_ms();
    loop {
        let path = dir.join(format!("{at}-{name}"));
        match std::fs::File::create_new(&path) {
            Ok(mut file) => {
                return file
                    .write_all(bytes)
                    .map(|()| path.clone())
                    .map_err(|err| format!("{}: {err}", path.display()));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => at += 1,
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_end(text: &str) -> Line {
        text.into()
    }

    #[test]
    fn the_at_word_at_the_caret_is_the_query() {
        assert_eq!(query(&at_end("@")), Some((0, "")));
        assert_eq!(query(&at_end("look at @src/ma")), Some((8, "src/ma")));
        assert_eq!(query(&at_end("look at @src/main.rs ")), None);
        assert_eq!(query(&at_end("mail me@host")), None);
        assert_eq!(query(&at_end("plain words")), None);
        let mut mid = at_end("see @ab and more");
        (mid.caret, mid.anchor) = (7, 7);
        assert_eq!(query(&mid), Some((4, "ab")));
        assert_eq!(without_query(&mid, 4), ("see  and more".to_owned(), 4));
    }

    #[test]
    fn a_reply_carries_the_paths_after_its_words() {
        let files = [PathBuf::from("/a/b.rs"), PathBuf::from("/my docs/c.pdf")];
        assert_eq!(
            reply(" fix it ", &files),
            "fix it /a/b.rs \"/my docs/c.pdf\""
        );
        assert_eq!(reply("", &files[..1]), "/a/b.rs");
        assert_eq!(reply("hi", &[]), "hi");
    }

    #[test]
    fn a_chip_names_its_file_from_the_agents_folder() {
        let cwd = Path::new("/repo");
        assert_eq!(label(Path::new("/repo/src/a.rs"), Some(cwd)), "src/a.rs");
        assert_eq!(label(Path::new("/elsewhere/b.txt"), Some(cwd)), "b.txt");
        assert_eq!(label(Path::new("/elsewhere/b.txt"), None), "b.txt");
    }

    #[test]
    fn copied_files_are_read_from_their_uri_list() {
        let list = "# copied\r\nfile:///home/me/My%20File.txt\r\nhttps://x.y/z\r\nfile://host/etc/hosts\r\n";
        assert_eq!(
            uri_paths(list),
            [
                PathBuf::from("/home/me/My File.txt"),
                PathBuf::from("/etc/hosts")
            ]
        );
        assert!(uri_paths("https://x.y/z").is_empty());
        assert_eq!(unescape("a%2"), "a%2");
    }

    #[test]
    fn a_sent_file_keeps_a_safe_form_of_its_name() {
        let dir = std::env::temp_dir().join(format!("gerdr-chat-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = save_in(&dir, "../../my notes.txt", b"hi").unwrap();
        let b = save_in(&dir, "../../my notes.txt", b"hi").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent().unwrap(), dir);
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-my-notes.txt")
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"hi");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name(""), "file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_without_git_is_walked_hidden_folders_left_out() {
        let dir = std::env::temp_dir().join(format!("gerdr-chat-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        std::fs::write(dir.join(".hidden/b"), "").unwrap();
        std::fs::write(dir.join("top.md"), "").unwrap();
        let mut found = walk(&dir);
        found.sort();
        assert_eq!(found, ["src/a.rs", "top.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
