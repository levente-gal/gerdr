//! The project's memory that spans a multi-repository feature's
//! repositories: terms and decisions its Build agent keeps under the
//! feature data dir's `memory/` (`GLOSSARY.md`, and one `adr/<slug>.md` per
//! decision), put into the project's home folder once the feature ships:
//! the terms merged into its `GLOSSARY.md` by name, the decisions numbered
//! on in its `docs/adr/`, and each repository's own glossary named in its
//! `GLOSSARY-MAP.md`. In a home repository, just those files are committed.
//! A single repository's memory is in the feature's own diff instead.
//!
//! The Build check: every decision marked `ADR: yes` in the decision log
//! has a decision record whose `Source:` line names it (`adr_lacks`).
//!
//! Runs git: call `apply` off the UI thread (checklist D3).

use std::path::{Path, PathBuf};

use super::layout::git;

/// The folder under the feature data dir its Build agent keeps it in.
pub const DIR: &str = "memory";

/// The glossary's entries in `text`, in order: each term's name, and its
/// lines up to the next entry or heading. An entry starts with its term in
/// bold and a colon (`**Invoice**:`), as a list item or not.
pub fn terms(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if let Some(name) = term_of(line) {
            found.push((name, format!("{line}\n")));
            open = true;
        } else if line.trim_start().starts_with('#') {
            open = false;
        } else if open && let Some((_, block)) = found.last_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    for (_, block) in &mut found {
        let kept = block.trim_end().len();
        block.truncate(kept);
        block.push('\n');
    }
    found
}

/// The term an entry starting at `line` defines: `**Invoice**:`.
fn term_of(line: &str) -> Option<String> {
    let line = line.trim_start();
    let line = line.strip_prefix("- ").unwrap_or(line);
    let rest = line.strip_prefix("**")?;
    let (name, after) = rest.split_once("**")?;
    let after = after.trim_start();
    (after.starts_with(':') && !name.trim().is_empty()).then(|| name.trim().to_owned())
}

/// Glossary `existing` (none: a new one titled `title`) with each entry of
/// `staged` in it: in place of an entry for the same term (case aside),
/// else added at its end. Also how many entries came in.
pub fn merge_glossary(existing: Option<&str>, title: &str, staged: &str) -> (String, usize) {
    let mut text = existing.map_or_else(
        || format!("# {title}\n\n## Language\n"),
        |t| t.trim_end().to_owned() + "\n",
    );
    let incoming = terms(staged);
    for (name, block) in &incoming {
        let block = block
            .trim_start()
            .strip_prefix("- ")
            .unwrap_or(block.trim_start());
        let old = terms(&text)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name));
        match old.and_then(|(_, old)| text.find(&old).map(|at| (at, old.len()))) {
            Some((at, len)) => text.replace_range(at..at + len, block),
            None => text = format!("{}\n\n{block}", text.trim_end()),
        }
    }
    (text.trim_end().to_owned() + "\n", incoming.len())
}

/// The number the next decision record in `dir` takes: one past the
/// highest `NNNN-…md` there.
pub fn next_adr(dir: &Path) -> u32 {
    let names = std::fs::read_dir(dir).into_iter().flatten().flatten();
    (names.filter_map(|e| adr_number(&e.file_name().to_string_lossy())))
        .max()
        .map_or(1, |n| n + 1)
}

