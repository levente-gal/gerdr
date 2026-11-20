//! What the phone view may do to a feature (see `phone` and
//! `phone_control`): press the buttons of the page of the stage it is at,
//! as the desktop shows them, decide its AI review's findings, and send a
//! stage's file back to its agent.
//!
//! Only buttons that act without a desk: none that shows or opens
//! something here, or edits in a dialog. What gerdr says while it acts
//! (its toasts) goes back as the answer. A confirm it asks is put to the
//! phone: cancelled here, and pressed again with `confirm` it is answered
//! Go, so no dialog waits on an empty desk.

use serde_json::{Value, json};

use super::*;
use crate::{confirm::Outcome, toast::Level};

/// Whether the phone may press a button doing `outcome`.
fn phone_may(outcome: &PanelOutcome) -> bool {
    use PanelOutcome as O;
    matches!(
        outcome,
        O::Tick
            | O::StartAgent(..)
            | O::SendPrompt(_)
            | O::ResumeAgent(_)
            | O::ApproveDoc(_)
            | O::Continue(_)
            | O::RunChecks
            | O::RunFailed
            | O::RerunCheck(_)
            | O::StopCheck(_)
            | O::StopChecks
            | O::SetUp
            | O::FixSandbox
            | O::SendFixes
            | O::MakeFindingFixes
            | O::FixAllFindings
            | O::PlanFixes
            | O::MakeFixes
            | O::ResendFixes
            | O::MakeVerifyFixes
            | O::AskFixPlan
            | O::DropFixPlan
            | O::AcceptAllSuggestions
            | O::AskReviewer
            | O::WriteFinalPlan
            | O::SkipPlanReview
            | O::RerunResearch(_)
            | O::RerunCi
            | O::RerunShipCi(_)
            | O::FixCi(..)
            | O::ResolveConflicts(..)
            | O::ResolveAllConflicts(_)
            | O::UpdateAllBases
            | O::StopResolving(_)
            | O::FreshMilestones(_)
            | O::StartOver(_)
            | O::NotYet(_)
    )
}

/// A stage's mark as the phone's styles name it: `done`, `current`,
/// `waiting`.
fn mark_word(mark: NodeMark) -> String {
    format!("{mark:?}").to_lowercase()
}

/// A page's line as the phone shows it: its words and their tone.
fn check_json(line: &chrome::CheckLine) -> Value {
    let tone = match line.tone {
        chrome::CheckTone::Blocker => "blocker",
        chrome::CheckTone::Ready => "ready",
        chrome::CheckTone::Note => "note",
        chrome::CheckTone::Failed => "failed",
    };
    json!({ "text": line.text, "tone": tone })
}

/// A dot as the phone's styles name it: `working`, `done`, `failed`.
fn dot_word(dot: chrome::Dot) -> String {
    format!("{dot:?}").to_lowercase()
}

