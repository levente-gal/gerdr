//! The feature tab's keys as tables (`key_table`): the handler, the footer,
//! a click on a hint and F1 all read the same rows (checklist K3).
//!
//! - `SHIP_KEYS`: Ship's page's buttons, by their keys; `SUBMIT_KEYS` in
//!   their place while a pull request's review submits.

use super::*;
use crate::key_table::{self, Binding};

/// What Ship's page offers now, for its keys: the buttons, each with the
/// repository it acts on, and the repository of the row the keyboard is
/// on.
pub(super) struct ShipCtx {
    offered: Vec<(PageButton, PanelOutcome, Option<String>)>,
    repo: Option<String>,
}

impl ShipCtx {
    /// Ship's page of `feature` as `ctx` and `probe` have it, the keyboard
    /// on the row of `repo`.
    pub(super) fn new(
        feature: &Feature,
        ctx: RowCtx,
        probe: Option<&Probe>,
        repo: Option<&str>,
    ) -> Self {
        let mut offered = page_actions(feature, Stage::Ship, ctx);
        offered.retain(|(_, o)| !undoes_merged(o, probe));
        offered.extend(ship_base_actions(feature, probe));
        let offered = (offered.into_iter())
            .map(|(b, o)| {
                let of = ship_outcome_repo(feature, &o);
                (b, o, of)
            })
            .collect();
        Self {
            offered,
            repo: repo.map(str::to_owned),
        }
    }

    /// The first button offered that `want`s: one acting on a repository
    /// goes by the repository of the row the keyboard is on; with none, by
    /// the first that offers it (the latest step to undo, the card's way to
    /// take its base in).
    fn find(&self, want: impl Fn(&PanelOutcome) -> bool) -> Option<PanelOutcome> {
        (self.offered.iter())
            .filter(|(_, o, _)| want(o))
            .find(|(_, o, of)| match (&self.repo, of) {
                (Some(on), Some(of)) => on == of,
                // On a repository, Alt+Z undoes its step, not what followed.
                (Some(_), None) => !matches!(o, PanelOutcome::UndoAfter(_)),
                _ => true,
            })
            .map(|(_, o, _)| o.clone())
    }

    /// What the button asking `outcome` says, as the footer writes it.
    pub(super) fn said(&self, outcome: &PanelOutcome) -> String {
        (self.offered.iter())
            .find(|(_, o, _)| o == outcome)
            .map_or_else(String::new, |(b, ..)| lower_first(&b.label))
    }
}

/// The keys of Ship's page's buttons, in the footer's order.
pub(super) const SHIP_KEYS: &[Binding<ShipCtx, PanelOutcome>] = &[
    Binding {
        chord: "Ctrl+M",
        does: |c| {
            c.find(|o| {
                matches!(
                    o,
                    PanelOutcome::UpdateBase(_, feature::UpdateWay::Merge)
                        | PanelOutcome::ResolveConflicts(_, false)
                )
            })
        },
        footer: true,
        f1: "Ship: merge the base into the selected repository's branch; with conflicts, have an agent resolve them, as planned once a plan is written",
    },
    Binding {
        chord: "Ctrl+Shift+M",
        does: |c| c.find(|o| matches!(o, PanelOutcome::UpdateBase(_, feature::UpdateWay::Rebase))),
        footer: true,
        f1: "Ship: rebase the selected repository's branch onto its base",
    },
    Binding {
        chord: "Alt+M",
        does: |c| c.find(|o| matches!(o, PanelOutcome::ResolveConflicts(_, true))),
        footer: true,
        f1: "Ship: have an agent plan how to resolve the conflicts first",
    },
    Binding {
        chord: "Ctrl+Shift+O",
        // A plan to review comes before a pull request to open.
        does: |c| {
            c.find(|o| matches!(o, PanelOutcome::ReviewConflictPlan))
                .or_else(|| c.find(|o| matches!(o, PanelOutcome::OpenPr(_))))
        },
        footer: true,
        f1: "Ship: review the plan of a resolution, or open the selected repository's pull request",
    },
    Binding {
        chord: "Ctrl+Shift+Del",
        does: |c| c.find(|o| matches!(o, PanelOutcome::StopResolving)),
        footer: true,
        f1: "Ship: stop an agent's merge and abort it, after asking",
    },
    Binding {
        chord: "Alt+Z",
        does: |c| c.find(|o| matches!(o, PanelOutcome::UndoShip(..) | PanelOutcome::UndoAfter(_))),
        footer: true,
        f1: "Ship: undo the selected repository's last step, after asking; off the rows, the latest, or what followed shipping",
    },
    Binding {
        chord: "Ctrl+R",
        does: |c| c.find(|o| matches!(o, PanelOutcome::WriteSummary)),
        footer: true,
        f1: "Ship: write the summary again",
    },
    Binding {
        chord: "Ctrl+E",
        does: |c| c.find(|o| matches!(o, PanelOutcome::EditSummary)),
        footer: true,
        f1: "Ship: edit the summary",
    },
    Binding {
        chord: "F5",
        does: |c| c.find(|o| matches!(o, PanelOutcome::CheckShip)),
        footer: true,
        f1: "Ship: check again now, fetching origin too",
    },
];

/// The pull request `feature` reviews while its review can be submitted:
/// open, at Ship.
fn submits(feature: &Feature) -> Option<&feature::pr::PullRef> {
    let open = feature.status == Status::Open && feature.current() == Some(Stage::Ship);
    feature.pull().filter(|_| open)
}

/// The keys of Ship's page while it submits a pull request's review:
/// Ctrl+Enter approves someone else's, Ctrl+Shift+Enter requests changes,
/// Ctrl+E edits the review's text. Enter is the page's primary, as on any.
pub(super) const SUBMIT_KEYS: &[Binding<Feature, PanelOutcome>] = &[
    Binding {
        chord: "Ctrl+Enter",
        does: |f| {
            submits(f).filter(|p| !p.mine)?;
            Some(PanelOutcome::SubmitReview(feature::pr_submit::Event::Approve))
        },
        footer: true,
        f1: "reviewing someone else's pull request, on Ship's page: approve it, after asking (Enter comments, or approves with nothing to post)",
    },
    Binding {
        chord: "Ctrl+Shift+Enter",
        does: |f| {
            submits(f).filter(|p| !p.mine)?;
            Some(PanelOutcome::SubmitReview(feature::pr_submit::Event::RequestChanges))
        },
        footer: true,
        f1: "reviewing someone else's pull request, on Ship's page: request changes, after asking",
    },
    Binding {
        chord: "Ctrl+E",
        does: |f| submits(f).map(|_| PanelOutcome::EditSummary),
        footer: true,
        f1: "reviewing a pull request, on Ship's page: edit the review's text",
    },
];

/// The footer's words for a key of `SUBMIT_KEYS`.
pub(super) fn submit_said(outcome: &PanelOutcome) -> String {
    match outcome {
        PanelOutcome::EditSummary => "edit text".to_owned(),
        o => action_hint(o).to_owned(),
    }
}

/// The keys of the buttons Ship's page offers now, as `ctx` and `probe` have
/// it, the keyboard on the row of `repo` (see `ShipCtx::find`).
pub(super) fn ship_keys(
    feature: &Feature,
    ctx: RowCtx,
    probe: Option<&Probe>,
    repo: Option<&str>,
) -> Vec<ShipKey> {
    let c = ShipCtx::new(feature, ctx, probe, repo);
    (SHIP_KEYS.iter())
        .filter_map(|b| {
            let outcome = (b.does)(&c)?;
            // The hint says what the button says, so the two can't drift.
            let what = c.said(&outcome);
            Some(ShipKey {
                chord: b.chord,
                what,
                outcome,
            })
        })
        .collect()
}

/// What a key does on Ship's page of `feature`, a pull request's review.
pub(super) fn submit_key(
    feature: &Feature,
    mods: &ModifiersState,
    sym: Keysym,
) -> Option<PanelOutcome> {
    key_table::press(SUBMIT_KEYS, feature, mods, sym)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ship_tables_hint_what_they_press() {
        let f = Feature::default();
        key_table::check(SUBMIT_KEYS, &[f]);
        let f = Feature::default();
        key_table::check(SHIP_KEYS, &[ShipCtx::new(&f, RowCtx::default(), None, None)]);
    }
}

/// The F1 sheet's sections, in its order: where the keys work.
pub(super) const SECTIONS: [&str; 7] = [
    "Stages",
    "Manual test's criteria",
    "AI review's findings",
    "Plan review's suggestions",
    "Code review",
    "Repositories",
    "Anywhere on the tab",
];
const STAGES: usize = 0;
const CRITERIA: usize = 1;
const FINDINGS: usize = 2;
const SUGGESTIONS: usize = 3;
const CODE_REVIEW: usize = 4;
const REPOS: usize = 5;
const ANYWHERE: usize = 6;

/// What a key of the feature tab does: an outcome for `Gerdr`, or a change
/// to the panel itself.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TabKey {
    Out(PanelOutcome),
    /// Taken, doing nothing now: no later row has the key.
    Swallow,
    ShowKeys,
    /// Show or hide all repositories.
    FoldRepos,
    /// Go through the row's buttons (`back`: to the left).
    PickButton(RepoEntry, bool),
    Step(Step),
    Focus(TabArea),
    /// A first Del, arming `key` (checklist C4).
    Arm(String),
    /// A key of an item's card (`review_cards::card_key`).
    Card(CardKey),
    /// Ctrl+O on a card: the code it cites, else a tab in the folder.
    CardOpen,
    ToggleRounds,
    DeleteRow(RepoEntry),
}

/// The feature tab as its keys see it: the panel over `feature` on
/// `stage`'s page, with what that page holds.
pub(super) struct TabCtx<'a> {
    pub(super) panel: &'a FeaturePanel,
    pub(super) feature: &'a Feature,
    pub(super) stage: Stage,
    slugs: &'a [String],
    pub(super) ctx: RowCtx,
    /// Where the keyboard is, once settled: the repositories with none
    /// shown, the cards with none or with all repositories shown, aren't.
    pub(super) focus: TabArea,
    pub(super) repo: usize,
    pub(super) finding: usize,
    pub(super) repos: usize,
    pub(super) entry: Option<RepoEntry>,
    pub(super) row: PanelRow,
    pub(super) cards: Vec<ReviewCard<'a>>,
    pub(super) checks: Vec<CheckEntry>,
    /// The criteria tried on the page, marked off on it, and shown on it.
    criteria: usize,
    marking: usize,
    shown: usize,
    /// The user's comments come after the brief's criteria.
    first_comment: usize,
    pub(super) ships: &'a [ShipRow],
    pub(super) carded: usize,
    refused: Option<&'static str>,
    /// What the last press armed.
    armed: Option<String>,
}

/// The review cards on `stage`'s page: none while the review itself is on
/// it, with its own keys.
pub(super) fn page_cards<'a>(
    panel: &FeaturePanel,
    feature: &'a Feature,
    stage: Stage,
    notes: &'a [Comment],
) -> Vec<ReviewCard<'a>> {
    match crate::feature_diff::review_page(panel, feature) {
        true => Vec::new(),
        false => review_cards(feature, stage, notes),
    }
}

