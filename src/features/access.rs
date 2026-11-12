//! Reading and changing `Features`: the features, their busy work, probes and panel.

use super::*;

impl Features {
    /// Let go of the start times of work no longer running.
    pub(super) fn forget_ended(&mut self) {
        let busy = &self.busy;
        self.since.retain(|(project, slug, job), _| {
            (busy.iter()).any(|b| &b.project == project && &b.slug == slug && b.job == *job)
        });
    }

    /// Feature `slug` of project `project`, whichever machine writes it.
    pub(crate) fn feature(&self, project: &str, slug: &str) -> Option<&Feature> {
        let own = self.all.get(project).into_iter().flatten();
        let viewed = self.viewed.get(project).into_iter().flatten();
        own.chain(viewed).find(|f| f.slug == slug)
    }

    /// Feature `slug` of project `project`, to change: only this machine's own
    /// can be.
    pub(crate) fn feature_mut(&mut self, project: &str, slug: &str) -> Option<&mut Feature> {
        self.all
            .get_mut(project)?
            .iter_mut()
            .find(|f| f.slug == slug)
    }

    /// What the read of feature `slug` found of repositories built on another
    /// branch than their base, `sits` by `Repo::key`, where `was` is what the
    /// read before found: kept as a problem, and a repository newly found so
    /// gets a warning with a button to make that branch its base.
    pub(crate) fn feature_bases_read(
        &mut self,
        ui: &mut crate::toast::Ui,
        project: &str,
        slug: &str,
        was: &BTreeMap<String, (String, String)>,
        sits: &BTreeMap<String, (String, String)>,
    ) {
        let at = format!("{project}/{slug}");
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let mut lines = Vec::new();
        let mut fresh = Vec::new();
        for (key, (branch, oid)) in sits {
            let Some(repo) = feature.repo(key) else {
                continue;
            };
            let text = format!(
                "{key} of \u{201c}{title}\u{201d} is built on {branch}, not on its base {}",
                repo.base
            );
            // The branch moving on isn't news.
            if was.get(key).is_none_or(|(b, _)| b != branch) {
                fresh.push((key.clone(), branch.clone(), oid.clone(), text.clone()));
            }
            lines.push(text);
        }
        if lines.is_empty() {
            return crate::problems::clear_at(BASE_PROBLEM, &at);
        }
        crate::problems::report_at(BASE_PROBLEM, &at, lines.join("; "));
        for (repo, branch, oid, text) in fresh {
            let then = MenuAction::SetFeatureBase {
                project: project.to_owned(),
                slug: slug.to_owned(),
                repo,
                to: (branch, oid),
                undoing: false,
            };
            ui.toast_with(text, Level::Warning, Some(("Use as base".into(), then)));
        }
    }

    /// The chats at work now, on every feature this machine writes: the
    /// AI work keeps them again while they last.
    pub(crate) fn ai_work_chats(&self) -> Vec<(String, String, ai_work::Chat)> {
        let mut chats = Vec::new();
        for (project, features) in &self.all {
            for feature in features.iter().filter(|f| f.status == Status::Open) {
                let live = ai_work::observe::runs_of(feature).filter(|(_, run)| run.live());
                for (stage, run) in live {
                    let chat = ai_work::observe::chat(feature, stage, run, run.session());
                    chats.push((project.clone(), feature.slug.clone(), chat));
                }
            }
        }
        chats
    }

    /// `job` on feature `slug` of `project` is over.
    pub(crate) fn unbusy(&mut self, project: &str, slug: &str, job: Job) {
        self.busy
            .retain(|b| !(b.project == project && b.slug == slug && b.job == job));
        self.forget_ended();
    }

