//! Ship's page of a pull request's review: submitting the review to GitHub
//! (see `feature::pr_submit`), pushing the fixes of the user's own pull
//! request, and finishing.
//!
//! A submitted review leaves it open: once the author pushes again, it
//! goes over the new code from Verify on (`pr_watch`), what was posted kept
//! as posted, and a later review posts only what is new.

use std::path::{Path, PathBuf};

use crate::{
    Gerdr,
    confirm::Then,
    feature::{
        self, Feature, Stage, Status, manual,
        pr_submit::{self, BODY_FILE, Event, From, Posted},
        review_import::FindingState,
        store,
    },
    feature_diff::COMMENTS,
    features::{Busy, Done, Job},
    menu::MenuAction,
    review,
    toast::Level,
};

/// What a submit took, for its end to mark.
pub(crate) struct Submitted {
    pub posted: Posted,
    /// What went, to mark posted.
    pub from: Vec<From>,
}

impl Gerdr {
    /// What a review of `f` would post now: its findings to comment, its
    /// comments of Code review as their file has them, and Manual test's
    /// failures and comments not sent.
    fn submit_items(&self, f: &Feature) -> Vec<pr_submit::Item> {
        let data = store::dir(None, &f.project, &f.slug);
        let own = review::load_file(&data.join(COMMENTS));
        let criteria = self.criteria(&f.project, &f.slug).to_vec();
        let unsent = manual::unsent(f.manual.as_ref(), &criteria);
        pr_submit::items(f, &own, &unsent)
    }

    /// Why review `slug` can't be submitted as `event` now, if it can't.
    fn submit_refusal(&self, f: &Feature, event: Event) -> Option<String> {
        let pull = f.pull()?;
        if f.status != Status::Open || f.writer.is_some() {
            return Some(format!("\u{201c}{}\u{201d} isn't open here", f.title));
        }
        if f.current() != Some(Stage::Ship) {
            let at = f.current().map_or("its stages", Stage::label);
            return Some(format!("Finish {at} first"));
        }
        if self.feature_busy(&f.project, &f.slug) {
            return Some(format!(
                "\u{201c}{}\u{201d} has work running: wait for it",
                f.title
            ));
        }
        if pull.mine && event != Event::Comment {
            return Some(
                "GitHub doesn't take an approval or a change request of your own pull request"
                    .into(),
            );
        }
        if !self.row_ctx(f).on_head {
            return Some(match pull.works_here() {
                true => "The code here isn't on GitHub yet: push it first".into(),
                false => format!(
                    "The code here isn't #{}'s head: update it first",
                    pull.number
                ),
            });
        }
        None
    }

