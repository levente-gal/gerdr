//! `Gerdr`'s side of the new feature dialog and of a feature's repositories.

use super::*;

impl Gerdr {
    // ----------------------------------------------------------- dialog

    /// Open the new feature dialog for the active project, offering all
    /// its repositories: the one workspace `at` is in, else the active
    /// one's, starts ticked.
    pub fn open_new_feature(&mut self, at: Option<usize>) {
        let from = self.dialog_from(at);
        self.open_dialog(from, None);
    }

    /// Open the new feature dialog to pick pull requests to review, one per
    /// repository, from the folder of workspace `at` as `open_new_feature`.
    pub fn open_review_pulls(&mut self, at: Option<usize>) {
        let from = self.dialog_from(at);
        self.open_dialog(from, Some((String::new(), String::new(), 0)));
    }

    /// Open the review dialog on the pull requests review `slug` holds, to
    /// add or leave out some (see `change_pulls`).
    pub(crate) fn open_review_edit(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug).cloned() else {
            return;
        };
        if let Some(why) = self.review_refusal(&f) {
            return self.show_warning(why);
        }
        if self.project.as_deref() != Some(project) {
            return;
        }
        self.open_dialog(
            Some(f.root.clone()),
            Some((String::new(), String::new(), 0)),
        );
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        dialog.edit(&f);
        self.find_others();
    }

    /// The folder the dialog opens from: workspace `at`'s, else the active
    /// one's, else the project's first workspace's.
    pub(crate) fn dialog_from(&self, at: Option<usize>) -> Option<PathBuf> {
        let project = self.projects.project(self.project.as_deref()?)?;
        at.and_then(|i| self.workspaces.get(i))
            .or_else(|| self.workspace())
            .map(|ws| ws.dir.clone())
            .or_else(|| project.workspaces.first().map(|w| w.dir.clone()))
    }

    /// Open the new feature dialog from folder `from`; with `wanted`, to
    /// review pull requests, that one (host, slug, number) picked once
    /// listed (number 0: none).
    pub(crate) fn open_dialog(
        &mut self,
        from: Option<PathBuf>,
        wanted: Option<(String, String, u64)>,
    ) {
        let what = match wanted {
            Some(_) => "review pull requests",
            None => "start a feature",
        };
        let Some(id) = self.project.clone() else {
            return self.show_warning(format!("Open a project to {what} in"));
        };
        let Some(from) = from else {
            return self.show_warning(format!("Add a workspace with a folder to {what}"));
        };
        self.features.serial += 1;
        let serial = self.features.serial;
        self.overlays.context_menu = None;
        self.stop_key_repeat();
        let mut dialog = NewFeatureDialog::new(id.clone(), serial, from.clone());
        let workflow = self.projects.project(&id).map(|p| &p.workflow);
        dialog.review = wanted.map(|w| {
            let (reviewers, default_reviewer) =
                crate::feature_agents::new_review_chips(workflow, &self.launcher.cli_agents);
            Reviewing {
                wanted: (w.2 != 0).then_some(w),
                reviewers,
                default_reviewer,
                ..Reviewing::default()
            }
        });
        self.features.dialog = Some(dialog);
        self.find_home_root(&id, from, serial);
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Find project `id`'s root from folder `from` off the thread, for the
    /// dialog of `serial`.
    fn find_home_root(&mut self, id: &str, from: PathBuf, serial: u64) {
        let Some(project) = self.projects.project(id) else {
            return;
        };
        let (workspace_dirs, ignore, bases) = root_search(project);
        let others = self.other_projects_dirs(id);
        self.feature_work(None, move |_| Done::Found {
            serial,
            from: layout::main_top(&from),
            result: layout::find_project_root(&from, &workspace_dirs, &others, &ignore, &bases),
        });
    }

    /// Read off the thread (checklist D3) which repositories the feature
    /// panel can offer: those of its project, and of the projects it links
    /// for what it holds or held of them. What was found before stays until
    /// the new read is in. A linked project with no workspace folder has
    /// none to find. A shipped feature's
    /// repositories can't change: it looks nowhere.
    pub(crate) fn find_repo_sources(&mut self) {
        let Some(panel) = self.features.panel.as_ref() else {
            return;
        };
        let (project, slug) = (panel.project.clone(), panel.slug.clone());
        let Some(feature) = self.feature(&project, &slug).filter(|f| linkable(f)) else {
            return;
        };
        let home_dir = feature.dir.clone();
        let known = |source: &Option<String>| {
            (panel.sources.iter())
                .find(|s| s.project == *source)
                .map_or(Finding::Looking, |s| s.finding.clone())
        };
        let mut sources = Vec::new();
        let mut finds = Vec::new();
        // The home project first, whatever its place in the store.
        let (home, others): (Vec<_>, Vec<_>) = (self.projects.projects.iter())
            .filter(|p| p.id == project || feature.link(&p.id).is_some())
            .partition(|p| p.id == project);
        for p in home.into_iter().chain(others) {
            let is_home = p.id == project;
            let search = root_search(p);
            let from = match is_home {
                true => Some(home_dir.clone()),
                false => search.0.first().cloned(),
            };
            let Some(from) = from else { continue };
            let source = (!is_home).then(|| p.id.clone());
            sources.push(RepoSource {
                finding: known(&source),
                project: source.clone(),
                name: p.name.clone(),
            });
            finds.push((source, from, search, self.other_projects_dirs(&p.id)));
        }
        if let Some(panel) = self.features.panel.as_mut() {
            panel.sources = sources;
        }
        for (source, from, (dirs, ignore, bases), others) in finds {
            let (project, slug) = (project.clone(), slug.clone());
            self.feature_work(None, move |_| Done::FoundRepos {
                project,
                slug,
                source,
                result: layout::find_project_root(&from, &dirs, &others, &ignore, &bases),
            });
        }
    }

    /// The workspace folders of every project but `id`, whose repositories
    /// are theirs.
    pub(crate) fn other_projects_dirs(&self, id: &str) -> Vec<PathBuf> {
        (self.projects.projects.iter())
            .filter(|p| p.id != id)
            .flat_map(|p| p.workspaces.iter().map(|w| w.dir.clone()))
            .collect()
    }

    /// Make feature `slug` hold repository `key` on its branch: a new
    /// worktree from its base. Taken
    /// out before, it gets a new branch.
    pub(super) fn add_repo_to_feature(&mut self, project: &str, slug: &str, key: &str) {
        if self.features.repos_locked(&mut ui!(self), project, slug) {
            return;
        }
        let Some((source, found)) = self.features.offered_repo(key) else {
            return self.show_warning(format!(
                "{key} wasn't found among the projects' repositories"
            ));
        };
        let Finding::Found(root) = source.finding else {
            return;
        };
        let add = AddSpec {
            project: source.project,
            name: source.name,
            root,
            names: vec![found.name],
        };
        self.link_repos(project.to_owned(), slug.to_owned(), vec![add]);
    }

    /// Let agents of feature `slug` read repository `key`'s main checkout:
    /// no git, no worktree. An agent running keeps what it started with,
    /// which the toast says.
    pub(super) fn add_context_repo(&mut self, project: &str, slug: &str, key: &str) {
        if self.features.repos_locked(&mut ui!(self), project, slug) {
            return;
        }
        let Some((source, found)) = self.features.offered_repo(key) else {
            return self.show_warning(format!(
                "{key} wasn't found among the projects' repositories"
            ));
        };
        let repo = feature::ContextRepo {
            name: found.name,
            main: found.main,
            project: source.project,
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.log(feature::now(), format!("added {key} as context"));
        feature.add_context(repo);
        self.save_feature(project, slug);
        self.features.select_repo(project, slug, key);
        self.show_success(context_text(&self.repo_name(project, slug, key)));
    }

    /// Stop offering repository `key` to agents of feature `slug`: nothing
    /// is lost, and the toast can undo it (checklist C4).
    pub(super) fn drop_context_repo(&mut self, project: &str, slug: &str, key: &str) {
        if self.features.repos_locked(&mut ui!(self), project, slug) {
            return;
        }
        let named = self.repo_name(project, slug, key);
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(repo) = feature.context.iter().find(|c| c.key() == key).cloned() else {
            return;
        };
        let now = feature::now();
        feature.context.retain(|c| c.key() != key);
        feature.note_removed(repo.project.clone(), repo.name.clone(), now);
        feature.log(now, format!("removed {key} from context"));
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.features.select_repo(project, slug, key);
        let undo = MenuAction::RestoreContextRepo {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: Box::new(repo),
        };
        self.toast_with(
            format!("\u{201c}{title}\u{201d} no longer has {named} as context"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// Undo `drop_context_repo`.
    pub(crate) fn restore_context_repo(
        &mut self,
        project: &str,
        slug: &str,
        repo: feature::ContextRepo,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let key = repo.key();
        // Added to the feature since: it holds it, which is more.
        if feature.repo(&key).is_some() {
            return;
        }
        feature.log(feature::now(), format!("added {key} as context"));
        feature.add_context(repo);
        self.save_feature(project, slug);
        self.features.select_repo(project, slug, &key);
    }

    /// List the store's other projects in the dialog's picker, once, and
    /// find each one's root off the thread (checklist D3), as the home
    /// one's is: from its first workspace's folder.
    fn find_others(&mut self) {
        let Some(dialog) = self.features.dialog.as_ref() else {
            return;
        };
        if (!dialog.picking && dialog.link_first.is_empty()) || !dialog.others.is_empty() {
            return;
        }
        let (home, serial) = (dialog.project.clone(), dialog.serial);
        let mut others = Vec::new();
        let mut finds = Vec::new();
        for p in self.projects.projects.iter().filter(|p| p.id != home) {
            let (dirs, ignore, bases) = root_search(p);
            let root = match dirs.first() {
                Some(from) => {
                    let theirs = self.other_projects_dirs(&p.id);
                    finds.push((
                        p.id.clone(),
                        from.clone(),
                        dirs.clone(),
                        theirs,
                        ignore,
                        bases,
                    ));
                    Finding::Looking
                }
                None => Finding::Failed("it has no workspace with a folder".into()),
            };
            others.push(Other {
                project: p.id.clone(),
                name: p.name.clone(),
                root,
                linked: dialog.link_first.contains(&p.id),
                touched: Vec::new(),
            });
        }
        if let Some(dialog) = self.features.dialog.as_mut() {
            dialog.others = others;
            dialog.link_first.clear();
        }
        for (project, from, dirs, theirs, ignore, bases) in finds {
            self.feature_work(None, move |_| Done::FoundOther {
                serial,
                project,
                result: layout::find_project_root(&from, &dirs, &theirs, &ignore, &bases),
            });
        }
    }

    /// Read off the thread (checklist D3) the branches of the dialog's
    /// repositories not looked at yet, then what origin says their default
    /// is: a clone's `origin/HEAD` can be long out of date. A base the
    /// project's settings set isn't asked about. Takes as long as the
    /// network does, so the branches come first, and the row says which it
    /// has meanwhile.
    pub(super) fn find_bases(&mut self) {
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        if dialog.review.is_some() {
            return self.find_pulls();
        }
        let (home, serial) = (dialog.project.clone(), dialog.serial);
        for (repo, found) in dialog.unlooked() {
            let of = repo.0.as_deref().unwrap_or(&home);
            let set =
                (self.projects.project(of)).is_some_and(|p| p.workflow.base.contains_key(&repo.1));
            self.feature_work(None, move |tx| {
                let result = layout::branches_of(&found.main);
                let branches = Done::BaseBranches {
                    serial,
                    repo: repo.clone(),
                    result,
                };
                if set {
                    return branches;
                }
                // It fails only once the loop is gone, as in `feature_work`.
                let _ = tx.send(branches);
                Done::RemoteBase {
                    serial,
                    repo,
                    result: crate::git_ops::remote_default(&found.main),
                }
            });
        }
    }

    /// List off the thread (checklist D3) the open pull requests of the
    /// review dialog's repositories not listed yet.
    fn find_pulls(&mut self) {
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        let serial = dialog.serial;
        for (repo, main) in dialog.unlisted() {
            self.feature_work(None, move |_| Done::OpenPulls {
                serial,
                result: feature::pr::open_pulls(&main),
                repo,
            });
        }
    }

    /// How many repositories the dialog shows at once.
    fn new_feature_fit(&self) -> usize {
        let Some(dialog) = &self.features.dialog else {
            return 1;
        };
        dialog.layout(self.output_size()).rows.len()
    }

    /// A key while the new feature dialog has the keyboard.
    pub fn new_feature_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<DialogOutcome> {
        let fit = self.new_feature_fit();
        let outcome = self.features.dialog.as_mut()?.key(mods, sym, typed, fit);
        self.find_others();
        self.find_bases();
        outcome
    }

    /// Do what the new feature dialog asked: create the feature off the
    /// thread, shown in the tasks panel meanwhile.
    pub fn apply_new_feature(&mut self, outcome: DialogOutcome) {
        let Some(dialog) = self.features.dialog.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        let (title, root, touched, links) = match outcome {
            DialogOutcome::Cancel => return,
            DialogOutcome::Create {
                title,
                root,
                touched,
                links,
            } => (title, root, touched, links),
            DialogOutcome::Review {
                title,
                root,
                links,
                picks,
                into: Some(slug),
                ..
            } => return self.change_pulls(dialog.project, slug, root, links, picks, title),
            DialogOutcome::Review {
                title,
                root,
                links,
                picks,
                into: None,
                reviewer,
            } => {
                let project = dialog.project;
                return self.start_review_of(project, root, links, picks, title, reviewer);
            }
        };
        let project = dialog.project;
        let all = self
            .features
            .all
            .get(&project)
            .map_or(&[][..], Vec::as_slice);
        // One another machine has shown here keeps its name.
        let shown: Vec<Feature> = (all.iter().chain(self.viewed_of(&project)))
            .cloned()
            .collect();
        let slug = new_slug(&title, &shown);
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        self.feature_work(Some(busy), move |_| {
            let result = layout::create_spanning(&root, &slug, &touched, &links);
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            }
        });
    }

    /// A feature's worktrees were made, or not: keep it, show it, and say
    /// what was changed outside its folder.
    pub(super) fn feature_created(
        &mut self,
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<(Created, Vec<(feature::Link, Created)>), String>,
    ) {
        let (created, linked) = match result {
            Ok(made) => made,
            Err(why) => {
                tracing::warn!("creating feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't create \u{201c}{title}\u{201d}: {why}"));
            }
        };
        // A slug used before: this folder has no link yet.
        self.features.docs_linked.remove(&created.dir);
        let mut ignores = created.ignores.clone();
        ignores.extend(linked.iter().flat_map(|(_, c)| c.ignores.iter().cloned()));
        let text = created_text(&title, &root.path, &ignores);
        let now = feature::now();
        let mut repos = created.repos;
        let mut links = Vec::new();
        for (link, made) in linked {
            links.push(link);
            repos.extend(made.repos);
        }
        let feature = Feature {
            version: feature::VERSION,
            kind: Default::default(),
            writer: None,
            slug: slug.clone(),
            title: title.clone(),
            project: project.clone(),
            created: now,
            root: root.path,
            dir: created.dir,
            multi: root.multi,
            repos,
            links,
            status: Status::Open,
            marks: BTreeMap::new(),
            run_counts: BTreeMap::new(),
            context: Vec::new(),
            removed: Vec::new(),
            history: vec![Event {
                at: now,
                what: "Created".into(),
            }],
            shipping: Default::default(),
            ship_plan: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            stage_machines: Default::default(),
            setups_elsewhere: Vec::new(),
            came_from: None,
            epoch: 0,
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            verify_fixes: Vec::new(),
            fix_asked: Default::default(),
            comment_fixes: Vec::new(),
            ran_once: Default::default(),
            fix_rounds: 0,
            past_rounds: Vec::new(),
            drafts: Vec::new(),
            carry: None,
            unverified: None,
            back_from: None,
        };
        self.features
            .all
            .entry(project.clone())
            .or_default()
            .push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        self.show_success(text);
        self.start_setup(&project, &slug);
        self.refresh_feature_graphs(&project, &slug, true);
    }

    /// Bring `adds` into the folders of open feature `slug` off the thread, shown in the tasks panel meanwhile. A setup or check
    /// under way has a copy of the feature that wouldn't see them: refused
    /// meanwhile.
    fn link_repos(&mut self, project: String, slug: String, adds: Vec<AddSpec>) {
        let Some(feature) = self.feature(&project, &slug).cloned() else {
            return self.show_warning("That feature is gone".into());
        };
        if self.feature_busy(&project, &slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let names: Vec<String> = adds.iter().flat_map(|a| a.names.iter().cloned()).collect();
        // A base the project's settings set isn't asked about, as in the new
        // feature dialog (see `find_bases`).
        let set: Vec<std::collections::BTreeMap<String, String>> = (adds.iter())
            .map(|a| {
                let of = a.project.as_deref().unwrap_or(&project);
                self.projects
                    .project(of)
                    .map(|p| p.workflow.base.clone())
                    .unwrap_or_default()
            })
            .collect();
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Link,
            repo: None,
            what: format!(
                "Linking {} to \u{201c}{}\u{201d}",
                names.join(", "),
                feature.title
            ),
        };
        self.feature_work(Some(busy), move |_| {
            let mut adds = adds;
            for (add, set) in adds.iter_mut().zip(&set) {
                let names = &add.names;
                (add.root.repos.iter_mut())
                    .filter(|r| names.contains(&r.name) && !set.contains_key(&r.name))
                    .for_each(layout::follow_origin);
            }
            let result = layout::link_more(&feature, &[], &adds);
            Done::Linked {
                project,
                slug,
                names,
                result,
            }
        });
    }

    /// `names` were linked to feature `slug`, or not: kept, and what joined
    /// set up.
    pub(super) fn feature_linked(
        &mut self,
        project: &str,
        slug: &str,
        names: &[String],
        result: Result<layout::MoreLinked, String>,
    ) {
        let (made, repos) = match result {
            Ok(made) => made,
            Err(why) => {
                tracing::warn!("linking repositories to feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't link {}: {why}", names.join(", ")));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        let root = feature.root.clone();
        let mut ignores = Vec::new();
        let mut keys = Vec::new();
        let mut joined = repos;
        for (link, created) in made {
            feature.log(now, format!("linked project {}", link.project));
            feature.links.push(link);
            ignores.extend(created.ignores);
            joined.extend(created.repos);
        }
        for repo in &joined {
            keys.push(repo.key());
            feature.log(now, format!("linked {}", repo.key()));
        }
        feature.insert_repos(joined);
        let title = feature.title.clone();
        self.save_feature(project, slug);
        if let Some(key) = keys.first() {
            self.features.select_repo(project, slug, key);
        }
        self.show_success(linked_text(&title, names, &root, &ignores));
        self.probe_feature(project, slug);
        self.rebuild_feature_graphs(project, slug);
        self.start_setup_of(project, slug, Some(keys));
    }

    /// A left click while the new feature dialog is open.
    pub fn new_feature_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        let layout = dialog.layout(size);
        let mut picked = false;
        let outcome = match dialog.hit(&layout, pos) {
            // The picker over the form leaves its buttons off.
            Some(NewFeatureHit::Button(_) | NewFeatureHit::Link) if dialog.choosing.is_some() => {
                None
            }
            Some(NewFeatureHit::Reviewer(_)) if dialog.choosing.is_some() => None,
            Some(NewFeatureHit::Button(chrome::CREATE)) => match dialog.create() {
                Ok(outcome) => Some(outcome),
                Err(why) => {
                    dialog.error = Some(why);
                    None
                }
            },
            Some(NewFeatureHit::Button(_)) => Some(DialogOutcome::Cancel),
            Some(NewFeatureHit::Base(i)) => {
                dialog.open_bases(i);
                None
            }
            Some(NewFeatureHit::Repo(i)) if dialog.choosing.is_some() => {
                match dialog.review {
                    Some(_) => dialog.pick_pull(i),
                    None => dialog.pick_base(i),
                }
                None
            }
            Some(NewFeatureHit::Repo(i)) => {
                if dialog.multi() {
                    dialog.focus = Some(i);
                    dialog.toggle(i);
                }
                None
            }
            // Its filter has the keys all along.
            Some(NewFeatureHit::Field) if dialog.choosing.is_some() => None,
            Some(NewFeatureHit::Field) => {
                dialog.focus = None;
                None
            }
            Some(NewFeatureHit::Reviewer(i)) => {
                dialog.focus = Some(REVIEWER);
                dialog.pick_reviewer(i);
                None
            }
            Some(NewFeatureHit::Link) => {
                dialog.pick();
                picked = true;
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            // Off it, it stays: a click away never throws the title away.
            None => None,
        };
        if picked {
            self.find_others();
        }
        self.find_bases();
        if let Some(outcome) = outcome {
            self.apply_new_feature(outcome);
        }
    }

    /// The pointer moved while the new feature dialog is open: a reviewer
    /// chip offers its agent's name.
    pub fn new_feature_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let size = self.output_size();
        let dialog = self.features.dialog.as_mut()?;
        let layout = dialog.layout(size);
        dialog.hover = dialog.hit(&layout, pos);
        let (Some(NewFeatureHit::Reviewer(i)), None) = (dialog.hover, &dialog.choosing) else {
            return None;
        };
        let chip = dialog.reviewers().get(i)?;
        let (_, _, spots) = layout.reviewer.as_ref()?;
        Some(Tooltip::new(&chip.name, *spots.get(i)?))
    }

    /// Scroll the dialog's repositories `by` rows.
    pub fn new_feature_scroll(&mut self, by: isize) {
        let fit = self.new_feature_fit();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let most = dialog.repos().saturating_sub(fit.max(1));
            dialog.top = dialog.top.saturating_add_signed(by).min(most);
        }
    }

    /// The dialog's repository rows shown, for the wheel's pitch.
    pub fn new_feature_rows(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        dialog.layout(self.output_size()).rows
    }

    /// What the pointer can press in the new feature dialog.
    pub fn new_feature_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        dialog.layout(self.output_size()).controls()
    }

    pub fn new_feature_view(&self) -> Option<NewFeatureView> {
        self.features.new_feature_view(&self.projects)
    }
}
