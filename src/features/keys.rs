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