    /// Ask before submitting review `slug` as `event`: what goes, and that
    /// it goes out as the user.
    pub(crate) fn ask_submit_review(&mut self, project: &str, slug: &str, event: Event) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let Some(pull) = f.pull() else {
            return;
        };
        if let Some(why) = self.submit_refusal(f, event) {
            return self.show_warning(why);
        }
        let items = self.submit_items(f);
        let data = store::dir(None, project, slug);
        let own = std::fs::read_to_string(data.join(BODY_FILE)).unwrap_or_default();
        if items.is_empty() && own.trim().is_empty() && event != Event::Approve {
            return self.show_warning(
                "Nothing to post: comment on a finding or the code, or write the review's text (Ctrl+E)"
                    .into(),
            );
        }
        let what = match items.len() {
            0 => "Only the review's text goes".to_owned(),
            1 => "1 comment goes, on its line where GitHub shows it".to_owned(),
            n => format!("{n} comments go, each on its line where GitHub shows it"),
        };
        let body = format!("{what}\nIt is posted as you and can't be taken back from gerdr");
        let verb = event.label().trim_end_matches('\u{2026}');
        let title = format!("{verb} on #{}?", pull.number);
        let then = Then::SubmitReview {
            project: project.to_owned(),
            slug: slug.to_owned(),
            event,
        };
        self.ask(title, body, "Submit", then);
    }

    /// Submit review `slug` as `event`, off the thread: its items on lines
    /// GitHub's diff shows, the rest in its text; GitHub refusing the lines,
    /// all in its text.
    pub(crate) fn submit_review(&mut self, project: &str, slug: &str, event: Event) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        if let Some(why) = self.submit_refusal(f, event) {
            return self.show_warning(why);
        }
        let (Some(pull), Some(repo)) = (f.pull().cloned(), f.repos.first().cloned()) else {
            return;
        };
        let items = self.submit_items(f);
        let data = store::dir(None, project, slug);
        let n = f.shipping.reviews.len() + 1;
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Submitting the review of #{}", pull.number),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let result = post(&repo, &pull, &data, n, event, items).map(Box::new);
            Done::ReviewPosted {
                project,
                slug,
                result,
            }
        });
    }

    /// Review `slug` was submitted, or not: mark what went as posted, keep
    /// the review, and let the next one start afresh.
    pub(crate) fn review_posted(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<Box<Submitted>, String>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let done = match result {
            Ok(done) => *done,
            Err(why) => {
                tracing::warn!("submitting the review of {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't submit the review: {why}"));
            }
        };
        let data = store::dir(None, project, slug);
        let n = self
            .feature(project, slug)
            .map_or(1, |f| f.shipping.reviews.len() + 1);
        // The comments that went leave Code review's file, kept apart.
        let posted: Vec<&review::Comment> = (done.from.iter())
            .filter_map(|from| match from {
                From::Comment(c) => Some(c),
                _ => None,
            })
            .collect();
        if !posted.is_empty() {
            let path = data.join(COMMENTS);
            let mut own = review::load_file(&path);
            let kept = review::Review {
                comments: posted.iter().map(|c| (*c).clone()).collect(),
                ..review::Review::default()
            };
            own.comments.retain(|c| !posted.contains(&c));
            let archive = data.join(format!("posted-comments-{n}.json"));
            let saved =
                review::save_file(&archive, &kept).and_then(|()| review::save_file(&path, &own));
            if let Err(err) = saved {
                tracing::warn!("keeping the posted comments of {project}/{slug}: {err}");
                self.show_warning(format!(
                    "The comments went, but couldn't be cleared here: {err}"
                ));
            }
        }
        // The review's text went with it: the next starts empty.
        let body = data.join(BODY_FILE);
        if body.exists()
            && let Err(err) = std::fs::rename(&body, data.join(format!("review-{n}.md")))
        {
            tracing::warn!("keeping {}: {err}", body.display());
        }
        let criteria = self.criteria(project, slug).to_vec();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        for from in &done.from {
            if let From::Finding(id) = from
                && let Some(found) = f
                    .review
                    .as_mut()
                    .and_then(|r| r.findings.iter_mut().find(|x| x.id == *id))
            {
                found.state = FindingState::Posted;
            }
        }
        if done.from.iter().any(|from| matches!(from, From::Manual(_))) {
            manual::mark_sent(&mut f.manual, &criteria);
        }
        let posted = done.posted;
        let mut text = format!(
            "Review of #{} submitted: {}",
            f.pull().map_or(0, |p| p.number),
            posted.event.done()
        );
        match (posted.inline, posted.in_body) {
            (0, 0) => {}
            (i, 0) => text.push_str(&format!(", {i} on lines")),
            (0, b) => text.push_str(&format!(", {b} in its text")),
            (i, b) => text.push_str(&format!(", {i} on lines, {b} in its text")),
        }
        if posted.flattened {
            text.push_str(" (GitHub refused the lines: all in its text)");
        }
        f.log(feature::now(), text.clone());
        let url = posted.url.clone();
        f.shipping.reviews.push(posted);
        self.save_feature(project, slug);
        let w = self
            .feature_workspace(project, slug)
            .unwrap_or(self.active_workspace);
        let open = ("Open".to_owned(), MenuAction::OpenPage(w, url));
        self.toast_with(text, Level::Success, Some(open));
        self.probe_feature(project, slug);
        self.refresh_pr_reviews();
    }

    /// Ask before pushing the commits of the user's own pull request
    /// review `slug` to its branch on GitHub.
    pub(crate) fn ask_push_pr(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let Some(pull) = f.pull().filter(|p| p.works_here()) else {
            return self
                .show_warning("Only your own pull request's branch is pushed from here".into());
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has work running: wait for it",
                f.title
            ));
        }
        let body = format!(
            "Its fixes go to GitHub, onto {}\nOnly while GitHub's branch is still at {}",
            pull.head_ref,
            pull.short_head()
        );
        let then = Then::PushPr {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        self.ask(format!("Push to {}?", pull.head_ref), body, "Push", then);
    }

    /// Push the commits of the user's own pull request review `slug`, off
    /// the thread, with a lease on the head it was last on.
    pub(crate) fn push_pr(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let (Some(pull), Some(repo)) = (
            f.pull().filter(|p| p.works_here()).cloned(),
            f.repos.first().cloned(),
        ) else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Pushing to {}", pull.head_ref),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PrPushed {
            result: push(&repo, &pull),
            project,
            slug,
        });
    }

    /// The user's own pull request review `slug` was pushed, or not: its
    /// worktree's head is the pull request's now.
    pub(crate) fn pr_pushed(&mut self, project: &str, slug: &str, result: Result<String, String>) {
        self.unbusy(project, slug, Job::PrHead);
        let head = match result {
            Ok(head) => head,
            Err(why) => {
                tracing::warn!("pushing {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't push: {why}"));
            }
        };
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        // shortcut: the first pull request only, until increment B pushes each.
        let Some(pull) = f.pulls_mut().first_mut() else {
            return;
        };
        let base = pull.base_ref.clone();
        pull.moved_to(&head, &base);
        let text = format!("Pushed to {} at {}", pull.head_ref, pull.short_head());
        f.log(feature::now(), text.clone());
        self.save_feature(project, slug);
        self.show_success(text);
        self.probe_feature(project, slug);
        self.refresh_pr_reviews();
    }
}

