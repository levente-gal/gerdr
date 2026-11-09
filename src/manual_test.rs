//! Manual test: a feature's acceptance criteria (from its brief), a row
//! each of a sheet of text on the stage's page in the feature tab, to try by
//! hand and mark passed or failed, a failure with a note saying what went
//! wrong, written on its row. Each row folds to its line or opens to its
//! steps; they start folded, one folds as it passes, and what is open and
//! read is kept in `state.json` (`manual::Folds`). A search over them hides
//! those not saying what is typed and opens the rest; `n` and `N` go
//! through the places it found, as the page lays them out (`next_match`),
//! and `*` / `#` search for text selected on the page (`page_text`). The
//! results save as they are given (`Feature::manual`, see
//! `feature::manual`) and stay as the code changes. Plan fixes starts a fix
//! round on the failures found so far (see `feature_fix`): the user goes on
//! trying the rest while the Build agent fixes them, the feature stays on
//! Manual test, and Verify runs again on the fixes (`Feature::carry`). A
//! failure whose fix is made says so, to try again. Mark passed marks Manual
//! test done once every criterion passed and Verify holds on the code as it
//! is now. Once done the cards stay on the page, to read: reopening the stage
//! is what changes them.
//!
//! The cards' keys are in `features`, with the rest of the tab's. This holds
//! what they do, and the note being written.

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    Gerdr,
    chrome::{TabArea, TabColumn},
    feature::{
        self, Feature, Stage, Status, artifacts,
        manual::{self, Criterion, Verdict},
        store,
    },
    features::{CardNote, plural},
    text_field::{Edit, TextField},
};

/// What Enter says with no note written.
const NO_NOTE: &str = "Not failed: say what went wrong first";

/// What the Manual test page's criteria ask gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Mark criterion `i` passed, or clear it when it is.
    Toggle(usize),
    /// Start writing what went wrong with criterion `i`.
    Write(usize),
    /// Mark criterion `i` failed, with this note.
    Fail(usize, String),
    /// Clear criterion `i`'s result.
    Clear(usize),
    /// Mark Manual test done.
    MarkPassed,
    /// Send the failures to the Build agent.
    SendFailures,
}

impl Outcome {
    /// The criterion it is about, if one.
    pub(crate) fn criterion(&self) -> Option<usize> {
        match self {
            Self::Toggle(i) | Self::Write(i) | Self::Fail(i, _) | Self::Clear(i) => Some(*i),
            Self::MarkPassed | Self::SendFailures => None,
        }
    }
}

/// A criterion as its card shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// `AC-1`…
    pub id: String,
    /// What it checks, in a few words.
    pub title: String,
    /// Its Given/When/Then.
    pub text: String,
    /// What the user found, on the code as it is now.
    pub found: Option<Verdict>,
    /// What went wrong, for a failure.
    pub note: String,
    /// The failure went to the Build agent.
    pub sent: bool,
    /// The Build agent made the fix of the failure: to try again.
    pub made: bool,
    /// It was tried on other code than the feature's now.
    pub earlier: bool,
    /// Untested as the fix round of this number changed it after it passed
    /// (see `ManualTest::retest`).
    pub retest: Option<u32>,
    /// It shows its steps, as the user left it (see `manual::Folds`).
    pub open: bool,
    /// The user read it: opened it, or marked it.
    pub seen: bool,
    /// The screenshots of a failure, by file name (see `manual::SHOTS_DIR`).
    pub shots: Vec<String>,
}

impl Row {
    /// Whether it says `query` anywhere the page shows: its id, title,
    /// steps or note, as `crate::find` finds it (any case for one in lower
    /// case; a `whole` word only as a word of its own).
    pub fn says(&self, query: &str, whole: bool) -> bool {
        [&self.id, &self.title, &self.text, &self.note]
            .iter()
            .any(|t| !crate::find::find(t, query, whole).is_empty())
    }
}

/// The search through the criteria on Manual test's page: a field over
/// them, the criteria not saying what it holds hidden. It stays with the
/// panel while other pages show.
#[derive(Debug, Default)]
pub struct Search {
    pub query: TextField,
    /// The field has the keys, not the criteria.
    pub typing: bool,
    /// It finds the query only as a word of its own: `*` and `#` searched
    /// for the word under the pointer. Typing ends it.
    pub whole: bool,
    /// The match gone to, by its index down the page
    /// (`chrome::FeatureTabLayout::matches`); `None` before any.
    pub at: Option<usize>,
}

impl Search {
    /// What it looks for, as typed (see `crate::find`); empty for nothing.
    pub fn needle(&self) -> String {
        self.query.trim().to_owned()
    }
}

