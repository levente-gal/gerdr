//! `Gerdr`'s side of features: loading, saving, the AI work kept, work done off the thread.

use super::*;

impl Gerdr {
    // ------------------------------------------------------------ state

    /// Read the features of every project from their store, as at start
    /// and when the projects' file is read again. A project whose features
    /// can't be read keeps those it had, and says so.
    pub fn load_features(&mut self) {
        let ids: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let mut all = BTreeMap::new();
        self.features.kept.clear();
        for (id, name) in ids {
            crate::machine_lease::restore_left_records(&id);
            for kept in crate::machine_lease::load_kept(&id) {
                (self.features.kept).insert((kept.project.clone(), kept.slug.clone()), kept);
            }
            match store::load_all(None, &id) {
                Ok(features) => {
                    all.insert(id, features);
                }
                Err(why) => {
                    tracing::warn!("reading the features of {id}: {why}");
                    self.show_error(format!("Couldn't read the features of {name}: {why}"));
                    if let Some(kept) = self.features.all.remove(&id) {
                        all.insert(id, kept);
                    }
                }
            }
        }
        self.features.all = all;
        match store::take_moved() {
            0 => {}
            1 => self.show_success("Moved the activity of 1 feature to files of its own".into()),
            n => self.show_success(format!(
                "Moved the activity of {n} features to files of their own"
            )),
        }
        // What the records hold already happened: the AI work tells only
        // what changes from here.
        self.ai_work.seen.clear();
        let keys: Vec<(String, String)> = (self.features.all.iter())
            .flat_map(|(id, fs)| fs.iter().map(move |f| (id.clone(), f.slug.clone())))
            .collect();
        for (project, slug) in keys {
            self.ai_work_seed(&project, &slug);
        }
        // Chats at work before gerdr started, or before the projects were
        // read again, are kept again while they last.
        if self.ai_work.is_on() {
            let chats = self.features.ai_work_chats();
            self.ai_work.send(ai_work::thread::Job::Watch { chats });
        }
        self.seen_folders_changed();
        let all = &self.features.all;
        let viewed = &self.features.viewed;
        let kept = |(project, slug): &(String, String)| {
            [all, viewed].into_iter().any(|map| {
                map.get(project)
                    .is_some_and(|fs| fs.iter().any(|f| f.slug == *slug))
            })
        };
        self.features.probes.retain(|key, _| kept(key));
        self.features.asked.retain(|key, _| kept(key));
        self.watch_features();
        // The workspaces on features get their tab, those on features gone
        // lose it, and the panel follows.
        self.prune();
        self.sync_feature_panel();
    }

    pub fn feature(&self, project: &str, slug: &str) -> Option<&Feature> {
        self.features.feature(project, slug)
    }

    pub(crate) fn feature_mut(&mut self, project: &str, slug: &str) -> Option<&mut Feature> {
        self.features.feature_mut(project, slug)
    }

