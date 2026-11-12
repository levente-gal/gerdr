use super::{tests::feature, tests::repo, *};
use crate::feature::{
    Kind,
    pr::{PullRef, Remote},
};

/// A review of pull request 12 at Verify, its worktree at `abc1234…`.
fn review() -> Feature {
    let mut f = feature("pr-12", vec![repo("api", true)], 4);
    f.kind = Kind::PrReview(vec![PullRef {
        repo: "api".into(),
        host: "github.com".into(),
        slug: "o/api".into(),
        number: 12,
        url: "https://github.com/o/api/pull/12".into(),
        author: "bob".into(),
        base_ref: "main".into(),
        head_ref: "fix".into(),
        head_sha: "abc1234567".into(),
        ..PullRef::default()
    }]);
    f
}

fn github(f: &mut Feature, head: &str, state: &str) {
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    pull.take_remote(Remote {
        at: 1_900,
        head_sha: head.into(),
        base_ref: "main".into(),
        state: state.into(),
        checks: Vec::new(),
    });
}

fn card(page: &chrome::StagePage) -> (&CheckLine, Vec<&str>) {
    page.sections
        .iter()
        .find_map(|s| match s {
            PageSection::File {
                name,
                state,
                buttons,
            } if name == "Pull request #12" => {
                let labels = (buttons.iter())
                    .map(|i| page.buttons[*i].label.as_str())
                    .collect();
                Some((state, labels))
            }
            _ => None,
        })
        .expect("the pull request's card")
}

fn page_of(f: &Feature, stage: Stage) -> chrome::StagePage {
    let mut actions = page_actions(f, stage, RowCtx::default());
    with_pull_actions(&mut actions, f, stage, RowCtx::default());
    stage_page(f, stage, &PanelState::default(), &actions, 2_000)
}

#[test]
fn its_card_says_whose_it_is_and_opens_it() {
    let f = review();
    let page = page_of(&f, Stage::Verify);
    let (state, buttons) = card(&page);
    assert_eq!(state.text, "by @bob · fix into main · at abc1234");
    assert_eq!(buttons, vec![FIX_HERE, EDIT_PULLS, OPEN_PULL]);
    let ctx = RowCtx::default();
    assert_ne!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::UpdatePrHead)
    );
    assert_eq!(feature_agents_gate(&f), None);
}

#[test]
fn each_pull_request_has_its_card() {
    let mut f = review();
    f.repos.push(repo("web", true));
    let Kind::PrReview(pulls) = &mut f.kind else {
        unreachable!()
    };
    pulls.push(PullRef {
        repo: "web".into(),
        number: 13,
        url: "https://github.com/o/web/pull/13".into(),
        author: "bob".into(),
        head_sha: "abc1234567".into(),
        ..PullRef::default()
    });
    let page = page_of(&f, Stage::Verify);
    let cards: Vec<(&str, Vec<&str>)> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::File { name, buttons, .. } if name.starts_with("Pull request") => {
                let labels = buttons.iter().map(|i| page.buttons[*i].label.as_str());
                Some((name.as_str(), labels.collect()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        cards,
        vec![
            (
                "Pull request #12 \u{b7} api",
                vec![FIX_HERE, EDIT_PULLS, OPEN_PULL]
            ),
            ("Pull request #13 \u{b7} web", vec![OPEN_PULL]),
        ]
    );
    // The second moving is new commits for the whole review.
    let Some(pull) = f.pulls_mut().get_mut(1) else {
        unreachable!()
    };
    pull.take_remote(Remote {
        at: 1_900,
        head_sha: "fff0000999".into(),
        state: "OPEN".into(),
        ..Remote::default()
    });
    let ctx = RowCtx::default();
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::UpdatePrHead)
    );
}

#[test]
fn new_commits_on_github_wait_for_enter() {
    let mut f = review();
    github(&mut f, "fff0000999", "OPEN");
    let ctx = RowCtx::default();
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::UpdatePrHead)
    );
    let page = page_of(&f, Stage::Verify);
    let (state, buttons) = card(&page);
    assert_eq!(state.tone, CheckTone::Blocker);
    assert!(
        state.text.contains("at fff0000, reviewed at abc1234"),
        "{}",
        state.text
    );
    // Update to new head, the primary, is on the page's bar.
    assert_eq!(buttons, vec![FIX_HERE, EDIT_PULLS, OPEN_PULL]);
    assert_eq!(page.primary, Some(0));
    assert_eq!(page.buttons[0].label, UPDATE_HEAD);
    assert_eq!(
        pull_tip(&f, Stage::Verify, &page, 0).as_deref(),
        Some("Fetch the new commits and review them, from Verify on (Enter)")
    );
    // A done stage's page only says so: Enter there goes elsewhere.
    let page = page_of(&f, Stage::Build);
    assert_eq!(card(&page).1, vec![FIX_HERE, EDIT_PULLS, OPEN_PULL]);
    assert_eq!(
        feature_agents_gate(&f),
        Some((
            Stage::Verify,
            1_900,
            crate::feature_agents::GateKind::NewHead
        ))
    );
}

