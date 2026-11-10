//! Show in plan: Plan review's suggestions as flagged comments on the lines
//! of `plan.md` they apply to, in a doc tab of their own (`DocFile::Suggestions`).
//!
//! The plan is approved, so it is only read; the suggestions are decided on
//! it with the keys AI review's findings have in the feature's diff (`f`,
//! Del, `c`, Shift+E), in their own words (`review_items::words`). Where each
//! goes is worked out again from the plan's text whenever it shows
//! (`plan_review::place`): the line it quotes, else the heading it names,
//! else the file's top, saying it isn't placed. While the final plan is
//! written from them, and once Plan review is done, they are frozen: only to
//! read, each key saying why.

use std::path::Path;

use crate::{
    Gerdr,
    diff::{FileDiff, LineKind},
    doc_review::DocFile,
    feature::{
        Feature, Stage, Status,
        plan_review::{Placed, Source, State, Suggestion, place},
        review_import::Decision,
        review_items::{self, ItemOf},
    },
    review::{Comment, Flag, Quoted},
    review_cards::ItemAct,
    worktree_diff::{Env, WorktreeDiff},
};

/// Why the suggestions can't be decided while the final plan is written,
/// and once Plan review is done: the page's words.
pub(crate) use crate::review_cards::{SUGGESTIONS_DONE as DONE, SUGGESTIONS_WRITING as WRITING};
/// Why they can't be on a feature another machine writes.
const ELSEWHERE: &str = "This feature is written on another machine: decide on its page there";
/// Why they can't be on a closed feature.
const CLOSED: &str = "The feature is closed: its suggestions are only read";
/// Why they can't be when nothing else says.
const NOT_NOW: &str = "The suggestions can't be decided now";
/// Why a comment of the user's own does nothing on the plan.
pub const FLAGS_ONLY: &str =
    "Here c comments on a suggestion: add your own on the Plan review page";
/// The label of Send back on the plan: the notes go to the reviewer.
pub const ASK_REVIEWER: &str = "Ask reviewer";

/// Plan review's suggestions as a `DocFile::Suggestions` tab shows them
/// (`DocOf::suggestions`): as the record had them last, the one to put the
/// cursor on next, and why they are only read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuggestionsOf {
    /// In the page's order (`PlanReview::in_order`).
    pub list: Vec<Suggestion>,
    /// The suggestion the cursor goes to once it is laid, by id.
    pub focus: Option<String>,
    /// Why they can't be decided now; none while they can.
    pub frozen: Option<&'static str>,
}

impl SuggestionsOf {
    /// `feature`'s suggestions as they stand, the cursor to go to `focus`.
    pub fn of(feature: Option<&Feature>, focus: Option<String>) -> Self {
        let list = (feature.and_then(|f| f.plan_review.as_ref()))
            .map(|r| r.in_order().into_iter().cloned().collect())
            .unwrap_or_default();
        SuggestionsOf {
            list,
            focus,
            frozen: frozen(feature),
        }
    }

    /// What Approve says: the final plan is written from what was accepted,
    /// or with nothing accepted the plan is used as it is.
    pub fn approve_label(&self) -> &'static str {
        match self.list.iter().any(|s| s.state == State::Accepted) {
            true => "Write final plan",
            false => "Use the plan as it is",
        }
    }

    /// Whether there is anything to ask the reviewer: a comment on one of
    /// its suggestions.
    pub fn asks(&self) -> bool {
        (self.list.iter()).any(|s| s.source == Source::Reviewer && !s.note.trim().is_empty())
    }
}

