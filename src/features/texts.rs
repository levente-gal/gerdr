//! The feature tab's strip of running work, and the words of toasts and asks.

use super::*;

/// How long work must have run before the feature tab's strip shows it:
/// a quick read of the code doesn't flash there.
const RUNNING_AFTER: Duration = Duration::from_millis(300);
/// How many rows of work the strip shows; it counts the rest.
const RUNNING_ROWS: usize = 3;

/// The feature tab's strip of what runs on feature `title`: each of `busy`
/// with when it started, shown once it ran `RUNNING_AFTER` at `now`, the
/// first `RUNNING_ROWS` of them with their jobs, and how many more run.
pub(super) fn running_rows(
    busy: &[(&Busy, Instant)],
    title: &str,
    now: Instant,
) -> (Vec<(chrome::RunningRow, Job)>, usize) {
    let shown: Vec<_> = (busy.iter())
        .filter(|(_, since)| now.saturating_duration_since(*since) >= RUNNING_AFTER)
        .collect();
    let more = shown.len().saturating_sub(RUNNING_ROWS);
    let rows = (shown.into_iter().take(RUNNING_ROWS))
        .map(|(b, since)| {
            let (what, steps) = steps_of(&running_what(&b.what, title));
            let secs = now.saturating_duration_since(*since).as_secs();
            let stage = match b.job {
                Job::Setup => Some(Stage::Build),
                Job::Verify => Some(Stage::Verify),
                Job::Sandbox | Job::HandBack => Some(Stage::ManualTest),
                Job::Ship => Some(Stage::Ship),
                _ => None,
            };
            let row = chrome::RunningRow {
                word: running_word(b.job),
                what,
                steps,
                took: format!("{}:{:02}", secs / 60, secs % 60),
                buttons: running_buttons(b.job).into_iter().map(|(l, _)| l).collect(),
                node: stage.and_then(|s| Stage::ALL.iter().position(|a| *a == s)),
            };
            (row, b.job)
        })
        .collect();
    (rows, more)
}

/// What the strip calls work of kind `job`, inline and so lowercase.
fn running_word(job: Job) -> &'static str {
    match job {
        Job::Create => "creating",
        Job::Link => "linking",
        Job::Touch => "updating",
        Job::Losses | Job::Read => "reading",
        Job::Remove | Job::RemoveRepos => "removing",
        Job::Ship => "shipping",
        Job::Setup => "setting up",
        Job::Verify => "checks",
        Job::Graph => "code graph",
        Job::Sandbox => "sandbox",
        Job::HandBack => "handing back",
        Job::PrHead => "on GitHub",
    }
}

/// The buttons on the strip's row for work of kind `job`: what the
/// stage's page offers for it too.
pub(super) fn running_buttons(job: Job) -> Vec<(&'static str, PanelOutcome)> {
    match job {
        Job::Setup => vec![("Log", PanelOutcome::SetupLog)],
        Job::Verify => vec![("Stop", PanelOutcome::StopChecks)],
        _ => Vec::new(),
    }
}

/// `what` (the tasks panel's title) without the feature's quoted `title`
/// and what goes before it, which the strip on its tab needn't say:
/// "Setting up “Export”: api: npm ci" is "api: npm ci".
fn running_what(what: &str, title: &str) -> String {
    let quoted = format!("\u{201c}{title}\u{201d}");
    match what.split_once(&quoted) {
        Some((_, rest)) => rest.trim_start_matches(':').trim().to_owned(),
        None => what.to_owned(),
    }
}

/// `text` without the count of steps it ends on, and that count: "api: npm
/// ci (2 of 3)" and "2 of 5 done" count, as setup, the sandbox and Verify
/// say them.
// shortcut: reads the count back from the progress text the workers write;
// give `Done::Progress` a count of its own if they ever word it otherwise.
pub(super) fn steps_of(text: &str) -> (String, Option<(u32, u32)>) {
    let none = || (text.to_owned(), None);
    let Some(at) = text.rfind(" of ") else {
        return none();
    };
    let (head, tail) = (&text[..at], &text[at + 4..]);
    let before = head.trim_end_matches(|c: char| c.is_ascii_digit());
    let n_len = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (Ok(i), Ok(n)) = (head[before.len()..].parse(), tail[..n_len].parse()) else {
        return none();
    };
    let before = before.trim_end_matches('(').trim_end();
    let after = tail[n_len..].trim_start_matches(')');
    let after = after.strip_prefix(" done").unwrap_or(after).trim();
    let text: Vec<&str> = [before, after]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    (text.join(" "), Some((i, n)))
}