#[test]
fn a_merged_one_is_to_finish() {
    let mut f = review();
    github(&mut f, "fff0000999", "MERGED");
    let ctx = RowCtx::default();
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::FinishReview)
    );
    let page = page_of(&f, Stage::Verify);
    let (state, buttons) = card(&page);
    assert!(state.text.starts_with("merged on GitHub"), "{}", state.text);
    // Finish, the primary, is on the page's bar.
    assert_eq!(buttons, vec![OPEN_PULL]);
    let primary = page.primary.map(|i| page.buttons[i].label.as_str());
    assert_eq!(primary, Some("Finish\u{2026}"));
    assert_eq!(
        pull_tip(&f, Stage::Verify, &page, page.primary.unwrap_or(0)).as_deref(),
        Some("Finish the review: nothing is posted (Enter)")
    );
    assert_eq!(
        feature_agents_gate(&f),
        Some((
            Stage::Verify,
            1_900,
            crate::feature_agents::GateKind::PrEnded { merged: true }
        ))
    );
}

fn with_checks(f: &mut Feature, states: &[crate::ci::CiState]) {
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    let checks = (states.iter().enumerate())
        .map(|(k, state)| crate::feature::pr::CiCheck {
            name: format!("job {k}"),
            state: *state,
            link: Some(format!("https://github.com/o/api/actions/runs/5/job/{k}")),
        })
        .collect();
    pull.take_remote(Remote {
        at: 1_900,
        head_sha: "abc1234567".into(),
        base_ref: "main".into(),
        state: "OPEN".into(),
        checks,
    });
}

#[test]
fn at_its_head_verify_goes_by_githubs_checks() {
    use crate::ci::CiState::{Failed, Passed, Running};
    let mut f = review();
    let here = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    // Not read at its head: its checks run here, as for a feature.
    assert_eq!(
        page_enter(&f, Stage::Verify, here),
        Some(PanelOutcome::RunChecks)
    );
    let ctx = RowCtx {
        on_head: true,
        ..here
    };
    let checks = PanelOutcome::OpenPr("https://github.com/o/api/pull/12/checks".into());
    assert_eq!(page_enter(&f, Stage::Verify, ctx), Some(checks.clone()));
    assert_eq!(
        ci_line(&f, ctx).as_deref(),
        Some("reading its checks on GitHub")
    );
    with_checks(&mut f, &[Passed, Running]);
    assert_eq!(page_enter(&f, Stage::Verify, ctx), Some(checks));
    assert_eq!(
        verify_line(&f, ctx, None).as_deref(),
        Some("GitHub's checks running: 1 of 2 done")
    );
    with_checks(&mut f, &[Passed, Failed]);
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::RerunCi)
    );
    assert_eq!(
        ci_line(&f, ctx).as_deref(),
        Some("1 of 2 failed on GitHub: job 1")
    );
    // Its checks can run here too, asked: held over its list.
    let mut actions = page_actions(&f, Stage::Verify, ctx);
    with_pull_actions(&mut actions, &f, Stage::Verify, ctx);
    let run = actions
        .iter()
        .position(|(b, _)| b.label == RUN_HERE)
        .unwrap();
    assert_eq!(actions[run].1, PanelOutcome::RunChecks);
    let sections = ci_sections(&f, ctx, &actions);
    assert!(matches!(&sections[0], PageSection::Lines { label, lines }
        if label == "GitHub checks" && lines[1].text == "job 1: failed"));
    assert!(
        matches!(&sections[1], PageSection::Toolbar { groups } if groups[0].1 == vec![run])
    );
    with_checks(&mut f, &[]);
    assert_eq!(page_enter(&f, Stage::Verify, ctx), Some(PanelOutcome::Tick));
}