    /// Whether feature `slug` of `project` has work running off the thread
    /// that other work on it must wait for.
    pub(crate) fn feature_busy(&self, project: &str, slug: &str) -> bool {
        self.busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.job != Job::Graph)
    }

    /// Feature `slug`'s work of kind `job` running off the thread.
    pub(crate) fn busy_with(&self, project: &str, slug: &str, job: Job) -> Option<&Busy> {
        self.busy
            .iter()
            .find(|b| b.project == project && b.slug == slug && b.job == job)
    }

    /// What the latest probe of feature `slug` of `project` read of its
    /// code, if it could.
    pub(crate) fn feature_reading(&self, project: &str, slug: &str) -> Option<&Reading> {
        let key = (project.to_owned(), slug.to_owned());
        let probe = self.probes.get(&key)?.last.as_ref()?;
        probe.reading.as_ref().ok()
    }

    /// What the latest probe of feature `slug` of `project` found of its
    /// files, if one is back.
    pub(crate) fn feature_docs(&self, project: &str, slug: &str) -> Option<&feature::page::Docs> {
        let key = (project.to_owned(), slug.to_owned());
        Some(&self.probes.get(&key)?.last.as_ref()?.docs)
    }

    /// A read of pane `pane`'s tail is back. One for a pane no longer the
    /// tail's (the agent or the tab changed meanwhile) is dropped: the
    /// tail of the new one is read on its own.
    pub(crate) fn feature_tail_read(&mut self, pane: &str, lines: Result<Vec<String>, String>) {
        let Some(tail) = self.tail.as_mut().filter(|t| t.pane == pane) else {
            return;
        };
        tail.read(pane, lines);
    }

    /// The tasks running on features, for the tasks panel.
    pub(crate) fn feature_tasks(&self, projects: &projects::Store) -> Vec<Task> {
        self.busy
            .iter()
            .map(|b| {
                let project = projects
                    .project(&b.project)
                    .map_or(b.project.as_str(), |p| p.name.as_str());
                let title = self.feature(&b.project, &b.slug).map(|f| f.title.as_str());
                task_of(b, title, project)
            })
            .collect()
    }

    /// The repository `key` as the feature panel's sources found it: its
    /// source, the root holding it, and where it is.
    pub(crate) fn offered_repo(&self, key: &str) -> Option<(RepoSource, Found)> {
        let panel = self.panel.as_ref()?;
        panel.sources.iter().find_map(|source| {
            let Finding::Found(root) = &source.finding else {
                return None;
            };
            let found = (root.repos.iter())
                .find(|r| feature::repo_key(source.project.as_deref(), &r.name) == key)?;
            Some((source.clone(), found.clone()))
        })
    }

    /// Why feature `slug` can't change its repositories now, said; `false`
    /// when it can.
    pub(crate) fn repos_locked(
        &mut self,
        ui: &mut crate::toast::Ui,
        project: &str,
        slug: &str,
    ) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            ui.show_warning("That feature is gone".into());
            return true;
        };
        let title = feature.title.clone();
        if !linkable(feature) {
            ui.show_warning(format!(
                "\u{201c}{title}\u{201d} has shipped; its repositories can't change now"
            ));
            return true;
        }
        if self.feature_busy(project, slug) {
            ui.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
            return true;
        }
        false
    }

    /// The row of repository `key` takes the keyboard on the panel of
    /// feature `slug`, wherever it went: it moved group.
    pub(crate) fn select_repo(&mut self, project: &str, slug: &str, key: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let open = (self.panel.as_mut()).filter(|p| p.project == project && p.slug == slug);
        if let Some(panel) = open {
            panel.select_repo(&feature, key);
        }
    }

    pub(crate) fn new_feature_view(&self, projects: &projects::Store) -> Option<NewFeatureView> {
        let dialog = self.dialog.as_ref()?;
        let project = projects
            .project(&dialog.project)
            .map_or_else(|| dialog.project.clone(), |p| p.name.clone());
        Some(new_feature_view(dialog, project))
    }

    /// Hold the feature tab's Page scroll for the page it shows now (see
    /// `FeaturePanel::sync_page`).
    pub(crate) fn sync_feature_page(&mut self) {
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        let Some(key) = (self.feature(&panel.project, &panel.slug)).map(|f| panel.page_key(f))
        else {
            return;
        };
        if let Some(panel) = self.panel.as_mut() {
            panel.hold_page(key);
        }
    }

    /// The feature the panel is on.
    pub(crate) fn panel_feature(&self) -> Option<&Feature> {
        let panel = self.panel.as_ref()?;
        self.feature(&panel.project, &panel.slug)
    }

    /// Show the current stage's page of feature `slug`, when the panel
    /// shows it: it moved on.
    pub(crate) fn feature_cursor_to_current_of(&mut self, project: &str, slug: &str) {
        let on_it = |p: &&mut FeaturePanel| p.project == project && p.slug == slug;
        if let Some(panel) = self.panel.as_mut().filter(on_it) {
            panel.unpick();
        }
    }

    /// The latest read of feature `slug` of `project`, if one came back.
    pub(crate) fn last_probe(&self, project: &str, slug: &str) -> Option<&Probe> {
        let key = (project.to_owned(), slug.to_owned());
        self.probes.get(&key)?.last.as_ref()
    }

    /// Whether the Ship page's ticks of feature `slug` can change now; else
    /// says why not.
    pub(crate) fn ship_plan_open(
        &mut self,
        ui: &mut crate::toast::Ui,
        project: &str,
        slug: &str,
    ) -> bool {
        if !self.feature_busy(project, slug) {
            return true;
        }
        let title = (self.feature(project, slug)).map_or(String::new(), |f| f.title.clone());
        ui.show_warning(format!(
            "\u{201c}{title}\u{201d} is busy; try again once it is done"
        ));
        false
    }

    /// The plan of repository `repo` of feature `slug`, kept from now on, with
    /// the route it has now if it had no plan yet.
    pub(crate) fn ship_repo_plan(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
    ) -> Option<&mut feature::RepoPlan> {
        let key = (project.to_owned(), slug.to_owned());
        let default = (self.probes.get(&key))
            .and_then(|p| p.last.as_ref())
            .and_then(|p| p.ship.as_ref())
            .and_then(|c| c.repos.iter().find(|r| r.key == repo))?
            .route;
        let feature = self.feature_mut(project, slug)?;
        let plans = &mut feature.ship_plan.repos;
        Some(
            plans
                .entry(repo.to_owned())
                .or_insert_with(|| feature::RepoPlan::new(default)),
        )
    }

    /// Row `row` of the Ship page takes the keyboard, and is what a click
    /// on it acts on.
    pub(crate) fn ship_row_at(&mut self, feature: &Feature, row: usize) -> Option<ShipRow> {
        let key = (feature.project.clone(), feature.slug.clone());
        let probe = (self.probes.get(&key)).and_then(|p| p.last.as_ref());
        let at = ship_rows(feature, probe).get(row).cloned()?;
        if let Some(panel) = self.panel.as_mut() {
            panel.focus = TabArea::Findings;
            panel.finding = row;
        }
        Some(at)
    }

    /// The cards of the review stage's page in the panel (AI review's, Plan
    /// review's), with what `visit` makes of them and the panel's selection.
    pub(crate) fn with_review_cards<R>(
        &self,
        visit: impl FnOnce(&[ReviewCard], usize) -> R,
    ) -> Option<R> {
        let panel = self.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let key = (panel.project.clone(), panel.slug.clone());
        let probe = (self.probes.get(&key)).and_then(|p| p.last.as_ref());
        let stage = panel.page_stage(feature);
        let cards = review_cards(feature, stage, notes_of(probe));
        Some(visit(&cards, panel.finding))
    }

    /// The comments feature `slug`'s latest Code review round took, as its page
    /// has them, to show in its diff.
    pub(crate) fn sent_notes(&self, project: &str, slug: &str) -> Vec<Comment> {
        let key = (project.to_owned(), slug.to_owned());
        let made = self
            .feature(project, slug)
            .and_then(|f| f.comment_fixes.last());
        (self.probes.get(&key).and_then(|p| p.last.as_ref()))
            .map_or(&[][..], |p| p.docs.notes.as_slice())
            .iter()
            .filter(|c| c.flag.is_some())
            .cloned()
            .map(|mut c| {
                if let (Some(flag), Some(fix)) = (c.flag.as_mut(), made) {
                    flag.made = fix.made.contains(&flag.id);
                }
                let feature = self.feature(project, slug);
                if let Some(flag) = c.flag.as_mut() {
                    flag.planning = feature::fix_round::planning(
                        feature.and_then(|f| f.fix_round.as_ref()),
                        Stage::CodeReview,
                    );
                    // Where its round left the code: the fix is there.
                    let round = (feature.and_then(|f| f.last_fix_round.as_ref()))
                        .filter(|r| made.is_some_and(|fix| fix.round == r.n));
                    let repo =
                        (feature.into_iter().flat_map(|f| &f.repos)).find(|r| r.worktree == c.root);
                    flag.fixed_in = (round.zip(repo))
                        .filter(|_| flag.made)
                        .and_then(|(round, repo)| round.trail.to.get(&repo.key()))
                        .map(|sha| sha.chars().take(7).collect());
                }
                c
            })
            .collect()
    }

    /// The card of AI review's page with the selection, to find it again.
    /// Open review card `i` of the page (true), or fold it to its line; it
    /// takes the keyboard.
    pub(crate) fn fold_card(&mut self, i: usize, open: bool) {
        let id = self.with_review_cards(|cards, _| cards.get(i).map(|c| c.id()));
        let (Some(Some(id)), Some(panel)) = (id, self.panel.as_mut()) else {
            return;
        };
        panel.cards_open.retain(|(c, _)| *c != id);
        panel.cards_open.push((id, open));
        panel.focus = TabArea::Findings;
        panel.finding = i;
    }

    pub(crate) fn selected_review_card(&self) -> Option<CardId> {
        self.with_review_cards(|cards, at| cards.get(at).map(|c| c.id()))
            .flatten()
    }

    /// The selection goes back on card `id` once the list around it changed.
    /// Nothing moves if the card is gone: the next takes its place.
    pub(crate) fn reselect_review_card(&mut self, id: Option<CardId>) {
        let Some(id) = id else {
            return;
        };
        let at = self
            .with_review_cards(|cards, _| cards.iter().position(|c| c.id() == id))
            .flatten();
        if let (Some(at), Some(panel)) = (at, self.panel.as_mut()) {
            panel.finding = at;
        }
    }

    /// Stop the feature tab's columns flung; whether one was.
    pub(crate) fn feature_tab_stop_fling(&mut self) -> bool {
        (self.panel.as_mut()).is_some_and(|p| {
            p.scroll
                .iter_mut()
                .fold(false, |any, s| s.stop_fling() | any)
        })
    }

    /// The feature tab's strip of what runs on feature `slug` of
    /// `project`, called `title`, each row with its job (see
    /// `running_rows`).
    pub(crate) fn running_of(
        &self,
        project: &str,
        slug: &str,
        title: &str,
    ) -> (Vec<(chrome::RunningRow, Job)>, usize) {
        let since = |b: &Busy| {
            let key = (b.project.clone(), b.slug.clone(), b.job);
            self.since.get(&key).copied()
        };
        let busy: Vec<_> = (self.busy.iter())
            .filter(|b| b.project == project && b.slug == slug)
            .filter_map(|b| Some((b, since(b)?)))
            .collect();
        running_rows(&busy, title, Instant::now())
    }
}