/// A page's section as the phone shows it: what it says, not its buttons
/// or where it goes. None for a row of buttons alone.
fn section_json(section: &chrome::PageSection) -> Option<Value> {
    use chrome::PageSection as S;
    let checks = |lines: &[chrome::CheckLine]| lines.iter().map(check_json).collect::<Vec<_>>();
    Some(match section {
        S::Agent {
            name,
            dot,
            status,
            tail,
            waits,
            ..
        } => json!({
            "kind": "agent",
            "name": name,
            "dot": dot_word(*dot),
            "status": status,
            "waits": waits,
            "tail": tail,
        }),
        S::File { name, state, .. } => {
            json!({ "kind": "file", "name": name, "state": check_json(state) })
        }
        S::Finding(c) => json!({
            "kind": "finding",
            "tag": c.tag.word(),
            "dot": c.tag.dot().map(dot_word),
            "title": c.title,
            "state": c.state,
            "clock": c.clock,
            "place": c.place,
            "body": c.body,
            "preview": c.preview,
            "fix": c.fix,
            "note": c.note,
            "comment": c.comment,
            "made": c.made,
            "faded": c.faded,
        }),
        S::Round(r) => json!({
            "kind": "round",
            "label": r.label,
            "dot": dot_word(r.dot),
            "state": check_json(&r.state),
            "meta": r.meta,
            "askedCaption": r.asked_caption,
            "asked": (r.asked.iter())
                .map(|a| json!({ "id": a.id, "title": a.title, "note": a.note, "now": a.now.as_ref().map(check_json) }))
                .collect::<Vec<_>>(),
            "fixes": r.fixes,
            "running": r.running,
            "changed": r.changed,
        }),
        S::Toolbar { .. } => return None,
        S::Lines { label, lines } => {
            json!({ "kind": "lines", "label": label, "lines": checks(lines) })
        }
        S::Doc { name, .. } => json!({ "kind": "doc", "name": name }),
        S::MilestoneMode { fresh } => {
            let text = match fresh {
                true => "a fresh agent for each milestone",
                false => "one agent through all milestones",
            };
            json!({ "kind": "lines", "label": "Agents", "lines": [{ "text": text, "tone": "note" }] })
        }
        S::ShipGroup(g) => json!({
            "kind": "ship",
            "head": g.head,
            "card": g.card.as_ref().map(|c| json!({
                "dot": dot_word(c.dot),
                "state": check_json(&c.state),
                "meta": c.meta,
                "base": c.base.as_ref().map(check_json),
                "ci": c.ci.as_ref().map(check_json),
            })),
            "note": g.note,
            "steps": (g.steps.iter())
                .map(|s| json!({ "label": s.label, "detail": s.detail.as_ref().map(check_json), "ticked": s.ticked, "done": s.done, "off": s.off }))
                .collect::<Vec<_>>(),
        }),
        S::Recap(r) => json!({
            "kind": "recap",
            "summary": r.summary,
            "summaryNote": r.summary_note,
            "figures": r.figures,
            "stages": r.stages,
            "lines": checks(&r.lines),
        }),
        S::Milestones {
            label,
            state,
            done,
            total,
            items,
            running,
            ..
        } => json!({
            "kind": "milestones",
            "label": label,
            "state": state.as_ref().map(check_json),
            "done": done,
            "total": total,
            "items": items,
            "running": running,
        }),
        S::Review(r) => json!({
            "kind": "review",
            "label": r.label,
            "words": r.words,
            "decided": r.decided,
            "refused": r.refused,
            "total": r.total,
            "notes": checks(&r.notes),
        }),
        S::Heading(text) => json!({ "kind": "heading", "text": text }),
    })
}

/// The stage whose file waits for the user's review: the brief or a plan
/// its agent finished.
fn doc_waiting(feature: &Feature) -> Option<Stage> {
    let stage = feature.current()?;
    (feature.status == Status::Open
        && matches!(stage, Stage::Discover | Stage::Plan | Stage::PlanReview)
        && feature.runs.get(&stage).is_some_and(|r| r.ready.is_some()))
    .then_some(stage)
}

/// What the phone asks a feature: its page's buttons, or a press.
#[derive(Debug, PartialEq)]
pub(crate) enum PhoneAct {
    /// The button labelled so on the page of the stage it is at.
    Press(String),
    /// Decide finding `id` of its AI review.
    Decide(String, Decision),
    /// Send the current stage's file back to its agent, with these words.
    SendBack(String),
}