#[test]
fn only_its_head_clean_counts_as_at_it() {
    let f = review();
    let reading = |head: &str, dirty: &[&str]| {
        Ok(crate::feature::patch::Reading {
            heads: [("api".to_owned(), head.to_owned())].into(),
            dirty: dirty.iter().map(|d| (*d).to_owned()).collect(),
            ..Default::default()
        })
    };
    assert!(on_head(&f, Some(&reading("abc1234567", &[]))));
    assert!(!on_head(&f, Some(&reading("abc1234567", &["api"]))));
    assert!(!on_head(&f, Some(&reading("fff", &[]))));
    assert!(!on_head(&f, None));
    let plain = feature("x", vec![repo("api", true)], 4);
    assert!(!on_head(&plain, Some(&reading("abc1234567", &[]))));
}

#[test]
fn its_findings_are_to_comment_on_github() {
    use crate::feature::review_import::{AiReview, Finding as F};
    let mut f = review();
    assert_eq!(
        crate::feature::agents::run_needs(&f, Stage::AiReview).map(|n| n.template),
        Some("pr-review")
    );
    let finding = |id: &str, state| F {
        id: id.into(),
        title: id.into(),
        state,
        ..F::default()
    };
    f.review = Some(AiReview {
        n: 1,
        posts: true,
        findings: vec![
            finding("r1-1", FindingState::Open),
            finding("r1-2", FindingState::Fix),
        ],
        ..AiReview::default()
    });
    assert!(f.posts_review());
    assert_eq!(f.review.as_ref().unwrap().open(), 1);
    let items = items(&f, Stage::AiReview);
    let open = items.iter().find(|i| i.id() == "r1-1").copied().unwrap();
    let page = CardPage {
        decides: true,
        posts: true,
    };
    let labels: Vec<String> = (item_buttons(open, page).into_iter())
        .map(|(b, _)| b.label)
        .collect();
    assert!(labels.contains(&"Comment".to_owned()), "{labels:?}");
    let wanted = items.iter().find(|i| i.id() == "r1-2").copied().unwrap();
    assert_eq!(item_card(wanted, false, &[], true).state, "to comment");
    // Its own: fixed by an agent, as a feature's.
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    pull.mine = true;
    assert!(!f.posts_review());
}

#[test]
fn its_manual_test_failures_go_with_the_review() {
    use crate::manual_test::Outcome;
    let mut f = review();
    for stage in [Stage::Verify, Stage::AiReview, Stage::CodeReview] {
        f.tick(1, "you", None);
        assert!(f.is_done(stage));
    }
    let ctx = RowCtx {
        criteria: 2,
        manual: feature::manual::Counts {
            passed: 1,
            failed: 1,
            ..Default::default()
        },
        ..RowCtx::default()
    };
    let actions = page_actions(&f, Stage::ManualTest, ctx);
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    assert!(
        !labels.contains(&"Make fixes") && !labels.contains(&"Plan fixes"),
        "{labels:?}"
    );
    let done = actions
        .iter()
        .find(|(b, _)| b.label == "Mark done")
        .unwrap();
    assert_eq!(done.1, PanelOutcome::Manual(Outcome::MarkPassed));
    assert_eq!(done.0.style, ButtonStyle::Primary);
    // Its own goes to Build to fix, as a feature's does.
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    pull.mine = true;
    let labels: Vec<String> = (page_actions(&f, Stage::ManualTest, ctx).into_iter())
        .map(|(b, _)| b.label)
        .collect();
    assert!(labels.contains(&"Make fixes".to_owned()), "{labels:?}");
}

/// `review()` at Ship, its review stages done.
fn at_ship() -> Feature {
    let mut f = review();
    while f.current() != Some(Stage::Ship) {
        f.tick(1, "you", None);
    }
    f
}

