//! What the phone view may do to a feature (see `phone` and
//! `phone_control`): press the buttons of a stage's page as the desktop
//! shows them, each by what it does, on the card that holds it; pick its
//! agents and models; write what a card's field takes; and send a stage's
//! file back to its agent.
//!
//! Only buttons that act without a desk: none that shows or opens
//! something here. One whose desktop opens a field to write in (a reason,
//! an answer, a comment) asks for the text on the phone, and gets it with
//! the press. What gerdr says while it acts (its toasts) goes back as the
//! answer. A confirm it asks is put to the phone: cancelled here, and
//! pressed again with `confirm` it is answered Go, so no dialog waits on
//! an empty desk.

use serde_json::{Value, json};

use super::*;
use crate::{confirm::Outcome, review_cards::ItemAct, toast::Level};

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
            | O::DecideFinding(..)
            | O::DeleteFindingComment(_)
            | O::DeleteOwnFinding(_)
            | O::OnSuggestion(_, ItemAct::Decide(_) | ItemAct::DeleteComment | ItemAct::DeleteOwn)
            | O::ChangeDecisions
            | O::SkipCheck { .. }
            | O::PickAgent(..)
            | O::CycleMachine(_)
            | O::StopOnMachine(_)
    )
}

/// What the phone asks the user before it presses a button whose desktop
/// opens a field: the field's question. None for one that acts at once.
fn phone_asks(outcome: &PanelOutcome) -> Option<&'static str> {
    use PanelOutcome as O;
    Some(match outcome {
        O::OnSuggestion(_, ItemAct::AskRefuse) => "Why reject it?",
        O::OnSuggestion(_, ItemAct::Answer) => "Your answer",
        O::OnSuggestion(_, ItemAct::Comment) | O::CommentFinding(_) => "Your comment",
        O::AskDismiss(_) => "Why dismiss it?",
        O::AddComment => "What should change?",
        O::PickModel(_) => "The model to start it on (none: the agent's own default)",
        _ => return None,
    })
}

/// A button as the phone has it: what it says and does, how it stands, and
/// what it asks first, if anything, with the text to start from.
#[derive(Clone, Debug)]
struct PhoneButton {
    label: String,
    outcome: PanelOutcome,
    primary: bool,
    off: bool,
    asks: Option<&'static str>,
    value: String,
}

impl PhoneButton {
    fn new(label: &str, outcome: PanelOutcome, primary: bool) -> Self {
        Self {
            label: label.to_owned(),
            asks: phone_asks(&outcome),
            outcome,
            primary,
            off: false,
            value: String::new(),
        }
    }

    /// How the phone names it, to press it: what it does, so two of one
    /// label ("Run again" on each check) are told apart.
    fn id(&self) -> String {
        format!("{:?}", self.outcome)
    }

    fn json(&self) -> Value {
        json!({
            "label": self.label,
            "id": self.id(),
            "primary": self.primary,
            "off": self.off,
            "asks": self.asks,
            "value": self.value,
        })
    }
}

/// A stage's page as the phone shows it: the desktop's, with the buttons
/// the phone may press, by their place in the page's buttons, and those
/// it adds (agent picks, a model, Approve).
struct PhonePage {
    shown: Stage,
    view: chrome::FeatureTabView,
    buttons: Vec<Option<PhoneButton>>,
    extra: Vec<PhoneButton>,
}

impl PhonePage {
    fn at(&self, indices: &[usize]) -> Vec<Value> {
        (indices.iter())
            .filter_map(|i| self.buttons.get(*i)?.as_ref())
            .map(PhoneButton::json)
            .collect()
    }

    /// Every button the phone may press here.
    fn all(&self) -> impl Iterator<Item = &PhoneButton> {
        self.buttons.iter().flatten().chain(&self.extra)
    }

