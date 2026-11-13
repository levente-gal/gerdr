//! `Gerdr`'s side of the feature tab: pointer, menus, scrolling, its view and sidebar rows.

use super::*;

impl Gerdr {
    /// The feature tab's view and its layout in the tab's area.
    pub(crate) fn feature_tab_parts(
        &self,
    ) -> Option<(chrome::FeatureTabView, chrome::FeatureTabLayout)> {
        let view = self.feature_tab_view()?;
        let layout = chrome::feature_tab_layout(&view, self.content_rect());
        Some((view, layout))
    }

    /// What is at `pos` on the feature tab.
    fn feature_tab_hit(&self, pos: Point<f64, Logical>) -> Option<FeatureTabHit> {
        let (view, layout) = self.feature_tab_parts()?;
        layout.hit(&view, pos.to_i32_round())
    }

    /// Whether a press of `hit` at `pos` is the second of a double click on
    /// the page's bar, there on another button than the first press took:
    /// the bar's buttons change under the pointer as the page does, and
    /// that one was not aimed at. Any other press of the bar is kept as the
    /// last one.
    fn bar_press_again(&mut self, hit: FeatureTabHit, pos: Point<f64, Logical>) -> bool {
        let label = self.feature_tab_parts().and_then(|(view, layout)| {
            let at = pos.to_i32_round();
            match hit {
                FeatureTabHit::PageButton(i) => (layout.bar_buttons.iter())
                    .any(|(b, r)| *b == i && r.contains(at))
                    .then(|| view.page.buttons.get(i).map(|b| b.label.clone()))?,
                FeatureTabHit::ReviewButton(i) => {
                    Some(view.review_switch?.buttons.get(i)?.label.clone())
                }
                _ => None,
            }
        });
        let (Some(label), Some(panel)) = (label, self.features.panel.as_mut()) else {
            return false;
        };
        let again = (panel.bar_press.as_ref())
            .is_some_and(|last| other_button_again(last, Instant::now(), pos, &label));
        if !again {
            panel.bar_press = Some((Instant::now(), pos, label));
        }
        again
    }