impl Gerdr {
    /// Fix review `slug`, of someone else's pull request, here, or go back
    /// to commenting. Fixing asks first, once gerdr read off the thread
    /// that the user can push to its branch; going back is refused while
    /// fixes made here aren't on GitHub.
    pub(crate) fn toggle_fix_here(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let Some(pull) = f.pull().filter(|p| !p.own()).cloned() else {
            return self.show_warning("Your own pull request is fixed here already".into());
        };
        if f.status != Status::Open || f.writer.is_some() {
            return self.show_warning(format!("\u{201c}{}\u{201d} isn't open here", f.title));
        }
        if self.feature_busy(project, slug) || f.fix_round.is_some() {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has work running: wait for it",
                f.title
            ));
        }
        if pull.fix_here {
            if !self.row_ctx(f).on_head {
                return self.show_warning(
                    "Fixes made here aren't on GitHub: push them, or drop them, first".into(),
                );
            }
            return self.set_fix_here(project, slug, None);
        }
        let Some(dir) = f.repos.first().map(|r| r.main.clone()) else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Asking GitHub who can push to #{}", pull.number),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PushAccess {
            result: feature::pr::push_access(&dir, &pull),
            project,
            slug,
        });
    }

    /// Whether the user can push to review `slug`'s pull request is known:
    /// ask before fixing it here, or say why it can't be.
    pub(crate) fn push_access_read(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<Result<String, String>, String>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let head_repo = match result {
            Ok(Ok(head_repo)) => head_repo,
            Ok(Err(why)) => return self.show_warning(format!("Can't fix it here: {why}")),
            Err(why) => {
                tracing::warn!("reading who can push to {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't tell whether you can push to it: {why}"));
            }
        };
        let Some(pull) = self.feature(project, slug).and_then(Feature::pull) else {
            return;
        };
        let body = format!(
            "Its code runs here: setup, its checks, and an agent fixing it\nIts fixes go to {} on {head_repo} when you push them from Ship",
            pull.head_ref
        );
        let then = Then::FixHere {
            project: project.to_owned(),
            slug: slug.to_owned(),
            head_repo,
        };
        self.ask(
            format!("Fix #{} here?", pull.number),
            body,
            "Fix here",
            then,
        );
    }

    /// Review `slug` fixes its pull request here (`head_repo` its branch's
    /// repository), or comments again (`None`).
    pub(crate) fn set_fix_here(&mut self, project: &str, slug: &str, head_repo: Option<String>) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let on = head_repo.is_some();
        // The switch is the review's: every pull request of someone else's.
        for pull in f.pulls_mut().iter_mut().filter(|p| !p.own()) {
            pull.fix_here = on;
            if let Some(head_repo) = &head_repo {
                pull.head_repo = head_repo.clone();
            }
        }
        let Some(pull) = f.pull() else {
            return;
        };
        let (number, head_ref) = (pull.number, pull.head_ref.clone());
        if let Some(review) = f.review.as_mut() {
            review.posts = !on;
        }
        let text = match on {
            true => format!(
                "Fixing #{number} here: its fixes go to an agent, and Push… on Ship sends them to {head_ref}"
            ),
            false => {
                format!("#{number} is commented on again: what its review wants goes to GitHub")
            }
        };
        f.log(feature::now(), text.clone());
        self.save_feature(project, slug);
        self.show_success(text);
        // Trusted now: it can be set up for the agent and its checks.
        if on
            && self
                .feature(project, slug)
                .is_some_and(|f| f.setup.is_none())
        {
            self.start_setup(project, slug);
        }
        self.probe_feature(project, slug);
    }
}

