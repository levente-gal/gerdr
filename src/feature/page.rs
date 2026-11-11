//! What the feature tab's stage pages show beyond the feature's own
//! record, read off the UI thread: the stage files there, the blocks of
//! those shown on their pages, and their milestones by name (with the
//! probe, `ship::probe`), each touched
//! repository's stat (files and lines changed, with the probe too), and
//! the last lines of the current stage agent's pane (`tail`, every
//! `TAIL_EVERY` while the tab shows).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use super::{
    Feature, Repo, Stage, artifacts, fix_round,
    layout::{base_ref, git},
    review_import::FindingState,
    verify,
};
use crate::{
    herdr::Herdr,
    md_blocks::{self, DocBlock},
    md_preview,
};

/// How often the agent's tail is read while the feature tab shows.
pub const TAIL_EVERY: Duration = Duration::from_secs(2);
/// How many of the pane's last lines the tail keeps.
pub const TAIL_LINES: usize = 3;
/// How many lines a read of the pane takes, for `TAIL_LINES` with text.
const READ_LINES: usize = 40;

/// The stage files of a feature's data dir, as last read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Docs {
    /// The stages whose file is there (`artifacts::outputs`).
    pub written: BTreeSet<Stage>,
    /// The approved stages whose file changed since, with no way to tell
    /// by whom: an agent's tab left open, or a hand.
    pub changed: BTreeSet<Stage>,
    /// Build's milestones by name, `true` = ticked (see
    /// `artifacts::milestone_items`); none for a file not there.
    pub milestones: BTreeMap<Stage, Vec<(bool, String)>>,
    /// Which of Build's milestones its agent says it is at work on (see
    /// `artifacts::milestones_running`).
    pub milestones_running: Vec<usize>,
    /// The fixes each fix round listed in the build log, by round, oldest
    /// first (see `fix_round::rounds`).
    pub fix_rounds: Vec<(u32, Vec<(bool, String)>)>,
    /// The rounds whose plans were put aside for the next (see
    /// `fix_round::archive_path`), by number.
    pub plans: BTreeSet<u32>,
    /// Whether the latest round's plan is there (`fix_round::FILE`).
    pub plan: bool,
    /// The brief and the plan as their pages show them; none for a file
    /// not there, and a note for one that can't be read.
    pub previews: BTreeMap<Stage, Arc<Vec<DocBlock>>>,
    /// How many comments the user left in the feature's diff (`notes`).
    pub comments: usize,
    /// Its summary (`SUMMARY_FILE`) and when it was last written, once there;
    /// why it couldn't be read, if so.
    pub summary: Option<(Result<String, String>, u64)>,
    /// The comments the user left in the feature's diff: they go to Build
    /// with AI review's fixes, or wait for Code review. After them, flagged
    /// sent, those the latest of Code review's fix rounds took (`round_notes`).
    pub notes: Vec<crate::review::Comment>,
    /// The comments Code review's earlier fix rounds took, by round.
    pub old_notes: BTreeMap<u32, Vec<crate::review::Comment>>,
    /// The fix plan of the latest of Code review's fix rounds, as its page
    /// shows it.
    pub comment_plan: Option<Arc<Vec<DocBlock>>>,
}

impl Docs {
    /// These docs with the previews of `old` that are the same: what they
    /// painted is kept by what it was drawn from, and needn't be compared
    /// word by word every frame after every read.
    pub fn keeping(mut self, old: Option<&Docs>) -> Docs {
        for (stage, blocks) in &mut self.previews {
            if let Some(same) = old
                .and_then(|o| o.previews.get(stage))
                .filter(|o| *o == blocks)
            {
                *blocks = same.clone();
            }
        }
        if let (Some(blocks), Some(old)) = (&mut self.comment_plan, old)
            && let Some(same) = old.comment_plan.as_ref().filter(|o| *o == blocks)
        {
            *blocks = same.clone();
        }
        self
    }

