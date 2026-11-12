//! The feature panel's own methods: keys, selection, scrolling.

use super::*;

impl FeaturePanel {
    /// Write the note on `on`, starting from `field`, or from the draft Esc
    /// left there.
    pub fn open_note(&mut self, on: CardNote, field: TextField) {
        let field = match self.drafts.iter().position(|(d, _)| d.same_card(&on)) {
            Some(i) => self.drafts.remove(i).1,
            None => field,
        };
        self.note = Some((on, field));
    }

    /// Esc on the note being written: put away unsaved, as a draft when
    /// anything is written, for the next `open_note` on its card.
    pub fn park_note(&mut self) {
        if let Some((on, field)) = self.note.take() {
            self.drafts.retain(|(d, _)| !d.same_card(&on));
            if !field.trim().is_empty() {
                self.drafts.push((on, field));
            }
        }
    }

    /// Discard the note being written, and its card's draft.
    pub fn discard_note(&mut self) {
        if let Some((on, _)) = self.note.take() {
            self.drafts.retain(|(d, _)| !d.same_card(&on));
        }
    }

    /// The stage whose page shows on `feature`: the one picked, while the
    /// feature is still at the stage it was at then, else its current
    /// one (Ship once every stage is done).
    pub fn page_stage(&self, feature: &Feature) -> Stage {
        let current = feature.current();
        // The first commit of a fix round sends the feature back, and Code
        // review's fixes are followed where they were asked for.
        let round = (feature.fix_round.as_ref())
            .filter(|round| round.stage == Stage::CodeReview && feature.status == Status::Open)
            .map(|round| round.stage);
        (self.stage.filter(|_| self.picked_at == current))
            .or(round)
            .or(current)
            .unwrap_or(Stage::Ship)
    }

    /// Show `stage`'s page of `feature`, until the feature moves on.
    pub fn pick(&mut self, feature: &Feature, stage: Stage) {
        self.stage = Some(stage);
        self.picked_at = feature.current();
    }

    /// Show the current stage's page again.
    pub fn unpick(&mut self) {
        self.stage = None;
        self.picked_at = None;
    }

    /// The page the Page column shows on `feature`: its stage, and
    /// whether the keys list shows in its place.
    pub(super) fn page_key(&self, feature: &Feature) -> (Stage, bool) {
        (self.page_stage(feature), self.keys)
    }

    /// The Page column's scroll as it shows on `feature`: none while it
    /// was scrolled for another page.
    pub(super) fn page_scroll(&self, feature: &Feature) -> Option<&Scroll> {
        let col = TabColumn::Page as usize;
        (self.page_of == Some(self.page_key(feature))).then(|| &self.scroll[col])
    }

    /// Hold the Page column's scroll for page `key`, from its top if it
    /// was for another.
    pub(super) fn hold_page(&mut self, key: (Stage, bool)) {
        if self.page_of != Some(key) {
            self.scroll[TabColumn::Page as usize] = Scroll::default();
            self.page_of = Some(key);
        }
    }

    /// The repository row at `at` takes the keyboard.
    pub(super) fn select_row(&mut self, at: usize) {
        self.focus = TabArea::Repos;
        self.repo_pick = (self.repo == at).then_some(self.repo_pick).flatten();
        self.repo = at;
    }

    /// The row of repository `key` takes the keyboard, if it has one.
    pub fn select_repo(&mut self, feature: &Feature, key: &str) {
        // The rows just changed under a pointer that stayed: what it was
        // over is somewhere else now, until it moves.
        self.tab_hover = None;
        let at = repo_entries(feature, &self.sources, self.expanded)
            .into_iter()
            .position(|e| entry_key(feature, &self.sources, e).as_deref() == Some(key));
        if let Some(at) = at {
            self.select_row(at);
            self.repo_pick = None;
        }
    }

    /// `select_repo`, leaving the keyboard where it is and the button
    /// `pick` picked: what the rows did before them changed, not the user.
    pub(super) fn select_repo_keeping(
        &mut self,
        feature: &Feature,
        key: &str,
        pick: Option<usize>,
    ) {
        let focus = self.focus;
        self.select_repo(feature, key);
        self.focus = focus;
        self.repo_pick = pick;
    }

