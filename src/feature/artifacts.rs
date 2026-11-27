//! The files agent stages write into the feature data dir, and the checks
//! that make one good enough to review: `brief.md` (Discover), `plan.md`
//! (Plan) and `build.md` (Build).
//!
//! A brief needs `## Outcome`, `## In scope`, `## Out of scope` and
//! `## Acceptance criteria` sections, none empty, and at least one
//! criterion in Given/When/Then form. A plan needs `## Milestones` with
//! at least one checkbox item, each with a `Seam:` line under it,
//! `## Repositories` and `## Merge order` sections, none empty, and a
//! `Migrations:` line; `named_repos` tells
//! whether it names the touched repositories. Headings match at any
//! level, case-insensitively; fenced code doesn't count.
//!
//! A build log (`build.md`, Build) needs `## Milestones` with at least one
//! checkbox item, and is good once every one is ticked (`- [x] …`; `- [~] …`
//! is one its agent says it is at work on);
//! `milestones` counts them, for the stops between milestones. A
//! checkbox item may be a numbered one too (`1. [ ] …`).

use std::{
    collections::BTreeSet,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::Stage;

/// The file `stage` writes, if any: `brief.md`, `plan.md`.
pub fn name(stage: Stage) -> Option<&'static str> {
    super::agents::needs(stage)?.artifact
}

/// Where `stage`'s file is, under feature data dir `dir`.
pub fn path(dir: &Path, stage: Stage) -> Option<PathBuf> {
    Some(dir.join(name(stage)?))
}

/// The plan a stage's agent works from, under feature data dir `dir`: past
/// Plan review, the final plan if it made one; else the plan as approved.
pub fn plan_for(dir: &Path, stage: Stage) -> Option<PathBuf> {
    let final_plan =
        path(dir, Stage::PlanReview).filter(|p| stage > Stage::PlanReview && p.is_file());
    final_plan.or_else(|| path(dir, Stage::Plan))
}

/// Where `stage`'s file is kept as its agent last wrote it, under
/// feature data dir `dir`: `brief-written.md`. Compared with the file at
/// send back, it tells whether the user edited it in the review.
pub fn written_path(dir: &Path, stage: Stage) -> Option<PathBuf> {
    let name = name(stage)?;
    let stem = name.strip_suffix(".md").unwrap_or(name);
    Some(dir.join(format!("{stem}-written.md")))
}

/// Keep `stage`'s file as its agent wrote it (`written_path`); why it
/// couldn't be.
pub fn keep_written(dir: &Path, stage: Stage) -> Result<(), String> {
    let (Some(from), Some(to)) = (path(dir, stage), written_path(dir, stage)) else {
        return Ok(());
    };
    std::fs::copy(&from, &to)
        .map(drop)
        .map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))
}

/// Whether `stage`'s file changed since its agent wrote it: false when
/// either can't be read.
pub fn edited_since_written(dir: &Path, stage: Stage) -> bool {
    let read = |p: Option<PathBuf>| p.and_then(|p| std::fs::read(p).ok());
    match (read(path(dir, stage)), read(written_path(dir, stage))) {
        (Some(now), Some(written)) => now != written,
        _ => false,
    }
}

/// Where `stage`'s file is kept as it was approved, under feature data
/// dir `dir`: `brief-approved.md`. Compared with the file, it tells whether
/// something changed it since: an agent still open, or a hand.
pub fn approved_path(dir: &Path, stage: Stage) -> Option<PathBuf> {
    let name = name(stage)?;
    let stem = name.strip_suffix(".md").unwrap_or(name);
    Some(dir.join(format!("{stem}-approved.md")))
}

/// Keep `stage`'s file as it is approved (`approved_path`); why it
/// couldn't be.
pub fn keep_approved(dir: &Path, stage: Stage) -> Result<(), String> {
    let (Some(from), Some(to)) = (path(dir, stage), approved_path(dir, stage)) else {
        return Ok(());
    };
    std::fs::copy(&from, &to)
        .map(drop)
        .map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))
}

/// Forget `stage`'s file as it was approved: it isn't approved now. One
/// that won't go is only logged: a copy no longer used changes nothing
/// until the stage is approved again, which writes it anew.
pub fn forget_approved(dir: &Path, stage: Stage) {
    let Some(path) = approved_path(dir, stage) else {
        return;
    };
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != ErrorKind::NotFound => {
            tracing::warn!("removing {}: {err}", path.display());
        }
        _ => {}
    }
}

/// Whether `stage`'s file differs from how it was approved: false with
/// no copy to compare, or a file that can't be read.
pub fn changed_since_approved(dir: &Path, stage: Stage) -> bool {
    let read = |p: Option<PathBuf>| p.and_then(|p| std::fs::read(p).ok());
    match (read(approved_path(dir, stage)), read(path(dir, stage))) {
        (Some(approved), Some(now)) => approved != now,
        _ => false,
    }
}

/// Where the review comments on `stage`'s file are kept, in feature data
/// dir `dir`: `discover-comments.json`.
pub fn comments_path(dir: &Path, stage: Stage) -> PathBuf {
    let stage = stage.label().to_lowercase().replace(' ', "-");
    dir.join(format!("{stage}-comments.json"))
}

/// The stages with a file of their own in feature data dir `dir`, as
/// `Feature::has_output` takes them: what its agent wrote, the code
/// review's comments. Only looks at the folder: cheap enough for a key
/// press, not for a frame.
pub fn outputs(dir: &Path) -> BTreeSet<Stage> {
    let there = |stage: Stage| {
        let code_review = (stage == Stage::CodeReview).then(|| comments_path(dir, stage));
        path(dir, stage)
            .or(code_review)
            .is_some_and(|path| path.is_file())
    };
    Stage::ALL.into_iter().filter(|s| there(*s)).collect()
}

/// The folder a stage started over leaves its files in, under feature
/// data dir `dir`. Apart from the live files, so an agent reading the
/// feature's files doesn't take an old one for the current one.
pub const ARCHIVE: &str = "archive";

