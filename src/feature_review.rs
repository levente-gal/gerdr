//! The AI review stage of features, run by gerdr (inc 5, agent A): the
//! reviewer agent, its `review.json` read in as findings (see
//! `feature::review_import`), the user's Fix and Dismiss, and the fixes
//! sent to the Build agent.
//!
//! Verify passing starts the reviewer, an agent other than Build's (see
//! `feature_agents::pick_reviewer`). Once it is done, its findings are read
//! in against the code Verify passed: code that changed since sends the
//! feature back. A review with no finding that holds up marks AI review
//! done; else its gate waits for the user to decide each one, then to send
//! those to fix to the Build agent. Fixed code goes through Build and
//! Verify again, and a new review checks the fixes.

use std::{path::PathBuf, time::Instant};

use crate::{
    Gerdr,
    feature::{
        self, Feature, Stage, Status, artifacts,
        review_import::{self, AiReview, CodeRead, Decision, Finding, FindingState},
        store,
    },
    feature_agents::{Stamp, vars},
    features::{Done, plural},
    prompts,
};

/// The values the AI review prompt gets of `feature` besides the stage's
/// own: per touched repository the diff to review (all of the change the
/// first time, since the last review after), and the earlier findings,
/// sent, dismissed and still open.
pub(crate) fn review_vars(feature: &Feature) -> Vec<(String, String)> {
    let review = feature.review.as_ref();
    let diffs = feature
        .touched()
        .map(|repo| {
            let whole = format!("git diff {}...HEAD", repo.base_commit);
            let at = repo.worktree.display();
            let key = repo.key();
            match review.and_then(|r| r.reviewed.get(&key)) {
                Some(sha) => format!(
                    "   - {key}, in {at}: `git diff {sha}..HEAD` (what changed since the last review; `{whole}` shows the whole change)"
                ),
                None => format!("   - {key}, in {at}: `{whole}`"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let list = |states: &[FindingState]| {
        review
            .map(|r| {
                r.findings
                    .iter()
                    .filter(|f| states.contains(&f.state))
                    .map(|f| finding_line(f, feature.many_repos()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    vec![
        ("review.diffs".into(), diffs),
        ("review.sent".into(), list(&[FindingState::Sent])),
        ("review.dismissed".into(), list(&[FindingState::Dismissed])),
        (
            "review.open".into(),
            list(&[FindingState::Open, FindingState::Fix]),
        ),
    ]
}

/// A finding as the reviewer's prompt lists it: "- r1-2 (high,
/// src/a.rs:12): Off by one", with the reason a dismissed one has.
fn finding_line(f: &Finding, multi: bool) -> String {
    let mut line = format!(
        "- {} ({}, {}): {}",
        f.id,
        f.severity.label(),
        review_import::location(f, multi),
        f.title
    );
    if f.state == FindingState::Dismissed && !f.reason.is_empty() {
        line.push_str(&format!(". Dismissed because: {}", f.reason));
    }
    line
}

/// `what` as a file name has it: its words, "review-comments" of "review
/// comments", "findings" of "2 findings".
fn file_key(what: &str) -> String {
    let words: Vec<String> = what
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "fixes".into()
    } else {
        words.join("-")
    }
}

/// What AI review of `title` waits for in `review`, for its toast and
/// notification; `None` once nothing does.
fn waiting_text(title: &str, review: &AiReview) -> Option<String> {
    let open = review.count(FindingState::Open);
    let fix = review.count(FindingState::Fix);
    let what = if open > 0 {
        format!("{} to decide", plural(open, "finding"))
    } else if fix > 0 {
        format!("{} to send to Build", plural(fix, "finding"))
    } else {
        return None;
    };
    Some(format!("AI review of \u{201c}{title}\u{201d}: {what}"))
}

impl Gerdr {
    /// Read the code of feature `slug` and its `review.json` off the
    /// thread for run `n` of its AI review agent, whose file has `stamp`
    /// (see `review_read`).
    pub(crate) fn check_review_code(&mut self, project: &str, slug: &str, n: u32, stamp: Stamp) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let data = store::dir(None, project, slug);
        let Some(path) = artifacts::path(&data, Stage::AiReview) else {
            return;
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::ReviewRead {
            result: review_import::read_code(&feature, &path).map(Box::new),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// The review of run `n` (file at `stamp`) can't be read in, as `why`
    /// says for the panel; `text` is told once until it changes (`error`:
    /// as an error, else a warning).
    fn refuse_review(
        &mut self,
        project: &str,
        slug: &str,
        stamp: Stamp,
        why: String,
        text: String,
        error: bool,
    ) {
        let key = (project.to_owned(), slug.to_owned(), Stage::AiReview);
        let Some(watch) = self.features.agents.get_mut(&key) else {
            return;
        };
        let again = watch.refused.as_ref() == Some(&(stamp, why.clone()));
        watch.refused = Some((stamp, why.clone()));
        if again {
            return;
        }
        tracing::warn!("feature {project}/{slug}: AI review not read: {why}");
        if let Some(feature) = self.feature_mut(project, slug) {
            feature.log(feature::now(), format!("AI review not read: {why}"));
            self.save_feature(project, slug);
        }
        if error {
            self.show_error(text);
        } else {
            self.show_warning(text);
        }
    }

    /// The code and `review.json` of feature `slug` were read for run `n`
    /// of its AI review agent, whose file had `stamp`. The agent must have
    /// left the code as Verify passed it; then its findings are read in,
    /// and AI review is marked done when none holds up, else its gate
    /// waits.
    pub(crate) fn review_read(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Box<CodeRead>, String>,
    ) {
        let key = (project.to_owned(), slug.to_owned(), Stage::AiReview);
        let Some(watch) = self.features.agents.get_mut(&key).filter(|w| w.n == n) else {
            return;
        };
        watch.checking = None;
        watch.refused_at = Some(Instant::now());
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let read = match result {
            Ok(read) => read,
            Err(why) => {
                let text = format!("Couldn't read the code of \u{201c}{title}\u{201d}: {why}");
                let why = format!("couldn't read the code: {why}");
                return self.refuse_review(project, slug, stamp, why, text, true);
            }
        };
        let running = feature.status == Status::Open
            && feature.current() == Some(Stage::AiReview)
            && feature.runs.get(&Stage::AiReview).is_some_and(|r| r.n == n);
        if !running {
            return;
        }
        let mut changed: Vec<String> = read.reading.dirty.clone();
        changed.extend(read.reading.stray.iter().cloned());
        changed.sort();
        changed.dedup();
        if !changed.is_empty() {
            let repos = changed.join(", ");
            let text = format!(
                "The AI review agent changed the code in {repos}; undo its changes for its findings to be read, or start a fresh agent"
            );
            let why = format!("the agent changed the code in {repos}");
            return self.refuse_review(project, slug, stamp, why, text, false);
        }
        let patch = read.reading.patch.clone();
        let verified = feature
            .marks
            .get(&Stage::Verify)
            .and_then(|m| m.patch.as_deref());
        if verified != Some(patch.as_str()) {
            // Reviewed on other code than Verify passed: that code goes
            // through Build and Verify, and is reviewed again.
            if let Some(f) = self.feature_mut(project, slug) {
                f.log(
                    feature::now(),
                    "AI review not read: the code changed since Verify",
                );
                self.save_feature(project, slug);
            }
            self.features.agents.remove(&key);
            self.feature_code_is(project, slug, &patch);
            let still = self
                .feature(project, slug)
                .is_some_and(|f| f.current() == Some(Stage::AiReview));
            if still {
                self.show_warning(format!(
                    "The code of \u{201c}{title}\u{201d} changed since Verify; mark Verify again"
                ));
            }
            return;
        }
        let raws = match read
            .text
            .clone()
            .and_then(|text| review_import::parse(&text).map_err(|lacks| lacks.join("; ")))
        {
            Ok(raws) => raws,
            Err(why) => {
                let text = format!("Couldn't read AI review's findings: {why}");
                let why = format!("couldn't read {}: {why}", review_import::REVIEW);
                return self.refuse_review(project, slug, stamp, why, text, true);
            }
        };
        if let Some(watch) = self.features.agents.get_mut(&key) {
            watch.refused = None;
        }
        let repos: Vec<(String, PathBuf)> = feature
            .touched()
            .map(|r| (r.key(), r.worktree.clone()))
            .collect();
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let review = f.review.get_or_insert_with(AiReview::default);
        let got = review_import::import(review, n, raws, &repos, &read.diff);
        review.reviewed = read.heads.clone();
        review.patch = Some(patch.clone());
        let waiting = waiting_text(&title, review);
        let pane = f.runs.get_mut(&Stage::AiReview).and_then(|r| {
            r.ready = Some(now);
            r.pane.clone()
        });
        f.log(
            now,
            format!(
                "AI review run {n} read: {} new, {} raised again, {} fixed",
                got.new, got.again, got.fixed
            ),
        );
        let Some(summary) = waiting else {
            f.tick(now, "gerdr", Some(&patch));
            self.save_feature(project, slug);
            self.features.agents.remove(&key);
            self.feature_cursor_to_current_of(project, slug);
            let summary = if got.fixed > 0 {
                format!("AI review of \u{201c}{title}\u{201d}: every finding fixed")
            } else {
                format!("AI review of \u{201c}{title}\u{201d} found nothing to fix")
            };
            self.notify_feature(project, slug, pane, &summary);
            return self.show_success(summary);
        };
        self.save_feature(project, slug);
        self.notify_feature(project, slug, pane, &summary);
        self.show_warning(summary);
    }

    /// Decide finding `id` of feature `slug`'s AI review: Fix, Dismiss
    /// with a reason, or back to undecided. Saves the record; tells the
    /// user why when it can't (the feature isn't at AI review, no such
    /// finding, an empty reason). Once no finding holds up, AI review is
    /// marked done on the code as it is.
    pub fn decide_finding(&mut self, project: &str, slug: &str, id: &str, decision: Decision) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let title = f.title.clone();
        if f.status != Status::Open || f.current() != Some(Stage::AiReview) {
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
        }
        let Some(review) = f.review.as_mut() else {
            return self.show_warning(format!("There's no finding {id}"));
        };
        let what = match review_import::decide(review, id, &decision) {
            Ok(what) => what,
            Err(why) => return self.show_warning(why),
        };
        let open = review.open();
        let fix = review.count(FindingState::Fix);
        let undecided = review.count(FindingState::Open);
        f.log(feature::now(), format!("Finding {id}: {what}"));
        self.save_feature(project, slug);
        let word = match decision {
            Decision::Fix => "marked to fix",
            Decision::Dismiss(_) => "dismissed",
            Decision::Reopen => "reopened",
        };
        if open == 0 {
            self.show_success(format!("Finding {id} {word}"));
            return self.read_and_tick(project, slug, Stage::AiReview);
        }
        if undecided == 0 && fix > 0 {
            return self.show_success(format!(
                "Finding {id} {word}; {} to send to Build",
                plural(fix, "finding")
            ));
        }
        self.show_success(format!("Finding {id} {word}"));
    }

    /// Send feature `slug`'s AI review findings the user wants fixed to
    /// its Build agent, once none waits to be decided; they are sent then,
    /// and the next review checks them.
    pub fn send_findings(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let title = f.title.clone();
        if f.status != Status::Open || f.current() != Some(Stage::AiReview) {
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
        }
        let Some(review) = f.review.as_ref() else {
            return self.show_warning(format!("\u{201c}{title}\u{201d} has no findings to fix"));
        };
        let undecided = review.count(FindingState::Open);
        if undecided > 0 {
            return self.show_warning(format!(
                "Decide the {} of \u{201c}{title}\u{201d} first",
                plural(undecided, "finding")
            ));
        }
        let (n, text) = review_import::fixes_markdown(review, &title, f.many_repos());
        if n == 0 {
            return self.show_warning(format!("\u{201c}{title}\u{201d} has no findings to fix"));
        }
        let what = plural(n, "finding");
        if !self.send_to_build(project, slug, &what, text) {
            return;
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if let Some(review) = f.review.as_mut() {
            for finding in &mut review.findings {
                if finding.state == FindingState::Fix {
                    finding.state = FindingState::Sent;
                }
            }
        }
        self.save_feature(project, slug);
    }

    /// Send `text` (Markdown) to feature `slug`'s Build agent to act on:
    /// into its tab while it lives, else to a fresh Build agent with it.
    /// `what` names it for the log and the toast: "review comments",
    /// "2 findings". Whether it went.
    ///
    /// It goes to `prompts/build-<n>-<what>-<k>.md`, and the tab is told
    /// where in one line. Build stays done: the fixes' commits send the
    /// feature back to it, and gerdr reads the code once the agent rests.
    pub(crate) fn send_to_build(
        &mut self,
        project: &str,
        slug: &str,
        what: &str,
        text: String,
    ) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let title = &feature.title;
        if feature.status != Status::Open || !feature.is_done(Stage::Build) {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} isn't built yet; nothing to send {what} to"
            ));
            return false;
        }
        let data = store::dir(None, project, slug);
        let dir = data.join("prompts");
        let n = feature.runs.get(&Stage::Build).map_or(0, |r| r.n);
        let key = file_key(what);
        let path = (1..)
            .map(|k| dir.join(format!("build-{n}-{key}-{k}.md")))
            .find(|p| !p.exists())
            .unwrap_or_default();
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &text));
        if let Err(err) = written {
            tracing::warn!("writing {}: {err}", path.display());
            self.show_error(format!("Couldn't write {}: {err}", path.display()));
            return false;
        }
        let shown = path.display().to_string();
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Build);
        values.insert("fixes.what".into(), what.to_owned());
        values.insert("fixes.path".into(), shown.clone());
        let sent = prompts::render_builtin("send-fixes", &values)
            .map_err(|e| e.to_string())
            .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        let fresh = sent.is_err();
        if let Err(why) = &sent {
            tracing::info!("sending {what} of {project}/{slug} to its Build agent: {why}");
            let build_at = feature.marks.get(&Stage::Build).map(|m| m.at);
            let extra = [
                ("fixes", text),
                ("fixes.path", shown),
                ("fixes.what", what.to_owned()),
            ];
            if !self.launch_stage_agent(project, slug, Stage::Build, &extra) {
                return false;
            }
            if let Some(r) = self
                .feature_mut(project, slug)
                .and_then(|f| f.runs.get_mut(&Stage::Build))
            {
                r.ready = build_at;
            }
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return false;
        };
        let agent = (f.runs.get(&Stage::Build)).and_then(|r| r.agent_key());
        let to = if fresh {
            "a fresh Build agent"
        } else {
            "the Build agent"
        };
        f.log(feature::now(), format!("Sent {what} to {to}"));
        self.save_feature(project, slug);
        if let Some(agent) = agent {
            self.watch_fixing(project, slug, agent);
        }
        self.show_success(format!("Sent {what} of \u{201c}{title}\u{201d} to {to}"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        feature::review_import::{Raw, Severity},
        features::tests::{feature, repo},
    };

    fn reviewed() -> Feature {
        let mut f = feature("a", vec![repo("api", true), repo("web", true)], 4);
        let mut review = AiReview::default();
        let raw = |title: &str| Raw {
            file: "src/a.rs".into(),
            repo: Some("api".into()),
            line: Some(3),
            title: title.into(),
            severity: Severity::High,
            ..Raw::default()
        };
        review_import::import(
            &mut review,
            1,
            vec![raw("sent"), raw("dismissed"), raw("open")],
            &[],
            &Default::default(),
        );
        review.findings[0].state = FindingState::Sent;
        review.findings[1].state = FindingState::Dismissed;
        review.findings[1].reason = "by design".into();
        review.reviewed.insert("api".into(), "f00d".into());
        f.review = Some(review);
        f
    }

    #[test]
    fn the_first_review_reads_the_whole_change() {
        let f = feature("a", vec![repo("api", true), repo("web", false)], 4);
        let v: prompts::render::Vars = review_vars(&f).into_iter().collect();
        assert_eq!(
            v["review.diffs"],
            "   - api, in /work/.features/bulk/api: `git diff abc123...HEAD`"
        );
        assert_eq!(v["review.sent"], "");
        assert_eq!(v["review.open"], "");
    }

    #[test]
    fn a_later_review_reads_what_changed_and_the_earlier_findings() {
        let v: prompts::render::Vars = review_vars(&reviewed()).into_iter().collect();
        assert!(
            v["review.diffs"].contains("api, in /work/.features/bulk/api: `git diff f00d..HEAD`")
        );
        assert!(v["review.diffs"].contains("`git diff abc123...HEAD` shows the whole change"));
        assert!(
            v["review.diffs"]
                .contains("web, in /work/.features/bulk/web: `git diff abc123...HEAD`")
        );
        assert_eq!(v["review.sent"], "- r1-1 (high, api/src/a.rs:3): sent");
        assert_eq!(
            v["review.dismissed"],
            "- r1-2 (high, api/src/a.rs:3): dismissed. Dismissed because: by design"
        );
        assert_eq!(v["review.open"], "- r1-3 (high, api/src/a.rs:3): open");
        let mut all = crate::feature_agents::vars(
            &reviewed(),
            &crate::feature::PerProject::all("Acme".to_owned()),
            std::path::Path::new("/d"),
            Stage::AiReview,
        );
        all.extend(review_vars(&reviewed()));
        let text = prompts::render_builtin("ai-review", &all).unwrap();
        assert!(text.contains("/d/review.json") && text.contains("by design"));
    }

    #[test]
    fn what_is_sent_names_its_file() {
        assert_eq!(file_key("2 findings"), "findings");
        assert_eq!(file_key("review comments"), "review-comments");
        assert_eq!(file_key("42"), "fixes");
    }

    #[test]
    fn the_toast_says_what_waits() {
        let mut review = reviewed().review.unwrap();
        assert_eq!(
            waiting_text("T", &review).as_deref(),
            Some("AI review of \u{201c}T\u{201d}: 1 finding to decide")
        );
        review.findings[2].state = FindingState::Fix;
        assert_eq!(
            waiting_text("T", &review).as_deref(),
            Some("AI review of \u{201c}T\u{201d}: 1 finding to send to Build")
        );
        review.findings[2].state = FindingState::Dismissed;
        assert_eq!(waiting_text("T", &review), None);
    }
}
