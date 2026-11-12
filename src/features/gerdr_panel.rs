//! `Gerdr`'s side of the feature panel: showing it, its keys and what they do.

use super::*;

impl Gerdr {
    // ------------------------------------------------------------ panel

    /// The features key: the panel on the active project's features, or
    /// the new feature dialog when it has none.
    pub fn open_features(&mut self) {
        let Some(project) = self.project.clone() else {
            return self.show_warning("Open a project to see its features".into());
        };
        let slugs = self.listed_slugs(&project);
        let open = self.features.panel.as_ref().map(|p| p.slug.as_str());
        let last = self.features.shown.get(&project).map(String::as_str);
        match pick_feature(&slugs, open, last) {
            Some(slug) => self.open_feature(&project, &slug),
            // None open: a shipped one whose worktrees went can come back.
            None if self.open_feature_picker() => {}
            None => self.open_new_feature(None),
        }
    }

    /// Show feature `slug` of project `home` in the panel, entering that
    /// project first when another is shown: a linked project's sidebar row
    /// opens it so.
    pub fn open_linked_feature(&mut self, home: &str, slug: &str) {
        if self.feature(home, slug).is_none() {
            return self.show_warning("That feature is gone".into());
        }
        if self.project.as_deref() == Some(home) {
            return self.open_feature(home, slug);
        }
        self.features.pending = Some((home.to_owned(), slug.to_owned()));
        self.switch_project(home);
    }

    /// Once project `id` was entered: the feature a linked project's row
    /// asked for, if it is that project's.
    pub fn open_pending_feature(&mut self, id: &str) {
        if let Some((home, slug)) = self.features.pending.take()
            && home == id
        {
            self.open_feature(&home, &slug);
        }
    }

    /// Show the feature panel on feature `slug` of `project`: the first
    /// tab of the feature's workspace, which opens if none is, on the
    /// feature's current stage's page. The other panels close: they would
    /// cover it.
    pub fn open_feature(&mut self, project: &str, slug: &str) {
        self.show_feature(project, slug, true);
    }

    /// Back on the feature panel of feature `slug` of `project` from what
    /// covered it, on the page it was left on.
    pub(crate) fn back_to_feature(&mut self, project: &str, slug: &str) {
        self.show_feature(project, slug, false);
    }

    /// See `open_feature`; `to_current`: on its current stage's page even
    /// when it shows already.
    fn show_feature(&mut self, project: &str, slug: &str, to_current: bool) {
        if self.feature(project, slug).is_none() {
            return self.show_warning("That feature is gone".into());
        }
        self.overlays.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.overlays.project_menu = None;
        self.overlays.scrollback = None;
        self.overlays.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.hover.tooltip.hide();
        self.chrome.drop_tooltip();
        // It shows as it was left (its parked panel), on the picked
        // stage while the feature didn't move on.
        let same = |p: &FeaturePanel| p.project == project && p.slug == slug;
        let shown = self.features.panel.as_ref().is_some_and(same);
        if !self.show_feature_tab(project, slug) {
            return;
        }
        // Showing it again where it shows goes back to its current stage.
        if shown
            && to_current
            && let Some(panel) = self.features.panel.as_mut().filter(|p| same(p))
        {
            panel.unpick();
            panel.keys = false;
        }
        self.sync_feature_page();
        // Showing its tab read the feature again, unless it showed already.
        if shown {
            self.probe_feature(project, slug);
        }
    }