impl<'a> TabCtx<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        panel: &'a FeaturePanel,
        feature: &'a Feature,
        stage: Stage,
        slugs: &'a [String],
        ctx: RowCtx,
        plan: &[PlannedCheck],
        ships: &'a [ShipRow],
        notes: &'a [Comment],
        armed: Option<String>,
    ) -> Self {
        let entries = repo_entries(feature, &panel.sources, panel.expanded);
        let repos = entries.len();
        let repo = panel.repo.min(repos.saturating_sub(1));
        let mut focus = panel.focus;
        if repos == 0 && focus == TabArea::Repos {
            focus = TabArea::Stages;
        }
        let entry = entries.get(repo).copied();
        let cards = page_cards(panel, feature, stage, notes);
        let checks = check_entries(feature, stage, plan);
        let count = |on: bool| if on { ctx.criteria } else { 0 };
        let criteria = count(tries_criteria(feature, stage));
        // Marked off it too, they wait for it: only at it do they move it on.
        let marking = count(marks_criteria(feature, stage));
        let shown = count(shows_criteria(feature, stage));
        let ships = if stage == Stage::Ship { ships } else { &[] };
        let carded = cards.len() + checks.len() + shown + ships.len();
        let finding = panel.finding.min(carded.saturating_sub(1));
        if focus == TabArea::Findings && (carded == 0 || panel.expanded) {
            focus = TabArea::Stages;
        }
        let refused = match cards.get(finding).copied().and_then(ReviewCard::diff_note) {
            Some(c) => why_not_edit_note(feature, stage, c),
            None => why_not_decide(feature, stage),
        };
        let row = match (focus, entry) {
            (TabArea::Stages | TabArea::Findings, _) => PanelRow::Stage(stage),
            (TabArea::Repos, Some(RepoEntry::Held(i))) => PanelRow::Repo(i),
            (TabArea::Repos, _) => PanelRow::Other,
        };
        Self {
            panel,
            feature,
            stage,
            slugs,
            ctx,
            focus,
            repo,
            finding,
            repos,
            entry,
            row,
            cards,
            checks,
            criteria,
            marking,
            shown,
            first_comment: marking.saturating_sub(ctx.manual.comments()),
            ships,
            carded,
            refused,
            armed,
        }
    }

    /// The card with the keyboard, an item's or a comment's.
    fn on(&self) -> Option<ReviewCard<'a>> {
        self.cards.get(self.finding).copied()
    }

    fn item(&self) -> Option<Item<'a>> {
        self.on().and_then(ReviewCard::item)
    }

    fn note(&self) -> Option<&'a Comment> {
        self.on().and_then(ReviewCard::diff_note)
    }

    /// The card with the keyboard, a comment of the user's on the criteria.
    fn comment_at(&self) -> Option<usize> {
        let at = self.finding;
        (at >= self.first_comment && at < self.marking).then_some(at)
    }

    fn findings(&self) -> bool {
        self.focus == TabArea::Findings
    }

    fn on_repos(&self) -> bool {
        self.focus == TabArea::Repos
    }

    /// The page has the keyboard: the rail or its cards.
    fn page(&self) -> bool {
        self.focus != TabArea::Repos
    }

    /// The F1 section of the arrows here, by what they go through.
    fn area(&self) -> usize {
        match self.focus {
            TabArea::Stages => STAGES,
            TabArea::Repos => REPOS,
            _ if self.shown > 0 => CRITERIA,
            _ if !self.checks.is_empty() || !self.ships.is_empty() => STAGES,
            _ if self.stage == Stage::PlanReview => SUGGESTIONS,
            _ if self.stage == Stage::CodeReview => CODE_REVIEW,
            _ => FINDINGS,
        }
    }

    /// The arrows of section `area` move, `step`.
    fn step(&self, area: usize, step: Step) -> Option<TabKey> {
        (self.area() == area).then_some(TabKey::Step(step))
    }

    /// The item's card with the keyboard, as `review_cards`' keys read it.
    fn card_ctx(&self) -> Option<crate::review_cards::CardCtx> {
        let page = CardPage {
            decides: can_decide(self.feature, self.stage),
            posts: self.feature.posts_review(),
        };
        (self.item()).map(|i| crate::review_cards::CardCtx::of(i, page, self.armed.is_some()))
    }

    /// Alt+E: fold the earlier reviews and fix round cards, on a page of
    /// `stages` (none: any other).
    fn folds(&self, stages: &[Stage]) -> Option<TabKey> {
        let page = match stages.is_empty() {
            true => !FOLD_STAGES.contains(&self.stage),
            false => stages.contains(&self.stage),
        };
        let folds = earlier_reviews(self.feature, self.stage).next().is_some()
            || folds_round_cards(self.feature, self.stage);
        (page && !self.panel.expanded && folds).then_some(TabKey::ToggleRounds)
    }

    /// Ctrl+Enter on the criteria tried: the user's comments go through a
    /// plan, so it never plans them; Ctrl+Shift+Enter plans the fixes.
    fn manual_gate(&self, plans: bool) -> Option<TabKey> {
        use crate::manual_test::Outcome;
        let on =
            self.criteria > 0 && self.page() && fix_round_on(self.feature, self.stage).is_none();
        if !on {
            return None;
        }
        let sends = manual_sends(self.feature, self.ctx);
        Some(TabKey::Out(match plans {
            true => PanelOutcome::Manual(Outcome::SendFailures),
            false if sends && manual_own_words(self.ctx).is_some() => {
                PanelOutcome::FindingRefused(PLAN_FIRST)
            }
            false if sends => PanelOutcome::Manual(Outcome::MakeFixes),
            false => PanelOutcome::Manual(Outcome::MarkPassed),
        }))
    }

    /// Enter on the repositories: show them all; shown, the row's button
    /// picked, else what the row does.
    fn repos_enter(&self) -> Option<TabKey> {
        self.on_repos().then(|| self.rows_enter())
    }

    /// Enter with a modifier on the page: its main button on the rail, else
    /// the repositories' Enter, wherever the keyboard is.
    fn any_enter(&self) -> TabKey {
        match self.focus {
            TabArea::Stages => {
                page_enter(self.feature, self.stage, self.ctx).map_or(TabKey::Swallow, TabKey::Out)
            }
            _ => self.rows_enter(),
        }
    }

    /// The repositories' Enter: show them all; shown, the row's button.
    fn rows_enter(&self) -> TabKey {
        if !self.panel.expanded {
            return TabKey::FoldRepos;
        }
        let (feature, panel) = (self.feature, self.panel);
        let picked = self.entry.and_then(|e| {
            let mut buttons = entry_buttons(feature, &panel.sources, e);
            let at = panel.picked(feature, e)?;
            (at < buttons.len()).then(|| buttons.swap_remove(at).1)
        });
        let did = picked.or_else(|| row_action(feature, self.row, self.ctx));
        did.map_or(TabKey::Swallow, TabKey::Out)
    }

    /// Ctrl+Shift+O: the row's page, a fix plan sent, or the pull request.
    fn open_page(&self) -> TabKey {
        let (feature, stage) = (self.feature, self.stage);
        let page = row_page(feature, self.row)
            .or_else(|| opens_fix_plan(feature, stage).then_some(PanelOutcome::ReviewFixPlan))
            .or_else(|| feature.pull().map(|p| PanelOutcome::OpenPr(p.url.clone())));
        page.map_or(TabKey::Swallow, TabKey::Out)
    }

    /// Tab's next stop, Shift+Tab's (`back`) the one before.
    fn tab(&self, back: bool) -> TabKey {
        let mut stops = vec![TabArea::Stages];
        if self.carded > 0 && !self.panel.expanded {
            stops.push(TabArea::Findings);
        }
        if self.repos > 0 {
            stops.push(TabArea::Repos);
        }
        let at = stops.iter().position(|s| *s == self.focus).unwrap_or(0);
        let by = if back { stops.len() - 1 } else { 1 };
        TabKey::Focus(stops[(at + by) % stops.len()])
    }

    /// The key `chord` (the first of "A / B") pressed now.
    pub(super) fn press_chord(&self, chord: &str) -> Option<TabKey> {
        let first = chord.split(" / ").next().unwrap_or(chord);
        let (mods, sym) = crate::keys::hint_key(first)?;
        // A hint is the key as it shows: not an alias's press.
        self.press_in(&mods, sym, false)
    }

    /// What the key `mods` and `sym` does now: the first row of the chain
    /// that is it and is offered.
    pub(super) fn press(&self, mods: &ModifiersState, sym: Keysym) -> Option<TabKey> {
        self.press_in(mods, sym, true)
    }

    /// `press`, alias rows too while `aliases`.
    fn press_in(&self, mods: &ModifiersState, sym: Keysym, aliases: bool) -> Option<TabKey> {
        let (mods, sym) = plain_key(mods, sym);
        let shown = |b: &&Binding<TabCtx<'a>, TabKey>| aliases || b.footer || !b.f1.is_empty();
        chain().iter().find_map(|link| match link {
            Link::Rows(_, rows) => (rows.iter())
                .filter(shown)
                .filter(|b| key_table::is(b.chord, &mods, sym))
                .find_map(|b| (b.does)(self)),
            // The keys of an item's card, as `review_cards` reads them.
            Link::Cards => {
                let card = self.card_ctx().filter(|_| self.findings())?;
                crate::review_cards::card_key_of(&card, &mods, sym).map(|k| match k {
                    CardKey::CtrlO => TabKey::CardOpen,
                    k => TabKey::Card(k),
                })
            }
        })
    }

    /// The tab as it is with the keyboard on the rail.
    pub(super) fn on_rail(mut self) -> Self {
        (self.focus, self.row) = (TabArea::Stages, PanelRow::Stage(self.stage));
        self
    }

    /// The first key of the chain that asks `outcome` now, for its menu
    /// row.
    pub(super) fn chord_of(&self, outcome: &PanelOutcome) -> Option<&'static str> {
        let want = TabKey::Out(outcome.clone());
        let rows = chain().iter().flat_map(|link| match link {
            Link::Rows(_, rows) => rows.iter(),
            Link::Cards => [].iter(),
        });
        rows.filter(|b| (b.does)(self).as_ref() == Some(&want))
            .map(|b| b.chord)
            .find(|chord| self.press_chord(chord).as_ref() == Some(&want))
    }
}

/// The pages whose Alt+E has its own F1 row.
const FOLD_STAGES: [Stage; 3] = [Stage::ManualTest, Stage::AiReview, Stage::CodeReview];

/// `mods` and `sym` as the tables match them: a keypad's arrows, page keys
/// and digits are the keys, and Shift+Tab is Shift with Tab, however it
/// comes.
fn plain_key(mods: &ModifiersState, sym: Keysym) -> (ModifiersState, Keysym) {
    let mut mods = *mods;
    let sym = match sym {
        Keysym::KP_Up => Keysym::Up,
        Keysym::KP_Down => Keysym::Down,
        Keysym::KP_Left => Keysym::Left,
        Keysym::KP_Right => Keysym::Right,
        Keysym::KP_Home => Keysym::Home,
        Keysym::KP_End => Keysym::End,
        Keysym::KP_Prior => Keysym::Prior,
        Keysym::KP_Next => Keysym::Next,
        Keysym::KP_1 => Keysym::_1,
        Keysym::KP_2 => Keysym::_2,
        Keysym::ISO_Left_Tab => {
            mods.shift = true;
            Keysym::Tab
        }
        sym => sym,
    };
    (mods, sym)
}

/// A run of the tab's handler: rows of one F1 section, or an item's card's
/// keys (`review_cards::CARD_KEYS`).
pub(super) enum Link<'a> {
    Rows(usize, &'a [Binding<TabCtx<'a>, TabKey>]),
    Cards,
}

/// A row of a table: offered and hinted while `does` says what it does.
macro_rules! row {
    ($chord:expr, $f1:expr, $does:expr) => {
        Binding {
            chord: $chord,
            does: $does,
            footer: true,
            f1: $f1,
        }
    };
}

/// An alias row: a press the old match took too, with a modifier more or
/// less than its key's own (Shift+Enter for Enter, say), kept so it still
/// works; never hinted, never on F1.
macro_rules! alias {
    ($chord:expr, $does:expr) => {
        Binding {
            chord: $chord,
            does: $does,
            footer: false,
            f1: "",
        }
    };
}

/// `outcome`, as a row does it.
fn out(outcome: PanelOutcome) -> Option<TabKey> {
    Some(TabKey::Out(outcome))
}

