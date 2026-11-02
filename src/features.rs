//! Features in gerdr's window: the new feature dialog, the feature panel,
//! and the work they start off the UI thread (see `feature` for the record
//! and where it lives).
//!
//! The features of every project are held here, loaded at start and when
//! the projects' file is read again, and saved after every change.
//!
//! The new feature dialog: a title, where the feature goes (found off the
//! thread), and over several repositories which ones it changes. Enter
//! creates it, Tab and Shift+Tab (Up/Down) move between the title and the
//! repositories, Space ticks one, Esc clears the title, then closes.
//!
//! The feature panel: a feature's repositories and its eight stages. Up and
//! Down (Ctrl+J/K) move, Enter does the row's own thing (mark the current
//! stage done, reopen a done one, touch a repository or open a tab in it),
//! Left/Right go to the previous or next feature, Ctrl+O opens a tab in the
//! feature's folder, Ctrl+C copies its path, Ctrl+N starts a new one, Del
//! abandons it (or, once done, removes its worktrees) after asking, Esc
//! closes.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, Dot, FeatureHit, FeaturePanelView, FeatureRepoRow, NewFeatureHit, NewFeatureRepo,
        NewFeatureView, StageMark, StageRow,
    },
    feature::{
        self, Event, Feature, Stage, Status,
        layout::{self, Created, Root},
        store,
    },
    home_dir, paths, projects,
    tasks::Task,
    text_field::{Edit, TextField},
};

/// The features of every project, and the dialog and panel on them.
#[derive(Default)]
pub struct Features {
    /// By project id, oldest first.
    pub all: BTreeMap<String, Vec<Feature>>,
    pub dialog: Option<NewFeatureDialog>,
    pub panel: Option<FeaturePanel>,
    /// The feature last shown in the panel, by project: the features key
    /// opens it again.
    shown: BTreeMap<String, String>,
    /// What runs off the thread now, for the tasks panel.
    busy: Vec<Busy>,
    /// The serial of the last dialog opened: a root found for an older one
    /// is dropped.
    serial: u64,
    done: Option<channel::Sender<Done>>,
}

/// Work on a feature running off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Busy {
    project: String,
    /// The feature's slug; for one being created, the slug it gets.
    slug: String,
    /// The repository being touched, if that is the work.
    repo: Option<String>,
    /// What it does, for the tasks panel: "Creating “Bulk export”".
    what: String,
}

/// What the work off the thread came to.
enum Done {
    Found {
        serial: u64,
        result: Result<Root, String>,
    },
    Created {
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<Created, String>,
    },
    Touched {
        project: String,
        slug: String,
        repo: String,
        result: Result<feature::Repo, String>,
    },
    Losses {
        project: String,
        slug: String,
        losses: Vec<String>,
    },
    Removed {
        project: String,
        slug: String,
        abandon: bool,
        result: Result<(), String>,
    },
}

/// Where a feature would go, as the dialog knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    Looking,
    Found(Root),
    Failed(String),
}

/// The new feature dialog's state, while it is open.
#[derive(Debug)]
pub struct NewFeatureDialog {
    pub project: String,
    pub serial: u64,
    pub title: TextField,
    pub root: Finding,
    /// Whether the feature changes each repository of the root.
    pub touched: Vec<bool>,
    /// The repository row with the keyboard; `None`: the title.
    pub focus: Option<usize>,
    /// The first repository shown, when not all fit.
    pub top: usize,
    pub error: Option<String>,
    pub hover: Option<NewFeatureHit>,
    /// The folder it was opened from: the repository ticked first.
    pub from: PathBuf,
}

/// What the new feature dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Cancel,
    /// Create a feature with this title in the root found, changing these
    /// repositories.
    Create {
        title: String,
        root: Root,
        touched: Vec<String>,
    },
}

impl NewFeatureDialog {
    fn new(project: String, serial: u64, from: PathBuf) -> Self {
        Self {
            project,
            serial,
            title: TextField::default(),
            root: Finding::Looking,
            touched: Vec::new(),
            focus: None,
            top: 0,
            error: None,
            hover: None,
            from,
        }
    }

    /// The root was found, or not: over several repositories the one it
    /// was opened from is ticked.
    fn found(&mut self, result: Result<Root, String>) {
        match result {
            Ok(root) => {
                self.touched = root
                    .repos
                    .iter()
                    .map(|r| !root.multi || self.from.starts_with(&r.main))
                    .collect();
                self.root = Finding::Found(root);
            }
            Err(why) => self.root = Finding::Failed(why),
        }
        self.error = None;
    }

    /// Whether its repositories can be ticked: over several only.
    fn multi(&self) -> bool {
        matches!(&self.root, Finding::Found(root) if root.multi)
    }

    fn repos(&self) -> usize {
        match &self.root {
            Finding::Found(root) => root.repos.len(),
            _ => 0,
        }
    }

    fn toggle(&mut self, i: usize) {
        if self.multi()
            && let Some(t) = self.touched.get_mut(i)
        {
            *t = !*t;
            self.error = None;
        }
    }

    /// What creating it would do, or why it can't, as a sentence.
    fn create(&self) -> Result<DialogOutcome, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Give the feature a title.".into());
        }
        let root = match &self.root {
            Finding::Looking => {
                return Err("Still finding the repositories; try again in a moment.".into());
            }
            Finding::Failed(why) => return Err(format!("No repositories to start from: {why}.")),
            Finding::Found(root) => root,
        };
        let touched: Vec<String> = root
            .repos
            .iter()
            .zip(&self.touched)
            .filter(|(_, t)| **t)
            .map(|(r, _)| r.name.clone())
            .collect();
        if touched.is_empty() {
            return Err("Tick at least one repository the feature changes.".into());
        }
        Ok(DialogOutcome::Create {
            title: title.to_owned(),
            root: root.clone(),
            touched,
        })
    }

    /// Move the keyboard between the title and the repositories.
    fn step(&mut self, back: bool) {
        let n = if self.multi() { self.repos() } else { 0 };
        let at = self.focus.map_or(0, |i| i + 1);
        let to = if back {
            (at + n) % (n + 1)
        } else {
            (at + 1) % (n + 1)
        };
        self.focus = to.checked_sub(1);
    }

    /// Scroll so the focused repository is among the `fit` shown.
    fn reveal(&mut self, fit: usize) {
        let fit = fit.max(1);
        if let Some(i) = self.focus {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + fit {
                self.top = i + 1 - fit;
            }
        }
        self.top = self.top.min(self.repos().saturating_sub(fit));
    }

    /// Handle a key, `fit` repositories shown at once: `sym` is the key's
    /// own symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        fit: usize,
    ) -> Option<DialogOutcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        match sym {
            Keysym::Escape => {
                // Back to the title, then the title cleared, then closed.
                if self.focus.is_some() {
                    self.focus = None;
                } else if !self.title.is_empty() {
                    self.title.clear();
                    self.error = None;
                } else {
                    return Some(DialogOutcome::Cancel);
                }
            }
            Keysym::Return | Keysym::KP_Enter => match self.create() {
                Ok(outcome) => return Some(outcome),
                Err(why) => self.error = Some(why),
            },
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::Down | Keysym::KP_Down => self.step(false),
            Keysym::Up | Keysym::KP_Up => self.step(true),
            Keysym::j | Keysym::J if ctrl => self.step(false),
            Keysym::k | Keysym::K if ctrl => self.step(true),
            Keysym::space if plain && self.focus.is_some() => {
                if let Some(i) = self.focus {
                    self.toggle(i);
                }
            }
            _ if self.focus.is_none() && self.title.key(mods, sym, typed) == Edit::Changed => {
                self.error = None;
            }
            _ => {}
        }
        self.reveal(fit);
        None
    }
}