    /// A left click in the feature tab: a stage's node shows its page, a
    /// repository takes the keyboard, a button does what it says.
    pub fn feature_panel_click(&mut self, pos: Point<f64, Logical>) {
        use crate::manual_test::Outcome;
        let Some(feature) = self.features.panel_feature().cloned() else {
            return;
        };
        // A press on the header or the rail: the page it leaves on has the
        // keys, the review if that is what shows.
        if let Some(panel) = self.features.panel.as_mut() {
            panel.rail_keys = false;
        }
        let ctx = self.row_ctx(&feature);
        let Some(hit) = self.feature_tab_hit(pos) else {
            return;
        };
        if self.bar_press_again(hit, pos) {
            return;
        }
        // A press off the search gives the keys back to the page.
        if !matches!(hit, FeatureTabHit::Search(_))
            && let Some(search) = (self.features.panel.as_mut()).and_then(|p| p.search.as_mut())
        {
            search.typing = false;
        }
        let outcome = match hit {
            FeatureTabHit::Tab | FeatureTabHit::Column(_) | FeatureTabHit::AgentPickOff(..) => None,
            FeatureTabHit::MilestoneMode(i) => Some(PanelOutcome::FreshMilestones(i == 1)),
            FeatureTabHit::Fold(i) => {
                let card = (self.feature_tab_parts()).and_then(|(view, _)| {
                    (view.page.sections.into_iter())
                        .filter_map(|s| match s {
                            PageSection::Finding(card) => Some(card),
                            _ => None,
                        })
                        .nth(i)
                });
                match card {
                    Some(card) if card.sheet.is_none() => {
                        Some(PanelOutcome::FoldCard(i, card.collapsed))
                    }
                    _ => (self.criterion_rows(&feature).get(i))
                        .map(|row| PanelOutcome::FoldCriterion(i, !row.open)),
                }
            }
            FeatureTabHit::Search(part) => {
                match part {
                    chrome::SearchPart::Close => self.close_criteria_search(),
                    chrome::SearchPart::Prev | chrome::SearchPart::Next => {
                        let back = part == chrome::SearchPart::Prev;
                        self.go_to_match(back, crate::manual_test::MatchFrom::Here);
                    }
                    _ => self.open_criteria_search(),
                }
                None
            }
            // The review's own: pressed as on its title row in the popup.
            FeatureTabHit::ReviewButton(i) => {
                let button = (self.feature_tab_parts())
                    .and_then(|(view, _)| Some(view.review_switch?.buttons.get(i)?.button));
                if let Some(button) = button {
                    let hit = chrome::ReposHit::Button(button);
                    self.worktree_diff_hit(hit, crate::worktree_diff::DiffClick::default());
                }
                return;
            }
            FeatureTabHit::ReviewView(i) => Some(PanelOutcome::ReviewView(match i {
                0 => chrome::ReviewView::Code,
                _ => chrome::ReviewView::Fixes,
            })),
            FeatureTabHit::ShipRoute(row, segment) => self
                .features
                .ship_row_at(&feature, row)
                .and_then(|row| match row {
                    ShipRow::Route { repo, .. } => {
                        Some(PanelOutcome::ShipRoute(repo, *Route::ALL.get(segment)?))
                    }
                    _ => None,
                }),
            FeatureTabHit::ShipRow(row) => {
                self.features
                    .ship_row_at(&feature, row)
                    .and_then(|at| match at {
                        ShipRow::Route { .. } => None,
                        at => Some(at.outcome()),
                    })
            }
            // A summary's pill: its tag's first card takes the keys, in view.
            FeatureTabHit::Jump(tag) => {
                let key = (feature.project.clone(), feature.slug.clone());
                let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
                let notes = notes_of(probe).to_vec();
                if let Some(panel) = self.features.panel.as_mut() {
                    let stage = panel.page_stage(&feature);
                    let cards = review_cards(&feature, stage, &notes);
                    if let Some(i) = cards.iter().position(|c| card_tag(*c) == tag) {
                        panel.focus = TabArea::Findings;
                        panel.finding = i;
                    }
                }
                self.sync_feature_page();
                self.reveal_feature_finding();
                None
            }
            FeatureTabHit::Finding(i) => {
                let key = (feature.project.clone(), feature.slug.clone());
                let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
                let notes = notes_of(probe).to_vec();
                // A card selected opens, and the one left closes: the card
                // clicked stays under the pointer.
                let top = |this: &Self| {
                    let (_, layout) = this.feature_tab_parts()?;
                    Some(layout.finding_cards.get(i)?.loc.y)
                };
                let before = top(self);
                if let Some(panel) = self.features.panel.as_mut() {
                    // An earlier review's card is a record: no keys to give it.
                    let stage = panel.page_stage(&feature);
                    let has_records = matches!(stage, Stage::AiReview | Stage::CodeReview);
                    if !has_records || i < review_cards(&feature, stage, &notes).len() {
                        panel.focus = TabArea::Findings;
                        panel.finding = i;
                    }
                }
                if let (Some(before), Some(after)) = (before, top(self))
                    && before != after
                {
                    self.shift_feature_page(f64::from(after - before));
                }
                None
            }
            FeatureTabHit::AgentPick(stage, i) => {
                let keys = self.features.panel.as_ref().is_none_or(|p| p.keys);
                let mut chips = self.chips_of(&feature.project, &feature.slug, stage);
                (!keys && i < chips.len())
                    .then(|| PanelOutcome::PickAgent(stage, chips.swap_remove(i).name))
            }
            FeatureTabHit::AgentModel(stage) => (self.features.panel.as_ref())
                .filter(|p| !p.keys)
                .map(|_| PanelOutcome::PickModel(stage)),
            FeatureTabHit::Hint(i) => {
                let keys = self
                    .feature_tab_view()
                    .and_then(|v| v.hints.get(i).cloned());
                if let Some((keys, _)) = keys {
                    self.press_hint(crate::input::Overlay::Features, &keys, pos);
                }
                None
            }
            FeatureTabHit::Prev | FeatureTabHit::Next => {
                let slugs = self.listed_slugs(&feature.project);
                let back = hit == FeatureTabHit::Prev;
                switch(&slugs, &feature.slug, back).map(PanelOutcome::Show)
            }
            FeatureTabHit::Node(i) => {
                if let (Some(panel), Some(stage)) =
                    (self.features.panel.as_mut(), Stage::ALL.get(i))
                {
                    panel.focus = TabArea::Stages;
                    panel.keys = false;
                    panel.pick(&feature, *stage);
                }
                None
            }
            FeatureTabHit::ToggleRepos => {
                self.toggle_feature_repos();
                None
            }
            // Work running goes to its stage's page.
            FeatureTabHit::Running(i) => {
                let (rows, _) =
                    self.features
                        .running_of(&feature.project, &feature.slug, &feature.title);
                let stage = rows.get(i).and_then(|(row, _)| Stage::ALL.get(row.node?));
                if let (Some(panel), Some(stage)) = (self.features.panel.as_mut(), stage) {
                    panel.focus = TabArea::Stages;
                    panel.keys = false;
                    panel.pick(&feature, *stage);
                }
                None
            }
            FeatureTabHit::RunningButton(i, b) => {
                let (rows, _) =
                    self.features
                        .running_of(&feature.project, &feature.slug, &feature.title);
                rows.get(i).and_then(|(_, job)| {
                    let mut buttons = running_buttons(*job);
                    (b < buttons.len()).then(|| buttons.swap_remove(b).1)
                })
            }
            // A chip unfolds the rows, on its own.
            FeatureTabHit::Repo(i) => {
                if (self.features.panel.as_ref()).is_some_and(|p| !p.expanded) {
                    self.toggle_feature_repos();
                }
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.select_row(i);
                }
                None
            }
            FeatureTabHit::MoreRepos(i) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.select_row(i);
                }
                None
            }
            FeatureTabHit::RepoButton(i, b) => {
                // Its row takes the keyboard too.
                let panel = self.features.panel.as_mut();
                panel.and_then(|panel| {
                    panel.select_row(i);
                    panel.repo_pick = Some(b);
                    let entries = repo_entries(&feature, &panel.sources, panel.expanded);
                    let mut buttons = entry_buttons(&feature, &panel.sources, *entries.get(i)?);
                    (b < buttons.len()).then(|| buttons.swap_remove(b).1)
                })
            }
            FeatureTabHit::RepoRemove(i) => {
                let panel = self.features.panel.as_mut();
                panel.and_then(|panel| {
                    panel.select_row(i);
                    let entries = repo_entries(&feature, &panel.sources, panel.expanded);
                    match entries.get(i)? {
                        RepoEntry::Held(i) => {
                            Some(PanelOutcome::RemoveRepo(feature.repos.get(*i)?.key()))
                        }
                        RepoEntry::Context(i) => {
                            Some(PanelOutcome::DropContext(feature.context.get(*i)?.key()))
                        }
                        _ => None,
                    }
                })
            }
            FeatureTabHit::PageButton(i) => {
                // A button on a card selects that card first: the keys and
                // the footer then aim at what was pressed.
                let card = (self.feature_tab_parts()).and_then(|(view, _)| {
                    (view.page.sections.into_iter())
                        .filter_map(|s| match s {
                            PageSection::Finding(card) => Some(card),
                            _ => None,
                        })
                        .position(|c| c.buttons.contains(&i) || c.comment_buttons.contains(&i))
                });
                if let (Some(at), Some(panel)) = (card, self.features.panel.as_mut()) {
                    panel.focus = TabArea::Findings;
                    panel.finding = at;
                }
                let stage = (self.features.panel.as_ref())
                    .filter(|p| !p.keys)
                    .map(|p| p.page_stage(&feature));
                stage.and_then(|stage| {
                    let mut actions = self.page_button_actions(&feature, stage, ctx);
                    (i < actions.len()).then(|| actions.swap_remove(i).1)
                })
            }
            FeatureTabHit::Button(_) => {
                self.open_feature_menu(&feature, ctx);
                return;
            }
        };
        self.sync_feature_page();
        // A note being written: a click in it goes on writing, as does its own
        // card's button; one anywhere else keeps it first, and if it can't be
        // kept yet it waits.
        if self.writing_criterion_note() {
            // Its own Save and Discard act on it as they say.
            if let Some(o @ (PanelOutcome::NoteSave | PanelOutcome::NoteDiscard)) = &outcome {
                return self.apply_feature_panel(o.clone());
            }
            let spot = self.feature_tab_view().and_then(|view| {
                chrome::feature_tab_field(&view, self.content_rect(), self.scale())
            });
            let in_it = spot.is_some_and(|field| field.area.to_f64().contains(pos));
            let at = (self.features.panel.as_ref()).and_then(|p| p.note.as_ref().map(|n| &n.0));
            let own = match (&outcome, at) {
                (Some(PanelOutcome::Manual(Outcome::Write(i))), Some(CardNote::Criterion(at))) => {
                    i == at
                }
                (
                    Some(PanelOutcome::CommentFinding(id)),
                    Some(CardNote::Item {
                        of: ItemOf::Finding,
                        id: at,
                        ..
                    }),
                ) => id == at,
                (
                    Some(PanelOutcome::OnSuggestion(id, ItemAct::Comment | ItemAct::Answer)),
                    Some(CardNote::Item {
                        of: ItemOf::Suggestion,
                        id: at,
                        ..
                    }),
                ) => id == at,
                _ => false,
            };
            let picked = (self.features.panel.as_ref()).map(|p| p.finding);
            if in_it || own || !self.keep_criterion_note() {
                return;
            }
            // Keeping it selects its card: the click was for another.
            if let Some(panel) = self.features.panel.as_mut()
                && matches!(hit, FeatureTabHit::Finding(_))
            {
                panel.finding = picked.unwrap_or(panel.finding);
            }
        }
        if let Some(outcome) = outcome {
            self.apply_feature_panel(outcome);
        }
        self.see_selected_criterion();
    }

    /// Finding `id` of the page's stage has the keyboard, and shows.
    pub(crate) fn select_panel_finding(&mut self, id: &str) {
        self.select_panel_item(ItemOf::Finding, id);
    }

    /// Item `id` of kind `of` on the page's stage has the keyboard, and
    /// shows.
    pub(crate) fn select_panel_item(&mut self, of: ItemOf, id: &str) {
        let at = self
            .features
            .with_review_cards(|cards, _| {
                (cards.iter()).position(|c| c.item().is_some_and(|i| i.of() == of && i.id() == id))
            })
            .flatten();
        if let Some(at) = at {
            self.select_criterion(at);
        }
    }

    pub(crate) fn sent_notes(&self, project: &str, slug: &str) -> Vec<Comment> {
        self.features.sent_notes(project, slug)
    }

    /// The comments feature `slug`'s diff holds are `notes` now, without
    /// waiting for its next probe.
    pub(crate) fn notes_are(&mut self, project: &str, slug: &str, notes: Vec<Comment>) {
        let key = (project.to_owned(), slug.to_owned());
        if let Some(counts) = self.code_reviews.get_mut(&key) {
            counts.comments = notes.len();
        }
        let last = (self.features.probes.get_mut(&key)).and_then(|p| p.last.as_mut());
        if let Some(probe) = last {
            probe.docs.comments = notes.len();
            // Those that went to Build stay, after them.
            let sent = probe
                .docs
                .notes
                .iter()
                .filter(|c| c.flag.is_some())
                .cloned();
            let sent: Vec<Comment> = sent.collect();
            probe.docs.notes = notes;
            probe.docs.notes.extend(sent);
        }
    }

    pub(crate) fn selected_review_card(&self) -> Option<CardId> {
        self.features.selected_review_card()
    }

    pub(crate) fn reselect_review_card(&mut self, id: Option<CardId>) {
        self.features.reselect_review_card(id)
    }

    /// The keyboard goes from finding `id`, just decided, to the next card
    /// left to decide, if there is one.
    pub(crate) fn select_next_open_after(&mut self, id: &str) {
        self.select_next_open_item(ItemOf::Finding, id);
    }

    /// The keyboard goes from item `id` of kind `of`, just decided, to the
    /// next card left to decide, if there is one.
    pub(crate) fn select_next_open_item(&mut self, of: ItemOf, id: &str) {
        let next = self
            .features
            .with_review_cards(|cards, _| {
                let at = (cards.iter())
                    .position(|c| c.item().is_some_and(|i| i.of() == of && i.id() == id))?;
                let next = next_open_card(cards, at);
                (next != at).then_some(next)
            })
            .flatten();
        if let Some(next) = next {
            self.select_criterion(next);
        }
    }

    /// Scroll the feature tab's page `by` further, within what it can.
    fn shift_feature_page(&mut self, by: f64) {
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let room = f64::from(layout.scroll_room(TabColumn::Page));
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Page as usize];
        let to = (scroll.pos() + by).clamp(0.0, room);
        scroll.jump(to);
    }

    /// The header's `⋯` menu: the page's rare buttons (`StagePage::menu`), then
    /// what acts on the feature as a whole, removing it last.
    /// The key of the `⋯` menu's row asking `outcome`, shown at its right:
    /// the tab's key asking it now, as on the rail.
    fn menu_chord(
        &self,
        feature: &Feature,
        ctx: RowCtx,
        outcome: &PanelOutcome,
    ) -> Option<&'static str> {
        if *outcome == PanelOutcome::Stats {
            // The prefix's key (`keys::ACTIONS`), not the tab's.
            return Some("Prefix, then U");
        }
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let plan = self.planned_checks(feature);
        let key = (feature.project.clone(), feature.slug.clone());
        let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
        let ships = ship_rows(feature, probe);
        let stage = panel.page_stage(feature);
        let notes = notes_of(probe);
        let t = keys::TabCtx::new(
            panel, feature, stage, &slugs, ctx, &plan, &ships, notes, None,
        );
        t.on_rail().chord_of(outcome)
    }

    pub(super) fn open_feature_menu(&mut self, feature: &Feature, ctx: RowCtx) {
        use crate::menu::{MenuAction, MenuItem, MenuSpec};
        let Some((view, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(anchor) = layout.buttons.first().copied() else {
            return;
        };
        let item = |label: &str, outcome: PanelOutcome| {
            MenuItem::new(label, MenuAction::FeatureOutcome(Box::new(outcome)))
        };
        let mut items = Vec::new();
        let stage = (self.features.panel.as_ref()).map(|p| p.page_stage(feature));
        let actions = stage.map_or_else(Vec::new, |stage| {
            self.page_button_actions(feature, stage, ctx)
        });
        let menu_key = |outcome: &PanelOutcome| self.menu_chord(feature, ctx, outcome);
        for i in &view.page.menu {
            if let (Some(b), Some((_, outcome))) = (view.page.buttons.get(*i), actions.get(*i)) {
                let on = b.style != ButtonStyle::Off;
                let mut row = item(&b.label, outcome.clone()).enabled(on);
                if let Some(key) = menu_key(outcome) {
                    row = row.hint(key);
                }
                items.push(row);
            }
        }
        if !view.remove.is_empty() {
            let outcomes = [
                PanelOutcome::Open(feature.dir.clone()),
                PanelOutcome::CopyPath,
                PanelOutcome::Stats,
                PanelOutcome::New,
                PanelOutcome::Remove,
            ];
            let last = chrome::FEATURE_BUTTONS.len();
            let labels = chrome::FEATURE_BUTTONS.iter().copied().chain([view.remove]);
            for (i, (label, o)) in labels.zip(outcomes).enumerate() {
                // A rule sets the page's rows apart, and the one removing
                // the feature from those that only open something.
                if (i == 0 && !items.is_empty()) || i == last {
                    items.push(MenuItem::note(""));
                }
                let key = menu_key(&o);
                let mut row = item(label, o);
                if let Some(key) = key {
                    row = row.hint(key);
                }
                // Abandon loses work; removing a shipped feature's
                // worktrees doesn't, so that row is plain.
                if i == last && label == "Abandon" {
                    row = row.danger();
                }
                items.push(row);
            }
        }
        if !items.is_empty() {
            self.open_menu(MenuSpec { anchor, items });
        }
    }

    /// The pointer moved while the feature tab shows: what is under it
    /// lights up, and a stage's node tells what is done in it.
    /// The buttons of `stage`'s page of `feature`, in the order the page
    /// lays them out, with what each asks.
    fn page_button_actions(
        &self,
        feature: &Feature,
        stage: Stage,
        ctx: RowCtx,
    ) -> Vec<(PageButton, PanelOutcome)> {
        let plan = self.planned_checks(feature);
        let rows = self.criterion_rows(feature);
        let key = (feature.project.clone(), feature.slug.clone());
        let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
        let notes = notes_of(probe);
        let mut actions = stage_actions(feature, stage, ctx, &plan, &rows, notes);
        actions.retain(|(_, o)| !undoes_merged(o, probe));
        actions.extend(round_plan_actions(feature, stage, probe));
        let place = self.place_label(feature, stage);
        actions.extend(place_action(place.as_deref(), stage));
        actions.extend(stop_action(feature, stage));
        with_pull_actions(&mut actions, feature, stage, ctx);
        let writing = (self.features.panel.as_ref()).is_some_and(|p| p.note.is_some() && !p.keys);
        actions.extend(editing_actions(writing));
        actions
    }

    /// What page button `i` of Ship's page does and its key, while a key
    /// presses it as things stand.
    fn ship_button_tip(&self, i: usize) -> Option<String> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        if panel.keys || panel.page_stage(feature) != Stage::Ship {
            return None;
        }
        let ctx = self.row_ctx(feature);
        let outcome = self
            .page_button_actions(feature, Stage::Ship, ctx)
            .get(i)?
            .1
            .clone();
        let key = (feature.project.clone(), feature.slug.clone());
        let probe = (self.features.probes.get(&key)).and_then(|p| p.last.as_ref());
        let repo = ship_row_repo(panel, &ship_rows(feature, probe));
        let keys = ship_keys(feature, ctx, probe, repo.as_deref());
        let key = keys.into_iter().find(|k| k.outcome == outcome)?;
        let mut what = key.what.chars();
        let first = what.next()?.to_uppercase();
        Some(format!("{}{} ({})", first, what.as_str(), key.chord))
    }

    pub fn feature_panel_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        if let Some(wd) = self.inline_wd_mut() {
            wd.hover = None;
        }
        let (view, layout) = self.feature_tab_parts()?;
        let hit = layout.hit(&view, pos.to_i32_round());
        if let Some(panel) = self.features.panel.as_mut() {
            panel.tab_hover = hit;
        }
        match hit? {
            FeatureTabHit::Node(i) => {
                let stage = Stage::ALL.get(i)?;
                let feature = self.features.panel_feature()?;
                let text = match feature.marks.get(stage) {
                    Some(mark) => mark_detail(mark, feature::now()),
                    None => stage.about().to_owned(),
                };
                Some(Tooltip::new(&text, *layout.nodes.get(i)?))
            }
            // An icon alone: what it opens.
            FeatureTabHit::Button(i) => {
                Some(Tooltip::new(chrome::MORE_ACTIONS, *layout.buttons.get(i)?))
            }
            FeatureTabHit::Jump(tag) => {
                let stage = self
                    .features
                    .panel
                    .as_ref()?
                    .page_stage(self.features.panel_feature()?);
                let w = words(stage_items(stage));
                let text = match tag.word()? {
                    "yours" => format!("Go to your first {}", w.item),
                    word => format!("Go to the first {word} {}", w.item),
                };
                Some(Tooltip::new(
                    &text,
                    smithay::utils::Rectangle::new(pos.to_i32_round(), (1, 1).into()),
                ))
            }
            FeatureTabHit::Fold(i) => {
                let open = self
                    .features
                    .panel_feature()
                    .map(|f| self.criterion_rows(f))?
                    .get(i)?
                    .open;
                let text = match open {
                    true => "Fold it to its line (Left)",
                    false => "Open it to its steps (Right)",
                };
                Some(Tooltip::new(
                    text,
                    smithay::utils::Rectangle::new(pos.to_i32_round(), (1, 1).into()),
                ))
            }
            FeatureTabHit::Search(part) => {
                let spot = layout.search_part(&view, part)?;
                let text = match part {
                    chrome::SearchPart::Close => "Close the search (Esc)",
                    chrome::SearchPart::Field => return None,
                    chrome::SearchPart::Prev => "Previous match (Shift+N)",
                    chrome::SearchPart::Next => "Next match (N)",
                    _ => "Search the criteria (/ or Ctrl+F)",
                };
                Some(Tooltip::new(text, spot))
            }
            // The fold buttons over the criteria: what they fold, and the key.
            FeatureTabHit::PageButton(i) => {
                let text = fold_tip(&view.page, i)
                    .or_else(|| mark_tip(&view.page, i))
                    .or_else(|| self.ship_button_tip(i))
                    .or_else(|| {
                        let feature = self.features.panel_feature()?;
                        let panel = self.features.panel.as_ref()?;
                        pull_tip(feature, panel.page_stage(feature), &view.page, i)
                    })?;
                // On the page's bar, not scrolled; a card may show it too,
                // so the one under the pointer.
                let at = pos.to_i32_round();
                let bar = (layout.bar_buttons.iter()).find(|(b, r)| *b == i && r.contains(at));
                let spot = match (bar, layout.page_buttons.get(i).copied().flatten()) {
                    (Some((_, spot)), _) => *spot,
                    (None, Some(mut spot)) => {
                        spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                        spot
                    }
                    (None, None) => (layout.bar_buttons.iter().find(|(b, _)| *b == i))?.1,
                };
                Some(Tooltip::new(&text, spot))
            }
            FeatureTabHit::ToggleRepos => {
                let text = match view.expanded {
                    true => "Hide the repositories (Ctrl+Shift+L)",
                    false => "Show all repositories (Ctrl+Shift+L)",
                };
                Some(Tooltip::new(text, layout.repos_toggle))
            }
            // What joining or leaving makes of the repository.
            FeatureTabHit::RepoButton(i, b) => {
                let feature = self.features.panel_feature()?;
                let panel = self.features.panel.as_ref()?;
                let entries = repo_entries(feature, &panel.sources, panel.expanded);
                let buttons = entry_buttons(feature, &panel.sources, *entries.get(i)?);
                let text = repo_button_tip(feature, &buttons.get(b)?.1)?;
                let mut spot = *layout.repo_buttons.get(i)?.get(b)?;
                spot.loc.y -= view.scroll[TabColumn::Repos as usize].round() as i32;
                Some(Tooltip::new(&text, spot))
            }
            // Logos alone: their names.
            FeatureTabHit::AgentPick(stage, i) | FeatureTabHit::AgentPickOff(stage, i) => {
                // On Code review's bar, unscrolled; else on the page.
                let on_bar = (layout.review_switch.as_ref())
                    .and_then(|s| s.picks.as_ref())
                    .filter(|_| stage == Stage::Build && view.review_page);
                let spot = match on_bar {
                    Some((_, chips)) => *chips.get(i)?,
                    None => {
                        let (_, _, chips) =
                            layout.agent_picks.iter().find(|(s, ..)| *s == stage)?;
                        let mut spot = *chips.get(i)?;
                        spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                        spot
                    }
                };
                let chip = view.agent_chip(stage, i)?;
                let text = match chip.off {
                    true => format!("{} built the code, so it can't review it", chip.name),
                    false => chip.name.clone(),
                };
                Some(Tooltip::new(&text, spot))
            }
            FeatureTabHit::AgentModel(stage) => {
                let on_bar = (layout.review_switch.as_ref())
                    .and_then(|s| s.model)
                    .filter(|_| stage == Stage::Build && view.review_page);
                let spot = match on_bar {
                    Some(spot) => spot,
                    None => {
                        let (_, mut spot) =
                            *layout.agent_model.iter().find(|(s, _)| *s == stage)?;
                        spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                        spot
                    }
                };
                // Its key acts on this card, or on it from the findings.
                let feature = self.features.panel_feature()?;
                let page = self.features.panel.as_ref()?.page_stage(feature);
                let keyed = |findings| keys::keyed_stage(feature, page, findings) == Some(stage);
                let key = match (keyed(false), keyed(true)) {
                    (true, _) => format!(" ({MODEL_KEY})"),
                    (false, true) => format!(" ({MODEL_KEY} from the findings)"),
                    _ => String::new(),
                };
                let text = match stage == Stage::Build && page != Stage::Build {
                    true => format!(
                        "The model the next fixes start on; an agent already open keeps its own{key}"
                    ),
                    false => format!(
                        "The model its next start uses; an agent already open keeps its own{key}"
                    ),
                };
                Some(Tooltip::new(&text, spot))
            }
            FeatureTabHit::ReviewView(i) => {
                let spots = layout.review_switch.as_ref()?;
                let spot = *spots.segments.get(i)?;
                let text = match i {
                    0 => "The code and its comments (Alt+1)",
                    _ => "The fixes the comments asked for, and their plan (Alt+2)",
                };
                Some(Tooltip::new(text, spot))
            }
            // What holds it up, else what it does and its key.
            FeatureTabHit::ReviewButton(i) => {
                let spot = (*layout.review_switch.as_ref()?.buttons.get(i)?)?;
                let b = view.review_switch.as_ref()?.buttons.get(i)?;
                let text = match (&b.disabled, b.key.is_empty()) {
                    (Some(why), _) => why.clone(),
                    (None, true) => b.label.clone(),
                    (None, false) => format!("{} ({})", b.label, b.key),
                };
                Some(Tooltip::new(&text, spot))
            }
            FeatureTabHit::MilestoneMode(i) => {
                let (_, segments) = layout.milestone_mode.as_ref()?;
                let mut spot = *segments.get(i)?;
                spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                let text = match i {
                    0 => "One agent keeps its chat through all milestones",
                    _ => "A fresh agent builds each milestone, reading the build log and plan",
                };
                Some(Tooltip::new(text, spot))
            }
            FeatureTabHit::ShipRoute(row, segment) => {
                let (_, _, segments) = (layout.ship.routes.iter()).find(|r| r.0 == row)?;
                let mut spot = *segments.get(segment)?;
                spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                let text = match segment {
                    0 => "Through origin, for a review",
                    1 => "Into the base in its main worktree; nothing leaves it unless ticked",
                    _ => "It stays as it is",
                };
                Some(Tooltip::new(text, spot))
            }
            FeatureTabHit::Prev | FeatureTabHit::Next => {
                let (_, prev, next) = layout.switch?;
                Some(match hit? {
                    FeatureTabHit::Prev => Tooltip::new("Previous feature (Ctrl+PgUp)", prev),
                    _ => Tooltip::new("Next feature (Ctrl+PgDn)", next),
                })
            }
            _ => None,
        }
    }

    /// Whether something that acts is at `pos` on the feature tab: the
    /// pointer is a hand over it.
    pub fn feature_tab_control_at(&self, pos: Point<f64, Logical>) -> bool {
        !matches!(
            self.feature_tab_hit(pos),
            None | Some(
                FeatureTabHit::Tab
                    | FeatureTabHit::Column(_)
                    | FeatureTabHit::AgentPickOff(..)
                    | FeatureTabHit::Finding(_)
            )
        )
    }

    /// The column of the feature tab at `pos`, as laid out now, and what
    /// it scrolls within.
    fn feature_tab_column(&self, pos: Point<f64, Logical>) -> Option<(TabColumn, Bounds)> {
        let (_, layout) = self.feature_tab_parts()?;
        let col = layout.column_at(pos.to_i32_round())?;
        Some((col, tab_bounds(&layout, col)))
    }

    /// The wheel or the touchpad over the feature tab: the column under
    /// the pointer moves.
    /// Over the narrow tab's row of repository chips, the wheel steps
    /// through the repositories instead. What is under the pointer is
    /// found again once the column stops (see `tick_scrolls`).
    pub fn feature_panel_scroll(&mut self, motion: Motion) {
        self.sync_feature_page();
        let pos = self.mouse.pos.to_i32_round();
        let on_strip = (self.feature_tab_parts()).is_some_and(|(_, l)| {
            l.columns[TabColumn::Repos as usize].is_none() && l.strip.contains(pos)
        });
        if on_strip {
            return self.feature_strip_scroll(motion);
        }
        if let Some((col, bounds)) = self.feature_tab_column(self.mouse.pos)
            && let Some(panel) = self.features.panel.as_mut()
        {
            panel.scroll[col as usize].apply(motion, Instant::now(), bounds);
        }
        // What is under the pointer is found again now, and once the
        // column stops (see `tick_scrolls`): a tooltip left goes.
        self.mouse.scrolling = true;
        let time = self.start_time.elapsed().as_millis() as u32;
        self.pointer_moved(self.mouse.pos, time);
    }

    /// The wheel over the narrow tab's repository chips: the next one down
    /// (or up) takes the keyboard.
    fn feature_strip_scroll(&mut self, motion: Motion) {
        let sources = (self.features.panel.as_ref()).map_or(&[][..], |p| p.sources.as_slice());
        let Some(repos) = self
            .features
            .panel_feature()
            .map(|f| repo_entries(f, sources, false).len())
        else {
            return;
        };
        let rows = self.motion_rows(motion, TAB_NOTCH);
        if rows == 0 || repos == 0 {
            return;
        }
        if let Some(panel) = self.features.panel.as_mut() {
            panel.select_row(strip_step(panel.repo, rows, repos));
        }
        let time = self.start_time.elapsed().as_millis() as u32;
        self.pointer_moved(self.mouse.pos, time);
    }

    /// Move the feature tab's columns on to `now` if they move on their
    /// own; whether one still does.
    pub fn feature_tab_scroll_tick(&mut self, now: Instant) -> bool {
        // Every frame: a page shown afresh (picked, or the feature moved
        // on) starts at its top.
        self.sync_feature_page();
        let moving = (self.features.panel.as_ref()).is_some_and(|p| {
            p.scroll
                .iter()
                .any(|s| s.moving(now) || s.overscroll() != 0.0)
        });
        if !moving {
            return false;
        }
        let Some((_, layout)) = self.feature_tab_parts() else {
            return false;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return false;
        };
        let mut still = false;
        for col in TabColumn::ALL {
            still |= panel.scroll[col as usize].tick(now, tab_bounds(&layout, col));
        }
        still
    }

    pub fn feature_tab_stop_fling(&mut self) -> bool {
        self.features.feature_tab_stop_fling()
    }

    /// The feature tab's view, while a feature's tab shows.
    pub fn feature_tab_view(&self) -> Option<chrome::FeatureTabView> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let slugs = self.listed_slugs(&panel.project);
        let mine = || {
            self.features
                .busy
                .iter()
                .filter(|b| b.project == panel.project && b.slug == panel.slug)
        };
        let busy: Vec<&str> = mine().filter_map(|b| b.repo.as_deref()).collect();
        let shipping = mine().any(|b| b.job == Job::Ship);
        let doing = |job: Job| mine().find(|b| b.job == job).map(|b| b.what.as_str());
        let key = (panel.project.clone(), panel.slug.clone());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        let now = feature::now();
        let current = feature.current().filter(|s| needs(*s).is_some());
        let run = current.and_then(|stage| Some((stage, feature.runs.get(&stage)?)));
        let agent_status = run.and_then(|(_, run)| self.stage_run_state(run).0);
        let mut agent_read = false;
        let agent = run.map(|(stage, run)| {
            let watch_key = (panel.project.clone(), panel.slug.clone(), stage);
            let watch = self.features.agents.get(&watch_key);
            let run = crate::feature_agents::as_read(feature, stage, run);
            agent_read = crate::feature_agents::gerdr_reads(watch, &run);
            crate::feature_agents::agent_line(&run, stage, agent_status, watch, now)
        });
        let manual_counts = self.manual_counts(feature);
        let pane = run.and_then(|(_, run)| run.pane.as_deref());
        let tail = (self.features.tail.as_ref())
            .filter(|t| Some(t.pane.as_str()) == pane)
            .map_or(&[][..], |t| t.lines.as_slice());
        let state = PanelState {
            busy: &busy,
            names: self.projects.names(feature),
            probe,
            shipping,
            agent,
            agent_status,
            agent_standing: self.agent_standing_of(feature),
            own_status: (feature.own_agents.iter())
                .filter_map(|a| {
                    let key = crate::agents::AgentKey::Herdr(a.pane.clone()?);
                    Some((a.n, self.agent_key_state(&key).0))
                })
                .collect(),
            agent_read,
            tail,
            ctx: self.row_ctx(feature),
            setup: doing(Job::Setup),
            sandbox: doing(Job::Sandbox),
            verify: doing(Job::Verify),
            plan: self.planned_checks(feature),
            check_tails: Some(&self.features.check_tails),
            code_review: self.code_review_line(&panel.project, &panel.slug),
            code_counts: self
                .code_reviews
                .get(&(panel.project.clone(), panel.slug.clone()))
                .copied(),
            picks: self.chips_of(&panel.project, &panel.slug, panel.page_stage(feature)),
            model: self
                .model_slot_of(&panel.project, &panel.slug, panel.page_stage(feature))
                .map(|slot| model_label(slot.picked.as_deref())),
            fix_picks: match keys::fix_page(feature, panel.page_stage(feature)) {
                true => self.chips_of(&panel.project, &panel.slug, Stage::Build),
                false => Vec::new(),
            },
            fix_model: (self.model_slot_of(&panel.project, &panel.slug, Stage::Build))
                .filter(|_| keys::fix_page(feature, panel.page_stage(feature)))
                .map(|slot| model_label(slot.picked.as_deref())),
            fix_moved: self.stage_agent_moved(feature, Stage::Build),
            place: self.place_label(feature, panel.page_stage(feature)),
            machine_line: self.machine_line(feature, panel.page_stage(feature)),
            seen_note: self.seen_note(feature),
            setup_on: (feature.setup.as_ref())
                .and_then(|s| s.machine.as_deref())
                .map(|id| self.machine_name(id)),
            sandbox_seen: self.sandbox_seen(feature),
            sandbox_lan: self.sandbox_lan(&feature.project, &feature.slug),
            manual: manual_counts.map(crate::manual_test::counts_text),
            criteria: self.criterion_rows(feature),
            manual_gate: manual_counts.map(manual_gate),
            rounds_open: panel.rounds_open.clone(),
            fix_rounds_flipped: panel.fix_rounds_flipped.clone(),
        };
        let mut view = tab_view(panel, feature, &slugs, &state, now);
        let (running, more) = self
            .features
            .running_of(&panel.project, &panel.slug, &feature.title);
        view.running = running.into_iter().map(|(row, _)| row).collect();
        view.running_more = more;
        // Under a dialog or a panel the page takes no keys: its footer
        // offers none.
        if self.grabbing_overlay().is_some() {
            view.hints.clear();
        }
        // The search counts the places it found as the page shows them.
        let typed = (panel.search.as_ref()).is_some_and(|s| !s.needle().is_empty());
        if typed {
            let layout = chrome::feature_tab_layout(&view, self.content_rect());
            let n = layout.matches.len();
            let count = chrome::search_count(true, n, view.marks.found_at.filter(|a| *a < n));
            if let Some(bar) = view.test_strip.as_mut().and_then(|s| s.search.as_mut()) {
                bar.count = count;
                bar.found = n > 0;
            }
        }
        Some(view)
    }

    /// The sidebar's rows for the shown project's open features.
    pub fn feature_rows(&self) -> Vec<chrome::FeatureRowView> {
        self.sidebar_listed()
            .into_iter()
            .map(|f| chrome::FeatureRowView {
                title: self.row_title(f),
                card: self.feature_card(f),
            })
            .collect()
    }

    /// What `f`'s card says, on its row or its workspace's header.
    pub(crate) fn feature_card(&self, f: &Feature) -> chrome::FeatureCard {
        let mine = || {
            let busy = self.features.busy.iter();
            busy.filter(|b| b.project == f.project && b.slug == f.slug)
        };
        let doing = |job: Job| mine().find(|b| b.job == job).map(|b| b.what.as_str());
        let key = (f.project.clone(), f.slug.clone());
        let manual = self.manual_counts(f).map(manual_gate);
        let ctx = self.row_ctx(f);
        let machine = f.current().and_then(|stage| self.machine_line(f, stage));
        let line = LineState {
            machine,
            probe: (self.features.probes.get(&key)).and_then(|p| p.last.as_ref()),
            shipping: mine().any(|b| b.job == Job::Ship),
            checks: ctx.checks,
            setup: doing(Job::Setup),
            verify: doing(Job::Verify),
            manual_gate: manual,
            code_counts: self.code_reviews.get(&key).copied(),
            agent: self.agent_standing_of(f),
        };
        let gate = crate::feature_agents::gate_of(f, |_| manual).is_some();
        let busy = mine().next().is_some();
        let hold = CardHold { gate, busy };
        let back = rework(f, ctx);
        let (stage, text, tone) = card_status(f, &line, hold, back, feature::now());
        // What another machine writes says which, first as the end may not fit.
        let text = match f.writer.as_deref() {
            Some(id) => format!("{}: {text}", self.machine_name(id)),
            None => text,
        };
        chrome::FeatureCard {
            marks: (Stage::ALL.iter())
                .map(|s| node_mark(f, *s, ctx, &line))
                .collect(),
            stage,
            line: text,
            tone,
            gate,
            repos: f.repos.len(),
        }
    }

    /// A sidebar row's title for `f`: another project's feature (one
    /// spanning the shown project) says whose it is.
    pub(crate) fn row_title(&self, f: &Feature) -> String {
        if self.project.as_deref() == Some(f.project.as_str()) {
            return f.title.clone();
        }
        format!("{} \u{b7} in {}", f.title, self.home_name(f))
    }

    /// The name of `f`'s home project.
    pub(crate) fn home_name<'a>(&'a self, f: &'a Feature) -> &'a str {
        (self.projects.project(&f.project)).map_or(f.project.as_str(), |p| &p.name)
    }

    /// Feature `i` of the sidebar's rows.
    pub fn sidebar_feature(&self, i: usize) -> Option<&Feature> {
        self.sidebar_listed().get(i).copied()
    }

    /// The features the sidebar's Features section lists, in order: the
    /// shown project's open ones with no workspace open (their own
    /// workspace's header shows them instead), then other projects' open
    /// features that span the shown project too.
    fn sidebar_listed(&self) -> Vec<&Feature> {
        let Some(project) = self.project.as_deref() else {
            return Vec::new();
        };
        let dirs: Vec<&Path> = self.workspaces.iter().map(|ws| ws.dir.as_path()).collect();
        let own = self
            .features
            .all
            .get(project)
            .map(|all| sidebar_features(all));
        let mut features = own.unwrap_or_default();
        features.extend(sidebar_features(self.viewed_of(project)));
        let mut listed = crate::feature_workspace::without_workspace(features, &dirs);
        listed.extend(
            linked_in(&self.features.all, project)
                .into_iter()
                .map(|(_, f)| f),
        );
        listed
    }
}