/// The feature tab's keys in the order the handler tries them: the first
/// row that is the key and is offered wins. Rows of one key keep the order
/// the handler had; runs of rows are by F1 section.
pub(super) fn chain<'a>() -> &'a [Link<'a>] {
    use crate::feature::manual::FoldScope;
    use crate::manual_test::Outcome;
    &[
        Link::Rows(
            REPOS,
            &[
                // Esc folds the rows first, and steps back as ever after that.
                row!("Esc", "hide them again", |t| t
                    .panel
                    .expanded
                    .then_some(TabKey::FoldRepos)),
                // On the rows Left and Right go through the row's buttons.
                row!(
                    "Left",
                    "previous or next repository; with all shown, the row's button",
                    |t| {
                        let e = t.entry.filter(|_| t.panel.expanded && t.on_repos())?;
                        Some(TabKey::PickButton(e, true))
                    }
                ),
                row!(
                    "Right",
                    "previous or next repository; with all shown, the row's button",
                    |t| {
                        let e = t.entry.filter(|_| t.panel.expanded && t.on_repos())?;
                        Some(TabKey::PickButton(e, false))
                    }
                ),
                row!("Up", "previous or next repository", |t| t
                    .step(REPOS, Step::Back)),
                row!("Down", "previous or next repository", |t| t
                    .step(REPOS, Step::On)),
                row!(
                    "Left",
                    "previous or next repository; with all shown, the row's button",
                    |t| t.step(REPOS, Step::Back)
                ),
                row!(
                    "Right",
                    "previous or next repository; with all shown, the row's button",
                    |t| t.step(REPOS, Step::On)
                ),
                row!("Ctrl+K", "previous or next repository", |t| t
                    .step(REPOS, Step::Back)),
                row!("Ctrl+J", "previous or next repository", |t| t
                    .step(REPOS, Step::On)),
                row!("Home", "first or last repository", |t| t
                    .step(REPOS, Step::First)),
                row!("End", "first or last repository", |t| t
                    .step(REPOS, Step::Last)),
                // With Shift the arrows went the same way, on any area.
                alias!("Shift+Left", |t| {
                    let e = t.entry.filter(|_| t.panel.expanded && t.on_repos())?;
                    Some(TabKey::PickButton(e, true))
                }),
                alias!("Shift+Right", |t| {
                    let e = t.entry.filter(|_| t.panel.expanded && t.on_repos())?;
                    Some(TabKey::PickButton(e, false))
                }),
                alias!("Shift+Left", |_| Some(TabKey::Step(Step::Back))),
                alias!("Shift+Right", |_| Some(TabKey::Step(Step::On))),
                alias!("Shift+Up", |_| Some(TabKey::Step(Step::Back))),
                alias!("Shift+Down", |_| Some(TabKey::Step(Step::On))),
                alias!("Shift+Home", |_| Some(TabKey::Step(Step::First))),
                alias!("Shift+End", |_| Some(TabKey::Step(Step::Last))),
                row!(
                    "Enter",
                    "show all repositories; with all shown, press the row's button",
                    |t| t.repos_enter()
                ),
                row!("Ctrl+Shift+O", "open its pull request", |t| t
                    .on_repos()
                    .then(|| t.open_page())),
                row!(
                    "Del",
                    "on a repository of several, remove it (one on the branch asks, others press twice); on a card, what the card says; never the feature",
                    |t| {
                        t.on_repos()
                            .then(|| t.entry.map_or(TabKey::Swallow, TabKey::DeleteRow))
                    }
                ),
            ],
        ),
        Link::Rows(
            STAGES,
            &[
                // A tick box or choice of the Ship page with the keyboard;
                // Enter still presses the page's button.
                row!(
                    "Space",
                    "Ship's steps: tick it, or its route's next way",
                    |t| {
                        (t.findings() && !t.ships.is_empty()).then(|| {
                            (t.ships.get(t.finding))
                                .map_or(TabKey::Swallow, |r| TabKey::Out(r.outcome()))
                        })
                    }
                ),
                row!("Enter", "Ship's steps: the page's main button", |t| {
                    (t.findings() && !t.ships.is_empty()).then(|| {
                        page_enter(t.feature, t.stage, t.ctx).map_or(TabKey::Swallow, TabKey::Out)
                    })
                }),
            ],
        ),
        Link::Rows(
            CRITERIA,
            &[
                // A criterion opens to its steps and folds to its line, as a
                // tree's rows do; with Alt, all of them, those passed or read.
                row!(
                    "Left",
                    "fold it to its line, or open it to its steps",
                    |t| {
                        (t.shown > 0 && t.findings())
                            .then_some(TabKey::Out(PanelOutcome::FoldCriterion(t.finding, false)))
                    }
                ),
                row!(
                    "Right",
                    "fold it to its line, or open it to its steps",
                    |t| {
                        (t.shown > 0 && t.findings())
                            .then_some(TabKey::Out(PanelOutcome::FoldCriterion(t.finding, true)))
                    }
                ),
                row!("Alt+Left", "fold all, or open all", |t| (t.shown > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::All,
                    false
                )))),
                row!("Alt+Right", "fold all, or open all", |t| (t.shown > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::All,
                    true
                )))),
                row!("Alt+D", "fold those passed, or open them", |t| (t.shown
                    > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::Done,
                    false
                )))),
                row!("Alt+Shift+D", "fold those passed, or open them", |t| (t
                    .shown
                    > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::Done,
                    true
                )))),
                row!("Alt+S", "fold those seen, or open them", |t| (t.shown > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::Seen,
                    false
                )))),
                row!("Alt+Shift+S", "fold those seen, or open them", |t| (t
                    .shown
                    > 0
                    && t.page())
                .then_some(TabKey::Out(PanelOutcome::FoldCriteria(
                    FoldScope::Seen,
                    true
                )))),
                row!("Up", "previous or next criterion", |t| t
                    .step(CRITERIA, Step::Back)),
                row!("Down", "previous or next criterion", |t| t
                    .step(CRITERIA, Step::On)),
                row!("Home", "first or last criterion", |t| t
                    .step(CRITERIA, Step::First)),
                row!("End", "first or last criterion", |t| t
                    .step(CRITERIA, Step::Last)),
                // A criterion's card with the keyboard; a comment's the same,
                // done for passed, and Del deletes it, the first press arming
                // it (checklist C4).
                row!("Space", "mark it passed, or not; passed, it folds", |t| {
                    (t.findings() && t.marking > 0).then_some(TabKey::Out(PanelOutcome::Manual(
                        Outcome::Toggle(t.finding),
                    )))
                }),
                row!("Enter", "mark it passed, or not; passed, it folds", |t| {
                    (t.findings() && t.marking > 0).then_some(TabKey::Out(PanelOutcome::Manual(
                        Outcome::Toggle(t.finding),
                    )))
                }),
                row!(
                    "f",
                    "mark it failed, saying what went wrong, or edit that",
                    |t| {
                        (t.findings() && t.marking > 0).then(|| {
                            TabKey::Out(PanelOutcome::Manual(match t.comment_at() {
                                Some(_) => Outcome::Fail(t.finding, String::new()),
                                None => Outcome::Write(t.finding),
                            }))
                        })
                    }
                ),
                row!("Ctrl+E", "a comment of yours: edit it", |t| {
                    (t.findings() && t.comment_at().is_some()).then_some(TabKey::Out(
                        PanelOutcome::Manual(Outcome::EditComment(t.finding)),
                    ))
                }),
                row!(
                    "Del",
                    "clear its result; a comment of yours: press twice to delete",
                    |t| {
                        if !(t.findings() && t.marking > 0) {
                            return None;
                        }
                        let Some(at) = t.comment_at() else {
                            return out(PanelOutcome::Manual(Outcome::Clear(t.finding)));
                        };
                        let key = format!("comment:{at}");
                        if t.armed.as_ref() != Some(&key) {
                            return Some(TabKey::Arm(key));
                        }
                        out(PanelOutcome::Manual(Outcome::DeleteComment(t.finding)))
                    }
                ),
                // From anywhere on the page, but for a fix plan's: with
                // failures or comments to send, Ctrl+Enter makes their fixes.
                row!(
                    "Ctrl+Enter",
                    "mark Manual test passed, or make the fixes of its failures",
                    |t| t.manual_gate(false)
                ),
                row!("Ctrl+Shift+Enter", "plan the fixes of the failures", |t| t
                    .manual_gate(true)),
                row!("Alt+E", "fold the fix round cards, or open them all", |t| t
                    .folds(&[Stage::ManualTest])),
            ],
        ),
        Link::Rows(
            FINDINGS,
            &[
                // So does a review card.
                row!(
                    "Left",
                    "fold the card to its line, or open it (also its chevron)",
                    |t| {
                        (t.on().is_some() && t.findings() && t.stage != Stage::PlanReview)
                            .then_some(TabKey::Out(PanelOutcome::FoldCard(t.finding, false)))
                    }
                ),
                row!(
                    "Right",
                    "fold the card to its line, or open it (also its chevron)",
                    |t| {
                        (t.on().is_some() && t.findings() && t.stage != Stage::PlanReview)
                            .then_some(TabKey::Out(PanelOutcome::FoldCard(t.finding, true)))
                    }
                ),
                row!("Up", "previous or next finding", |t| t
                    .step(FINDINGS, Step::Back)),
                row!("Down", "previous or next finding", |t| t
                    .step(FINDINGS, Step::On)),
                row!("Home", "first or last finding", |t| t
                    .step(FINDINGS, Step::First)),
                row!("End", "first or last finding", |t| t
                    .step(FINDINGS, Step::Last)),
                row!("Shift+F", "fix all open", |t| {
                    (t.stage != Stage::PlanReview && offers_fix_all(t.feature, t.stage))
                        .then(|| TabKey::Out(want_all_outcome(t.stage)))
                }),
                row!(
                    "Alt+E",
                    "fold the earlier reviews and the fix round cards, or open them all",
                    |t| t.folds(&[Stage::AiReview])
                ),
            ],
        ),
        Link::Rows(
            SUGGESTIONS,
            &[
                row!(
                    "Left",
                    "fold the card to its line, or open it (also its chevron)",
                    |t| {
                        (t.on().is_some() && t.findings() && t.stage == Stage::PlanReview)
                            .then_some(TabKey::Out(PanelOutcome::FoldCard(t.finding, false)))
                    }
                ),
                row!(
                    "Right",
                    "fold the card to its line, or open it (also its chevron)",
                    |t| {
                        (t.on().is_some() && t.findings() && t.stage == Stage::PlanReview)
                            .then_some(TabKey::Out(PanelOutcome::FoldCard(t.finding, true)))
                    }
                ),
                row!("Up", "previous or next suggestion", |t| t
                    .step(SUGGESTIONS, Step::Back)),
                row!("Down", "previous or next suggestion", |t| t
                    .step(SUGGESTIONS, Step::On)),
                row!("Home", "first or last suggestion", |t| t
                    .step(SUGGESTIONS, Step::First)),
                row!("End", "first or last suggestion", |t| t
                    .step(SUGGESTIONS, Step::Last)),
                row!("Shift+F", "accept all open", |t| {
                    (t.stage == Stage::PlanReview && offers_fix_all(t.feature, t.stage))
                        .then(|| TabKey::Out(want_all_outcome(t.stage)))
                }),
            ],
        ),
        Link::Rows(
            STAGES,
            &[
                row!("Left", "previous or next stage", |t| t
                    .page()
                    .then_some(TabKey::Step(Step::Back))),
                row!("Right", "previous or next stage", |t| t
                    .page()
                    .then_some(TabKey::Step(Step::On))),
                row!("Up", "previous or next stage", |t| t
                    .step(STAGES, Step::Back)),
                row!("Down", "previous or next stage", |t| t
                    .step(STAGES, Step::On)),
                row!("Ctrl+K", "previous or next stage", |t| t
                    .page()
                    .then_some(TabKey::Step(Step::Back))),
                row!("Ctrl+J", "previous or next stage", |t| t
                    .page()
                    .then_some(TabKey::Step(Step::On))),
                row!("Home", "first or last stage", |t| t
                    .step(STAGES, Step::First)),
                row!("End", "first or last stage", |t| t.step(STAGES, Step::Last)),
                // A check's card with the keyboard.
                row!("Enter", "a check: show its terminal", |t| {
                    (t.findings() && !t.checks.is_empty()).then(|| {
                        let row = t.checks.get(t.finding).and_then(|c| c.row);
                        row.map_or(TabKey::Swallow, |r| TabKey::Out(PanelOutcome::ShowCheck(r)))
                    })
                }),
                alias!("Shift+Enter", |t| {
                    (t.findings() && !t.checks.is_empty()).then(|| {
                        let row = t.checks.get(t.finding).and_then(|c| c.row);
                        row.map_or(TabKey::Swallow, |r| TabKey::Out(PanelOutcome::ShowCheck(r)))
                    })
                }),
                row!("r", "a check: run it again", |t| {
                    (t.findings() && !t.checks.is_empty()).then(|| {
                        let check = t.checks.get(t.finding).filter(|c| !c.skipped);
                        (check.and_then(|c| c.row)).map_or(TabKey::Swallow, |r| {
                            TabKey::Out(PanelOutcome::RerunCheck(r))
                        })
                    })
                }),
                row!("s", "a check: stop it", |t| {
                    (t.findings() && !t.checks.is_empty()).then(|| {
                        let row = t.checks.get(t.finding).and_then(|c| c.row);
                        // One that ended has nothing left to stop, one told
                        // already ends by itself.
                        let check =
                            row.and_then(|r| check_rows(t.feature, t.stage).get(r).map(|c| (r, c)));
                        match check {
                            Some((r, c)) if c.outcome.is_none() && !c.stopping => {
                                TabKey::Out(PanelOutcome::StopCheck(r))
                            }
                            _ => TabKey::Swallow,
                        }
                    })
                }),
                row!("Space", "a check: skip it, or stop skipping it", |t| {
                    (t.findings() && !t.checks.is_empty()).then(|| {
                        let skip = (t.checks.get(t.finding))
                            .and_then(|c| skip_action(t.feature, t.stage, c, t.ctx));
                        skip.map_or(TabKey::Swallow, |(_, o)| TabKey::Out(o))
                    })
                }),
                // All of them, from anywhere on the page.
                row!(
                    "Shift+R",
                    "Verify: run the failed checks; reviewing a pull request at its head, run the checks here",
                    |t| {
                        (t.page() && has_failed_checks(t.feature, t.stage))
                            .then_some(TabKey::Out(PanelOutcome::RunFailed))
                    }
                ),
                // A review at its head: the project's checks, here.
                row!(
                    "Shift+R",
                    "Verify: run the failed checks; reviewing a pull request at its head, run the checks here",
                    |t| {
                        let here = t.stage == Stage::Verify
                            && t.ctx.on_head
                            && t.ctx.checks
                            && t.feature.current() == Some(Stage::Verify);
                        (t.page() && here).then_some(TabKey::Out(PanelOutcome::RunChecks))
                    }
                ),
                row!("Shift+S", "Verify: stop all checks", |t| {
                    (t.page() && t.stage == Stage::Verify && can_stop_checks(t.feature))
                        .then_some(TabKey::Out(PanelOutcome::StopChecks))
                }),
                row!(
                    "Alt+E",
                    "Verify, Build: fold the fix round cards; Discover, Plan: the draft cards; or open them all",
                    |t| t.folds(&[])
                ),
            ],
        ),
        Link::Rows(
            CODE_REVIEW,
            &[
                row!("Up", "fixes: previous or next comment", |t| t
                    .step(CODE_REVIEW, Step::Back)),
                row!("Down", "fixes: previous or next comment", |t| t
                    .step(CODE_REVIEW, Step::On)),
                row!("Home", "fixes: first or last comment", |t| t
                    .step(CODE_REVIEW, Step::First)),
                row!("End", "fixes: first or last comment", |t| t
                    .step(CODE_REVIEW, Step::Last)),
                row!(
                    "Alt+1",
                    "show the code, or the fixes the comments asked for",
                    |t| {
                        let views = t.stage == Stage::CodeReview
                            && crate::feature_diff::review_views(t.feature);
                        views.then_some(TabKey::Out(PanelOutcome::ReviewView(
                            chrome::ReviewView::Code,
                        )))
                    }
                ),
                row!(
                    "Alt+2",
                    "show the code, or the fixes the comments asked for",
                    |t| {
                        let views = t.stage == Stage::CodeReview
                            && crate::feature_diff::review_views(t.feature);
                        views.then_some(TabKey::Out(PanelOutcome::ReviewView(
                            chrome::ReviewView::Fixes,
                        )))
                    }
                ),
                row!(
                    "Alt+E",
                    "fixes: fold the fix round cards, or open them all",
                    |t| t.folds(&[Stage::CodeReview])
                ),
                // The card with the keyboard: the diff's keys for a comment.
                row!("f", "a comment: it has nothing to fix or accept", |t| {
                    let c = t.note().filter(|_| t.findings())?;
                    out(PanelOutcome::FindingRefused(match c.flag.is_some() {
                        true => SENT_NOTE,
                        false => DIFF_NOTE,
                    }))
                }),
                row!("Ctrl+E", "fixes: edit a comment not sent yet", |t| {
                    let c = t.note().filter(|_| t.findings())?;
                    out(match t.refused {
                        Some(why) => PanelOutcome::FindingRefused(why),
                        None => PanelOutcome::EditDiffNote(c.clone()),
                    })
                }),
                row!("c", "fixes: edit a comment not sent yet", |t| {
                    let c = t.note().filter(|_| t.findings())?;
                    out(match t.refused {
                        Some(why) => PanelOutcome::FindingRefused(why),
                        None => PanelOutcome::EditDiffNote(c.clone()),
                    })
                }),
                row!(
                    "Del",
                    "fixes: delete a comment not sent yet, pressed twice",
                    |t| {
                        let c = t.note().filter(|_| t.findings())?;
                        if let Some(why) = t.refused {
                            return out(PanelOutcome::FindingRefused(why));
                        }
                        // The first press arms it (checklist C4): the next card
                        // takes its place, so a press too many would hit that one.
                        let key = format!("note:{}:{}", note_id(c), c.text);
                        if t.armed.as_ref() != Some(&key) {
                            return Some(TabKey::Arm(key));
                        }
                        out(PanelOutcome::DeleteDiffNote(c.clone()))
                    }
                ),
                row!("Enter", "fixes: show the comment in the diff", |t| {
                    let c = t.note().filter(|_| t.findings())?;
                    out(match (crate::review::is_general(c), t.refused) {
                        (true, Some(why)) => PanelOutcome::FindingRefused(why),
                        (true, None) => PanelOutcome::EditDiffNote(c.clone()),
                        (false, _) => PanelOutcome::ShowDiffNote(c.clone()),
                    })
                }),
                alias!("Shift+Enter", |t| {
                    let c = t.note().filter(|_| t.findings())?;
                    out(match (crate::review::is_general(c), t.refused) {
                        (true, Some(why)) => PanelOutcome::FindingRefused(why),
                        (true, None) => PanelOutcome::EditDiffNote(c.clone()),
                        (false, _) => PanelOutcome::ShowDiffNote(c.clone()),
                    })
                }),
            ],
        ),
        // An item's card: the same keys for either kind.
        Link::Cards,
        // Off an item, the card keys do nothing.
        Link::Rows(
            FINDINGS,
            &[
                row!("f", "fix the finding, or take that back", |t| t
                    .findings()
                    .then_some(TabKey::Swallow)),
                row!("Ctrl+E", "edit its fix, or a comment of your own", |t| t
                    .findings()
                    .then_some(TabKey::Swallow)),
                row!(
                    "Del",
                    "dismiss it, saying why, or reopen it; a comment of your own: press twice to delete",
                    |t| t.findings().then_some(TabKey::Swallow)
                ),
                row!("Enter", "show it in the diff", |t| t
                    .findings()
                    .then_some(TabKey::Swallow)),
                // Enter on a card, with Shift as without.
                alias!("Shift+Enter", |t| {
                    t.findings().then(|| match t.item() {
                        Some(_) => TabKey::Card(CardKey::Enter),
                        None => TabKey::Swallow,
                    })
                }),
                row!("c", "off the cards, add a comment of your own", |t| {
                    let own = t.stage != Stage::PlanReview && t.stage != Stage::CodeReview;
                    (own && t.page() && can_add_comment(t.feature, t.stage))
                        .then_some(TabKey::Out(PanelOutcome::AddComment))
                }),
            ],
        ),
        Link::Rows(
            SUGGESTIONS,
            &[
                row!("c", "off the cards, add a suggestion of your own", |t| {
                    (t.stage == Stage::PlanReview
                        && t.page()
                        && can_add_comment(t.feature, t.stage))
                    .then_some(TabKey::Out(PanelOutcome::AddComment))
                }),
                // Its suggestions decided, off the repositories as well: the
                // final plan, or the plan as it is; Ctrl+Shift+Enter asks the
                // reviewer. Before Reopen's: the stage is current.
                row!(
                    "Ctrl+Enter",
                    "write the final plan, or use the plan as it is, once none is left to decide",
                    |t| {
                        let deciding = plan_review_open(t.feature, t.stage)
                            && crate::feature_plan_review::deciding(t.feature).is_some();
                        (t.page() && deciding).then_some(TabKey::Out(PanelOutcome::WriteFinalPlan))
                    }
                ),
                row!(
                    "Ctrl+Shift+Enter",
                    "ask the reviewer about your comments",
                    |t| {
                        let deciding = plan_review_open(t.feature, t.stage)
                            && crate::feature_plan_review::deciding(t.feature).is_some();
                        (t.page() && deciding).then_some(TabKey::Out(PanelOutcome::AskReviewer))
                    }
                ),
            ],
        ),
        Link::Rows(
            CODE_REVIEW,
            &[row!(
                "c",
                "fixes: off the cards, add a comment of your own",
                |t| {
                    (t.stage == Stage::CodeReview
                        && t.page()
                        && can_add_comment(t.feature, t.stage))
                    .then_some(TabKey::Out(PanelOutcome::AddComment))
                }
            )],
        ),
        Link::Rows(
            FINDINGS,
            &[
                // Its fixes, off the repositories as well: Ctrl+Enter makes
                // them while no plan is needed, Ctrl+Shift+Enter always plans
                // them first, as on every page.
                row!(
                    "Ctrl+Enter",
                    "make the findings' fixes when nothing you wrote needs a plan",
                    |t| {
                        let send =
                            sends_findings(t.feature, t.stage, t.ctx).filter(|_| t.page())?;
                        out(match send == PanelOutcome::MakeFindingFixes {
                            true => send,
                            false => PanelOutcome::FindingRefused(PLAN_FIRST),
                        })
                    }
                ),
                row!(
                    "Ctrl+Shift+Enter",
                    "plan the fixes of the findings first",
                    |t| {
                        sends_findings(t.feature, t.stage, t.ctx).filter(|_| t.page())?;
                        out(PanelOutcome::SendFixes)
                    }
                ),
            ],
        ),
        Link::Rows(
            STAGES,
            &[
                // Verify's, the same way, off its check cards too.
                row!("Ctrl+Enter", "Verify: make its failures' fixes", |t| {
                    (t.page() && verify_sends(t.feature, t.stage))
                        .then_some(TabKey::Out(PanelOutcome::MakeVerifyFixes))
                }),
                row!(
                    "Ctrl+Shift+Enter",
                    "Verify: plan its failures' fixes",
                    |t| {
                        (t.page() && verify_sends(t.feature, t.stage))
                            .then_some(TabKey::Out(PanelOutcome::PlanFixes))
                    }
                ),
                // A locked stage's is refused when done, saying why; any
                // other stage has nothing to reopen.
                row!(
                    "Ctrl+Shift+Enter",
                    "reopen a done stage, unless it is locked",
                    |t| {
                        let on = t.focus == TabArea::Stages
                            || (t.findings() && t.shown > 0 && t.criteria == 0);
                        let done = stage_mark(t.feature, t.stage) == StageMark::Done;
                        on.then_some(match done {
                            true => TabKey::Out(PanelOutcome::Untick(t.stage)),
                            false => TabKey::Swallow,
                        })
                    }
                ),
                // The file's review has it too.
                row!(
                    "Ctrl+Enter",
                    "make the fixes of a fix plan ready for review, or again once its agent stopped with fixes left",
                    |t| {
                        let on = t.focus == TabArea::Stages || (t.findings() && t.criteria > 0);
                        let fixes =
                            fix_round_on(t.feature, t.stage).is_some_and(FixRound::can_make_fixes);
                        (on && fixes).then_some(TabKey::Out(PanelOutcome::MakeFixes))
                    }
                ),
                alias!("Ctrl+Shift+Enter", |t| {
                    let on = t.focus == TabArea::Stages || (t.findings() && t.criteria > 0);
                    let fixes =
                        fix_round_on(t.feature, t.stage).is_some_and(FixRound::can_make_fixes);
                    (on && fixes).then_some(TabKey::Out(PanelOutcome::MakeFixes))
                }),
                // The gate passes from any focus on the page, cards included.
                row!(
                    "Ctrl+Enter",
                    "approve a brief or plan ready for review",
                    |t| {
                        approves_doc(t.feature, t.stage, t.ctx)
                            .then_some(TabKey::Out(PanelOutcome::ApproveDoc(t.stage)))
                    }
                ),
                // Written, but not read yet: say why it can't be approved.
                row!(
                    "Ctrl+Enter",
                    "approve a brief or plan ready for review",
                    |t| {
                        doc_unread(t.feature, t.stage, t.ctx)
                            .then_some(TabKey::Out(PanelOutcome::FindingRefused(DOC_UNREAD)))
                    }
                ),
                row!(
                    "Enter",
                    "the page's main button; on a done stage, go to the current one",
                    |t| {
                        (t.focus == TabArea::Stages).then(|| {
                            page_enter(t.feature, t.stage, t.ctx)
                                .map_or(TabKey::Swallow, TabKey::Out)
                        })
                    }
                ),
                row!(
                    "Alt+W",
                    "reviewing someone else's pull request: fix it here and push the fixes from Ship, asking first, or comment on it again",
                    |t| {
                        (t.feature.pull().is_some_and(|p| !p.own()))
                            .then_some(TabKey::Out(PanelOutcome::FixHere))
                    }
                ),
                row!(
                    "Alt+N",
                    "reviewing pull requests: add those of other repositories, or leave some out, in the review dialog",
                    |t| {
                        changes_pulls(t.feature).then_some(TabKey::Out(PanelOutcome::EditPulls))
                    }
                ),
                row!(
                    "Ctrl+Shift+O",
                    "Verify's evidence, or a fix plan already sent; else, reviewing a pull request, its page on GitHub",
                    |t| t.page().then(|| t.open_page())
                ),
                row!(
                    "Ctrl+Shift+R",
                    "show the stage's agent, or resume its chat",
                    |t| {
                        Some(
                            agent_key(t.feature, t.stage, t.ctx)
                                .map_or(TabKey::Swallow, |(_, o)| TabKey::Out(o)),
                        )
                    }
                ),
                row!("Ctrl+R", "start the stage's agent afresh", |t| {
                    Some(fresh_stage(t.feature, t.row).map_or(TabKey::Swallow, |s| {
                        TabKey::Out(PanelOutcome::StartAgent(s, true))
                    }))
                }),
                row!(
                    "Ctrl+Shift+A",
                    "pick the stage's next agent, or the one its fixes go to",
                    |t| keyed_stage(t.feature, t.stage, t.findings())
                        .map(|s| TabKey::Out(PanelOutcome::CycleAgent(s)))
                ),
                row!(
                    MODEL_KEY,
                    "pick the model of the stage's agent, or of the one its fixes go to",
                    |t| keyed_stage(t.feature, t.stage, t.findings())
                        .map(|s| TabKey::Out(PanelOutcome::PickModel(s)))
                ),
                row!(
                    "Ctrl+Shift+W",
                    "pick the machine the stage's next agent or check run goes on",
                    |t| {
                        takes_a_place(t.stage)
                            .then_some(TabKey::Out(PanelOutcome::CycleMachine(t.stage)))
                    }
                ),
                row!(
                    "Ctrl+Shift+Q",
                    "stop the stage's agent on another machine; Manual test: stop the sandbox there",
                    |t| {
                        stops_on_machine(t.feature, t.stage)
                            .then_some(TabKey::Out(PanelOutcome::StopOnMachine(t.stage)))
                    }
                ),
                // Ctrl+Shift+T shows its terminals there, where this would stop it.
                row!(
                    "Ctrl+Shift+Q",
                    "stop the stage's agent on another machine; Manual test: stop the sandbox there",
                    |t| {
                        let f = t.feature;
                        let on = offers_sandbox(f, t.stage)
                            && sandbox_elsewhere(f)
                            && (t.ctx.sandbox_busy || sandbox_alive(f));
                        on.then_some(TabKey::Out(PanelOutcome::StopSandbox))
                    }
                ),
                row!(
                    "Ctrl+Shift+E",
                    "make a feature another machine writes this machine's, or hand one back to the machine it came from",
                    |t| {
                        let f = t.feature;
                        (f.writer.is_some() && f.status == Status::Open)
                            .then_some(TabKey::Out(PanelOutcome::TakeOver))
                    }
                ),
                row!(
                    "Ctrl+Shift+E",
                    "make a feature another machine writes this machine's, or hand one back to the machine it came from",
                    |t| { hands_back(t.feature).then_some(TabKey::Out(PanelOutcome::HandBack)) }
                ),
                row!(
                    "Ctrl+Shift+B",
                    "Build: one agent, or a fresh agent per milestone",
                    |t| {
                        let f = t.feature;
                        (milestone_switch(f, t.stage)).then_some({
                            TabKey::Out(PanelOutcome::FreshMilestones(!f.fresh_milestones))
                        })
                    }
                ),
                row!(
                    "Ctrl+Shift+U",
                    "copy what the worktrees lack: Build and Verify",
                    |t| {
                        offers_setup(t.feature, t.stage).then_some(TabKey::Out(PanelOutcome::SetUp))
                    }
                ),
                row!(
                    "Ctrl+Shift+I",
                    "what the setup runs and printed: Build and Verify",
                    |t| {
                        offers_setup_log(t.feature, t.stage)
                            .then_some(TabKey::Out(PanelOutcome::SetupLog))
                    }
                ),
                row!(
                    "Ctrl+E",
                    "edit the project's commands: Build, Verify, Manual test",
                    |t| {
                        let (f, s) = (t.feature, t.stage);
                        (offers_setup(f, s) || offers_sandbox(f, s))
                            .then(|| TabKey::Out(PanelOutcome::EditCommands(commands_tab(s))))
                    }
                ),
                row!(
                    "Ctrl+Shift+T",
                    "Manual test: start or stop the sandbox, or show its terminals if on another machine",
                    |t| {
                        let f = t.feature;
                        if !offers_sandbox(f, t.stage) {
                            return None;
                        }
                        if sandbox_elsewhere(f) && sandbox_alive(f) {
                            return out(PanelOutcome::ShowSandbox);
                        }
                        out(match t.ctx.sandbox_busy || sandbox_alive(f) {
                            true => PanelOutcome::StopSandbox,
                            false => PanelOutcome::StartSandbox,
                        })
                    }
                ),
                row!("Ctrl+Shift+Y", "Manual test: open the sandbox's app", |t| {
                    (offers_sandbox(t.feature, t.stage) && sandbox_alive(t.feature))
                        .then_some(TabKey::Out(PanelOutcome::OpenSandbox))
                }),
                row!(
                    "Alt+Y",
                    "Manual test: open the sandbox's next page, as the buttons after Open app do",
                    |t| {
                        let f = t.feature;
                        (offers_sandbox(f, t.stage)
                            && sandbox_alive(f)
                            && sandbox_pages(f).len() > 1)
                            .then_some(TabKey::Out(PanelOutcome::NextSandboxPage))
                    }
                ),
                row!(
                    "Ctrl+Shift+N",
                    "Manual test: open the sandbox to your network, for a phone, or close it",
                    |t| {
                        let f = t.feature;
                        (offers_sandbox(f, t.stage) && sandbox_alive(f) && !sandbox_elsewhere(f))
                            .then_some(TabKey::Out(PanelOutcome::SandboxLan))
                    }
                ),
                row!("Ctrl+Shift+G", "Manual test: restart the sandbox", |t| {
                    (offers_sandbox(t.feature, t.stage) && sandbox_alive(t.feature))
                        .then_some(TabKey::Out(PanelOutcome::RestartSandbox))
                }),
                row!(
                    "Ctrl+Shift+X",
                    "Manual test: reset the sandbox's data, or remove it once stopped",
                    |t| {
                        let f = t.feature;
                        if !offers_sandbox(f, t.stage) {
                            return None;
                        }
                        match (sandbox_alive(f), f.sandbox.is_some()) {
                            (true, _) => out(PanelOutcome::ResetSandbox),
                            (false, true) if !t.ctx.sandbox_busy => {
                                out(PanelOutcome::RemoveSandbox)
                            }
                            _ => Some(TabKey::Swallow),
                        }
                    }
                ),
                row!("Ctrl+Shift+K", "skip Plan review", |t| plan_review_open(
                    t.feature, t.stage
                )
                .then_some(TabKey::Out(PanelOutcome::SkipPlanReview))),
                row!(
                    "Ctrl+Shift+D",
                    "change the decisions on a final plan",
                    |t| {
                        let writing = t.feature.plan_review.as_ref().is_some_and(|r| r.writing);
                        (plan_review_open(t.feature, t.stage) && writing)
                            .then_some(TabKey::Out(PanelOutcome::ChangeDecisions))
                    }
                ),
                row!(
                    START_OVER_KEY,
                    "start the current stage over, or drop its fix plan, after asking (not Discover: Ctrl+R starts its agent afresh)",
                    |t| {
                        if fix_round_on(t.feature, t.stage).is_some_and(FixRound::can_drop) {
                            return out(PanelOutcome::DropFixPlan);
                        }
                        Some(
                            start_over_stage(t.feature, t.stage, t.ctx)
                                .map_or(TabKey::Swallow, |s| {
                                    TabKey::Out(PanelOutcome::StartOver(s))
                                }),
                        )
                    }
                ),
            ],
        ),
        // With a fix plan under way a criterion's card has no use for it.
        Link::Rows(
            CRITERIA,
            &[
                row!(
                    "Ctrl+Enter",
                    "mark Manual test passed, or make the fixes of its failures",
                    |t| { (t.findings() && t.criteria > 0).then_some(TabKey::Swallow) }
                ),
                alias!("Ctrl+Shift+Enter", |t| {
                    (t.findings() && t.criteria > 0).then_some(TabKey::Swallow)
                }),
            ],
        ),
        // Enter with a modifier no row above takes: the page's Enter, or
        // the repositories', as the old match's last arm had it.
        Link::Rows(
            STAGES,
            &[
                alias!("Shift+Enter", |t| Some(t.any_enter())),
                alias!("Ctrl+Enter", |t| Some(t.any_enter())),
                alias!("Alt+Enter", |t| Some(t.any_enter())),
                alias!("Ctrl+Shift+Enter", |t| Some(t.any_enter())),
            ],
        ),
        Link::Rows(
            SUGGESTIONS,
            &[row!("Ctrl+O", "open the code it cites", |t| {
                let suggestion = t.item().is_some_and(|i| i.of() == ItemOf::Suggestion);
                (t.findings() && suggestion).then_some(TabKey::CardOpen)
            })],
        ),
        Link::Rows(
            ANYWHERE,
            &[
                row!(
                    "F1",
                    "list these keys; Esc goes back to the page",
                    |_| Some(TabKey::ShowKeys)
                ),
                row!(
                    "Tab",
                    "stages, findings, suggestions, checks, criteria or repositories",
                    |t| Some(t.tab(false))
                ),
                row!(
                    "Shift+Tab",
                    "stages, findings, suggestions, checks, criteria or repositories",
                    |t| Some(t.tab(true))
                ),
                row!("Ctrl+Page_Up", "previous or next feature", |t| {
                    Some(
                        switch(t.slugs, &t.panel.slug, true)
                            .map_or(TabKey::Swallow, |s| TabKey::Out(PanelOutcome::Show(s))),
                    )
                }),
                row!("Ctrl+Page_Down", "previous or next feature", |t| {
                    Some(
                        switch(t.slugs, &t.panel.slug, false)
                            .map_or(TabKey::Swallow, |s| TabKey::Out(PanelOutcome::Show(s))),
                    )
                }),
                row!("Ctrl+O", "open a tab in the feature's folder", |t| out(
                    PanelOutcome::Open(t.feature.dir.clone())
                )),
                row!("Ctrl+C", "copy the feature's path", |_| out(
                    PanelOutcome::CopyPath
                )),
                row!("Ctrl+N", "new feature", |_| out(PanelOutcome::New)),
                // Ctrl+C and Ctrl+N took Shift too; Ctrl+Shift+N opens a
                // running sandbox to the network first, as hinted.
                alias!("Ctrl+Shift+C", |_| out(PanelOutcome::CopyPath)),
                alias!("Ctrl+Shift+N", |_| out(PanelOutcome::New)),
                row!("Ctrl+Shift+L", "show or hide all repositories", |_| Some(
                    TabKey::FoldRepos
                )),
                // Never a bare Del: one slip of the focus would abandon it.
                row!(
                    REMOVE_KEY,
                    "abandon the feature, or remove its worktrees once done, after asking",
                    |_| out(PanelOutcome::Remove)
                ),
            ],
        ),
    ]
}