/// The tasks panel's row for `busy`, work on the feature called `title`
/// (when it has one yet) of the project called `project`: setup and
/// Verify's checks name the feature when their progress doesn't.
pub(super) fn task_of(busy: &Busy, title: Option<&str>, project: &str) -> Task {
    let chip = match busy.job {
        Job::Setup => "setup",
        Job::Verify => "checks",
        Job::Graph => "graph",
        Job::Sandbox => "sandbox",
        Job::HandBack => "hand back",
        Job::PrHead => "update",
        _ => "feature",
    };
    let detail = match title {
        Some(title) if !busy.what.contains(title) => {
            format!("\u{201c}{title}\u{201d} \u{b7} {project}")
        }
        _ => project.to_owned(),
    };
    let actions = match busy.job {
        Job::Setup => vec![(
            "Log",
            MenuAction::SetupLog(busy.project.clone(), busy.slug.clone()),
        )],
        _ => Vec::new(),
    };
    Task {
        title: busy.what.clone(),
        chip: chip.into(),
        detail,
        dot: Dot::Working,
        actions,
    }
}

/// What the toast says once feature `title` is made under `root`, naming
/// the files `.features` was added to (`ignores`) from the root: whole
/// paths are long enough to cut the last ones off.
pub(crate) fn created_text(title: &str, root: &Path, ignores: &[PathBuf]) -> String {
    if ignores.is_empty() {
        return format!("Created \u{201c}{title}\u{201d}");
    }
    let names: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| short_folder(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "Created \u{201c}{title}\u{201d} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        names.join(", ")
    )
}

/// What the toast says once `.features` left `root`'s `.cursorignore`
/// (checklist D1).
pub(super) fn cursor_freed_text(root: &Path) -> String {
    format!(
        "Cursor agents can edit feature folders \u{b7} moved {} from {} to {} in {}",
        layout::FEATURES_DIR,
        layout::CURSOR_DENY,
        layout::CURSOR_INDEX,
        short_folder(root)
    )
}

/// What the toast says once `names` were linked to feature `title`, and
/// the ignore files that took it (checklist D1).
pub(super) fn linked_text(
    title: &str,
    names: &[String],
    root: &Path,
    ignores: &[PathBuf],
) -> String {
    let text = format!("\u{201c}{title}\u{201d} holds {} now", names.join(", "));
    if ignores.is_empty() {
        return text;
    }
    let files: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| short_folder(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "{text} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        files.join(", ")
    )
}

/// Whether `feature`'s code is still the code whose patch-id is `patch`,
/// the one its Ship check was on, with nothing uncommitted; else why not.
/// Runs git: off the UI thread.
pub(super) fn still_checked(feature: &Feature, patch: Option<&str>) -> Result<(), String> {
    let reading = patch::read(feature)?;
    if !reading.dirty.is_empty() {
        return Err(format!(
            "{} has uncommitted changes",
            reading.dirty.join(", ")
        ));
    }
    if patch != Some(reading.patch.as_str()) {
        return Err("its code changed since the check".into());
    }
    Ok(())
}

/// Drop repository `key` from `feature`, and the link it was the last of.
pub(super) fn drop_repo(feature: &mut Feature, key: &str, now: u64) {
    // A review's pull request goes with its repository.
    // An unnamed one, of a record from before, is its first repository's.
    let first = feature.repos.first().is_some_and(|r| r.key() == key);
    if let feature::Kind::PrReview(pulls) = &mut feature.kind {
        pulls.retain(|p| !(p.repo == key || p.repo.is_empty() && first));
    }
    if let Some(repo) = feature.repo(key).cloned() {
        feature.note_removed(repo.project, repo.name, now);
    }
    feature.repos.retain(|r| r.key() != key);
    let repos = &feature.repos;
    feature
        .links
        .retain(|l| repos.iter().any(|r| r.project.as_ref() == Some(&l.project)));
}

/// Whether taking `keys` out of `feature` asks first: when `losses` go
/// with them, and always for a repository on the feature's branch.
pub(super) fn removal_asks(feature: &Feature, keys: &[String], losses: &[String]) -> bool {
    !losses.is_empty() || (keys.iter()).any(|k| feature.repo(k).is_some_and(|r| r.touched))
}