fn adr_number(name: &str) -> Option<u32> {
    let (digits, rest) = name.split_once('-')?;
    (rest.ends_with(".md") && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Glossary map `existing` (none: a new one) naming each of `repos`'
/// glossaries, by repository folder name, that it doesn't name yet.
pub fn merge_map(existing: Option<&str>, repos: &[String]) -> String {
    let mut text = existing.map_or_else(
        || "# Glossary Map\n\n## Contexts\n".to_owned(),
        |t| t.trim_end().to_owned() + "\n",
    );
    for repo in repos {
        let link = format!("(./{repo}/GLOSSARY.md)");
        if !text.contains(&link) {
            text.push_str(&format!("- [{repo}]{link}: the terms of {repo}\n"));
        }
    }
    text
}

/// Put the memory kept in feature data folder `data`'s `memory/` into home
/// folder `root`, titled `title` where it starts a glossary, with
/// `repos` (those whose worktree has a `GLOSSARY.md`) named in the map, and
/// commit just those files when `root` is a repository's top, with message
/// `message`. What it did, for the feature's log and a toast: none when
/// there was nothing to put.
pub fn apply(
    root: &Path,
    data: &Path,
    title: &str,
    repos: &[String],
    message: &str,
) -> Result<Option<String>, String> {
    let staged = data.join(DIR);
    let mut written: Vec<PathBuf> = Vec::new();
    let mut said = Vec::new();
    let write = |path: &Path, text: &str| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    };
    if let Ok(text) = std::fs::read_to_string(staged.join("GLOSSARY.md")) {
        let path = root.join("GLOSSARY.md");
        let existing = std::fs::read_to_string(&path).ok();
        let (merged, n) = merge_glossary(existing.as_deref(), title, &text);
        if n > 0 {
            write(&path, &merged)?;
            written.push(path);
            said.push(count(n, "term"));
        }
    }
    let adr_dir = root.join("docs/adr");
    let mut adrs: Vec<PathBuf> = (std::fs::read_dir(staged.join("adr")).into_iter())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    adrs.sort();
    for adr in &adrs {
        let text = std::fs::read_to_string(adr).map_err(|e| format!("{}: {e}", adr.display()))?;
        let stem = adr.file_stem().unwrap_or_default().to_string_lossy();
        let stem = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
        let path = adr_dir.join(format!("{:04}-{stem}.md", next_adr(&adr_dir)));
        write(&path, &text)?;
        written.push(path);
    }
    if !adrs.is_empty() {
        said.push(count(adrs.len(), "decision"));
    }
    if !repos.is_empty() {
        let path = root.join("GLOSSARY-MAP.md");
        let existing = std::fs::read_to_string(&path).ok();
        let merged = merge_map(existing.as_deref(), repos);
        if existing.as_deref() != Some(merged.as_str()) {
            write(&path, &merged)?;
            written.push(path);
        }
    }
    if written.is_empty() {
        return Ok(None);
    }
    let what = if said.is_empty() {
        "the glossary map".to_owned()
    } else {
        said.join(" and ")
    };
    let top = git(root, &["rev-parse", "--show-toplevel"]).ok();
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    let is_top = top.is_some_and(|t| same(Path::new(t.trim()), root));
    if !is_top {
        return Ok(Some(format!(
            "added {what} to the project's memory (not committed: its home folder isn't a git repository)"
        )));
    }
    let paths: Vec<String> = (written.iter())
        .filter_map(|p| p.strip_prefix(root).ok())
        .map(|p| p.display().to_string())
        .collect();
    let mut add = vec!["add", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(root, &add).map_err(|e| format!("couldn't add {what}: {e}"))?;
    // Only these paths: whatever else is staged there stays as it was.
    let mut commit = vec!["commit", "-q", "-m", message, "--"];
    commit.extend(paths.iter().map(String::as_str));
    git(root, &commit).map_err(|e| format!("couldn't commit {what}: {e}"))?;
    Ok(Some(format!("added {what} to the project's memory")))
}

fn count(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// The decisions of decision log `log` marked `ADR: yes`, not replaced, that
/// no decision record in `dirs` names in a `Source:` line as
/// `<slug> D<n>`: one line each, for the Build check.
pub fn adr_lacks(log: &str, slug: &str, dirs: &[PathBuf]) -> Vec<String> {
    let wanted = super::artifacts::adr_decisions(log);
    if wanted.is_empty() {
        return Vec::new();
    }
    let sources: Vec<String> = (dirs.iter())
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|text| {
            (text.lines())
                .filter_map(|l| {
                    l.trim()
                        .strip_prefix("Source:")
                        .map(|s| s.trim().to_owned())
                })
                .collect::<Vec<_>>()
        })
        .collect();
    (wanted.into_iter())
        .filter(|n| {
            let id = format!("{slug} D{n}");
            !sources
                .iter()
                .any(|s| s.split(',').any(|one| one.trim() == id))
        })
        .map(|n| {
            format!(
                "decision D{n} is marked ADR: yes but no docs/adr/ file says “Source: {slug} D{n}”"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGED: &str = "- **Invoice**: a request for payment for an **Order**.\n  _Avoid_: bill.\n\n- **Order**: what a **Customer** buys.\n";

    #[test]
    fn entries_are_read_by_their_bold_terms() {
        let found = terms(STAGED);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Invoice", "Order"]);
        assert_eq!(
            found[0].1,
            "- **Invoice**: a request for payment for an **Order**.\n  _Avoid_: bill.\n"
        );
        // A heading ends an entry; bold text that isn't a term isn't one.
        let text = "**Order**:\nWhat is bought.\n## More\nNot it.\n**note** this\n";
        assert_eq!(
            terms(text),
            [("Order".into(), "**Order**:\nWhat is bought.\n".into())]
        );
    }

    #[test]
    fn a_glossary_takes_new_terms_and_replaces_known_ones() {
        let (fresh, n) = merge_glossary(None, "Shop", STAGED);
        assert_eq!(n, 2);
        assert!(
            fresh.starts_with("# Shop\n\n## Language\n\n**Invoice**:"),
            "{fresh}"
        );
        let old = "# Shop\n\n## Language\n\n**order**:\nOld meaning.\n\n**Cart**:\nA basket.\n";
        let (merged, _) = merge_glossary(Some(old), "Shop", STAGED);
        assert_eq!(
            merged,
            "# Shop\n\n## Language\n\n**Order**: what a **Customer** buys.\n\n**Cart**:\nA basket.\n\n**Invoice**: a request for payment for an **Order**.\n  _Avoid_: bill.\n"
        );
    }

    #[test]
    fn decisions_and_maps_number_and_name_on() {
        let root = std::env::temp_dir().join(format!("gerdr-memory-{}", std::process::id()));
        let adr = root.join("docs/adr");
        std::fs::create_dir_all(&adr).unwrap();
        assert_eq!(next_adr(&adr), 1);
        std::fs::write(adr.join("0007-csv.md"), "x").unwrap();
        std::fs::write(adr.join("notes.md"), "x").unwrap();
        assert_eq!(next_adr(&adr), 8);
        std::fs::remove_dir_all(&root).unwrap();
        let map = merge_map(None, &["api".into()]);
        assert_eq!(
            map,
            "# Glossary Map\n\n## Contexts\n- [api](./api/GLOSSARY.md): the terms of api\n"
        );
        assert_eq!(merge_map(Some(&map), &["api".into()]), map);
    }

    #[test]
    fn memory_goes_to_the_home_repository_in_one_commit_of_its_own() {
        let root = std::env::temp_dir().join(format!("gerdr-memory-home-{}", std::process::id()));
        let data = root.join("data");
        let home = root.join("home");
        std::fs::create_dir_all(data.join("memory/adr")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(data.join("memory/GLOSSARY.md"), STAGED).unwrap();
        std::fs::write(
            data.join("memory/adr/one-api.md"),
            "# One API\n\nSource: shop D2\n",
        )
        .unwrap();
        let run = |args: &[&str]| git(&home, args).unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(home.join("other.txt"), "staged, not ours").unwrap();
        run(&["add", "other.txt"]);
        let did = apply(&home, &data, "Shop", &["api".into()], "Memory of shop").unwrap();
        assert_eq!(
            did.as_deref(),
            Some("added 2 terms and 1 decision to the project's memory")
        );
        assert!(home.join("docs/adr/0001-one-api.md").is_file());
        let files = run(&["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(
            files,
            "GLOSSARY-MAP.md\nGLOSSARY.md\ndocs/adr/0001-one-api.md\n"
        );
        // What else was staged stays staged.
        assert_eq!(run(&["diff", "--cached", "--name-only"]), "other.txt\n");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_decision_marked_for_a_record_needs_one_naming_it() {
        let log = "## D1: CSV\n- Decided: CSV.\n- ADR: yes\n\n## D2: Pages\n- Decided: none.\n\n## D3: Old\n- Decided: x.\n- ADR: yes\n- Replaced by: D4\n";
        let root = std::env::temp_dir().join(format!("gerdr-memory-adr-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let lacks = adr_lacks(log, "shop", std::slice::from_ref(&root));
        assert_eq!(lacks.len(), 1, "{lacks:?}");
        assert!(lacks[0].contains("D1"));
        std::fs::write(root.join("0001-csv.md"), "# CSV\n\nSource: shop D1\n").unwrap();
        assert!(adr_lacks(log, "shop", std::slice::from_ref(&root)).is_empty());
        // Another feature's D1 isn't this one's.
        assert_eq!(adr_lacks(log, "cart", &[root.clone()]).len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
