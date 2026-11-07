//! Manual test: a feature's acceptance criteria (from its brief), one card
//! each on the stage's page in the feature tab, to try by hand and mark passed
//! or failed, a failure with a note saying what went wrong, written on its
//! card. The results save as they are given (`Feature::manual`, see
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
    chrome::TabArea,
    feature::{
        self, Feature, Stage, Status, artifacts,
        manual::{self, Criterion, Verdict},
        store,
    },
    features::{CardNote, plural},
    text_field::TextField,
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
}

/// The cards' rows: `criteria` as `test` has them, the feature's code being
/// `patch`'s (`None`: unknown, or not to tell).
fn rows(
    test: Option<&manual::ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
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
    let data = store::dir(None, project, slug);
    let Some(path) = artifacts::path(&data, Stage::Discover) else {
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
        )
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
        let stage = match panel.note {
            Some((CardNote::Finding(_), _)) => Stage::AiReview,
            _ => Stage::ManualTest,
        };
        !panel.keys
            && !panel.expanded
            && panel.page_stage(feature) == stage
            && feature.status == Status::Open
            && feature.current() == Some(stage)
    }

    /// A key while the note of a criterion is being written; false when none
    /// is, or its page has gone (the note is kept then), and the key is the
    /// tab's. Enter saves it, as does Esc; with nothing written, Enter says
    /// why not and Esc leaves the criterion as it was.
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
        let Some(feature) = self.feature(&project, &slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        let done = feature.is_done(Stage::ManualTest);
        let criteria = self.criteria(&project, &slug).to_vec();
        let reading = self.feature_reading(&project, &slug).cloned();
        let patch = reading.as_ref().map(|r| r.patch.clone());
        let reading_text =
            format!("Still reading the code of \u{201c}{title}\u{201d}; try again in a moment");
        if let Some(i) = outcome.criterion() {
            self.select_criterion(i);
        }
        match outcome {
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
                if done {
                    return self.show_warning(format!(
                        "{} is done: reopen it to change what it found",
                        Stage::ManualTest.label()
                    ));
                }
                let Some(patch) = patch else {
                    // The note goes back in its field, not thrown away.
                    if let Outcome::Fail(_, note) = &outcome {
                        self.write_criterion_note(i, note);
                    }
                    return self.show_warning(reading_text);
                };
                let Some(f) = self.feature_mut(&project, &slug) else {
                    return;
                };
                let passed = manual::check(f.manual.as_ref(), c)
                    .is_some_and(|k| k.state == Verdict::Pass);
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
                let rows = rows(feature.manual.as_ref(), &criteria, Some(&reading.patch));
                let left = left_text(&rows);
                if !left.is_empty() {
                    return self.show_warning(format!("Not passed yet: {left}"));
                }
                if let Some(fixed) = feature.unverified.clone()
                    && self.fixes_hold_up(&project, &slug, &fixed)
                {
                    return;
                }
                self.read_and_tick(&project, &slug, Stage::ManualTest);
            }
            Outcome::SendFailures => {
                let Some(feature) = self.feature(&project, &slug) else {
                    return;
                };
                let (n, text) = manual::failures_markdown(feature.manual.as_ref(), &criteria, &title);
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
                }
                tracing::info!("sent {n} manual test failures of {project}/{slug} to Build");
                self.save_feature(&project, &slug);
                self.show_build_agent(&project, &slug);
            }
        }
    }

    /// Whether Manual test of feature `slug` can't be marked passed as
    /// Verify hasn't passed on `fixed`, the code its fixes made. The toast
    /// says why, and checks that never ran on it start.
    fn fixes_hold_up(&mut self, project: &str, slug: &str, fixed: &str) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            return true;
        };
        let title = feature.title.clone();
        let run = (feature.verify.as_ref()).filter(|r| r.patch.as_deref() == Some(fixed));
        if run.is_some_and(|r| r.passed == Some(true)) {
            if let Some(f) = self.feature_mut(project, slug) {
                f.unverified = None;
                self.save_feature(project, slug);
            }
            return false;
        }
        let failed = run.and_then(feature::verify::to_fix);
        if self.checks_under_way(project, slug) {
            self.show_warning(format!(
                "Verify is checking the fixes of \u{201c}{title}\u{201d}: mark it passed once they pass"
            ));
        } else if let Some(failed) = failed {
            let first = failed.first().cloned().unwrap_or_default();
            self.show_warning(format!(
                "Verify failed on the fixes of \u{201c}{title}\u{201d}: {first}; Make Verify fixes sends it"
            ));
        } else {
            self.recheck(project, slug);
            self.show_warning(format!(
                "Verify hadn't run on the fixes of \u{201c}{title}\u{201d}; it runs now: mark it passed once it passes"
            ));
        }
        true
    }

    /// Start writing criterion `i`'s note, from `note`, on its card.
    fn write_criterion_note(&mut self, i: usize, note: &str) {
        let mut field = TextField::default();
        field.set(note);
        if let Some(panel) = self.features.panel.as_mut() {
            panel.note = Some((CardNote::Criterion(i), field));
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
    fn only_a_criterions_own_outcomes_name_it() {
        assert_eq!(Outcome::Toggle(1).criterion(), Some(1));
        assert_eq!(Outcome::Write(2).criterion(), Some(2));
        assert_eq!(Outcome::Clear(3).criterion(), Some(3));
        assert_eq!(Outcome::MarkPassed.criterion(), None);
        assert_eq!(Outcome::SendFailures.criterion(), None);
    }
}
