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