/// The feature panel's state, while it is open.
#[derive(Debug, Default)]
pub struct FeaturePanel {
    pub project: String,
    pub slug: String,
    /// The row with the keyboard: the repositories', then the stages'.
    pub cursor: usize,
    /// The first repository shown, when not all fit.
    pub repo_top: usize,
    pub hover: Option<FeatureHit>,
}

/// What the feature panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelOutcome {
    Close,
    /// Mark the current stage done.
    Tick,
    /// Clear this stage's mark and the later ones'.
    Untick(Stage),
    /// A later stage can't be marked yet: this one comes first.
    NotYet(Stage),
    /// Make this repository one the feature changes.
    Touch(String),
    /// Open a tab in this folder: the feature's, or a repository's
    /// worktree.
    Open(PathBuf),
    CopyPath,
    /// Abandon it, or once done remove its worktrees, after asking.
    Remove,
    New,
    /// Show the feature with this slug.
    Show(String),
}

/// A row of the feature panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelRow {
    Repo(usize),
    Stage(Stage),
}

/// The rows of the feature panel for `feature`: its repositories, then
/// its stages.
pub fn panel_rows(feature: &Feature) -> Vec<PanelRow> {
    (0..feature.repos.len())
        .map(PanelRow::Repo)
        .chain(Stage::ALL.into_iter().map(PanelRow::Stage))
        .collect()
}

/// Where `stage` stands in `feature`.
pub fn stage_mark(feature: &Feature, stage: Stage) -> StageMark {
    if feature.is_done(stage) {
        StageMark::Done
    } else if feature.current() == Some(stage) {
        StageMark::Current
    } else {
        StageMark::Later
    }
}

/// What Enter (or the row's button) does on `row` of `feature`.
pub fn row_action(feature: &Feature, row: PanelRow) -> Option<PanelOutcome> {
    match row {
        PanelRow::Repo(i) => {
            let repo = feature.repos.get(i)?;
            Some(if repo.touched {
                PanelOutcome::Open(repo.worktree.clone())
            } else {
                PanelOutcome::Touch(repo.name.clone())
            })
        }
        PanelRow::Stage(stage) => Some(match stage_mark(feature, stage) {
            StageMark::Done => PanelOutcome::Untick(stage),
            StageMark::Current => PanelOutcome::Tick,
            StageMark::Later => PanelOutcome::NotYet(feature.current()?),
        }),
    }
}

/// What Enter does on `row`, in words for its hint.
fn row_hint(feature: &Feature, row: PanelRow) -> &'static str {
    match row_action(feature, row) {
        Some(PanelOutcome::Tick) => "mark done",
        Some(PanelOutcome::Untick(_)) => "reopen",
        Some(PanelOutcome::Touch(_)) => "touch",
        Some(PanelOutcome::Open(_)) => "open tab",
        _ => "",
    }
}

/// The label of what removes `feature`'s worktrees: abandoning it while
/// open, removing them once done.
pub fn remove_label(feature: &Feature) -> &'static str {
    if feature.status == Status::Done {
        "Remove worktrees"
    } else {
        "Abandon"
    }
}

/// The features the panel lists, of a project's `all`: the open ones, and
/// the done ones whose worktrees are still there to remove.
pub fn listed(all: &[Feature]) -> Vec<&Feature> {
    all.iter()
        .filter(|f| match f.status {
            Status::Open => true,
            Status::Done => f.dir.exists(),
            Status::Abandoned => false,
        })
        .collect()
}

/// The features the sidebar shows, of a project's `all`: the open ones.
pub fn sidebar_features(all: &[Feature]) -> Vec<&Feature> {
    all.iter().filter(|f| f.status == Status::Open).collect()
}

/// A slug for a feature called `title`, unique among `all`.
pub fn new_slug(title: &str, all: &[Feature]) -> String {
    let taken: Vec<&str> = all.iter().map(|f| f.slug.as_str()).collect();
    projects::slugify(title, &taken)
}

impl FeaturePanel {
    /// Handle a key over `feature`, one of `slugs` listed.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        feature: &Feature,
        slugs: &[String],
    ) -> Option<PanelOutcome> {
        let rows = panel_rows(feature);
        let last = rows.len().saturating_sub(1);
        self.cursor = self.cursor.min(last);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        match sym {
            Keysym::Escape => return Some(PanelOutcome::Close),
            Keysym::Up | Keysym::KP_Up => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Down | Keysym::KP_Down => self.cursor = (self.cursor + 1).min(last),
            Keysym::k | Keysym::K if ctrl => self.cursor = self.cursor.saturating_sub(1),
            Keysym::j | Keysym::J if ctrl => self.cursor = (self.cursor + 1).min(last),
            Keysym::Home | Keysym::KP_Home => self.cursor = 0,
            Keysym::End | Keysym::KP_End => self.cursor = last,
            Keysym::Return | Keysym::KP_Enter => return row_action(feature, rows[self.cursor]),
            Keysym::Left | Keysym::KP_Left | Keysym::Right | Keysym::KP_Right if plain => {
                let back = matches!(sym, Keysym::Left | Keysym::KP_Left);
                return switch(slugs, &self.slug, back).map(PanelOutcome::Show);
            }
            Keysym::o | Keysym::O if ctrl => return Some(PanelOutcome::Open(feature.dir.clone())),
            Keysym::c | Keysym::C if ctrl => return Some(PanelOutcome::CopyPath),
            Keysym::n | Keysym::N if ctrl => return Some(PanelOutcome::New),
            Keysym::Delete | Keysym::KP_Delete if plain => return Some(PanelOutcome::Remove),
            _ => {}
        }
        None
    }

    /// Scroll so the cursor's repository is among the `fit` shown, of
    /// `repos`.
    fn reveal(&mut self, repos: usize, fit: usize) {
        let fit = fit.max(1);
        if self.cursor < repos {
            if self.cursor < self.repo_top {
                self.repo_top = self.cursor;
            } else if self.cursor >= self.repo_top + fit {
                self.repo_top = self.cursor + 1 - fit;
            }
        }
        self.repo_top = self.repo_top.min(repos.saturating_sub(fit));
    }
}