/// The first free name for `name` in the archive, numbered from 1:
/// `plan-1.md`, `plan-2.md`.
fn archive_slot(archive: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(path.as_os_str());
    let stem = stem.to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy());
    (1u32..)
        .map(|n| match &ext {
            Some(ext) => archive.join(format!("{stem}-{n}.{ext}")),
            None => archive.join(format!("{stem}-{n}")),
        })
        .find(|slot| !slot.exists())
        .unwrap_or_else(|| archive.join(name))
}

/// The files of `stage` a start over moves away, by name, under feature
/// data dir `dir`: its own, Verify's evidence, its review comments.
fn stage_files(stage: Stage) -> Vec<&'static str> {
    let mut files: Vec<&'static str> = name(stage).into_iter().collect();
    if stage == Stage::Verify {
        files.push(super::verify::EVIDENCE);
    }
    if stage == Stage::PlanReview {
        files.push(super::plan_review::FILE);
    }
    files
}

/// Move what `stage` made into the archive of feature data dir `dir`, and
/// drop the copies kept of it. Returns the live files moved and where to,
/// relative to `dir`, its own file first. A failure says why, and puts back
/// what was moved: the stage is then as it was.
pub fn archive_stage(dir: &Path, stage: Stage) -> Result<Vec<(String, String)>, String> {
    let archive = dir.join(ARCHIVE);
    let comments = comments_path(dir, stage);
    let comments = comments
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let files = (stage_files(stage).into_iter().map(str::to_owned)).chain(comments);
    let there: Vec<String> = files.filter(|name| dir.join(name).is_file()).collect();
    if there.is_empty() {
        drop_kept(dir, stage);
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&archive).map_err(|e| format!("making {}: {e}", archive.display()))?;
    let mut moved: Vec<(String, PathBuf)> = Vec::new();
    for name in there {
        let slot = archive_slot(&archive, &name);
        if let Err(err) = std::fs::rename(dir.join(&name), &slot) {
            for (name, slot) in &moved {
                if let Err(err) = std::fs::rename(slot, dir.join(name)) {
                    tracing::warn!("putting {name} back from {}: {err}", slot.display());
                }
            }
            return Err(format!("moving {name} to {}: {err}", slot.display()));
        }
        moved.push((name, slot));
    }
    drop_kept(dir, stage);
    let relative = |slot: &Path| {
        let slot = slot.strip_prefix(dir).unwrap_or(slot);
        slot.to_string_lossy().into_owned()
    };
    Ok(moved
        .into_iter()
        .map(|(name, slot)| {
            let to = relative(&slot);
            (name, to)
        })
        .collect())
}

/// Move the final plan and its review comments to the archive of feature
/// data dir `dir`, leaving the suggestions: the decisions are still good.
pub fn archive_final_plan(dir: &Path) -> Result<(), String> {
    let stage = Stage::PlanReview;
    let archive = dir.join(ARCHIVE);
    let comments = comments_path(dir, stage);
    for from in path(dir, stage).into_iter().chain([comments]) {
        if !from.is_file() {
            continue;
        }
        let name = from
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        std::fs::create_dir_all(&archive)
            .map_err(|e| format!("making {}: {e}", archive.display()))?;
        let slot = archive_slot(&archive, &name);
        std::fs::rename(&from, &slot)
            .map_err(|e| format!("moving {name} to {}: {e}", slot.display()))?;
    }
    drop_kept(dir, stage);
    Ok(())
}

/// Drop the copies kept of `stage`'s file: they compare with a file now
/// gone. One that won't go is only logged: each is written anew before it
/// is next compared.
fn drop_kept(dir: &Path, stage: Stage) {
    forget_approved(dir, stage);
    let Some(path) = written_path(dir, stage) else {
        return;
    };
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != ErrorKind::NotFound => {
            tracing::warn!("removing {}: {err}", path.display());
        }
        _ => {}
    }
}

/// Whether `text` is a good `stage` file; else what it lacks, one line
/// each ("no Outcome section").
pub fn check(stage: Stage, text: &str) -> Result<(), Vec<String>> {
    let doc = Sections::of(text);
    let mut lacks = Vec::new();
    match stage {
        Stage::Discover => {
            for heading in ["Outcome", "In scope", "Out of scope", "Acceptance criteria"] {
                doc.need(heading, &mut lacks);
            }
            if let Some(body) = doc.get("Acceptance criteria")
                && !body.is_empty()
                && !body.iter().any(|line| given_when_then(line))
            {
                lacks.push("no acceptance criterion as Given/When/Then".into());
            }
        }
        Stage::Plan | Stage::PlanReview => {
            if doc.need("Milestones", &mut lacks)
                && !doc
                    .get("Milestones")
                    .unwrap_or_default()
                    .iter()
                    .any(|l| checkbox(l).is_some())
            {
                lacks.push("no milestone as a checkbox item (“- [ ] …”)".into());
            }
            let body = doc.get("Milestones").unwrap_or_default();
            for milestone in milestones_lacking(&body, "Seam") {
                lacks.push(format!("milestone “{milestone}” has no “Seam:” line"));
            }
            doc.need("Repositories", &mut lacks);
            doc.need("Merge order", &mut lacks);
            if !doc.lines.iter().any(|line| migrations_line(line)) {
                lacks.push("no “Migrations:” line".into());
            }
            if stage == Stage::PlanReview {
                doc.need("Review changes", &mut lacks);
            }
        }
        Stage::Build if doc.need("Milestones", &mut lacks) => match milestones(text) {
            (_, 0) => lacks.push("no milestone as a checkbox item (“- [ ] …”)".into()),
            (ticked, total) if ticked < total => lacks.push(format!(
                "{} of {total} milestones not ticked",
                total - ticked
            )),
            _ => {}
        },
        Stage::AiReview => return super::review_import::parse(text).map(|_| ()),
        _ => {}
    }
    if lacks.is_empty() { Ok(()) } else { Err(lacks) }
}

