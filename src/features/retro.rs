//! A shipped feature's retro: an agent of its own reads what gerdr kept of
//! it and proposes, shaped as a brief, what would stop its problems
//! happening to the next feature. Reviewed as a stage's file is; approved,
//! it becomes a new feature with that brief, at Plan.

use super::*;
use crate::projects::Retro;

/// The retro, in the feature's data folder.
pub(crate) const FILE: &str = "retro.md";
/// The comments it was last sent back with, for its agent.
const SENT: &str = "retro-comments.md";
/// What its review says while its agent hasn't written it.
pub(crate) const NONE_YET: &str = "No retro yet: press Retro on the feature's Ship page";
/// Why a retro made into a feature can't change.
const MADE: &str = "This retro is a feature already";

/// Where the retro of `feature` stands, as its review's header says it.
pub(crate) fn state(feature: Option<&Feature>) -> String {
    match feature.and_then(|f| f.retro_into.as_deref()) {
        Some(slug) => format!("made into feature {slug}"),
        None => "Approve makes it a new feature".into(),
    }
}

/// Why the retro of `feature` can't be edited now. None: it can.
pub(crate) fn locked(feature: Option<&Feature>) -> Option<&'static str> {
    feature.and_then(|f| f.retro_into.as_ref()).map(|_| MADE)
}

/// The title of the feature the retro of feature `title` becomes.
fn retro_title(title: &str) -> String {
    format!("Retro: {title}")
}

/// Whether `feature`'s Ship page offers its retro: done, here, not made
/// into a feature yet, and its project's `retro` not off.
pub(super) fn offers_retro(feature: &Feature, ctx: RowCtx) -> bool {
    ctx.retro_offered
        && feature.status == Status::Done
        && feature.writer.is_none()
        && feature.retro_into.is_none()
}

impl Gerdr {
    /// When `feature`'s retro runs, by its project's `retro`.
    pub(crate) fn retro_setting(&self, feature: &Feature) -> Retro {
        self.projects.workflows(feature, |w| w.retro).home
    }