/// The slug before (`back`) or after `slug` in `slugs`, round the ends;
/// `None` with only one.
fn switch(slugs: &[String], slug: &str, back: bool) -> Option<String> {
    if slugs.len() < 2 {
        return None;
    }
    let at = slugs.iter().position(|s| s == slug).unwrap_or(0);
    let n = slugs.len();
    let to = if back { (at + n - 1) % n } else { (at + 1) % n };
    Some(slugs[to].clone())
}

/// The feature the features key shows, of `slugs` listed: the one open,
/// else the one last shown, else the newest.
fn pick_feature(slugs: &[String], open: Option<&str>, last: Option<&str>) -> Option<String> {
    [open, last]
        .into_iter()
        .flatten()
        .find(|s| slugs.iter().any(|l| l == s))
        .map(str::to_owned)
        .or_else(|| slugs.last().cloned())
}

/// The cursor a feature's panel starts at: its current stage, else its
/// last.
fn start_cursor(feature: &Feature) -> usize {
    let stage = feature.current().unwrap_or(Stage::Ship);
    let at = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
    feature.repos.len() + at
}

/// A folder as the chrome shows it, from the home folder as `~`.
fn tilde(path: &Path) -> String {
    paths::display(path, home_dir().as_deref(), 60)
}

/// What a done stage's row says of its mark.
fn mark_detail(mark: &feature::Mark, now: u64) -> String {
    let when = projects::ago(now, mark.at);
    if mark.note.is_empty() {
        format!("done {when} by {}", mark.by)
    } else {
        format!("done {when} by {} \u{b7} {}", mark.by, mark.note)
    }
}

impl Gerdr {
    // ------------------------------------------------------------ state

    /// Read the features of every project from their store, as at start
    /// and when the projects' file is read again. A project whose features
    /// can't be read keeps those it had, and says so.
    pub fn load_features(&mut self) {
        let ids: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let mut all = BTreeMap::new();
        for (id, name) in ids {
            match store::load_all(None, &id) {
                Ok(features) => {
                    all.insert(id, features);
                }
                Err(why) => {
                    tracing::warn!("reading the features of {id}: {why}");
                    self.show_error(format!("Couldn't read the features of {name}: {why}"));
                    if let Some(kept) = self.features.all.remove(&id) {
                        all.insert(id, kept);
                    }
                }
            }
        }
        self.features.all = all;
        // A panel on a feature gone closes.
        if let Some(panel) = &self.features.panel
            && self.feature(&panel.project, &panel.slug).is_none()
        {
            self.features.panel = None;
            self.refresh_pointer_focus();
        }
    }

    /// Feature `slug` of project `project`.
    pub fn feature(&self, project: &str, slug: &str) -> Option<&Feature> {
        self.features
            .all
            .get(project)?
            .iter()
            .find(|f| f.slug == slug)
    }

    fn feature_mut(&mut self, project: &str, slug: &str) -> Option<&mut Feature> {
        self.features
            .all
            .get_mut(project)?
            .iter_mut()
            .find(|f| f.slug == slug)
    }