impl Gerdr {
    /// The buttons the phone may press on feature `slug`, as the desktop's
    /// page of the stage it is at shows them.
    fn phone_buttons(
        &self,
        project: &str,
        slug: &str,
    ) -> Result<Vec<(PageButton, PanelOutcome)>, String> {
        let feature = self
            .feature(project, slug)
            .ok_or_else(|| "gerdr has no such feature".to_owned())?;
        let ctx = self.row_ctx(feature);
        let mut buttons = page_actions(feature, own_page(feature), ctx);
        buttons.retain(|(_, outcome)| phone_may(outcome));
        // The desktop approves a file in its review tab; the phone, here.
        if let Some(stage) = doc_waiting(feature)
            && !buttons
                .iter()
                .any(|(_, o)| *o == PanelOutcome::ApproveDoc(stage))
        {
            let button = PageButton {
                label: "Approve".to_owned(),
                style: ButtonStyle::Primary,
            };
            buttons.insert(0, (button, PanelOutcome::ApproveDoc(stage)));
        }
        Ok(buttons)
    }

    /// What the phone may do to feature `slug`, for its page, with its
    /// rail and the page of `stage` (by its place in `Stage::ALL`), the
    /// one the desktop shows when none is asked.
    pub(crate) fn phone_offer(&self, project: &str, slug: &str, stage: Option<usize>) -> Value {
        let buttons = match self.phone_buttons(project, slug) {
            Ok(buttons) => buttons,
            Err(why) => return json!({ "error": why }),
        };
        let elsewhere = self.feature_writer(project, slug).map(str::to_owned);
        let send_back = self.feature(project, slug).and_then(doc_waiting).is_some();
        let buttons: Vec<Value> = buttons
            .iter()
            .map(|(b, _)| json!({ "label": b.label, "primary": b.style == ButtonStyle::Primary }))
            .collect();
        let (rail, page) = self.phone_view(project, slug, stage).unwrap_or_default();
        json!({
            "buttons": buttons,
            "sendBack": send_back && elsewhere.is_none(),
            "elsewhere": elsewhere,
            "rail": rail,
            "page": page,
        })
    }

    /// Feature `slug`'s rail as its feature tab draws it (each stage's
    /// mark, short line and agent, and the line back to Build), and the
    /// page of `stage` as its sections say it, in words.
    fn phone_view(
        &self,
        project: &str,
        slug: &str,
        stage: Option<usize>,
    ) -> Option<(Value, Value)> {
        let feature = self.feature(project, slug)?;
        let panel = FeaturePanel {
            project: project.to_owned(),
            slug: slug.to_owned(),
            stage: stage.and_then(|i| Stage::ALL.get(i).copied()),
            picked_at: feature.current(),
            ..FeaturePanel::default()
        };
        let shown = panel.page_stage(feature);
        let view = self.feature_tab_view_of(&panel)?;
        let nodes: Vec<Value> = (view.rail.iter())
            .map(|n| {
                json!({
                    "label": n.label,
                    "line": n.line,
                    "mark": mark_word(n.mark),
                    "word": n.mark.word(),
                    "agent": n.agent.as_ref().map(|(_, what)| what),
                })
            })
            .collect();
        let rail = json!({
            "nodes": nodes,
            "back": view.back.map(|b| json!({ "from": b.from, "to": b.to })),
        });
        let page = &view.page;
        let page = json!({
            "stage": Stage::ALL.iter().position(|s| *s == shown),
            "label": shown.label(),
            "own": shown == own_page(feature),
            "decides": shown == Stage::AiReview,
            "title": page.title,
            "about": page.about,
            "status": page.status.as_ref().map(check_json),
            "sections": page.sections.iter().filter_map(section_json).collect::<Vec<_>>(),
        });
        Some((rail, page))
    }

    /// The sidebar cards of the features asked for, by `project/slug`:
    /// each stage's mark, the line, the tone, and the stage dot that is
    /// big and whether it blinks.
    pub(crate) fn phone_cards(&self, features: &[(String, String)]) -> Value {
        let cards: serde_json::Map<String, Value> = (features.iter())
            .filter_map(|(project, slug)| {
                let card = self.feature_card(self.feature(project, slug)?);
                let tone = match card.tone {
                    chrome::CardTone::Idle => "idle",
                    chrome::CardTone::Working => "working",
                    chrome::CardTone::Waits => "waits",
                    chrome::CardTone::Failed => "failed",
                };
                let marks: Vec<String> = card.marks.iter().map(|m| mark_word(*m)).collect();
                let value = json!({
                    "line": card.line,
                    "tone": tone,
                    "marks": marks,
                    "big": card.big_dot(),
                    "running": card.running_dot().is_some(),
                });
                Some((format!("{project}/{slug}"), value))
            })
            .collect();
        json!({ "cards": cards })
    }