    /// These docs with what `feature`'s Code review rounds left in data dir
    /// `data`.
    pub fn with_comment_fixes(mut self, data: &Path, feature: &Feature) -> Docs {
        let mut rounds = feature.comment_fixes.iter().map(|fix| fix.round).rev();
        let latest = rounds.next().or_else(|| {
            (feature.fix_round.as_ref())
                .filter(|round| round.stage == Stage::CodeReview)
                .map(|round| round.n)
        });
        let Some(latest) = latest else {
            return self;
        };
        self.notes.extend(sent_notes(data, latest));
        self.old_notes = rounds.map(|n| (n, sent_notes(data, n))).collect();
        // Once the next round starts its plan is put away.
        let path = match latest == feature.fix_rounds {
            true => fix_round::path(data),
            false => fix_round::archive_path(data, latest),
        };
        self.comment_plan = match preview_of(&path) {
            Ok(blocks) => Some(Arc::new(blocks)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                let note = DocBlock::Note(format!("Couldn't read this file: {err}"));
                Some(Arc::new(vec![note]))
            }
        };
        self
    }
}

/// The comments fix round `n` took from Code review, as they went: each flagged
/// sent, under the id the agent had it by, so they are never saved as the user's.
fn sent_notes(data: &Path, n: u32) -> Vec<crate::review::Comment> {
    let path = fix_round::comments_copy(data, n);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        // Keeping it failed, which said so, or the round is from before there
        // were cards to show it on.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            return Vec::new();
        }
    };
    let kept = match serde_json::from_slice::<crate::review::Review>(&raw) {
        Ok(kept) => kept,
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            return Vec::new();
        }
    };
    (kept.without_findings().comments.into_iter().enumerate())
        .map(|(at, mut comment)| {
            comment.flag = Some(crate::review::Flag {
                id: crate::review::comment_id(at),
                state: FindingState::Sent,
                yours: true,
                sent: true,
                ..crate::review::Flag::default()
            });
            comment
        })
        .collect()
}

/// What a stage's page shows of a file too big to read through there.
const TOO_BIG: &str = "Too big to show here: use Review";

/// The blocks of stage file `path` for its page, if it can be read. One
/// `md_preview` wouldn't render either gets a note instead.
fn preview_of(path: &Path) -> Result<Vec<DocBlock>, std::io::Error> {
    if std::fs::metadata(path)?.len() > md_preview::FILE_MAX {
        return Ok(vec![DocBlock::Note(TOO_BIG.to_owned())]);
    }
    Ok(md_blocks::blocks(&std::fs::read_to_string(path)?))
}

/// The comments of the user's the diff of the feature with data dir `data`
/// holds. A bad file counts as none: opening the diff says so, and sets it
/// aside.
fn own_comments(data: &Path) -> Vec<crate::review::Comment> {
    std::fs::read(data.join(crate::feature_diff::COMMENTS))
        .ok()
        .and_then(|raw| serde_json::from_slice::<crate::review::Review>(&raw).ok())
        .map_or_else(Vec::new, |review| review.without_findings().comments)
}

/// The rounds whose plans are put aside under feature data dir `data`.
fn archived_plans(data: &Path) -> BTreeSet<u32> {
    let Ok(dir) = std::fs::read_dir(data.join("prompts")) else {
        return BTreeSet::new();
    };
    (dir.flatten())
        .filter_map(|entry| {
            let name = entry.file_name();
            let n = name.to_str()?.strip_prefix("fix-plan-")?;
            n.strip_suffix(".md")?.parse().ok()
        })
        .collect()
}