#[test]
fn ship_submits_the_review() {
    use crate::feature::pr_submit::{Event, Posted};
    let mut f = at_ship();
    let head = RowCtx {
        on_head: true,
        ..RowCtx::default()
    };
    // Nothing to post: approve it.
    assert_eq!(
        page_enter(&f, Stage::Ship, head),
        Some(PanelOutcome::SubmitReview(Event::Approve))
    );
    assert_eq!(
        submit_line(&f, head).as_deref(),
        Some("nothing to post: approve, or finish")
    );
    let with = RowCtx {
        comments: 2,
        ..head
    };
    assert_eq!(
        page_enter(&f, Stage::Ship, with),
        Some(PanelOutcome::SubmitReview(Event::Comment))
    );
    assert_eq!(submit_line(&f, with).as_deref(), Some("2 comments to post"));
    let mut actions = page_actions(&f, Stage::Ship, with);
    with_pull_actions(&mut actions, &f, Stage::Ship, with);
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    for want in [
        "Comment\u{2026}",
        "Approve\u{2026}",
        "Request changes\u{2026}",
        "Edit text",
    ] {
        assert!(labels.contains(&want), "{want}: {labels:?}");
    }
    assert!(!labels.contains(&"Push\u{2026}"));
    let ctrl = |shift| ModifiersState {
        ctrl: true,
        shift,
        ..ModifiersState::default()
    };
    assert_eq!(
        submit_key(&f, &ctrl(false), Keysym::Return),
        Some(PanelOutcome::SubmitReview(Event::Approve))
    );
    assert_eq!(
        submit_key(&f, &ctrl(true), Keysym::Return),
        Some(PanelOutcome::SubmitReview(Event::RequestChanges))
    );
    assert_eq!(
        submit_key(&f, &ctrl(false), Keysym::e),
        Some(PanelOutcome::EditSummary)
    );
    // Submitted, nothing new: finish.
    f.shipping.reviews.push(Posted {
        repo: "api".into(),
        at: 1,
        event: Event::RequestChanges,
        url: "https://github.com/o/api/pull/12#pullrequestreview-1".into(),
        commit: "abc1234567".into(),
        inline: 2,
        in_body: 1,
        flattened: false,
    });
    assert_eq!(
        page_enter(&f, Stage::Ship, head),
        Some(PanelOutcome::FinishReview)
    );
    let page = page_of(&f, Stage::Ship);
    assert!(
        page.sections
            .iter()
            .any(|s| matches!(s, PageSection::Lines { label, lines }
        if label == "Submitted" && lines[0].text.starts_with("requested changes")))
    );
}

#[test]
fn ones_own_is_pushed_then_finished() {
    let mut f = at_ship();
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    pull.mine = true;
    let moved = RowCtx::default();
    assert_eq!(
        page_enter(&f, Stage::Ship, moved),
        Some(PanelOutcome::PushPr)
    );
    let head = RowCtx {
        on_head: true,
        ..RowCtx::default()
    };
    assert_eq!(
        page_enter(&f, Stage::Ship, head),
        Some(PanelOutcome::FinishReview)
    );
    let mut actions = page_actions(&f, Stage::Ship, head);
    with_pull_actions(&mut actions, &f, Stage::Ship, head);
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    assert!(!labels.contains(&"Approve\u{2026}"), "{labels:?}");
    assert!(labels.contains(&"Comment\u{2026}"));
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    // GitHub takes no approval of one's own.
    assert_eq!(submit_key(&f, &ctrl, Keysym::Return), None);
}

#[test]
fn someone_elses_can_be_fixed_here_and_pushed() {
    let mut f = at_ship();
    let head = RowCtx {
        on_head: true,
        ..RowCtx::default()
    };
    // Commented: the card offers to fix it here.
    let mut actions = page_actions(&f, Stage::Verify, head);
    with_pull_actions(&mut actions, &f, Stage::Verify, head);
    assert!(
        actions
            .iter()
            .any(|(b, o)| b.label == FIX_HERE && *o == PanelOutcome::FixHere)
    );
    assert!(f.posts_review());
    let Some(pull) = f.pulls_mut().first_mut() else {
        unreachable!()
    };
    pull.fix_here = true;
    pull.head_repo = "o/api".into();
    assert!(!f.posts_review());
    let page = page_of(&f, Stage::Verify);
    let (state, buttons) = card(&page);
    assert!(state.text.ends_with("fixing here"), "{}", state.text);
    assert_eq!(buttons, vec![COMMENT_INSTEAD, EDIT_PULLS, OPEN_PULL]);
    // Fixed here: Ship pushes, then finishes without a review.
    assert_eq!(
        page_enter(&f, Stage::Ship, RowCtx::default()),
        Some(PanelOutcome::PushPr)
    );
    assert_eq!(
        page_enter(&f, Stage::Ship, head),
        Some(PanelOutcome::FinishReview)
    );
    assert_eq!(
        submit_line(&f, RowCtx::default()).as_deref(),
        Some("fixes here not on GitHub: push them")
    );
    let mut actions = page_actions(&f, Stage::Ship, head);
    with_pull_actions(&mut actions, &f, Stage::Ship, head);
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    assert!(
        labels.contains(&"Push\u{2026}") && labels.contains(&"Approve\u{2026}"),
        "{labels:?}"
    );
}

fn feature_agents_gate(f: &Feature) -> Option<(Stage, u64, crate::feature_agents::GateKind)> {
    crate::feature_agents::gate_of(f, |_| None)
}