    /// Do `act` to feature `slug`, for the phone. `confirm`: answer a
    /// confirm it asks with Go, which the phone asked the user first.
    pub(crate) fn phone_act(
        &mut self,
        project: &str,
        slug: &str,
        act: PhoneAct,
        confirm: bool,
    ) -> Value {
        if let Some(machine) = self.feature_writer(project, slug) {
            let why = format!("{machine} writes it: act there, or take it over on the desktop");
            return json!({ "error": why });
        }
        let outcome = match act {
            PhoneAct::Press(label) => {
                let buttons = match self.phone_buttons(project, slug) {
                    Ok(buttons) => buttons,
                    Err(why) => return json!({ "error": why }),
                };
                match buttons.into_iter().find(|(b, _)| b.label == label) {
                    Some((_, outcome)) => outcome,
                    None => {
                        let why = format!("“{label}” isn't on its page any more");
                        return json!({ "error": why });
                    }
                }
            }
            PhoneAct::Decide(id, decision) => PanelOutcome::DecideFinding(id, decision),
            PhoneAct::SendBack(text) => {
                let Some(stage) = self.feature(project, slug).and_then(Feature::current) else {
                    return json!({ "error": "gerdr has no such feature" });
                };
                let (project, slug) = (project.to_owned(), slug.to_owned());
                return self.phone_said(confirm, move |g| {
                    g.send_back_stage(&project, &slug, stage, text);
                });
            }
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.phone_said(confirm, move |g| {
            g.apply_feature_outcome(project, slug, outcome)
        })
    }

    /// Run `act`, and say what gerdr said while it did: its toasts, and
    /// the confirm it asked, answered Go if `confirm`, else cancelled and
    /// put to the phone.
    fn phone_said(&mut self, confirm: bool, act: impl FnOnce(&mut Self)) -> Value {
        let last = self.toasts.iter().map(|t| t.id).max().unwrap_or(0);
        let asking = self.overlays.confirm.is_some();
        act(self);
        let mut asks = Value::Null;
        let asked = (self.overlays.confirm.as_ref())
            .filter(|_| !asking)
            .map(|d| json!({ "title": d.title, "body": d.body, "go": d.go }));
        if let Some(question) = asked {
            if confirm {
                self.apply_confirm(Outcome::Go);
            } else {
                asks = question;
                self.apply_confirm(Outcome::Cancel);
            }
        }
        let said: Vec<Value> = (self.toasts.iter())
            .filter(|t| t.id > last)
            .map(|t| {
                let level = match t.level {
                    Level::Info => "info",
                    Level::Success => "success",
                    Level::Warning => "warning",
                    Level::Error => "error",
                };
                json!({ "text": t.text, "level": level })
            })
            .collect();
        json!({ "said": said, "asks": asks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_presses_only_what_needs_no_desk() {
        assert!(phone_may(&PanelOutcome::ApproveDoc(Stage::Plan)));
        assert!(phone_may(&PanelOutcome::Continue(Stage::Build)));
        assert!(phone_may(&PanelOutcome::PlanFixes));
        assert!(!phone_may(&PanelOutcome::OpenDoc(Stage::Plan)));
        assert!(!phone_may(&PanelOutcome::ShowAgent(Stage::Build)));
        assert!(!phone_may(&PanelOutcome::Ship));
        assert!(!phone_may(&PanelOutcome::TakeOver));
        assert!(!phone_may(&PanelOutcome::Remove));
    }
}