/// The milestones of Milestones section `body`, by their text, with no
/// `label:` line among the lines under them (up to the next milestone).
fn milestones_lacking(body: &[&str], label: &str) -> Vec<String> {
    let mut lacking = Vec::new();
    let mut current: Option<(String, bool)> = None;
    for line in body {
        if let Some(mark) = checkbox(line) {
            lacking.extend(
                current
                    .take()
                    .filter(|(_, seen)| !seen)
                    .map(|(name, _)| name),
            );
            let rest = list_item(line).unwrap_or(line);
            current = Some((rest[mark.len()..].trim().to_owned(), false));
        } else if let Some((_, seen)) = current.as_mut()
            && labelled(line, label).is_some()
        {
            *seen = true;
        }
    }
    lacking.extend(current.filter(|(_, seen)| !seen).map(|(name, _)| name));
    lacking
}

/// The checkbox items of `text`'s Milestones section: how many are ticked
/// (`[x]` or `[X]`), and how many there are.
pub fn milestones(text: &str) -> (usize, usize) {
    let body = Sections::of(text).get("Milestones").unwrap_or_default();
    let boxes: Vec<&str> = body.iter().filter_map(|l| checkbox(l)).collect();
    let ticked = boxes.iter().filter(|b| ticked(b)).count();
    (ticked, boxes.len())
}

/// What a build log's `Blocked: …` line under its Milestones section says:
/// the Build agent stopped because the plan doesn't hold. As a list item or
/// in bold too; one saying nothing doesn't count.
pub fn blocked(text: &str) -> Option<String> {
    let body = Sections::of(text).get("Milestones")?;
    body.iter().find_map(|line| blocked_why(line))
}

/// What `Blocked: …` line `line` (trimmed) says, if it is one.
fn blocked_why(line: &str) -> Option<String> {
    let line = list_item(line).unwrap_or(line);
    let rest = line.trim_start_matches('*').strip_prefix("Blocked:")?;
    let why = rest.trim_start_matches('*').trim().trim_end_matches('*');
    (!why.trim().is_empty()).then(|| why.trim().to_owned())
}

/// Build log `text` without the `Blocked: …` lines `blocked` reads: work
/// sent since settles them, and its agent writes one again if it still
/// can't go on.
pub fn without_blocked(text: &str) -> String {
    let body = Sections::of(text).get("Milestones").unwrap_or_default();
    let lines: Vec<&str> = body
        .into_iter()
        .filter(|l| blocked_why(l).is_some())
        .collect();
    if lines.is_empty() {
        return text.to_owned();
    }
    let mut kept: String = (text.split_inclusive('\n'))
        .filter(|line| !lines.contains(&line.trim()))
        .collect();
    if !text.ends_with('\n') && kept.ends_with('\n') {
        kept.pop();
    }
    kept
}

/// Each checkbox item of `text`'s Milestones section, in order: whether
/// it is ticked, and its text after the box ("API endpoint").
pub fn milestone_items(text: &str) -> Vec<(bool, String)> {
    items_under(text, "Milestones")
}

/// Each checkbox item of `text`'s section headed `heading`, in order:
/// whether it is ticked, and its text after the box.
pub fn items_under(text: &str, heading: &str) -> Vec<(bool, String)> {
    let body = Sections::of(text).get(heading).unwrap_or_default();
    body.iter()
        .filter_map(|line| {
            let mark = checkbox(line)?;
            let rest = list_item(line)?.strip_prefix(mark)?;
            Some((ticked(mark), rest.trim().to_owned()))
        })
        .collect()
}

/// The lines of section `heading` of `text`, trimmed, blank ones and those
/// in code fences left out: none when it isn't there.
pub fn lines_under(text: &str, heading: &str) -> Vec<String> {
    let body = Sections::of(text).get(heading).unwrap_or_default();
    body.into_iter().map(str::to_owned).collect()
}

/// Where in `milestone_items(text)` the items its agent says it is at work
/// on (`- [~] …`) are.
pub fn milestones_running(text: &str) -> Vec<usize> {
    let body = Sections::of(text).get("Milestones").unwrap_or_default();
    (body.iter().filter_map(|line| checkbox(line)))
        .enumerate()
        .filter(|(_, mark)| *mark == RUNNING)
        .map(|(i, _)| i)
        .collect()
}

/// The repositories of `repos` the plan's Repositories section doesn't
/// name (a word match, case-insensitive).
pub fn unnamed_repos<'a>(text: &str, repos: &[&'a str]) -> Vec<&'a str> {
    let doc = Sections::of(text);
    let body = doc
        .get("Repositories")
        .unwrap_or_default()
        .join("\n")
        .to_lowercase();
    repos
        .iter()
        .copied()
        .filter(|repo| {
            let repo = repo.to_lowercase();
            !body
                .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
                .any(|word| word.trim_end_matches('.') == repo)
        })
        .collect()
}

/// The decision log Discover's agent keeps beside the brief: one
/// `## D<n>: <title>` entry per decision, with a `Decided:` line and the
/// user's own words. The brief's lines cite them as `(D1)`.
pub const DECISIONS: &str = "decisions.md";

/// What the user said they want of a feature as they started it, from the
/// phone: Discover's agent starts from it.
pub const IDEA: &str = "idea.md";

/// The number of decision heading `line` (trimmed): `## D3: Export` is 3.
fn decision_number(line: &str) -> Option<u32> {
    numbered_heading(line, 'd')
}

/// The number of heading `line` (trimmed) that starts with `mark` and a
/// number: `## R2: Does it page?` is 2 for `'r'`.
fn numbered_heading(line: &str, mark: char) -> Option<u32> {
    let text = heading(line)?;
    let rest = text.strip_prefix(mark)?;
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let after = &rest[digits..];
    if digits == 0 || !(after.is_empty() || after.starts_with([':', ' '])) {
        return None;
    }
    rest[..digits].parse().ok()
}