    /// What Code review's page of `feature` shows: what was picked, else its
    /// code until fixes were asked for, and those from then on.
    pub fn review_view_of(&self, feature: &Feature) -> chrome::ReviewView {
        self.review_view
            .unwrap_or(match crate::feature_diff::has_rounds(feature) {
                true => chrome::ReviewView::Fixes,
                false => chrome::ReviewView::Code,
            })
    }

    /// Open fix round `n`'s card, or fold it.
    pub fn toggle_fix_round(&mut self, n: u32) {
        if !self.fix_rounds_flipped.remove(&n) {
            self.fix_rounds_flipped.insert(n);
        }
    }

    /// Show the earlier review `n`'s findings, or fold them.
    pub fn toggle_round(&mut self, n: u32) {
        if !self.rounds_open.remove(&n) {
            self.rounds_open.insert(n);
        }
    }

    /// Fold the earlier reviews of `stage`'s page of `feature` if any shows,
    /// else show them all.
    pub(super) fn toggle_rounds(&mut self, feature: &Feature, stage: Stage) {
        let cards = card_numbers(feature, stage);
        let newest = newest_round(feature, stage);
        let reviews: BTreeSet<u32> = earlier_reviews(feature, stage).map(|r| r.n).collect();
        // Anything open folds all; else all open, the fix rounds and the
        // earlier reviews alike.
        let folds = folds_round_cards(feature, stage);
        let any_open = (folds
            && (cards.iter()).any(|&n| fix_round_open(newest, n, &self.fix_rounds_flipped)))
            || reviews.iter().any(|n| self.rounds_open.contains(n));
        if folds {
            // Folded, the newest is the one flipped; opened, the older ones.
            self.fix_rounds_flipped = match any_open {
                true => newest.into_iter().collect(),
                false => cards
                    .iter()
                    .copied()
                    .filter(|n| Some(*n) != newest)
                    .collect(),
            };
        }
        self.rounds_open = match any_open {
            true => BTreeSet::new(),
            false => reviews,
        };
    }

    /// Show the repositories as rows, or back as the strip's chips. The
    /// keyboard goes to the rows, and back to where it was. A row past
    /// what collapsed lists is left to clamp to the last chip.
    pub fn toggle_repos(&mut self) {
        self.expanded = !self.expanded;
        self.repo_pick = None;
        self.tab_hover = None;
        if self.expanded {
            self.collapse_to = self.focus;
            self.focus = TabArea::Repos;
            self.scroll[TabColumn::Repos as usize] = Scroll::default();
        } else {
            self.focus = self.collapse_to;
        }
    }

    /// The button on the row of `entry` that Enter presses: the one Left
    /// and Right picked, else the first of a row that has nothing else to
    /// do. A row the feature holds opens its tab until one is picked.
    pub fn picked(&self, feature: &Feature, entry: RepoEntry) -> Option<usize> {
        let buttons = entry_buttons(feature, &self.sources, entry).len();
        let first = (!matches!(entry, RepoEntry::Held(_))).then_some(0);
        self.repo_pick
            .filter(|at| *at < buttons)
            .or(first)
            .filter(|at| *at < buttons)
    }

    /// Left (`back`) or Right on the row of `entry`.
    fn pick_button(&mut self, feature: &Feature, entry: RepoEntry, back: bool) {
        let buttons = entry_buttons(feature, &self.sources, entry).len();
        let picked = self.picked(feature, entry);
        self.repo_pick = match (picked, back) {
            _ if buttons == 0 => None,
            (None, false) => Some(0),
            (Some(at), false) => Some((at + 1).min(buttons - 1)),
            (Some(at), true) if at > 0 => Some(at - 1),
            (Some(_), true) if matches!(entry, RepoEntry::Held(_)) => None,
            (at, true) => at,
        };
    }