/// The footer's hints of the tab, from the chain: a hint shows what the
/// key does now, in `words`' terms; a key offered whose words are empty
/// isn't hinted.
struct Footer<'t, 'a> {
    t: &'t TabCtx<'a>,
    /// The page's buttons: a key doing what one does says what it says.
    offered: Vec<(PageButton, PanelOutcome)>,
}

impl Footer<'_, '_> {
    /// What the button asking `outcome` says, else `action_hint`'s words.
    fn said(&self, outcome: &PanelOutcome) -> String {
        (self.offered.iter())
            .find(|(_, o)| o == outcome)
            .map_or_else(
                || action_hint(outcome).to_owned(),
                |(b, _)| lower_first(&b.label),
            )
    }

    /// The hint `keys` (its first key pressed now), in `words`.
    fn key(&self, keys: &str, words: impl FnOnce(&TabKey) -> String) -> Option<(String, String)> {
        let what = words(&self.t.press_chord(keys)?);
        (!what.is_empty()).then(|| (keys.to_owned(), what))
    }

    /// The hint `keys`, in the words of the button doing it, for an outcome
    /// `want` takes.
    fn said_key(
        &self,
        keys: &str,
        want: impl FnOnce(&PanelOutcome) -> bool,
    ) -> Option<(String, String)> {
        self.key(keys, |k| match k {
            TabKey::Out(o) if want(o) => self.said(o),
            _ => String::new(),
        })
    }