    /// Start done feature `slug`'s retro agent, an agent of its own on AI
    /// review's agent CLI, in a tab of its workspace; `sent`: the comments
    /// its last retro was sent back with.
    pub(crate) fn start_retro(&mut self, project: &str, slug: &str, sent: Option<PathBuf>) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.status != Status::Done {
            let title = &feature.title;
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't done yet"));
        }
        if feature.retro_into.is_some() {
            return self.show_warning(MADE.into());
        }
        let (_, command) = match self.picked_cli(&feature, Stage::AiReview) {
            Ok(picked) => picked,
            Err(why) => return self.show_warning(why),
        };
        let data = store::dir(None, project, slug);
        let sent = sent.map(|p| p.display().to_string()).unwrap_or_default();
        let vars = [
            ("retro.path", data.join(FILE).display().to_string()),
            ("retro.comments", sent),
        ];
        let task = crate::feature_agents::OwnTask {
            template: "retro",
            vars: vars.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
            called: "Retro agent",
            picked: "AI review agent",
        };
        let Some((w, _)) = self.ensure_feature_workspace(project, slug) else {
            return;
        };
        self.start_own_agent(w, project, slug, &command, Some(task));
    }

    /// Start shipped feature `slug`'s retro as it ships, when its project
    /// asks for that (`Retro::Auto`) and its worktrees stay to run it in.
    pub(super) fn retro_after_ship(&mut self, project: &str, slug: &str, removing: bool) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let unshipped = (feature.marks.get(&Stage::Ship))
            .is_some_and(|m| m.note == super::ship_page::FINISHED_UNSHIPPED);
        if removing || unshipped || self.retro_setting(feature) != Retro::Auto {
            return;
        }
        self.start_retro(project, slug, None);
    }

    /// Feature `slug`'s retro is approved: a new feature of the same
    /// repositories, its linked projects' too, is made off the thread, the
    /// retro its brief (`retro_created`), and the review closes.
    pub(crate) fn approve_retro(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.retro_into.is_some() {
            return self.show_warning(MADE.into());
        }
        let data = store::dir(None, project, slug);
        let Ok(text) = std::fs::read_to_string(data.join(FILE)) else {
            return self.show_warning(NONE_YET.into());
        };
        if let Err(lacks) = feature::artifacts::check(Stage::Discover, &text) {
            return self.show_warning(format!(
                "The retro can't be a brief yet: {}",
                lacks.join(", ")
            ));
        }
        let Some(p) = self.projects.project(project) else {
            return self.show_warning("gerdr has no such project".into());
        };
        let search = texts::root_search(p);
        let others = self.other_projects_dirs(project);
        let named = |of: Option<&str>| -> Vec<String> {
            (feature.touched())
                .filter(|r| r.project.as_deref() == of)
                .map(|r| r.name.clone())
                .collect()
        };
        let home = named(None);
        // Each linked project the feature touched comes along, found as the
        // new feature dialog finds it.
        let mut linked = Vec::new();
        for link in &feature.links {
            let touched = named(Some(&link.project));
            let Some(lp) = self.projects.project(&link.project) else {
                continue;
            };
            if touched.is_empty() {
                continue;
            }
            let (search, others) = (
                texts::root_search(lp),
                self.other_projects_dirs(&link.project),
            );
            linked.push((
                link.project.clone(),
                lp.name.clone(),
                link.root.clone(),
                search,
                others,
                touched,
            ));
        }
        let title = retro_title(&feature.title);
        let new = self.new_feature_slug(project, &title);
        let key = (project.to_owned(), new.clone());
        if self.features.retro_new.contains_key(&key) {
            return self.show_warning(format!("\u{201c}{title}\u{201d} is being made already"));
        }
        self.features.retro_new.insert(key, slug.to_owned());
        self.doc_done(project, slug, DocFile::Retro);
        let busy = Busy {
            project: project.to_owned(),
            slug: new.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        let (from, project) = (feature.root.clone(), project.to_owned());
        self.feature_work(Some(busy), move |_| {
            let links = linked
                .into_iter()
                .map(|(project, name, root, search, others, named)| {
                    let (root, touched) = phone_page::phone_root(&root, &search, &others, &named)
                        .map_err(|why| format!("{name}: {why}"))?;
                    Ok(layout::LinkSpec {
                        project,
                        name,
                        root,
                        touched,
                    })
                });
            let links: Result<Vec<_>, String> = links.collect();
            let made = phone_page::phone_root(&from, &search, &others, &home)
                .and_then(|(root, touched)| Ok((root, touched, links?)));
            let (root, result) = match made {
                Ok((root, touched, links)) => {
                    let result = layout::create_spanning(&root, &new, &touched, &links);
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
                slug: new,
                root,
                result,
            }
        });
    }

    /// The worktrees of the feature feature `from`'s retro becomes were
    /// made, or not: kept as any new feature is, its brief the retro,
    /// Discover approved.
    pub(super) fn retro_created(
        &mut self,
        from: String,
        project: String,
        title: String,
        slug: String,
        root: layout::Root,
        result: Result<(layout::Created, Vec<(feature::Link, layout::Created)>), String>,
    ) {
        self.feature_created(project.clone(), title.clone(), slug.clone(), root, result);
        if self.feature(&project, &slug).is_none() {
            return;
        }
        let (old, new) = (
            store::dir(None, &project, &from),
            store::dir(None, &project, &slug),
        );
        let Some(brief) = feature::artifacts::path(&new, Stage::Discover) else {
            return;
        };
        let copied = std::fs::read(old.join(FILE))
            .and_then(|text| crate::projects::write_atomic(&brief, &text));
        if let Err(err) = copied {
            tracing::warn!("writing {}: {err}", brief.display());
            return self.show_error(format!(
                "Couldn't give \u{201c}{title}\u{201d} the retro as its brief: {err}"
            ));
        }
        let now = feature::now();
        if let Some(f) = self.feature_mut(&project, &from) {
            f.retro_into = Some(slug.clone());
            f.log(now, format!("Retro approved: it is feature {slug}"));
            self.save_feature(&project, &from);
        }
        if let Some(f) = self.feature_mut(&project, &slug) {
            f.log(now, format!("Made from the retro of feature {from}"));
            self.save_feature(&project, &slug);
        }
        self.approve_stage(&project, &slug, Stage::Discover);
    }

    /// Send feature `slug`'s retro back with `comments` (Markdown), kept
    /// beside it: pasted into its retro agent while that still runs, as a
    /// stage's file goes back, else a new retro agent writes it again.
    pub(crate) fn send_back_retro(&mut self, project: &str, slug: &str, comments: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.retro_into.is_some() {
            return self.show_warning(MADE.into());
        }
        let data = store::dir(None, project, slug);
        let sent = data.join(SENT);
        if let Err(err) = crate::projects::write_atomic(&sent, comments.as_bytes()) {
            tracing::warn!("writing {}: {err}", sent.display());
            return self.show_error(format!("Couldn't keep the comments: {err}"));
        }
        self.doc_done(project, slug, DocFile::Retro);
        let last = (feature.own_agents.iter().rev())
            .find(|a| a.task.as_deref() == Some("retro"))
            .and_then(|a| Some((a.n, a.agent.clone(), a.pane.clone()?)));
        let told = last.and_then(|(n, agent, pane)| {
            let values = [
                ("artifact.path", data.join(FILE).display().to_string()),
                ("comments.path", sent.display().to_string()),
            ];
            let values = values.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            let line = crate::prompts::render_builtin("send-back", &values).ok()?;
            self.tell_own_agent(&pane, line.trim()).ok()?;
            Some((n, agent))
        });
        let Some((n, agent)) = told else {
            return self.start_retro(project, slug, Some(sent));
        };
        if let Some(f) = self.feature_mut(project, slug) {
            f.log(
                feature::now(),
                format!("Retro sent back: {agent} (agent {n})"),
            );
            self.save_feature(project, slug);
        }
        self.show_success(format!("Sent the retro back to {agent}"));
    }

    /// Type `line` into own agent pane `pane` while an agent still runs
    /// there: one back at its shell's prompt would run it as a command.
    fn tell_own_agent(&mut self, pane: &str, line: &str) -> Result<(), String> {
        if !self.pane_there(pane) || crate::machine_work::split_pane(pane).is_some() {
            return Err(crate::feature_build::TAB_GONE.into());
        }
        let Some(herdr) = self.session.herdr.as_mut() else {
            return Err("herdr isn't connected".into());
        };
        if crate::prepare::step(&herdr.process_info(pane)) != crate::prepare::Step::Busy {
            return Err(crate::feature_build::AGENT_QUIT.into());
        }
        herdr.send_agent_line(pane, line)
    }
}