/// Each entry of decision log `text`, in order: its number and its lines
/// after the heading, trimmed, blank ones and fenced code left out.
fn decision_entries(text: &str) -> Vec<(u32, Vec<&str>)> {
    let mut entries: Vec<(u32, Vec<&str>)> = Vec::new();
    for line in Sections::of(text).lines {
        let line = line.trim();
        if let Some(n) = decision_number(line) {
            entries.push((n, Vec::new()));
        } else if heading(line).is_some() {
            // Another heading ends the entry.
            entries.push((0, Vec::new()));
        } else if let Some((_, body)) = entries.last_mut()
            && !line.is_empty()
        {
            body.push(line);
        }
    }
    entries.retain(|(n, _)| *n > 0);
    entries
}

/// The numbers of the decisions in decision log `text`.
pub fn decision_ids(text: &str) -> BTreeSet<u32> {
    decision_entries(text).into_iter().map(|(n, _)| n).collect()
}

/// Whether decision log `text` is good: at least one entry, none twice,
/// each with a `Decided:` line; else what it lacks, one line each.
pub fn check_decisions(text: &str) -> Result<(), Vec<String>> {
    let entries = decision_entries(text);
    let mut lacks = Vec::new();
    if entries.is_empty() {
        lacks.push("no decision as a \u{201c}## D1: \u{2026}\u{201d} heading".into());
    }
    let mut seen = BTreeSet::new();
    for (n, body) in &entries {
        if !seen.insert(*n) {
            lacks.push(format!("D{n} twice"));
        }
        if !body.iter().any(|line| decided_line(line)) {
            lacks.push(format!("D{n} has no \u{201c}Decided:\u{201d} line"));
        }
    }
    if lacks.is_empty() { Ok(()) } else { Err(lacks) }
}

/// Whether `line` (trimmed) is a decision's `Decided:` line: as a list item
/// or in bold too.
fn decided_line(line: &str) -> bool {
    let line = list_item(line)
        .unwrap_or(line)
        .trim_start_matches(['*', '_']);
    let lower = line.to_lowercase();
    lower
        .strip_prefix("decided")
        .is_some_and(|rest| rest.trim_start_matches(['*', '_']).starts_with(':'))
}

/// The decisions and the research answers `text` cites: the `D<n>` and
/// `R<n>` of each parenthesised list made only of such ids (`(D1)`,
/// `(D1, R2)`), fenced code left out. A parenthesis with other words in it
/// (`(D3 in the old code)`) cites none.
pub fn cited(text: &str) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let (mut decisions, mut research) = (BTreeSet::new(), BTreeSet::new());
    for line in Sections::of(text).lines {
        for group in line.split('(').skip(1) {
            let Some((inside, _)) = group.split_once(')') else {
                continue;
            };
            let ids: Option<Vec<(char, u32)>> = (inside.split(','))
                .map(|id| {
                    let id = id.trim();
                    let mark = id.chars().next().filter(|c| matches!(c, 'D' | 'R'))?;
                    Some((mark, id[1..].parse().ok()?))
                })
                .collect();
            for (mark, n) in ids.unwrap_or_default() {
                match mark {
                    'D' => decisions.insert(n),
                    _ => research.insert(n),
                };
            }
        }
    }
    (decisions, research)
}

/// What is wrong with what brief `text` cites, in feature data dir `dir`: a
/// decision log that isn't good, a cited decision it lacks, or a cited
/// research answer not written. A brief citing none, with no log, lacks
/// nothing.
pub fn citation_lacks(text: &str, dir: &Path) -> Vec<String> {
    let (decisions, research) = cited(text);
    let mut lacks = decision_lacks(&decisions, dir);
    for n in research {
        let path = research_path(dir, n);
        let good = std::fs::read_to_string(&path).is_ok_and(|t| check_research(&t).is_ok());
        if !good {
            lacks.push(format!("cites R{n}, which has no answer yet"));
        }
    }
    lacks
}

/// What is wrong with the decision log in feature data dir `dir`, the brief
/// citing `cited`: a log that isn't good, or a cited decision it lacks.
fn decision_lacks(cited: &BTreeSet<u32>, dir: &Path) -> Vec<String> {
    let path = dir.join(DECISIONS);
    let log = match std::fs::read_to_string(&path) {
        Ok(log) => log,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return match cited.first() {
                Some(n) => vec![format!("cites D{n}, but there is no {DECISIONS}")],
                None => Vec::new(),
            };
        }
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            return vec![format!("couldn't read {DECISIONS}: {err}")];
        }
    };
    let mut lacks: Vec<String> = (check_decisions(&log).err().unwrap_or_default().into_iter())
        .map(|lack| format!("{DECISIONS}: {lack}"))
        .collect();
    let ids = decision_ids(&log);
    for n in cited.difference(&ids) {
        lacks.push(format!("cites D{n}, which {DECISIONS} lacks"));
    }
    lacks
}

/// The questions Discover's agent asks to have researched, one
/// `## R<n>: <question>` section each, with an optional `Why:` line.
pub const RESEARCH: &str = "research.md";

/// The file research question `id` is answered in, under feature data dir
/// `dir`: `research-R1.md`.
pub fn research_path(dir: &Path, id: u32) -> PathBuf {
    dir.join(format!("research-R{id}.md"))
}

/// A question to research, as `research.md` asks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u32,
    /// As written, its case kept.
    pub question: String,
    /// Why it matters, if said.
    pub why: Option<String>,
}

/// The questions of `research.md` reading `text`, in order, the first of
/// each number only; fenced code left out.
pub fn research_asks(text: &str) -> Vec<Ask> {
    let mut asks: Vec<Ask> = Vec::new();
    let mut open = false;
    for line in Sections::of(text).lines {
        let line = line.trim();
        if let Some(id) = numbered_heading(line, 'r') {
            open = !asks.iter().any(|a| a.id == id);
            if open {
                // Its words as written, not lowercased as the heading is.
                let words = line.trim_start_matches('#').trim();
                let question = words[1..].trim_start_matches(|c: char| c.is_ascii_digit());
                let question = question.trim_start_matches(':').trim().to_owned();
                asks.push(Ask {
                    id,
                    question,
                    why: None,
                });
            }
        } else if heading(line).is_some() {
            open = false;
        } else if open
            && let Some(ask) = asks.last_mut()
            && ask.why.is_none()
            && let Some(why) = labelled(line, "why")
        {
            ask.why = Some(why);
        }
    }
    asks
}