#[cfg(test)]
mod access_tests {
    use super::{
        tests::{feature, repo},
        *,
    };

    fn busy(slug: &str, job: Job) -> Busy {
        Busy {
            project: "acme".into(),
            slug: slug.into(),
            job,
            repo: None,
            what: format!("Working on {slug}"),
        }
    }

    fn with(own: Vec<Feature>, viewed: Vec<Feature>) -> Features {
        let mut features = Features::default();
        features.all.insert("acme".into(), own);
        features.viewed.insert("acme".into(), viewed);
        features
    }

    #[test]
    fn a_feature_is_found_written_here_or_elsewhere_but_changes_only_here() {
        let mut theirs = feature("theirs", vec![repo("api", true)], 1);
        theirs.writer = Some("m1".into());
        let mine = feature("mine", vec![repo("api", true)], 1);
        let mut features = with(vec![mine], vec![theirs]);
        assert_eq!(
            features.feature("acme", "mine").map(|f| f.slug.as_str()),
            Some("mine")
        );
        assert!(features.feature("acme", "theirs").is_some());
        assert!(features.feature("other", "mine").is_none());
        assert!(features.feature_mut("acme", "mine").is_some());
        assert!(features.feature_mut("acme", "theirs").is_none());
    }

    #[test]
    fn busy_work_counts_until_it_is_over_and_a_graph_holds_nothing_up() {
        let mut features = Features::default();
        features.busy.push(busy("f", Job::Graph));
        assert!(!features.feature_busy("acme", "f"));
        assert!(features.busy_with("acme", "f", Job::Graph).is_some());
        features.busy.push(busy("f", Job::Verify));
        features
            .since
            .insert(("acme".into(), "f".into(), Job::Verify), Instant::now());
        assert!(features.feature_busy("acme", "f"));
        assert!(!features.feature_busy("acme", "g"));
        features.unbusy("acme", "f", Job::Verify);
        assert!(!features.feature_busy("acme", "f"));
        assert!(features.busy_with("acme", "f", Job::Verify).is_none());
        assert!(features.since.is_empty(), "the start time goes with it");
        assert_eq!(features.busy.len(), 1);
    }

