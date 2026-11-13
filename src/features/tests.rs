use super::*;
use crate::feature::{
    Mark,
    fix_round::{CommentFix, VerifyFix},
    ship::ShipCheck,
};

mod ai_review;
mod manual_tests;
mod new_feature;
mod shipping;
mod stage_agents;
mod stage_pages;
mod verify_review;

pub(crate) fn repo(name: &str, touched: bool) -> Repo {
    Repo {
        name: name.into(),
        main: PathBuf::from(format!("/work/{name}")),
        base: "main".into(),
        base_commit: "abc123".into(),
        worktree: PathBuf::from(format!("/work/.features/bulk/{name}")),
        touched,
        branch: touched.then(|| "feature/bulk".into()),
        project: None,
    }
}

/// A feature over `repos`, its first `done` stages marked.
pub(crate) fn feature(slug: &str, repos: Vec<Repo>, done: usize) -> Feature {
    Feature {
        version: feature::VERSION,
        kind: Default::default(),
        writer: None,
        slug: slug.into(),
        title: format!("Feature {slug}"),
        project: "acme".into(),
        created: 1_000,
        root: "/work".into(),
        dir: format!("/work/.features/{slug}").into(),
        multi: repos.len() > 1,
        repos,
        links: Vec::new(),
        status: Status::Open,
        run_counts: BTreeMap::new(),
        context: Vec::new(),
        removed: Vec::new(),
        marks: Stage::ALL
            .iter()
            .take(done)
            .map(|s| {
                let mark = Mark {
                    at: 2_000,
                    by: "you".into(),
                    note: String::new(),
                    patch: None,
                };
                (*s, mark)
            })
            .collect(),
        history: Vec::new(),
        shipping: Default::default(),
        ship_plan: Default::default(),
        runs: Default::default(),
        own_agents: Vec::new(),
        stage_agents: Default::default(),
        stage_machines: Default::default(),
        setups_elsewhere: Vec::new(),
        came_from: None,
        epoch: 0,
        stage_models: Default::default(),
        fresh_milestones: false,
        setup: None,
        checks: Default::default(),
        skip_checks: Default::default(),
        verify: None,
        sandbox: None,
        sandbox_picks: Default::default(),
        review: None,
        plan_review: None,
        viewed: Default::default(),
        manual: None,
        fix_round: None,
        last_fix_round: None,
        verify_fixes: Vec::new(),
        fix_asked: Default::default(),
        comment_fixes: Vec::new(),
        ran_once: Default::default(),
        fix_rounds: 0,
        past_rounds: Vec::new(),
        drafts: Vec::new(),
        carry: None,
        unverified: None,
        back_from: None,
    }
}

fn key(sym: Keysym) -> (ModifiersState, Keysym) {
    (ModifiersState::default(), sym)
}

fn ctrl(sym: Keysym) -> (ModifiersState, Keysym) {
    let mods = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    (mods, sym)
}

fn press(
    panel: &mut FeaturePanel,
    f: &Feature,
    (mods, sym): (ModifiersState, Keysym),
) -> Option<PanelOutcome> {
    let slugs = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    panel.key(&mods, sym, f, &slugs, RowCtx::default(), &[], &[], &[])
}

fn root(names: &[&str]) -> Root {
    Root {
        path: "/work".into(),
        multi: names.len() > 1,
        repos: names
            .iter()
            .map(|n| Found {
                name: (*n).into(),
                main: PathBuf::from(format!("/work/{n}")),
                base: "main".into(),
            })
            .collect(),
    }
}

fn type_into(d: &mut NewFeatureDialog, text: &str) {
    for c in text.chars() {
        let sym = Keysym::from_char(c);
        d.key(&ModifiersState::default(), sym, sym, 8);
    }
}

fn dkey(
    d: &mut NewFeatureDialog,
    (mods, sym): (ModifiersState, Keysym),
) -> Option<DialogOutcome> {
    d.key(&mods, sym, sym, 8)
}

fn alt(sym: Keysym) -> (ModifiersState, Keysym) {
    let mods = ModifiersState {
        alt: true,
        ..Default::default()
    };
    (mods, sym)
}

