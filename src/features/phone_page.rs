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

    /// What the phone may do to feature `slug`, for its page.
    pub(crate) fn phone_offer(&self, project: &str, slug: &str) -> Value {
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
        json!({
            "buttons": buttons,
            "sendBack": send_back && elsewhere.is_none(),
            "elsewhere": elsewhere,
        })
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
