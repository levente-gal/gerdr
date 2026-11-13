//! Conflicts taking a base into a shipping feature's branch, from Ship's
//! page, the way other changes go: an agent plans how to resolve them for
//! the user to review, or resolves them at once. It takes on every
//! repository that conflicts at once. To resolve, gerdr starts the merges in
//! the feature's worktrees and one agent finishes them in a tab; once their
//! commits land, the feature goes back to Verify as any update from the base
//! does (`Feature::updated_from_base`).

use crate::{
    Gerdr,
    confirm::Then,
    doc_review::{CONFLICT_PLAN_PAST, DocFile},
    feature::{
        self, Resolving, Stage, Taking, UpdateWay,
        agents::{command_line, with_prompt},
        ship::{self, MergeBegun, Resolution},
        store,
    },
    feature_agents::{vars, write_prompt},
    features::{Busy, Done, Job},
    prompts,
};

/// The next turn at conflicts in data dir `data`: past any whose files are
/// there, its plan and questions or its prompt (`conflicts-<n>.md`,
/// `conflicts-plan-<n>.md`), so an old plan is never taken for this one's
/// nor an old prompt, or its agent's chat, written over.
fn next_turn(data: &std::path::Path) -> u32 {
    let names = [data.to_path_buf(), data.join("prompts")]
        .into_iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.file_name().to_string_lossy().into_owned());
    let taken = names
        .filter_map(|name| {
            let rest = name.strip_prefix("conflicts-")?;
            let rest = rest.strip_prefix("plan-").unwrap_or(rest);
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0);
    taken + 1
}