/// Why `feature`'s suggestions can't be decided now; none while they can.
pub fn frozen(feature: Option<&Feature>) -> Option<&'static str> {
    let Some(feature) = feature else {
        return Some("That feature is gone");
    };
    if feature.is_done(Stage::PlanReview) {
        return Some(DONE);
    }
    if feature.plan_review.as_ref().is_some_and(|r| r.writing) {
        return Some(WRITING);
    }
    if feature.writer.is_some() {
        return Some(ELSEWHERE);
    }
    if feature.status != Status::Open {
        return Some(CLOSED);
    }
    if crate::review_cards::can_decide(feature, Stage::PlanReview) {
        return None;
    }
    Some(crate::review_cards::why_not_decide(feature, Stage::PlanReview).unwrap_or(NOT_NOW))
}

/// `text` with its first letter a capital: "Not placed in the plan".
fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What suggestion `s`'s flag says, `placed` saying whether it is on a line:
/// its title, why it was rejected, what it is, why, the change and what it
/// cites.
fn suggestion_text(s: &Suggestion, placed: bool) -> String {
    let words = review_items::words(ItemOf::Suggestion);
    let mut text = s.title.clone();
    if s.state == State::Rejected && !s.reason.is_empty() {
        text.push_str(&format!("\n{}: {}", capital(words.refused), s.reason));
    }
    let mut what = vec![s.kind.label().to_owned()];
    if !s.plan.is_empty() {
        what.push(s.plan.clone());
    }
    if !placed {
        what.push(words.not_placed.to_owned());
    }
    if s.dropped {
        what.push("left out by a later run".into());
    }
    text.push('\n');
    text.push_str(&capital(&what.join(" \u{b7} ")));
    if !s.why.is_empty() {
        text.push('\n');
        text.push_str(&s.why);
    }
    match &s.my_change {
        Some(mine) => text.push_str(&format!("\n{}: {mine}", words.my_change)),
        None if !s.change.is_empty() => {
            text.push_str(&format!("\n{}: {}", words.change, s.change));
        }
        None => {}
    }
    for e in &s.evidence {
        text.push_str(&format!("\n{}:{} {}", e.file, e.line, e.quote));
        if e.found == Some(false) {
            text.push_str(" (not found in the code)");
        }
    }
    text
}

/// The suggestions of `list` as flagged comments on `name` (the plan, read
/// as `text` and shown as `file`) under `root`: each on the line it is
/// placed at, else on the whole file; the user's comment on one right under
/// it. `frozen` says why none can be decided now.
pub fn suggestion_comments(
    list: &[Suggestion],
    text: &str,
    root: &Path,
    name: &str,
    file: Option<&FileDiff>,
    frozen: Option<&'static str>,
) -> Vec<Comment> {
    let line = |n: usize| {
        let mut lines = file?.hunks.iter().flat_map(|h| &h.lines);
        lines.find(|l| l.number == n && l.kind != LineKind::Removed)
    };
    let mut comments = Vec::new();
    for s in list {
        let on = match place(s, text) {
            Placed::Quote(n) | Placed::Heading(n) => line(n),
            Placed::Nowhere => None,
        };
        let flag = Flag {
            id: s.id.clone(),
            severity: s.severity,
            state: s.standing(),
            yours: s.source == Source::You,
            of: ItemOf::Suggestion,
            frozen,
            unanswered: s.unanswered(),
            ..Flag::default()
        };
        let comment = Comment {
            root: root.to_path_buf(),
            path: name.to_owned(),
            lines: on.map(Quoted::of).into_iter().collect(),
            text: suggestion_text(s, on.is_some()),
            flag: Some(flag.clone()),
        };
        let note = (!s.note.is_empty() && s.source != Source::You).then(|| Comment {
            text: s.note.clone(),
            flag: Some(Flag {
                yours: true,
                note: true,
                ..flag
            }),
            ..comment.clone()
        });
        comments.push(comment);
        comments.extend(note);
    }
    comments
}