/// What `line` (trimmed) says after its label `label` and a colon, as a list
/// item or in bold too: `Why: it blocks D3` is `it blocks D3`.
fn labelled(line: &str, label: &str) -> Option<String> {
    let line = list_item(line)
        .unwrap_or(line)
        .trim_start_matches(['*', '_']);
    let head = line.get(..label.len())?;
    if !head.eq_ignore_ascii_case(label) {
        return None;
    }
    let rest = line[label.len()..].trim_start_matches(['*', '_']);
    let rest = rest
        .strip_prefix(':')?
        .trim_start_matches(['*', '_'])
        .trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Whether `text` is a good answer to a research question: an Answer and a
/// Sources section, neither empty; else what it lacks, one line each.
pub fn check_research(text: &str) -> Result<(), Vec<String>> {
    let doc = Sections::of(text);
    let mut lacks = Vec::new();
    doc.need("Answer", &mut lacks);
    doc.need("Sources", &mut lacks);
    if lacks.is_empty() { Ok(()) } else { Err(lacks) }
}

/// A Markdown file cut at its headings; code fences left out.
struct Sections<'a> {
    /// Every line outside fences.
    lines: Vec<&'a str>,
    /// Each heading, normalised, with its body's lines: those of its deeper
    /// headings too, and those headings themselves.
    sections: Vec<(String, Vec<&'a str>)>,
}

impl<'a> Sections<'a> {
    fn of(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut sections: Vec<(String, Vec<&'a str>)> = Vec::new();
        // The sections the line is in, by index, with their heading's level:
        // the outermost first.
        let mut open: Vec<(usize, usize)> = Vec::new();
        let mut fenced = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            lines.push(line);
            let found = heading(trimmed);
            if found.is_some() {
                let level = trimmed.len() - trimmed.trim_start_matches('#').len();
                open.retain(|(_, outer)| *outer < level);
                open.push((sections.len(), level));
            }
            if !trimmed.is_empty() {
                let last = open.len() - usize::from(found.is_some());
                for (at, _) in open.iter().take(last) {
                    sections[*at].1.push(trimmed);
                }
            }
            if let Some(heading) = found {
                sections.push((heading, Vec::new()));
            }
        }
        Sections { lines, sections }
    }

    /// The body of the first section headed `heading`.
    fn get(&self, heading: &str) -> Option<Vec<&'a str>> {
        let want = heading.to_lowercase();
        self.sections
            .iter()
            .find(|(h, _)| *h == want)
            .map(|(_, body)| body.clone())
    }

    /// Note in `lacks` when section `heading` is missing or empty; true
    /// when it is there with a body.
    fn need(&self, heading: &str, lacks: &mut Vec<String>) -> bool {
        match self.get(heading) {
            None => lacks.push(format!("no {heading} section")),
            Some(body) if body.is_empty() => lacks.push(format!("empty {heading} section")),
            Some(_) => return true,
        }
        false
    }
}

/// The heading `line` (trimmed) is, lowercased, its marks and a trailing
/// colon off.
fn heading(line: &str) -> Option<String> {
    let rest = line.trim_start_matches('#');
    if rest.len() == line.len() || !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let text = rest
        .trim()
        .trim_end_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim();
    Some(text.to_lowercase())
}

/// Whether `line` reads Given …, when …, then …, in that order.
pub(crate) fn given_when_then(line: &str) -> bool {
    let words: Vec<String> = line
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let at = |word: &str, from: usize| {
        words[from..]
            .iter()
            .position(|w| w == word)
            .map(|i| i + from)
    };
    at("given", 0)
        .and_then(|g| at("when", g + 1))
        .and_then(|w| at("then", w + 1))
        .is_some()
}

/// What list item `line` (trimmed) says after its marker: `- `, `* `,
/// `+ `, or a number (`1. `, `1) `). `None` when it isn't one.
pub(crate) fn list_item(line: &str) -> Option<&str> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = if digits > 0 {
        line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
    } else {
        line.strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("+ "))
    };
    Some(rest?.trim_start())
}

/// The box of an item its agent says it is at work on.
const RUNNING: &str = "[~]";

/// Whether checkbox `mark` (see `checkbox`) says its item is done.
pub(crate) fn ticked(mark: &str) -> bool {
    matches!(mark, "[x]" | "[X]")
}

/// The box of checkbox list item `line` (trimmed): `[ ]`, `[~]`, `[x]` or
/// `[X]`. A numbered item (`1. [ ] …`, `1) [ ] …`) counts too.
pub(crate) fn checkbox(line: &str) -> Option<&'static str> {
    let rest = list_item(line)?;
    ["[ ]", RUNNING, "[x]", "[X]"]
        .into_iter()
        .find(|b| rest.starts_with(b))
}

/// The lines of the first section of `text` headed `heading` (any level,
/// case-insensitive), trimmed, blank ones and fenced code left out.
pub(crate) fn section<'a>(text: &'a str, heading: &str) -> Option<Vec<&'a str>> {
    Sections::of(text).get(heading)
}

