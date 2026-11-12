//! Conflicts taking a base into a shipping feature's branch, from Ship's
//! page, the way other changes go: an agent plans how to resolve them for
//! the user to review, or resolves them at once. To resolve, gerdr starts
//! the merge in the feature's worktree and Build's agent finishes it in a
//! tab; once its commit lands, the feature goes back to Verify as any
//! update from the base does (`Feature::updated_from_base`).

use crate::{
    Gerdr,
    confirm::Then,
    doc_review::{CONFLICT_PLAN_PAST, DocFile},
    feature::{
        self, Resolving, Stage, UpdateWay,
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

/// The files as a prompt lists them.
fn listed(files: &[String]) -> String {
    files
        .iter()
        .map(|f| format!("- {f}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Gerdr {
    /// Take on the conflicts of repository `repo` of feature `slug`:
    /// `planning`, an agent plans their resolution and changes nothing; else
    /// gerdr starts the merge and an agent resolves it.
    pub(crate) fn resolve_conflicts(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        planning: bool,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        // One at a time: resolving as planned goes on from the plan.
        let going = feature.shipping.resolving.as_ref();
        if let Some(r) = going.filter(|r| r.repo != repo || !r.planning) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is taking on the conflicts of {} already",
                r.repo
            ));
        }
        if let Err(why) = self.summary_agent(&feature) {
            return self.show_warning(format!("Couldn't take the conflicts on: {why}"));
        }
        let known = (self.last_probe(project, slug))
            .and_then(|p| p.bases.get(repo))
            .map(|b| b.conflicts.clone())
            .unwrap_or_default();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: Some(repo.to_owned()),
            what: format!("Taking on the conflicts of \u{201c}{title}\u{201d}"),
        };
        let (project, slug, repo) = (project.to_owned(), slug.to_owned(), repo.to_owned());
        self.feature_work(Some(busy), move |_| {
            let result = match planning {
                true => ship::conflict_ends(&feature, &repo).map(|ends| MergeBegun {
                    files: known,
                    ..ends
                }),
                false => ship::begin_conflict_merge(&feature, &repo),
            };
            Done::ConflictsBegun {
                project,
                slug,
                repo,
                planning,
                result,
            }
        });
    }

    /// The merge for repository `repo`'s conflicts started, or the plan of
    /// them is to be written: an agent takes them on in a tab.
    pub(crate) fn conflicts_begun(
        &mut self,
        project: &str,
        slug: &str,
        (repo, planning): (&str, bool),
        result: Result<MergeBegun, String>,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let begun = match result {
            Ok(begun) => begun,
            Err(why) => {
                tracing::warn!("taking on the conflicts of {project}/{slug}: {why}");
                return self.show_error(format!(
                    "Couldn't take on the conflicts of \u{201c}{title}\u{201d}: {why}"
                ));
            }
        };
        let data = store::dir(None, project, slug);
        // Resolving as planned stays the plan's turn; else a new one.
        let n = match feature.shipping.resolving.as_ref() {
            Some(r) if r.planning && !planning => r.n,
            _ => next_turn(&data),
        };
        let mut resolving = Resolving {
            at: feature::now(),
            repo: repo.to_owned(),
            onto: begun.onto.clone(),
            onto_sha: begun.onto_sha.clone(),
            before: begun.before.clone(),
            files: begun.files.clone(),
            agent: String::new(),
            pane: None,
            n,
            planning,
            nudged: false,
        };
        // It merged cleanly after all: the next read finds it done.
        if begun.files.is_empty() && !planning {
            return self.set_resolving(project, slug, resolving, "Merged cleanly");
        }
        // The merge is under way: kept now, so it can be stopped whatever
        // comes of the agent.
        if !planning {
            let what = format!(
                "Merge of {} into {repo} started for its conflicts",
                begun.onto
            );
            self.set_resolving(project, slug, resolving.clone(), &what);
        }
        let (agent, base) = match self.summary_agent(&feature) {
            Ok(pick) => pick,
            Err(why) => return self.show_warning(format!("Couldn't take the conflicts on: {why}")),
        };
        let Some(dir) = feature.repo(repo).map(|r| r.worktree.clone()) else {
            return;
        };
        let names = self.projects.names(&feature);
        let mut values = vars(&feature, &names, &data, Stage::Ship);
        values.insert("conflicts.dir".into(), dir.display().to_string());
        values.insert("conflicts.onto".into(), begun.onto.clone());
        values.insert("conflicts.files".into(), listed(&begun.files));
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
            true => format!("{agent} plans the resolution of the conflicts in {repo}"),
            false => format!("{agent} resolves the conflicts in {repo}"),
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
    /// `patch`'s: once the merge is committed it goes back to Verify; a
    /// merge aborted ends it. Says whether it took the code on, so the
    /// marks needn't go.
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
        if other || resolving.planning || found.merging {
            return false;
        }
        let title = feature.title.clone();
        let now = feature::now();
        feature.shipping.resolving = None;
        // Committed: the base is in.
        if found.took_in && found.head != resolving.before {
            let update = feature::BaseUpdate {
                at: now,
                repo: resolving.repo.clone(),
                way: UpdateWay::Merge,
                onto: resolving.onto.clone(),
                onto_sha: resolving.onto_sha.clone(),
                before: resolving.before.clone(),
                after: found.head.clone(),
            };
            feature.log(now, format!("Conflicts in {} resolved", resolving.repo));
            feature.updated_from_base(update, patch, now);
            self.save_feature(project, slug);
            self.feature_cursor_to_current_of(project, slug);
            self.show_success(format!(
                "\u{201c}{title}\u{201d}: the conflicts in {} are resolved; back to Verify",
                resolving.repo
            ));
            let key = (project.to_owned(), slug.to_owned());
            if !self.features.build.verifying.contains(&key) {
                self.recheck(project, slug);
            }
            return true;
        }
        feature.log(
            now,
            format!("Conflicts in {}: the merge was stopped", resolving.repo),
        );
        self.save_feature(project, slug);
        self.show_warning(format!(
            "\u{201c}{title}\u{201d}: the merge in {} was stopped before it was committed",
            resolving.repo
        ));
        false
    }

    /// Ask before stopping the conflicts feature `slug`'s agent takes on.
    pub(crate) fn ask_stop_resolving(&mut self, project: &str, slug: &str) {
        let Some(resolving) = self
            .feature(project, slug)
            .and_then(|f| f.shipping.resolving.clone())
        else {
            return;
        };
        let (question, body) = match resolving.planning {
            true => (
                format!("Stop planning the conflicts of {}?", resolving.repo),
                "The plan, if written, stays. Its agent's tab stays open.".to_owned(),
            ),
            false => (
                format!("Stop the merge in {}?", resolving.repo),
                format!(
                    "The merge of {} is aborted: what was resolved goes, and the branch \
                     stays as it was. Its agent's tab stays open: close it too.",
                    resolving.onto
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

    /// The merge feature `slug`'s agent resolved was aborted, or wasn't.
    pub(crate) fn resolving_stopped(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<(), String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("stopping the merge of {project}/{slug}: {why}");
            return self.show_error(why);
        }
        let now = feature::now();
        if let Some(r) = feature.shipping.resolving.take() {
            feature.log(now, format!("Conflicts in {}: stopped", r.repo));
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
        let Some(r) = (self.feature(project, slug))
            .and_then(|f| f.shipping.resolving.clone())
            .filter(|r| r.planning && DocFile::ConflictPlan(r.n) == file)
        else {
            return self.show_warning(CONFLICT_PLAN_PAST.into());
        };
        self.doc_done(project, slug, file);
        self.resolve_conflicts(project, slug, &r.repo, false);
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
            feature.log(feature::now(), format!("Conflicts plan of {} sent back", r.repo));
            self.save_feature(project, slug);
        }
        self.show_success(format!("Sent the plan back to {}", r.agent));
    }
}

#[cfg(test)]
mod tests {
    use super::next_turn;

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