    /// Make the feature panel what the shown tab is: the panel of the
    /// feature whose workspace it is when it is that workspace's pinned
    /// tab, else none. A panel left is kept to show as it was when its
    /// tab shows again.
    pub(crate) fn sync_feature_panel(&mut self) {
        let w = self.active_workspace;
        let on_tab = self.tab().is_some_and(crate::Tab::is_feature);
        let shown = (self.project.clone())
            .zip(on_tab.then(|| self.workspace_feature(w)).flatten())
            .map(|(project, f)| (project, f.slug.clone()));
        let open = (self.features.panel.as_ref()).map(|p| (p.project.clone(), p.slug.clone()));
        if open == shown {
            return;
        }
        if let Some(panel) = self.features.panel.take() {
            let key = (panel.project.clone(), panel.slug.clone());
            self.features.parked.insert(key, panel);
        }
        if let Some((project, slug)) = shown {
            let key = (project.clone(), slug.clone());
            let panel = self.features.parked.remove(&key).unwrap_or(FeaturePanel {
                project: project.clone(),
                slug: slug.clone(),
                ..FeaturePanel::default()
            });
            let fresh = panel.sources.is_empty();
            // Coming to show, its review has the keys.
            let mut panel = panel;
            panel.rail_keys = false;
            self.features.panel = Some(panel);
            self.features.shown.insert(project.clone(), slug.clone());
            self.sync_feature_page();
            self.probe_feature(&project, &slug);
            self.watch_feature_tail();
            if fresh && self.feature_writer(&project, &slug).is_none() {
                self.find_repo_sources();
            }
        }
        self.refresh_pointer_focus();
    }