/// What a confirm dialog about taking repositories out says of `losses`,
/// what goes with their worktrees, and of what happens after: as context
/// agents go on reading them; else they can come back, on a new branch
/// and without what was lost. A line each: together they don't fit the
/// dialog's width.
pub(super) fn remove_repos_body(repos: usize, losses: &[String], to_context: bool) -> String {
    let what = if repos == 1 {
        "Its worktree and branch go"
    } else {
        "Their worktrees and branches go"
    };
    let lost = match losses {
        [] => format!("{what}; nothing in them is lost"),
        losses => format!("{what}, and with them {}", losses.join(", ")),
    };
    let after = match (to_context, repos) {
        (true, 1) => "Agents started from now on read its main checkout",
        (true, _) => "Agents started from now on read their main checkouts",
        (false, 1) => "You can add it back, on a new branch",
        (false, _) => "You can add them back, on new branches",
    };
    format!("{lost}.\n{after}.")
}

/// What a confirm dialog about removing a feature's worktrees says of
/// `losses`, what goes with them.
pub(super) fn removal_body(what: &str, losses: &[String]) -> String {
    match losses {
        [] => format!("Its {what} go; nothing in them is lost"),
        [one] => format!("Its {what} go, and with them {one}"),
        more => format!("Its {what} go, and with them {}", more.join(", ")),
    }
}

/// What removing `feature`'s worktrees takes, in words: "worktree and
/// branch", or "worktrees and branches" with several repositories.
pub(super) fn worktrees_words(feature: &Feature) -> &'static str {
    match feature.touched().count() > 1 {
        true => "worktrees and branches",
        false => "worktree and branch",
    }
}