    /// Save feature `slug` of `project` as it is now; a failure says so.
    fn save_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if let Err(err) = store::save(None, feature) {
            tracing::warn!("saving feature {project}/{slug}: {err}");
            let title = feature.title.clone();
            self.show_error(format!("Couldn't save \u{201c}{title}\u{201d}: {err}"));
        }
    }

    /// The slugs of the features the panel lists for `project`.
    fn listed_slugs(&self, project: &str) -> Vec<String> {
        self.features
            .all
            .get(project)
            .map(|all| listed(all).into_iter().map(|f| f.slug.clone()).collect())
            .unwrap_or_default()
    }

    /// Where the threads send what they came to; set up the first time.
    fn feature_sender(&mut self) -> Option<channel::Sender<Done>> {
        if self.features.done.is_none() {
            let (tx, rx) = channel::channel::<Done>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg(done) => state.feature_done(done),
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.features.done = Some(tx),
                Err(err) => {
                    tracing::warn!("working on features: {err}");
                    self.show_error(format!("Couldn't start the work on features: {err}"));
                    return None;
                }
            }
        }
        self.features.done.clone()
    }

    /// Run `work` off the thread, its result sent back as a `Done`; `busy`
    /// shows in the tasks panel meanwhile.
    fn feature_work(&mut self, busy: Option<Busy>, work: impl FnOnce() -> Done + Send + 'static) {
        let Some(tx) = self.feature_sender() else {
            return;
        };
        self.features.busy.extend(busy);
        std::thread::spawn(move || {
            // It fails only once the loop is gone: gerdr is quitting, and
            // nothing is left to show the result in.
            let _ = tx.send(work());
        });
    }

    /// The end of work on a feature, the panel open or not: what it came
    /// to is kept and said.
    fn feature_done(&mut self, done: Done) {
        match done {
            Done::Found { serial, result } => {
                if let Err(why) = &result {
                    tracing::warn!("finding where a feature goes: {why}");
                }
                // Dropped when the dialog it was for closed: a new one
                // looks again.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.found(result);
                }
            }
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            } => {
                self.features
                    .busy
                    .retain(|b| !(b.project == project && b.slug == slug));
                self.feature_created(project, title, slug, root, result);
            }
            Done::Touched {
                project,
                slug,
                repo,
                result,
            } => {
                self.features.busy.retain(|b| {
                    !(b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo))
                });
                self.feature_touched(&project, &slug, &repo, result);
            }
            Done::Losses {
                project,
                slug,
                losses,
            } => {
                self.features
                    .busy
                    .retain(|b| !(b.project == project && b.slug == slug));
                self.ask_remove_feature(project, slug, losses);
            }
            Done::Removed {
                project,
                slug,
                abandon,
                result,
            } => {
                self.features
                    .busy
                    .retain(|b| !(b.project == project && b.slug == slug));
                self.feature_removed(&project, &slug, abandon, result);
            }
        }
    }

    /// The tasks running on features, for the tasks panel.
    pub fn feature_tasks(&self) -> Vec<Task> {
        self.features
            .busy
            .iter()
            .map(|b| Task {
                title: b.what.clone(),
                chip: "feature".into(),
                detail: self
                    .projects
                    .project(&b.project)
                    .map_or_else(|| b.project.clone(), |p| p.name.clone()),
                dot: Dot::Working,
                actions: Vec::new(),
            })
            .collect()
    }

    // ----------------------------------------------------------- dialog

    /// Open the new feature dialog for the active project, from the
    /// folder of workspace `at`, else the active one's.
    pub fn open_new_feature(&mut self, at: Option<usize>) {
        let Some(id) = self.project.clone() else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let Some(project) = self.projects.project(&id) else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let workspace_dirs: Vec<PathBuf> =
            project.workspaces.iter().map(|w| w.dir.clone()).collect();
        let ignore = project.workflow.ignore.clone();
        let bases: Vec<(String, String)> = project
            .workflow
            .base
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let from = at
            .and_then(|i| self.workspaces.get(i))
            .or_else(|| self.workspace())
            .map(|ws| ws.dir.clone())
            .or_else(|| workspace_dirs.first().cloned());
        let Some(from) = from else {
            return self.show_warning("Add a workspace with a folder to start a feature".into());
        };
        self.features.serial += 1;
        let serial = self.features.serial;
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.dialog = Some(NewFeatureDialog::new(id, serial, from.clone()));
        self.feature_work(None, move || Done::Found {
            serial,
            result: layout::find_root(&from, &workspace_dirs, &ignore, &bases),
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// How many repositories the dialog shows at once.
    fn new_feature_fit(&self) -> usize {
        let Some(dialog) = &self.features.dialog else {
            return 1;
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .rows
            .len()
    }

    /// A key while the new feature dialog has the keyboard.
    pub fn new_feature_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<DialogOutcome> {
        let fit = self.new_feature_fit();
        self.features.dialog.as_mut()?.key(mods, sym, typed, fit)
    }

    /// Do what the new feature dialog asked: create the feature off the
    /// thread, shown in the tasks panel meanwhile.
    pub fn apply_new_feature(&mut self, outcome: DialogOutcome) {
        let Some(dialog) = self.features.dialog.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        let DialogOutcome::Create {
            title,
            root,
            touched,
        } = outcome
        else {
            return;
        };
        let project = dialog.project;
        let all = self
            .features
            .all
            .get(&project)
            .map_or(&[][..], Vec::as_slice);
        let slug = new_slug(&title, all);
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        self.feature_work(Some(busy), move || {
            let result = layout::create(&root, &slug, &touched);
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            }
        });
    }

    /// A feature's worktrees were made, or not: keep it, show it, and say
    /// what was changed outside its folder.
    fn feature_created(
        &mut self,
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<Created, String>,
    ) {
        let created = match result {
            Ok(created) => created,
            Err(why) => {
                tracing::warn!("creating feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't create \u{201c}{title}\u{201d}: {why}"));
            }
        };
        let text = created_text(&title, &root.path, &created.ignores);
        let now = feature::now();
        let feature = Feature {
            version: feature::VERSION,
            slug: slug.clone(),
            title: title.clone(),
            project: project.clone(),
            created: now,
            root: root.path,
            dir: created.dir,
            multi: root.multi,
            repos: created.repos,
            status: Status::Open,
            marks: BTreeMap::new(),
            history: vec![Event {
                at: now,
                what: "created".into(),
            }],
            shipping: Default::default(),
        };
        self.features
            .all
            .entry(project.clone())
            .or_default()
            .push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        self.show_success(text);
    }

    /// A left click while the new feature dialog is open.
    pub fn new_feature_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
        let outcome = match layout.hit(dialog.top, pos) {
            Some(NewFeatureHit::Button(chrome::CREATE)) => match dialog.create() {
                Ok(outcome) => Some(outcome),
                Err(why) => {
                    dialog.error = Some(why);
                    None
                }
            },
            Some(NewFeatureHit::Button(_)) => Some(DialogOutcome::Cancel),
            Some(NewFeatureHit::Repo(i)) => {
                if dialog.multi() {
                    dialog.focus = Some(i);
                    dialog.toggle(i);
                }
                None
            }
            Some(NewFeatureHit::Field) => {
                dialog.focus = None;
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            // Off it, it stays: a click away never throws the title away.
            None => None,
        };
        if let Some(outcome) = outcome {
            self.apply_new_feature(outcome);
        }
    }

    /// The pointer moved while the new feature dialog is open.
    pub fn new_feature_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
            dialog.hover = layout.hit(dialog.top, pos);
        }
    }

    /// Scroll the dialog's repositories `by` rows.
    pub fn new_feature_scroll(&mut self, by: isize) {
        let fit = self.new_feature_fit();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let most = dialog.repos().saturating_sub(fit.max(1));
            dialog.top = dialog.top.saturating_add_signed(by).min(most);
        }
    }

    /// The dialog's repository rows shown, for the wheel's pitch.
    pub fn new_feature_rows(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some()).rows
    }

    /// What the pointer can press in the new feature dialog.
    pub fn new_feature_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .controls()
    }

    pub fn new_feature_view(&self) -> Option<NewFeatureView> {
        let dialog = self.features.dialog.as_ref()?;
        let project = self
            .projects
            .project(&dialog.project)
            .map_or_else(|| dialog.project.clone(), |p| p.name.clone());
        Some(new_feature_view(dialog, project))
    }

    // ------------------------------------------------------------ panel

    /// The features key: the panel on the active project's features, or
    /// the new feature dialog when it has none.
    pub fn open_features(&mut self) {
        let Some(project) = self.project.clone() else {
            return self.show_warning("Open a project to see its features".into());
        };
        let slugs = self.listed_slugs(&project);
        let open = self.features.panel.as_ref().map(|p| p.slug.as_str());
        let last = self.features.shown.get(&project).map(String::as_str);
        match pick_feature(&slugs, open, last) {
            Some(slug) => self.open_feature(&project, &slug),
            None => self.open_new_feature(None),
        }
    }

    /// Open the feature panel on feature `slug` of `project`. The other
    /// panels close: it would cover them.
    pub fn open_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let cursor = start_cursor(feature);
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.features
            .shown
            .insert(project.to_owned(), slug.to_owned());
        self.features.panel = Some(FeaturePanel {
            project: project.to_owned(),
            slug: slug.to_owned(),
            cursor,
            ..FeaturePanel::default()
        });
        self.reveal_feature_row();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the feature panel.
    pub fn close_feature_panel(&mut self) {
        self.features.panel = None;
        self.refresh_pointer_focus();
    }

    /// The feature the panel is on.
    fn panel_feature(&self) -> Option<&Feature> {
        let panel = self.features.panel.as_ref()?;
        self.feature(&panel.project, &panel.slug)
    }

    /// Scroll the panel's repositories to the cursor.
    fn reveal_feature_row(&mut self) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(
            view.repos.len(),
            view.stages.len(),
            view.hints.len(),
            self.output_size(),
        );
        if let Some(panel) = self.features.panel.as_mut() {
            panel.reveal(view.repos.len(), fit);
        }
    }

    /// A key while the feature panel has the keyboard.
    pub fn feature_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
    ) -> Option<PanelOutcome> {
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let Some(feature) = self.feature(&panel.project, &panel.slug).cloned() else {
            return Some(PanelOutcome::Close);
        };
        let out = self
            .features
            .panel
            .as_mut()?
            .key(mods, sym, &feature, &slugs);
        self.reveal_feature_row();
        out
    }

    /// Do what the feature panel asked.
    pub fn apply_feature_panel(&mut self, outcome: PanelOutcome) {
        let Some((project, slug)) = self
            .features
            .panel
            .as_ref()
            .map(|p| (p.project.clone(), p.slug.clone()))
        else {
            return;
        };
        let now = feature::now();
        match outcome {
            PanelOutcome::Close => self.close_feature_panel(),
            PanelOutcome::Tick => {
                let Some(feature) = self.feature_mut(&project, &slug) else {
                    return;
                };
                match feature.tick(now, "you", None) {
                    Some(stage) => {
                        let shipped = feature.status == Status::Done;
                        let title = feature.title.clone();
                        self.save_feature(&project, &slug);
                        self.show_success(if shipped {
                            format!("\u{201c}{title}\u{201d} is done")
                        } else {
                            format!("{} marked done", stage.label())
                        });
                        self.feature_cursor_to_current();
                    }
                    None => self.show_warning("Nothing left to mark done".into()),
                }
            }
            PanelOutcome::Untick(stage) => {
                let Some(feature) = self.feature_mut(&project, &slug) else {
                    return;
                };
                if feature.untick(stage, now) {
                    self.save_feature(&project, &slug);
                    self.show_success(format!("{} reopened", stage.label()));
                } else {
                    self.show_warning(format!("{} isn't done", stage.label()));
                }
            }
            PanelOutcome::NotYet(first) => {
                self.show_warning(format!("Mark {} done first", first.label()));
            }
            PanelOutcome::Touch(repo) => self.touch_repo(&project, &slug, repo),
            PanelOutcome::Open(dir) => {
                if !dir.exists() {
                    return self.show_error(format!("{} is gone", tilde(&dir)));
                }
                self.close_feature_panel();
                let w = self.active_workspace;
                self.new_project_tab(w, &dir);
            }
            PanelOutcome::CopyPath => {
                if let Some(dir) = self.feature(&project, &slug).map(|f| f.dir.clone()) {
                    self.copy_text(dir.display().to_string());
                    self.show_success(format!("Copied {}", tilde(&dir)));
                }
            }
            PanelOutcome::Remove => self.check_remove_feature(&project, &slug),
            PanelOutcome::New => self.open_new_feature(None),
            PanelOutcome::Show(to) => {
                if let Some(feature) = self.feature(&project, &to) {
                    let cursor = start_cursor(feature);
                    self.features.shown.insert(project.clone(), to.clone());
                    if let Some(panel) = self.features.panel.as_mut() {
                        panel.slug = to;
                        panel.cursor = cursor;
                        panel.repo_top = 0;
                    }
                    self.reveal_feature_row();
                }
            }
        }
    }

    /// Put the panel's cursor on its feature's current stage.
    fn feature_cursor_to_current(&mut self) {
        let Some(cursor) = self.panel_feature().map(start_cursor) else {
            return;
        };
        if let Some(panel) = self.features.panel.as_mut() {
            panel.cursor = cursor;
        }
    }

    /// Make `repo` one feature `slug` changes, off the thread.
    fn touch_repo(&mut self, project: &str, slug: &str, repo: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let doing = self
            .features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo));
        if doing {
            return self.show_warning(format!("{repo} is being touched already"));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: Some(repo.clone()),
            what: format!("Touching {repo} for \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || {
            let result = layout::touch(&feature, &repo);
            Done::Touched {
                project,
                slug,
                repo,
                result,
            }
        });
    }

    fn feature_touched(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        result: Result<feature::Repo, String>,
    ) {
        let touched = match result {
            Ok(touched) => touched,
            Err(why) => {
                tracing::warn!("touching {repo} for feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't touch {repo}: {why}"));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let branch = touched.branch.clone().unwrap_or_else(|| feature.branch());
        match feature.repos.iter_mut().find(|r| r.name == repo) {
            Some(r) => *r = touched,
            None => feature.repos.push(touched),
        }
        feature.log(feature::now(), format!("touched {repo}"));
        self.save_feature(project, slug);
        self.show_success(format!("{repo} is on {branch} now"));
    }

    /// Look at what removing feature `slug`'s worktrees loses, off the
    /// thread, then ask (see `ask_remove_feature`).
    fn check_remove_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self
            .features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug)
        {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        if let Some(user) = self.folder_in_use(&feature.dir) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has {user} in it; close it first",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || Done::Losses {
            losses: layout::losses(&feature),
            project,
            slug,
        });
    }

    /// Ship feature `slug` of `project` as its last Ship check said, once
    /// the Ship sheet is confirmed: open its pull requests off the thread,
    /// or start its squash in a tab.
    pub fn ship_feature(&mut self, project: &str, slug: &str) {
        let _ = (project, slug);
    }

    /// Ask before removing feature `slug`'s worktrees, naming what goes
    /// with them.
    fn ask_remove_feature(&mut self, project: String, slug: String, losses: Vec<String>) {
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let title = feature.title.clone();
        let abandon = feature.status != Status::Done;
        let (question, go) = if abandon {
            (format!("Abandon \u{201c}{title}\u{201d}?"), "Abandon")
        } else {
            (
                format!("Remove the worktrees of \u{201c}{title}\u{201d}?"),
                "Remove worktrees",
            )
        };
        let body = removal_body(&losses);
        let force = !losses.is_empty();
        let then = crate::confirm::Then::RemoveFeature {
            project,
            slug,
            abandon,
            force,
        };
        self.ask_remove_worktrees(question, body, go, then);
    }

    /// Remove feature `slug`'s worktrees off the thread (`force`: with
    /// what they lose), then mark it abandoned if `abandon`.
    pub fn remove_feature(&mut self, project: &str, slug: &str, abandon: bool, force: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: None,
            what: format!(
                "Removing the worktrees of \u{201c}{}\u{201d}",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || Done::Removed {
            result: layout::remove(&feature, force),
            project,
            slug,
            abandon,
        });
    }

    fn feature_removed(
        &mut self,
        project: &str,
        slug: &str,
        abandon: bool,
        result: Result<(), String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("removing the worktrees of feature {project}/{slug}: {why}");
            return self.show_error(format!(
                "Couldn't remove the worktrees of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        let now = feature::now();
        if abandon {
            feature.status = Status::Abandoned;
            feature.log(now, "abandoned");
        } else {
            feature.log(now, "worktrees removed");
        }
        self.save_feature(project, slug);
        // The panel moves on to another feature, or closes.
        let on_it = self
            .features
            .panel
            .as_ref()
            .is_some_and(|p| p.project == project && p.slug == slug);
        if on_it {
            match self.listed_slugs(project).into_iter().find(|s| s != slug) {
                Some(next) => self.apply_feature_panel(PanelOutcome::Show(next)),
                None => self.close_feature_panel(),
            }
        }
        self.show_success(if abandon {
            format!("Abandoned \u{201c}{title}\u{201d}")
        } else {
            format!("Removed the worktrees of \u{201c}{title}\u{201d}")
        });
    }

    /// A left click while the feature panel is open: off it closes it.
    pub fn feature_panel_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let Some(feature) = self.panel_feature().cloned() else {
            return;
        };
        let layout = chrome::feature_panel_layout(&view, self.output_size());
        let rows = panel_rows(&feature);
        let repos = feature.repos.len();
        let stage = |i: usize| PanelRow::Stage(Stage::ALL[i.min(Stage::ALL.len() - 1)]);
        let outcome = match layout.hit(view.repo_top, pos) {
            None => Some(PanelOutcome::Close),
            Some(FeatureHit::Panel) => None,
            Some(FeatureHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i).cloned() {
                    self.press_hint(crate::input::Overlay::Features, &keys, pos);
                }
                None
            }
            Some(FeatureHit::Prev | FeatureHit::Next) => {
                let slugs = self.listed_slugs(&feature.project);
                let back = layout.hit(view.repo_top, pos) == Some(FeatureHit::Prev);
                switch(&slugs, &feature.slug, back).map(PanelOutcome::Show)
            }
            Some(FeatureHit::Repo(i) | FeatureHit::Stage(i)) => {
                let at = match layout.hit(view.repo_top, pos) {
                    Some(FeatureHit::Stage(_)) => repos + i,
                    _ => i,
                };
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.cursor = at.min(rows.len().saturating_sub(1));
                }
                None
            }
            Some(FeatureHit::RepoButton(i)) => row_action(&feature, PanelRow::Repo(i)),
            Some(FeatureHit::StageButton(i)) => row_action(&feature, stage(i)),
            Some(FeatureHit::Button(0)) => Some(PanelOutcome::Open(feature.dir.clone())),
            Some(FeatureHit::Button(1)) => Some(PanelOutcome::CopyPath),
            Some(FeatureHit::Button(2)) => Some(PanelOutcome::New),
            Some(FeatureHit::Button(_)) => Some(PanelOutcome::Remove),
        };
        if let Some(outcome) = outcome {
            self.apply_feature_panel(outcome);
        }
    }

    /// The pointer moved while the feature panel is open.
    pub fn feature_panel_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = self.feature_panel_view().and_then(|view| {
            chrome::feature_panel_layout(&view, self.output_size()).hit(view.repo_top, pos)
        });
        if let Some(panel) = self.features.panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel over the feature panel: its repositories `by` rows on.
    pub fn feature_panel_scroll(&mut self, by: isize) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(
            view.repos.len(),
            view.stages.len(),
            view.hints.len(),
            self.output_size(),
        );
        if let Some(panel) = self.features.panel.as_mut() {
            let most = view.repos.len().saturating_sub(fit);
            panel.repo_top = panel.repo_top.saturating_add_signed(by).min(most);
        }
    }

    /// The panel's repository rows shown, for the wheel's pitch.
    pub fn feature_repo_rows(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.output_size())
                .repos
                .into_iter()
                .map(|(row, _)| row)
                .collect()
        })
    }

    /// What the pointer can press in the feature panel.
    pub fn feature_panel_controls(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.output_size()).controls()
        })
    }

    pub fn feature_panel_view(&self) -> Option<FeaturePanelView> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let slugs = self.listed_slugs(&panel.project);
        let busy: Vec<&str> = self
            .features
            .busy
            .iter()
            .filter(|b| b.project == panel.project && b.slug == panel.slug)
            .filter_map(|b| b.repo.as_deref())
            .collect();
        Some(panel_view(panel, feature, &slugs, &busy, feature::now()))
    }

    /// The sidebar's rows for the shown project's open features.
    pub fn feature_rows(&self) -> Vec<chrome::FeatureRowView> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let Some(all) = self.features.all.get(project) else {
            return Vec::new();
        };
        sidebar_features(all)
            .into_iter()
            .map(|f| chrome::FeatureRowView {
                title: f.title.clone(),
                marks: Stage::ALL.iter().map(|s| stage_mark(f, *s)).collect(),
            })
            .collect()
    }

    /// How many feature rows the sidebar shows.
    pub fn sidebar_feature_count(&self) -> usize {
        let all = self.project.as_ref().and_then(|p| self.features.all.get(p));
        all.map_or(0, |all| sidebar_features(all).len())
    }

    /// Feature `i` of the sidebar's rows.
    pub fn sidebar_feature(&self, i: usize) -> Option<&Feature> {
        let all = self.features.all.get(self.project.as_ref()?)?;
        sidebar_features(all).get(i).copied()
    }
}

