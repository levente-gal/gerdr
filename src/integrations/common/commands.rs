//! Reading an agent's commands on disk, for the chat's `/` menu: command
//! files, skills, and what their front matter says.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use crate::chat::commands::Command;

/// The `*.md` files right in `dir`, by name without `.md`, in name order.
pub fn md_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_owned(), p)))
        .collect();
    files.sort();
    files
}

/// The folders right in `dir`, by name, in name order.
pub fn sub_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_owned(), p)))
        .collect();
    dirs.sort();
    dirs
}

/// The `*.md` files in `dir` as commands named `<prefix><name>`.
pub fn tree(dir: &Path, prefix: &str, found: &mut Vec<Command>) {
    for (name, path) in md_files(dir) {
        found.push(read_md(&path, format!("{prefix}{name}")));
    }
}

/// The skills in `dir` (`<name>/SKILL.md`) as commands `<prefix><name>`.
pub fn skills(dir: &Path, prefix: &str, found: &mut Vec<Command>) {
    for (name, sub) in sub_dirs(dir) {
        let path = sub.join("SKILL.md");
        if path.is_file() {
            found.push(read_md(&path, format!("{prefix}{name}")));
        }
    }
}

/// The skills in `dir` named as their front matter says (else by folder),
/// as `<prefix><name>`; those with `disable-slash-command` left out.
pub fn named_skills(dir: &Path, prefix: &str, found: &mut Vec<Command>) {
    for (folder, sub) in sub_dirs(dir) {
        let path = sub.join("SKILL.md");
        let front = front_matter(&fs::read_to_string(&path).unwrap_or_default());
        let hidden = front.get("disable-slash-command").and_then(|v| v.as_bool());
        if !path.is_file() || hidden == Some(true) {
            continue;
        }
        let name = front
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&folder);
        found.push(read_md(&path, format!("{prefix}{name}")));
    }
}

/// Command `name` from markdown file `path`, with what its front matter
/// says: `description` and `argument-hint`.
pub fn read_md(path: &Path, name: String) -> Command {
    let text = fs::read_to_string(path).unwrap_or_default();
    let front = front_matter(&text);
    let field = |key: &str| {
        front
            .get(key)
            .and_then(|v| v.as_str())
            .map(one_line)
            .unwrap_or_default()
    };
    Command {
        name,
        about: field("description"),
        hint: field("argument-hint"),
        mention: false,
    }
}

/// The YAML front matter at the top of `text`, as text fields by key.
fn front_matter(text: &str) -> HashMap<String, serde_norway::Value> {
    let Some(rest) = text.strip_prefix("---") else {
        return HashMap::new();
    };
    let Some(end) = rest.find("\n---") else {
        return HashMap::new();
    };
    serde_norway::from_str(&rest[..end]).unwrap_or_default()
}

/// `text` on one line, its runs of whitespace one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
