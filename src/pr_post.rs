//! Ship's page of a pull request's review: submitting the review to GitHub
//! (see `feature::pr_submit`), pushing the fixes of the user's own pull
//! request, and finishing.
//!
//! A submitted review leaves it open: once the author pushes again, it
//! goes over the new code from Verify on (`pr_watch`), what was posted kept
//! as posted, and a later review posts only what is new.

use std::path::Path;

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
        if f.pulls().iter().all(|p| p.mine) && event != Event::Comment {
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
        if f.pulls().is_empty() {
            return;
        }
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
        let mut body = format!("{what}\nIt is posted as you and can't be taken back from gerdr");
        // GitHub refuses Approve and Request changes on your own pull request.
        let yours: Vec<String> = (f.pulls().iter())
            .filter(|p| p.mine && event != Event::Comment)
            .map(|p| format!("#{}", p.number))
            .collect();
        if !yours.is_empty() {
            body.push_str(&format!(
                "\n{} is yours: it gets a comment",
                yours.join(", ")
            ));
        }
        let verb = event.label().trim_end_matches('\u{2026}');
        let numbers: Vec<String> = f.pulls().iter().map(|p| format!("#{}", p.number)).collect();
        let title = format!("{verb} on {}?", numbers.join(" and "));
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
        let Some(first) = f.pull().cloned() else {
            return;
        };
        let items = self.submit_items(f);
        let data = store::dir(None, project, slug);
        let n = f.shipping.reviews.len() + 1;
        let text = std::fs::read_to_string(data.join(BODY_FILE)).unwrap_or_default();
        // One review per pull request: its own items; the first takes the
        // rest and the user's words, naming the others. GitHub takes only a
        // comment on one's own.
        let others: Vec<String> = (f.pulls().iter().skip(1))
            .map(|p| format!("#{}", p.number))
            .collect();
        let mut jobs = Vec::new();
        for (k, pull) in f.pulls().iter().enumerate() {
            let Some(repo) = f.repo(&pull.repo).or(f.repos.first()).cloned() else {
                continue;
            };
            let mine = pr_submit::of_pull(f, &items, &pull.repo, k == 0);
            let ev = if pull.mine { Event::Comment } else { event };
            let own = match (k, others.is_empty()) {
                (0, true) => text.clone(),
                (0, false) => format!(
                    "{}\n\nReviewed together with {}.",
                    text.trim(),
                    others.join(", ")
                )
                .trim()
                .to_owned(),
                _ => String::new(),
            };
            // Nothing to say on it: left out.
            if ev == Event::Comment && mine.is_empty() && own.trim().is_empty() && k > 0 {
                continue;
            }
            jobs.push((repo, pull.clone(), mine, ev, own));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Submitting the review of #{}", first.number),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let results = (jobs.into_iter())
                .map(|(repo, pull, items, ev, own)| {
                    let done = post(&repo, &pull, &data, n, ev, items, &own).map(Box::new);
                    (pull.number, done)
                })
                .collect();
            Done::ReviewPosted {
                project,
                slug,
                results,
            }
        });
    }

    /// Review `slug` was submitted, or not: mark what went as posted, keep
    /// the review, and let the next one start afresh.
    pub(crate) fn review_posted(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(u64, Result<Box<Submitted>, String>)>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let mut went = false;
        for (number, result) in results {
            match result {
                Ok(done) => {
                    went = true;
                    self.one_posted(project, slug, *done);
                }
                Err(why) => {
                    tracing::warn!("submitting the review of #{number} of {project}/{slug}: {why}");
                    self.show_error(format!("Couldn't submit the review of #{number}: {why}"));
                }
            }
        }
        if !went {
            return;
        }
        // The review's text went with it: the next starts empty.
        let data = store::dir(None, project, slug);
        let n = self
            .feature(project, slug)
            .map_or(1, |f| f.shipping.reviews.len());
        let body = data.join(BODY_FILE);
        if body.exists()
            && let Err(err) = std::fs::rename(&body, data.join(format!("review-{n}.md")))
        {
            tracing::warn!("keeping {}: {err}", body.display());
        }
        self.probe_feature(project, slug);
        self.refresh_pr_reviews();
    }

    /// One pull request's review went (`done`): mark what went as posted and
    /// keep the review.
    fn one_posted(&mut self, project: &str, slug: &str, done: Submitted) {
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
        let number = f.pull_of(&posted.repo).map_or(0, |p| p.number);
        let mut text = format!("Review of #{number} submitted: {}", posted.event.done());
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
    }

    /// Ask before pushing the commits of the user's own pull request
    /// review `slug` to its branch on GitHub.
    pub(crate) fn ask_push_pr(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has work running: wait for it",
                f.title
            ));
        }
        let pushes = self.to_push(f);
        if pushes.is_empty() {
            return self.show_warning("Nothing fixed here to push".into());
        }
        let lines: Vec<String> = (pushes.iter())
            .map(|(_, p)| {
                format!(
                    "#{} onto {}, while still at {}",
                    p.number,
                    p.head_ref,
                    p.short_head()
                )
            })
            .collect();
        let body = format!("Its fixes go to GitHub:\n{}", lines.join("\n"));
        let then = Then::PushPr {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        let to = match pushes.as_slice() {
            [(_, one)] => one.head_ref.clone(),
            many => format!("{} branches", many.len()),
        };
        self.ask(format!("Push to {to}?"), body, "Push", then);
    }

    /// The pull requests of `f` worked on here whose repository's head, as
    /// its code was last read, isn't theirs on GitHub; all worked on here
    /// before it was read.
    fn to_push(&self, f: &Feature) -> Vec<(feature::Repo, feature::pr::PullRef)> {
        let heads = (self.last_probe(&f.project, &f.slug))
            .and_then(|p| p.reading.as_ref().ok())
            .map(|r| &r.heads);
        (f.pulls().iter())
            .filter(|p| p.works_here())
            .filter_map(|p| {
                let repo = f.repo(&p.repo).or(f.repos.first())?;
                let moved = heads.is_none_or(|h| h.get(&repo.key()) != Some(&p.head_sha));
                moved.then(|| (repo.clone(), p.clone()))
            })
            .collect()
    }

    /// Push the commits of the user's own pull request review `slug`, off
    /// the thread, with a lease on the head it was last on.
    pub(crate) fn push_pr(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let pushes = self.to_push(f);
        let Some((_, first)) = pushes.first() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Pushing to {}", first.head_ref),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PrPushed {
            results: (pushes.iter())
                .map(|(repo, pull)| (pull.repo.clone(), pull.number, push(repo, pull)))
                .collect(),
            project,
            slug,
        });
    }

    /// The user's own pull request review `slug` was pushed, or not: its
    /// worktree's head is the pull request's now.
    pub(crate) fn pr_pushed(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(String, u64, Result<String, String>)>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let mut done = Vec::new();
        for (key, number, result) in results {
            let head = match result {
                Ok(head) => head,
                Err(why) => {
                    tracing::warn!("pushing #{number} of {project}/{slug}: {why}");
                    self.show_error(format!("Couldn't push #{number}: {why}"));
                    continue;
                }
            };
            let Some(f) = self.feature_mut(project, slug) else {
                return;
            };
            let Some(pull) = f.pulls_mut().iter_mut().find(|p| p.repo == key) else {
                continue;
            };
            let base = pull.base_ref.clone();
            pull.moved_to(&head, &base);
            let text = format!("Pushed to {} at {}", pull.head_ref, pull.short_head());
            f.log(feature::now(), text.clone());
            done.push(text);
        }
        if done.is_empty() {
            return;
        }
        self.save_feature(project, slug);
        self.show_success(done.join(", "));
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
        let others: Vec<_> = (f.pulls().iter()).filter(|p| !p.own()).cloned().collect();
        let Some(pull) = others.first() else {
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
        let asks: Vec<_> = (others.iter())
            .filter_map(|p| Some((f.repo(&p.repo).or(f.repos.first())?.main.clone(), p.clone())))
            .collect();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Asking GitHub who can push to #{}", pull.number),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PushAccess {
            results: (asks.iter())
                .map(|(dir, pull)| {
                    (
                        pull.repo.clone(),
                        pull.number,
                        feature::pr::push_access(dir, pull),
                    )
                })
                .collect(),
            project,
            slug,
        });
    }

    /// Whether the user can push to each of review `slug`'s pull requests
    /// is known: ask before fixing them here, or say why one can't be.
    pub(crate) fn push_access_read(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(String, u64, feature::pr::Access)>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let mut head_repos = Vec::new();
        for (key, number, result) in results {
            match result {
                Ok(Ok(head_repo)) => head_repos.push((key, head_repo)),
                Ok(Err(why)) => {
                    return self.show_warning(format!("Can't fix #{number} here: {why}"));
                }
                Err(why) => {
                    tracing::warn!("reading who can push to #{number} of {project}/{slug}: {why}");
                    return self.show_error(format!(
                        "Couldn't tell whether you can push to #{number}: {why}"
                    ));
                }
            }
        }
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let lines: Vec<String> = (head_repos.iter())
            .filter_map(|(key, at)| {
                let pull = f.pulls().iter().find(|p| p.repo == *key)?;
                Some(format!("#{} to {} on {at}", pull.number, pull.head_ref))
            })
            .collect();
        let body = format!(
            "Its code runs here: setup, its checks, and an agent fixing it\nIts fixes go out when you push them from Ship:\n{}",
            lines.join("\n")
        );
        let numbers: Vec<String> = (head_repos.iter())
            .filter_map(|(key, _)| f.pulls().iter().find(|p| p.repo == *key))
            .map(|p| format!("#{}", p.number))
            .collect();
        let then = Then::FixHere {
            project: project.to_owned(),
            slug: slug.to_owned(),
            head_repos,
        };
        self.ask(
            format!("Fix {} here?", numbers.join(", ")),
            body,
            "Fix here",
            then,
        );
    }

    /// Review `slug` fixes its pull requests here (`head_repos` their
    /// branches' repositories, by `PullRef::repo`), or comments again (`None`).
    pub(crate) fn set_fix_here(
        &mut self,
        project: &str,
        slug: &str,
        head_repos: Option<Vec<(String, String)>>,
    ) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let on = head_repos.is_some();
        // The switch is the review's: every pull request of someone else's.
        for pull in f.pulls_mut().iter_mut().filter(|p| !p.own()) {
            pull.fix_here = on;
            let to = (head_repos.iter().flatten()).find(|(key, _)| *key == pull.repo);
            if let Some((_, head_repo)) = to {
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
    own: &str,
) -> Result<Submitted, String> {
    let range = format!("{}...{}", repo.base, pull.head_sha);
    let diff = run_git(
        &repo.worktree,
        &["diff", "-U3", "--no-color", "--no-ext-diff", &range],
    )?;
    let hunks = pr_submit::Hunks::of(&diff);
    let from: Vec<From> = items.iter().map(|i| i.from.clone()).collect();
    let path = format!("repos/{}/pulls/{}/reviews", pull.slug, pull.number);
    let send = |placed: &pr_submit::Placed| -> Result<String, String> {
        let payload = pr_submit::payload(&pull.head_sha, event, own, placed);
        let file = data.join(format!("review-post-{n}-{}.json", pull.number));
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
            repo: repo.key(),
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
fn write(file: &Path, payload: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(payload).map_err(|err| err.to_string())?;
    crate::projects::write_atomic(file, text.as_bytes())
        .map_err(|err| format!("couldn't write {}: {err}", file.display()))
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

/// How long a git of `run_git`, a push among them, may take.
const GIT_TIME: std::time::Duration = std::time::Duration::from_secs(120);

/// What `git args` in `dir` prints, or why it failed.
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    // A push stuck on the network is stopped, its group with it.
    let out = crate::t3_update::output_within(crate::git::command(dir, args), GIT_TIME)
        .map_err(|err| format!("git {}: {err}", args[0]))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(why.map_or_else(|| format!("git {} failed", args[0]), str::to_owned));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::tests::{Temp, commit, run};

    #[test]
    fn a_head_moved_on_the_remote_refuses_the_push() {
        let t = Temp::new();
        let bare = t.0.join("origin.git");
        let bare_arg = bare.to_str().unwrap();
        run(&t.0, &["init", "-q", "--bare", "-b", "main", bare_arg]);
        let mine = t.repo("api", "main");
        run(&mine, &["remote", "add", "origin", bare_arg]);
        run(&mine, &["push", "-q", "origin", "main"]);
        let seen = run(&mine, &["rev-parse", "HEAD"]).trim().to_owned();
        // Someone else pushes to the branch after it was last seen.
        let other = t.0.join("other");
        run(&t.0, &["clone", "-q", bare_arg, other.to_str().unwrap()]);
        commit(&other, "b", "theirs");
        run(&other, &["push", "-q", "origin", "main"]);
        let theirs = run(&other, &["rev-parse", "HEAD"]).trim().to_owned();
        commit(&mine, "a", "mine");
        let repo = feature::Repo {
            name: "api".into(),
            main: mine.clone(),
            worktree: mine.clone(),
            base: "main".into(),
            base_commit: seen.clone(),
            touched: true,
            branch: Some("main".into()),
            project: None,
        };
        let mut pull = feature::pr::PullRef {
            mine: true,
            head_ref: "main".into(),
            head_sha: seen,
            ..Default::default()
        };
        assert!(push(&repo, &pull).is_err());
        assert_eq!(run(&bare, &["rev-parse", "main"]).trim(), theirs);
        // At the head GitHub has, it goes over it.
        pull.head_sha = theirs;
        let head = push(&repo, &pull).unwrap();
        assert_eq!(run(&bare, &["rev-parse", "main"]).trim(), head);
    }
}