/// What the toast says once feature `title` is made under `root`, naming
/// the files `.features` was added to (`ignores`) from the root: whole
/// paths are long enough to cut the last ones off.
fn created_text(title: &str, root: &Path, ignores: &[PathBuf]) -> String {
    if ignores.is_empty() {
        return format!("Created \u{201c}{title}\u{201d}");
    }
    let names: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| tilde(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "Created \u{201c}{title}\u{201d} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        names.join(", ")
    )
}

/// What a confirm dialog about removing a feature's worktrees says of
/// `losses`, what goes with them.
fn removal_body(losses: &[String]) -> String {
    match losses {
        [] => "Its worktrees go; nothing in them is lost".into(),
        [one] => format!("Its worktrees go, and with them {one}"),
        more => format!("Its worktrees go, and with them {}", more.join(", ")),
    }
}

/// What the new feature dialog shows of `dialog`, in project `project`.
fn new_feature_view(dialog: &NewFeatureDialog, project: String) -> NewFeatureView {
    let (root, root_failed, repos) = match &dialog.root {
        Finding::Looking => ("finding repositories\u{2026}".to_owned(), false, Vec::new()),
        Finding::Failed(why) => (format!("No repositories found: {why}"), true, Vec::new()),
        Finding::Found(root) => {
            let n = root.repos.len();
            let line = if root.multi {
                format!("In {} \u{b7} {n} repositories", tilde(&root.path))
            } else {
                format!("In {}", tilde(&root.path))
            };
            let repos = root
                .repos
                .iter()
                .zip(&dialog.touched)
                .map(|(r, t)| NewFeatureRepo {
                    name: r.name.clone(),
                    base: r.base.clone(),
                    touched: *t,
                    fixed: !root.multi,
                })
                .collect();
            (line, false, repos)
        }
    };
    let mut keys = if dialog.multi() {
        vec![("Tab", "next"), ("Space", "tick")]
    } else {
        Vec::new()
    };
    // Esc steps back as `NewFeatureDialog::key` has it.
    keys.push(if dialog.focus.is_some() {
        ("Esc", "back")
    } else if !dialog.title.is_empty() {
        ("Esc", "clear")
    } else {
        ("Esc", "close")
    });
    NewFeatureView {
        project,
        title: dialog.title.line(),
        focus: dialog.focus,
        root,
        root_failed,
        repos,
        top: dialog.top,
        error: dialog.error.clone(),
        hover: dialog.hover,
        keys,
    }
}