    /// Del with the keyboard on the row of `entry`, if it is one the
    /// feature can do without. A repository on the feature's branch asks
    /// (see `ask_remove_repos`), so its Del acts at once; one that goes by
    /// itself arms first (checklist C4). Rows the feature doesn't hold have
    /// nothing to take out, and never fall through to abandoning it.
    fn delete_row(
        &mut self,
        feature: &Feature,
        entry: RepoEntry,
        armed: Option<String>,
    ) -> Option<PanelOutcome> {
        let arm = |panel: &mut Self, key: String, then: PanelOutcome| {
            if armed.as_ref() == Some(&key) {
                return Some(then);
            }
            panel.armed = Some(key);
            None
        };
        match entry {
            RepoEntry::Held(i) if feature.many_repos() => {
                let repo = feature.repos.get(i)?;
                let key = repo.key();
                match repo.touched {
                    true => Some(PanelOutcome::RemoveRepo(key)),
                    false => arm(self, key.clone(), PanelOutcome::RemoveRepo(key)),
                }
            }
            RepoEntry::Held(_) => None,
            RepoEntry::Context(i) if linkable(feature) => {
                let key = feature.context.get(i)?.key();
                arm(self, key.clone(), PanelOutcome::DropContext(key))
            }
            _ => None,
        }
    }

    /// `key` on the item's card with the keyboard, of `cards` (see
    /// `review_cards::card_key`): `armed` is what the last press armed.
    fn card_key(
        &mut self,
        key: CardKey,
        cards: &[ReviewCard],
        refused: Option<&'static str>,
        armed: Option<&str>,
    ) -> Option<PanelOutcome> {
        card_key(
            key,
            cards,
            &mut self.finding,
            refused,
            armed,
            &mut self.armed,
        )
    }

    /// Handle a key over `feature`, one of `slugs` listed: what the first
    /// row of `keys::chain` that is it and is offered does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        feature: &Feature,
        slugs: &[String],
        ctx: RowCtx,
        plan: &[PlannedCheck],
        ships: &[ShipRow],
        notes: &[Comment],
    ) -> Option<PanelOutcome> {
        let armed = self.armed.take();
        // The pick no longer holds once the feature moved on.
        if self.stage.is_some() && self.picked_at != feature.current() {
            self.unpick();
        }
        let stage = self.page_stage(feature);
        let (settled, key) = {
            let t = keys::TabCtx::new(
                self,
                feature,
                stage,
                slugs,
                ctx,
                plan,
                ships,
                notes,
                armed.clone(),
            );
            // The keys list: F1 or Esc puts it away, as does any other key
            // (it does nothing else then).
            let key = match self.keys {
                true => Some(keys::TabKey::Swallow),
                false => t.press(mods, sym),
            };
            let settled = (t.focus, t.repo, t.finding, t.repos, t.carded);
            (settled, key)
        };
        let (focus, repo, finding, repos, carded) = settled;
        (self.focus, self.repo, self.finding) = (focus, repo, finding);
        if self.keys {
            self.keys = false;
            return None;
        }
        use keys::TabKey;
        match key? {
            TabKey::Out(outcome) => return Some(outcome),
            TabKey::Swallow => {}
            TabKey::ShowKeys => self.keys = true,
            TabKey::FoldRepos => self.toggle_repos(),
            TabKey::PickButton(entry, back) => self.pick_button(feature, entry, back),
            TabKey::Step(step) => match self.focus {
                TabArea::Stages => {
                    let at = stage_index(stage);
                    let to = step.from(at, Stage::ALL.len() - 1);
                    self.pick(feature, Stage::ALL[to]);
                }
                TabArea::Repos => {
                    let to = step.from(self.repo, repos.saturating_sub(1));
                    if to != self.repo {
                        self.repo = to;
                        self.repo_pick = None;
                    }
                }
                TabArea::Findings => {
                    self.finding = step.from(self.finding, carded.saturating_sub(1));
                }
            },
            TabKey::Focus(to) => self.focus = to,
            TabKey::Arm(key) => self.armed = Some(key),
            TabKey::Card(key) => {
                let cards = keys::page_cards(self, feature, stage, notes);
                let refused = card_refused(feature, stage, &cards, self.finding);
                return self.card_key(key, &cards, refused, armed.as_deref());
            }
            TabKey::CardOpen => {
                let cards = keys::page_cards(self, feature, stage, notes);
                let refused = card_refused(feature, stage, &cards, self.finding);
                let open = self.card_key(CardKey::CtrlO, &cards, refused, armed.as_deref());
                return open.or_else(|| Some(PanelOutcome::Open(feature.dir.clone())));
            }
            TabKey::ToggleRounds => self.toggle_rounds(feature, stage),
            TabKey::DeleteRow(entry) => return self.delete_row(feature, entry, armed),
        }
        None
    }
}