/// Which of `rows` show while searching for `needle` (see `Row::says`):
/// all with nothing typed.
pub fn shown(rows: &[Row], needle: &str, whole: bool) -> Vec<bool> {
    rows.iter()
        .map(|r| needle.is_empty() || r.says(needle, whole))
        .collect()
}

/// The match to go to of `matches` (each with the criterion it is in),
/// from `at` with `selected` the criterion selected: the next (`back`:
/// the one before), round the ends; with the selection moved off `at`'s
/// criterion, the first in or after it (`back`: the last in or before
/// it), as vim's `n` goes from the cursor. `None` with none.
pub fn next_match(
    matches: &[Option<usize>],
    at: Option<usize>,
    selected: usize,
    back: bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let on = at.filter(|a| *a < n && matches[*a] == Some(selected));
    if let Some(a) = on {
        return Some(if back { (a + n - 1) % n } else { (a + 1) % n });
    }
    let card = |i: usize| matches[i].unwrap_or(0);
    match back {
        false => (0..n).find(|&i| card(i) >= selected).or(Some(0)),
        true => (0..n).rev().find(|&i| card(i) <= selected).or(Some(n - 1)),
    }
}

/// The criterion to select of those `shown` instead of hidden `at`: the
/// next one shown on from it (`back`: before it), else the nearest the
/// other way; `None` with none shown.
pub fn settle(at: usize, back: bool, shown: &[bool]) -> Option<usize> {
    if shown.get(at).copied().unwrap_or(false) {
        return Some(at);
    }
    let on = (at..shown.len()).find(|i| shown[*i]);
    let before = (0..at.min(shown.len())).rev().find(|i| shown[*i]);
    match back {
        true => before.or(on),
        false => on.or(before),
    }
}

/// The key `state.json` keeps feature `slug`'s folds under.
fn folds_key(project: &str, slug: &str) -> String {
    format!("{project}/{slug}")
}

/// The cards' rows: `criteria` as `test` has them, the feature's code being
/// `patch`'s (`None`: unknown, or not to tell).
fn rows(
    test: Option<&manual::ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
    folds: Option<&manual::Folds>,
) -> Vec<Row> {
    criteria
        .iter()
        .map(|c| {
            let check = manual::check(test, c);
            Row {
                id: c.id.clone(),
                title: c.title.clone(),
                text: c.text.clone(),
                found: check.map(|k| k.state),
                note: check
                    .filter(|k| k.state == Verdict::Fail)
                    .map_or_else(String::new, |k| k.note.clone()),
                sent: check.is_some_and(|k| k.state == Verdict::Fail && k.sent),
                made: check.is_some_and(manual::Check::fix_made),
                earlier: (test.zip(check)).zip(patch).is_some_and(|((t, k), p)| {
                    manual::tried_on(t, k) != p
                }),
                retest: (test.filter(|_| check.is_none()))
                    .and_then(|t| t.retest.get(&c.id).copied()),
                open: folds.is_some_and(|f| f.open.contains(&c.text)),
                seen: folds.is_some_and(|f| f.seen.contains(&c.text)) || check.is_some(),
                shots: check
                    .filter(|k| k.state == Verdict::Fail)
                    .map_or_else(Vec::new, |k| k.shots.clone()),
            }
        })
        .collect()
}

/// How far the manual test of `criteria` went, in words: "2 of 3
/// criteria passed · 1 failed", "1 of 3 criteria passed · 1 failure
/// sent to Build", "… · 1 fix made: try again".
pub(crate) fn counts_text(n: manual::Counts) -> String {
    let total = n.total();
    let what = if total == 1 { "criterion" } else { "criteria" };
    let passed = format!("{} of {total} {what} passed", n.passed);
    if n.made > 0 {
        format!(
            "{passed} \u{b7} {} made: try again",
            fixes(n.made)
        )
    } else if n.failed > 0 && n.sent == n.failed {
        format!(
            "{passed} \u{b7} {} sent to Build",
            plural(n.sent, "failure")
        )
    } else if n.failed > 0 {
        format!("{passed} \u{b7} {} failed", n.failed)
    } else {
        passed
    }
}

/// "1 fix", "2 fixes".
fn fixes(n: usize) -> String {
    match n {
        1 => "1 fix".into(),
        n => format!("{n} fixes"),
    }
}

/// The counts in a few words, for the bar over the cards: "3 passed · 1
/// failed · 2 untested", and how many fixes wait to be tried.
pub(crate) fn strip_text(n: manual::Counts) -> String {
    let mut text = format!(
        "{} passed \u{b7} {} failed \u{b7} {} untested",
        n.passed, n.failed, n.untested
    );
    if n.made > 0 {
        text.push_str(&format!(" \u{b7} {} made", fixes(n.made)));
    }
    text
}