/// What the feature panel shows of `feature`, one of `slugs`, its
/// repositories named in `busy` being touched.
fn panel_view(
    panel: &FeaturePanel,
    feature: &Feature,
    slugs: &[String],
    busy: &[&str],
    now: u64,
) -> FeaturePanelView {
    let repos = feature
        .repos
        .iter()
        .map(|r| FeatureRepoRow {
            name: r.name.clone(),
            line: if r.touched {
                r.branch.clone().unwrap_or_else(|| feature.branch())
            } else {
                format!("context, at {}", r.base)
            },
            touched: r.touched,
            busy: busy.contains(&r.name.as_str()),
        })
        .collect();
    let stages = Stage::ALL
        .iter()
        .map(|s| {
            let mark = stage_mark(feature, *s);
            StageRow {
                label: s.label().to_owned(),
                detail: match feature.marks.get(s) {
                    Some(m) => mark_detail(m, now),
                    None => s.about().to_owned(),
                },
                mark,
            }
        })
        .collect();
    let rows = panel_rows(feature);
    let row = rows[panel.cursor.min(rows.len() - 1)];
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    let mut hints = vec![hint("Up / Down", "move")];
    let enter = row_hint(feature, row);
    if !enter.is_empty() {
        hints.push(hint("Enter", enter));
    }
    if slugs.len() > 1 {
        hints.push(hint("Left / Right", "other feature"));
    }
    hints.extend([
        hint("Ctrl+O", "open tab"),
        hint("Ctrl+C", "copy path"),
        hint("Ctrl+N", "new"),
        hint("Del", &remove_label(feature).to_lowercase()),
        hint("Esc", "close"),
    ]);
    let which = slugs
        .iter()
        .position(|s| *s == feature.slug)
        .map(|i| (i, slugs.len()));
    let mut sub = format!("{} \u{b7} {}", feature.slug, tilde(&feature.dir));
    if feature.status == Status::Done {
        sub = format!("done \u{b7} {sub}");
    }
    FeaturePanelView {
        title: feature.title.clone(),
        sub,
        which,
        repos,
        stages,
        cursor: panel.cursor,
        repo_top: panel.repo_top,
        remove: remove_label(feature),
        hover: panel.hover,
        hints,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::feature::{Mark, Repo};
    use layout::Found;

    pub(crate) fn repo(name: &str, touched: bool) -> Repo {
        Repo {
            name: name.into(),
            main: PathBuf::from(format!("/work/{name}")),
            base: "main".into(),
            base_commit: "abc123".into(),
            worktree: PathBuf::from(format!("/work/.features/bulk/{name}")),
            touched,
            branch: touched.then(|| "feature/bulk".into()),
        }
    }

    /// A feature over `repos`, its first `done` stages marked.
    pub(crate) fn feature(slug: &str, repos: Vec<Repo>, done: usize) -> Feature {
        Feature {
            version: feature::VERSION,
            slug: slug.into(),
            title: format!("Feature {slug}"),
            project: "acme".into(),
            created: 1_000,
            root: "/work".into(),
            dir: format!("/work/.features/{slug}").into(),
            multi: repos.len() > 1,
            repos,
            status: Status::Open,
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
        panel.key(&mods, sym, f, &slugs)
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

    #[test]
    fn esc_goes_back_to_the_title_then_clears_it_then_closes() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
        d.found(Ok(root(&["api", "web"])));
        type_into(&mut d, "Bulk");
        assert_eq!(&*d.title, "Bulk");
        dkey(&mut d, key(Keysym::Tab));
        assert_eq!(d.focus, Some(0));
        assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
        assert_eq!(d.focus, None);
        assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
        assert!(d.title.is_empty());
        assert_eq!(
            dkey(&mut d, key(Keysym::Escape)),
            Some(DialogOutcome::Cancel)
        );
    }

    #[test]
    fn enter_creates_with_the_ticked_repositories_or_says_what_is_missing() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api/src".into());
        assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
        assert_eq!(d.error.as_deref(), Some("Give the feature a title."));
        type_into(&mut d, "Bulk export");
        // Typing clears the error; still looking, Enter says so.
        dkey(&mut d, key(Keysym::Return));
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Still finding"))
        );
        d.found(Ok(root(&["api", "web"])));
        // The repository it was opened from is ticked.
        assert_eq!(d.touched, [true, false]);
        // Space ticks the focused one; in the title it types.
        dkey(&mut d, key(Keysym::Down));
        dkey(&mut d, key(Keysym::Down));
        assert_eq!(d.focus, Some(1));
        dkey(&mut d, key(Keysym::space));
        dkey(&mut d, ctrl(Keysym::k));
        dkey(&mut d, key(Keysym::space));
        assert_eq!(d.touched, [false, true]);
        let out = dkey(&mut d, key(Keysym::Return));
        assert_eq!(
            out,
            Some(DialogOutcome::Create {
                title: "Bulk export".into(),
                root: root(&["api", "web"]),
                touched: vec!["web".into()],
            })
        );
        dkey(&mut d, key(Keysym::Down));
        dkey(&mut d, key(Keysym::space));
        assert_eq!(d.touched, [false, false]);
        assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Tick at least"))
        );
        // Shift+Tab from the title goes round to the last repository.
        d.focus = None;
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        dkey(&mut d, (shift, Keysym::ISO_Left_Tab));
        assert_eq!(d.focus, Some(1));
    }

    #[test]
    fn a_single_repository_is_ticked_and_fixed() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/elsewhere".into());
        d.found(Ok(root(&["api"])));
        assert_eq!(d.touched, [true]);
        dkey(&mut d, key(Keysym::Tab));
        assert_eq!(d.focus, None, "nothing to move to");
        d.toggle(0);
        assert_eq!(d.touched, [true]);
        let view = new_feature_view(&d, "Acme".into());
        assert!(view.repos[0].fixed);
        assert_eq!(view.keys, [("Esc", "close")]);
    }

    #[test]
    fn panel_keys_ask_for_the_rows_and_the_features_actions() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 2);
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        // It starts on the current stage: Plan done, Build next.
        assert_eq!(panel.cursor, 2 + 2);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Tick)
        );
        press(&mut panel, &f, key(Keysym::Up));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Untick(Stage::Plan))
        );
        press(&mut panel, &f, ctrl(Keysym::j));
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::NotYet(Stage::Build))
        );
        press(&mut panel, &f, key(Keysym::Home));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Open("/work/.features/bulk/api".into()))
        );
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Touch("web".into()))
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::o)),
            Some(PanelOutcome::Open("/work/.features/b".into()))
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::c)),
            Some(PanelOutcome::CopyPath)
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::n)),
            Some(PanelOutcome::New)
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Left)),
            Some(PanelOutcome::Show("a".into()))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Right)),
            Some(PanelOutcome::Show("c".into()))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Escape)),
            Some(PanelOutcome::Close)
        );
        // Ctrl+N and Ctrl+P never move.
        let at = panel.cursor;
        press(&mut panel, &f, ctrl(Keysym::p));
        assert_eq!(panel.cursor, at);
    }

    #[test]
    fn the_panel_shows_repositories_then_stages_and_the_right_hints() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
        let panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["b".to_owned()];
        let view = panel_view(&panel, &f, &slugs, &["web"], 2_060);
        assert_eq!(view.repos[0].line, "feature/bulk");
        assert_eq!(view.repos[1].line, "context, at main");
        assert!(view.repos[1].busy);
        let marks: Vec<StageMark> = view.stages.iter().map(|s| s.mark).collect();
        assert_eq!(
            &marks[..4],
            [
                StageMark::Done,
                StageMark::Done,
                StageMark::Done,
                StageMark::Current
            ]
        );
        assert_eq!(view.stages[0].detail, "done 1m ago by you");
        assert_eq!(view.stages[3].detail, Stage::Verify.about());
        assert_eq!(view.which, Some((0, 1)));
        let keys: Vec<&str> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "Up / Down",
                "Enter",
                "Ctrl+O",
                "Ctrl+C",
                "Ctrl+N",
                "Del",
                "Esc"
            ]
        );
        assert_eq!(view.hints[1].1, "mark done");
        assert_eq!(view.remove, "Abandon");
    }

    #[test]
    fn sidebar_and_panel_list_the_right_features() {
        let mut open = feature("a", vec![repo("api", true)], 0);
        open.title = "Open".into();
        let mut abandoned = feature("b", vec![repo("api", true)], 1);
        abandoned.status = Status::Abandoned;
        let mut done = feature("c", vec![repo("api", true)], 8);
        done.status = Status::Done;
        let all = vec![open, abandoned, done];
        let shown: Vec<&str> = sidebar_features(&all)
            .iter()
            .map(|f| f.slug.as_str())
            .collect();
        assert_eq!(shown, ["a"]);
        // A done feature's folder is gone here: nothing to remove.
        let listed: Vec<&str> = listed(&all).iter().map(|f| f.slug.as_str()).collect();
        assert_eq!(listed, ["a"]);
        assert_eq!(remove_label(&all[2]), "Remove worktrees");
    }

    #[test]
    fn slugs_are_unique_in_the_project() {
        let all = vec![
            feature("bulk-export", vec![], 0),
            feature("bulk-export-2", vec![], 0),
        ];
        assert_eq!(new_slug("Bulk export!", &all), "bulk-export-3");
        assert_eq!(new_slug("Other thing", &all), "other-thing");
    }

    #[test]
    fn the_features_key_shows_the_open_then_the_last_shown_then_the_newest() {
        let slugs = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(pick_feature(&slugs, Some("a"), Some("b")), Some("a".into()));
        assert_eq!(pick_feature(&slugs, None, Some("a")), Some("a".into()));
        // One gone (abandoned) is passed over.
        assert_eq!(pick_feature(&slugs, None, Some("gone")), Some("b".into()));
        assert_eq!(pick_feature(&[], None, Some("a")), None);
    }

    #[test]
    fn the_created_toast_names_the_ignore_files_from_the_root() {
        let root = Path::new("/w/multi");
        let ignores = vec![
            root.join(".cursorignore"),
            root.join("pyrightconfig.json"),
            PathBuf::from("/elsewhere/.git/info/exclude"),
        ];
        assert_eq!(
            created_text("Bulk export", root, &ignores),
            "Created \u{201c}Bulk export\u{201d} \u{b7} added .features to .cursorignore, \
             pyrightconfig.json, /elsewhere/.git/info/exclude"
        );
        assert_eq!(created_text("X", root, &[]), "Created \u{201c}X\u{201d}");
    }

    #[test]
    fn removing_names_what_is_lost() {
        assert_eq!(
            removal_body(&[]),
            "Its worktrees go; nothing in them is lost"
        );
        assert_eq!(
            removal_body(&["2 changed files in api".into()]),
            "Its worktrees go, and with them 2 changed files in api"
        );
    }

    #[test]
    fn switching_goes_round_and_needs_two() {
        let slugs: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(switch(&slugs, "b", false).as_deref(), Some("a"));
        assert_eq!(switch(&slugs, "a", true).as_deref(), Some("b"));
        assert_eq!(switch(&slugs[..1], "a", true), None);
    }
}