/// Post review number `n` of `pull` as `event`, from the worktree of `repo`,
/// its payload kept in `data`: what went.
fn post(
    repo: &feature::Repo,
    pull: &feature::pr::PullRef,
    data: &Path,
    n: usize,
    event: Event,
    items: Vec<pr_submit::Item>,
) -> Result<Submitted, String> {
    let range = format!("{}...{}", repo.base, pull.head_sha);
    let diff = run_git(
        &repo.worktree,
        &["diff", "-U3", "--no-color", "--no-ext-diff", &range],
    )?;
    let hunks = pr_submit::Hunks::of(&diff);
    let own = std::fs::read_to_string(data.join(BODY_FILE)).unwrap_or_default();
    let from: Vec<From> = items.iter().map(|i| i.from.clone()).collect();
    let path = format!("repos/{}/pulls/{}/reviews", pull.slug, pull.number);
    let send = |placed: &pr_submit::Placed| -> Result<String, String> {
        let payload = pr_submit::payload(&pull.head_sha, event, &own, placed);
        let file = data.join(format!("review-post-{n}.json"));
        write(&file, &payload)?;
        let file = file.to_string_lossy();
        let args = [
            "api",
            "--hostname",
            &pull.host,
            "-X",
            "POST",
            &path,
            "--input",
            &file,
        ];
        feature::ship::run_gh("gh", data, &args)
    };
    let placed = pr_submit::place(items.clone(), &hunks, true);
    let (out, placed, flattened) = match send(&placed) {
        Ok(out) => (out, placed, false),
        // A line GitHub won't take fails the whole review: all in its text.
        Err(why) if !placed.inline.is_empty() && refused_lines(&why) => {
            tracing::warn!(
                "GitHub refused the lines of #{}'s review: {why}",
                pull.number
            );
            let flat = pr_submit::place(items, &hunks, false);
            (send(&flat)?, flat, true)
        }
        Err(why) => return Err(why),
    };
    let url = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v["html_url"].as_str().map(str::to_owned))
        .unwrap_or_else(|| pull.url.clone());
    Ok(Submitted {
        posted: Posted {
            at: feature::now(),
            event,
            url,
            commit: pull.head_sha.clone(),
            inline: placed.inline.len(),
            in_body: placed.in_body.len(),
            flattened,
        },
        from,
    })
}

/// Whether GitHub's answer `why` says it couldn't place a comment's line.
fn refused_lines(why: &str) -> bool {
    why.contains("422") || why.contains("Unprocessable") || why.contains("could not be resolved")
}

/// Write `payload` to `file`.
fn write(file: &PathBuf, payload: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(payload).map_err(|err| err.to_string())?;
    std::fs::write(file, text).map_err(|err| format!("couldn't write {}: {err}", file.display()))
}

/// Push `dir`'s HEAD to `pull`'s branch, only while GitHub's is at the head
/// last seen of it: the head pushed.
/// Someone else's goes to their branch, in a fork when it is one's, past
/// the guard on gerdr's branch for it (`hook`) for this push only; what
/// was fetched of it moves along, as it is GitHub's now.
fn push(repo: &feature::Repo, pull: &feature::pr::PullRef) -> Result<String, String> {
    let dir = &repo.worktree;
    let origin = run_git(dir, &["remote", "get-url", "origin"])?;
    let Some(remote) = pull.push_remote(&origin) else {
        return Err(format!("couldn't tell the address of {}", pull.head_repo));
    };
    let head = run_git(dir, &["rev-parse", "HEAD"])?.trim().to_owned();
    let to = format!("HEAD:refs/heads/{}", pull.head_ref);
    let lease = format!(
        "--force-with-lease=refs/heads/{}:{}",
        pull.head_ref, pull.head_sha
    );
    let branch = pull.branch();
    let guarded = !pull.own();
    if guarded {
        feature::hook::allow(&repo.main, &branch, &head)?;
    }
    let pushed = run_git(dir, &["push", "-q", &lease, &remote, &to]);
    if guarded && let Err(err) = feature::hook::guard(&repo.main, &branch) {
        tracing::warn!("guarding {branch} again: {err}");
    }
    pushed?;
    if guarded && let Err(err) = run_git(dir, &["update-ref", &pull.fetch_ref(), &head]) {
        tracing::warn!("moving {}: {err}", pull.fetch_ref());
    }
    Ok(head)
}

/// What `git args` in `dir` prints, or why it failed.
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("couldn't run git: {err}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(why.map_or_else(|| format!("git {} failed", args[0]), str::to_owned));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