    /// The hint `keys` in `what` while it does `want`.
    fn says(
        &self,
        keys: &str,
        what: &str,
        want: impl FnOnce(&TabKey) -> bool,
    ) -> Option<(String, String)> {
        self.key(keys, |k| match want(k) {
            true => what.to_owned(),
            false => String::new(),
        })
    }

    /// Tab, named by where it goes.
    fn tab(&self) -> Option<(String, String)> {
        let t = self.t;
        self.key("Tab", |k| {
            let TabKey::Focus(to) = k else {
                return String::new();
            };
            match to {
                _ if *to == t.focus => "",
                TabArea::Stages => "stages",
                TabArea::Repos => "repositories",
                TabArea::Findings if !t.ships.is_empty() => "steps",
                TabArea::Findings if !t.cards.is_empty() => words(stage_items(t.stage)).items,
                TabArea::Findings if !t.checks.is_empty() => "checks",
                TabArea::Findings => "criteria",
            }
            .to_owned()
        })
    }

    /// Ctrl+Enter and Ctrl+Shift+Enter sending `stage`'s findings or
    /// failures to be fixed: making the fixes when no plan is needed,
    /// always planning them first.
    fn sends(&self) -> Vec<(String, String)> {
        use PanelOutcome as O;
        let make = self.said_key("Ctrl+Enter", |o| {
            matches!(o, O::MakeVerifyFixes | O::MakeFindingFixes)
        });
        let plan = self.said_key("Ctrl+Shift+Enter", |o| {
            matches!(o, O::PlanFixes | O::SendFixes)
        });
        make.into_iter().chain(plan).collect()
    }