    #[test]
    fn the_tasks_panel_lists_busy_work_by_its_feature() {
        let mut features = with(vec![feature("f", Vec::new(), 1)], Vec::new());
        features.busy.push(busy("f", Job::Setup));
        let tasks = features.feature_tasks(&projects::Store::default());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].chip, "setup");
        assert_eq!(tasks[0].detail, "\u{201c}Feature f\u{201d} \u{b7} acme");
    }

    #[test]
    fn without_a_probe_or_a_panel_nothing_is_read_or_selected() {
        let mut features = with(vec![feature("f", Vec::new(), 1)], Vec::new());
        assert!(features.last_probe("acme", "f").is_none());
        assert!(features.feature_reading("acme", "f").is_none());
        assert!(features.feature_docs("acme", "f").is_none());
        assert!(features.sent_notes("acme", "f").is_empty());
        assert!(features.panel_feature().is_none());
        assert!(features.selected_review_card().is_none());
        assert!(features.offered_repo("api").is_none());
        assert!(!features.feature_tab_stop_fling());
        assert!(
            features
                .new_feature_view(&projects::Store::default())
                .is_none()
        );
        assert!(features.ai_work_chats().is_empty());
        features.sync_feature_page();
        features.feature_cursor_to_current_of("acme", "f");
        assert!(features.panel.is_none());
    }

    #[test]
    fn the_panel_shows_the_current_stage_again_only_for_its_feature() {
        let mut features = with(vec![feature("f", Vec::new(), 1)], Vec::new());
        features.panel = Some(FeaturePanel {
            project: "acme".into(),
            slug: "f".into(),
            stage: Some(Stage::Plan),
            ..FeaturePanel::default()
        });
        assert_eq!(features.panel_feature().map(|f| f.slug.as_str()), Some("f"));
        features.feature_cursor_to_current_of("acme", "g");
        assert_eq!(
            features.panel.as_ref().and_then(|p| p.stage),
            Some(Stage::Plan)
        );
        features.feature_cursor_to_current_of("acme", "f");
        assert_eq!(features.panel.as_ref().and_then(|p| p.stage), None);
    }

    #[test]
    fn the_strip_shows_only_the_features_work() {
        let mut features = Features::default();
        features.busy.push(busy("f", Job::Verify));
        features.busy.push(busy("g", Job::Verify));
        let long_ago = Instant::now() - Duration::from_secs(60);
        for slug in ["f", "g"] {
            features
                .since
                .insert(("acme".into(), slug.into(), Job::Verify), long_ago);
        }
        let (rows, more) = features.running_of("acme", "f", "Feature f");
        assert_eq!((rows.len(), more), (1, 0));
        assert_eq!(rows[0].1, Job::Verify);
    }
}