fn alt_shift(sym: Keysym) -> (ModifiersState, Keysym) {
    let mods = ModifiersState {
        alt: true,
        shift: true,
        ..Default::default()
    };
    (mods, sym)
}

fn base_of(d: &NewFeatureDialog, name: &str) -> String {
    let key = (None, name.to_owned());
    d.repo_found(&key).unwrap().base.clone()
}

/// A dialog over `names`, each with these branches read.
fn with_branches(names: &[&str], branches: &[&str]) -> NewFeatureDialog {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.found(Ok(root(names)));
    for (key, _) in d.unlooked() {
        let all = branches.iter().map(|b| (*b).to_owned()).collect();
        d.base_branches(&key, Ok(all));
    }
    d
}

/// Another project of the store, as `find_others` lists it.
fn other(project: &str, name: &str) -> Other {
    Other {
        project: project.into(),
        name: name.into(),
        root: Finding::Looking,
        linked: false,
        touched: Vec::new(),
    }
}

fn ctrl_shift(sym: Keysym) -> (ModifiersState, Keysym) {
    let mods = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    (mods, sym)
}

fn agent_run(pane: Option<&str>, ready: Option<u64>) -> feature::agents::StageRun {
    feature::agents::StageRun {
        n: 1,
        agent: "Claude Code".into(),
        pane: pane.map(str::to_owned),
        ready,
        ..feature::agents::StageRun::default()
    }
}

/// A run of `base` whose agent waits for its prompt, its tab `pane`.
fn held_run(pane: Option<&str>) -> feature::agents::StageRun {
    feature::agents::StageRun {
        base: "claude".into(),
        held: true,
        ..agent_run(pane, None)
    }
}

/// A review page's summary as one line: "Findings · 2 to decide", with
/// its notes.
fn summary_line(sections: &[PageSection]) -> Option<(String, Vec<CheckLine>)> {
    sections.iter().find_map(|s| match s {
        PageSection::Review(r) => {
            let label = match r.words.is_empty() {
                true => r.label.clone(),
                false => format!("{} \u{b7} {}", r.label, r.words),
            };
            Some((label, r.notes.clone()))
        }
        _ => None,
    })
}

fn page_outcomes(f: &Feature, stage: Stage) -> Vec<PanelOutcome> {
    let actions = page_actions(f, stage, RowCtx::default());
    actions.into_iter().map(|(_, o)| o).collect()
}

/// A feature on Verify with a passed and a failed check, its run ended.
fn verified(running: bool) -> Feature {
    use feature::verify::{CheckRun, Outcome, VerifyRun};
    let mut f = feature("v", vec![repo("api", true)], 4);
    let row = |command: &str, exit: i32| CheckRun {
        repo: "api".into(),
        command: command.into(),
        pane: Some(format!("p-{command}")),
        run: 2,
        outcome: Some(Outcome {
            exit: Some(exit),
            tail: "first\nboom\n".into(),
            ..Outcome::default()
        }),
        ..CheckRun::default()
    };
    let mut checks = vec![row("lint", 0), row("test", 1)];
    if running {
        checks[1].outcome = None;
    }
    f.verify = Some(VerifyRun {
        n: 2,
        opened: true,
        finished: (!running).then_some(1),
        checks,
        ..VerifyRun::default()
    });
    f
}

/// A comment in `path` of the kind Code review's round took, id `id`.
fn sent(id: &str, path: &str, text: &str) -> Comment {
    Comment {
        path: path.into(),
        text: text.into(),
        lines: vec![crate::review::Quoted {
            number: 3,
            ..Default::default()
        }],
        flag: Some(crate::review::Flag {
            id: id.into(),
            state: FindingState::Sent,
            yours: true,
            sent: true,
            ..Default::default()
        }),
        ..Comment::default()
    }
}

pub(crate) fn in_round(mut f: Feature, stage: Stage, phase: FixPhase) -> Feature {
    f.fix_round = Some(FixRound {
        n: 1,
        what: "Verify failures".into(),
        input: "/d/a.md".into(),
        stage,
        phase,
        sent_back: 0,
        asked: 1_000,
        ready: (phase == FixPhase::Ready).then_some(1_100),
        stalled: None,
        give_back: Default::default(),
        items: Vec::new(),
        done: 0,
        ended: None,
        trail: Default::default(),
    });
    f
}

