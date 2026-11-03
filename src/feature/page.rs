//! What the feature tab's stage pages show beyond the feature's own
//! record, read off the UI thread: the stage files there and their
//! milestones by name (with the probe, `ship::probe`), each touched
//! repository's line stat (`git diff --numstat`, with the probe too), and
//! the last lines of the current stage agent's pane (`tail`, every
//! `TAIL_EVERY` while the tab shows).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    Repo, Stage, artifacts,
    layout::{base_ref, git},
};
use crate::herdr::Herdr;

/// How often the agent's tail is read while the feature tab shows.
pub const TAIL_EVERY: Duration = Duration::from_secs(2);
/// How many of the pane's last lines the tail keeps.
pub const TAIL_LINES: usize = 3;
/// How many lines a read of the pane takes, for `TAIL_LINES` with text.
const READ_LINES: usize = 40;

/// The stage files of a feature's data dir, as last read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Docs {
    /// The stages whose file is there (`brief.md`, `plan.md`, `build.md`).
    pub written: BTreeSet<Stage>,
    /// Plan's and Build's milestones by name, `true` = ticked (see
    /// `artifacts::milestone_items`); none for a file not there.
    pub milestones: BTreeMap<Stage, Vec<(bool, String)>>,
}

/// Read the stage files in feature data dir `data`. A file that can't be
/// read counts as not there: its stage's page says it isn't written, and
/// Review says why when pressed.
pub fn read_docs(data: &Path) -> Docs {
    let mut docs = Docs::default();
    for stage in [Stage::Discover, Stage::Plan, Stage::Build] {
        let Some(path) = artifacts::path(data, stage) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        docs.written.insert(stage);
        if stage == Stage::Discover {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                docs.milestones
                    .insert(stage, artifacts::milestone_items(&text));
            }
            Err(err) => tracing::warn!("reading {}: {err}", path.display()),
        }
    }
    docs
}

/// Lines added and deleted in `git diff --numstat` output `out`; binary
/// files ("-\t-\tpath") count for nothing.
pub fn numstat(out: &str) -> (usize, usize) {
    out.lines().fold((0, 0), |(add, del), line| {
        let mut cols = line.split('\t');
        let parse = |c: Option<&str>| c.and_then(|c| c.trim().parse::<usize>().ok());
        match (parse(cols.next()), parse(cols.next())) {
            (Some(a), Some(d)) => (add + a, del + d),
            _ => (add, del),
        }
    })
}

/// The lines touched repository `repo` adds and deletes against its base:
/// its commits since it left the base, and its uncommitted changes.
pub fn stat(repo: &Repo) -> Result<(usize, usize), String> {
    let name = repo.key();
    let dir = &repo.worktree;
    let range = format!("{}...HEAD", base_ref(repo));
    let flags = ["diff", "--numstat", "--no-ext-diff", "--no-textconv"];
    let read = |what: &str| {
        let args: Vec<&str> = flags.iter().copied().chain([what]).collect();
        git(dir, &args).map_err(|err| format!("{name}: couldn't count its lines: {err}"))
    };
    let committed = numstat(&read(&range)?);
    let uncommitted = numstat(&read("HEAD")?);
    Ok((committed.0 + uncommitted.0, committed.1 + uncommitted.1))
}

/// The line stat of each touched repository of `repos`, by `Repo::key`;
/// one that can't be read is left out, so its row shows no stat rather
/// than a wrong one. Only to debug: the probe runs every few seconds, and
/// the same git failing fails the probe's reading too, which the Repos
/// column and the log say once.
pub fn stats<'a>(repos: impl Iterator<Item = &'a Repo>) -> BTreeMap<String, (usize, usize)> {
    repos
        .filter_map(|repo| match stat(repo) {
            Ok(stat) => Some((repo.key(), stat)),
            Err(why) => {
                tracing::debug!("feature line stat: {why}");
                None
            }
        })
        .collect()
}

/// The last `TAIL_LINES` lines of `text` worth showing: box drawing and
/// block characters gone, runs of spaces one, lines with no letter left
/// out (rules, prompts, spinners).
pub fn tail(text: &str) -> Vec<String> {
    let boxy = |c: char| ('\u{2500}'..='\u{259f}').contains(&c);
    let mut out: Vec<String> = text
        .lines()
        .map(|line| {
            let line: String = line
                .chars()
                .map(|c| if boxy(c) { ' ' } else { c })
                .collect();
            line.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .filter(|line| line.chars().any(char::is_alphabetic))
        .collect();
    let skip = out.len().saturating_sub(TAIL_LINES);
    out.drain(..skip);
    out
}

/// Read pane `pane`'s tail from the herdr server on `socket` (blocking:
/// run it off the UI thread). Its scrollback first; an app on the
/// alternate screen has none worth showing, so its screen then.
pub fn read_tail(socket: PathBuf, pane: &str) -> Result<Vec<String>, String> {
    let mut herdr = Herdr::on_socket(socket);
    let lines = tail(&herdr.pane_read(pane, "recent_unwrapped", READ_LINES)?);
    if !lines.is_empty() {
        return Ok(lines);
    }
    Ok(tail(&herdr.pane_read(pane, "visible", READ_LINES)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_sums_text_files_and_skips_binary_ones() {
        let out = "3\t1\tsrc/a.rs\n-\t-\tlogo.png\n10\t0\tdocs/b.md\n\n";
        assert_eq!(numstat(out), (13, 1));
        assert_eq!(numstat(""), (0, 0));
        assert_eq!(numstat("garbage line\n"), (0, 0));
    }

    #[test]
    fn the_tail_drops_box_drawing_and_lines_without_letters() {
        let screen = "\
╭──────────────────────────╮
│ Reading src/main.rs      │
│                          │
╰──────────────────────────╯
⏺ Editing   two   files
  ⎿  Updated src/a.rs
────────────
>
  ? for shortcuts
";
        assert_eq!(
            tail(screen),
            vec![
                "⏺ Editing two files",
                "⎿ Updated src/a.rs",
                "? for shortcuts"
            ]
        );
        assert_eq!(tail("one\n\ntwo\n"), vec!["one", "two"]);
        assert!(tail("───\n> \n 42 \n").is_empty());
    }

    #[test]
    fn the_docs_are_the_files_there_and_their_milestones() {
        let dir = std::env::temp_dir().join(format!("gerdr-page-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("brief.md"), "# Brief\n").unwrap();
        std::fs::write(dir.join("plan.md"), "## Milestones\n- [x] API\n- [ ] UI\n").unwrap();
        let docs = read_docs(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(docs.written, BTreeSet::from([Stage::Discover, Stage::Plan]));
        assert_eq!(
            docs.milestones,
            BTreeMap::from([(
                Stage::Plan,
                vec![(true, "API".to_owned()), (false, "UI".to_owned())]
            )])
        );
    }
}