/// What `layout::find_root` goes by in project `project`: its workspaces'
/// folders, the repositories it ignores, and their base branches.
pub(crate) fn root_search(
    project: &crate::projects::Project,
) -> (Vec<PathBuf>, Vec<String>, Vec<(String, String)>) {
    let dirs = project.workspaces.iter().map(|w| w.dir.clone()).collect();
    let bases = (project.workflow.base.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (dirs, project.workflow.ignore.clone(), bases)
}

/// What a repository row's button does beyond its one word, as its
/// tooltip says it.
pub(super) fn repo_button_tip(feature: &Feature, outcome: &PanelOutcome) -> Option<String> {
    match outcome {
        PanelOutcome::Touch(_) | PanelOutcome::AddToFeature(_) => {
            Some(format!("Add a worktree on {}", feature.branch()))
        }
        PanelOutcome::AddContext(_) => {
            Some("Agents can read its main checkout; no worktree".to_owned())
        }
        PanelOutcome::MakeContext(_) => {
            Some("Removes its worktree; agents can read its main checkout".to_owned())
        }
        _ => None,
    }
}

/// What the toast says when repository `named` becomes context: an agent
/// already running keeps what it started with.
pub(super) fn context_text(named: &str) -> String {
    format!("Agents started from now on can read {named}")
}

/// Whether the repositories of `feature` can still change: open, and not
/// shipped.
pub(super) fn linkable(feature: &Feature) -> bool {
    feature.status == Status::Open && !feature.marks.contains_key(&Stage::Ship)
}

/// What the new feature dialog shows of `dialog`, in project `project`.
pub(super) fn new_feature_view(dialog: &NewFeatureDialog, project: String) -> NewFeatureView {
    let place = |base: &str, ticked: bool, fixed: bool| {
        let word = match (fixed, ticked) {
            (true, _) => "the only repository",
            (_, true) => "linked",
            (_, false) => "left out",
        };
        (word.to_owned(), Some(format!("from {base}")))
    };
    let found = |root: &Root| {
        let n = root.repos.len();
        if root.multi {
            format!("In {} \u{b7} {n} repositories", short_folder(&root.path))
        } else {
            format!("In {}", short_folder(&root.path))
        }
    };
    let chosen = dialog.choosing.as_ref().map(|c| &c.repo);
    let of_chosen = chosen.and_then(|repo| dialog.bases.get(repo));
    let chosen_name = chosen.map_or("", |repo| repo.1.as_str());
    let review = dialog.review.as_ref();
    let pulls_of = |key: &RepoKey| review.and_then(|r| r.pulls.get(key));
    let (root, root_failed) = if let (Some(repo), Some(_)) = (chosen, review) {
        match pulls_of(repo).and_then(|e| e.list.as_ref()) {
            None => (
                format!("listing the pull requests of {chosen_name}\u{2026}"),
                false,
            ),
            Some(Err(why)) => (
                format!("Couldn't list the pull requests of {chosen_name}: {why}"),
                true,
            ),
            Some(Ok(all)) if all.is_empty() => {
                (format!("{chosen_name} has no open pull request"), true)
            }
            Some(Ok(_)) if dialog.repos() == 0 => ("No pull request matches that".to_owned(), true),
            Some(Ok(_)) => (format!("Open pull requests of {chosen_name}"), false),
        }
    } else if let Some(note) = review
        .and_then(|r| r.note.as_ref())
        .filter(|_| !dialog.picking)
    {
        (note.clone(), false)
    } else if chosen.is_some() {
        match of_chosen.and_then(|e| e.branches.as_ref()) {
            None => (
                format!("listing the branches of {chosen_name}\u{2026}"),
                false,
            ),
            Some(Err(why)) => (
                format!("Couldn't list the branches of {chosen_name}: {why}"),
                true,
            ),
            Some(Ok(_)) if dialog.repos() == 0 => ("No branch matches that".to_owned(), true),
            Some(Ok(_)) => (format!("Pick the branch {chosen_name} starts from"), false),
        }
    } else if dialog.picking && dialog.others.is_empty() {
        // Nothing went wrong: there is just nothing to link.
        (
            "No other project to link: add one with the project menu first".to_owned(),
            false,
        )
    } else {
        match &dialog.root {
            Finding::Looking => ("finding repositories\u{2026}".to_owned(), false),
            Finding::Failed(why) => (format!("No repositories found: {why}"), true),
            Finding::Found(root) => (found(root), false),
        }
    };
    let branches = dialog.branch_rows();
    let pull_rows = dialog.pull_rows();
    let picked = chosen.and_then(|repo| pulls_of(repo)?.picked);
    // A repository's row in a review: its pull request picked, or why none.
    let pull_row = |key: RepoKey, name: String| {
        let entry = pulls_of(&key);
        let pull = entry.and_then(RepoPulls::picked_pull);
        let place = match (pull, entry.and_then(|e| e.list.as_ref())) {
            (Some(p), _) => {
                let by = (!p.author.is_empty()).then(|| format!("@{}", p.author));
                let parts: Vec<String> = (!p.title.is_empty())
                    .then(|| p.title.clone())
                    .into_iter()
                    .chain(by)
                    .collect();
                parts.join(" \u{b7} ")
            }
            (None, None) => "listing pull requests\u{2026}".to_owned(),
            (None, Some(Err(why))) => format!("can't list: {why}"),
            (None, Some(Ok(all))) if all.is_empty() => "no open pull request".to_owned(),
            (None, Some(Ok(_))) => "left out".to_owned(),
        };
        NewFeatureRepo {
            name,
            place,
            touched: pull.is_some(),
            fixed: false,
            base: Some(
                pull.map_or_else(|| "Pick\u{2026}".to_owned(), |p| format!("#{}", p.number)),
            ),
        }
    };
    let current = chosen.and_then(|repo| dialog.repo_found(repo).map(|r| r.base.clone()));
    let repos = dialog
        .rows()
        .into_iter()
        .filter_map(|row| {
            let fixed = dialog.fixed(row);
            Some(match row {
                DialogRow::Home(_) | DialogRow::Linked(..) if review.is_some() => {
                    let key = dialog.row_repo(row)?;
                    let name = match &key.0 {
                        Some(p) => {
                            let other = dialog.others.iter().find(|o| o.project == *p)?;
                            format!("{}: {}", other.name, key.1)
                        }
                        None => key.1.clone(),
                    };
                    pull_row(key, name)
                }
                DialogRow::Branch(i) if review.is_some() => {
                    let p = pull_rows.get(i)?;
                    let mut place = vec![format!("@{}", p.author), p.head_ref.clone()];
                    if p.draft {
                        place.push("draft".into());
                    }
                    NewFeatureRepo {
                        name: format!("#{} {}", p.number, p.title),
                        place: place.join(" \u{b7} "),
                        touched: picked == Some(p.number),
                        fixed: false,
                        base: None,
                    }
                }
                DialogRow::Home(i) => {
                    let Finding::Found(root) = &dialog.root else {
                        return None;
                    };
                    let (r, touched) = (root.repos.get(i)?, *dialog.touched.get(i)?);
                    let (place, base) = place(&r.base, touched, fixed);
                    NewFeatureRepo {
                        name: r.name.clone(),
                        place,
                        touched,
                        fixed,
                        base,
                    }
                }
                DialogRow::Linked(k, i) => {
                    let other = &dialog.others[k];
                    let Finding::Found(root) = &other.root else {
                        return None;
                    };
                    let (r, touched) = (root.repos.get(i)?, *other.touched.get(i)?);
                    let (place, base) = place(&r.base, touched, false);
                    NewFeatureRepo {
                        name: format!("{}: {}", other.name, r.name),
                        place,
                        touched,
                        fixed: false,
                        base,
                    }
                }
                DialogRow::Project(k) => {
                    let other = &dialog.others[k];
                    let place = match &other.root {
                        Finding::Looking => "finding repositories\u{2026}".to_owned(),
                        Finding::Failed(why) => format!("can't link: {why}"),
                        Finding::Found(root) => found(root),
                    };
                    NewFeatureRepo {
                        name: other.name.clone(),
                        place,
                        touched: other.linked,
                        fixed,
                        base: None,
                    }
                }
                DialogRow::Branch(i) => {
                    let name = branches.get(i)?.clone();
                    NewFeatureRepo {
                        place: match of_chosen.is_some_and(|e| e.default == name) {
                            true => "default".to_owned(),
                            false => String::new(),
                        },
                        touched: current.as_ref() == Some(&name),
                        fixed: false,
                        base: None,
                        name,
                    }
                }
            })
        })
        .collect();
    let mut projects = format!("Projects: {project} (home)");
    for other in dialog.others.iter().filter(|o| o.linked) {
        projects.push_str(&format!(" \u{b7} {}", other.name));
    }
    let reviewing = review.is_some() && !dialog.picking;
    let mut keys = match (dialog.choosing.is_some(), dialog.multi()) {
        (true, _) if review.is_some() => vec![("Tab", "next"), ("Enter", "pick")],
        (true, _) => vec![("Tab", "next"), ("Enter", "use branch")],
        (false, true) if reviewing && dialog.focus == Some(REVIEWER) => {
            vec![("Tab", "next"), ("Left / Right", "reviewer")]
        }
        (false, true) if reviewing => vec![("Tab", "next"), ("Space", "pick or leave out")],
        // Nothing to tick with no project to link.
        (false, true) if dialog.picking && dialog.others.is_empty() => Vec::new(),
        (false, true) => vec![("Tab", "next"), ("Space", "tick")],
        (false, false) => Vec::new(),
    };
    if !dialog.picking && dialog.choosing.is_none() && dialog.base_row().is_some() {
        keys.push(match reviewing {
            true => ("Alt+B", "pull request"),
            false => ("Alt+B", "base branch"),
        });
    }
    keys.push(if dialog.choosing.is_some() {
        ("Ctrl+Shift+L", "link project")
    } else if dialog.picking {
        ("Ctrl+Shift+L", "repositories")
    } else {
        ("Ctrl+Shift+L", "link project")
    });
    // Esc steps back as `NewFeatureDialog::key` has it.
    keys.push(if dialog.choosing.is_some() && !dialog.query.is_empty() {
        ("Esc", "clear")
    } else if dialog.choosing.is_some() || dialog.picking || dialog.focus.is_some() {
        ("Esc", "back")
    } else if !dialog.title.is_empty() {
        ("Esc", "clear")
    } else {
        ("Esc", "close")
    });
    let heading = match review {
        Some(Reviewing {
            into: Some((_, title)),
            ..
        }) => format!("Pull requests of \u{201c}{title}\u{201d}"),
        // A picker of its own over the form.
        _ if chosen.is_some() && review.is_some() => {
            format!("Pull request of \u{201c}{chosen_name}\u{201d}")
        }
        _ if chosen.is_some() => format!("Base branch of \u{201c}{chosen_name}\u{201d}"),
        Some(_) => format!("Review pull requests in \u{201c}{project}\u{201d}"),
        None => format!("New feature in \u{201c}{project}\u{201d}"),
    };
    let create = match review.and_then(|r| r.into.as_ref()) {
        Some(_) => chrome::SAVE_PULLS,
        None => chrome::NEW_FEATURE_BUTTONS[chrome::CREATE],
    };
    NewFeatureView {
        heading,
        title: match dialog.choosing {
            Some(_) => dialog.query.line(),
            None => dialog.title.line(),
        },
        hint: match (&dialog.choosing, review) {
            (Some(_), Some(_)) => "Filter pull requests",
            (Some(_), None) => "Filter branches",
            (None, Some(r)) if r.into.is_some() => chrome::RETITLE_HINT,
            (None, Some(_)) => chrome::REVIEW_TITLE_HINT,
            (None, None) => chrome::TITLE_HINT,
        },
        // The filter takes the keys while a branch row is selected.
        field_focus: dialog.choosing.is_some() || dialog.focus.is_none(),
        focus: dialog.focus,
        slots: dialog.slots(),
        root,
        root_failed,
        projects,
        link: chrome::LINK_BUTTON[usize::from(dialog.picking)],
        repos,
        top: dialog.top,
        error: dialog.error.clone(),
        hover: dialog.hover,
        keys,
        create,
        choosing: chosen.is_some(),
        reviewers: dialog.reviewers().to_vec(),
        reviewer_focus: dialog.choosing.is_none() && dialog.focus == Some(REVIEWER),
    }
}