/// The repositories as a prompt lists them: each worktree `dirs` has for
/// it, what it takes in and its conflicted files. Those that merged cleanly
/// are left out: nothing is left there to do.
fn listed(repos: &[Taking], dirs: &[(String, String)], planning: bool) -> String {
    let doing = match planning {
        true => "would conflict in",
        false => "merged in, stopped on conflicts in",
    };
    (repos.iter())
        .filter(|t| planning || !t.files.is_empty())
        .map(|t| {
            let dir = (dirs.iter())
                .find(|(key, _)| *key == t.repo)
                .map_or(t.repo.as_str(), |(_, dir)| dir.as_str());
            let files: String = t.files.iter().map(|f| format!("\n  - {f}")).collect();
            format!("- {dir}: {} {doing}:{files}", t.onto)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Start the merges of `repos` (each with the files its base conflicts
/// in) of `feature`, or for `planning` only read what each takes in: every
/// worktree is checked before any merge starts. Those that started, and
/// why the rest didn't.
fn begin_all(
    feature: &feature::Feature,
    repos: Vec<(String, Vec<String>)>,
    planning: bool,
) -> (Vec<(String, MergeBegun)>, Option<String>) {
    if !planning {
        let unready: Vec<String> = (repos.iter())
            .filter_map(|(repo, _)| ship::conflict_merge_ready(feature, repo).err())
            .collect();
        if !unready.is_empty() {
            return (Vec::new(), Some(unready.join("; ")));
        }
    }
    let mut begun = Vec::new();
    for (repo, known) in repos {
        let one = match planning {
            true => ship::conflict_ends(feature, &repo).map(|ends| MergeBegun {
                files: known,
                ..ends
            }),
            false => ship::begin_conflict_merge(feature, &repo),
        };
        match one {
            Ok(b) => begun.push((repo, b)),
            // Those begun go on to the agent, and can be stopped there.
            Err(why) => return (begun, Some(why)),
        }
    }
    (begun, None)
}

impl Gerdr {
    /// Take on the conflicts of every repository of feature `slug` with
    /// its base, at once: `planning`, an agent plans their resolution and
    /// changes nothing; else gerdr starts the merges and an agent resolves
    /// them. Resolving as planned takes on the plan's repositories.
    pub(crate) fn resolve_conflicts(&mut self, project: &str, slug: &str, planning: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        // One take at a time: resolving as planned goes on from the plan.
        let going = feature.shipping.resolving.as_ref();
        if let Some(r) = going.filter(|r| !r.planning) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is taking on the conflicts of {} already",
                r.names()
            ));
        }
        if let Err(why) = self.summary_agent(&feature) {
            return self.show_warning(format!("Couldn't take the conflicts on: {why}"));
        }
        let bases = self.last_probe(project, slug).map(|p| p.bases.clone());
        let known = |key: &str| {
            (bases.as_ref().and_then(|b| b.get(key)))
                .map(|b| b.conflicts.clone())
                .unwrap_or_default()
        };
        let repos: Vec<(String, Vec<String>)> = match going {
            Some(r) => (r.repos.iter())
                .map(|t| (t.repo.clone(), known(&t.repo)))
                .collect(),
            None => (feature.repos.iter())
                .map(|r| r.key())
                .map(|key| {
                    let files = known(&key);
                    (key, files)
                })
                .filter(|(_, files)| !files.is_empty())
                .collect(),
        };
        if repos.is_empty() {
            return self.show_warning(format!(
                "Nothing of \u{201c}{title}\u{201d} conflicts with its base now"
            ));
        }
        let only = (repos.len() == 1).then(|| repos[0].0.clone());
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: only,
            what: format!("Taking on the conflicts of \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let (begun, failed) = begin_all(&feature, repos, planning);
            Done::ConflictsBegun {
                project,
                slug,
                planning,
                begun,
                failed,
            }
        });
    }

    /// The merges for the conflicts of `begun`'s repositories started, or
    /// the plan of them is to be written: one agent takes them on in a tab.
    /// `failed`: why the rest didn't start.
    pub(crate) fn conflicts_begun(
        &mut self,
        project: &str,
        slug: &str,
        planning: bool,
        begun: Vec<(String, MergeBegun)>,
        failed: Option<String>,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        if let Some(why) = failed {
            tracing::warn!("taking on the conflicts of {project}/{slug}: {why}");
            self.show_error(format!(
                "Couldn't take on the conflicts of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        if begun.is_empty() {
            return;
        }
        let data = store::dir(None, project, slug);
        // Resolving as planned stays the plan's turn; else a new one.
        let n = match feature.shipping.resolving.as_ref() {
            Some(r) if r.planning && !planning => r.n,
            _ => next_turn(&data),
        };
        let repos: Vec<Taking> = (begun.into_iter())
            .map(|(repo, b)| Taking {
                repo,
                onto: b.onto,
                onto_sha: b.onto_sha,
                before: b.before,
                files: b.files,
            })
            .collect();
        let mut resolving = Resolving {
            at: feature::now(),
            repos,
            agent: String::new(),
            pane: None,
            n,
            planning,
            nudged: false,
        };
        let names = resolving.names();
        // They merged cleanly after all: the next read finds them done.
        if !planning && resolving.repos.iter().all(|t| t.files.is_empty()) {
            return self.set_resolving(project, slug, resolving, "Merged cleanly");
        }
        // The merges are under way: kept now, so they can be stopped
        // whatever comes of the agent.
        if !planning {
            let what = format!("Merges into {names} started for their conflicts");
            self.set_resolving(project, slug, resolving.clone(), &what);
        }
        let (agent, base) = match self.summary_agent(&feature) {
            Ok(pick) => pick,
            Err(why) => return self.show_warning(format!("Couldn't take the conflicts on: {why}")),
        };
        let dirs: Vec<(String, String)> = (resolving.repos.iter())
            .filter_map(|t| {
                let dir = feature.repo(&t.repo)?.worktree.display().to_string();
                Some((t.repo.clone(), dir))
            })
            .collect();
        let to_do: Vec<&Taking> = (resolving.repos.iter())
            .filter(|t| planning || !t.files.is_empty())
            .collect();
        // One repository: its agent works in its worktree; more, in the
        // feature's folder over them all.
        let dir = match to_do.as_slice() {
            [one] => match feature.repo(&one.repo) {
                Some(repo) => repo.worktree.clone(),
                None => return,
            },
            _ => feature.dir.clone(),
        };
        let names_vars = self.projects.names(&feature);
        let mut values = vars(&feature, &names_vars, &data, Stage::Ship);
        values.insert(
            "conflicts.repos".into(),
            listed(&resolving.repos, &dirs, planning),
        );
        let plan = resolving.plan(&data);
        let followed = if !planning && plan.exists() {
            format!(
                "Follow the plan at {}, as the user reviewed it.\n",
                plan.display()
            )
        } else {
            String::new()
        };
        values.insert("conflicts.plan".into(), followed);
        values.insert("conflicts.plan_path".into(), plan.display().to_string());
        let questions = resolving.questions(&data);
        values.insert(
            "conflicts.questions".into(),
            questions.display().to_string(),
        );
        let template = if planning {
            "conflicts-plan"
        } else {
            "conflicts"
        };
        let text = match prompts::render_builtin(template, &values) {
            Ok(text) => text,
            Err(err) => {
                return self.show_error(format!("Couldn't write the conflicts prompt: {err}"));
            }
        };
        let prompt = match write_prompt(&data, &format!("{template}-{n}.md"), &text) {
            Ok(prompt) => prompt,
            Err(why) => {
                tracing::warn!("{why}");
                return self.show_error(why);
            }
        };
        let line = command_line(&base, &prompt, &feature.agent_dirs_for(&base, &data));
        let command = with_prompt(&line, &format!("Read {} and follow it.", prompt.display()));
        let Some(pane) = self.open_feature_tab_behind(project, slug, &dir, command) else {
            return;
        };
        resolving.agent = agent.clone();
        resolving.pane = Some(pane);
        // Its agent too, now.
        let what = match planning {
            true => format!("{agent} plans the resolution of the conflicts in {names}"),
            false => format!("{agent} resolves the conflicts in {names}"),
        };
        self.set_resolving(project, slug, resolving, &what);
        self.show_success(format!("\u{201c}{title}\u{201d}: {what}"));
    }

    /// Keep `resolving` on feature `slug`, logging `what`.
    fn set_resolving(&mut self, project: &str, slug: &str, resolving: Resolving, what: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        feature.shipping.resolving = Some(resolving);
        feature.log(now, what.to_owned());
        self.save_feature(project, slug);
        self.probe_feature(project, slug);
    }

    /// A read of feature `slug` found its conflicts as `found`, its code
    /// `patch`'s: once no merge is under way any more, each committed one
    /// takes it back to Verify; an aborted one ends there. Says whether it
    /// took the code on, so the marks needn't go.
    pub(crate) fn resolution_read(
        &mut self,
        project: &str,
        slug: &str,
        found: &Resolution,
        patch: Option<&str>,
    ) -> bool {
        let Some(feature) = self.feature_mut(project, slug) else {
            return false;
        };
        let Some(resolving) = feature.shipping.resolving.clone() else {
            return false;
        };
        // Read for another take: old by now.
        let other = found.at != resolving.at || found.planning != resolving.planning;
        if other || resolving.planning || found.merging() {
            return false;
        }
        let title = feature.title.clone();
        let now = feature::now();
        feature.shipping.resolving = None;
        let mut resolved = Vec::new();
        let mut stopped = Vec::new();
        for t in &resolving.repos {
            // Committed: the base is in.
            let Some(f) = found
                .of(&t.repo)
                .filter(|f| f.took_in && f.head != t.before)
            else {
                stopped.push(t.repo.clone());
                continue;
            };
            let update = feature::BaseUpdate {
                at: now,
                repo: t.repo.clone(),
                way: UpdateWay::Merge,
                onto: t.onto.clone(),
                onto_sha: t.onto_sha.clone(),
                before: t.before.clone(),
                after: f.head.clone(),
            };
            feature.log(now, format!("Conflicts in {} resolved", t.repo));
            feature.updated_from_base(update, patch, now);
            resolved.push(t.repo.clone());
        }
        let (resolved, stopped) = (resolved.join(", "), stopped.join(", "));
        if !stopped.is_empty() {
            feature.log(
                now,
                format!("Conflicts in {stopped}: the merge was stopped"),
            );
        }
        self.save_feature(project, slug);
        if resolved.is_empty() {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d}: the merge in {stopped} was stopped before it was committed"
            ));
            return false;
        }
        self.feature_cursor_to_current_of(project, slug);
        match stopped.is_empty() {
            true => self.show_success(format!(
                "\u{201c}{title}\u{201d}: the conflicts in {resolved} are resolved; back to Verify"
            )),
            false => self.show_warning(format!(
                "\u{201c}{title}\u{201d}: the conflicts in {resolved} are resolved, the merge in \
                 {stopped} was stopped; back to Verify"
            )),
        }
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.build.verifying.contains(&key) {
            self.recheck(project, slug);
        }
        true
    }

    /// Ask before stopping the conflicts feature `slug`'s agent takes on.
    pub(crate) fn ask_stop_resolving(&mut self, project: &str, slug: &str) {
        let Some(resolving) = self
            .feature(project, slug)
            .and_then(|f| f.shipping.resolving.clone())
        else {
            return;
        };
        // The question stays one line: past two, a count; the body names them.
        let names = match resolving.repos.len() {
            0..=2 => resolving.names(),
            n => format!("{n} repositories"),
        };
        let (question, body) = match (resolving.planning, resolving.repos.len()) {
            (true, 0..=2) => (
                format!("Stop planning the conflicts of {names}?"),
                "The plan, if written, stays. Its agent's tab stays open.".to_owned(),
            ),
            (true, _) => (
                format!("Stop planning the conflicts of {names}?"),
                format!(
                    "Of {}. The plan, if written, stays. Its agent's tab stays open.",
                    resolving.names()
                ),
            ),
            (false, 1) => (
                format!("Stop the merge in {names}?"),
                format!(
                    "The merge of {} is aborted: what was resolved goes, and the branch \
                     stays as it was. Its agent's tab stays open: close it too.",
                    resolving.onto()
                ),
            ),
            (false, _) => (
                format!("Stop the merges in {names}?"),
                format!(
                    "Each merge of {} into {} not committed yet is aborted: what was resolved \
                     there goes, and its branch stays as it was. Its agent's tab stays open: \
                     close it too.",
                    resolving.onto(),
                    resolving.names()
                ),
            ),
        };
        let then = Then::StopResolving {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        self.ask(question, body, "Stop", then);
    }

    /// Stop the conflicts feature `slug`'s agent takes on, once asked: the
    /// merge is aborted off the thread.
    pub(crate) fn stop_resolving(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: None,
            what: format!("Stopping the merge of \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::ResolvingStopped {
            result: ship::abort_conflict_merge(&feature),
            project,
            slug,
        });
    }

    /// The merges feature `slug`'s agent resolved were aborted, or weren't;
    /// those `committed` already stay, and the next read takes them back
    /// to Verify.
    pub(crate) fn resolving_stopped(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<Vec<String>, String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let committed = match result {
            Ok(committed) => committed,
            Err(why) => {
                tracing::warn!("stopping the merge of {project}/{slug}: {why}");
                return self.show_error(why);
            }
        };
        let now = feature::now();
        if !committed.is_empty() {
            let committed = committed.join(", ");
            feature.log(
                now,
                format!("Conflicts stopped; the merge in {committed} was committed already"),
            );
            self.save_feature(project, slug);
            self.probe_feature(project, slug);
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d}: the merge in {committed} was committed already: \
                 it goes back to Verify"
            ));
        }
        if let Some(r) = feature.shipping.resolving.take() {
            feature.log(now, format!("Conflicts in {}: stopped", r.names()));
        }
        self.save_feature(project, slug);
        self.probe_feature(project, slug);
        self.show_success(format!(
            "Stopped taking on the conflicts of \u{201c}{title}\u{201d}"
        ));
    }

    /// Open feature `slug`'s plan of its conflicts' resolution, to review.
    pub(crate) fn review_conflict_plan(&mut self, project: &str, slug: &str) {
        let Some(resolving) = self
            .feature(project, slug)
            .and_then(|f| f.shipping.resolving.clone())
        else {
            return;
        };
        let data = store::dir(None, project, slug);
        if !resolving.plan(&data).exists() {
            return self.show_warning("The plan isn't written yet".to_owned());
        }
        // Reviewed as a stage's file is: Approve resolves as planned.
        self.open_doc_tab(project, slug, DocFile::ConflictPlan(resolving.n));
    }

    /// The plan of conflicts `file` shows is approved: they are resolved as
    /// planned, its review done.
    pub(crate) fn approve_conflict_plan(&mut self, project: &str, slug: &str, file: DocFile) {
        let open = (self.feature(project, slug))
            .and_then(|f| f.shipping.resolving.as_ref())
            .is_some_and(|r| r.planning && DocFile::ConflictPlan(r.n) == file);
        if !open {
            return self.show_warning(CONFLICT_PLAN_PAST.into());
        }
        self.doc_done(project, slug, file);
        self.resolve_conflicts(project, slug, false);
    }

    /// Send the plan of conflicts `file` shows back to its agent with
    /// `comments` (Markdown), kept beside it: it writes the plan again.
    pub(crate) fn send_back_conflict_plan(
        &mut self,
        project: &str,
        slug: &str,
        file: DocFile,
        comments: String,
    ) {
        let Some(r) = (self.feature(project, slug))
            .and_then(|f| f.shipping.resolving.clone())
            .filter(|r| r.planning && DocFile::ConflictPlan(r.n) == file)
        else {
            return self.show_warning(CONFLICT_PLAN_PAST.into());
        };
        let data = store::dir(None, project, slug);
        let sent = data.join(format!("conflicts-{}-comments.md", r.n));
        if let Err(err) = crate::projects::write_atomic(&sent, comments.as_bytes()) {
            tracing::warn!("writing {}: {err}", sent.display());
            return self.show_error(format!("Couldn't keep the comments: {err}"));
        }
        let line = format!(
            "The user sent your plan back: read their comments in {} and write the plan at {} again.",
            sent.display(),
            r.plan(&data).display()
        );
        // A pane back at its shell would run the line as a command.
        let told = match (r.pane.as_deref(), self.session.herdr.as_mut()) {
            (Some(pane), Some(herdr)) => match crate::prepare::step(&herdr.process_info(pane)) {
                crate::prepare::Step::Busy => herdr.send_agent_line(pane, &line),
                _ => Err(crate::feature_build::AGENT_QUIT.to_owned()),
            },
            (None, _) => Err("its agent's tab is gone".to_owned()),
            (_, None) => Err("herdr isn't connected".to_owned()),
        };
        if let Err(why) = told {
            tracing::warn!("sending the conflicts plan of {project}/{slug} back: {why}");
            return self.show_error(format!("Couldn't send the plan back: {why}"));
        }
        self.doc_done(project, slug, file);
        if let Some(feature) = self.feature_mut(project, slug) {
            feature.log(
                feature::now(),
                format!("Conflicts plan of {} sent back", r.names()),
            );
            self.save_feature(project, slug);
        }
        self.show_success(format!("Sent the plan back to {}", r.agent));
    }
}