/// Whether `line` is the `Migrations:` line: as a list item or in bold
/// too, or a `Migrations` heading.
fn migrations_line(line: &str) -> bool {
    let line = line.trim().trim_start_matches(['-', '*', '+', '#', ' ']);
    let line = line.trim_start_matches('*').trim_start_matches('_');
    let lower = line.to_lowercase();
    lower.starts_with("migrations:")
        || lower.starts_with("migrations**:")
        || lower.starts_with("migrations__:")
        || lower == "migrations"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_holds_its_deeper_headings_and_ends_at_its_own_level() {
        let text = "# Plan\n\n## Fixes\n\n### One\nCause.\n\n#### Deep\nMore.\n\n### Two\nOther.\n\n## Questions\nWhy?\n";
        assert_eq!(
            section(text, "Fixes").unwrap(),
            [
                "### One",
                "Cause.",
                "#### Deep",
                "More.",
                "### Two",
                "Other."
            ]
        );
        assert_eq!(
            section(text, "One").unwrap(),
            ["Cause.", "#### Deep", "More."]
        );
        assert_eq!(section(text, "Two").unwrap(), ["Other."]);
        assert_eq!(section(text, "Questions").unwrap(), ["Why?"]);
        // A shallower heading ends it too.
        assert_eq!(section("### A\nb\n# C\nd\n", "A").unwrap(), ["b"]);
        // An empty section stays empty with its next one at the same level.
        assert!(section("## A\n\n## B\nc\n", "A").unwrap().is_empty());
    }

    #[test]
    fn checkboxes_under_a_subheading_count_as_milestones() {
        let text = "## Milestones\n\n### First\n- [x] One\n\n### Second\n- [ ] Two\n\n## Notes\n- [ ] Not one\n";
        assert_eq!(milestones(text), (1, 2));
    }

    /// A folder of its own, as `name`.
    fn folder(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("gerdr-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    #[test]
    fn an_approved_file_changed_later_is_told() {
        let dir = folder("approved");
        let plan = dir.path().join("plan.md");
        std::fs::write(&plan, "plan").unwrap();
        // Nothing kept: nothing to tell.
        assert!(!changed_since_approved(dir.path(), Stage::Plan));
        keep_approved(dir.path(), Stage::Plan).unwrap();
        assert!(!changed_since_approved(dir.path(), Stage::Plan));
        std::fs::write(&plan, "plan, changed").unwrap();
        assert!(changed_since_approved(dir.path(), Stage::Plan));
        forget_approved(dir.path(), Stage::Plan);
        assert!(!changed_since_approved(dir.path(), Stage::Plan));
        // Forgetting what isn't there is fine.
        forget_approved(dir.path(), Stage::Plan);
    }

    #[test]
    fn outputs_are_the_files_stages_made() {
        let dir = folder("outputs");
        assert!(outputs(dir.path()).is_empty());
        std::fs::write(dir.path().join("plan.md"), "plan").unwrap();
        std::fs::write(comments_path(dir.path(), Stage::CodeReview), "[]").unwrap();
        // Comments on the brief are the brief's review's, not its output.
        std::fs::write(comments_path(dir.path(), Stage::Discover), "[]").unwrap();
        assert_eq!(
            outputs(dir.path()).into_iter().collect::<Vec<_>>(),
            [Stage::Plan, Stage::CodeReview]
        );
    }

    #[test]
    fn starting_over_moves_a_stages_files_to_the_archive() {
        let dir = folder("archive");
        let d = dir.path();
        // Nothing there: nothing moved, no folder made.
        assert_eq!(archive_stage(d, Stage::Plan), Ok(Vec::new()));
        assert!(!d.join(ARCHIVE).exists());
        let write = || {
            std::fs::write(d.join("plan.md"), "plan").unwrap();
            std::fs::write(comments_path(d, Stage::Plan), "[]").unwrap();
        };
        write();
        keep_approved(d, Stage::Plan).unwrap();
        let moved = archive_stage(d, Stage::Plan).unwrap();
        let there = |to: &str| Path::new(ARCHIVE).join(to).display().to_string();
        assert_eq!(
            moved,
            [
                ("plan.md".to_owned(), there("plan-1.md")),
                (
                    "plan-comments.json".to_owned(),
                    there("plan-comments-1.json")
                ),
            ]
        );
        assert!(!d.join("plan.md").exists());
        assert!(!comments_path(d, Stage::Plan).exists());
        assert!(!approved_path(d, Stage::Plan).unwrap().exists());
        assert_eq!(
            std::fs::read_to_string(d.join(there("plan-1.md"))).unwrap(),
            "plan"
        );
        // A second time doesn't overwrite the first, and the live files of
        // other stages stay.
        std::fs::write(d.join("brief.md"), "brief").unwrap();
        write();
        let moved = archive_stage(d, Stage::Plan).unwrap();
        assert_eq!(moved[0].1, there("plan-2.md"));
        assert!(d.join(there("plan-1.md")).exists());
        assert!(d.join("brief.md").exists());
    }

    #[test]
    fn a_file_edited_since_its_agent_wrote_it_is_told() {
        let dir = std::env::temp_dir().join(format!("gerdr-written-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = Dir(dir);
        let brief = dir.path().join("brief.md");
        assert_eq!(
            written_path(dir.path(), Stage::Discover),
            Some(dir.path().join("brief-written.md"))
        );
        // Nothing kept yet: not edited.
        std::fs::write(&brief, "# Brief\n").unwrap();
        assert!(!edited_since_written(dir.path(), Stage::Discover));
        keep_written(dir.path(), Stage::Discover).unwrap();
        assert!(!edited_since_written(dir.path(), Stage::Discover));
        std::fs::write(&brief, "# Brief, edited\n").unwrap();
        assert!(edited_since_written(dir.path(), Stage::Discover));
    }

    /// A scratch dir, deleted when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const BRIEF: &str = "# Bulk edit\n\n## Outcome\nEdit many rows at once.\n\n## In scope\n- price\n\n## Out of Scope:\n- stock\n\n## Acceptance criteria\n- Given two rows, when I edit both, then both change.\n";

    #[test]
    fn a_blocked_line_in_the_milestones_says_why() {
        let log = |line: &str| {
            format!("## Milestones\n- [x] API\n{line}\n- [ ] UI\n\n## Notes\nBlocked: not here\n")
        };
        assert_eq!(
            blocked(&log("Blocked: no such table")).as_deref(),
            Some("no such table")
        );
        assert_eq!(
            blocked(&log("- Blocked: no such table")).as_deref(),
            Some("no such table")
        );
        assert_eq!(
            blocked(&log("**Blocked:** no such table")).as_deref(),
            Some("no such table")
        );
        assert_eq!(blocked(&log("  Endpoint added.")), None);
        assert_eq!(blocked(&log("Blocked:")), None);
        assert_eq!(blocked("## Notes\nBlocked: elsewhere\n"), None);
        assert_eq!(
            blocked("## Milestones\n```\nBlocked: a sample\n```\n- [ ] a\n"),
            None
        );
    }

    #[test]
    fn work_sent_drops_the_blocked_lines_of_the_milestones() {
        let log = "## Milestones\n- [x] API\nBlocked: no such table\n- [ ] UI\n  - Blocked: twice\n\n## Notes\nBlocked: not here\n";
        let kept = without_blocked(log);
        assert_eq!(
            kept,
            "## Milestones\n- [x] API\n- [ ] UI\n\n## Notes\nBlocked: not here\n"
        );
        assert_eq!(blocked(&kept), None);
        assert_eq!(
            without_blocked("## Milestones\n- [ ] a"),
            "## Milestones\n- [ ] a"
        );
        assert_eq!(
            without_blocked("## Milestones\n- [ ] a\nBlocked: b"),
            "## Milestones\n- [ ] a"
        );
    }

    const PLAN: &str = "## Milestones\n- [ ] API\n  Seam: `POST /rows`\n- [x] UI\n  - **Seam:** none: styles only\n\n## Repositories\n- api: endpoint\n- web: form\n\n## Merge order\napi then web\n\n**Migrations:** none\n";

    #[test]
    fn names_follow_the_stages() {
        assert_eq!(name(Stage::Discover), Some("brief.md"));
        assert_eq!(name(Stage::Plan), Some("plan.md"));
        assert_eq!(name(Stage::Build), Some("build.md"));
        assert_eq!(name(Stage::Verify), None);
        assert_eq!(name(Stage::AiReview), Some("review.json"));
        assert_eq!(
            path(Path::new("/d"), Stage::Plan),
            Some(PathBuf::from("/d/plan.md"))
        );
    }

    #[test]
    fn a_review_passes_when_it_parses() {
        assert_eq!(check(Stage::AiReview, r#"{"findings": []}"#), Ok(()));
        let lacks = check(Stage::AiReview, r#"{"findings": [{"file": "a"}]}"#).unwrap_err();
        assert_eq!(lacks, ["finding 1 has no title"]);
    }

    #[test]
    fn a_good_brief_passes() {
        assert_eq!(check(Stage::Discover, BRIEF), Ok(()));
    }

    #[test]
    fn a_brief_lists_what_it_lacks() {
        assert_eq!(
            check(Stage::Discover, "## Outcome\n\n## In scope\n- a\n"),
            Err(vec![
                "empty Outcome section".to_owned(),
                "no Out of scope section".to_owned(),
                "no Acceptance criteria section".to_owned(),
            ])
        );
        let vague = BRIEF.replace("Given two rows, when I edit both, then", "It works and");
        assert_eq!(
            check(Stage::Discover, &vague),
            Err(vec![
                "no acceptance criterion as Given/When/Then".to_owned()
            ])
        );
        // Headings inside code don't count.
        let fenced = format!("```\n{BRIEF}```\n");
        assert_eq!(check(Stage::Discover, &fenced).unwrap_err().len(), 4);
        // Nor does a hashtag.
        assert!(check(Stage::Discover, "#Outcome\nx").is_err());
    }

    #[test]
    fn each_milestone_of_a_plan_needs_a_seam_line() {
        let plan = PLAN.replace("  Seam: `POST /rows`\n", "  Check: rows.test.ts\n");
        assert_eq!(
            check(Stage::Plan, &plan),
            Err(vec!["milestone “API” has no “Seam:” line".to_owned()])
        );
        // An empty one doesn't count; the last milestone is checked too.
        let plan = PLAN.replace("none: styles only", "");
        assert_eq!(
            check(
                Stage::PlanReview,
                &format!("{plan}## Review changes\n- none\n")
            ),
            Err(vec!["milestone “UI” has no “Seam:” line".to_owned()])
        );
    }

    #[test]
    fn a_plan_needs_milestones_repositories_merge_order_and_migrations() {
        assert_eq!(check(Stage::Plan, PLAN), Ok(()));
        assert_eq!(
            check(
                Stage::Plan,
                &PLAN.replace("**Migrations:**", "- Migrations:")
            ),
            Ok(())
        );
        assert_eq!(
            check(Stage::Plan, "## Milestones\n1. API\n## Repositories\n"),
            Err(vec![
                "no milestone as a checkbox item (“- [ ] …”)".to_owned(),
                "empty Repositories section".to_owned(),
                "no Merge order section".to_owned(),
                "no “Migrations:” line".to_owned(),
            ])
        );
        // A stage without a file has nothing to check.
        assert_eq!(check(Stage::Verify, ""), Ok(()));
    }

    #[test]
    fn a_build_log_is_good_once_every_milestone_is_ticked() {
        let log = "# Build\n\n## Milestones\n- [x] API\n  Endpoint added.\n* [X] UI\n1. [ ] Docs\n2) [ ] Tests\n\n## Notes\n- [ ] not a milestone\n";
        assert_eq!(milestones(log), (2, 4));
        assert_eq!(
            check(Stage::Build, log),
            Err(vec!["2 of 4 milestones not ticked".to_owned()])
        );
        let done = log
            .replace("[ ] Docs", "[x] Docs")
            .replace("[ ] Tests", "[x] Tests");
        assert_eq!(milestones(&done), (4, 4));
        assert_eq!(check(Stage::Build, &done), Ok(()));
        // Boxes in code don't count.
        let fenced = "## Milestones\n```\n- [ ] a\n```\n- [x] b\n";
        assert_eq!(milestones(fenced), (1, 1));
    }

    #[test]
    fn a_milestone_at_work_is_there_but_not_ticked() {
        let log = "## Milestones\n- [x] API\n- [~] UI\n- [ ] Docs\n1. [~] Tests\n";
        assert_eq!(milestones(log), (1, 4));
        assert_eq!(milestones_running(log), [1, 3]);
        assert_eq!(milestone_items(log)[1], (false, "UI".to_owned()));
        assert!(check(Stage::Build, log).is_err());
        assert!(milestones_running("## Milestones\n- [ ] a\n- [x] b\n").is_empty());
        assert!(milestones_running("## Notes\n- [~] a\n").is_empty());
    }

    #[test]
    fn milestone_items_name_each_box_and_whether_it_is_ticked() {
        let log = "# Build\n\n## Milestones\n- [x] API\n  Endpoint added.\n* [X] UI\n1. [ ] Docs\n2) [ ]Tests\n```\n- [x] fenced\n```\n\n## Notes\n- [ ] not a milestone\n";
        assert_eq!(
            milestone_items(log),
            vec![
                (true, "API".to_owned()),
                (true, "UI".to_owned()),
                (false, "Docs".to_owned()),
                (false, "Tests".to_owned()),
            ]
        );
        assert_eq!(milestone_items(log).len(), milestones(log).1);
        assert!(milestone_items("").is_empty());
        assert!(milestone_items("## Milestones\n- API\n").is_empty());
    }

    #[test]
    fn a_build_log_needs_milestones_as_checkboxes() {
        assert_eq!(milestones(""), (0, 0));
        assert_eq!(
            check(Stage::Build, ""),
            Err(vec!["no Milestones section".to_owned()])
        );
        assert_eq!(
            check(Stage::Build, "## Milestones\n- API\n1. UI\n"),
            Err(vec![
                "no milestone as a checkbox item (“- [ ] …”)".to_owned()
            ])
        );
        assert_eq!(milestones("## Milestones\n- API\n- [x]API\n"), (1, 1));
    }

    #[test]
    fn the_plan_names_the_touched_repositories() {
        assert!(unnamed_repos(PLAN, &["api", "web"]).is_empty());
        assert_eq!(
            unnamed_repos(PLAN, &["api", "worker", "Web"]),
            vec!["worker"]
        );
        assert_eq!(unnamed_repos("x", &["api"]), vec!["api"]);
    }

    const LOG: &str = "# Decisions\n\n## D1: Export format\nDecided: one CSV per export.\nUser said: \"CSV only\"\n\n## D2 Where it saves\n- **Decided:** the Downloads folder.\n\n```\n## D9: in a fence\n```\n";

    #[test]
    fn a_decision_log_lists_its_entries_by_number() {
        assert_eq!(decision_ids(LOG), BTreeSet::from([1, 2]));
        assert_eq!(check_decisions(LOG), Ok(()));
        assert_eq!(
            decision_ids("## Dx: no\n## Done\n## D: no\n"),
            BTreeSet::new()
        );
    }

    #[test]
    fn each_decision_needs_a_decided_line_and_a_number_of_its_own() {
        let bad =
            "## D1: A\nWhy: none.\n## Other\nDecided: not D1's.\n## D1: B\n**Decided**: yes\n";
        assert_eq!(
            check_decisions(bad),
            Err(vec![
                "D1 has no \u{201c}Decided:\u{201d} line".to_owned(),
                "D1 twice".to_owned()
            ])
        );
        assert!(check_decisions("# Decisions\n").is_err());
    }

    #[test]
    fn a_brief_cites_decisions_only_in_parentheses_of_ids() {
        let brief =
            "- Exports CSV (D1)\n- Saves (D2, D4) and more (D3 in the old code)\n```\n(D7)\n```\n";
        assert_eq!(cited(brief).0, BTreeSet::from([1, 2, 4]));
        assert_eq!(
            cited("- Pages (D1, R2)\n- (R3, maybe)\n"),
            (BTreeSet::from([1]), BTreeSet::from([2]))
        );
        assert_eq!(
            cited("Nothing (here) (d1)"),
            (BTreeSet::new(), BTreeSet::new())
        );
    }

    #[test]
    fn a_brief_citing_a_missing_decision_lacks_it() {
        let dir = folder("decisions");
        assert!(citation_lacks("- plain\n", dir.path()).is_empty());
        assert_eq!(
            citation_lacks("- CSV (D1)\n", dir.path()),
            vec![format!("cites D1, but there is no {DECISIONS}")]
        );
        std::fs::write(dir.path().join(DECISIONS), LOG).unwrap();
        assert!(citation_lacks("- CSV (D1, D2)\n", dir.path()).is_empty());
        assert_eq!(
            citation_lacks("- CSV (D3)\n", dir.path()),
            vec![format!("cites D3, which {DECISIONS} lacks")]
        );
        std::fs::write(dir.path().join(DECISIONS), "## D1: A\n").unwrap();
        assert_eq!(
            citation_lacks("- plain\n", dir.path()),
            vec![format!(
                "{DECISIONS}: D1 has no \u{201c}Decided:\u{201d} line"
            )]
        );
        std::fs::remove_file(dir.path().join(DECISIONS)).unwrap();
        assert_eq!(
            citation_lacks("- Pages (R2)\n", dir.path()),
            vec!["cites R2, which has no answer yet".to_owned()]
        );
        let answer = "## Answer\nYes\n## Sources\n- x\n";
        std::fs::write(research_path(dir.path(), 2), answer).unwrap();
        assert!(citation_lacks("- Pages (R2)\n", dir.path()).is_empty());
    }

    #[test]
    fn research_asks_are_read_once_each_with_their_why() {
        let text = "# Research\n\n## R1: Does the Stripe API page its results?\nWhy: decides D3.\n\n## R2 Which CSV dialect?\n- **Why:** the accountant's tool\n\n## Notes\nWhy: not R2's\n\n## R1: Asked again\n```\n## R9: fenced\n```\n";
        assert_eq!(
            research_asks(text),
            vec![
                Ask {
                    id: 1,
                    question: "Does the Stripe API page its results?".into(),
                    why: Some("decides D3.".into()),
                },
                Ask {
                    id: 2,
                    question: "Which CSV dialect?".into(),
                    why: Some("the accountant's tool".into()),
                },
            ]
        );
        assert!(research_asks("## Results\n## R: none\n").is_empty());
    }

    #[test]
    fn an_answer_needs_its_answer_and_its_sources() {
        assert_eq!(
            check_research("## Answer\nYes.\n## Sources\n- https://x\n"),
            Ok(())
        );
        assert_eq!(
            check_research("## Answer\nYes.\n## Sources\n"),
            Err(vec!["empty Sources section".to_owned()])
        );
    }
}