    /// Manual test's gate where its criteria are tried: Ctrl+Enter makes
    /// the fixes (or marks it passed with none to send), Ctrl+Shift+Enter
    /// plans them.
    fn manual_gate(&self) -> Vec<(String, String)> {
        use crate::manual_test::Outcome;
        let gate = self.said_key("Ctrl+Enter", |o| {
            matches!(
                o,
                PanelOutcome::Manual(Outcome::MakeFixes | Outcome::MarkPassed)
            )
        });
        let send = self.said_key("Ctrl+Shift+Enter", |o| {
            matches!(o, PanelOutcome::Manual(Outcome::SendFailures))
        });
        gate.into_iter().chain(send).collect()
    }

    /// Plan review's keys while its suggestions are decided: going on once
    /// none is left, and asking the reviewer about the notes on them.
    fn plan_review(&self) -> Vec<(String, String)> {
        let Some(review) = crate::feature_plan_review::deciding(self.t.feature) else {
            return Vec::new();
        };
        let go_on = match review.any_accepted() {
            true => "write final plan",
            false => "use the plan as it is",
        };
        let gate = self.says("Ctrl+Enter", go_on, |k| {
            review.open() == 0 && *k == TabKey::Out(PanelOutcome::WriteFinalPlan)
        });
        let ask = self.says("Ctrl+Shift+Enter", "ask reviewer", |k| {
            !feature::plan_review::notes(review).is_empty()
                && *k == TabKey::Out(PanelOutcome::AskReviewer)
        });
        gate.into_iter().chain(ask).collect()
    }

    /// Alt+E, named by what it folds.
    fn fold(&self) -> Option<(String, String)> {
        let (feature, stage) = (self.t.feature, self.t.stage);
        let what = fold_hint(feature, stage).map_or(String::new(), |(_, w)| w);
        self.says("Alt+E", &what, |k| *k == TabKey::ToggleRounds)
    }

    /// Reopen's key while the stage reopens: a locked one refuses, saying
    /// why, and isn't hinted. `what` says it, else its button.
    fn reopen(&self, what: Option<&str>) -> Option<(String, String)> {
        let (feature, stage) = (self.t.feature, self.t.stage);
        let row = row_action(feature, PanelRow::Stage(stage), self.t.ctx);
        if !matches!(row, Some(PanelOutcome::Untick(_))) {
            return None;
        }
        self.key(REOPEN_KEY, |k| match (k, what) {
            (TabKey::Out(PanelOutcome::Untick(_)), Some(what)) => what.to_owned(),
            (TabKey::Out(o @ PanelOutcome::Untick(_)), None) => self.said(o),
            _ => String::new(),
        })
    }

    /// Del on a card the user may delete, a first press arming it.
    fn delete(&self, word: &str) -> String {
        match self.t.panel.armed.is_some() {
            true => "again to delete".to_owned(),
            false => word.to_owned(),
        }
    }
}

/// What can be picked of a stage's next agent, for the feature tab's footer.
#[derive(Clone, Copy, Default)]
pub(super) struct AgentChoices<'a> {
    pub(super) picks: &'a [AgentChip],
    /// Its model too.
    pub(super) model: bool,
    /// The machine it runs on too.
    pub(super) place: bool,
    /// The same of the agent fixes go to, on a page that sends them
    /// (`fix_page`).
    pub(super) fix_picks: &'a [AgentChip],
    pub(super) fix_model: bool,
}

/// Whether `stage`'s page of `feature` offers to pick the agent its fixes
/// go to (Build's): a stage that sends fixes, at it, while they can go.
pub(super) fn fix_page(feature: &Feature, stage: Stage) -> bool {
    matches!(
        stage,
        Stage::AiReview | Stage::Verify | Stage::CodeReview | Stage::ManualTest
    ) && feature.current() == Some(stage)
        && crate::feature_agents::fires(feature, Stage::Build)
}

/// The stage whose agent Ctrl+Shift+A and Alt+Shift+A pick on `stage`'s
/// page: its own while one can start, else the one fixes go to; on AI
/// review, that one while the keyboard is on the findings.
pub(super) fn keyed_stage(feature: &Feature, stage: Stage, findings: bool) -> Option<Stage> {
    let own = crate::feature_agents::fires(feature, stage);
    match (own, fix_page(feature, stage)) {
        (true, true) if findings => Some(Stage::Build),
        (true, _) => Some(stage),
        (false, true) => Some(Stage::Build),
        (false, false) => None,
    }
}

/// The feature tab's key hints over `feature`, one of `slugs`, as `panel`
/// has the keyboard on `stage`'s page.
#[allow(clippy::too_many_arguments)]
pub(super) fn tab_hints(
    panel: &FeaturePanel,
    feature: &Feature,
    stage: Stage,
    slugs: &[String],
    ctx: RowCtx,
    choices: AgentChoices,
    plan: &[PlannedCheck],
    ships: &[ShipRow],
    notes: &[Comment],
) -> Vec<(String, String)> {
    let armed = panel.armed.clone();
    let t = TabCtx::new(panel, feature, stage, slugs, ctx, plan, ships, notes, armed);
    let f = Footer {
        t: &t,
        offered: page_actions(feature, stage, ctx),
    };
    let mut hints = page_hints(&f, choices);
    // Text selected on the page goes first: what it is for is copying.
    let typing = panel.search.as_ref().is_some_and(|s| s.typing);
    let picked = panel.pick.as_ref().is_some_and(|p| p.anchor != p.cursor);
    if picked && !panel.keys && panel.note.is_none() && !typing {
        hints.retain(|(k, _)| k != "Esc");
        hints.splice(
            0..0,
            crate::page_text::pick_hints(shows_criteria(feature, stage)),
        );
    }
    let quiet = panel.keys || panel.note.is_some();
    if !quiet {
        // The view switch's key doing something here, in its words.
        let view = panel.review_view_of(feature);
        let to = |v: chrome::ReviewView| {
            move |k: &TabKey| v != view && *k == TabKey::Out(PanelOutcome::ReviewView(v))
        };
        hints.extend(f.says("Alt+1", "code review", to(chrome::ReviewView::Code)));
        hints.extend(f.says("Alt+2", "fixes", to(chrome::ReviewView::Fixes)));
    }
    if feature.writer.is_some() {
        for (key, what) in &mut hints {
            // Enter goes by the page's own action, which only reads a file there.
            if key == "Enter"
                && let Some(rest) = what.strip_prefix("review ")
            {
                *what = format!("open {rest}");
            }
        }
    }
    if !quiet {
        hints.extend(f.key("Ctrl+Shift+E", |k| match k {
            TabKey::Out(PanelOutcome::TakeOver) => "take over".to_owned(),
            TabKey::Out(PanelOutcome::HandBack) => "hand back".to_owned(),
            _ => String::new(),
        }));
    }
    hints
}

