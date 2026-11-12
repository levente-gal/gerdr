//! `Gerdr`'s side of features: the patch watch, probes and reads of their code.

use super::*;

impl Gerdr {
    // ------------------------------------------------------ patch watch

    /// Start the patch watch, once: every `PANEL_EVERY` it probes the
    /// feature shown, and the watched ones not probed for as long as
    /// `watch_every` says.
    pub(super) fn watch_features(&mut self) {
        if self.features.watching {
            return;
        }
        let timer = Timer::from_duration(PANEL_EVERY);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.feature_watch_tick();
            TimeoutAction::ToDuration(PANEL_EVERY)
        });
        match inserted {
            Ok(_) => self.features.watching = true,
            Err(err) => tracing::warn!("watching the features' code: {err}"),
        }
    }

    /// Read the shown feature's agent tail now, then every
    /// `page::TAIL_EVERY` while a feature's tab shows; the timer stops
    /// once none does.
    pub(super) fn watch_feature_tail(&mut self) {
        self.feature_tail_tick();
        if self.features.tailing {
            return;
        }
        let every = feature::page::TAIL_EVERY;
        let inserted =
            self.loop_handle
                .insert_source(Timer::from_duration(every), move |_, _, state| {
                    if state.feature_tail_tick() {
                        TimeoutAction::ToDuration(every)
                    } else {
                        state.features.tailing = false;
                        TimeoutAction::Drop
                    }
                });
        match inserted {
            Ok(_) => self.features.tailing = true,
            Err(err) => tracing::warn!("reading the feature agent's pane: {err}"),
        }
    }

    /// The pane of the shown feature's current stage agent, while it is
    /// there.
    fn tail_pane(&self) -> Option<String> {
        let feature = self.features.panel_feature()?;
        let stage = feature.current().filter(|s| needs(*s).is_some())?;
        let pane = feature.runs.get(&stage)?.pane.clone()?;
        self.pane_there(&pane).then_some(pane)
    }

    /// Read the shown feature's agent tail off the thread, unless a read
    /// runs; with no agent pane, the tail goes. Whether a feature's tab
    /// still shows.
    fn feature_tail_tick(&mut self) -> bool {
        if self.features.panel.is_none() {
            self.features.tail = None;
            self.features.check_tails.clear();
            return false;
        }
        self.check_tails_tick();
        let Some(pane) = self.tail_pane() else {
            self.features.tail = None;
            return true;
        };
        let tail = self.features.tail.get_or_insert_with(AgentTail::default);
        if tail.pane != pane {
            *tail = AgentTail {
                pane: pane.clone(),
                ..AgentTail::default()
            };
        }
        if tail.reading {
            return true;
        }
        let Some((socket, bare)) = self.pane_socket(&pane) else {
            return true;
        };
        if let Some(tail) = self.features.tail.as_mut() {
            tail.reading = true;
        }
        self.feature_work(None, move |_| Done::Tail {
            lines: feature::page::read_tail(socket, &bare),
            pane,
        });
        true
    }

    /// Read off the thread the panes of the checks running on the page
    /// shown, each unless a read of it runs. A check that ended takes its
    /// tail along: its card has what it printed by then.
    fn check_tails_tick(&mut self) {
        let panes: Vec<(String, bool)> = (self
            .features
            .panel_feature()
            .zip(self.features.panel.as_ref()))
        .filter(|(feature, panel)| panel.page_stage(feature) == Stage::Verify && !panel.keys)
        .map(|(feature, _)| {
            (check_rows(feature, Stage::Verify).iter())
                .filter(|row| row.outcome.is_none() && checks_running(feature))
                .filter_map(|row| Some((row.pane.clone()?, row.began.is_some())))
                .filter(|(pane, _)| self.pane_there(pane))
                .collect()
        })
        .unwrap_or_default();
        (self.features.check_tails).retain(|pane, _| panes.iter().any(|(p, _)| p == pane));
        for (pane, began) in panes {
            let Some((socket, there)) = self.pane_socket(&pane) else {
                continue;
            };
            let tail = (self.features.check_tails.entry(pane.clone())).or_default();
            if tail.reading {
                continue;
            }
            tail.pane = pane.clone();
            tail.reading = true;
            self.feature_work(None, move |_| Done::CheckTail {
                lines: feature::page::read_check_tail(socket, &there, began),
                pane,
            });
        }
    }

    /// Probe what is due.
    fn feature_watch_tick(&mut self) {
        self.plan_doc_follows();
        let mut due = Vec::new();
        if let Some(panel) = &self.features.panel {
            due.push((panel.project.clone(), panel.slug.clone(), PANEL_EVERY));
        }
        // A file's tab says whether its stage is locked by now.
        if let Some(doc) = (self.tab().and_then(|t| t.review())).and_then(|r| r.wd.doc.as_ref()) {
            due.push((doc.project.clone(), doc.slug.clone(), PANEL_EVERY));
        }
        for (project, all) in &self.features.all {
            for feature in all.iter().filter(|f| watched(f)) {
                due.push((project.clone(), feature.slug.clone(), watch_every(feature)));
            }
        }
        let now = Instant::now();
        // The timer's own jitter doesn't skip a round.
        let slack = Duration::from_secs(1);
        for (project, slug, every) in due {
            let probed = self.features.probes.get(&(project.clone(), slug.clone()));
            let fresh = probed
                .and_then(|p| p.at)
                .is_some_and(|at| now.duration_since(at) + slack < every);
            if !fresh && !probed.is_some_and(|p| p.running) {
                self.probe_feature(&project, &slug);
            }
        }
    }

    /// Probe open feature `slug` of `project` off the thread (see
    /// `ship::probe`); one running already goes again once it is back.
    pub(crate) fn probe_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        // The brief may have changed: its criteria are read again. A finished
        // feature's page shows them too.
        self.load_criteria(project, slug);
        if feature.status != Status::Open && !feature.ships_more() {
            self.probe_finished(project, slug, &feature);
            return;
        }
        if self.feature_sender().is_none() {
            return;
        }
        let modes = self.projects.workflows(&feature, |w| w.ship);
        let probed = self
            .features
            .probes
            .entry((project.to_owned(), slug.to_owned()))
            .or_default();
        if probed.running {
            probed.again = true;
            return;
        }
        probed.running = true;
        let now = Instant::now();
        probed.at = Some(now);
        // Its pull requests, once a while: gh asks GitHub.
        let read_prs = feature.ships()
            && (feature.ship_log().iter()).any(|d| d.step == ShipStep::OpenPr)
            && probed
                .prs_at
                .is_none_or(|at| now.duration_since(at) >= PRS_EVERY);
        if read_prs {
            probed.prs_at = Some(now);
        }
        let fetch = feature.ships()
            && probed
                .fetch_at
                .is_none_or(|at| now.duration_since(at) >= FETCH_EVERY);
        if fetch {
            probed.fetch_at = Some(now);
        }
        let data = store::dir(None, project, slug);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::Probed {
            probe: Box::new(match feature.writer {
                Some(_) => Probe {
                    docs: feature::page::read_docs(&data).with_comment_fixes(&data, &feature),
                    ..ship::probe_seen(&feature)
                },
                None => Probe {
                    docs: feature::page::read_docs(&data).with_comment_fixes(&data, &feature),
                    models: feature::agents::read_models(&feature),
                    prs: read_prs.then(|| ship::read_prs(&feature, "gh")),
                    bases: match feature.ships() {
                        true => ship::read_bases(&feature, fetch),
                        false => BTreeMap::new(),
                    },
                    resolution: ship::read_resolution(&feature, &data),
                    ..ship::probe(&feature, &modes)
                },
            }),
            project,
            slug,
        });
    }

    /// Read finished `feature` (`slug` of `project`) for its tab: its
    /// stage files and line counts only (`ship::probe_finished`). Its code
    /// isn't watched any more, nor its hooks made sure of.
    fn probe_finished(&mut self, project: &str, slug: &str, feature: &Feature) {
        if self.feature_sender().is_none() {
            return;
        }
        let probed = self
            .features
            .probes
            .entry((project.to_owned(), slug.to_owned()))
            .or_default();
        if probed.running {
            return;
        }
        probed.running = true;
        probed.at = Some(Instant::now());
        let data = store::dir(None, project, slug);
        let (project, slug, feature) = (project.to_owned(), slug.to_owned(), feature.clone());
        self.feature_work(None, move |_| Done::Probed {
            probe: Box::new(Probe {
                docs: feature::page::read_docs(&data).with_comment_fixes(&data, &feature),
                models: feature::agents::read_models(&feature),
                ..ship::probe_finished(&feature)
            }),
            project,
            slug,
        });
    }

    /// A probe of feature `slug` of `project` is back: its code changed
    /// sends it back, its squash merged makes it done.
    pub(super) fn feature_probed(&mut self, project: &str, slug: &str, mut probe: Probe) {
        self.feature_models_are(project, slug, std::mem::take(&mut probe.models));
        let key = (project.to_owned(), slug.to_owned());
        // The diff's comments may be others now: the keyboard stays on its card.
        let on_tab =
            (self.features.panel.as_ref()).is_some_and(|p| p.project == project && p.slug == slug);
        let notes_changed = on_tab
            && (self.features.probes.get(&key))
                .and_then(|p| p.last.as_ref())
                .is_none_or(|last| last.docs.notes != probe.docs.notes);
        let selected = notes_changed.then(|| self.selected_review_card()).flatten();
        // Written elsewhere, what its code says is for its writer to act on.
        let open = (self.feature(project, slug))
            .is_some_and(|f| f.status == Status::Open && f.writer.is_none());
        let probed = self.features.probes.entry(key.clone()).or_default();
        probed.running = false;
        let docs = std::mem::take(&mut probe.docs);
        probe.docs = docs.keeping(probed.last.as_ref().map(|last| &last.docs));
        // Not read this time: as last read.
        if probe.prs.is_none() {
            probe.prs = probed.last.as_ref().and_then(|last| last.prs.clone());
        }
        // A finished feature's read is for its tab only, and what it ships
        // more.
        let resolution = probe.resolution.clone();
        if !open {
            crate::problems::clear_at(BASE_PROBLEM, &format!("{project}/{slug}"));
            probed.again = false;
            let merged = probe.merged;
            let patch = probe.reading.as_ref().ok().map(|r| r.patch.clone());
            probed.last = Some(probe);
            if let Some(found) = &resolution {
                self.resolution_read(project, slug, found, patch.as_deref());
            }
            let waited = self.features.ship_waits.remove(&key);
            self.reselect_review_card(selected);
            let more = (self.feature(project, slug)).is_some_and(Feature::ships_more);
            if more && merged {
                self.feature_merged(project, slug, patch.as_deref());
            }
            if more && waited {
                self.ask_ship_checked(project, slug, true);
            }
            return;
        }
        let again = std::mem::take(&mut probed.again);
        match &probe.reading {
            Err(why) if probed.logged.as_ref() != Some(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                probed.logged = Some(why.clone());
            }
            Err(_) => {}
            Ok(_) => probed.logged = None,
        }
        let patch = probe.reading.as_ref().ok().map(|r| r.patch.clone());
        let heads = (probe.reading.as_ref().ok()).map(|r| (r.heads.clone(), r.at));
        let merged = probe.merged;
        let sits_on = |p: &Probe| (p.reading.as_ref().ok()).map(|r| r.sits_on.clone());
        let was = (probed.last.as_ref()).and_then(sits_on).unwrap_or_default();
        let sits = sits_on(&probe);
        probed.last = Some(probe);
        self.reselect_review_card(selected);
        if let Some(sits) = &sits {
            self.features
                .feature_bases_read(&mut ui!(self), project, slug, &was, sits);
        }
        // The conflicts resolved, the code is the update's: its marks hold.
        if let Some(found) = &resolution {
            self.resolution_read(project, slug, found, patch.as_deref());
        }
        // The fix rounds' diffs take where the code stands.
        if let Some((heads, at)) = heads
            && (self.feature_mut(project, slug)).is_some_and(|f| f.rounds_read(&heads, at))
        {
            self.save_feature(project, slug);
        }
        if let Some(patch) = &patch {
            self.feature_code_is(project, slug, patch);
            self.feature_criteria_are(project, slug);
        }
        if merged {
            self.feature_merged(project, slug, patch.as_deref());
        }
        // At Ship: its summary is written, once.
        let data = store::dir(None, project, slug);
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.summary_tried.contains(&key)
            && (self.feature(project, slug))
                .is_some_and(|f| crate::ship_summary::wants_summary(f, &data))
        {
            self.features.summary_tried.insert(key.clone());
            self.write_ship_summary(project, slug, false);
        }
        if again {
            self.probe_feature(project, slug);
        } else if self.features.ship_waits.remove(&key) {
            self.ask_ship_checked(project, slug, true);
        }
    }

    /// The models feature `slug`'s agents use, as last read: kept with their
    /// runs. A run started since the read isn't the one it was of.
    fn feature_models_are(
        &mut self,
        project: &str,
        slug: &str,
        models: BTreeMap<Stage, (u32, String)>,
    ) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let mut changed = false;
        for (stage, (n, model)) in models {
            if let Some(run) = f.runs.get_mut(&stage).filter(|r| r.n == n)
                && run.model.as_ref() != Some(&model)
            {
                run.model = Some(model);
                changed = true;
            }
        }
        if changed {
            self.save_feature(project, slug);
        }
    }

    /// The criteria of feature `slug` are as last read from its brief:
    /// Manual test, passed on its page, is to do
    /// again when one of them was added or reworded since, and the toast
    /// says so. A Manual test ticked plainly (no results kept) stays.
    fn feature_criteria_are(&mut self, project: &str, slug: &str) {
        let criteria = self.criteria(project, slug).to_vec();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let reopen = feature.status == Status::Open
            && feature.marks.contains_key(&Stage::ManualTest)
            && feature.manual.is_some()
            && !criteria.is_empty()
            && !feature::manual::all_passed(feature.manual.as_ref(), &criteria);
        if !reopen || !feature.untick(Stage::ManualTest, feature::now()) {
            return;
        }
        let title = feature.title.clone();
        tracing::info!("criteria of {project}/{slug} changed: Manual test to do again");
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_warning(format!(
            "The acceptance criteria of \u{201c}{title}\u{201d} changed: back to Manual test"
        ));
    }

    /// The code of feature `slug` is `patch`'s now: the marks that no
    /// longer hold go, and the toast says so. The fixes Manual test asked
    /// for leave them, and Verify runs on them (see `Feature::keep_marks`).
    pub(crate) fn feature_code_is(&mut self, project: &str, slug: &str, patch: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.keep_marks(patch, feature::now()) {
            return self.fixes_landed(project, slug, patch);
        }
        // The code is read after its round is over: its last commit counted.
        if feature.carry.is_some() && feature.fix_round.as_ref().map(|r| r.n) != feature.carry {
            feature.carry = None;
            self.save_feature(project, slug);
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.invalidate(patch, feature::now()) else {
            // Checks held while an agent fixed a review run once it is done.
            return self.pr_code_changed(project, slug, false);
        };
        let pr = feature.pull().is_some();
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_warning(changed_text(&title, stage));
        // No Build agent marks a review's Build done, which starts the checks.
        self.pr_code_changed(project, slug, pr && stage == Stage::Verify);
    }

    /// The squash of feature `slug` is merged: Ship is done.
    fn feature_merged(&mut self, project: &str, slug: &str, patch: Option<&str>) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        // Shipped already, a later squash finishes nothing.
        let finishing = feature.status == Status::Open && feature.current() == Some(Stage::Ship);
        let later = feature.status == Status::Done;
        if !feature.squash_going() || !(finishing || later) {
            return;
        }
        let now = feature::now();
        for key in feature.shipping.squash_repos.clone() {
            feature.ship_ran(feature::ShipDone {
                at: now,
                repo: key,
                step: ShipStep::Merge,
                sha: None,
                url: None,
                undone: None,
            });
        }
        feature.shipping.squash_at = None;
        if finishing {
            feature.tick(now, "gerdr", patch);
        } else {
            feature.log(now, "Squash merged");
        }
        let title = feature.title.clone();
        let bases: Vec<feature::Repo> = (feature.shipping.push_base.iter())
            .filter_map(|key| feature.repo(key).cloned())
            .collect();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        let mut text = format!("\u{201c}{title}\u{201d} is merged: done");
        // Pushed here, not in the tab: a tab closing with the workspace would
        // take a failed push's words with it. What follows waits for it.
        if !bases.is_empty() {
            self.show_success(format!("{text}; pushing"));
            let busy = Busy {
                project: project.to_owned(),
                slug: slug.to_owned(),
                job: Job::Ship,
                repo: None,
                what: format!("Pushing what \u{201c}{title}\u{201d} merged"),
            };
            let (project, slug) = (project.to_owned(), slug.to_owned());
            return self.feature_work(Some(busy), move |_| Done::BasesPushed {
                results: (bases.iter())
                    .map(|repo| (repo.key(), repo.base.clone(), ship::push_base(repo)))
                    .collect(),
                finishing,
                project,
                slug,
            });
        }
        if finishing && let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
    }

    /// The bases feature `slug` merged into went to origin, or some didn't.
    pub(super) fn bases_pushed(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(String, String, Result<String, String>)>,
        finishing: bool,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let now = feature::now();
        let mut failed = None;
        let mut went = Vec::new();
        for (key, base, result) in results {
            match result {
                Ok(sha) => {
                    feature.log(now, format!("Pushed {base} of {key}"));
                    feature.ship_ran(feature::ShipDone {
                        at: now,
                        repo: key.clone(),
                        step: ShipStep::PushBase,
                        sha: Some(sha),
                        url: None,
                        undone: None,
                    });
                    went.push(format!("{base} of {key}"));
                }
                Err(why) => {
                    let text = format!("Couldn't push {base} of {key}: {why}");
                    feature.log(now, text.clone());
                    tracing::warn!("shipping feature {project}/{slug}: {text}");
                    failed.get_or_insert(text);
                }
            }
        }
        self.save_feature(project, slug);
        let kept = finishing.then(|| self.after_ship(project, slug)).flatten();
        if let Some(text) = failed {
            self.features.just_closed = None;
            return self.show_error(format!("\u{201c}{title}\u{201d}: {text}"));
        }
        let mut text = format!("Pushed {} for \u{201c}{title}\u{201d}", went.join(", "));
        if let Some(kept) = kept {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
    }

    /// Read the code of feature `slug` off the thread for run `n` of its
    /// Plan agent, whose file has `stamp` (see `plan_code_checked`).
    pub(crate) fn check_plan_code(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: crate::feature_agents::Stamp,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let there = self.machine_dirt(&project, &slug);
        self.feature_work(None, move |_| Done::PlanChecked {
            result: patch::read(&feature).map(|mut reading| {
                crate::machine_work::with_dirt(&feature, &mut reading, &there);
                reading
            }),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// Mark bound stage `stage` of feature `slug` done once its code is
    /// read off the thread (see `feature_read`).
    pub(crate) fn read_and_tick(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Read,
            repo: None,
            what: format!("Checking \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::Read {
            result: patch::read(&feature),
            project,
            slug,
            stage,
        });
    }

    /// The code of feature `slug` is read to mark `stage` done: done on
    /// that code, unless it has changes not committed, or `stage` is no
    /// longer the current one. AI review is marked by gerdr, once no
    /// finding holds up.
    pub(super) fn feature_read(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        result: Result<Reading, String>,
    ) {
        let reading = match result {
            Ok(reading) => reading,
            Err(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't mark {} done: {why}", stage.label()));
            }
        };
        if !reading.dirty.is_empty() {
            return self.show_warning(format!(
                "Commit the changes in {} first",
                reading.dirty.join(", ")
            ));
        }
        self.feature_code_is(project, slug, &reading.patch);
        let still = self
            .feature(project, slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage));
        let review = self.feature(project, slug).and_then(|f| f.review.as_ref());
        let read_in = review.is_some();
        let open = review.map_or(0, |r| r.open());
        match stage {
            _ if !still => {}
            Stage::AiReview if !read_in => {
                self.show_warning("AI review has no findings read in yet".into());
            }
            Stage::AiReview if open > 0 => {
                let title = self.feature(project, slug).map_or("", |f| f.title.as_str());
                let text = format!(
                    "AI review of \u{201c}{title}\u{201d} still has {}",
                    plural(open, "finding")
                );
                self.show_warning(text);
            }
            Stage::AiReview => self.tick_feature(project, slug, Some(&reading.patch), "gerdr"),
            Stage::ManualTest if !self.criteria(project, slug).is_empty() => {
                let criteria = self.criteria(project, slug).to_vec();
                let passed = self
                    .feature(project, slug)
                    .is_some_and(|f| feature::manual::all_passed(f.manual.as_ref(), &criteria));
                let fixed = (self.feature(project, slug)).is_some_and(|f| f.unverified.is_some());
                if !passed {
                    let comments = criteria.iter().any(|c| c.comment);
                    self.show_warning(match comments {
                        true => "Not every criterion passed and comment is done yet".into(),
                        false => "Not every criterion passed yet; try them first".into(),
                    });
                } else if fixed {
                    // Fixes landed while it was tried: the stages after
                    // Build go over them again first.
                    self.back_to_verify(project, slug);
                } else {
                    let comments = criteria.iter().filter(|c| c.comment).count();
                    let n = criteria.len() - comments;
                    let mut all = match n {
                        0 => String::new(),
                        1 => "its 1 criterion".to_owned(),
                        n => format!("all {n} criteria"),
                    };
                    if comments > 0 {
                        let done = format!("{} done", plural(comments, "comment"));
                        all = match all.is_empty() {
                            true => done,
                            false => format!("{all}, {done}"),
                        };
                    }
                    let said = format!("passed: {all}");
                    self.tick_feature_said(project, slug, Some(&reading.patch), "you", &said);
                    tracing::info!("manual test of {project}/{slug} passed: {n} criteria");
                }
            }
            Stage::Verify => {
                // Marked by hand, as when no checks are set: AI review
                // starts as on a pass, its last run's findings checked again.
                self.tick_feature(project, slug, Some(&reading.patch), "you");
                let reviewing = self.feature(project, slug).is_some_and(|f| {
                    f.status == Status::Open && f.current() == Some(Stage::AiReview)
                });
                if reviewing {
                    self.launch_stage_agent(project, slug, Stage::AiReview, &[]);
                }
            }
            _ => self.tick_feature(project, slug, Some(&reading.patch), "you"),
        }
        self.probe_feature(project, slug);
    }

    /// Mark the current stage of feature `slug` done by `by` ("you",
    /// "gerdr"), on the code whose patch-id is `patch`, and say so.
    pub(super) fn tick_feature(
        &mut self,
        project: &str,
        slug: &str,
        patch: Option<&str>,
        by: &str,
    ) {
        let word = if by == "you" { "approved" } else { "done" };
        self.tick_feature_said(project, slug, patch, by, word);
    }

    /// `tick_feature`, the activity's line saying `said` after the stage.
    fn tick_feature_said(
        &mut self,
        project: &str,
        slug: &str,
        patch: Option<&str>,
        by: &str,
        said: &str,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.tick_said(feature::now(), by, patch, said) else {
            return self.show_warning("Nothing left to mark done".into());
        };
        let toast = done_toast(feature, stage);
        self.save_feature(project, slug);
        self.show_success(toast);
        self.feature_cursor_to_current_of(project, slug);
        self.feature_diff_changed(project, slug);
    }

    pub fn feature_tasks(&self) -> Vec<Task> {
        self.features.feature_tasks(&self.projects)
    }
}