    /// Open the feature panel on feature `slug` of `project` on `stage`'s
    /// page, so what comes next is in sight: the page's button opens a
    /// review's diff. Manual test with criteria has the keyboard on them,
    /// Plan review's suggestions to decide on the first of them open.
    pub fn open_feature_at(&mut self, project: &str, slug: &str, stage: Stage) {
        // Asked for what shows already: it says so, not to look like a
        // click that did nothing.
        let here = (self.tab()).is_some_and(crate::Tab::is_feature)
            && (self.features.panel.as_ref()).is_some_and(|p| {
                p.project == project
                    && p.slug == slug
                    && self
                        .feature(project, slug)
                        .is_some_and(|f| p.page_stage(f) == stage)
            });
        self.open_feature(project, slug);
        if here {
            self.show_notice(format!("{}'s page is the one open", stage.label()));
        }
        let first_open = (stage == Stage::PlanReview)
            .then(|| self.feature(project, slug))
            .flatten()
            .filter(|f| f.status == Status::Open && f.current() == Some(stage))
            .and_then(crate::feature_plan_review::deciding)
            .and_then(|r| Some(r.first_open()?.id.clone()));
        let testing = stage == Stage::ManualTest
            && self
                .feature(project, slug)
                .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage))
            && !self.criteria(project, slug).is_empty();
        let Some(feature) = self.features.panel_feature().cloned() else {
            return;
        };
        let on_it = |p: &&mut FeaturePanel| p.project == project && p.slug == slug;
        if let Some(panel) = self.features.panel.as_mut().filter(on_it) {
            panel.focus = TabArea::Stages;
            panel.keys = false;
            panel.pick(&feature, stage);
        }
        self.sync_feature_page();
        if testing {
            self.focus_criteria(project, slug);
        }
        if let Some(id) = first_open {
            self.select_panel_item(ItemOf::Suggestion, &id);
        }
    }

    pub(crate) fn sync_feature_page(&mut self) {
        self.features.sync_feature_page()
    }

    /// A key while the feature panel has the keyboard.
    pub fn feature_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<PanelOutcome> {
        if self.criterion_note_key(mods, sym, typed) {
            return None;
        }
        if self.page_text_key(mods, sym, typed) {
            return None;
        }
        if self.criteria_search_key(mods, sym, typed) {
            return None;
        }
        let pre = keys::PreCtx {
            feature: self.features.panel_feature().is_some(),
            inline: self.inline_wd().is_some(),
            keys: (self.features.panel.as_ref()).is_some_and(|p| p.keys),
        };
        match keys::pre_key(&pre, mods, sym) {
            // Its first row picked: the arrows, Enter and Esc go on in it.
            Some(keys::PreKey::Menu) => {
                let feature = self.features.panel_feature().cloned()?;
                let ctx = self.row_ctx(&feature);
                self.open_feature_menu(&feature, ctx);
                if let Some(menu) = self.overlays.context_menu.as_mut() {
                    menu.step_to_end(false);
                }
                return None;
            }
            Some(keys::PreKey::Rail) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.rail_keys = false;
                }
                return None;
            }
            Some(keys::PreKey::Page(up)) => {
                self.page_feature_tab(if up { -1.0 } else { 1.0 });
                return None;
            }
            Some(keys::PreKey::Notch(up)) => {
                self.feature_panel_scroll_page(Motion::Notches(if up { -1.0 } else { 1.0 }));
                return None;
            }
            None => {}
        }
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let feature = self.feature(&panel.project, &panel.slug).cloned()?;
        let ctx = self.row_ctx(&feature);
        let plan = self.planned_checks(&feature);
        let key = (feature.project.clone(), feature.slug.clone());
        let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
        let ships = ship_rows(&feature, probe);
        // Ship's buttons, by their keys, while the keys list and notes are
        // away and the page has the keyboard.
        let on = self.features.panel.as_ref()?;
        let quiet = on.keys || on.note.is_some() || on.focus == TabArea::Repos;
        let reviews = feature.pull().is_some();
        if !quiet && on.page_stage(&feature) == Stage::Ship && reviews {
            if let Some(outcome) = submit_key(&feature, mods, sym) {
                return Some(outcome);
            }
        } else if !quiet && on.page_stage(&feature) == Stage::Ship {
            let repo = ship_row_repo(on, &ships);
            let ship = keys::ShipCtx::new(&feature, ctx, probe, repo.as_deref());
            if let Some(outcome) = crate::key_table::press(keys::SHIP_KEYS, &ship, mods, sym) {
                return Some(outcome);
            }
        }
        let notes = notes_of(probe).to_vec();
        let panel = self.features.panel.as_mut()?;
        let before = (panel.focus, panel.repo, panel.expanded);
        let before_stage = panel.stage;
        let card_before = panel.finding;
        let outcome = panel.key(mods, sym, &feature, &slugs, ctx, &plan, &ships, &notes);
        let now = (self.features.panel.as_ref()).map(|p| (p.focus, p.repo, p.expanded));
        if now.is_some_and(|(focus, repo, expanded)| {
            focus == TabArea::Repos && (focus, repo, expanded) != before
        }) {
            self.reveal_feature_repo();
        }
        // A criterion the search hides is stepped over, the way the key went.
        if (self.features.panel.as_ref()).is_some_and(|p| p.focus == TabArea::Findings) {
            let back = (self.features.panel.as_ref()).is_some_and(|p| p.finding < card_before);
            self.settle_criteria(back);
        }
        let card_now = (self.features.panel.as_ref()).map(|p| (p.focus, p.finding));
        if card_now.is_some_and(|(focus, card)| {
            focus == TabArea::Findings && (focus != before.0 || card != card_before)
        }) {
            self.sync_feature_page();
            self.reveal_feature_finding();
        }
        self.see_selected_criterion();
        if now.is_some_and(|(_, _, expanded)| expanded && !before.2) {
            self.find_repo_sources();
        }
        self.sync_feature_page();
        // Walking the rail, a stop at the review's page doesn't take the keys.
        if let Some(panel) = self.features.panel.as_mut()
            && panel.stage != before_stage
        {
            panel.rail_keys = true;
        }
        outcome
    }

    /// Move the feature tab's Page column by `motion`, wherever the
    /// pointer is.
    fn feature_panel_scroll_page(&mut self, motion: Motion) {
        self.sync_feature_page();
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let col = TabColumn::Page;
        if let Some(panel) = self.features.panel.as_mut() {
            panel.scroll[col as usize].apply(motion, Instant::now(), tab_bounds(&layout, col));
        }
    }

    /// Glide the feature tab's page column a page down (`sign` 1) or up
    /// (-1), keeping a little of what showed in view, as PgUp / PgDn do.
    fn page_feature_tab(&mut self, sign: f64) {
        self.sync_feature_page();
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let col = TabColumn::Page;
        let Some(place) = layout.columns[col as usize] else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let by = (place.size.h - PAGE_KEEP).max(place.size.h / 2) as f64;
        let scroll = &mut panel.scroll[col as usize];
        let to = scroll.pos() + sign * by;
        scroll.glide_to(to, Instant::now(), tab_bounds(&layout, col));
    }

    /// Scroll the feature tab's repositories' column to show the one with
    /// the keyboard, as laid out now.
    fn reveal_feature_repo(&mut self) {
        let Some((view, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Repos as usize];
        if let Some(to) = layout.reveal_repo(view.repo, scroll.pos())
            && to != scroll.pos()
        {
            scroll.jump(to);
        }
    }

    /// Scroll the feature tab's page to show the finding with the keyboard,
    /// as laid out now.
    pub(crate) fn reveal_feature_finding(&mut self) {
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Page as usize];
        if let Some(to) = layout.reveal_finding(panel.finding, scroll.pos())
            && to != scroll.pos()
        {
            scroll.jump(to);
        }
    }

    /// Unfold the feature tab's repositories or fold them, reading again
    /// what they can offer when they unfold.
    pub(super) fn toggle_feature_repos(&mut self) {
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        panel.toggle_repos();
        if panel.expanded {
            self.find_repo_sources();
        }
    }

    /// Mark feature `slug`'s current stage done, as its page's Ctrl+Enter
    /// does: a file approved, a stage bound to its output read first.
    pub(crate) fn tick_current(&mut self, project: &str, slug: &str) {
        match self.feature(project, slug).and_then(Feature::current) {
            Some(stage) if needs(stage).is_some() => {
                self.approve_stage(project, slug, stage);
            }
            Some(stage) if stage.bound() => self.read_and_tick(project, slug, stage),
            Some(_) => {
                self.tick_feature(project, slug, None, "you");
                self.probe_feature(project, slug);
            }
            None => self.show_warning("Nothing left to mark done".into()),
        }
    }

    /// Do what the feature panel asked.
    pub fn apply_feature_panel(&mut self, outcome: PanelOutcome) {
        let Some((project, slug)) = self
            .features
            .panel
            .as_ref()
            .map(|p| (p.project.clone(), p.slug.clone()))
        else {
            return;
        };
        if self.feature_writer(&project, &slug).is_some()
            && !outcome.reads_only()
            && !outcome.acts_on_sandbox()
        {
            let press = (outcome != PanelOutcome::TakeOver).then_some(outcome);
            return self.take_feature(&project, &slug, press);
        }
        match outcome {
            // Only offered on what another machine writes, above.
            PanelOutcome::TakeOver => {}
            PanelOutcome::HandBack => self.hand_back(&project, &slug),
            PanelOutcome::Tick => self.tick_current(&project, &slug),
            PanelOutcome::Ship | PanelOutcome::Finish => self.ask_ship_feature(&project, &slug),
            PanelOutcome::ShipRoute(repo, route) => {
                self.set_ship_route(&project, &slug, &repo, route);
            }
            PanelOutcome::ShipTick(repo, step) => {
                self.tick_ship_step(&project, &slug, &repo, step);
            }
            PanelOutcome::ShipAfter(after) => self.tick_ship_after(&project, &slug, after),
            PanelOutcome::SetUp => self.set_up_again(&project, &slug),
            PanelOutcome::SetupLog => self.open_setup_log(&project, &slug),
            PanelOutcome::StartSandbox => self.ask_start_sandbox(&project, &slug),
            PanelOutcome::OpenSandbox => self.open_sandbox_app(&project, &slug, Some(0)),
            PanelOutcome::OpenSandboxPage(i) => self.open_sandbox_app(&project, &slug, Some(i)),
            PanelOutcome::NextSandboxPage => self.open_sandbox_app(&project, &slug, None),
            PanelOutcome::SandboxLan => self.ask_sandbox_lan(&project, &slug),
            PanelOutcome::ShowSandbox => self.show_sandbox_terminals(&project, &slug),
            PanelOutcome::RestartSandbox => {
                let what = crate::feature_sandbox::StopFor::Restart;
                self.finish_stop(&project, &slug, what);
            }
            PanelOutcome::ResetSandbox => self.ask_reset_sandbox(&project, &slug),
            PanelOutcome::StopSandbox => self.ask_stop_sandbox(&project, &slug),
            PanelOutcome::RemoveSandbox => self.ask_remove_sandbox(&project, &slug),
            PanelOutcome::StartAgent(stage, fresh) => {
                self.start_stage_agent(&project, &slug, stage, fresh)
            }
            PanelOutcome::SendPrompt(stage) => self.send_stage_prompt(&project, &slug, stage),
            PanelOutcome::ShowAgent(stage) => self.show_stage_agent(&project, &slug, stage),
            PanelOutcome::ResumeAgent(stage) => self.resume_stage_agent(&project, &slug, stage),
            PanelOutcome::OpenDoc(stage) => {
                self.open_doc_tab(&project, &slug, DocFile::Stage(stage));
            }
            PanelOutcome::ApproveDoc(stage) => self.approve_doc(&project, &slug, stage),
            PanelOutcome::Continue(stage) => self.continue_stage_agent(&project, &slug, stage),
            PanelOutcome::RunChecks => match self.feature(&project, &slug) {
                Some(f) if self.busy_with(&project, &slug, Job::Verify).is_some() => {
                    let text = format!("The checks of \u{201c}{}\u{201d} run already", f.title);
                    self.show_warning(text);
                }
                Some(_) => self.run_verify(&project, &slug),
                None => {}
            },
            PanelOutcome::RunFailed => self.rerun_failed_checks(&project, &slug),
            PanelOutcome::RerunCheck(index) => self.rerun_check(&project, &slug, index),
            PanelOutcome::StopCheck(index) => self.stop_checks(&project, &slug, Some(index)),
            PanelOutcome::StopChecks => self.stop_checks(&project, &slug, None),
            PanelOutcome::ShowCheck(index) => self.show_check(&project, &slug, index),
            PanelOutcome::SkipCheck {
                repo,
                command,
                skip,
            } => self.set_check_skipped(&project, &slug, &repo, &command, skip),
            PanelOutcome::Evidence => self.open_evidence(&project, &slug),
            PanelOutcome::Findings => {
                let first = self
                    .feature(&project, &slug)
                    .and_then(|f| Some(f.review.as_ref()?.first_open()?.id.clone()));
                self.open_feature_diff(&project, &slug, first.as_deref());
            }
            PanelOutcome::SendFixes => self.send_findings(&project, &slug),
            PanelOutcome::MakeFindingFixes => self.make_finding_fixes(&project, &slug),
            PanelOutcome::FixAllFindings => self.fix_all_findings(&project, &slug),
            PanelOutcome::PlanFixes => self.plan_verify_fixes(&project, &slug),
            PanelOutcome::MakeVerifyFixes => self.make_verify_fixes(&project, &slug),
            PanelOutcome::ReviewFixPlan => self.open_doc_tab(&project, &slug, DocFile::FixPlan),
            PanelOutcome::MakeFixes => self.make_fixes_now(&project, &slug),
            PanelOutcome::ResendFixes => self.resend_fixes(&project, &slug),
            PanelOutcome::OpenRoundPlan(n) => self.open_round_plan(&project, &slug, n),
            PanelOutcome::RoundDiff(n) => self.open_round_diff(&project, &slug, n),
            PanelOutcome::PlanChanges => self.open_doc_tab(&project, &slug, DocFile::PlanChanges),
            PanelOutcome::DraftChanges(stage, k) => {
                self.open_doc_tab(&project, &slug, DocFile::DraftChanges(stage, k));
            }
            PanelOutcome::FoldFixRound(n) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.toggle_fix_round(n);
                }
            }
            PanelOutcome::AskFixPlan => self.ask_fix_plan(&project, &slug),
            PanelOutcome::DropFixPlan => self.ask_drop_fix_plan(project.clone(), slug.clone()),
            PanelOutcome::EditCommands(tab) => {
                // With nothing to run yet, what a repository always checks
                // comes first.
                let any = (self.feature(&project, &slug)).is_some_and(|f| {
                    self.row_ctx(f).checks || !f.checks.is_empty() || !f.skip_checks.is_empty()
                });
                let tab = match tab {
                    CommandsTab::Feature if !any => CommandsTab::Checks,
                    tab => tab,
                };
                self.open_project_commands(&project, Some(&slug), tab);
            }
            PanelOutcome::DecideFinding(id, decision) => {
                self.decide_finding(&project, &slug, &id, decision);
            }
            PanelOutcome::AskDismiss(id) => self.ask_dismiss_finding(&project, &slug, &id),
            PanelOutcome::EditFinding(id) => self.ask_edit_finding(&project, &slug, &id),
            PanelOutcome::CommentFinding(id) => self.write_finding_comment(&project, &slug, &id),
            PanelOutcome::DeleteFindingComment(id) => {
                self.delete_finding_comment(&project, &slug, &id);
            }
            PanelOutcome::DeleteOwnFinding(id) => {
                self.delete_own_finding(&project, &slug, &id);
            }
            PanelOutcome::EditDiffNote(c) => self.ask_edit_diff_note(&project, &slug, &c),
            PanelOutcome::DeleteDiffNote(c) => self.delete_diff_note(&project, &slug, &c),
            PanelOutcome::ShowDiffNote(c) => self.show_diff_note(&project, &slug, c),
            PanelOutcome::AddComment => self.ask_add_comment(&project, &slug),
            PanelOutcome::ShowFinding(id) => {
                self.open_feature_diff(&project, &slug, Some(&id));
            }
            PanelOutcome::FindingRefused(why) => self.show_warning(why.to_owned()),
            PanelOutcome::ReviewView(view) => self.show_review_view(view),
            PanelOutcome::FoldRound(n) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.toggle_round(n);
                }
            }
            PanelOutcome::CodeReview => self.open_feature_diff(&project, &slug, None),
            PanelOutcome::ManualTest => self.focus_criteria(&project, &slug),
            PanelOutcome::Manual(outcome) => self.apply_manual_test(outcome),
            PanelOutcome::FoldCriterion(i, open) => self.fold_criterion(i, open),
            PanelOutcome::FoldCard(i, open) => self.features.fold_card(i, open),
            PanelOutcome::FoldCriteria(scope, open) => self.fold_criteria(scope, open),
            PanelOutcome::NoteSave => {
                self.keep_criterion_note();
            }
            PanelOutcome::NoteDiscard => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.note = None;
                }
            }
            PanelOutcome::ReviewInPlan => self.review_in_plan(&project, &slug),
            PanelOutcome::OnSuggestion(id, act) => self.apply_suggestion(&project, &slug, &id, act),
            PanelOutcome::AcceptAllSuggestions => {
                self.accept_all_suggestions(&project, &slug);
            }
            PanelOutcome::AskReviewer => self.ask_reviewer(&project, &slug),
            PanelOutcome::WriteFinalPlan => self.write_final_plan(&project, &slug),
            PanelOutcome::SkipPlanReview => self.ask_skip_plan_review(&project, &slug),
            PanelOutcome::ChangeDecisions => self.ask_change_decisions(&project, &slug),
            PanelOutcome::OpenPr(url) => self.open_feature_page(&project, &slug, url),
            PanelOutcome::UpdatePrHead => self.update_pr_head(&project, &slug),
            PanelOutcome::RerunCi => self.rerun_ci(&project, &slug),
            PanelOutcome::SubmitReview(event) => self.ask_submit_review(&project, &slug, event),
            PanelOutcome::PushPr => self.ask_push_pr(&project, &slug),
            PanelOutcome::FixHere => self.toggle_fix_here(&project, &slug),
            PanelOutcome::EditPulls => self.open_review_edit(&project, &slug),
            PanelOutcome::FinishReview => self.ask_finish_pr_review(&project, &slug),
            PanelOutcome::WriteSummary => self.write_ship_summary(&project, &slug, true),
            PanelOutcome::UndoShip(repo, step) => self.ask_undo_ship(&project, &slug, &repo, step),
            PanelOutcome::UndoAfter(step) => self.undo_after_ship(&project, &slug, step),
            PanelOutcome::UpdateBase(repo, way) => {
                self.ask_update_base(&project, &slug, &repo, way)
            }
            PanelOutcome::PullBase(repo) => self.pull_base(&project, &slug, &repo),
            PanelOutcome::CloseShipped => self.ask_close_shipped(&project, &slug),
            PanelOutcome::ResolveConflicts(repo, planning) => {
                self.resolve_conflicts(&project, &slug, &repo, planning);
            }
            PanelOutcome::ReviewConflictPlan => self.review_conflict_plan(&project, &slug),
            PanelOutcome::StopResolving => self.ask_stop_resolving(&project, &slug),
            PanelOutcome::EditSummary => self.edit_ship_summary(&project, &slug),
            PanelOutcome::PickAgent(stage, name) => {
                self.pick_stage_agent(&project, &slug, stage, &name);
            }
            PanelOutcome::CycleAgent(stage) => self.cycle_stage_agent(&project, &slug, stage),
            PanelOutcome::PickModel(stage) => self.pick_stage_model(&project, &slug, stage),
            PanelOutcome::CycleMachine(stage) => self.cycle_stage_machine(&project, &slug, stage),
            PanelOutcome::StopOnMachine(stage) => {
                self.ask_stop_on_machine(&project, &slug, stage);
            }
            PanelOutcome::FreshMilestones(fresh) => {
                self.set_fresh_milestones(&project, &slug, fresh);
            }
            PanelOutcome::Untick(stage) => self.reopen_stage(&project, &slug, stage),
            PanelOutcome::GoToCurrent => self.feature_cursor_to_current_of(&project, &slug),
            PanelOutcome::StartOver(stage) => self.ask_start_over(&project, &slug, stage),
            PanelOutcome::NotYet(first) => {
                self.show_warning(format!("Mark {} done first", first.label()));
            }
            PanelOutcome::Touch(repo) => self.touch_repo(&project, &slug, repo),
            PanelOutcome::RemoveRepo(key) => {
                let keys = vec![key];
                if self.may_remove_repos(&project, &slug, &keys, false) {
                    self.check_remove_repos(project.clone(), slug.clone(), keys, false);
                }
            }
            PanelOutcome::MakeContext(key) => {
                let keys = vec![key];
                if self.may_remove_repos(&project, &slug, &keys, true) {
                    self.check_remove_repos(project.clone(), slug.clone(), keys, true);
                }
            }
            PanelOutcome::AddToFeature(key) => self.add_repo_to_feature(&project, &slug, &key),
            PanelOutcome::AddContext(key) => self.add_context_repo(&project, &slug, &key),
            PanelOutcome::DropContext(key) => self.drop_context_repo(&project, &slug, &key),
            PanelOutcome::Open(dir) => {
                if !dir.exists() {
                    return self.show_error(format!("{} is gone", short_folder(&dir)));
                }
                self.open_feature_project_tab(&project, &slug, &dir);
            }
            PanelOutcome::CopyPath => {
                if let Some(dir) = self.feature(&project, &slug).map(|f| f.dir.clone()) {
                    self.copy_text(dir.display().to_string());
                    self.show_success(format!("Copied {}", short_folder(&dir)));
                }
            }
            PanelOutcome::Remove => self.check_remove_feature(&project, &slug),
            // On this feature, as it is the one shown.
            PanelOutcome::Stats => self.open_ai_stats(),
            PanelOutcome::New => self.open_new_feature(None),
            // In its own tab, its workspace opened if need be.
            PanelOutcome::Show(to) => self.open_feature(&project, &to),
        }
    }

    /// Show the tab of `stage`'s agent; one gone is forgotten, and Enter
    /// starts a new one.
    pub(crate) fn show_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let in_t3 = (self.feature(project, slug))
            .and_then(|f| f.runs.get(&stage))
            .is_some_and(|run| run.t3.is_some());
        if let (true, Some(writer)) = (in_t3, self.feature_writer(project, slug)) {
            let name = self.machine_name(writer);
            return self.show_warning(format!("That agent runs in T3 Code on {name}"));
        }
        if in_t3 {
            return self.show_t3_stage(project, slug, stage);
        }
        let pane = self
            .feature(project, slug)
            .and_then(|f| f.runs.get(&stage)?.pane.clone());
        if let Some(pane) = pane.filter(|p| self.pane_there(p)) {
            return self.focus_agent(&crate::agents::AgentKey::Herdr(pane));
        }
        if let Some(writer) = self.feature_writer(project, slug) {
            let name = self.machine_name(writer);
            return self.show_warning(format!("{}'s agent isn't running on {name}", stage.label()));
        }
        let mut resumes = false;
        if let Some(run) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&stage))
        {
            run.pane = None;
            resumes = run.resumes();
            self.save_feature(project, slug);
        }
        // Build is locked by the stage that asked for the fix plan, and what
        // comes of its chat is the round's to say.
        let round = stage == Stage::Build
            && (self.feature(project, slug)).is_some_and(|f| f.fix_round.is_some());
        if round {
            return self.show_warning(
                "The Build agent's tab is gone: a fresh agent takes the fix plan on".to_owned(),
            );
        }
        if resumes {
            return self.resume_stage_agent(project, slug, stage);
        }
        self.show_warning(format!(
            "{}'s agent tab is gone; Enter starts a new one",
            stage.label()
        ));
    }

    pub(crate) fn feature_cursor_to_current_of(&mut self, project: &str, slug: &str) {
        self.features.feature_cursor_to_current_of(project, slug)
    }

    /// What repository `key` of `feature` is called on screen.
    pub(super) fn repo_label(&self, feature: &Feature, key: &str) -> String {
        match feature.repo(key) {
            Some(repo) => self.projects.names(feature).label(repo),
            None => key.to_owned(),
        }
    }

    /// What repository `key` of feature `slug` is called on screen, held
    /// by it or not.
    pub(super) fn repo_name(&self, project: &str, slug: &str, key: &str) -> String {
        let Some(feature) = self.feature(project, slug) else {
            return key.to_owned();
        };
        let names = self.projects.names(feature);
        if let Some(repo) = feature.repo(key) {
            return names.label(repo);
        }
        let context = (feature.context.iter()).find(|c| c.key() == key);
        let gone = (feature.removed.iter()).find(|r| r.key() == key);
        match (context, gone) {
            (Some(c), _) => names.label_of(c.project.as_deref(), &c.name),
            (_, Some(r)) => names.label_of(r.project.as_deref(), &r.name),
            _ => key.to_owned(),
        }
    }

    /// Make `repo` (a `Repo::key`) one feature `slug` changes, off the
    /// thread.
    fn touch_repo(&mut self, project: &str, slug: &str, repo: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let doing = self
            .features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo));
        let label = self.repo_label(&feature, &repo);
        if doing {
            return self.show_warning(format!("{label} is being touched already"));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Touch,
            repo: Some(repo.clone()),
            what: format!("Touching {label} for \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let result = layout::touch(&feature, &repo);
            Done::Touched {
                project,
                slug,
                repo,
                result,
            }
        });
    }

    pub(super) fn feature_touched(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        result: Result<feature::Repo, String>,
    ) {
        let label = match self.feature(project, slug) {
            Some(feature) => self.repo_label(feature, repo),
            None => repo.to_owned(),
        };
        let touched = match result {
            Ok(touched) => touched,
            Err(why) => {
                tracing::warn!("touching {repo} for feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't touch {label}: {why}"));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let branch = touched.branch.clone().unwrap_or_else(|| feature.branch());
        match feature.repos.iter_mut().find(|r| r.key() == repo) {
            Some(r) => *r = touched,
            None => feature.repos.push(touched),
        }
        feature.log(feature::now(), format!("touched {repo}"));
        self.save_feature(project, slug);
        self.show_success(format!("{label} is on {branch} now"));
        self.probe_feature(project, slug);
    }
}