/// The summary in data dir `data`, and when it was written (unix seconds), if
/// there is one.
pub fn read_summary(data: &Path) -> Option<(Result<String, String>, u64)> {
    let path = data.join(super::SUMMARY_FILE);
    let meta = std::fs::metadata(&path).ok()?;
    let at = (meta.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let text = std::fs::read_to_string(&path).map_err(|err| {
        tracing::warn!("reading {}: {err}", path.display());
        err.to_string()
    });
    Some((text, at))
}

/// Read the stage files in feature data dir `data`. A file that can't be
/// read is still there: its stage's page shows why in its place, and Review
/// says so too when pressed.
pub fn read_docs(data: &Path) -> Docs {
    let notes = own_comments(data);
    let mut docs = Docs {
        written: artifacts::outputs(data),
        comments: notes.len(),
        notes,
        ..Docs::default()
    };
    docs.plans = archived_plans(data);
    docs.plan = fix_round::path(data).exists();
    docs.summary = read_summary(data);
    docs.changed = (docs.written.iter().copied())
        .filter(|stage| artifacts::changed_since_approved(data, *stage))
        .collect();
    for stage in [
        Stage::Discover,
        Stage::Plan,
        Stage::PlanReview,
        Stage::Build,
    ] {
        let Some(path) = artifacts::path(data, stage).filter(|_| docs.written.contains(&stage))
        else {
            continue;
        };
        if stage != Stage::Build {
            match preview_of(&path) {
                Ok(blocks) => {
                    docs.previews.insert(stage, Arc::new(blocks));
                }
                Err(err) => {
                    tracing::warn!("reading {}: {err}", path.display());
                    let note = DocBlock::Note(format!("Couldn't read this file: {err}"));
                    docs.previews.insert(stage, Arc::new(vec![note]));
                }
            }
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                docs.milestones
                    .insert(stage, artifacts::milestone_items(&text));
                docs.milestones_running = artifacts::milestones_running(&text);
                docs.fix_rounds = fix_round::rounds(&text);
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

/// What a touched repository changes against its base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// The files its code review lists: untracked ones too, which `added`
    /// doesn't count, and a folder untracked whole as one.
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// What touched repository `repo` changes against its base: its commits
/// since it left the base, and its uncommitted changes.
pub fn stat(repo: &Repo) -> Result<Stat, String> {
    let name = repo.key();
    let dir = &repo.worktree;
    let base = base_ref(repo);
    let range = format!("{base}...HEAD");
    let flags = ["diff", "--numstat", "--no-ext-diff", "--no-textconv"];
    let read = |what: &str| {
        let args: Vec<&str> = flags.iter().copied().chain([what]).collect();
        git(dir, &args).map_err(|err| format!("{name}: couldn't count its lines: {err}"))
    };
    let committed = numstat(&read(&range)?);
    let uncommitted = numstat(&read("HEAD")?);
    let since = crate::git_ops::changes_since(dir, Some(&base))
        .map_err(|err| format!("{name}: couldn't count its files: {err}"))?;
    Ok(Stat {
        files: since.changes.len(),
        added: committed.0 + uncommitted.0,
        removed: committed.1 + uncommitted.1,
    })
}

/// The stat of each touched repository of `repos`, by `Repo::key`;
/// one that can't be read is left out, so its row shows no stat rather
/// than a wrong one. Only to debug: the probe runs every few seconds, and
/// the same git failing fails the probe's reading too, which the Repos
/// column and the log say once.
pub fn stats<'a>(repos: impl Iterator<Item = &'a Repo>) -> BTreeMap<String, Stat> {
    repos
        .filter_map(|repo| match stat(repo) {
            Ok(stat) => Some((repo.key(), stat)),
            Err(why) => {
                tracing::debug!("feature stat: {why}");
                None
            }
        })
        .collect()
}

/// What a fix round's fixes changed in one repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundStat {
    /// `Repo::key`.
    pub repo: String,
    pub commits: usize,
    pub stat: Stat,
}

/// How many of a feature's latest rounds `round_stats` reads.
const ROUND_STATS: usize = 12;

/// What the fixes of each of `feature`'s latest rounds changed, by round
/// number: from where each repository stood before them to after them, or
/// to its HEAD while they go on. A repository whose code didn't move is left
/// out; one that can't be read makes the round's why.
pub fn round_stats(feature: &Feature) -> BTreeMap<u32, Result<Vec<RoundStat>, String>> {
    let mut rounds: Vec<&fix_round::FixRound> = (feature.fix_round.iter())
        .chain(&feature.last_fix_round)
        .chain(&feature.past_rounds)
        .filter(|r| !r.trail.from.is_empty())
        .collect();
    rounds.sort_by_key(|r| std::cmp::Reverse(r.n));
    rounds.dedup_by_key(|r| r.n);
    let mut out = BTreeMap::new();
    for round in rounds.into_iter().take(ROUND_STATS) {
        let mut stats = Ok(Vec::new());
        for (key, from, to) in fix_round::span(round) {
            let Some(repo) = feature.repo(&key).filter(|r| r.worktree.is_dir()) else {
                stats = Err(format!("{key} isn't there"));
                break;
            };
            let measured = match to {
                // Both ends are fixed commits: the answer never changes.
                Some(to) => {
                    let span = (repo.worktree.clone(), from, to);
                    let kept = SPANS.lock().ok().and_then(|m| m.get(&span).copied());
                    kept.map(Ok).unwrap_or_else(|| {
                        let measured = span_stat(&span.0, &span.1, &span.2);
                        if let (Ok(m), Ok(mut spans)) = (&measured, SPANS.lock()) {
                            spans.insert(span, *m);
                        }
                        measured
                    })
                }
                None => span_stat(&repo.worktree, &from, "HEAD"),
            };
            match measured {
                Ok((commits, stat)) => {
                    if (stat.files > 0 || commits > 0)
                        && let Ok(stats) = stats.as_mut()
                    {
                        stats.push(RoundStat {
                            repo: key,
                            commits,
                            stat,
                        });
                    }
                }
                // The card says so: the probe reads it again in a few seconds.
                Err(why) => {
                    stats = Err(format!("{key}: {why}"));
                    break;
                }
            }
        }
        out.insert(round.n, stats);
    }
    out
}

/// Finished rounds' spans measured already, by worktree, from and to.
static SPANS: LazyLock<Mutex<HashMap<(PathBuf, String, String), (usize, Stat)>>> =
    LazyLock::new(Default::default);

/// The commits and stat of `worktree` from `from` to `to`.
fn span_stat(worktree: &Path, from: &str, to: &str) -> Result<(usize, Stat), String> {
    let args = [
        "diff",
        "--numstat",
        "--no-ext-diff",
        "--no-textconv",
        from,
        to,
    ];
    let out = git(worktree, &args)?;
    let (added, removed) = numstat(&out);
    let files = out.lines().filter(|l| !l.trim().is_empty()).count();
    let commits = crate::git_ops::count_commits(worktree, &format!("{from}..{to}"));
    Ok((
        commits,
        Stat {
            files,
            added,
            removed,
        },
    ))
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

/// Read what the card of the check running in pane `pane` previews, which
/// `began` or not (see `verify::preview`), from the herdr server on `socket`
/// (blocking: run it off the UI thread).
pub fn read_check_tail(socket: PathBuf, pane: &str, began: bool) -> Result<Vec<String>, String> {
    let read = verify::PREVIEW_READ;
    let text = Herdr::on_socket(socket).pane_read(pane, "recent_unwrapped", read)?;
    Ok(verify::preview(&text, began))
}

#[cfg(test)]
mod comment_fix_tests {
    use super::*;
    use crate::{
        feature::fix_round::{CommentFix, FixPhase, FixRound, GiveBack},
        review::{Comment, Review},
    };

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gerdr-comment-fixes-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        dir
    }

    fn keep(data: &Path, n: u32, texts: &[&str]) {
        let review = Review {
            comments: (texts.iter())
                .map(|text| Comment {
                    text: (*text).to_owned(),
                    ..Comment::default()
                })
                .collect(),
            ..Review::default()
        };
        crate::review::save_file(&fix_round::comments_copy(data, n), &review).unwrap();
    }

    fn feature_with(rounds: &[u32], last: u32) -> Feature {
        let mut f = crate::features::tests::feature("f", Vec::new(), 0);
        f.comment_fixes = (rounds.iter())
            .map(|n| CommentFix {
                round: *n,
                made: Vec::new(),
            })
            .collect();
        f.fix_rounds = last;
        f
    }

    #[test]
    fn the_latest_rounds_comments_come_after_those_not_sent_flagged() {
        let data = scratch("latest");
        keep(&data, 1, &["Old"]);
        keep(&data, 2, &["New one", "New two"]);
        std::fs::write(fix_round::path(&data), "## Fixes\n### C1\n").unwrap();
        let docs = Docs {
            notes: vec![Comment {
                text: "Not sent".into(),
                ..Comment::default()
            }],
            ..Docs::default()
        }
        .with_comment_fixes(&data, &feature_with(&[1, 2], 2));
        let said: Vec<&str> = docs.notes.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(said, ["Not sent", "New one", "New two"]);
        assert!(docs.notes[0].flag.is_none());
        let flag = docs.notes[2].flag.as_ref().unwrap();
        assert_eq!(flag.id, "C2");
        assert!(flag.sent && flag.yours);
        assert_eq!(flag.state, FindingState::Sent);
        assert_eq!(docs.old_notes[&1][0].text, "Old");
        assert!(docs.comment_plan.is_some());
        // Never what is saved as the user's.
        let review = Review {
            comments: docs.notes,
            ..Review::default()
        };
        assert_eq!(review.without_findings().comments.len(), 1);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_later_rounds_plan_leaves_this_one_read_from_where_it_was_put() {
        let data = scratch("archive");
        keep(&data, 1, &["Mine"]);
        std::fs::write(fix_round::path(&data), "## Fixes\n### Someone else's\n").unwrap();
        std::fs::write(fix_round::archive_path(&data, 1), "## Fixes\n### C1: mine\n").unwrap();
        let docs = Docs::default().with_comment_fixes(&data, &feature_with(&[1], 2));
        let blocks = format!("{:?}", docs.comment_plan.unwrap());
        assert!(blocks.contains("C1: mine"), "{blocks}");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn no_round_of_code_reviews_reads_nothing_and_a_missing_copy_is_no_card() {
        let data = scratch("none");
        std::fs::write(fix_round::path(&data), "## Fixes\n### AI's\n").unwrap();
        let docs = Docs::default().with_comment_fixes(&data, &feature_with(&[], 1));
        assert_eq!(docs, Docs::default());
        // Its copy couldn't be kept: the round still shows, its plan and all.
        let mut f = feature_with(&[], 1);
        f.fix_round = Some(FixRound {
            n: 1,
            what: String::new(),
            input: PathBuf::new(),
            stage: Stage::CodeReview,
            phase: FixPhase::Ready,
            sent_back: 0,
            asked: 0,
            ready: None,
            stalled: None,
            give_back: GiveBack::Nothing,
            items: Vec::new(),
            done: 0,
            ended: None,
            trail: Default::default(),
        });
        let docs = Docs::default().with_comment_fixes(&data, &f);
        assert!(docs.notes.is_empty() && docs.comment_plan.is_some());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn the_same_plan_read_again_keeps_what_painted_it() {
        let data = scratch("keeping");
        keep(&data, 1, &["Mine"]);
        std::fs::write(fix_round::path(&data), "## Fixes\n### C1\n").unwrap();
        let f = feature_with(&[1], 1);
        let first = Docs::default().with_comment_fixes(&data, &f);
        let again = Docs::default()
            .with_comment_fixes(&data, &f)
            .keeping(Some(&first));
        assert!(Arc::ptr_eq(
            first.comment_plan.as_ref().unwrap(),
            again.comment_plan.as_ref().unwrap()
        ));
        let _ = std::fs::remove_dir_all(&data);
    }
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

    /// Files count what the code review lists, untracked ones too, once
    /// each, while lines count only what git diffs.
    #[test]
    fn the_stat_counts_committed_uncommitted_and_untracked_files() {
        let dir = std::env::temp_dir().join(format!("gerdr-page-stat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a"), "one\n").unwrap();
        std::fs::write(dir.join("b"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "base"]);
        run(&["checkout", "-qb", "feature/x"]);
        std::fs::write(dir.join("a"), "one\ntwo\n").unwrap();
        run(&["commit", "-qam", "more"]);
        // Committed and changed again: still one file.
        std::fs::write(dir.join("a"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("b"), "").unwrap();
        std::fs::write(dir.join("new"), "untracked\n").unwrap();
        let repo = Repo {
            name: "x".into(),
            main: dir.clone(),
            base: "main".into(),
            base_commit: String::new(),
            worktree: dir.clone(),
            touched: true,
            branch: None,
            project: None,
        };
        let stat = stat(&repo);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            stat,
            Ok(Stat {
                files: 3,
                added: 2,
                removed: 1,
            })
        );
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
        let milestones = "## Milestones\n- [x] API\n- [ ] UI\n";
        std::fs::write(dir.join("plan.md"), milestones).unwrap();
        let log = format!("{milestones}\n## Fix round 1\n\n- [x] r1-1\n- [ ] r1-2\n");
        std::fs::write(dir.join("build.md"), log).unwrap();
        let docs = read_docs(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            docs.written,
            BTreeSet::from([Stage::Discover, Stage::Plan, Stage::Build])
        );
        let heading = |text: &str| DocBlock::Heading {
            level: 1,
            text: text.to_owned(),
        };
        assert_eq!(
            docs.previews.get(&Stage::Discover).map(|b| b.as_slice()),
            Some([heading("Brief")].as_slice())
        );
        assert!(docs.previews.contains_key(&Stage::Plan));
        assert_eq!(
            docs.milestones,
            BTreeMap::from([(
                Stage::Build,
                vec![(true, "API".to_owned()), (false, "UI".to_owned())]
            )])
        );
        assert_eq!(
            docs.fix_rounds,
            [(
                1,
                vec![(true, "r1-1".to_owned()), (false, "r1-2".to_owned())]
            )]
        );
    }

    #[test]
    fn a_file_too_big_to_show_says_so_and_one_not_there_shows_nothing() {
        let dir = std::env::temp_dir().join(format!("gerdr-page-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = "word ".repeat(md_preview::FILE_MAX as usize / 5 + 1);
        std::fs::write(dir.join("brief.md"), big).unwrap();
        let docs = read_docs(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            docs.previews.get(&Stage::Discover).map(|b| b.as_slice()),
            Some([DocBlock::Note(TOO_BIG.to_owned())].as_slice())
        );
        assert!(!docs.previews.contains_key(&Stage::Plan));
    }

    #[test]
    fn a_file_that_cant_be_read_says_so_on_its_page() {
        let dir = std::env::temp_dir().join(format!("gerdr-page-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("brief.md"), [0xff, 0xfe]).unwrap();
        let docs = read_docs(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let blocks = docs.previews.get(&Stage::Discover).unwrap();
        assert!(matches!(
            blocks.as_slice(),
            [DocBlock::Note(note)] if note.starts_with("Couldn't read this file: ")
        ));
    }

    #[test]
    fn a_read_keeps_the_previews_that_are_the_same() {
        let docs = |text: &str| Docs {
            previews: BTreeMap::from([(Stage::Plan, Arc::new(md_blocks::blocks(text)))]),
            ..Docs::default()
        };
        let old = docs("# Plan");
        let same = docs("# Plan").keeping(Some(&old));
        assert!(Arc::ptr_eq(
            &old.previews[&Stage::Plan],
            &same.previews[&Stage::Plan]
        ));
        let changed = docs("# Other").keeping(Some(&old));
        assert!(!Arc::ptr_eq(
            &old.previews[&Stage::Plan],
            &changed.previews[&Stage::Plan]
        ));
        assert_eq!(docs("# Plan").keeping(None), old);
    }
}
