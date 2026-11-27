//! `Gerdr`'s side of taking repositories out, shipping and removing a feature.

use super::*;

impl Gerdr {
    /// Whether repositories `keys` can leave feature `slug` now, whatever
    /// they hold; else says why not.
    pub(super) fn may_remove_repos(
        &mut self,
        project: &str,
        slug: &str,
        keys: &[String],
        to_context: bool,
    ) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            self.show_warning("That feature is gone".into());
            return false;
        };
        let title = &feature.title;
        let named = keys
            .iter()
            .map(|k| self.repo_label(feature, k))
            .collect::<Vec<_>>()
            .join(", ");
        let why = if self.feature_busy(project, slug) {
            Some(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ))
        } else if !linkable(feature) {
            Some(format!(
                "\u{201c}{title}\u{201d} has shipped; its repositories can't change now"
            ))
        } else if let Some(why) = layout::removal_refusal(feature, keys) {
            Some(match to_context {
                true => format!("Can't make {named} context only: {why}"),
                false => format!("Can't remove {named}: {why}"),
            })
        } else {
            let goes = |r: &feature::Repo| keys.contains(&r.key());
            // A link left with none takes its folder along.
            let folders = (feature.links.iter())
                .filter(|l| feature.repos_of(Some(&l.project)).all(goes))
                .map(|l| l.dir.as_path());
            let worktrees = (feature.repos.iter())
                .filter(|r| goes(r))
                .map(|r| r.worktree.as_path());
            folders.chain(worktrees).find_map(|dir| {
                let user = self.folder_in_use(dir)?;
                Some(format!(
                    "{} has {user} in it; close it first",
                    short_folder(dir)
                ))
            })
        };
        match why {
            Some(why) => {
                self.show_warning(why);
                false
            }
            None => true,
        }
    }

    /// Look at what taking repositories `keys` out of feature `slug` loses,
    /// off the thread, then ask (see `ask_remove_repos`).
    pub(crate) fn check_remove_repos(
        &mut self,
        project: String,
        slug: String,
        keys: Vec<String>,
        to_context: bool,
    ) {
        let Some(feature) = self.feature(&project, &slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::RemoveRepos,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        self.feature_work(Some(busy), move |_| Done::RemovalLosses {
            result: layout::removal_losses(&feature, &keys),
            project,
            slug,
            keys,
            to_context,
        });
    }

    /// Take repositories `keys` out of feature `slug`. One on the feature's
    /// branch always asks first: its branch goes, whatever it holds, and
    /// the question names what is lost with it. Others go at once when
    /// nothing is lost. Commits stop them, with a word on why. `to_context`:
    /// they stay on as context (see `remove_repos`).
    pub(super) fn ask_remove_repos(
        &mut self,
        project: String,
        slug: String,
        keys: Vec<String>,
        to_context: bool,
        result: Result<Vec<String>, String>,
    ) {
        let losses = match result {
            Ok(losses) => losses,
            Err(why) => return self.show_warning(format!("Can't remove it: {why}")),
        };
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        if !removal_asks(feature, &keys, &losses) {
            let how = RepoRemoval {
                force: false,
                to_context,
            };
            return self.remove_repos(&project, &slug, &keys, how);
        }
        let named: Vec<String> = (keys.iter()).map(|k| self.repo_label(feature, k)).collect();
        let named = named.join(", ");
        let (question, go) = match to_context {
            true => (format!("Make {named} context only?"), "Make context"),
            false => (
                format!("Remove {named} from \u{201c}{}\u{201d}?", feature.title),
                "Remove",
            ),
        };
        let repos = keys.len();
        let then = crate::confirm::Then::RemoveRepos {
            project,
            slug,
            keys,
            force: !losses.is_empty(),
            to_context,
        };
        self.ask_remove_worktrees(
            question,
            remove_repos_body(repos, &losses, to_context),
            go,
            then,
        );
    }

    /// Take repositories `keys` out of feature `slug` off the thread, as
    /// `how` says: their worktrees and branches go, and agents may go on
    /// reading their main checkouts.
    pub fn remove_repos(&mut self, project: &str, slug: &str, keys: &[String], how: RepoRemoval) {
        let RepoRemoval { force, to_context } = how;
        if !self.may_remove_repos(project, slug, keys, to_context) {
            return;
        }
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let named: Vec<String> = (keys.iter())
            .map(|k| self.repo_label(&feature, k))
            .collect();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::RemoveRepos,
            repo: None,
            what: format!(
                "Removing {} from \u{201c}{}\u{201d}",
                named.join(", "),
                feature.title
            ),
        };
        let graphs = self.running_feature_graphs(project, slug);
        let (project, slug, keys) = (project.to_owned(), slug.to_owned(), keys.to_vec());
        self.feature_work(Some(busy), move |_| {
            stop_graphs(graphs);
            Done::ReposRemoved {
                results: layout::remove_repos(&feature, &keys, force),
                project,
                slug,
                to_context,
            }
        });
    }

    /// Repositories were taken out of feature `slug`, or not: those gone
    /// leave it, with a link left with none, kept.
    pub(super) fn feature_repos_removed(
        &mut self,
        project: &str,
        slug: &str,
        to_context: bool,
        results: Vec<(String, Result<(), String>)>,
    ) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let mut gone = Vec::new();
        let mut named = Vec::new();
        let mut failed = Vec::new();
        for (key, result) in results {
            let label = self.repo_label(feature, &key);
            match result {
                Ok(()) => {
                    gone.push(key);
                    named.push(label);
                }
                Err(why) => {
                    tracing::warn!("removing {key} from feature {project}/{slug}: {why}");
                    failed.push(format!("{label}: {why}"));
                }
            }
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let auto = feature.auto_title();
        let now = feature::now();
        let title = feature.title.clone();
        let mut left_out = Vec::new();
        for key in &gone {
            let held = feature.repo(key).cloned().filter(|_| to_context);
            left_out.extend(feature.pull_of(key).map(|p| p.url.clone()));
            drop_repo(feature, key, now);
            match held {
                Some(repo) => {
                    feature.add_context(feature::ContextRepo {
                        name: repo.name,
                        main: repo.main,
                        project: repo.project,
                    });
                    feature.log(now, format!("made {key} context only"));
                }
                None => feature.log(now, format!("removed {key}")),
            }
        }
        if !gone.is_empty() {
            self.save_feature(project, slug);
            crate::pr_review::brief_drops(project, slug, &left_out);
            self.follow_auto_title(project, slug, auto);
            self.show_success(match to_context {
                true => context_text(&named.join(", ")),
                false => format!(
                    "\u{201c}{title}\u{201d} no longer holds {}",
                    named.join(", ")
                ),
            });
            self.rebuild_feature_graphs(project, slug);
        }
        if let Some(key) = gone.first() {
            self.features.select_repo(project, slug, key);
        }
        if !failed.is_empty() {
            self.show_error(format!("Couldn't remove {}", failed.join("; ")));
        }
        self.probe_feature(project, slug);
    }

    /// Look at what removing feature `slug`'s worktrees loses, off the
    /// thread, then ask (see `ask_remove_feature`).
    pub(super) fn check_remove_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        // Its own workspace closes with it; anything else in it refuses.
        if let Some(user) = self.feature_folder_in_use(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has {user} in it; close it first",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Losses,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::Losses {
            losses: layout::losses(&feature),
            project,
            slug,
        });
    }

    /// Ship feature `slug` of `project` as its last Ship check said, once
    /// the Ship sheet is confirmed: open its pull requests off the thread,
    /// then squash it, committed with `message` (its title and summary
    /// without one).
    pub fn ship_feature(&mut self, project: &str, slug: &str, message: Option<&str>) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let message = message.map_or_else(|| ship::squash_message(&feature), str::to_owned);
        let key = (project.to_owned(), slug.to_owned());
        let asked = self.features.asked.remove(&key).or_else(|| {
            let probe = self.features.probes.get(&key)?.last.as_ref();
            match ship_ask(&feature, probe) {
                ShipAsk::Ready(asked) => Some(asked),
                _ => None,
            }
        });
        let Some(Asked { actions, patch }) = asked else {
            return self.show_warning(format!("Nothing to ship in \u{201c}{title}\u{201d}"));
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        // A base waits for its squash; one merged already goes with the rest.
        let squashing: Vec<String> = (actions.iter())
            .filter_map(|a| match a {
                Action::Squash { repo, .. } => Some(repo.clone()),
                _ => None,
            })
            .collect();
        let (squash, prs): (Vec<Action>, Vec<Action>) =
            actions.into_iter().partition(|a| match a {
                Action::Squash { .. } => true,
                Action::PushBase { repo, .. } => squashing.contains(repo),
                _ => false,
            });
        let pushed: Vec<String> = (prs.iter())
            .filter_map(|a| match a {
                Action::Push { repo } => Some(repo.clone()),
                _ => None,
            })
            .collect();
        self.settle_ship_plan(project, slug);
        // Shipping again: what stopped it last time is past.
        self.set_ship_problem(project, slug, None);
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: None,
            what: format!("Shipping \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // The code is read again first: a commit since the Ship check
        // would ship what no gate saw.
        self.feature_work(Some(busy), move |_| {
            let (ran, failed) = match still_checked(&feature, patch.as_deref()) {
                Err(why) => (ship::Ran::default(), Some(why)),
                Ok(()) if prs.is_empty() => (ship::Ran::default(), None),
                Ok(()) => ship::run_steps(&feature, &prs, "gh", feature::now()),
            };
            let result = match failed {
                None => Ok(ran.prs),
                Some(why) => Err((why, ran.prs)),
            };
            Done::Shipped {
                result,
                ran: ran.done,
                project,
                slug,
                patch,
                squash,
                message,
                pushed,
            }
        });
    }

    /// Every step ticked of feature `slug` ran already, Ship not done yet:
    /// it is now, with nothing more to run.
    fn ship_all_ran(&mut self, project: &str, slug: &str) {
        self.settle_ship_plan(project, slug);
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.status != Status::Open || feature.current() != Some(Stage::Ship) {
            return;
        }
        let now = feature::now();
        feature.tick(now, "you", None);
        let mut text = format!(
            "\u{201c}{}\u{201d} is shipped: every step ticked ran",
            feature.title
        );
        feature.log(now, "Shipped: every step ticked ran");
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        if let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
        self.probe_feature(project, slug);
    }

    /// Squash feature `slug` (`squash`, its Squash steps) off the event
    /// loop, committed with `message`: done once its branches read as
    /// merged.
    fn start_squash(&mut self, project: &str, slug: &str, (squash, message): (&[Action], &str)) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let check = self.last_probe(project, slug).and_then(|p| p.ship.as_ref());
        let trees = ship::squash_trees(&feature, squash, check);
        if trees.is_empty() {
            tracing::warn!("shipping feature {project}/{slug}: nothing to squash into");
            let why = "none of its repositories is there to squash into".to_owned();
            self.set_ship_problem(project, slug, Some(why.clone()));
            return self.show_error(format!("Couldn't ship \u{201c}{title}\u{201d}: {why}"));
        }
        if let Some((_, dir)) = trees.iter().find(|(_, dir)| !dir.exists()) {
            let why = format!("{} is gone", short_folder(dir));
            self.set_ship_problem(project, slug, Some(why.clone()));
            return self.show_error(format!("Couldn't ship \u{201c}{title}\u{201d}: {why}"));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: None,
            what: format!("Squashing \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let (squash, message) = (squash.to_vec(), message.to_owned());
        self.feature_work(Some(busy), move |_| Done::Squashed {
            done: ship::squash(&feature, &trees, &message),
            project,
            slug,
            squash,
            message,
        });
    }

    /// The squash of feature `slug` (`squash`, its Squash steps) ran: all
    /// of it, and it is done once its branches read as merged, which the
    /// next read finds; or it stopped at a repository, those squashed
    /// before kept as done, and `message` kept as the next try's draft.
    pub(super) fn feature_squashed(
        &mut self,
        project: &str,
        slug: &str,
        (squash, message): (&[Action], String),
        done: ship::Squashed,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let now = feature::now();
        for (key, _) in &done.went {
            feature.log(now, format!("Squashed into the base of {key}"));
        }
        // A commit hook changed what went in: said, as the squash then
        // isn't what Verify and the reviews saw.
        let hooked: Vec<(String, bool)> = (done.hooked.iter())
            .map(|(key, h)| (hooked_text(key, h), h.layout))
            .collect();
        for (text, _) in &hooked {
            feature.log(now, text.clone());
        }
        for (text, layout) in hooked {
            let text = format!("\u{201c}{title}\u{201d}: {text}");
            match layout {
                true => self.show_notice(text),
                false => self.show_warning(text),
            }
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some((key, why)) = done.failed else {
            feature.shipping.squash_at = Some(now);
            feature.shipping.squash_repos = done.went.iter().map(|(key, _)| key.clone()).collect();
            feature.shipping.squash_shas = done.went.into_iter().collect();
            feature.shipping.push_base = (squash.iter())
                .filter_map(|a| match a {
                    Action::PushBase { repo, .. } => Some(repo.clone()),
                    _ => None,
                })
                .collect();
            self.save_feature(project, slug);
            return self.probe_feature(project, slug);
        };
        // Those squashed stay so: shipping again pushes their bases, if
        // ticked, and squashes the rest.
        for (repo, sha) in done.went {
            feature.ship_ran(feature::ShipDone {
                at: now,
                repo,
                step: ShipStep::Merge,
                sha: Some(sha),
                url: None,
                undone: None,
            });
        }
        let text = format!("Couldn't squash \u{201c}{title}\u{201d} into the base of {key}: {why}");
        feature.log(now, text.clone());
        feature.shipping.problem = Some(feature::ShipProblem {
            at: now,
            why: format!("Couldn't squash into the base of {key}: {why}"),
        });
        self.save_feature(project, slug);
        self.keep_squash_draft(project, slug, "", message);
        tracing::warn!("shipping feature {project}/{slug}: {text}");
        self.ship_failed(project, slug, text);
        self.probe_feature(project, slug);
    }

    pub(crate) fn last_probe(&self, project: &str, slug: &str) -> Option<&Probe> {
        self.features.last_probe(project, slug)
    }

    /// Ship was asked for on feature `slug`: say what stops it, or ask
    /// with what shipping does, as its latest Ship check has it. With none
    /// yet, as just after it moved here, it asks once there is one.
    pub(super) fn ask_ship_feature(&mut self, project: &str, slug: &str) {
        self.ask_ship_checked(project, slug, false);
    }

    /// `ask_ship_feature`, `waited` for the check it lacked.
    pub(super) fn ask_ship_checked(&mut self, project: &str, slug: &str, waited: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let (title, branch) = (feature.title.clone(), feature.branch());
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let key = (project.to_owned(), slug.to_owned());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        match ship_ask(&feature, probe) {
            ShipAsk::Checking if waited => self.show_warning(format!(
                "Couldn't check \u{201c}{title}\u{201d}; try again in a moment"
            )),
            ShipAsk::Checking => {
                self.features.ship_waits.insert(key);
                self.show_notice(format!(
                    "Checking \u{201c}{title}\u{201d}: Ship asks once that is done"
                ));
                self.probe_feature(project, slug);
            }
            // Held up only by a base to pull: that is offered.
            ShipAsk::Blocked(why) => match pull_first_of(&feature, probe) {
                Some((repo, base, said)) => {
                    let then = crate::confirm::Then::PullBase {
                        project: project.to_owned(),
                        slug: slug.to_owned(),
                        repo: repo.clone(),
                    };
                    let body = format!("{said}\nIt moves on to origin/{base}, only fast-forward");
                    self.ask(format!("Pull {base} of {repo} first?"), body, "Pull", then);
                }
                None => self.show_warning(why),
            },
            ShipAsk::Finish(dirty) => {
                let check = probe.and_then(|p| p.ship.as_ref());
                let removing = feature.ship_plan.remove_worktrees
                    && check.is_some_and(|c| ship::removable(&feature, c));
                self.ask_finish(
                    project.to_owned(),
                    slug.to_owned(),
                    (&title, &branch),
                    (feature.repos.len(), removing),
                    &dirty,
                    (!removing).then(|| self.ending_said(project, slug)),
                );
            }
            ShipAsk::AllRan if feature.status == Status::Done => self.show_notice(format!(
                "Every step ticked of \u{201c}{title}\u{201d} ran already"
            )),
            ShipAsk::AllRan => self.ship_all_ran(project, slug),
            ShipAsk::Ready(asked) => {
                let mut repos: Vec<&str> = (asked.actions.iter())
                    .map(ship::action_repo)
                    .filter(|r| !r.is_empty())
                    .collect();
                repos.dedup();
                let one = repos.len() <= 1;
                let mut steps: Vec<String> = (asked.actions.iter())
                    .map(|a| a.label(&branch, one))
                    .collect();
                let check = probe.and_then(|p| p.ship.as_ref());
                let removing = check.is_some_and(|c| ship::removable(&feature, c));
                let running = self.ending_running(project, slug);
                steps.extend(ship_after_line(&feature, removing, &running));
                let squashes = (asked.actions.iter()).any(|a| matches!(a, Action::Squash { .. }));
                self.features.asked.insert(key, asked);
                if !squashes {
                    return self.ask_ship(
                        project.to_owned(),
                        slug.to_owned(),
                        &title,
                        &branch,
                        &steps,
                    );
                }
                // Its commit message is asked with it: git takes it from there.
                let (heading, about) = crate::confirm::ship_question(&title, &branch, &steps);
                let message = ship::squash_message(&feature);
                let ship = crate::finding_dialog::Squash::Ship;
                self.ask_squash_message(project, slug, "", heading, about, message, ship);
            }
        }
    }

    /// The pull requests of feature `slug` are open, or some are and the
    /// rest failed: they are kept, and once all are open, Ship is done
    /// (on the code whose patch-id is `patch`), unless a squash goes on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn feature_shipped(
        &mut self,
        project: &str,
        slug: &str,
        patch: Option<&str>,
        squash: (&[Action], &str),
        pushed: Vec<String>,
        ran: Vec<feature::ShipDone>,
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
    ) {
        // What ran is kept, however it ended.
        if let Some(feature) = self.feature_mut(project, slug)
            && !ran.is_empty()
        {
            for done in &ran {
                feature.ship_ran(done.clone());
            }
            self.save_feature(project, slug);
        }
        let bases: Vec<String> = (ran.iter())
            .filter(|d| d.step == ShipStep::PushBase)
            .map(|d| d.repo.clone())
            .collect();
        if result.is_ok() && !squash.0.is_empty() {
            self.start_squash(project, slug, squash);
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let now = feature::now();
        let (prs, failed) = match result {
            Ok(prs) => (prs, None),
            Err((why, prs)) => (prs, Some(why)),
        };
        let n = prs.len();
        // Those it had open already, the push updated.
        let updated = prs
            .iter()
            .filter(|pr| feature.shipping.prs.contains(pr))
            .count();
        for pr in prs {
            match feature.shipping.prs.iter_mut().find(|p| p.repo == pr.repo) {
                Some(old) => *old = pr,
                None => feature.shipping.prs.push(pr),
            }
        }
        if let Some(why) = failed {
            feature.log(now, format!("Shipping stopped: {why}"));
            feature.shipping.problem = Some(feature::ShipProblem {
                at: now,
                why: why.clone(),
            });
            self.save_feature(project, slug);
            tracing::warn!("shipping feature {project}/{slug}: {why}");
            let text = format!("Couldn't ship \u{201c}{title}\u{201d}: {why}");
            self.ship_failed(project, slug, text);
            return self.probe_feature(project, slug);
        }
        // Only a base merged already: it went.
        if n == 0 && pushed.is_empty() && !bases.is_empty() {
            let text = format!(
                "Pushed the base of {} for \u{201c}{title}\u{201d}",
                bases.join(", ")
            );
            feature.log(now, text.clone());
            self.save_feature(project, slug);
            self.show_success(text);
            return self.probe_feature(project, slug);
        }
        // Only a squash: it says so itself, once merged.
        if n == 0 && pushed.is_empty() {
            return self.probe_feature(project, slug);
        }
        let mut text = match n {
            0 => format!("Pushed {} for \u{201c}{title}\u{201d}", pushed.join(", ")),
            _ => opened_text(&title, n - updated, updated),
        };
        // Those pushed before stay pushed.
        for key in pushed {
            if !feature.shipping.pushed.contains(&key) {
                feature.shipping.pushed.push(key);
            }
        }
        feature.log(now, text.clone());
        let done = squash.0.is_empty()
            && feature.status == Status::Open
            && feature.current() == Some(Stage::Ship);
        if done {
            feature.tick(now, "gerdr", patch);
        }
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        if done && let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
        self.probe_feature(project, slug);
    }

    /// Feature `slug` is shipped or finished, whichever way: what it runs
    /// stops, its sandbox, checks and agents (here and on other machines).
    /// Then its worktrees go, or its workspace closes, as its ticks say; kept
    /// open, every tab but the feature tab closes. What keeps running, if
    /// anything: a T3 Code thread, which gerdr can't stop.
    pub(crate) fn after_ship(&mut self, project: &str, slug: &str) -> Option<String> {
        self.keep_memory(project, slug);
        let feature = self.feature(project, slug)?;
        let plan = feature.ship_plan.clone();
        if feature.sandbox.is_some() {
            self.finish_stop(project, slug, crate::feature_sandbox::StopFor::Remove);
        }
        crate::launch::remove_sandbox_profiles(project, slug);
        self.halt_checks(project, slug, None);
        if let Some(mut feature) = self.feature(project, slug).cloned() {
            // Its sandbox went its own way just now.
            feature.sandbox = None;
            self.close_machine_panes_of(&feature);
        }
        // Its worktrees removed, its T3 Code threads go too, stopped.
        let stages: Vec<Stage> = (self.feature(project, slug))
            .filter(|_| !plan.remove_worktrees)
            .map_or_else(Vec::new, |f| f.runs.keys().copied().collect());
        let t3: Vec<String> = (stages.into_iter())
            .filter_map(|stage| self.t3_left_running(project, slug, stage))
            .collect();
        if plan.remove_worktrees {
            self.check_removing_after_ship(project, slug);
        } else if plan.keep_workspace {
            self.close_feature_tabs(project, slug);
        } else {
            self.close_shipped_now(project, slug);
        }
        self.retro_after_ship(project, slug, plan.remove_worktrees);
        (!t3.is_empty()).then(|| t3.join("; "))
    }

    /// Shipped multi-repository feature `slug`'s memory that spans its
    /// repositories goes into the project's home folder, off the UI thread
    /// (`feature::memory`). Not for a feature finished without shipping.
    fn keep_memory(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let unshipped = (feature.marks.get(&Stage::Ship))
            .is_some_and(|m| m.note == super::ship_page::FINISHED_UNSHIPPED);
        let data = feature::store::dir(None, project, slug);
        if !feature.multi || unshipped || !data.join(feature::memory::DIR).is_dir() {
            return;
        }
        // Read now: the worktrees may go before the work runs.
        let repos: Vec<String> = (feature.touched())
            .filter(|r| r.project.is_none() && r.worktree.join("GLOSSARY.md").is_file())
            .map(|r| r.name.clone())
            .collect();
        let root = feature.root.clone();
        let title = self.projects.names(feature).home;
        let message = format!("Project memory from feature {}", feature.title);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::MemoryKept {
            result: feature::memory::apply(&root, &data, &title, &repos, &message),
            project,
            slug,
        });
    }

    /// The tick removing the worktrees holds only as long as what is ticked
    /// merges everything: settled before shipping, as a route may have
    /// changed since it was ticked.
    fn settle_ship_plan(&mut self, project: &str, slug: &str) {
        let key = (project.to_owned(), slug.to_owned());
        let check = (self.features.probes.get(&key))
            .and_then(|p| p.last.as_ref())
            .and_then(|p| p.ship.clone());
        let Some(check) = check else {
            return;
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.ship_plan.remove_worktrees && !ship::removable(feature, &check) {
            feature.ship_plan.remove_worktrees = false;
            self.save_feature(project, slug);
        }
    }

    /// Feature `slug`'s tick was to remove its worktrees: with nothing to
    /// lose they go at once, as asked; else it asks, naming what would.
    fn check_removing_after_ship(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if let Some(user) = self.feature_folder_in_use(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has {user} in it; close it, then remove its worktrees",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Losses,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::ShippedLosses {
            losses: layout::losses(&feature),
            project,
            slug,
        });
    }

    /// Finish feature `slug` with nothing shipped, once asked: Ship is done,
    /// its branch and worktrees stay.
    pub fn finish_unshipped(&mut self, project: &str, slug: &str) {
        if !self.features.ship_plan_open(&mut ui!(self), project, slug) {
            return;
        }
        self.settle_ship_plan(project, slug);
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.status != Status::Open || feature.current() != Some(Stage::Ship) {
            return;
        }
        let now = feature::now();
        feature.tick(now, "you", None);
        if let Some(mark) = feature.marks.get_mut(&Stage::Ship) {
            mark.note = FINISHED_UNSHIPPED.to_owned();
        }
        feature.log(now, FINISHED_UNSHIPPED);
        let mut text = format!(
            "\u{201c}{}\u{201d} finished without shipping",
            feature.title
        );
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        if let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
        self.probe_feature(project, slug);
    }

    /// Send repository `repo` of feature `slug` the way `route` says.
    pub(super) fn set_ship_route(&mut self, project: &str, slug: &str, repo: &str, route: Route) {
        if !self.features.ship_plan_open(&mut ui!(self), project, slug) {
            return;
        }
        if let Some(plan) = self.features.ship_repo_plan(project, slug, repo) {
            plan.route = route;
            self.save_feature(project, slug);
        }
    }

    /// Tick pushing the base of repository `repo` of feature `slug`, merged
    /// here only, then ask to ship it as any tick is.
    pub(super) fn push_merged_base(&mut self, project: &str, slug: &str, repo: &str) {
        let ticked = (self.feature(project, slug))
            .and_then(|f| f.ship_plan.repos.get(repo))
            .is_some_and(|plan| plan.ticked(ShipStep::PushBase));
        if !ticked {
            self.tick_ship_step(project, slug, repo, ShipStep::PushBase);
        }
        self.ask_ship_feature(project, slug);
    }

    /// Flip tick `step` of repository `repo` of feature `slug`.
    pub(super) fn tick_ship_step(&mut self, project: &str, slug: &str, repo: &str, step: ShipStep) {
        if !self.features.ship_plan_open(&mut ui!(self), project, slug) {
            return;
        }
        if let Some(plan) = self.features.ship_repo_plan(project, slug, repo) {
            plan.toggle(step);
            self.save_feature(project, slug);
        }
    }

    /// Flip a tick of what follows shipping feature `slug`.
    pub(super) fn tick_ship_after(&mut self, project: &str, slug: &str, after: ShipAfter) {
        if !self.features.ship_plan_open(&mut ui!(self), project, slug) {
            return;
        }
        let key = (project.to_owned(), slug.to_owned());
        let check = (self.features.probes.get(&key))
            .and_then(|p| p.last.as_ref())
            .and_then(|p| p.ship.clone());
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let removable = check.is_some_and(|c| ship::removable(feature, &c));
        let removing = removable && feature.ship_plan.remove_worktrees;
        match after {
            ShipAfter::Close if removing => {
                return self.show_warning(REMOVING_CLOSES.into());
            }
            ShipAfter::Remove if !removable => {
                return self.show_warning(ONLY_ONCE_MERGING.into());
            }
            _ => {}
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        match after {
            ShipAfter::Close => feature.ship_plan.keep_workspace ^= true,
            ShipAfter::Remove => feature.ship_plan.remove_worktrees = !removing,
        }
        self.save_feature(project, slug);
    }

    /// Ask before removing feature `slug`'s worktrees, naming what goes
    /// with them.
    pub(super) fn ask_remove_feature(
        &mut self,
        project: String,
        slug: String,
        losses: Vec<String>,
    ) {
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let title = feature.title.clone();
        let abandon = feature.status != Status::Done;
        let what = worktrees_words(feature);
        let (question, go) = if abandon {
            (format!("Abandon \u{201c}{title}\u{201d}?"), "Abandon")
        } else {
            (
                format!("Remove the {what} of \u{201c}{title}\u{201d}?"),
                "Remove",
            )
        };
        // Each on a line of its own: the dialog cuts a line to its width.
        // Abandoned, what they have is kept to come back with it.
        let mut lines = match abandon {
            true => vec![format!("Its {what} go; commits and changes are kept")],
            false => vec![removal_body(what, &losses)],
        };
        if let Some(w) = self.feature_workspace(&project, &slug) {
            let running = self.workspace_running(w);
            lines.push(crate::feature_workspace::closing_text(&running));
        }
        lines.push("Restore brings it back: its toast, or Find and switch".to_owned());
        let body = format!("{}.", lines.join(".\n"));
        let force = !losses.is_empty();
        let then = crate::confirm::Then::RemoveFeature {
            project,
            slug,
            abandon,
            force,
        };
        self.ask_remove_worktrees(question, body, go, then);
    }

    /// Remove feature `slug`'s worktrees off the thread as `how` says: with
    /// what they lose or not, then marking it abandoned or not.
    pub fn remove_feature(&mut self, project: &str, slug: &str, how: FeatureRemoval) {
        let FeatureRemoval { abandon, force } = how;
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Remove,
            repo: None,
            what: format!(
                "Removing the worktrees of \u{201c}{}\u{201d}",
                feature.title
            ),
        };
        // Its sandbox goes with it: nothing else would ever stop it.
        let sandbox = self
            .feature_mut(project, slug)
            .and_then(|f| f.sandbox.take());
        if sandbox.is_some() {
            self.save_feature(project, slug);
        }
        let removal = sandbox
            .as_ref()
            .map(|run| self.sandbox_removal(&feature, run));
        // What its worktrees left outside them goes with them (`workflow.cleanup`).
        let cleanup = self.projects.workflows(&feature, |w| w.cleanup.clone());
        let venv = self.feature_venv(project, slug);
        let cleanup_log =
            crate::feature::setup::cleanup_log_path(&feature::store::dir(None, project, slug));
        let shipped = feature.status == crate::feature::Status::Done;
        self.t3_forget_project(&feature.dir, shipped);
        // A shipped feature's threads go with its project, stopped.
        let stages = if shipped {
            Vec::new()
        } else {
            feature.runs.keys().copied().collect()
        };
        for stage in stages {
            if let Some(left) = self.t3_left_running(project, slug, stage) {
                self.show_warning(left);
            }
        }
        // Its agents on other machines go with its tabs.
        self.close_machine_panes_of(&feature);
        // It can come back as it was, abandoned too: with these.
        let tabs =
            (self.feature_workspace(project, slug)).map_or_else(Vec::new, |w| self.kept_tabs(w));
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // Its workspace closes first, its terminals gone before its
        // worktrees go.
        let (p, s) = (project.clone(), slug.clone());
        self.close_feature_workspace_then(&p, &s, move |gerdr| {
            let graphs = gerdr.running_feature_graphs(&project, &slug);
            gerdr.feature_work(Some(busy), move |_| {
                stop_graphs(graphs);
                if let Some(removal) = &removal {
                    removal.go();
                }
                // Only where the worktrees go: a removal refused for what it
                // would lose leaves them as they are.
                let clean = || match force || layout::losses(&feature).is_empty() {
                    true => crate::feature::setup::cleanup(&feature, &cleanup, &venv, &cleanup_log),
                    false => Vec::new(),
                };
                let mut cleaned = Vec::new();
                // Abandoned, all it has is kept first: commits, changes.
                let (heads, result) = match abandon {
                    true => match layout::keep(&feature) {
                        Ok(heads) => {
                            cleaned = clean();
                            let result = layout::remove(&feature, force);
                            // Stopped on the way: what stays has its changes back.
                            if result.is_err() {
                                layout::unkeep(&feature, &heads);
                            }
                            (heads, result)
                        }
                        Err(why) => (Vec::new(), Err(format!("nothing went: {why}"))),
                    },
                    false => {
                        cleaned = clean();
                        (layout::heads(&feature), layout::remove(&feature, force))
                    }
                };
                Done::Removed {
                    result,
                    project,
                    slug,
                    abandon,
                    heads,
                    tabs,
                    cleanup: cleaned,
                }
            });
        });
    }

    /// `workflow.cleanup` failed for feature `slug` as `failed` says: what
    /// its worktrees left outside them waits for the user, and the log
    /// says what its commands printed.
    fn cleanup_failed(&mut self, project: &str, slug: &str, failed: &str) {
        tracing::warn!("cleaning up after feature {project}/{slug}: {failed}");
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.log(feature::now(), format!("cleanup failed: {failed}"));
        let title = feature.title.clone();
        self.save_feature(project, slug);
        let log =
            crate::feature::setup::cleanup_log_path(&feature::store::dir(None, project, slug));
        self.show_warning(format!(
            "Cleanup of \u{201c}{title}\u{201d} failed: {failed}; its output is in {}",
            crate::paths::display(&log, crate::home_dir().as_deref(), 60)
        ));
    }

    pub(super) fn feature_removed(
        &mut self,
        project: &str,
        slug: &str,
        abandon: bool,
        result: Result<(), String>,
        (heads, tabs): (Vec<feature::KeptHead>, Vec<serde_json::Value>),
        cleanup: Vec<String>,
    ) {
        if !cleanup.is_empty() {
            self.cleanup_failed(project, slug, &cleanup.join("; "));
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("removing the worktrees of feature {project}/{slug}: {why}");
            return self.show_error(format!(
                "Couldn't remove the worktrees of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        let now = feature::now();
        forget_problems_of(&title);
        if abandon {
            feature.status = Status::Abandoned;
            feature.log(now, "abandoned");
            crate::launch::remove_sandbox_profiles(project, slug);
        } else {
            feature.log(now, "worktrees removed");
        }
        self.save_feature(project, slug);
        if abandon || !heads.is_empty() {
            self.note_after_ship(project, slug, feature::AfterStep::Remove, tabs, heads);
        }
        // Abandoned, it closed as it was saved.
        if !abandon {
            self.ai_work_close(project, slug);
        }
        let text = match abandon {
            true => format!("Abandoned \u{201c}{title}\u{201d}"),
            false => format!("Removed the worktrees of \u{201c}{title}\u{201d}"),
        };
        self.show_removed(project, slug, text, abandon);
        if let Some(feature) = self.feature(project, slug).cloned() {
            self.forget_on_machines(&feature);
        }
    }
}

/// What the feature's log and a toast say of a commit hook changing the
/// squash into repository `key`'s base, as `hooked` found.
fn hooked_text(key: &str, hooked: &ship::Hooked) -> String {
    let n = match hooked.files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    match hooked.layout {
        true => format!(
            "A commit hook reformatted {n} of the squash into {key}: only spacing, line breaks, commas and semicolons"
        ),
        false => format!(
            "A commit hook changed {n} of the squash into {key} beyond layout: what went in isn't what was checked; look at {}",
            hooked.files.first().map_or("", String::as_str)
        ),
    }
}