/// What is left before every criterion passed: "AC-2 failed, AC-3 not
/// tried".
fn left_text(rows: &[Row]) -> String {
    rows.iter()
        .filter_map(|r| match r.found {
            Some(Verdict::Pass) => None,
            Some(Verdict::Fail) => Some(format!("{} failed", r.id)),
            None => Some(format!("{} not tried", r.id)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The acceptance criteria in the brief of feature `slug` of `project`,
/// read from disk: none when it has no brief, or none in it.
pub(crate) fn read_criteria(project: &str, slug: &str) -> Vec<Criterion> {
    criteria_in(&store::dir(None, project, slug))
}

/// The acceptance criteria in the brief in feature data folder `data`.
pub(crate) fn criteria_in(data: &std::path::Path) -> Vec<Criterion> {
    let Some(path) = artifacts::path(data, Stage::Discover) else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => manual::parse_criteria(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            Vec::new()
        }
    }
}

impl Gerdr {
    /// Read the criteria of feature `slug` of `project` from its brief
    /// again: the page and the inbox show them.
    pub(crate) fn load_criteria(&mut self, project: &str, slug: &str) {
        let criteria = read_criteria(project, slug);
        let key = (project.to_owned(), slug.to_owned());
        if criteria.is_empty() {
            self.features.criteria.remove(&key);
        } else {
            self.features.criteria.insert(key, criteria);
        }
    }

    /// The criteria of feature `slug` of `project`, as last read.
    pub(crate) fn criteria(&self, project: &str, slug: &str) -> &[Criterion] {
        self.features
            .criteria
            .get(&(project.to_owned(), slug.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// The patch-id of feature `slug`'s code as last read, if it was.
    fn manual_patch(&self, project: &str, slug: &str) -> Option<String> {
        self.feature_reading(project, slug).map(|r| r.patch.clone())
    }

    /// How far feature `f`'s manual test went; `None` when its brief has no
    /// criteria.
    pub(crate) fn manual_counts(&self, f: &Feature) -> Option<manual::Counts> {
        let criteria = self.criteria(&f.project, &f.slug);
        if criteria.is_empty() {
            return None;
        }
        Some(manual::counts(f.manual.as_ref(), criteria))
    }

    /// The cards of feature `f`'s criteria, telling the ones tried on other
    /// code than its own as last read; once Manual test is done, as recorded:
    /// its page says if the code changed.
    pub(crate) fn criterion_rows(&self, f: &Feature) -> Vec<Row> {
        let patch = match f.is_done(Stage::ManualTest) {
            true => None,
            false => self.manual_patch(&f.project, &f.slug),
        };
        rows(
            f.manual.as_ref(),
            self.criteria(&f.project, &f.slug),
            patch.as_deref(),
            self.projects.criteria_folds.get(&folds_key(&f.project, &f.slug)),
        )
    }

    /// Change the folds of feature `slug`'s criteria by `change`, which
    /// says whether it changed them, and save that.
    fn change_folds(
        &mut self,
        project: &str,
        slug: &str,
        change: impl FnOnce(&mut manual::Folds, &[Criterion]) -> bool,
    ) {
        let criteria = self.criteria(project, slug).to_vec();
        if criteria.is_empty() {
            return;
        }
        let key = folds_key(project, slug);
        let folds = self.projects.criteria_folds.entry(key.clone()).or_default();
        // What the brief no longer says goes with the change.
        let pruned = folds.keep_only(&criteria);
        let changed = change(folds, &criteria);
        if folds.is_empty() {
            self.projects.criteria_folds.remove(&key);
        }
        if changed || pruned {
            self.projects.save();
        }
    }

    /// Open criterion `i` of the feature on the tab to show its steps, or
    /// fold it to its line.
    pub(crate) fn fold_criterion(&mut self, i: usize, open: bool) {
        let Some((project, slug)) = self.tab_feature_names() else {
            return;
        };
        self.change_folds(&project, &slug, |folds, criteria| {
            criteria.get(i).is_some_and(|c| folds.set(&c.text, open))
        });
        self.select_criterion(i);
    }

    /// Open or fold the criteria `scope` names of the feature on the tab.
    pub(crate) fn fold_criteria(&mut self, scope: manual::FoldScope, open: bool) {
        let Some((project, slug)) = self.tab_feature_names() else {
            return;
        };
        let rows = match self.feature(&project, &slug) {
            Some(f) => self.criterion_rows(f),
            None => return,
        };
        self.change_folds(&project, &slug, |folds, criteria| {
            let mut changed = false;
            for (c, row) in criteria.iter().zip(&rows) {
                let wanted = match scope {
                    manual::FoldScope::All => true,
                    manual::FoldScope::Done => row.found == Some(Verdict::Pass),
                    manual::FoldScope::Seen => row.seen,
                };
                if wanted && row.open != open {
                    // Opening many reads none of them.
                    changed |= match open {
                        true => folds.open.insert(c.text.clone()),
                        false => folds.open.remove(&c.text),
                    };
                }
            }
            changed
        });
        self.sync_feature_page();
        self.reveal_feature_finding();
    }

    /// Note the selected criterion as read, if it shows its steps.
    pub(crate) fn see_selected_criterion(&mut self) {
        let Some(panel) = self.features.panel.as_ref() else {
            return;
        };
        if panel.focus != TabArea::Findings || panel.keys || panel.expanded {
            return;
        }
        let (project, slug, i) = (panel.project.clone(), panel.slug.clone(), panel.finding);
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        if panel.page_stage(feature) != Stage::ManualTest {
            return;
        }
        let searching = (panel.search.as_ref()).is_some_and(|s| !s.needle().is_empty());
        let rows = self.criterion_rows(feature);
        let Some(row) = rows.get(i).filter(|r| (r.open || searching) && !r.seen) else {
            return;
        };
        let text = row.text.clone();
        self.change_folds(&project, &slug, |folds, _| folds.see(&text));
    }

    /// The project and slug of the feature on the tab.
    fn tab_feature_names(&self) -> Option<(String, String)> {
        let panel = self.features.panel.as_ref()?;
        Some((panel.project.clone(), panel.slug.clone()))
    }

    /// Whether the page with the criteria shows, so their search can.
    pub(crate) fn criteria_search_shows(&self) -> bool {
        let Some(panel) = &self.features.panel else {
            return false;
        };
        let Some(feature) = self.feature(&panel.project, &panel.slug) else {
            return false;
        };
        !panel.keys
            && !panel.expanded
            && crate::features::shows_criteria(feature, panel.page_stage(feature))
            && !self.criteria(&panel.project, &panel.slug).is_empty()
    }

    /// Open the criteria's search, the keys in its field.
    pub(crate) fn open_criteria_search(&mut self) {
        if !self.criteria_search_shows() {
            return;
        }
        if let Some(panel) = self.features.panel.as_mut() {
            panel.search.get_or_insert_with(Search::default).typing = true;
        }
        self.stop_key_repeat();
    }

    /// Close the criteria's search: they all show again.
    pub(crate) fn close_criteria_search(&mut self) {
        if let Some(panel) = self.features.panel.as_mut() {
            panel.search = None;
        }
        self.sync_feature_page();
        self.reveal_feature_finding();
    }

    /// The criterion selected is one the search shows: the nearest, `back`
    /// before it first.
    pub(crate) fn settle_criteria(&mut self, back: bool) {
        let Some(panel) = self.features.panel.as_ref() else {
            return;
        };
        let Some((needle, whole)) = (panel.search.as_ref()).map(|s| (s.needle(), s.whole)) else {
            return;
        };
        let Some(feature) = self.feature(&panel.project, &panel.slug) else {
            return;
        };
        if !crate::features::shows_criteria(feature, panel.page_stage(feature)) {
            return;
        }
        let shown = shown(&self.criterion_rows(feature), &needle, whole);
        if let Some(panel) = self.features.panel.as_mut()
            && let Some(at) = settle(panel.finding, back, &shown)
        {
            panel.finding = at;
        }
    }

    /// A key for the criteria's search; false when it isn't the search's.
    /// `/` and Ctrl+F open it; while its field has the keys, what is typed
    /// goes there, Enter, Tab and Down give the keys to the criteria it
    /// shows, and Esc clears it, then closes it. With the keys on the
    /// criteria, Esc does the same.
    pub(crate) fn criteria_search_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> bool {
        let ctrl = mods.ctrl && !(mods.alt || mods.logo || mods.shift);
        let bare = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let opens = (ctrl && sym == Keysym::f)
            || (!mods.ctrl && !mods.alt && (sym == Keysym::slash || typed == Keysym::slash));
        if !self.criteria_search_shows() {
            // Away from the criteria, the keys are the page's.
            if let Some(search) = (self.features.panel.as_mut()).and_then(|p| p.search.as_mut()) {
                search.typing = false;
            }
            return false;
        }
        let Some(panel) = self.features.panel.as_mut() else {
            return false;
        };
        let writing = panel.note.is_some();
        let Some(search) = panel.search.as_mut() else {
            if opens && !writing {
                self.open_criteria_search();
                return true;
            }
            return false;
        };
        if sym == Keysym::Escape && bare {
            if search.query.is_empty() {
                self.close_criteria_search();
            } else {
                search.query.clear();
                (search.typing, search.whole, search.at) = (true, false, None);
                self.sync_feature_page();
            }
            return true;
        }
        if !search.typing {
            if opens {
                search.typing = true;
                self.stop_key_repeat();
                return true;
            }
            // n and Shift+N go through what it found, as vim's.
            let shift = mods.shift && !(mods.ctrl || mods.alt || mods.logo);
            let next = (bare || shift) && matches!(sym, Keysym::n | Keysym::N);
            if next && !search.needle().is_empty() {
                self.go_to_match(shift, false);
                return true;
            }
            return false;
        }
        let to_list = matches!(
            sym,
            Keysym::Return | Keysym::KP_Enter | Keysym::Down | Keysym::KP_Down | Keysym::Tab
        ) && !(mods.ctrl || mods.alt || mods.logo);
        let edit = match to_list || (ctrl && sym == Keysym::f) {
            true => Edit::Moved,
            false => search.query.key(mods, sym, typed),
        };
        if to_list {
            search.typing = false;
        }
        if edit == Edit::Changed {
            // What is typed is found anywhere, not as a word of its own.
            (search.whole, search.at) = (false, None);
        }
        // Chords not the field's are the tab's: Ctrl+Enter marks passed.
        if edit == Edit::Ignored && (mods.ctrl || mods.alt || mods.logo) {
            return false;
        }
        if to_list || edit == Edit::Changed {
            panel.focus = TabArea::Findings;
            self.settle_criteria(false);
            self.sync_feature_page();
            self.reveal_feature_finding();
            if edit == Edit::Changed {
                self.go_to_match(false, true);
            }
        }
        true
    }

    /// Go to the next place the criteria's search found (`back`: the one
    /// before), as `n` and `N` do, its criterion selected and the page
    /// scrolled to it; `fresh` for what was just typed: the first in or
    /// after the criterion selected.
    pub(crate) fn go_to_match(&mut self, back: bool, fresh: bool) {
        self.sync_feature_page();
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let cards: Vec<Option<usize>> = layout.matches.iter().map(|m| m.card).collect();
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let selected = panel.finding;
        let Some(search) = panel.search.as_mut() else {
            return;
        };
        let from = if fresh { None } else { search.at };
        search.at = next_match(&cards, from, selected, back);
        let Some(at) = search.at else {
            return;
        };
        if let Some(card) = cards[at] {
            panel.finding = card;
            panel.focus = TabArea::Findings;
        }
        self.sync_feature_page();
        self.reveal_page_line(layout.matches[at].line);
    }

    /// Scroll the page to show line `line` of its text, as laid out now.
    pub(crate) fn reveal_page_line(&mut self, line: usize) {
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Page as usize];
        if let Some(to) = layout.reveal_line(line, scroll.pos())
            && to != scroll.pos()
        {
            scroll.jump(to);
        }
    }

    /// Give the keyboard to the criteria of the feature on the tab, at the
    /// first not tried yet, else the first.
    pub(crate) fn focus_criteria(&mut self, project: &str, slug: &str) {
        self.load_criteria(project, slug);
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        if self.criteria(project, slug).is_empty() {
            return self.show_warning(format!(
                "The brief of \u{201c}{title}\u{201d} has no acceptance criteria to try"
            ));
        }
        let first = (self.criterion_rows(feature).iter())
            .position(|r| r.found.is_none())
            .unwrap_or(0);
        self.select_criterion(first);
        self.probe_feature(project, slug);
    }

    /// Card `i` has the keyboard, and shows.
    pub(crate) fn select_criterion(&mut self, i: usize) {
        if let Some(panel) = self.features.panel.as_mut() {
            panel.focus = TabArea::Findings;
            panel.finding = i;
        }
        self.sync_feature_page();
        self.reveal_feature_finding();
    }

    /// Whether the page with the note being written shows, so its field does.
    fn criterion_note_shows(&self) -> bool {
        let Some(panel) = &self.features.panel else {
            return false;
        };
        let Some(feature) = self.feature(&panel.project, &panel.slug) else {
            return false;
        };
        // A criterion's note is written wherever they can be marked.
        let (stage, writes) = match panel.note {
            Some((CardNote::Finding(_), _)) => (
                Stage::AiReview,
                feature.status == Status::Open && feature.current() == Some(Stage::AiReview),
            ),
            _ => (
                Stage::ManualTest,
                crate::features::marks_criteria(feature, Stage::ManualTest),
            ),
        };
        !panel.keys && !panel.expanded && panel.page_stage(feature) == stage && writes
    }

    /// A key while the note of a criterion is being written; false when none
    /// is, or its page has gone (the note is kept then), and the key is the
    /// tab's. Enter saves it, as does Esc; with nothing written, Enter says
    /// why not and Esc leaves the criterion as it was. Shift+Enter breaks
    /// the line.
    pub(crate) fn criterion_note_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> bool {
        if !self.writing_criterion_note() {
            return false;
        }
        if !self.criterion_note_shows() {
            self.keep_criterion_note();
            return false;
        }
        let Some((on, field)) = (self.features.panel.as_mut()).and_then(|p| p.note.as_mut()) else {
            return false;
        };
        let finding = match on {
            CardNote::Finding(id) => Some(id.clone()),
            CardNote::Criterion(_) => None,
        };
        let enter = matches!(sym, Keysym::Return | Keysym::KP_Enter);
        let only_shift = mods.shift && !(mods.ctrl || mods.alt || mods.logo);
        if enter && only_shift {
            field.newline();
            return true;
        }
        match sym {
            Keysym::Return | Keysym::KP_Enter if field.trim().is_empty() => match finding {
                // Emptied, it goes; nothing there, there is nothing to do.
                Some(id) => {
                    let panel = self.features.panel.as_mut();
                    let Some((project, slug)) = panel.map(|p| {
                        p.note = None;
                        (p.project.clone(), p.slug.clone())
                    }) else {
                        return true;
                    };
                    self.delete_finding_comment(&project, &slug, &id);
                }
                None => self.show_warning(NO_NOTE.into()),
            },
            Keysym::Return | Keysym::KP_Enter | Keysym::Escape => {
                self.keep_criterion_note();
            }
            _ => {
                field.key(mods, sym, typed);
            }
        }
        true
    }

    /// Whether the note of a criterion is being written.
    pub(crate) fn writing_criterion_note(&self) -> bool {
        (self.features.panel.as_ref()).is_some_and(|p| p.note.is_some())
    }

    /// Stop writing a note, saving it if one was written: a note written is
    /// never thrown away. False when it could not be saved yet, and goes on
    /// being written.
    pub(crate) fn keep_criterion_note(&mut self) -> bool {
        let Some((on, field)) = (self.features.panel.as_mut()).and_then(|p| p.note.take()) else {
            return true;
        };
        let note = field.trim().to_owned();
        if !note.is_empty() {
            match on {
                CardNote::Criterion(i) => self.apply_manual_test(Outcome::Fail(i, note)),
                CardNote::Finding(id) => {
                    let names =
                        (self.features.panel.as_ref()).map(|p| (p.project.clone(), p.slug.clone()));
                    if let Some((project, slug)) = names {
                        self.note_finding(&project, &slug, &id, &note);
                    }
                }
            }
        }
        !self.writing_criterion_note()
    }

    /// Do what the criteria of the feature on the tab asked.
    pub fn apply_manual_test(&mut self, outcome: Outcome) {
        let Some((project, slug)) =
            (self.features.panel.as_ref()).map(|p| (p.project.clone(), p.slug.clone()))
        else {
            return;
        };
        self.apply_manual_test_on(&project, &slug, outcome);
    }

    /// Whether the feature tab shows feature `slug` of `project`.
    fn panel_on(&self, project: &str, slug: &str) -> bool {
        (self.features.panel.as_ref()).is_some_and(|p| p.project == project && p.slug == slug)
    }

    /// Do what the criteria of feature `slug` of `project` asked, from its
    /// page or from the criteria strip over its sandbox's app.
    pub(crate) fn apply_manual_test_on(&mut self, project: &str, slug: &str, outcome: Outcome) {
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let on_page = self.panel_on(&project, &slug);
        let Some(feature) = self.feature(&project, &slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        let marks = crate::features::marks_criteria(feature, Stage::ManualTest);
        let criteria = self.criteria(&project, &slug).to_vec();
        let reading = self.feature_reading(&project, &slug).cloned();
        let patch = reading.as_ref().map(|r| r.patch.clone());
        let reading_text =
            format!("Still reading the code of \u{201c}{title}\u{201d}; try again in a moment");
        if let Some(i) = outcome.criterion().filter(|_| on_page) {
            self.select_criterion(i);
        }
        match outcome {
            Outcome::Write(_) if !on_page => {}
            Outcome::Write(i) => {
                let Some(c) = criteria.get(i) else {
                    return;
                };
                let note = self.feature(&project, &slug).and_then(|f| {
                    let check = manual::check(f.manual.as_ref(), c)?;
                    Some(check.note.clone())
                });
                self.write_criterion_note(i, note.as_deref().unwrap_or(""));
            }
            Outcome::Toggle(i) | Outcome::Fail(i, _) => {
                let Some(c) = criteria.get(i) else {
                    return;
                };
                // Done, or gone back to Verify with its fixes, it still
                // takes marks: what fails waits for it to send.
                if !marks {
                    let why = match self.feature(&project, &slug).map(|f| f.status) {
                        Some(Status::Open) => "hasn't got to Manual test yet",
                        _ => "isn't open",
                    };
                    return self.show_warning(format!("\u{201c}{title}\u{201d} {why}"));
                }
                let Some(patch) = patch else {
                    // The note goes back in its field, not thrown away.
                    if let Outcome::Fail(_, note) = &outcome
                        && on_page
                    {
                        self.write_criterion_note(i, note);
                    }
                    return self.show_warning(reading_text);
                };
                let Some(f) = self.feature_mut(&project, &slug) else {
                    return;
                };
                let passed = manual::check(f.manual.as_ref(), c)
                    .is_some_and(|k| k.state == Verdict::Pass);
                let text = c.text.clone();
                // Marked, it was read; passed, it folds: what is left to try
                // is what stays open.
                let passes = !passed && matches!(outcome, Outcome::Toggle(_));
                match outcome {
                    Outcome::Fail(_, note) => {
                        manual::set(
                            &mut f.manual,
                            c,
                            &patch,
                            Verdict::Fail,
                            &note,
                            feature::now(),
                        );
                    }
                    _ if passed => _ = manual::clear(&mut f.manual, &c.id),
                    _ => manual::set(&mut f.manual, c, &patch, Verdict::Pass, "", feature::now()),
                }
                self.save_feature(&project, &slug);
                self.change_folds(&project, &slug, |folds, _| {
                    let folded = passes && folds.open.remove(&text);
                    folds.see(&text) || folded
                });
            }
            Outcome::Clear(i) => {
                let Some(c) = criteria.get(i) else {
                    return;
                };
                let Some(f) = self.feature_mut(&project, &slug) else {
                    return;
                };
                if manual::clear(&mut f.manual, &c.id) {
                    self.save_feature(&project, &slug);
                }
            }
            Outcome::MarkPassed => {
                let Some(feature) = self.feature(&project, &slug) else {
                    return;
                };
                if feature.status != Status::Open || feature.current() != Some(Stage::ManualTest) {
                    let why = match feature.current() {
                        Some(stage) if feature.status == Status::Open => {
                            format!("Mark {} done first", stage.label())
                        }
                        _ => format!("\u{201c}{title}\u{201d} isn't open"),
                    };
                    return self.show_warning(why);
                }
                let Some(reading) = reading else {
                    return self.show_warning(reading_text);
                };
                if !reading.dirty.is_empty() {
                    return self.show_warning(format!(
                        "Commit or drop the uncommitted changes in {} first",
                        reading.dirty.join(", ")
                    ));
                }
                let rows = rows(feature.manual.as_ref(), &criteria, Some(&reading.patch), None);
                let left = left_text(&rows);
                if !left.is_empty() {
                    return self.show_warning(format!("Not passed yet: {left}"));
                }
                // Fixes landed while it was tried: the stages after Build go
                // over them again first.
                if feature.unverified.is_some() {
                    return self.back_to_verify(&project, &slug);
                }
                self.read_and_tick(&project, &slug, Stage::ManualTest);
            }
            Outcome::SendFailures => {
                let Some(feature) = self.feature(&project, &slug) else {
                    return;
                };
                let (n, text) = manual::failures_markdown(feature.manual.as_ref(), &criteria, &title);
                let asked = manual::asked(feature.manual.as_ref(), &criteria);
                if n == 0 {
                    return self.show_warning("No failures to send: mark one failed first".into());
                }
                if !self.send_to_build(
                    &project,
                    &slug,
                    Stage::ManualTest,
                    "manual test failures",
                    text,
                ) {
                    return;
                }
                if let Some(f) = self.feature_mut(&project, &slug) {
                    let checks = manual::mark_sent(&mut f.manual, &criteria);
                    let back = crate::feature::fix_round::GiveBack::Manual { checks };
                    self.fix_round_gives_back(&project, &slug, back);
                    self.fix_round_asked(&project, &slug, asked);
                }
                tracing::info!("sent {n} manual test failures of {project}/{slug} to Build");
                self.save_feature(&project, &slug);
                self.show_build_agent(&project, &slug);
            }
        }
    }

    /// Start writing criterion `i`'s note, from `note`, on its card.
    fn write_criterion_note(&mut self, i: usize, note: &str) {
        let field = TextField::multiline(note);
        if let Some(panel) = self.features.panel.as_mut() {
            panel.note = Some((CardNote::Criterion(i), field));
            // The note has the keys, not the search.
            if let Some(search) = panel.search.as_mut() {
                search.typing = false;
            }
        }
        self.stop_key_repeat();
        self.select_criterion(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, found: Option<Verdict>) -> Row {
        Row {
            id: id.into(),
            title: "C".into(),
            text: "Given a, when b, then c".into(),
            found,
            note: String::new(),
            sent: false,
            made: false,
            earlier: false,
            retest: None,
            open: false,
            seen: false,
            shots: Vec::new(),
        }
    }

    #[test]
    fn counts_read_as_words() {
        let n = |passed, failed, sent, untested| manual::Counts {
            passed,
            failed,
            sent,
            made: 0,
            untested,
        };
        assert_eq!(
            counts_text(n(2, 1, 0, 0)),
            "2 of 3 criteria passed \u{b7} 1 failed"
        );
        assert_eq!(counts_text(n(0, 0, 0, 1)), "0 of 1 criterion passed");
        assert_eq!(
            counts_text(n(1, 1, 1, 1)),
            "1 of 3 criteria passed \u{b7} 1 failure sent to Build"
        );
        let made = manual::Counts {
            made: 1,
            ..n(1, 2, 1, 0)
        };
        assert_eq!(
            counts_text(made),
            "1 of 3 criteria passed \u{b7} 1 fix made: try again"
        );
        assert_eq!(
            strip_text(n(3, 1, 0, 2)),
            "3 passed \u{b7} 1 failed \u{b7} 2 untested"
        );
        assert_eq!(
            strip_text(made),
            "1 passed \u{b7} 2 failed \u{b7} 0 untested \u{b7} 1 fix made"
        );
        assert_eq!(
            left_text(&[
                row("AC-1", Some(Verdict::Pass)),
                row("AC-2", Some(Verdict::Fail)),
                row("AC-3", None),
            ]),
            "AC-2 failed, AC-3 not tried"
        );
    }

    #[test]
    fn a_search_finds_any_text_a_criterion_shows_and_steps_over_the_rest() {
        let mut rows = vec![
            row("AC-1", None),
            row("AC-2", Some(Verdict::Fail)),
            row("AC-3", None),
        ];
        rows[1].note = "Save stays ON".into();
        rows[2].title = "Exports".into();
        assert_eq!(shown(&rows, "", false), [true, true, true]);
        assert_eq!(shown(&rows, "save stays", false), [false, true, false]);
        assert_eq!(shown(&rows, "export", false), [false, false, true]);
        assert_eq!(shown(&rows, "ac-1", false), [true, false, false]);
        assert_eq!(shown(&rows, "when b", false), [true, true, true]);
        // One with a capital finds only that case; a whole word only alone.
        assert_eq!(shown(&rows, "Save", false), [false, true, false]);
        assert_eq!(shown(&rows, "ON", false), [false, true, false]);
        assert_eq!(shown(&rows, "Export", true), [false, false, false]);
        assert_eq!(shown(&rows, "Exports", true), [false, false, true]);
        // Hidden, the selection goes on the way it went, else back.
        let some = [true, false, true];
        assert_eq!(settle(1, false, &some), Some(2));
        assert_eq!(settle(1, true, &some), Some(0));
        assert_eq!(settle(0, false, &some), Some(0));
        assert_eq!(settle(2, false, &[true, false, false]), Some(0));
        assert_eq!(settle(1, false, &[false, false, false]), None);
    }

    #[test]
    fn n_goes_on_from_the_match_and_from_the_criterion_the_selection_moved_to() {
        // Two matches in criterion 0, one in 2, one in 3.
        let cards = [Some(0), Some(0), Some(2), Some(3)];
        assert_eq!(next_match(&cards, None, 0, false), Some(0));
        assert_eq!(next_match(&cards, Some(0), 0, false), Some(1));
        assert_eq!(next_match(&cards, Some(1), 0, false), Some(2));
        // Round the ends, both ways.
        assert_eq!(next_match(&cards, Some(3), 3, false), Some(0));
        assert_eq!(next_match(&cards, Some(0), 0, true), Some(3));
        // The selection moved to criterion 1: on from there, or back.
        assert_eq!(next_match(&cards, Some(0), 1, false), Some(2));
        assert_eq!(next_match(&cards, Some(0), 1, true), Some(1));
        // Past the last criterion found, `n` goes round to the first.
        assert_eq!(next_match(&cards, None, 5, false), Some(0));
        assert_eq!(next_match(&[], None, 0, false), None);
    }

    #[test]
    fn only_a_criterions_own_outcomes_name_it() {
        assert_eq!(Outcome::Toggle(1).criterion(), Some(1));
        assert_eq!(Outcome::Write(2).criterion(), Some(2));
        assert_eq!(Outcome::Clear(3).criterion(), Some(3));
        assert_eq!(Outcome::MarkPassed.criterion(), None);
        assert_eq!(Outcome::SendFailures.criterion(), None);
    }
}