    /// The page's buttons no section holds, for the row under its agents:
    /// Approve first.
    fn loose(&self) -> Vec<Value> {
        use chrome::PageSection as S;
        let mut held = Vec::new();
        for section in &self.view.page.sections {
            match section {
                S::Agent { buttons, .. }
                | S::File { buttons, .. }
                | S::Milestones { buttons, .. } => held.extend(buttons),
                S::Finding(c) => held.extend(c.buttons.iter().chain(&c.comment_buttons)),
                S::Round(r) => held.extend(&r.buttons),
                _ => {}
            }
        }
        let approve = (self.extra.iter()).filter(|b| matches!(b.outcome, PanelOutcome::ApproveDoc(_)));
        let rest = (self.buttons.iter().enumerate())
            .filter(|(i, _)| !held.contains(&i))
            .filter_map(|(_, b)| b.as_ref());
        approve.chain(rest).map(PhoneButton::json).collect()
    }

    /// The picks and model button of the agent card of `stage`.
    fn agent_extra(&self, stage: Stage) -> (Vec<Value>, Option<Value>) {
        let picks = (self.extra.iter())
            .filter(|b| matches!(&b.outcome, PanelOutcome::PickAgent(s, _) if *s == stage))
            .map(PhoneButton::json)
            .collect();
        let model = (self.extra.iter())
            .find(|b| b.outcome == PanelOutcome::PickModel(stage))
            .map(PhoneButton::json);
        (picks, model)
    }

    /// A section as the phone shows it, with its buttons.
    fn section(&self, section: &chrome::PageSection) -> Option<Value> {
        use chrome::PageSection as S;
        let mut value = section_json(section)?;
        match section {
            S::Agent { stage, buttons, .. } => {
                let (picks, model) = self.agent_extra(*stage);
                value["buttons"] = self.at(buttons).into();
                value["picks"] = picks.into();
                value["model"] = model.into();
            }
            S::File { buttons, .. } | S::Milestones { buttons, .. } => {
                value["buttons"] = self.at(buttons).into();
            }
            S::Finding(c) => {
                value["buttons"] = self.at(&c.buttons).into();
                value["commentButtons"] = self.at(&c.comment_buttons).into();
            }
            S::Round(r) => value["buttons"] = self.at(&r.buttons).into(),
            S::MilestoneMode { .. } => {
                let picks: Vec<Value> = (self.extra.iter())
                    .filter(|b| matches!(b.outcome, PanelOutcome::FreshMilestones(_)))
                    .map(PhoneButton::json)
                    .collect();
                value["buttons"] = picks.into();
            }
            _ => {}
        }
        Some(value)
    }
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

/// A new feature the phone asked for, while its worktrees are made.
pub(crate) struct PhoneNew {
    /// Where the answer goes: the phone waits for it.
    pub answer: std::sync::mpsc::Sender<Value>,
    /// What the user said they want of it, for Discover's agent.
    pub idea: String,
    /// The agent to run Discover on, if not the one it gets anyway.
    pub agent: Option<String>,
}

/// What the phone asked to start: the repositories it named of `root`, as
/// `layout::create` takes them. A single repository is always in, a root
/// of several needs one named.
fn phone_touched(root: &layout::Root, named: &[String]) -> Result<Vec<String>, String> {
    if !root.multi {
        return Ok(Vec::new());
    }
    if named.is_empty() {
        return Err("Pick a repository it changes".into());
    }
    match named.iter().find(|n| !root.repos.iter().any(|r| &r.name == *n)) {
        Some(name) => Err(format!("{} has no repository {name}", root.path.display())),
        None => Ok(named.to_vec()),
    }
}

/// Feature `slug`'s root as the new feature dialog finds it from `from`,
/// with each base origin says where the project's settings set none, and
/// the repositories the phone named of it.
fn phone_root(
    from: &Path,
    search: &(Vec<PathBuf>, Vec<String>, Vec<(String, String)>),
    others: &[PathBuf],
    named: &[String],
) -> Result<(layout::Root, Vec<String>), String> {
    let (dirs, ignore, bases) = search;
    let mut root = layout::find_project_root(from, dirs, others, ignore, bases)?;
    for found in &mut root.repos {
        if bases.iter().any(|(name, _)| *name == found.name) {
            continue;
        }
        // Offline, what git has here stands, as in the dialog.
        let Ok(Some(branch)) = crate::git_ops::remote_default(&found.main) else {
            continue;
        };
        if layout::branches_of(&found.main).is_ok_and(|all| all.contains(&branch)) {
            found.base = branch;
        }
    }
    let touched = phone_touched(&root, named)?;
    Ok((root, touched))
}

/// What the phone asks a feature: its page's buttons, or a press.
#[derive(Debug, PartialEq)]
pub(crate) enum PhoneAct {
    /// Press button `id` (see `PhoneButton::id`) of the page of stage
    /// `stage` (its place in `Stage::ALL`; none: the one the desktop
    /// shows), with the text it asked for, if it asks.
    Press {
        stage: Option<usize>,
        id: String,
        text: Option<String>,
    },
    /// Send the current stage's file back to its agent, with these words.
    SendBack(String),
}

impl Gerdr {
    /// What a new feature of each of `projects` may start on: the agents
    /// Discover may run on, and the one it runs unless another is picked.
    pub(crate) fn phone_new_offer(&self, projects: &[String]) -> Value {
        let agents: serde_json::Map<String, Value> = (projects.iter())
            .map(|id| {
                let workflow = self.projects.project(id).map(|p| &p.workflow);
                let (names, default) =
                    crate::feature_agents::new_discover_agents(workflow, &self.launcher.cli_agents);
                (id.clone(), json!({ "agents": names, "default": default }))
            })
            .collect();
        json!({ "agents": agents })
    }