    /// Save feature `slug` of `project` as it is now; a failure says so.
    pub(crate) fn save_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).filter(|f| f.writer.is_none()) else {
            return;
        };
        match store::save(None, feature) {
            Ok(()) => self.ai_work_saved(project, slug),
            Err(err) => {
                tracing::warn!("saving feature {project}/{slug}: {err}");
                let title = feature.title.clone();
                self.show_error(format!("Couldn't save \u{201c}{title}\u{201d}: {err}"));
            }
        }
        self.plan_doc_follows();
        self.feature_diff_stands(project, slug);
    }

    /// Make branch `to.0`, at commit `to.1`, the base of repository `key` of
    /// feature `slug`, with an Undo unless `undoing` already. Its diff and
    /// patch-id are taken against it from now on.
    pub(crate) fn set_feature_base(
        &mut self,
        project: &str,
        slug: &str,
        key: &str,
        to: (String, String),
        undoing: bool,
    ) {
        let repo = (self.feature_mut(project, slug))
            .and_then(|f| f.repos.iter_mut().find(|r| r.key() == key));
        let Some(repo) = repo else {
            return self.show_warning(format!("{key} isn't in that feature any more"));
        };
        let branch = to.0.clone();
        let old = (
            std::mem::replace(&mut repo.base, to.0),
            std::mem::replace(&mut repo.base_commit, to.1),
        );
        self.save_feature(project, slug);
        let text = format!("{key} now has base {branch}");
        let undo = (!undoing).then(|| {
            let then = MenuAction::SetFeatureBase {
                project: project.to_owned(),
                slug: slug.to_owned(),
                repo: key.to_owned(),
                to: old,
                undoing: true,
            };
            ("Undo".to_owned(), then)
        });
        self.toast_with(text, Level::Success, undo);
        self.probe_feature(project, slug);
    }

    /// The AI work takes feature `slug` of `project` as it is now, telling
    /// nothing of it: what it holds already happened. For a record read
    /// from disk, or brought from another machine.
    pub(crate) fn ai_work_seed(&mut self, project: &str, slug: &str) {
        if !self.ai_work.is_on() {
            return;
        }
        let feature =
            (self.features.all.get(project)).and_then(|fs| fs.iter().find(|f| f.slug == slug));
        if let Some(feature) = feature {
            let digest = ai_work::observe::digest(feature);
            (self.ai_work.seen).insert((project.to_owned(), slug.to_owned()), digest);
        }
    }

    /// Feature `slug` of `project` was saved: what changed since the save
    /// before goes to its AI work. One made since gerdr started is all new.
    fn ai_work_saved(&mut self, project: &str, slug: &str) {
        if !self.ai_work.is_on() {
            return;
        }
        let key = (project.to_owned(), slug.to_owned());
        let feature =
            (self.features.all.get(project)).and_then(|fs| fs.iter().find(|f| f.slug == slug));
        let Some(feature) = feature else {
            return;
        };
        let prev = self.ai_work.seen.remove(&key).unwrap_or_default();
        let diffed = ai_work::observe::diff(Some(&prev), feature);
        self.ai_work_keep(feature, diffed.events, diffed.follow);
        self.ai_work.seen.insert(key, diffed.digest);
    }

    /// Feature `slug` of `project` is closed for good, its worktrees gone:
    /// its record and its chats are kept as they are now. Nothing of its
    /// AI work goes with it.
    pub(super) fn ai_work_close(&self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).filter(|_| self.ai_work.is_on()) else {
            return;
        };
        let mut follow: Vec<Follow> = (ai_work::observe::runs_of(feature))
            .map(|(stage, run)| {
                Follow::Capture(ai_work::observe::chat(feature, stage, run, run.session()))
            })
            .collect();
        follow.push(Follow::Snapshot);
        self.ai_work_keep(feature, Vec::new(), follow);
    }

    /// Append `events` of `feature` to its AI work, and give the thread what
    /// `follow` asks. A failed write is said as a problem, gone once one
    /// works again.
    fn ai_work_keep(
        &self,
        feature: &Feature,
        events: Vec<(Option<u64>, ai_work::Event)>,
        follow: Vec<Follow>,
    ) {
        use ai_work::{Event, Line, thread::Job, write};
        let (project, slug) = (feature.project.as_str(), feature.slug.as_str());
        let Some(dir) = self.ai_work.dir(project, slug) else {
            return;
        };
        let now = ai_work::now_ms();
        let append = |event: Event, at: Option<u64>| {
            let line = Line {
                at_ms: at.unwrap_or(now),
                ..Line::now(event)
            };
            write::append(&dir, &line)
        };
        let mut written = Vec::new();
        for (at, mut event) in events {
            if let Event::RunStarted {
                stage, n, prompt, ..
            } = &mut event
            {
                *prompt = keep_prompt(&dir, feature, *stage, *n);
            }
            written.push(append(event, at));
        }
        let data = store::dir(None, project, slug);
        let (p, s) = (project.to_owned(), slug.to_owned());
        for follow in follow {
            match follow {
                Follow::Capture(mut chat) => {
                    let there = match chat.machine.as_deref() {
                        Some(id) => match self.archive_reach(id) {
                            Some(there) => {
                                // Found there by its folders as that machine has them.
                                if let Some(paths) = self.machine_paths(id, Some(feature)) {
                                    chat.folders =
                                        chat.folders.iter().map(|f| paths.map(f)).collect();
                                }
                                Some(there)
                            }
                            // Its chat stays there: it is kept once watched again.
                            None => {
                                tracing::info!(
                                    "{project}/{slug}: {}'s chat isn't kept now: its machine can't be reached",
                                    chat.stage.label()
                                );
                                continue;
                            }
                        },
                        None => None,
                    };
                    self.ai_work.send(Job::Capture {
                        project: p.clone(),
                        slug: s.clone(),
                        chat,
                        there,
                    });
                }
                Follow::Outputs { files, reason } => self.ai_work.send(Job::Outputs {
                    project: p.clone(),
                    slug: s.clone(),
                    files: (files.into_iter())
                        .map(|name| {
                            let path = data.join(&name);
                            (name, path)
                        })
                        .collect(),
                    reason,
                }),
                Follow::SetupSlice { run } => self.ai_work.send(Job::SetupSlice {
                    project: p.clone(),
                    slug: s.clone(),
                    run,
                    log: feature::setup::log_path(&data),
                }),
                Follow::Snapshot => written.push(
                    snapshot(&dir, feature)
                        .and_then(|record| append(Event::Snapshot { record }, None)),
                ),
                Follow::Watch => self.ai_work.send(Job::Watch {
                    chats: self.features.ai_work_chats(),
                }),
            }
        }
        let wrote = written.iter().any(Result::is_ok);
        // Each feature's own: one that keeps failing keeps its problem.
        let at = format!("{p}/{s}");
        match written.into_iter().find_map(Result::err) {
            Some(err) => crate::problems::report_at(
                "ai-work",
                &at,
                format!(
                    "Couldn't keep the AI work of \u{201c}{}\u{201d}: {err}",
                    feature.title
                ),
            ),
            None if wrote => crate::problems::clear_at("ai-work", &at),
            None => {}
        }
        if wrote {
            self.ai_work.send(Job::Index {
                project: p,
                slug: s,
            });
        }
    }

    /// The slugs of the features the panel lists for `project`.
    pub(super) fn listed_slugs(&self, project: &str) -> Vec<String> {
        let own = self.features.all.get(project).map(|all| listed(all));
        let viewed = listed(self.viewed_of(project));
        (own.unwrap_or_default().into_iter().chain(viewed))
            .map(|f| f.slug.clone())
            .collect()
    }

    /// Where the threads send what they came to; set up the first time.
    pub(crate) fn feature_sender(&mut self) -> Option<channel::Sender<Done>> {
        if self.features.done.is_none() {
            let (tx, rx) = channel::channel::<Done>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg(done) => state.feature_done(done),
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.features.done = Some(tx),
                Err(err) => {
                    tracing::warn!("working on features: {err}");
                    self.show_error(format!("Couldn't start the work on features: {err}"));
                    return None;
                }
            }
        }
        self.features.done.clone()
    }

    /// Run `work` off the thread, its result sent back as a `Done`; `busy`
    /// shows in the tasks panel meanwhile.
    pub(crate) fn feature_work(
        &mut self,
        busy: Option<Busy>,
        work: impl FnOnce(channel::Sender<Done>) -> Done + Send + 'static,
    ) {
        let Some(tx) = self.feature_sender() else {
            return;
        };
        if let Some(b) = &busy {
            let key = (b.project.clone(), b.slug.clone(), b.job);
            self.features.since.entry(key).or_insert_with(Instant::now);
        }
        self.features.busy.extend(busy);
        std::thread::spawn(move || {
            // It fails only once the loop is gone: gerdr is quitting, and
            // nothing is left to show the result in.
            let done = work(tx.clone());
            let _ = tx.send(done);
        });
    }

    /// The end of work on a feature, the panel open or not: what it came
    /// to is kept and said.
    fn feature_done(&mut self, done: Done) {
        match done {
            Done::Sandbox {
                project,
                slug,
                run,
                stopped,
                opened,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_started(&project, &slug, run, stopped, opened);
            }
            Done::SandboxStopped {
                project,
                slug,
                what,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_stopped(&project, &slug, what, result);
            }
            Done::HandBack {
                project,
                slug,
                result,
            } => self.handed_back(&project, &slug, result),
            Done::SameRepos {
                id,
                project,
                result,
            } => self.same_repos_found(&id, &project, result),
            Done::SandboxTools {
                project,
                slug,
                missing,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_tools_found(&project, &slug, missing);
            }
            Done::SandboxAsked {
                project,
                slug,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_asked(&project, &slug, result);
            }
            Done::SandboxLeftGone {
                file,
                title,
                name,
                result,
            } => self.sandbox_left_gone(&file, &title, &name, result),
            Done::SandboxLeftovers {
                machine,
                stacks,
                still_paused,
            } => {
                self.sandbox_still_paused(machine.is_none(), still_paused);
                self.sandbox_leftovers(machine, stacks);
            }
            Done::SandboxLeftoversRemoved { removed, problems } => {
                self.sandbox_leftovers_removed(removed, problems);
            }
            Done::CommandsFound {
                serial,
                project,
                result,
                pages,
            } => self.commands_found(serial, &project, result, pages),
            Done::Found {
                serial,
                from,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("finding where a feature goes: {why}");
                }
                // Dropped when the dialog it was for closed: a new one
                // looks again.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    if let Some(from) = from {
                        dialog.from = from;
                    }
                    dialog.found(result);
                }
                self.find_bases();
            }
            Done::FoundRepos {
                project,
                slug,
                source,
                result,
            } => {
                if let Err(why) = &result {
                    let of = source.as_deref().unwrap_or(&project);
                    tracing::warn!("finding the repositories of project {of}: {why}");
                }
                let found = match result {
                    Ok(root) => Finding::Found(root),
                    Err(why) => Finding::Failed(why),
                };
                let key = (project.clone(), slug.clone());
                let open = (self.features.panel.as_mut())
                    .filter(|p| p.project == project && p.slug == slug);
                let feature = (self.features.all.get(&project))
                    .and_then(|list| list.iter().find(|f| f.slug == slug));
                if let Some(panel) = open.or_else(|| self.features.parked.get_mut(&key))
                    && let Some(feature) = feature
                    && let Some(at) = panel.sources.iter().position(|s| s.project == source)
                {
                    // New rows may come before the selected one: it stays.
                    let entries = repo_entries(feature, &panel.sources, panel.expanded);
                    let held = (entries.get(panel.repo))
                        .and_then(|e| entry_key(feature, &panel.sources, *e));
                    let pick = panel.repo_pick;
                    panel.sources[at].finding = found;
                    if let Some(key) = held {
                        panel.select_repo_keeping(feature, &key, pick);
                    }
                }
            }
            Done::FoundOther {
                serial,
                project,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("finding the repositories of project {project}: {why}");
                }
                // Dropped when the dialog it was for closed, as above.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.found_other(&project, result);
                }
                self.find_bases();
            }
            Done::BaseBranches {
                serial,
                repo,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("listing the branches of {}: {why}", repo.1);
                }
                // Dropped when the dialog it was for closed, as above.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.base_branches(&repo, result);
                }
            }
            Done::OpenPulls {
                serial,
                repo,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("listing the pull requests of {}: {why}", repo.1);
                }
                // Dropped when the dialog it was for closed, as above.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.pulls_listed(&repo, result);
                }
            }
            Done::ReviewAdded {
                project,
                slug,
                drops,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Link);
                self.review_added(&project, &slug, drops, result);
            }
            Done::RemoteBase {
                serial,
                repo,
                result,
            } => {
                // Offline is no fault of the dialog's: what git has here
                // stands, which the row shows.
                let branch = result.unwrap_or_else(|why| {
                    tracing::warn!("asking origin for the default of {}: {why}", repo.1);
                    None
                });
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                    && let Some(branch) = branch
                {
                    dialog.remote_base(&repo, branch);
                }
            }
            Done::DocsLinked { title, result } => match result {
                Ok(None) => {}
                Ok(Some(exclude)) => self.show_success(format!(
                    "Added /{} to {}",
                    layout::DOCS_LINK,
                    short_folder(&exclude)
                )),
                Err(why) => {
                    tracing::warn!("linking the stage files of {title}: {why}");
                    self.show_warning(format!(
                        "Agents in \u{201c}{title}\u{201d} can't name its stage files: {why}"
                    ));
                }
            },
            Done::CursorFreed { root, result } => match result {
                Ok(false) => {}
                Ok(true) => self.show_success(cursor_freed_text(&root)),
                Err(why) => {
                    tracing::warn!("opening {} to Cursor: {why}", root.display());
                    self.show_warning(format!("Cursor agents can't edit feature folders: {why}"));
                }
            },
            Done::Graphed {
                project,
                slug,
                title,
                built,
            } => self.feature_graphs_built(&project, &slug, &title, built),
            Done::Linked {
                project,
                slug,
                names,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Link);
                self.feature_linked(&project, &slug, &names, result);
            }
            Done::PrRead { reads } => self.pr_reviews_read(reads),
            Done::ReviewPosted {
                project,
                slug,
                results,
            } => self.review_posted(&project, &slug, results),
            Done::PrPushed {
                project,
                slug,
                results,
            } => self.pr_pushed(&project, &slug, results),
            Done::PushAccess {
                project,
                slug,
                results,
            } => self.push_access_read(&project, &slug, results),
            Done::CiRerun {
                project,
                slug,
                number,
                ran,
                failed,
            } => self.ci_rerun(&project, &slug, number, ran, failed),
            Done::PrMoved {
                project,
                slug,
                results,
            } => self.pr_head_moved(&project, &slug, results),
            Done::PrReview {
                project,
                slug,
                numbers,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Create);
                self.pr_review_made(project, slug, numbers, result);
            }
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Create);
                self.feature_created(project, title, slug, root, result);
            }
            Done::Touched {
                project,
                slug,
                repo,
                result,
            } => {
                self.features.busy.retain(|b| {
                    !(b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo))
                });
                self.features.forget_ended();
                self.feature_touched(&project, &slug, &repo, result);
            }
            Done::Losses {
                project,
                slug,
                losses,
            } => {
                self.unbusy(&project, &slug, Job::Losses);
                self.ask_remove_feature(project, slug, losses);
            }
            Done::RemovalLosses {
                project,
                slug,
                keys,
                to_context,
                result,
            } => {
                self.unbusy(&project, &slug, Job::RemoveRepos);
                self.ask_remove_repos(project, slug, keys, to_context, result);
            }
            Done::ReposRemoved {
                project,
                slug,
                to_context,
                results,
            } => {
                self.unbusy(&project, &slug, Job::RemoveRepos);
                self.feature_repos_removed(&project, &slug, to_context, results);
            }
            Done::Removed {
                project,
                slug,
                abandon,
                result,
                heads,
                tabs,
            } => {
                self.unbusy(&project, &slug, Job::Remove);
                self.feature_removed(&project, &slug, abandon, result, (heads, tabs));
            }
            Done::WorktreesRestored {
                project,
                slug,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Remove);
                self.worktrees_restored(&project, &slug, result);
            }
            Done::Probed {
                project,
                slug,
                probe,
            } => self.feature_probed(&project, &slug, *probe),
            Done::Tail { pane, lines } => self.features.feature_tail_read(&pane, lines),
            Done::CheckTail { pane, lines } => {
                if let Some(tail) = self.features.check_tails.get_mut(&pane) {
                    tail.read(&pane, lines);
                }
            }
            Done::T3Told { thread, result } => self.t3_told(&thread, result),
            Done::Read {
                project,
                slug,
                stage,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Read);
                self.feature_read(&project, &slug, stage, result);
            }
            Done::PlanChecked {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.plan_code_checked(&project, &slug, n, stamp, result),
            Done::ReviewRead {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.review_read(&project, &slug, n, stamp, result),
            Done::PlanReviewBaseline {
                project,
                slug,
                result,
            } => self.plan_review_baseline(&project, &slug, result),
            Done::PlanReviewRead {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.plan_review_read(&project, &slug, n, stamp, result),
            Done::Shipped {
                project,
                slug,
                patch,
                squash,
                pushed,
                result,
                ran,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                let patch = patch.as_deref();
                self.feature_shipped(&project, &slug, patch, &squash, pushed, ran, result);
            }
            Done::ConflictsBegun {
                project,
                slug,
                planning,
                begun,
                failed,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.conflicts_begun(&project, &slug, planning, begun, failed);
            }
            Done::ResolvingStopped {
                project,
                slug,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.resolving_stopped(&project, &slug, result);
            }
            Done::BaseUpdated {
                project,
                slug,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.base_updated(&project, &slug, *result);
            }
            Done::BasePulled {
                project,
                slug,
                repo,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.base_pulled(&project, &slug, &repo, result);
            }
            Done::ShipUndone {
                project,
                slug,
                undone,
                failed,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.ship_undone(&project, &slug, undone, failed);
            }
            Done::BasesPushed {
                project,
                slug,
                results,
                finishing,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.bases_pushed(&project, &slug, results, finishing);
            }
            Done::ShippedLosses {
                project,
                slug,
                losses,
            } => {
                self.unbusy(&project, &slug, Job::Losses);
                match losses.is_empty() {
                    true => {
                        let how = FeatureRemoval {
                            abandon: false,
                            force: false,
                        };
                        self.remove_feature(&project, &slug, how)
                    }
                    false => self.ask_remove_feature(project, slug, losses),
                }
            }
            Done::BuildChecked {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.build_code_checked(&project, &slug, n, stamp, result),
            Done::Progress {
                project,
                slug,
                job,
                what,
            } => {
                if let Some(busy) = self
                    .features
                    .busy
                    .iter_mut()
                    .find(|b| b.project == project && b.slug == slug && b.job == job)
                {
                    busy.what = what;
                }
            }
            Done::SetUp {
                project,
                slug,
                report,
            } => {
                self.unbusy(&project, &slug, Job::Setup);
                self.setup_done(&project, &slug, report);
            }
            Done::Verified {
                project,
                slug,
                n,
                evidence,
            } => {
                self.unbusy(&project, &slug, Job::Verify);
                self.verify_done(&project, &slug, n, evidence);
            }
            Done::ChecksBegun {
                project,
                slug,
                n,
                begun,
            } => self.checks_begun(&project, &slug, n, begun),
            Done::ChecksOpened {
                project,
                slug,
                n,
                panes,
            } => self.checks_opened(&project, &slug, n, panes),
            Done::SetupPane {
                project,
                slug,
                pane,
            } => {
                let run = self
                    .feature_mut(&project, &slug)
                    .and_then(|f| f.setup.as_mut());
                if let Some(run) = run {
                    run.pane = Some(pane);
                    self.save_feature(&project, &slug);
                }
            }
            Done::CheckEnded {
                project,
                slug,
                n,
                index,
                outcome,
            } => self.check_ended(&project, &slug, n, index, outcome),
            Done::CheckTurn {
                project,
                slug,
                n,
                index,
                turn,
            } => self.check_turn(&project, &slug, n, index, turn),
            Done::AiStats { scope, result } => self.ai_stats_read(scope, result),
            Done::AiWork(report) => self.ai_work_done(report),
        }
    }

    pub(crate) fn unbusy(&mut self, project: &str, slug: &str, job: Job) {
        self.features.unbusy(project, slug, job)
    }

    pub(crate) fn feature_busy(&self, project: &str, slug: &str) -> bool {
        self.features.feature_busy(project, slug)
    }

    pub(crate) fn busy_with(&self, project: &str, slug: &str, job: Job) -> Option<&Busy> {
        self.features.busy_with(project, slug, job)
    }

    pub(crate) fn feature_reading(&self, project: &str, slug: &str) -> Option<&Reading> {
        self.features.feature_reading(project, slug)
    }

    pub(crate) fn feature_docs(&self, project: &str, slug: &str) -> Option<&feature::page::Docs> {
        self.features.feature_docs(project, slug)
    }

    /// The checks `feature`'s Verify page lists (see `verify::plan_cards`).
    pub(super) fn planned_checks(&self, feature: &Feature) -> Vec<PlannedCheck> {
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        feature::verify::plan_cards(feature, &config)
    }

    /// What the panel's rows need of `feature` besides it.
    pub(crate) fn row_ctx(&self, feature: &Feature) -> RowCtx {
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        let checks = !feature::verify::checks(feature, &config).is_empty();
        let all_skipped = !checks && !feature::verify::skipped(feature, &config).is_empty();
        let criteria = self.criteria(&feature.project, &feature.slug).len();
        let manual = self.manual_counts(feature).unwrap_or_default();
        let key = (feature.project.clone(), feature.slug.clone());
        let last = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
        let docs = last.map(|p| &p.docs);
        let written = Stage::ALL.map(|stage| docs.is_some_and(|d| d.written.contains(&stage)));
        // The open diff's may not be saved yet. Its count means nothing once
        // it is closed: the file says, and a press looks again (`findings_go`).
        let open = (self.diff_open_of(&feature.project, &feature.slug))
            .then(|| self.code_reviews.get(&key))
            .flatten();
        let comments = (docs.map_or(0, |d| d.comments)).max(open.map_or(0, |c| c.comments));
        RowCtx {
            checks,
            criteria,
            manual,
            written,
            fixing: self.build_agent_fixing(feature),
            sandbox_busy: (self.features.build.sandbox_stops)
                .contains_key(&(feature.project.clone(), feature.slug.clone()))
                || (feature.writer.is_some()
                    && (feature.sandbox.as_ref()).is_some_and(|run| {
                        run.finished.is_none() && run.failed.is_none() && !run.stopped
                    })),
            all_skipped,
            ship_nothing: ship_planned(feature, last).is_some_and(|planned| planned.nothing),
            ship_left: ship_planned(feature, last).map_or(0, |planned| planned.steps()),
            ship_read: last
                .and_then(|p| p.ship.as_ref())
                .is_some_and(|c| !c.unread),
            // Anything holding it up, an agent's work on its conflicts too:
            // then the card's way out is the one primary.
            ship_blocked: ship_planned(feature, last).is_some_and(|p| !p.blockers.is_empty())
                || feature.shipping.resolving.is_some()
                || last.is_some_and(|p| p.bases.values().any(|b| !b.conflicts.is_empty())),
            checks_busy: self.checks_busy(&feature.project, &feature.slug),
            setting_up: self.setting_up(&feature.project, &feature.slug),
            feature_busy: self.feature_busy(&feature.project, &feature.slug),
            comments,
            lan_open: (self.features.build.sandbox_lan)
                .contains_key(&(feature.project.clone(), feature.slug.clone())),
            on_head: on_head(feature, last.map(|p| &p.reading)),
        }
    }
}