/// `tab_hints` but for what the whole tab adds: the page's and the area's
/// with the keyboard.
fn page_hints(f: &Footer, choices: AgentChoices) -> Vec<(String, String)> {
    use PanelOutcome as O;
    let t = f.t;
    let (panel, feature, stage, ctx) = (t.panel, t.feature, t.stage, t.ctx);
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    if panel.keys {
        // Any key puts the list away (`FeaturePanel::key`).
        return vec![hint("Esc", "back"), hint("F1", "hide the keys")];
    }
    if let Some((on, field)) = &panel.note {
        return crate::manual_test::note_hints(on, field.trim().is_empty());
    }
    let typing = panel.search.as_ref().is_some_and(|s| s.typing);
    if typing && shows_criteria(feature, stage) {
        return crate::manual_test::search_hints(panel.search.as_ref());
    }
    let mut hints = Vec::new();
    let h = &mut hints;
    let going = verify_going(feature, stage, ctx);
    let is = |want: TabKey| move |k: &TabKey| *k == want;
    let outs = |want: fn(&O) -> bool| move |k: &TabKey| matches!(k, TabKey::Out(o) if want(o));
    match t.focus {
        TabArea::Repos => {
            let buttons =
                (t.entry).map_or(Vec::new(), |e| entry_buttons(feature, &panel.sources, e));
            if panel.expanded {
                h.extend(f.says("Up / Down", "repository", |_| true));
                h.extend(f.says("Left / Right", "button", |k| {
                    matches!(k, TabKey::PickButton(..)) && !buttons.is_empty()
                }));
                let picked = t.entry.and_then(|e| panel.picked(feature, e));
                let enter = match picked.and_then(|at| buttons.get(at)) {
                    Some((label, _)) => format!("{} button", label.to_lowercase()),
                    None => row_hint(feature, t.row, ctx).to_owned(),
                };
                h.extend(f.says("Enter", &enter, |k| *k != TabKey::Swallow));
                h.extend(f.says("Esc", "hide them", is(TabKey::FoldRepos)));
            } else {
                h.extend(f.says("Left / Right", "repository", |_| true));
                h.extend(f.says("Enter", "show all", is(TabKey::FoldRepos)));
            }
            h.extend(f.tab());
            let opens = row_page(feature, t.row).is_some();
            h.extend(f.says("Ctrl+Shift+O", "open PR", |_| opens));
            let removable = match t.entry {
                Some(RepoEntry::Held(i)) => feature.many_repos() && repo_removable(feature, i),
                Some(RepoEntry::Context(_)) => linkable(feature),
                _ => false,
            };
            let del = match panel.armed.is_some() {
                true => "again to remove",
                false => "remove repository",
            };
            h.extend(f.says("Del", del, |k| {
                removable && matches!(k, TabKey::DeleteRow(_))
            }));
        }
        TabArea::Findings if !t.ships.is_empty() => {
            h.extend(f.says("Up / Down", "row", |_| true));
            // A route row goes to its next way, named as its button is.
            let space = match t.ships.get(t.finding) {
                Some(ShipRow::Route { route, .. }) => {
                    let next = (Route::ALL.iter()).position(|r| *r == route.next());
                    next.map_or("tick".to_owned(), |i| chrome::SHIP_ROUTES[i].to_lowercase())
                }
                _ => "tick".to_owned(),
            };
            h.extend(f.says("Space", &space, |k| matches!(k, TabKey::Out(_))));
            h.extend(f.key("Enter", |k| match k {
                TabKey::Out(O::NotYet(_)) => String::new(),
                TabKey::Out(o) => action_hint(o).to_owned(),
                _ => String::new(),
            }));
            h.extend(f.tab());
        }
        TabArea::Findings if t.marking > 0 => {
            let comment = t.comment_at().is_some();
            h.extend(f.says("Up / Down", "criterion", |_| true));
            h.extend(f.says(
                "Left / Right",
                "fold / open",
                outs(|o| matches!(o, O::FoldCriterion(..))),
            ));
            let (space, fail) = match comment {
                true => ("done", "not done"),
                false => ("pass", "fail\u{2026}"),
            };
            h.extend(f.says("Space", space, |k| matches!(k, TabKey::Out(_))));
            h.extend(f.says("f", fail, |k| matches!(k, TabKey::Out(_))));
            h.extend(f.says(
                "Ctrl+E",
                "edit\u{2026}",
                outs(|o| matches!(o, O::Manual(_))),
            ));
            h.extend(f.key("Del", |k| match k {
                TabKey::Out(O::Manual(crate::manual_test::Outcome::Clear(_))) => "clear".to_owned(),
                TabKey::Out(O::Manual(_)) | TabKey::Arm(_) => f.delete("delete"),
                _ => String::new(),
            }));
            h.extend(f.says("c", "add comment\u{2026}", is(TabKey::Out(O::AddComment))));
            h.extend(crate::manual_test::search_hints(panel.search.as_ref()));
            h.extend(f.manual_gate());
            h.extend(f.reopen(None));
            h.extend(f.tab());
        }
        TabArea::Findings if t.shown > 0 => {
            h.extend(f.says("Up / Down", "criterion", |_| true));
            h.extend(f.says(
                "Left / Right",
                "fold / open",
                outs(|o| matches!(o, O::FoldCriterion(..))),
            ));
            h.extend(crate::manual_test::search_hints(panel.search.as_ref()));
            h.extend(f.reopen(Some("reopen")));
            h.extend(f.tab());
        }
        TabArea::Findings if !t.checks.is_empty() => {
            let entry = &t.checks[t.finding.min(t.checks.len() - 1)];
            let row = entry.row.and_then(|i| check_rows(feature, stage).get(i));
            h.extend(f.says("Up / Down", "check", |_| true));
            let pane = row.is_some_and(|row| row.pane.is_some());
            h.extend(f.says("Enter", "show terminal", |k| {
                pane && matches!(k, TabKey::Out(_))
            }));
            let running = checks_running(feature);
            h.extend(f.says("s", "stop", |k| running && matches!(k, TabKey::Out(_))));
            let again = !checks_going(feature, ctx) && feature.current() == Some(stage);
            h.extend(f.says("r", "run again", |k| again && matches!(k, TabKey::Out(_))));
            let skip =
                skip_action(feature, stage, entry, ctx).map(|(label, _)| label.to_lowercase());
            h.extend(f.key("Space", |k| match k {
                TabKey::Out(_) => skip.unwrap_or_default(),
                _ => String::new(),
            }));
            let idle = !checks_going(feature, ctx);
            h.extend(f.says("Shift+R", "run failed", |k| {
                idle && *k == TabKey::Out(O::RunFailed)
            }));
            h.extend(f.says("Shift+S", "stop all", is(TabKey::Out(O::StopChecks))));
            h.extend(f.sends());
            h.extend(f.tab());
        }
        TabArea::Findings => {
            let (on, item, note) = (t.on(), t.item(), t.note());
            let w = words(item.map_or(stage_items(stage), Item::of));
            // Named by the card with the keyboard: Build's and Code review's
            // are comments.
            let noun = match on {
                Some(ReviewCard::DiffNote(_)) => "comment",
                Some(ReviewCard::Item(_)) => w.item,
                None => "card",
            };
            h.extend(f.says("Up / Down", noun, |_| true));
            h.extend(f.says(
                "Left / Right",
                "fold / open",
                outs(|o| matches!(o, O::FoldCard(..))),
            ));
            if note.is_some() {
                h.extend(f.says(
                    "c",
                    "edit\u{2026}",
                    outs(|o| matches!(o, O::EditDiffNote(_))),
                ));
                h.extend(f.key("Del", |k| match k {
                    TabKey::Arm(_) | TabKey::Out(O::DeleteDiffNote(_)) => f.delete("delete"),
                    _ => String::new(),
                }));
                h.extend(f.key("Enter", |k| match k {
                    TabKey::Out(O::ShowDiffNote(_)) => "show in diff".to_owned(),
                    TabKey::Out(O::EditDiffNote(_)) => "edit\u{2026}".to_owned(),
                    _ => String::new(),
                }));
            }
            let decides = can_decide(feature, stage);
            if let Some(i) = item {
                let page = CardPage {
                    decides,
                    posts: feature.posts_review(),
                };
                h.extend(item_hints(i, page, panel.armed.is_some()));
            }
            // Off a reviewer's card, `c` adds a comment of the user's own.
            let own = words(stage_items(stage)).add_own.to_lowercase();
            h.extend(f.says("c", &own, |k| {
                decides && note.is_none() && *k == TabKey::Out(O::AddComment)
            }));
            h.extend(f.says("Shift+F", &w.want_all.to_lowercase(), |k| {
                matches!(k, TabKey::Out(_))
            }));
            h.extend(f.sends());
            h.extend(f.plan_review());
            h.extend(f.said_key("Ctrl+Enter", |o| matches!(o, O::ApproveDoc(_))));
            h.extend(f.fold());
            h.extend(f.tab());
        }
        TabArea::Stages => {
            h.extend(f.says("Left / Right", "stage", |_| true));
            h.extend(f.key("Enter", |k| match k {
                TabKey::Out(O::NotYet(_)) => String::new(),
                // It only says they run.
                TabKey::Out(O::RunChecks) if going => String::new(),
                TabKey::Out(o) => f.said(o),
                _ => String::new(),
            }));
            let all = words(stage_items(stage)).want_all.to_lowercase();
            h.extend(f.says("Shift+F", &all, |k| matches!(k, TabKey::Out(_))));
            h.extend(f.sends());
            h.extend(f.manual_gate());
            h.extend(f.plan_review());
            h.extend(f.said_key("Ctrl+Shift+K", |o| *o == O::SkipPlanReview));
            h.extend(f.said_key("Ctrl+Shift+D", |o| *o == O::ChangeDecisions));
            h.extend(f.fold());
            h.extend(f.reopen(None));
            if let Some(round) = fix_round_on(feature, stage) {
                let make = match (round.phase == FixPhase::Ready, round.fixes_stopped()) {
                    _ if going => String::new(),
                    (true, _) => f.said(&O::MakeFixes),
                    (_, true) => "make fixes again".to_owned(),
                    _ => String::new(),
                };
                h.extend(f.says("Ctrl+Enter", &make, is(TabKey::Out(O::MakeFixes))));
                h.extend(f.said_key(START_OVER_KEY, |o| *o == O::DropFixPlan));
            }
            h.extend(f.said_key("Ctrl+Enter", |o| matches!(o, O::ApproveDoc(_))));
            if can_decide(feature, stage) {
                let own = words(stage_items(stage)).add_own.to_lowercase();
                h.extend(f.says("c", &own, is(TabKey::Out(O::AddComment))));
            }
            let current = feature.current() == Some(stage);
            h.extend(f.key("Shift+R", |k| match k {
                TabKey::Out(O::RunFailed) if current && !going => "run failed".to_owned(),
                TabKey::Out(O::RunChecks) if !going => "run here".to_owned(),
                _ => String::new(),
            }));
            h.extend(f.says("Shift+S", "stop all", is(TabKey::Out(O::StopChecks))));
            h.extend(f.tab());
            let page = row_page(feature, t.row);
            h.extend(f.key("Ctrl+Shift+O", |k| match k {
                TabKey::Out(o) if page.as_ref() == Some(o) => f.said(o),
                TabKey::Out(O::ReviewFixPlan) => "fix plan".to_owned(),
                TabKey::Out(O::OpenPr(_)) => "pull request".to_owned(),
                _ => String::new(),
            }));
            let fix_here = match feature.pull().is_some_and(|p| p.fix_here) {
                true => "comment instead",
                false => "fix here\u{2026}",
            };
            let open = feature.status == Status::Open;
            h.extend(f.says("Alt+W", fix_here, |k| open && *k == TabKey::Out(O::FixHere)));
            h.extend(f.says(
                "Alt+N",
                "pull requests\u{2026}",
                is(TabKey::Out(O::EditPulls)),
            ));
            let commands = |o: &O| matches!(o, O::EditCommands(_));
            if offers_setup(feature, stage) {
                let set_up = can_set_up(feature, ctx);
                h.extend(f.said_key("Ctrl+Shift+U", |o| set_up && *o == O::SetUp));
                h.extend(f.said_key("Ctrl+Shift+I", |o| *o == O::SetupLog));
                h.extend(f.said_key("Ctrl+E", commands));
            }
            if offers_sandbox(feature, stage) {
                let busy = ctx.sandbox_busy;
                // What Ctrl+Shift+T does, as its handler has it.
                let sandbox = |k: &TabKey| {
                    match k {
                        TabKey::Out(O::ShowSandbox) => "show terminals",
                        TabKey::Out(O::StopSandbox) if busy => "stop",
                        TabKey::Out(O::StopSandbox) => "stop\u{2026}",
                        TabKey::Out(O::StartSandbox) => "start sandbox\u{2026}",
                        _ => "",
                    }
                    .to_owned()
                };
                let elsewhere = sandbox_elsewhere(feature);
                if busy {
                    h.extend(f.key("Ctrl+Shift+T", sandbox));
                } else if sandbox_alive(feature) {
                    h.extend(f.says("Ctrl+Shift+Y", "open app", is(TabKey::Out(O::OpenSandbox))));
                    h.extend(f.says("Alt+Y", "next page", is(TabKey::Out(O::NextSandboxPage))));
                    if elsewhere {
                        h.extend(f.key("Ctrl+Shift+T", sandbox));
                    } else {
                        let lan = match ctx.lan_open {
                            true => "close to network",
                            false => "open to network\u{2026}",
                        };
                        h.extend(f.says("Ctrl+Shift+N", lan, is(TabKey::Out(O::SandboxLan))));
                    }
                    h.extend(f.says(
                        "Ctrl+Shift+G",
                        "restart",
                        is(TabKey::Out(O::RestartSandbox)),
                    ));
                    h.extend(f.says(
                        "Ctrl+Shift+X",
                        "reset data\u{2026}",
                        is(TabKey::Out(O::ResetSandbox)),
                    ));
                    match elsewhere {
                        true => h.extend(f.says(
                            "Ctrl+Shift+Q",
                            "stop\u{2026}",
                            is(TabKey::Out(O::StopSandbox)),
                        )),
                        false => h.extend(f.key("Ctrl+Shift+T", sandbox)),
                    }
                } else {
                    h.extend(f.key("Ctrl+Shift+T", sandbox));
                    h.extend(f.says(
                        "Ctrl+Shift+X",
                        "remove sandbox\u{2026}",
                        is(TabKey::Out(O::RemoveSandbox)),
                    ));
                }
                if !offers_setup(feature, stage) {
                    h.extend(f.said_key("Ctrl+E", commands));
                }
            }
            h.extend(f.said_key("Ctrl+R", |o| matches!(o, O::StartAgent(..))));
            h.extend(f.said_key(START_OVER_KEY, |o| matches!(o, O::StartOver(_))));
            let agent = agent_key(feature, stage, ctx);
            let enter = page_enter(feature, stage, ctx);
            h.extend(f.key("Ctrl+Shift+R", |k| match (k, agent) {
                (TabKey::Out(o), Some((label, of))) if *o == of && enter.as_ref() != Some(o) => {
                    label.to_lowercase()
                }
                _ => String::new(),
            }));
            // The keys pick for the card they act on: the fixes' on a page
            // that sends them, while the page's own can't or the findings
            // have the keyboard.
            let fixes = stage != Stage::Build
                && keyed_stage(feature, stage, t.findings()) == Some(Stage::Build);
            let (picks, model, agent_word, model_word) = match fixes {
                true => (
                    choices.fix_picks,
                    choices.fix_model,
                    "pick fixes' agent",
                    "pick fixes' model",
                ),
                false => (choices.picks, choices.model, "pick agent", "pick model"),
            };
            let picks = picks.iter().filter(|c| !c.off).count() > 1;
            h.extend(f.says("Ctrl+Shift+A", agent_word, |k| {
                picks && matches!(k, TabKey::Out(_))
            }));
            h.extend(f.says(MODEL_KEY, model_word, |k| {
                model && matches!(k, TabKey::Out(_))
            }));
            h.extend(f.says("Ctrl+Shift+W", "pick machine", |k| {
                choices.place && matches!(k, TabKey::Out(_))
            }));
            h.extend(f.says(
                "Ctrl+Shift+Q",
                "stop agent",
                outs(|o| matches!(o, O::StopOnMachine(_))),
            ));
            let milestones = match feature.fresh_milestones {
                true => "one agent",
                false => "agent per milestone",
            };
            h.extend(f.says(
                "Ctrl+Shift+B",
                milestones,
                outs(|o| matches!(o, O::FreshMilestones(_))),
            ));
        }
    }
    // What the header and the strip's buttons do (Open tab, Copy path, New
    // feature..., the other features, all repositories, Abandon) is in their
    // tooltips and F1: the footer keeps to the page and what has the keys.
    // A key the footer gives to a gate (Ctrl+Enter, Ctrl+Shift+Enter) isn't
    // given again on Enter.
    let gates: Vec<String> = (hints.iter())
        .filter(|(k, _)| k == "Ctrl+Enter" || k == "Ctrl+Shift+Enter")
        .map(|(_, w)| w.clone())
        .collect();
    hints.retain(|(k, w)| k != "Enter" || !gates.contains(w));
    // F10 is `Gerdr::feature_panel_key`'s, before the panel's keys.
    let pre = PreCtx {
        feature: true,
        inline: false,
        keys: false,
    };
    let menu = key_table::hints(PRE_KEYS, &pre, |k| match k {
        PreKey::Menu => "more actions".to_owned(),
        _ => String::new(),
    });
    let menu = menu
        .into_iter()
        .find(|(k, w)| *k == MENU_KEY && !w.is_empty());
    hints.extend(menu.map(|(k, w)| (k.to_owned(), w)));
    hints.extend(f.says("F1", "keys", is(TabKey::ShowKeys)));
    hints
}