    /// Start feature `title` of `project` for the phone, on the
    /// repositories it `named`: its worktrees made off the thread, then
    /// its Discover agent started (see `phone_created`). The phone gets
    /// its answer once that is done, or at once if it can't be.
    pub(crate) fn phone_create(
        &mut self,
        project: String,
        title: String,
        named: Vec<String>,
        idea: String,
        agent: Option<String>,
        answer: std::sync::mpsc::Sender<Value>,
    ) {
        let refuse = |why: String| {
            // The phone may have given up on it.
            let _ = answer.send(json!({ "error": why }));
        };
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return refuse("Give it a title".into());
        }
        let Some(p) = self.projects.project(&project) else {
            return refuse("gerdr has no such project".into());
        };
        let Some(from) = p.workspaces.first().map(|w| w.dir.clone()) else {
            return refuse(format!("Add a workspace with a folder to {} first", p.name));
        };
        let search = root_search(p);
        let agent = agent.filter(|a| !a.trim().is_empty());
        if let Some(name) = &agent {
            let workflow = Some(&p.workflow);
            let (names, _) =
                crate::feature_agents::new_discover_agents(workflow, &self.launcher.cli_agents);
            if !names.contains(name) {
                return refuse(format!("{name} can't run Discover here"));
            }
        }
        let others = self.other_projects_dirs(&project);
        let slug = self.new_feature_slug(&project, &title);
        let key = (project.clone(), slug.clone());
        if self.features.phone_new.contains_key(&key) {
            return refuse(format!("\u{201c}{title}\u{201d} is being made already"));
        }
        let asked = PhoneNew {
            answer,
            idea,
            agent,
        };
        self.features.phone_new.insert(key, asked);
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        self.feature_work(Some(busy), move |_| {
            let (root, result) = match phone_root(&from, &search, &others, &named) {
                Ok((root, touched)) => {
                    let result = layout::create_spanning(&root, &slug, &touched, &[]);
                    (root, result)
                }
                Err(why) => {
                    let root = layout::Root {
                        path: from,
                        multi: false,
                        repos: Vec::new(),
                    };
                    (root, Err(why))
                }
            };
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            }
        });
    }

    /// The worktrees of a feature the phone asked for were made, or not:
    /// keep it as the dialog's are, start its Discover agent on what the
    /// phone gave, and answer the phone with what gerdr said.
    pub(super) fn phone_created(
        &mut self,
        asked: PhoneNew,
        project: String,
        title: String,
        slug: String,
        root: layout::Root,
        result: Result<(layout::Created, Vec<(feature::Link, layout::Created)>), String>,
    ) {
        let PhoneNew {
            answer,
            idea,
            agent,
        } = asked;
        let key = (project.clone(), slug.clone());
        let (p, s) = key.clone();
        let mut said = self.phone_said(false, move |g| {
            g.feature_created(project, title, slug, root, result);
            if g.feature(&p, &s).is_none() {
                return;
            }
            let idea = idea.trim();
            if !idea.is_empty() {
                let path = store::dir(None, &p, &s).join(feature::artifacts::IDEA);
                if let Err(err) = std::fs::write(&path, format!("{idea}\n")) {
                    tracing::warn!("writing {}: {err}", path.display());
                    g.show_error(format!("Couldn't keep what it is for: {err}"));
                }
            }
            if let Some(name) = agent {
                g.pick_stage_agent(&p, &s, Stage::Discover, &name);
            }
            g.start_stage_agent(&p, &s, Stage::Discover, false);
        });
        if self.feature(&key.0, &key.1).is_some() {
            said["slug"] = key.1.into();
        } else {
            // Why it wasn't made is the error gerdr said.
            let why = (said["said"].as_array().into_iter().flatten())
                .filter(|l| l["level"] == "error")
                .find_map(|l| l["text"].as_str().map(str::to_owned));
            said["error"] = why.unwrap_or_else(|| "Couldn't create it".into()).into();
        }
        // The phone may have given up on it.
        let _ = answer.send(said);
    }

    /// The page of `stage` (by its place in `Stage::ALL`; none: the one
    /// the desktop shows) of feature `slug`, as the phone shows it.
    fn phone_page(&self, project: &str, slug: &str, stage: Option<usize>) -> Option<PhonePage> {
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
        let ctx = self.row_ctx(feature);
        let actions = self.page_button_actions(feature, shown, ctx);
        let buttons: Vec<Option<PhoneButton>> = (actions.into_iter().enumerate())
            .map(|(i, (b, outcome))| {
                let mut button = PhoneButton::new(
                    &b.label,
                    outcome,
                    b.style == ButtonStyle::Primary || view.page.primary == Some(i),
                );
                button.off = b.style == ButtonStyle::Off;
                // A question is accepted by answering it: the desk opens its field.
                if let PanelOutcome::OnSuggestion(id, ItemAct::Decide(_)) = &button.outcome
                    && self
                        .suggestion_of(project, slug, id)
                        .is_some_and(|s| s.unanswered())
                {
                    button.asks = Some("Your answer");
                }
                (phone_may(&button.outcome) || button.asks.is_some()).then_some(button)
            })
            .collect();
        let mut extra = Vec::new();
        for section in &view.page.sections {
            // Build's choice is two segments to press on the desktop.
            if let chrome::PageSection::MilestoneMode { fresh } = section {
                for (label, to) in [("One agent", false), ("Fresh each milestone", true)] {
                    let mut pick = PhoneButton::new(label, PanelOutcome::FreshMilestones(to), *fresh == to);
                    pick.off = *fresh == to;
                    extra.push(pick);
                }
                continue;
            }
            let chrome::PageSection::Agent {
                stage, picks, model, ..
            } = section
            else {
                continue;
            };
            for chip in picks {
                let mut pick = PhoneButton::new(
                    &chip.name,
                    PanelOutcome::PickAgent(*stage, chip.name.clone()),
                    chip.lit,
                );
                // The one it runs already is no pick to make.
                pick.off = chip.off || chip.lit;
                extra.push(pick);
            }
            if let Some(label) = model {
                let mut pick = PhoneButton::new(label, PanelOutcome::PickModel(*stage), false);
                let slot = self.model_slot_of(project, slug, *stage);
                pick.value = slot.and_then(|s| s.picked).unwrap_or_default();
                extra.push(pick);
            }
        }
        // The desktop approves a file in its review tab; the phone, here.
        if let Some(stage) = doc_waiting(feature).filter(|s| *s == shown) {
            let approve = PanelOutcome::ApproveDoc(stage);
            let has = |b: &Option<PhoneButton>| b.as_ref().is_some_and(|b| b.outcome == approve);
            if !buttons.iter().any(has) {
                extra.push(PhoneButton::new("Approve", approve, true));
            }
        }
        Some(PhonePage {
            shown,
            view,
            buttons,
            extra,
        })
    }

    /// What the phone may do to feature `slug`, for its page, with its
    /// rail and the page of `stage` (by its place in `Stage::ALL`), the
    /// one the desktop shows when none is asked.
    pub(crate) fn phone_offer(&self, project: &str, slug: &str, stage: Option<usize>) -> Value {
        let (Some(feature), Some(page)) = (
            self.feature(project, slug),
            self.phone_page(project, slug, stage),
        ) else {
            return json!({ "error": "gerdr has no such feature" });
        };
        let elsewhere = self.feature_writer(project, slug).map(str::to_owned);
        let send_back = doc_waiting(feature) == Some(page.shown);
        let view = &page.view;
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
            "back": view.back.as_ref().map(|b| json!({ "from": b.from, "to": b.to })),
        });
        let shown = page.shown;
        let p = &view.page;
        let sections: Vec<Value> = (p.sections.iter())
            .filter_map(|s| page.section(s))
            .collect();
        let page_json = json!({
            "stage": Stage::ALL.iter().position(|s| *s == shown),
            "label": shown.label(),
            "own": shown == own_page(feature),
            "title": p.title,
            "about": p.about,
            "status": p.status.as_ref().map(check_json),
            "sections": sections,
        });
        json!({
            "buttons": page.loose(),
            "sendBack": send_back && elsewhere.is_none(),
            "elsewhere": elsewhere,
            "rail": rail,
            "page": page_json,
        })
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
        // Its answer would be taken for the one this asks.
        if self.overlays.confirm.is_some() {
            return json!({ "error": "A dialog is open on the desktop: answer it there first" });
        }
        let (project, slug) = (project.to_owned(), slug.to_owned());
        match act {
            PhoneAct::Press { stage, id, text } => {
                let Some(page) = self.phone_page(&project, &slug, stage) else {
                    return json!({ "error": "gerdr has no such feature" });
                };
                let Some(button) = page.all().find(|b| b.id() == id && !b.off).cloned() else {
                    return json!({ "error": "That button isn't on its page any more" });
                };
                let shown = page.shown;
                match (button.asks, text) {
                    (None, _) => self.phone_said(confirm, move |g| {
                        g.apply_feature_outcome(project, slug, button.outcome);
                    }),
                    (Some(_), Some(text))
                        if !text.trim().is_empty()
                            || matches!(button.outcome, PanelOutcome::PickModel(_)) =>
                    {
                        self.phone_said(confirm, move |g| {
                            g.phone_wrote(&project, &slug, shown, button.outcome, text.trim());
                        })
                    }
                    (Some(asks), _) => json!({ "error": format!("{asks}: write it first") }),
                }
            }
            PhoneAct::SendBack(text) => {
                let Some(stage) = self.feature(&project, &slug).and_then(Feature::current) else {
                    return json!({ "error": "gerdr has no such feature" });
                };
                self.phone_said(confirm, move |g| {
                    g.send_back_stage(&project, &slug, stage, text);
                })
            }
        }
    }

    /// Do what the field `outcome` opens on the desktop does once `text` is
    /// written in it, on page `shown` of feature `slug`.
    fn phone_wrote(
        &mut self,
        project: &str,
        slug: &str,
        shown: Stage,
        outcome: PanelOutcome,
        text: &str,
    ) {
        use PanelOutcome as O;
        match outcome {
            O::OnSuggestion(id, ItemAct::AskRefuse) => {
                self.reject_suggestion(project, slug, &id, text);
            }
            O::OnSuggestion(id, ItemAct::Comment) => {
                self.keep_suggestion_note(project, slug, &id, text, false);
            }
            O::OnSuggestion(id, ItemAct::Answer | ItemAct::Decide(_)) => {
                self.keep_suggestion_note(project, slug, &id, text, true);
            }
            O::AskDismiss(id) => {
                let why = Decision::Dismiss(text.to_owned());
                self.decide_finding(project, slug, &id, why);
                self.feature_findings_changed(project, slug);
            }
            O::CommentFinding(id) => {
                self.note_finding(project, slug, &id, text);
            }
            O::AddComment => match self.feature(project, slug).and_then(Feature::current) {
                Some(Stage::PlanReview) => {
                    self.add_suggestion(project, slug, text);
                }
                Some(Stage::ManualTest) => {
                    self.add_manual_comment(project, slug, text);
                }
                _ => self.add_finding_comment(project, slug, text),
            },
            O::PickModel(stage) => {
                let Some(slot) = self.model_slot_of(project, slug, stage) else {
                    return self.show_warning(format!(
                        "gerdr can't start {}'s agent on a model",
                        stage.label()
                    ));
                };
                let model = (!text.is_empty()).then(|| text.to_owned());
                self.set_stage_model(project, slug, stage, &slot.agent, model);
            }
            other => tracing::warn!("the phone wrote for {other:?} on {}", shown.label()),
        }
    }

    /// Run `act`, and say what gerdr said while it did: its toasts, and
    /// the confirm it asked, answered Go if `confirm`, else cancelled and
    /// put to the phone.
    fn phone_said(&mut self, confirm: bool, act: impl FnOnce(&mut Self)) -> Value {
        let last = self.toasts.iter().map(|t| t.id).max().unwrap_or(0);
        let asking = self.overlays.confirm.is_some();
        act(self);
        let mut asks = Value::Null;
        // One open before was the desk's own, not this act's.
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
        assert!(!phone_may(&PanelOutcome::EditCommands(Default::default())));
        assert!(phone_may(&PanelOutcome::PickAgent(Stage::Plan, "Codex".into())));
        assert!(phone_may(&PanelOutcome::FreshMilestones(true)));
        let accept = PanelOutcome::OnSuggestion("s1-1".into(), ItemAct::Decide(Decision::Fix));
        assert!(phone_may(&accept));
        assert!(!phone_may(&PanelOutcome::OnSuggestion("s1-1".into(), ItemAct::OpenCode)));
    }

    #[test]
    fn a_field_the_desk_opens_is_asked_on_the_phone() {
        let reject = PanelOutcome::OnSuggestion("s1-1".into(), ItemAct::AskRefuse);
        assert!(phone_asks(&reject).is_some());
        assert!(phone_asks(&PanelOutcome::AskDismiss("r1-1".into())).is_some());
        assert!(phone_asks(&PanelOutcome::PickModel(Stage::Build)).is_some());
        assert!(phone_asks(&PanelOutcome::RunChecks).is_none());
        assert!(!phone_may(&reject));
    }

    #[test]
    fn buttons_of_one_label_are_told_apart_by_what_they_do() {
        let first = PhoneButton::new("Run again", PanelOutcome::RerunCheck(0), false);
        let second = PhoneButton::new("Run again", PanelOutcome::RerunCheck(1), false);
        assert_ne!(first.id(), second.id());
        assert_eq!(second.json()["id"], "RerunCheck(1)");
    }

    #[test]
    fn a_new_feature_of_several_repositories_names_those_it_changes() {
        let repo = |name: &str| layout::Found {
            name: name.into(),
            main: PathBuf::from(format!("/w/{name}")),
            base: "main".into(),
        };
        let single = layout::Root {
            path: "/w/gerdr".into(),
            multi: false,
            repos: vec![repo("gerdr")],
        };
        assert_eq!(phone_touched(&single, &[]), Ok(Vec::new()));
        let multi = layout::Root {
            path: "/w".into(),
            multi: true,
            repos: vec![repo("api"), repo("web")],
        };
        assert!(phone_touched(&multi, &[]).is_err());
        assert!(phone_touched(&multi, &["nope".into()]).is_err());
        assert_eq!(phone_touched(&multi, &["web".into()]), Ok(vec!["web".into()]));
    }
}