fn probe(ship: Option<ShipCheck>) -> Probe {
    Probe {
        reading: Ok(Reading {
            patch: "p1".into(),
            ..Reading::default()
        }),
        guards: BTreeMap::new(),
        models: BTreeMap::new(),
        prs: None,
        bases: BTreeMap::new(),
        resolution: None,
        ship,
        merged: false,
        stats: BTreeMap::new(),
        rounds: BTreeMap::new(),
        docs: feature::page::Docs::default(),
    }
}

/// A check finding `repos` to ship through origin, each with commits.
fn check(blockers: &[&str], repos: &[&str], notes: &[&str]) -> Option<ShipCheck> {
    let repo = |key: &&str| ship::RepoCheck {
        key: (*key).to_owned(),
        base: "main".to_owned(),
        tree: PathBuf::from("/m"),
        commits: 1,
        route: Route::Pr,
        no_origin: None,
        no_merge: None,
        merged: false,
        head: None,
        pushed: None,
    };
    Some(ShipCheck {
        blockers: blockers.iter().map(|b| (*b).to_owned()).collect(),
        repos: repos.iter().map(repo).collect(),
        notes: notes.iter().map(|n| (*n).to_owned()).collect(),
        unread: false,
    })
}

fn cards_of(view: &FeatureTabView) -> Vec<&FindingCard> {
    (view.page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect()
}

/// A review read and a run of it, with the code changed since: AI review
/// is a stage to come.
fn sent_back(f: &mut Feature) {
    f.marks.retain(|stage, _| *stage < Stage::Build);
    assert_eq!(stage_mark(f, Stage::AiReview), StageMark::Later);
}

fn labels_of(view: &FeatureTabView, card: &FindingCard) -> Vec<String> {
    (card.buttons.iter())
        .map(|i| view.page.buttons[*i].label.clone())
        .collect()
}

/// An AI review page at run 6 after five earlier runs: round 9 open and
/// 8 over from run 6, 7 from run 5, 4 from run 3, and 2 from run 1 by its
/// findings' ids.
pub(crate) fn history_feature() -> Feature {
    use FindingState as S;
    let mut f = carded(&[S::Open]);
    let review = f.review.as_mut().unwrap();
    review.rounds = (1..=5)
        .map(|n| ReviewRound {
            n,
            findings: Vec::new(),
        })
        .collect();
    review.n = 6;
    let round = |n: u32, run: Option<u32>, ended: Option<u64>| {
        let mut r = manual_round(n, ended, None);
        r.stage = Stage::AiReview;
        r.trail.asked.clear();
        r.trail.run = run;
        r
    };
    // Round 2 kept no run: its findings' ids say run 1.
    let mut old = round(2, None, Some(1_500));
    old.give_back = crate::feature::fix_round::GiveBack::Findings {
        ids: vec!["r1-3".into()],
        comments: None,
    };
    f.past_rounds = vec![
        old,
        round(4, Some(3), Some(1_600)),
        round(7, Some(5), Some(1_700)),
    ];
    f.last_fix_round = Some(round(8, Some(6), Some(1_800)));
    f.fix_round = Some(round(9, Some(6), None));
    for n in [2, 4, 7, 8, 9] {
        f.fix_asked.insert(n, Stage::AiReview);
    }
    f.fix_rounds = 9;
    f
}

/// `history_feature`'s AI review page, for the golden fixture.
pub(crate) fn history_view() -> FeatureTabView {
    view_of(&history_feature(), &PanelState::default())
}

fn note(path: &str, text: &str) -> Comment {
    Comment {
        root: "/work/api".into(),
        path: path.into(),
        lines: vec![crate::review::Quoted {
            number: 12,
            ..crate::review::Quoted::default()
        }],
        text: text.into(),
        flag: None,
    }
}

fn texts(lines: &[CheckLine]) -> Vec<(&str, CheckTone)> {
    lines.iter().map(|l| (l.text.as_str(), l.tone)).collect()
}

/// What `actions` say on their buttons, with what they do.
fn labels(actions: &[(PageButton, PanelOutcome)]) -> Vec<(&str, PanelOutcome)> {
    (actions.iter())
        .map(|(b, o)| (b.label.as_str(), o.clone()))
        .collect()
}

fn verify_fix(round: u32, run: u32) -> VerifyFix {
    VerifyFix {
        round,
        run,
        at: 9_000,
        agent: "Claude Code".into(),
        failures: vec![
            "api: `test` exited with 1".into(),
            "web: `lint` exited with 2".into(),
        ],
        why: vec!["FAIL auth".into()],
        ..VerifyFix::default()
    }
}

/// `stages`' files are there.
fn wrote(stages: &[Stage]) -> RowCtx {
    RowCtx {
        written: Stage::ALL.map(|s| stages.contains(&s)),
        ..RowCtx::default()
    }
}

fn wrote_all() -> RowCtx {
    RowCtx {
        written: [true; Stage::ALL.len()],
        ..RowCtx::default()
    }
}

fn planned(command: &str, skipped: bool, own: bool) -> PlannedCheck {
    PlannedCheck {
        repo: "api".into(),
        command: command.into(),
        skipped,
        own,
    }
}

/// A file card as `stage_page` makes it.
fn file(name: &str, state: CheckLine, buttons: &[usize]) -> PageSection {
    PageSection::File {
        name: name.to_owned(),
        state,
        buttons: buttons.to_vec(),
    }
}

fn manual_round(n: u32, ended: Option<u64>, from: Option<&str>) -> FixRound {
    let mut trail = crate::feature::fix_round::Trail {
        went: Some(1_200),
        agent: "Claude Code".into(),
        asked: vec![crate::feature::fix_round::Asked {
            id: "AC-1".into(),
            title: "Save keeps the title".into(),
            note: "the title went back".into(),
            shots: 1,
        }],
        ..Default::default()
    };
    if let Some(from) = from {
        trail.from = BTreeMap::from([("api".to_owned(), from.to_owned())]);
    }
    FixRound {
        n,
        what: "1 failure".into(),
        input: "/d/a.md".into(),
        stage: Stage::ManualTest,
        phase: FixPhase::Fixing,
        sent_back: 0,
        asked: 1_000,
        ready: Some(1_100),
        stalled: None,
        give_back: Default::default(),
        items: vec!["Keep the title".into()],
        done: u32::from(ended.is_some()),
        ended,
        trail,
    }
}

/// `f` with its AI review on its page and its findings placed in a file.
fn carded(states: &[FindingState]) -> Feature {
    let mut f = reviewing(states, true);
    for (i, finding) in (f.review.as_mut().unwrap().findings.iter_mut()).enumerate() {
        finding.file = format!("src/{i}.rs");
        finding.title = format!("Title {i}");
        finding.placed = true;
    }
    f
}

fn view_of(f: &Feature, state: &PanelState) -> FeatureTabView {
    let panel = FeaturePanel {
        stage: Some(Stage::AiReview),
        picked_at: f.current(),
        ..FeaturePanel::default()
    };
    tab_view(&panel, f, &["r".to_owned()], state, 2_060)
}

fn reviewing(states: &[FindingState], ready: bool) -> Feature {
    use crate::feature::{
        agents::StageRun,
        review_import::{AiReview, Finding},
    };
    let mut f = feature("r", vec![repo("api", true)], 5);
    f.runs.insert(
        Stage::AiReview,
        StageRun {
            n: 1,
            agent: "Codex".into(),
            pane: Some("p1".into()),
            started: 2_000,
            ready: ready.then_some(2_050),
            ..StageRun::default()
        },
    );
    let findings = states
        .iter()
        .enumerate()
        .map(|(i, state)| Finding {
            id: format!("r1-{}", i + 1),
            state: *state,
            ..Finding::default()
        })
        .collect();
    f.review = Some(AiReview {
        n: 1,
        findings,
        ..AiReview::default()
    });
    f
}