/// The feature tab as `Gerdr::feature_panel_key` sees it, before the
/// panel's keys: whether a feature is on it, its rail is over Code review's
/// review, and the keys list shows.
pub(super) struct PreCtx {
    pub(super) feature: bool,
    pub(super) inline: bool,
    pub(super) keys: bool,
}

/// What a key `feature_panel_key` takes first does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PreKey {
    /// Open the `⋯` menu, its first row picked.
    Menu,
    /// Back from the rail to the review under it.
    Rail,
    /// Glide the page a page up (`true`) or down.
    Page(bool),
    /// Scroll the keys list a notch up (`true`) or down.
    Notch(bool),
}

/// The keys `Gerdr::feature_panel_key` takes before the panel's.
pub(super) const PRE_KEYS: &[Binding<PreCtx, PreKey>] = &[
    // F10 (Shift+F10, the Menu key) opens the `⋯` menu: the arrows, Enter
    // and Esc go on in it.
    row!(
        MENU_KEY,
        "open the \u{22ef} menu: Start over, Set up again, Commands, Abandon and more",
        |c| c.feature.then_some(PreKey::Menu)
    ),
    row!(
        "Shift+F10",
        "open the \u{22ef} menu: Start over, Set up again, Commands, Abandon and more",
        |c| c.feature.then_some(PreKey::Menu)
    ),
    row!(
        "Menu",
        "open the \u{22ef} menu: Start over, Set up again, Commands, Abandon and more",
        |c| c.feature.then_some(PreKey::Menu)
    ),
    alias!("Shift+Menu", |c| c.feature.then_some(PreKey::Menu)),
    // On the rail over the review, Tab and Down go back to it: the page has
    // nothing else to go to.
    row!("Tab", "on Code review's rail: back to its review", |c| c
        .inline
        .then_some(PreKey::Rail)),
    row!(
        "Shift+Tab",
        "on Code review's rail: back to its review",
        |c| c.inline.then_some(PreKey::Rail)
    ),
    row!("Down", "on Code review's rail: back to its review", |c| c
        .inline
        .then_some(PreKey::Rail)),
    row!("Page_Up", "scroll the page", |_| Some(PreKey::Page(true))),
    row!("Page_Down", "scroll the page", |_| Some(PreKey::Page(
        false
    ))),
    // Up and Down scroll the keys list a notch.
    row!("Up", "on this list: scroll it", |c| c
        .keys
        .then_some(PreKey::Notch(true))),
    row!("Down", "on this list: scroll it", |c| c
        .keys
        .then_some(PreKey::Notch(false))),
];

/// What a key `feature_panel_key` takes first does, before the panel's.
pub(super) fn pre_key(c: &PreCtx, mods: &ModifiersState, sym: Keysym) -> Option<PreKey> {
    let (mods, sym) = plain_key(mods, sym);
    key_table::press(PRE_KEYS, c, &mods, sym)
}

/// The page listing every key of the tab (F1), in place of a stage's: the
/// rows of every table of its keys, by section, keys doing the same in the
/// same words as one row.
pub(super) fn keys_page() -> StagePage {
    let mut rows: [Vec<(&str, String)>; 7] = Default::default();
    let mut add = |section: usize, table: Vec<(&'static str, &'static str)>| {
        rows[section].extend(table.into_iter().map(|(k, w)| (k, w.to_owned())));
    };
    for link in chain() {
        match link {
            Link::Rows(section, table) => add(*section, key_table::f1_rows(table)),
            Link::Cards => {
                let cards = key_table::f1_rows(crate::review_cards::CARD_KEYS);
                add(FINDINGS, cards.clone());
                add(SUGGESTIONS, cards);
            }
        }
    }
    add(STAGES, key_table::f1_rows(SHIP_KEYS));
    add(STAGES, key_table::f1_rows(SUBMIT_KEYS));
    add(
        CRITERIA,
        key_table::f1_rows(crate::manual_test::SEARCH_KEYS),
    );
    add(CRITERIA, key_table::f1_rows(crate::page_text::PICK_KEYS));
    add(ANYWHERE, key_table::f1_rows(PRE_KEYS));
    let notes = key_table::f1_rows(crate::manual_test::NOTE_KEYS);
    rows[ANYWHERE].extend(
        notes
            .into_iter()
            .map(|(k, w)| (k, format!("writing a note on a card: {w}"))),
    );
    // Not a key of the tab's: the prefix's (`keys::ACTIONS`).
    rows[ANYWHERE].push((
        "Prefix, then U",
        "show the feature's AI work stats".to_owned(),
    ));
    let sections = (SECTIONS.iter().zip(rows))
        .map(|(label, rows)| {
            // One row per words, its keys together: "Up / Down".
            let mut merged: Vec<(String, String)> = Vec::new();
            for (key, what) in rows {
                let key = key.replace("Page_Up", "PgUp").replace("Page_Down", "PgDn");
                match merged.iter_mut().find(|(_, w)| *w == what) {
                    Some((keys, _)) if keys.split(" / ").any(|k| k == key) => {}
                    Some((keys, _)) => {
                        keys.push_str(" / ");
                        keys.push_str(&key);
                    }
                    None => merged.push((key, what)),
                }
            }
            // A key column: the page lays "key\twhat" out that way.
            let lines_of = (merged.into_iter())
                .map(|(k, what)| line(format!("{k}\t{what}"), CheckTone::Note))
                .collect();
            lines(label, lines_of)
        })
        .collect();
    StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Keys".to_owned(),
        about: "Every key of the feature tab. Esc goes back to the page.".to_owned(),
        status: None,
        buttons: Vec::new(),
        sections,
    }
}