#[cfg(test)]
mod tests {
    use super::{Taking, listed, next_turn};

    /// The prompt lists each repository by its worktree with its files;
    /// resolving, one that merged cleanly is left out.
    #[test]
    fn the_prompt_lists_each_repository_with_its_files() {
        let take = |repo: &str, files: &[&str]| Taking {
            repo: repo.into(),
            onto: "origin/main".into(),
            onto_sha: "b".into(),
            before: "a".into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        };
        let repos = [take("api", &["a.rs", "b.rs"]), take("web", &[])];
        let dirs = [("api".to_owned(), "/w/x/api".to_owned())];
        assert_eq!(
            listed(&repos, &dirs, false),
            "- /w/x/api: origin/main merged in, stopped on conflicts in:\n  - a.rs\n  - b.rs"
        );
        assert_eq!(
            listed(&repos, &dirs, true),
            "- /w/x/api: origin/main would conflict in:\n  - a.rs\n  - b.rs\n\
             - web: origin/main would conflict in:"
        );
    }

    #[test]
    fn a_turn_at_conflicts_comes_after_any_with_files_or_prompts() {
        let data = std::env::temp_dir().join(format!("gerdr-turns-{}", std::process::id()));
        std::fs::create_dir_all(data.join("prompts")).unwrap();
        assert_eq!(next_turn(&data), 1);
        // A turn that wrote no plan nor questions still left its prompt.
        std::fs::write(data.join("prompts").join("conflicts-1.md"), "x").unwrap();
        assert_eq!(next_turn(&data), 2);
        std::fs::write(data.join("prompts").join("conflicts-plan-3.md"), "x").unwrap();
        assert_eq!(next_turn(&data), 4);
        std::fs::write(data.join("conflicts-4-questions.md"), "x").unwrap();
        assert_eq!(next_turn(&data), 5);
        std::fs::remove_dir_all(&data).unwrap();
    }
}