impl WorktreeDiff {
    /// Lay the suggestions, as `DocOf::suggestions` has them, on the plan
    /// as it reads now; the cursor on the one to focus, once shown.
    pub fn place_suggestions(&mut self, env: Env) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(of) = doc.suggestions.as_ref() else {
            return;
        };
        let text = doc.text.as_deref().unwrap_or_default();
        let file = self.diff.diffs.get(&self.root).and_then(|(_, f)| f.first());
        let name = doc.name();
        let comments = suggestion_comments(&of.list, text, &self.root, &name, file, of.frozen);
        let (focus, frozen) = (of.focus.clone(), of.frozen);
        self.diff.notes_open = frozen.is_none();
        self.diff.flags_only = Some(FLAGS_ONLY);
        // Not shown yet (the plan not read): it waits for the next lay.
        if self.lay_flags(comments, focus.as_deref(), None, env)
            && let Some(of) = self.doc.as_mut().and_then(|d| d.suggestions.as_mut())
        {
            of.focus = None;
        }
    }
}

impl Gerdr {
    /// Review feature `slug`'s suggestions where they apply in the plan:
    /// its Suggestions tab, the cursor on the first still to decide.
    pub(crate) fn review_in_plan(&mut self, project: &str, slug: &str) {
        let first = (self.feature(project, slug))
            .and_then(|f| f.plan_review.as_ref()?.first_open())
            .map(|s| s.id.clone());
        self.open_suggestions(project, slug, first);
    }

    /// Show suggestion `id` of feature `slug` where it applies in the plan,
    /// the cursor on it.
    pub(crate) fn show_in_plan(&mut self, project: &str, slug: &str, id: &str) {
        self.open_suggestions(project, slug, Some(id.to_owned()));
    }

    /// Open (or show) feature `slug`'s Suggestions tab, the cursor going to
    /// `focus` if given.
    fn open_suggestions(&mut self, project: &str, slug: &str, focus: Option<String>) {
        self.open_doc_tab(project, slug, DocFile::Suggestions);
        self.suggestions_follow(Some((project, slug)), focus);
    }

    /// Bring what shows Plan review's suggestions in line with the record:
    /// after a save, and as the watch reads a rewrite in.
    pub(crate) fn plan_doc_follows(&mut self) {
        self.suggestions_follow(None, None);
    }

    /// The Suggestions tabs, of feature `only` or of any: by workspace and
    /// tab, with their feature.
    fn suggestion_tabs(&self, only: Option<(&str, &str)>) -> Vec<(usize, usize, String, String)> {
        let mut tabs = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                let Some(doc) = tab.review().and_then(|r| r.wd.doc.as_ref()) else {
                    continue;
                };
                let wanted = only.is_none_or(|(p, s)| doc.project == p && doc.slug == s);
                if doc.file == DocFile::Suggestions && wanted {
                    tabs.push((w, t, doc.project.clone(), doc.slug.clone()));
                }
            }
        }
        tabs
    }

    /// Lay the suggestions again in the Suggestions tabs (of feature `only`,
    /// or of any) whose record changed, or, with `focus`, the cursor going
    /// to it. A comment being written there waits: they are laid once it is
    /// kept.
    fn suggestions_follow(&mut self, only: Option<(&str, &str)>, focus: Option<String>) {
        for (w, t, project, slug) in self.suggestion_tabs(only) {
            let fresh = SuggestionsOf::of(self.feature(&project, &slug), focus.clone());
            let Some(mode) = self.workspaces[w].tabs[t].review().map(|r| r.mode) else {
                continue;
            };
            let env = self.doc_env(mode);
            let Some(review) = self.workspaces[w].tabs[t].review_mut() else {
                continue;
            };
            let wd = &mut review.wd;
            let Some(doc) = wd.doc.as_mut() else {
                continue;
            };
            if wd.diff.editing.is_some() || doc.suggestions.as_ref() == Some(&fresh) {
                continue;
            }
            doc.suggestions = Some(fresh);
            wd.place_suggestions(env);
        }
    }

    /// The feature whose Suggestions tab shows.
    fn shown_suggestions(&self) -> Option<(String, String)> {
        let doc = self.wd()?.doc.as_ref()?;
        (doc.file == DocFile::Suggestions).then(|| (doc.project.clone(), doc.slug.clone()))
    }

    /// Suggestion `id`, flagged on the plan, decided: in the findings' words
    /// (Fix is accept, Dismiss reject, Reopen clear). Decided, the cursor
    /// goes on to the next still to decide.
    pub(crate) fn plan_doc_decide(&mut self, id: &str, decision: Decision) {
        let Some((project, slug)) = self.shown_suggestions() else {
            return;
        };
        let next = (!matches!(decision, Decision::Reopen))
            .then(|| self.wd()?.diff.next_undecided(id))
            .flatten();
        self.apply_suggestion(&project, &slug, id, ItemAct::Decide(decision));
        if next.is_some() {
            self.suggestions_follow(Some((&project, &slug)), next);
        }
    }

    /// The user's own suggestion `id`, flagged on the plan, deleted (with
    /// Undo).
    pub(crate) fn plan_doc_delete_own(&mut self, id: &str) {
        if let Some((project, slug)) = self.shown_suggestions() {
            self.apply_suggestion(&project, &slug, id, ItemAct::DeleteOwn);
        }
    }

    /// Suggestion `id`, flagged on the plan, reworded in its dialog: its
    /// change, or what one of the user's own says.
    pub(crate) fn plan_doc_edit(&mut self, id: &str) {
        if let Some((project, slug)) = self.shown_suggestions() {
            self.apply_suggestion(&project, &slug, id, ItemAct::Edit);
        }
    }

    /// The user's comment on suggestion `id`, written on the plan, kept: a
    /// question's answer. Emptied, it is deleted (with Undo).
    pub(crate) fn plan_doc_note(&mut self, id: &str, text: &str) {
        let Some((project, slug)) = self.shown_suggestions() else {
            return;
        };
        match text.is_empty() {
            true => self.apply_suggestion(&project, &slug, id, ItemAct::DeleteComment),
            false => {
                self.note_suggestion(&project, &slug, id, text);
                self.plan_doc_follows();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        doc_review::added_file,
        feature::{
            plan_review::{Evidence, Kind, PlanReview},
            review_import::{FindingState, Severity},
        },
    };

    const PLAN: &str =
        "# Plan\n\n## Milestone 1\nAdd the login form.\n\n## Milestone 2\nStore the token.\n";

    fn suggestion(id: &str) -> Suggestion {
        Suggestion {
            id: id.into(),
            severity: Severity::High,
            title: format!("Title of {id}"),
            why: "It breaks".into(),
            change: "Do it right".into(),
            run: 1,
            ..Suggestion::default()
        }
    }

    fn comments(list: &[Suggestion], frozen: Option<&'static str>) -> Vec<Comment> {
        let file = added_file("plan.md", PLAN);
        suggestion_comments(list, PLAN, Path::new("/d"), "plan.md", Some(&file), frozen)
    }

    #[test]
    fn a_suggestion_goes_on_its_quote_its_heading_or_the_file() {
        let quoted = Suggestion {
            plan_quote: "**Store** the  token".into(),
            ..suggestion("r1-1")
        };
        let headed = Suggestion {
            plan: "Milestone 1".into(),
            ..suggestion("r1-2")
        };
        let nowhere = Suggestion {
            plan: "Somewhere else".into(),
            ..suggestion("r1-3")
        };
        let laid = comments(&[quoted, headed, nowhere], None);
        let lines: Vec<Option<(usize, &str)>> = (laid.iter())
            .map(|c| c.lines.first().map(|q| (q.number, q.text.as_str())))
            .collect();
        assert_eq!(
            lines,
            [
                Some((7, "Store the token.")),
                Some((3, "## Milestone 1")),
                None
            ]
        );
        assert!((laid.iter()).all(|c| c.path == "plan.md" && c.root == Path::new("/d")));
        assert!(
            laid[2]
                .text
                .contains("Somewhere else \u{b7} not placed in the plan")
        );
        assert!(!laid[0].text.contains("not placed"));
        // No diff read yet: on the file till it is.
        let one = [suggestion("r1-1")];
        let unread = suggestion_comments(&one, PLAN, Path::new("/d"), "plan.md", None, None);
        assert!(unread[0].lines.is_empty());
    }

    #[test]
    fn a_flag_says_what_the_suggestion_is_in_its_words() {
        let rejected = Suggestion {
            state: State::Rejected,
            reason: "Out of scope".into(),
            my_change: Some("Mine".into()),
            evidence: vec![Evidence {
                file: "src/a.rs".into(),
                line: 4,
                quote: "fn a()".into(),
                found: Some(false),
                ..Evidence::default()
            }],
            ..suggestion("r1-1")
        };
        let laid = comments(&[rejected, suggestion("r1-2")], None);
        let text = &laid[0].text;
        assert!(
            text.starts_with("Title of r1-1\nRejected: Out of scope\nGap"),
            "{text}"
        );
        assert!(text.contains("\nYour change: Mine"));
        assert!(text.contains("\nsrc/a.rs:4 fn a() (not found in the code)"));
        assert!(laid[1].text.contains("\nChange: Do it right"));
        for c in &laid {
            let lower = c.text.to_lowercase();
            assert!(
                !lower.contains("fix") && !lower.contains("dismiss"),
                "{lower}"
            );
        }
        let flag = laid[0].flag.as_ref().unwrap();
        assert_eq!(
            (flag.of, flag.state, flag.id.as_str()),
            (ItemOf::Suggestion, FindingState::Dismissed, "r1-1")
        );
    }

    #[test]
    fn the_users_comment_follows_its_suggestion_and_own_ones_are_theirs() {
        let noted = Suggestion {
            kind: Kind::Question,
            note: "Yes, both".into(),
            ..suggestion("r1-1")
        };
        let question = Suggestion {
            kind: Kind::Question,
            ..suggestion("r1-2")
        };
        let own = Suggestion {
            source: Source::You,
            state: State::Accepted,
            ..suggestion("u1")
        };
        let laid = comments(&[noted, question, own], Some(WRITING));
        assert_eq!(laid.len(), 4);
        let flags: Vec<&Flag> = laid.iter().map(|c| c.flag.as_ref().unwrap()).collect();
        assert!(flags[1].note && flags[1].yours && flags[1].id == "r1-1");
        assert_eq!(laid[1].text, "Yes, both");
        assert_eq!(laid[1].lines, laid[0].lines);
        // Answered, a question can be accepted; not before.
        assert!(!flags[0].unanswered && flags[2].unanswered);
        assert!(flags[3].yours && !flags[3].note);
        assert!(flags.iter().all(|f| f.frozen == Some(WRITING)));
    }

    #[test]
    fn suggestions_freeze_while_written_from_and_once_done() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 2);
        f.plan_review = Some(PlanReview {
            n: 1,
            stamp: Some((1, 1)),
            suggestions: vec![suggestion("r1-1")],
            ..PlanReview::default()
        });
        // Decided only while the page decides them.
        let open = crate::review_cards::can_decide(&f, Stage::PlanReview);
        assert_eq!(frozen(Some(&f)).is_none(), open);
        f.plan_review.as_mut().unwrap().writing = true;
        assert_eq!(frozen(Some(&f)), Some(WRITING));
        let mut done = crate::features::tests::feature("login", Vec::new(), 3);
        done.plan_review = f.plan_review.clone();
        assert_eq!(frozen(Some(&done)), Some(DONE));
        assert_eq!(frozen(None), Some("That feature is gone"));
        let of = SuggestionsOf::of(Some(&f), Some("r1-1".into()));
        assert_eq!(of.list.len(), 1);
        assert_eq!(of.approve_label(), "Use the plan as it is");
        assert!(!of.asks());
    }

    #[test]
    fn the_cursor_goes_to_the_suggestion_asked_for() {
        let mut wd = crate::worktree_diff::tests::doc();
        let doc = wd.doc.as_mut().unwrap();
        doc.file = DocFile::Suggestions;
        let headed = Suggestion {
            plan: "Milestone 2".into(),
            ..suggestion("r1-2")
        };
        doc.suggestions = Some(SuggestionsOf {
            list: vec![suggestion("r1-1"), headed],
            focus: Some("r1-2".into()),
            frozen: None,
        });
        doc.text = None;
        wd.set_doc(Ok(PLAN.into()), crate::worktree_diff::tests::ENV);
        let of = wd.doc.as_ref().unwrap().suggestions.as_ref().unwrap();
        assert_eq!(of.focus, None);
        assert_eq!(on(&wd).as_deref(), Some("r1-2"));
        assert_eq!(wd.diff.comments_to_visit(), Some("suggestion"));
    }

    /// The id of the item the cursor is on.
    fn on(wd: &WorktreeDiff) -> Option<String> {
        let row = wd.diff.cursor.as_ref()?.row;
        let c = wd.diff.comment_of_row(row)?;
        Some(wd.diff.review.comments.get(c)?.flag.as_ref()?.id.clone())
    }

    /// The plan with `list` flagged on it, `frozen` if so, the cursor on the
    /// first.
    fn flagged(list: Vec<Suggestion>, frozen: Option<&'static str>) -> WorktreeDiff {
        let mut wd = crate::worktree_diff::tests::doc();
        let doc = wd.doc.as_mut().unwrap();
        doc.file = DocFile::Suggestions;
        let focus = list.first().map(|s| s.id.clone());
        doc.suggestions = Some(SuggestionsOf {
            list,
            focus,
            frozen,
        });
        doc.text = None;
        wd.set_doc(Ok(PLAN.into()), crate::worktree_diff::tests::ENV);
        wd
    }

    fn press(wd: &mut WorktreeDiff, shift: bool, sym: smithay::input::keyboard::Keysym) {
        let mods = smithay::input::keyboard::ModifiersState {
            shift,
            ..Default::default()
        };
        wd.key(&mods, sym, sym, crate::worktree_diff::tests::ENV);
    }

    fn labels(wd: &WorktreeDiff) -> Vec<String> {
        let view = wd.view(crate::worktree_diff::tests::ENV);
        view.row_buttons.into_iter().map(|(_, h)| h.label).collect()
    }

    #[test]
    fn a_suggestions_keys_say_its_words_and_signal_its_kind() {
        use smithay::input::keyboard::Keysym;
        let mut wd = flagged(vec![suggestion("r1-1")], None);
        assert_eq!(on(&wd).as_deref(), Some("r1-1"));
        let shown = labels(&wd);
        for want in ["Accept", "Reject\u{2026}", "Comment", "Edit change\u{2026}"] {
            assert!(shown.iter().any(|l| l == want), "{want} in {shown:?}");
        }
        assert!(
            !shown
                .iter()
                .any(|l| l.contains("Fix") || l.contains("Dismiss"))
        );
        press(&mut wd, false, Keysym::f);
        let taken = wd.take_error();
        assert_eq!(
            taken.decided,
            Some((ItemOf::Suggestion, "r1-1".into(), Decision::Fix))
        );
        // Del asks why, in the suggestion's words, and the reason goes with it.
        press(&mut wd, false, Keysym::Delete);
        assert!(wd.diff.editing.is_some());
        let view = wd.view(crate::worktree_diff::tests::ENV);
        let said = (view.rows.iter()).any(|row| {
            matches!(row, crate::chrome::DetailRow::Note(t) if t.starts_with("Why reject it?"))
        });
        assert!(said);
        press(&mut wd, false, Keysym::Return);
        assert_eq!(
            wd.take_error().error.as_deref(),
            Some(crate::diff_view::NO_REJECT_REASON)
        );
        for sym in [Keysym::n, Keysym::o] {
            press(&mut wd, false, sym);
        }
        press(&mut wd, false, Keysym::Return);
        assert_eq!(
            wd.take_error().decided,
            Some((
                ItemOf::Suggestion,
                "r1-1".into(),
                Decision::Dismiss("no".into())
            ))
        );
        press(&mut wd, true, Keysym::e);
        assert_eq!(
            wd.take_error().edit_finding,
            Some((ItemOf::Suggestion, "r1-1".into()))
        );
        // A comment of the user's own has no place on the plan.
        let down = Keysym::Down;
        for _ in 0..12 {
            press(&mut wd, false, down);
        }
        press(&mut wd, false, Keysym::c);
        assert_eq!(wd.take_error().error.as_deref(), Some(FLAGS_ONLY));
        assert!(wd.diff.editing.is_none());
    }

    #[test]
    fn a_question_is_answered_before_it_is_accepted() {
        use smithay::input::keyboard::Keysym;
        let question = Suggestion {
            kind: Kind::Question,
            ..suggestion("r1-1")
        };
        let mut wd = flagged(vec![question], None);
        let shown = labels(&wd);
        assert!(shown.iter().any(|l| l == "Answer") && !shown.iter().any(|l| l == "Accept"));
        press(&mut wd, false, Keysym::f);
        let taken = wd.take_error();
        assert_eq!(taken.error.as_deref(), Some(crate::diff_view::UNANSWERED));
        assert_eq!(taken.decided, None);
        // c writes the answer, kept as the suggestion's comment.
        press(&mut wd, false, Keysym::c);
        press(&mut wd, false, Keysym::y);
        press(&mut wd, false, Keysym::Return);
        assert_eq!(
            wd.take_error().noted,
            Some((ItemOf::Suggestion, "r1-1".into(), "y".into()))
        );
    }

    #[test]
    fn a_frozen_suggestion_refuses_every_key_saying_why() {
        use smithay::input::keyboard::Keysym;
        let noted = Suggestion {
            note: "Mine".into(),
            ..suggestion("r1-1")
        };
        let mut wd = flagged(vec![noted], Some(WRITING));
        assert!(!labels(&wd).iter().any(|l| l == "Accept"));
        for (shift, sym) in [
            (false, Keysym::f),
            (false, Keysym::Delete),
            (false, Keysym::c),
            (true, Keysym::e),
        ] {
            press(&mut wd, shift, sym);
            let taken = wd.take_error();
            assert_eq!(taken.error.as_deref(), Some(WRITING), "{sym:?}");
            assert!(taken.decided.is_none() && taken.edit_finding.is_none());
            assert!(taken.noted.is_none() && wd.diff.editing.is_none());
        }
    }

    #[test]
    fn the_suggestions_tab_writes_the_final_plan_and_asks_the_reviewer() {
        use crate::{chrome::ReposButton, worktree_diff::Done};
        let accepted = Suggestion {
            state: State::Accepted,
            note: "Why so?".into(),
            ..suggestion("r1-1")
        };
        let mut wd = flagged(vec![accepted], None);
        let env = crate::worktree_diff::tests::ENV;
        let view = wd.view(env);
        let title: Vec<&str> = view
            .title_buttons
            .iter()
            .map(|h| h.label.as_str())
            .collect();
        assert!(title.contains(&ASK_REVIEWER) && title.contains(&"Write final plan"));
        assert!(!title.contains(&"Comment on file"));
        assert_eq!(wd.press(ReposButton::Approve, env), Some(Done::Approve));
        assert_eq!(
            wd.press(ReposButton::SendBack, env),
            Some(Done::SendBack(String::new()))
        );
        let hints: Vec<String> = wd.hints().into_iter().map(|h| h.label).collect();
        assert!(hints.iter().any(|h| h == "write final plan"));
        assert!(hints.iter().any(|h| h == "ask reviewer"));
        assert!(hints.iter().any(|h| h == "next, previous suggestion"));
    }
}
