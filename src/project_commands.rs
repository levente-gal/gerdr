//! The project commands dialog: what Verify checks (`workflow.verify`,
//! `verify_timeout`), what sets a new worktree up (`workflow.setup`) and what a
//! sandbox runs (`workflow.run`, `workflow.prepare`), without editing the
//! project file by hand.
//!
//! Each repository has a field of lines, a command a line; emptied, its key
//! goes. Only what was edited is written, so a command a hand wrote over
//! several lines stays one unless its field is changed. Opened from a
//! feature, it holds the projects that feature links too, whose
//! repositories take their own project's commands (`feature::verify`).
//!
//! What is saved applies from the next run: nothing runs and no stage mark
//! changes here. A project file edited outside gerdr isn't saved over: the
//! dialog says so and offers the reload, what was typed surviving that. So
//! does closing it: what was typed comes back at the next opening.

use std::collections::BTreeMap;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Size},
};

use crate::{
    Gerdr,
    chrome::{
        self, COMMANDS_GO, COMMANDS_NOTCH, CommandsFocus, CommandsHit, CommandsRow, CommandsTab,
        CommandsView,
    },
    feature::{
        suggest::{self, Suggested},
        verify::FEATURE_KEY,
    },
    menu::MenuAction,
    projects::Workflow,
    scroll::Motion,
    text_field::{Edit, TextField},
    toast::Level,
};

/// Where a dialog was opened for: a project, or a feature of it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommandsScope {
    pub project: String,
    pub slug: Option<String>,
}

/// A field, with what it held when read from the store.
#[derive(Debug)]
struct Slot {
    field: TextField,
    was: String,
}

impl Slot {
    fn new(commands: Option<&Vec<String>>) -> Self {
        let was = commands.map_or_else(String::new, |c| c.join("\n"));
        Self {
            field: TextField::multiline(&was),
            was,
        }
    }

    fn changed(&self) -> bool {
        *self.field != *self.was
    }
}

/// A repository of the dialog, or the feature as a whole.
#[derive(Debug)]
struct Row {
    project: String,
    key: String,
    checks: Slot,
    /// None for the feature as a whole: only checks run there.
    setup: Option<Slot>,
    /// None likewise.
    run: Option<Slot>,
    /// None likewise.
    prepare: Option<Slot>,
    /// The feature changes it, so its checks run.
    in_feature: bool,
    /// Looked for and not there: its commands stay until emptied.
    missing: bool,
    suggested: Suggested,
}

impl Row {
    fn slot(&self, tab: CommandsTab) -> Option<&Slot> {
        match tab {
            CommandsTab::Checks => Some(&self.checks),
            CommandsTab::Setup => self.setup.as_ref(),
            CommandsTab::Run => self.run.as_ref(),
            CommandsTab::Prepare => self.prepare.as_ref(),
        }
    }

    fn slot_mut(&mut self, tab: CommandsTab) -> Option<&mut Slot> {
        match tab {
            CommandsTab::Checks => Some(&mut self.checks),
            CommandsTab::Setup => self.setup.as_mut(),
            CommandsTab::Run => self.run.as_mut(),
            CommandsTab::Prepare => self.prepare.as_mut(),
        }
    }

    fn is_feature(&self) -> bool {
        self.key == FEATURE_KEY
    }

    fn new(project: &str, key: &str, workflow: &Workflow) -> Self {
        Self {
            project: project.to_owned(),
            key: key.to_owned(),
            checks: Slot::new(workflow.verify.get(key)),
            setup: (key != FEATURE_KEY).then(|| Slot::new(workflow.setup.get(key))),
            run: (key != FEATURE_KEY).then(|| Slot::new(workflow.run.get(key))),
            prepare: (key != FEATURE_KEY).then(|| Slot::new(workflow.prepare.get(key))),
            in_feature: false,
            missing: false,
            suggested: Suggested::default(),
        }
    }

    /// The commands its suggestions make on `tab`, if any.
    fn suggestion(&self, tab: CommandsTab) -> &[String] {
        match tab {
            CommandsTab::Checks => &self.suggested.checks,
            CommandsTab::Setup => &self.suggested.setup,
            CommandsTab::Run => &self.suggested.run,
            CommandsTab::Prepare => &self.suggested.prepare,
        }
    }
}

/// What has the keyboard, by what it is: the rows come and go as the
/// repositories are found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Focus {
    Row(String, String),
    Limit,
}

/// What was typed and not saved, for the next opening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandsDraft {
    /// (project, key, tab, what its field holds).
    rows: Vec<(String, String, CommandsTab, String)>,
    limit: Option<String>,
}

/// How `verify`, `setup` and `verify_timeout` of a project stood before a
/// save, for its Undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandsBefore {
    project: String,
    verify: BTreeMap<String, Vec<String>>,
    setup: BTreeMap<String, Vec<String>>,
    verify_timeout: Option<u64>,
}

/// What the dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandsOutcome {
    Close,
    /// Save, or reload the project file first if it can't take that.
    Go,
}

/// The dialog's state, while it is open.
#[derive(Debug)]
pub struct CommandsDialog {
    /// Which opening it is: an older one's findings are stale.
    pub serial: u64,
    scope: CommandsScope,
    /// Its projects' ids and names, the home project first.
    projects: Vec<(String, String)>,
    rows: Vec<Row>,
    limit: TextField,
    limit_was: String,
    tab: CommandsTab,
    focus: Focus,
    down: i32,
    /// How many projects' repositories are still being looked for.
    looking: usize,
    /// Why looking failed, where it did.
    failed: Option<String>,
    /// A project has no folder to look for repositories in.
    nowhere: bool,
    error: Option<String>,
    stale: bool,
    pub hover: Option<CommandsHit>,
}

/// What a project's commands are as a list: each line, trimmed, the empty
/// ones gone.
fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Set `key`'s `commands` in `map`; none: its key goes.
fn set_commands(map: &mut BTreeMap<String, Vec<String>>, key: &str, commands: Vec<String>) {
    if commands.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_owned(), commands);
    }
}

/// The time limit in `text`, in seconds: none when empty; else why not, as
/// a sentence.
fn parse_limit(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<u64>() {
        Ok(seconds) if seconds > 0 => Ok(Some(seconds)),
        _ => Err("The time limit is a whole number of seconds above 0.".into()),
    }
}

impl CommandsDialog {
    /// A dialog holding `projects` (id, name, workflow; the home project
    /// first) as stored.
    fn new(
        serial: u64,
        scope: CommandsScope,
        projects: &[(String, String, Workflow)],
        tab: CommandsTab,
    ) -> Self {
        let mut rows = Vec::new();
        for (id, _, workflow) in projects {
            let keys = (workflow.verify.keys())
                .chain(workflow.setup.keys())
                .chain(workflow.run.keys())
                .chain(workflow.prepare.keys())
                .filter(|key| *key != FEATURE_KEY);
            let keys: std::collections::BTreeSet<_> = keys.collect();
            rows.extend(keys.into_iter().map(|key| Row::new(id, key, workflow)));
        }
        if let Some((id, _, workflow)) = projects.first() {
            rows.push(Row::new(id, FEATURE_KEY, workflow));
        }
        let limit_was = (projects.first())
            .and_then(|(_, _, w)| w.verify_timeout)
            .map_or_else(String::new, |seconds| seconds.to_string());
        let mut dialog = Self {
            serial,
            scope,
            projects: (projects.iter())
                .map(|(id, name, _)| (id.clone(), name.clone()))
                .collect(),
            rows,
            limit: limit_was.as_str().into(),
            limit_was,
            tab,
            focus: Focus::Limit,
            down: 0,
            looking: 0,
            failed: None,
            nowhere: false,
            error: None,
            stale: false,
            hover: None,
        };
        dialog.sort();
        dialog.focus_first();
        dialog
    }

    /// In the order they show: by project, the found by name, then those
    /// not found, the feature as a whole last.
    fn sort(&mut self) {
        let projects = self.projects.clone();
        let place = |row: &Row| projects.iter().position(|(id, _)| *id == row.project);
        self.rows
            .sort_by_cached_key(|row| (place(row), row.is_feature(), row.missing, row.key.clone()));
    }

    /// The indices of the rows shown on the tab.
    fn shown(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| self.rows[i].slot(self.tab).is_some())
            .collect()
    }

    fn has_limit(&self) -> bool {
        self.tab == CommandsTab::Checks
    }

    /// What has the keyboard, in the order of the shown: the tab's rows
    /// then the time limit.
    fn stops(&self) -> Vec<Focus> {
        let rows = self.shown().into_iter().map(|i| {
            let row = &self.rows[i];
            Focus::Row(row.project.clone(), row.key.clone())
        });
        let limit = self.has_limit().then_some(Focus::Limit);
        rows.chain(limit).collect()
    }

    fn focus_first(&mut self) {
        if let Some(first) = self.stops().into_iter().next() {
            self.focus = first;
        }
    }

    /// The next stop (`back`: the one before), round.
    fn step(&mut self, back: bool) {
        let stops = self.stops();
        if stops.is_empty() {
            return;
        }
        let at = stops.iter().position(|s| *s == self.focus).unwrap_or(0);
        let n = stops.len();
        self.focus = stops[(at + if back { n - 1 } else { 1 }) % n].clone();
    }

    fn set_tab(&mut self, tab: CommandsTab) {
        if self.tab != tab {
            self.tab = tab;
            self.down = 0;
            self.focus_first();
        }
    }

    fn row_of(&self, focus: &Focus) -> Option<usize> {
        let Focus::Row(project, key) = focus else {
            return None;
        };
        (self.rows.iter()).position(|row| row.project == *project && row.key == *key)
    }

    fn field_mut(&mut self) -> Option<&mut TextField> {
        if self.focus == Focus::Limit {
            return self.has_limit().then_some(&mut self.limit);
        }
        let (i, tab) = (self.row_of(&self.focus)?, self.tab);
        Some(&mut self.rows[i].slot_mut(tab)?.field)
    }

    /// Fill the focused row's empty field with what is suggested for it.
    fn use_suggestion(&mut self) {
        if let Some(i) = self.row_of(&self.focus) {
            self.use_row(i);
        }
    }

    fn use_row(&mut self, i: usize) {
        let tab = self.tab;
        let row = &mut self.rows[i];
        let text = row.suggestion(tab).join("\n");
        let Some(slot) = row.slot_mut(tab).filter(|s| s.field.is_empty()) else {
            return;
        };
        if !text.is_empty() {
            slot.field.set(&text);
            self.focus = Focus::Row(row.project.clone(), row.key.clone());
            self.typed();
        }
    }

    /// A field changed: what the form said of it goes; the project file
    /// edited outside gerdr stays said, until it is reloaded.
    fn typed(&mut self) {
        self.error = self.stale.then(|| STALE.to_owned());
    }

    /// A key while the dialog has the keyboard.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<CommandsOutcome> {
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let only_alt = mods.alt && !mods.ctrl && !mods.logo && !mods.shift;
        let only_shift = mods.shift && !mods.ctrl && !mods.alt && !mods.logo;
        let bare = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        match sym {
            // Closed, what was typed kept for the next opening.
            Keysym::Escape => return Some(CommandsOutcome::Close),
            Keysym::Return | Keysym::KP_Enter if only_shift => {
                if self.focus != Focus::Limit
                    && let Some(field) = self.field_mut()
                {
                    field.newline();
                    self.typed();
                }
            }
            Keysym::Return | Keysym::KP_Enter => return Some(CommandsOutcome::Go),
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::j | Keysym::J if ctrl => self.step(false),
            Keysym::k | Keysym::K if ctrl => self.step(true),
            Keysym::Page_Up | Keysym::KP_Page_Up if ctrl => self.set_tab(self.tab.next(true)),
            Keysym::Page_Down | Keysym::KP_Page_Down if ctrl => {
                self.set_tab(self.tab.next(false));
            }
            Keysym::u | Keysym::U if only_alt => self.use_suggestion(),
            _ => {
                let edit = self
                    .field_mut()
                    .map_or(Edit::Ignored, |field| field.key(mods, sym, typed));
                match edit {
                    Edit::Changed => self.typed(),
                    // Past a field's first or last line: the field beside.
                    Edit::Ignored if bare => match sym {
                        Keysym::Down | Keysym::KP_Down => self.step(false),
                        Keysym::Up | Keysym::KP_Up => self.step(true),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        None
    }

    /// Bring what has the keyboard into the list's view.
    fn follow(&mut self, size: Size<i32, Logical>) {
        let view = self.view();
        let layout = chrome::commands_layout(&view, size);
        let Some((top, height)) = layout.focus_span(&view) else {
            return;
        };
        if top < self.down {
            self.down = top;
        } else if top + height > self.down + layout.list.size.h {
            self.down = top + height - layout.list.size.h;
        }
        self.down = self.down.clamp(0, layout.most_down());
    }

    /// Move the list `by`, in logical pixels.
    fn scroll(&mut self, by: i32, size: Size<i32, Logical>) {
        let layout = chrome::commands_layout(&self.view(), size);
        self.down = (self.down + by).clamp(0, layout.most_down());
    }

    /// The repositories of `project` were found (`names`, with what their
    /// files suggest): each gets a row, what was typed staying.
    fn found(&mut self, project: &str, names: Vec<(String, Suggested)>, workflow: &Workflow) {
        for row in self.rows.iter_mut().filter(|r| r.project == project) {
            row.missing = !row.is_feature() && !names.iter().any(|(name, _)| *name == row.key);
        }
        for (name, suggested) in names {
            let at = (self.rows.iter()).position(|r| r.project == project && r.key == name);
            let i = at.unwrap_or_else(|| {
                self.rows.push(Row::new(project, &name, workflow));
                self.rows.len() - 1
            });
            self.rows[i].suggested = suggested;
        }
        self.sort();
        // The first field to come, unless the keyboard is on purpose where
        // it is. One with no field to be in goes there anyway.
        let on_field = match &self.focus {
            Focus::Limit => self.has_limit(),
            focus => self
                .row_of(focus)
                .is_some_and(|i| self.rows[i].slot(self.tab).is_some()),
        };
        let on_feature = self
            .row_of(&self.focus)
            .is_some_and(|i| self.rows[i].is_feature());
        if !on_field || (on_feature && !self.touched()) {
            self.focus_first();
        }
    }

    /// Whether anything was typed.
    fn touched(&self) -> bool {
        self.limit_changed()
            || (self.rows.iter())
                .any(|r| r.checks.changed() || r.setup.as_ref().is_some_and(Slot::changed))
    }

    fn limit_changed(&self) -> bool {
        *self.limit != *self.limit_was
    }

    /// What was typed, for the next opening.
    fn draft(&self) -> CommandsDraft {
        let mut rows = Vec::new();
        for row in &self.rows {
            for tab in CommandsTab::ALL {
                if let Some(slot) = row.slot(tab).filter(|s| s.changed()) {
                    let text = slot.field.to_string();
                    rows.push((row.project.clone(), row.key.clone(), tab, text));
                }
            }
        }
        CommandsDraft {
            rows,
            limit: self.limit_changed().then(|| self.limit.to_string()),
        }
    }

    /// Put `draft` back in its fields, over what is stored now. A row of a
    /// project no longer here is gone with it.
    fn restore(&mut self, draft: CommandsDraft, projects: &[(String, String, Workflow)]) {
        for (project, key, tab, text) in draft.rows {
            let Some((_, _, workflow)) = projects.iter().find(|(id, ..)| *id == project) else {
                continue;
            };
            let at = (self.rows.iter()).position(|r| r.project == project && r.key == key);
            let i = at.unwrap_or_else(|| {
                self.rows.push(Row::new(&project, &key, workflow));
                self.rows.len() - 1
            });
            if let Some(slot) = self.rows[i].slot_mut(tab) {
                slot.field.set(&text);
            }
        }
        if let Some(limit) = draft.limit {
            self.limit.set(&limit);
        }
        self.sort();
    }

    /// The projects with something to save.
    fn changed_projects(&self) -> Vec<String> {
        let edited = |id: &str| {
            (self.rows.iter()).any(|r| {
                r.project == id
                    && CommandsTab::ALL
                        .iter()
                        .any(|tab| r.slot(*tab).is_some_and(Slot::changed))
            })
        };
        let mut ids: Vec<String> = (self.projects.iter())
            .filter(|(id, _)| edited(id))
            .map(|(id, _)| id.clone())
            .collect();
        if self.limit_changed() && !ids.contains(&self.scope.project) {
            ids.push(self.scope.project.clone());
        }
        ids
    }

    /// Write what was edited of `project` into its `workflow`: the rest of
    /// it stays as it is. Whatever is wrong with the time limit was asked
    /// of `parse_limit` first.
    fn apply(&self, project: &str, workflow: &mut Workflow, limit: Option<u64>) {
        for row in self.rows.iter().filter(|r| r.project == project) {
            if row.checks.changed() {
                let commands = lines_of(&row.checks.field);
                set_commands(&mut workflow.verify, &row.key, commands);
            }
            if let Some(setup) = row.setup.as_ref().filter(|s| s.changed()) {
                set_commands(&mut workflow.setup, &row.key, lines_of(&setup.field));
            }
            if let Some(run) = row.run.as_ref().filter(|s| s.changed()) {
                set_commands(&mut workflow.run, &row.key, lines_of(&run.field));
            }
            if let Some(prepare) = row.prepare.as_ref().filter(|s| s.changed()) {
                set_commands(&mut workflow.prepare, &row.key, lines_of(&prepare.field));
            }
        }
        if project == self.scope.project && self.limit_changed() {
            workflow.verify_timeout = limit;
        }
    }

    /// What shows of `row` on the tab, under `heading` if it starts its
    /// project.
    fn row_view(&self, row: &Row, slot: &Slot, heading: Option<String>) -> CommandsRow {
        let tag = if row.is_feature() {
            Some("in the feature folder")
        } else if row.missing {
            Some("not found")
        } else if row.in_feature {
            Some("in this feature")
        } else {
            None
        };
        let suggestion = row.suggestion(self.tab);
        CommandsRow {
            heading,
            label: match row.is_feature() {
                true => "Whole feature".to_owned(),
                false => row.key.clone(),
            },
            tag: tag.map(str::to_owned),
            suggestion: (slot.field.is_empty() && !suggestion.is_empty())
                .then(|| suggestion.join(", ")),
            line: slot.field.line(),
        }
    }

    fn note(&self, rows: usize) -> Option<(String, bool)> {
        if self.looking > 0 {
            return Some(("finding repositories\u{2026}".to_owned(), false));
        }
        if let Some(why) = &self.failed {
            return Some((format!("Couldn't look for repositories: {why}"), true));
        }
        if rows == 0 {
            let text = match self.nowhere {
                true => "Add a workspace with a folder to set commands for its repositories",
                false => "No repositories found in this project",
            };
            return Some((text.to_owned(), false));
        }
        None
    }

    fn view(&self) -> CommandsView {
        let mut rows = Vec::new();
        let mut last = None;
        for i in self.shown() {
            let row = &self.rows[i];
            let Some(slot) = row.slot(self.tab) else {
                continue;
            };
            let starts = last != Some(&row.project);
            last = Some(&row.project);
            let name = (self.projects.iter())
                .find(|(id, _)| *id == row.project)
                .map(|(_, name)| name.clone());
            let heading = name.filter(|_| starts && self.projects.len() > 1);
            rows.push(self.row_view(row, slot, heading));
        }
        let shown = self.shown();
        let focus = match self.focus {
            Focus::Limit if self.has_limit() => CommandsFocus::Limit,
            _ => {
                let at = self.row_of(&self.focus);
                let at = at.and_then(|i| shown.iter().position(|s| *s == i));
                CommandsFocus::Row(at.unwrap_or(0))
            }
        };
        let can_use = match focus {
            CommandsFocus::Row(i) => rows
                .get(i)
                .is_some_and(|r: &CommandsRow| r.suggestion.is_some()),
            CommandsFocus::Limit => false,
        };
        let mut keys = Vec::new();
        if matches!(focus, CommandsFocus::Row(_)) {
            keys.push(("Shift+Enter", "new line"));
        }
        if can_use {
            keys.push(("Alt+U", "use"));
        }
        keys.push(("Esc", "close"));
        keys.push(("Tab", "next"));
        keys.push(("Ctrl+PgDn", "next tab"));
        CommandsView {
            tab: self.tab,
            body: match self.tab {
                CommandsTab::Checks => {
                    "One command per line, run at once (&& keeps an order). A check that changes code fails Verify."
                }
                CommandsTab::Setup => {
                    "One command per line. They run in a new worktree, once it has its files."
                }
                CommandsTab::Run => {
                    "One command per line, each in a tab of the sandbox. A line starting with # doesn't run."
                }
                CommandsTab::Prepare => {
                    "One command per line, in order, before a sandbox's commands: a migration on its data."
                }
            },
            note: self.note(rows.len()),
            limit: self.has_limit().then(|| self.limit.line()),
            rows,
            focus,
            down: self.down,
            error: self.error.clone(),
            stale: self.stale,
            hover: self.hover,
            keys,
        }
    }
}

/// What the dialog says once a project's file changed outside gerdr.
const STALE: &str = "The project file was edited outside gerdr. Reload it to save here.";

impl Gerdr {
    /// The projects a dialog for `scope` holds, the home project first:
    /// id, name, workflow. None when it is gone.
    fn commands_seen_running(&self, project: &str) -> Vec<(std::path::PathBuf, String)> {
        let Some(p) = self.projects.project(project) else {
            return Vec::new();
        };
        let mut seen = Vec::new();
        for record in &p.workspaces {
            for pane in &record.panes {
                let Some(command) = pane.restart_command() else {
                    continue;
                };
                if self.may_be_service(command) {
                    let dir = pane.cwd.as_ref().unwrap_or(&record.dir);
                    seen.push((dir.clone(), command.to_owned()));
                }
            }
        }
        seen
    }

    fn commands_projects(&self, scope: &CommandsScope) -> Option<Vec<(String, String, Workflow)>> {
        let entry = |id: &str| {
            (self.projects.project(id)).map(|p| (p.id.clone(), p.name.clone(), p.workflow.clone()))
        };
        let mut projects = vec![entry(&scope.project)?];
        if let Some(slug) = &scope.slug {
            let feature = self.feature(&scope.project, slug)?;
            projects.extend(feature.links.iter().filter_map(|l| entry(&l.project)));
        }
        Some(projects)
    }

    /// Open the dialog on `tab`, for feature `slug` of `project` if one is
    /// given: what was typed and not saved before comes back.
    pub fn open_project_commands(&mut self, project: &str, slug: Option<&str>, tab: CommandsTab) {
        let scope = CommandsScope {
            project: project.to_owned(),
            slug: slug.map(str::to_owned),
        };
        let Some(projects) = self.commands_projects(&scope) else {
            return self.show_warning("That project is gone".into());
        };
        self.features.serial += 1;
        let serial = self.features.serial;
        let mut dialog = CommandsDialog::new(serial, scope.clone(), &projects, tab);
        if let Some(draft) = self.features.commands_drafts.remove(&scope) {
            dialog.restore(draft, &projects);
        }
        // Rows of a feature's repositories show at once.
        if let Some(feature) = slug.and_then(|slug| self.feature(project, slug)) {
            for repo in feature.repos.iter().filter(|r| r.touched) {
                let id = repo.project.as_deref().unwrap_or(project);
                let Some((_, _, workflow)) = projects.iter().find(|(p, ..)| p == id) else {
                    continue;
                };
                let at = (dialog.rows.iter()).position(|r| r.project == id && r.key == repo.name);
                let i = at.unwrap_or_else(|| {
                    dialog.rows.push(Row::new(id, &repo.name, workflow));
                    dialog.rows.len() - 1
                });
                dialog.rows[i].in_feature = true;
            }
            dialog.sort();
        }
        dialog.focus_first();
        if (projects.iter()).any(|(id, ..)| self.projects.file_changed(id)) {
            dialog.stale = true;
            dialog.error = Some(STALE.into());
        }
        // Look for each project's repositories, off the thread (D3).
        let mut looks = Vec::new();
        for (id, ..) in &projects {
            let Some(p) = self.projects.project(id) else {
                continue;
            };
            let search = crate::features::root_search(p);
            let home_dir = (id == project)
                .then(|| slug.and_then(|s| self.feature(project, s)))
                .flatten()
                .map(|f| f.dir.clone());
            match home_dir.or_else(|| search.0.first().cloned()) {
                Some(from) => looks.push((
                    id.clone(),
                    from,
                    search,
                    self.other_projects_dirs(id),
                    self.commands_seen_running(id),
                )),
                None => dialog.nowhere = true,
            }
        }
        dialog.looking = looks.len();
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.commands = Some(dialog);
        for (id, from, (dirs, ignore, bases), others, seen) in looks {
            self.feature_work(None, move |_| crate::features::Done::CommandsFound {
                serial,
                project: id,
                result: crate::feature::layout::find_project_root(
                    &from, &dirs, &others, &ignore, &bases,
                )
                .map(|root| {
                    (root.repos.iter())
                        .map(|r| {
                            let mut suggested = suggest::suggest(&r.main);
                            suggest::with_seen(&mut suggested, &r.main, &seen);
                            (r.name.clone(), suggested)
                        })
                        .collect()
                }),
            });
        }
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The repositories of `project` were looked for, for the dialog of
    /// `serial`. Dropped when that one closed: a new one looks again.
    pub(crate) fn commands_found(
        &mut self,
        serial: u64,
        project: &str,
        result: Result<Vec<(String, Suggested)>, String>,
    ) {
        if let Err(why) = &result {
            tracing::warn!("finding the repositories of {project}: {why}");
        }
        let workflow = self.projects.project(project).map(|p| p.workflow.clone());
        let size = self.output_size();
        let Some(dialog) = self
            .features
            .commands
            .as_mut()
            .filter(|d| d.serial == serial)
        else {
            return;
        };
        dialog.looking = dialog.looking.saturating_sub(1);
        match (result, workflow) {
            (Ok(names), Some(workflow)) => dialog.found(project, names, &workflow),
            (Ok(_), None) => {}
            (Err(why), _) => dialog.failed = Some(why),
        }
        dialog.follow(size);
    }

    /// A key while the dialog has the keyboard.
    pub fn commands_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<CommandsOutcome> {
        let size = self.output_size();
        let dialog = self.features.commands.as_mut()?;
        let bare = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let page = chrome::commands_layout(&dialog.view(), size).list.size.h;
        match sym {
            Keysym::Page_Up | Keysym::KP_Page_Up if bare => dialog.scroll(-page, size),
            Keysym::Page_Down | Keysym::KP_Page_Down if bare => dialog.scroll(page, size),
            _ => {
                let outcome = dialog.key(mods, sym, typed);
                dialog.follow(size);
                return outcome;
            }
        }
        None
    }

    /// Do what the dialog asked.
    pub fn apply_commands(&mut self, outcome: CommandsOutcome) {
        match outcome {
            CommandsOutcome::Close => self.close_commands(),
            CommandsOutcome::Go if self.features.commands.as_ref().is_some_and(|d| d.stale) => {
                self.reload_projects();
            }
            CommandsOutcome::Go => self.save_commands(),
        }
    }

    /// Close the dialog, keeping what was typed for the next opening.
    fn close_commands(&mut self) {
        if let Some(dialog) = self.features.commands.take() {
            let draft = dialog.draft();
            if draft != CommandsDraft::default() {
                self.features.commands_drafts.insert(dialog.scope, draft);
            }
        }
        self.refresh_pointer_focus();
    }

    /// Write what was edited to the project files. Nothing edited closes
    /// it; a file it can't write to leaves it open, saying so.
    fn save_commands(&mut self) {
        let size = self.output_size();
        let Some(dialog) = self.features.commands.as_mut() else {
            return;
        };
        let limit = match parse_limit(&dialog.limit) {
            Ok(limit) => limit,
            Err(why) => {
                dialog.error = Some(why);
                dialog.set_tab(CommandsTab::Checks);
                dialog.focus = Focus::Limit;
                dialog.follow(size);
                return;
            }
        };
        let ids = dialog.changed_projects();
        if ids.is_empty() {
            return self.close_commands();
        }
        if ids.iter().any(|id| self.projects.file_changed(id)) {
            if let Some(dialog) = self.features.commands.as_mut() {
                dialog.stale = true;
                dialog.error = Some(STALE.into());
            }
            return;
        }
        let Some(dialog) = self.features.commands.take() else {
            return;
        };
        let mut before = Vec::new();
        for id in &ids {
            if let Some(p) = self.projects.project_mut(id) {
                before.push(CommandsBefore {
                    project: id.clone(),
                    verify: p.workflow.verify.clone(),
                    setup: p.workflow.setup.clone(),
                    verify_timeout: p.workflow.verify_timeout,
                });
                dialog.apply(id, &mut p.workflow, limit);
            }
        }
        if !self.projects.save_projects(&ids) {
            tracing::warn!("couldn't write the project commands of {ids:?}");
            self.put_commands_back(&before);
            self.projects.save();
            let mut dialog = dialog;
            dialog.error = Some("gerdr couldn't write the project file. See its log.".into());
            self.features.commands = Some(dialog);
            return;
        }
        self.features.commands_drafts.remove(&dialog.scope);
        self.refresh_pointer_focus();
        self.toast_with(
            "Project commands saved; they apply from the next run".to_owned(),
            Level::Success,
            Some((
                "Undo".to_owned(),
                MenuAction::RestoreProjectCommands(before),
            )),
        );
    }

    fn put_commands_back(&mut self, before: &[CommandsBefore]) {
        for was in before {
            if let Some(p) = self.projects.project_mut(&was.project) {
                p.workflow.verify = was.verify.clone();
                p.workflow.setup = was.setup.clone();
                p.workflow.verify_timeout = was.verify_timeout;
            }
        }
    }

    /// Undo a save of the dialog's, unless a file changed since.
    pub(crate) fn restore_project_commands(&mut self, before: Vec<CommandsBefore>) {
        let ids: Vec<String> = before.iter().map(|b| b.project.clone()).collect();
        if ids.iter().any(|id| self.projects.file_changed(id)) {
            return self.toast_with(
                "The project file was edited outside gerdr; reload it first".to_owned(),
                Level::Warning,
                Some(("Reload".to_owned(), MenuAction::ReloadProjects)),
            );
        }
        self.put_commands_back(&before);
        if self.projects.save_projects(&ids) {
            self.show_success("Project commands put back".into());
        } else {
            self.show_error("gerdr couldn't write the project file".into());
        }
    }

    /// The project files were read again: the dialog shows them, what was
    /// typed staying, or closes with its project.
    pub(crate) fn commands_reloaded(&mut self) {
        let Some(dialog) = self.features.commands.take() else {
            return;
        };
        let (scope, tab, focus, down) = (
            dialog.scope.clone(),
            dialog.tab,
            dialog.focus.clone(),
            dialog.down,
        );
        self.features
            .commands_drafts
            .insert(scope.clone(), dialog.draft());
        self.open_project_commands(&scope.project, scope.slug.as_deref(), tab);
        if let Some(dialog) = self.features.commands.as_mut() {
            dialog.focus = focus;
            dialog.down = down;
        }
    }

    /// A left click while the dialog is open.
    pub fn commands_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.features.commands.as_mut() else {
            return;
        };
        let view = dialog.view();
        let layout = chrome::commands_layout(&view, size);
        let shown = dialog.shown();
        match layout.hit(pos) {
            Some(CommandsHit::Button(COMMANDS_GO)) => self.apply_commands(CommandsOutcome::Go),
            Some(CommandsHit::Button(_)) => self.apply_commands(CommandsOutcome::Close),
            Some(CommandsHit::Tab(tab)) => dialog.set_tab(tab),
            Some(CommandsHit::Field(i)) => {
                if let Some(row) = shown.get(i).map(|&at| &dialog.rows[at]) {
                    dialog.focus = Focus::Row(row.project.clone(), row.key.clone());
                }
            }
            Some(CommandsHit::Use(i)) => {
                if let Some(&at) = shown.get(i) {
                    dialog.use_row(at);
                }
            }
            Some(CommandsHit::Limit) => dialog.focus = Focus::Limit,
            // Off it, it stays: a click away never throws what was typed
            // away.
            None => {}
        }
    }

    /// The pointer moved while the dialog is open.
    pub fn commands_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.features.commands.as_mut() {
            let layout = chrome::commands_layout(&dialog.view(), size);
            dialog.hover = layout.hit(pos);
        }
    }

    /// The wheel over the dialog.
    pub(crate) fn commands_scroll(&mut self, motion: Motion) {
        let rows = self.motion_rows(motion, COMMANDS_NOTCH as f64);
        let size = self.output_size();
        if let Some(dialog) = self.features.commands.as_mut() {
            dialog.scroll(rows as i32 * COMMANDS_NOTCH, size);
        }
    }

    /// What the pointer can press in the dialog.
    pub fn commands_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.commands else {
            return Vec::new();
        };
        chrome::commands_layout(&dialog.view(), self.output_size()).controls()
    }

    /// The dialog's focused field, for the pointer and pastes.
    pub(crate) fn commands_field_mut(&mut self) -> Option<&mut TextField> {
        self.features.commands.as_mut()?.field_mut()
    }

    pub fn commands_view(&self) -> Option<CommandsView> {
        self.features.commands.as_ref().map(CommandsDialog::view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow() -> Workflow {
        let mut w = Workflow::default();
        w.verify.insert(
            "api".into(),
            vec!["cargo test".into(), "cargo clippy".into()],
        );
        w.verify.insert(FEATURE_KEY.into(), vec!["./e2e.sh".into()]);
        w.setup.insert("web".into(), vec!["npm ci".into()]);
        w.verify_timeout = Some(90);
        w
    }

    fn projects() -> Vec<(String, String, Workflow)> {
        vec![("home".into(), "Home".into(), workflow())]
    }

    fn scope() -> CommandsScope {
        CommandsScope {
            project: "home".into(),
            slug: None,
        }
    }

    fn dialog() -> CommandsDialog {
        CommandsDialog::new(1, scope(), &projects(), CommandsTab::Checks)
    }

    fn press(d: &mut CommandsDialog, mods: ModifiersState, sym: Keysym) -> Option<CommandsOutcome> {
        d.key(&mods, sym, sym)
    }

    fn none() -> ModifiersState {
        ModifiersState::default()
    }

    fn shift() -> ModifiersState {
        ModifiersState {
            shift: true,
            ..Default::default()
        }
    }

    fn alt() -> ModifiersState {
        ModifiersState {
            alt: true,
            ..Default::default()
        }
    }

    fn ctrl() -> ModifiersState {
        ModifiersState {
            ctrl: true,
            ..Default::default()
        }
    }

    fn row<'a>(d: &'a mut CommandsDialog, key: &str) -> &'a mut Row {
        d.rows.iter_mut().find(|r| r.key == key).unwrap()
    }

    fn labels(d: &CommandsDialog) -> Vec<String> {
        d.view().rows.iter().map(|r| r.label.clone()).collect()
    }

    fn saved(d: &CommandsDialog, limit: Option<u64>) -> Workflow {
        let mut w = workflow();
        d.apply("home", &mut w, limit);
        w
    }

    #[test]
    fn its_rows_are_what_is_stored_with_the_feature_last() {
        let mut d = dialog();
        assert_eq!(labels(&d), ["api", "web", "Whole feature"]);
        assert_eq!(
            *row(&mut d, "api").checks.field,
            *"cargo test\ncargo clippy"
        );
        assert_eq!(d.view().limit.unwrap().as_str(), "90");
        // Only checks run for the feature as a whole; nor is there a limit
        // on setting up.
        d.set_tab(CommandsTab::Setup);
        assert_eq!(labels(&d), ["api", "web"]);
        assert!(d.view().limit.is_none());
    }

    #[test]
    fn found_repositories_get_rows_and_a_stored_one_stays() {
        let mut d = dialog();
        d.found(
            "home",
            vec![
                ("web".into(), Suggested::default()),
                ("zed".into(), Suggested::default()),
            ],
            &workflow(),
        );
        assert_eq!(labels(&d), ["web", "zed", "api", "Whole feature"]);
        let view = d.view();
        assert_eq!(view.rows[2].tag.as_deref(), Some("not found"));
        assert_eq!(view.rows[2].line.as_str(), "cargo test\ncargo clippy");
        assert_eq!(view.rows[0].tag, None);
    }

    #[test]
    fn a_found_repository_keeps_what_was_typed() {
        let mut d = dialog();
        row(&mut d, "api").checks.field.set("make");
        d.focus = Focus::Row("home".into(), "api".into());
        d.found(
            "home",
            vec![("api".into(), Suggested::default())],
            &workflow(),
        );
        assert_eq!(*row(&mut d, "api").checks.field, *"make");
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
    }

    #[test]
    fn only_the_home_project_has_the_whole_feature_and_the_limit() {
        let projects = vec![
            ("home".into(), "Home".into(), Workflow::default()),
            ("other".into(), "Other".into(), workflow()),
        ];
        let d = CommandsDialog::new(1, scope(), &projects, CommandsTab::Checks);
        let view = d.view();
        let feature: Vec<_> = (d.rows.iter()).filter(|r| r.is_feature()).collect();
        assert_eq!(feature.len(), 1);
        assert_eq!(feature[0].project, "home");
        // Under their own project's heading, its `_feature` not offered.
        assert_eq!(view.rows[0].heading.as_deref(), Some("Home"));
        assert_eq!(view.rows[1].heading.as_deref(), Some("Other"));
        assert_eq!(view.limit.unwrap().as_str(), "");
    }

    #[test]
    fn a_single_project_has_no_headings() {
        assert!(dialog().view().rows.iter().all(|r| r.heading.is_none()));
    }

    #[test]
    fn blank_lines_go_and_an_emptied_field_removes_its_key() {
        let mut d = dialog();
        row(&mut d, "api").checks.field.set("  one  \n\n two\n");
        assert_eq!(saved(&d, Some(90)).verify["api"], ["one", "two"]);
        row(&mut d, "api").checks.field.set("  \n ");
        assert!(!saved(&d, Some(90)).verify.contains_key("api"));
    }

    #[test]
    fn a_field_left_alone_keeps_its_command_as_written() {
        let mut w = workflow();
        w.verify.insert("api".into(), vec!["a &&\nb".into()]);
        let projects = vec![("home".to_owned(), "Home".to_owned(), w.clone())];
        let mut d = CommandsDialog::new(1, scope(), &projects, CommandsTab::Checks);
        let mut after = w.clone();
        d.apply("home", &mut after, Some(90));
        assert_eq!(after, w);
        // Changed, its lines are commands of their own.
        row(&mut d, "api").checks.field.set("a &&\nb\nc");
        d.apply("home", &mut after, Some(90));
        assert_eq!(after.verify["api"], ["a &&", "b", "c"]);
    }

    #[test]
    fn clearing_everything_leaves_an_empty_workflow() {
        let mut d = dialog();
        for key in ["api", FEATURE_KEY] {
            row(&mut d, key).checks.field.clear();
        }
        row(&mut d, "web").setup.as_mut().unwrap().field.clear();
        d.limit.clear();
        assert!(saved(&d, None).is_empty());
    }

    #[test]
    fn the_rest_of_the_workflow_stays() {
        let mut d = dialog();
        row(&mut d, "api").checks.field.set("x");
        let mut w = workflow();
        w.ignore.push("skip".into());
        w.allow_same_reviewer = true;
        d.apply("home", &mut w, Some(90));
        assert_eq!(w.ignore, ["skip"]);
        assert!(w.allow_same_reviewer);
        assert_eq!(w.setup["web"], ["npm ci"]);
    }

    #[test]
    fn the_time_limit_is_whole_seconds_above_zero() {
        assert_eq!(parse_limit(" 90 "), Ok(Some(90)));
        assert_eq!(parse_limit(""), Ok(None));
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("1.5").is_err());
        assert!(parse_limit("soon").is_err());
    }

    #[test]
    fn only_a_project_with_changes_gets_saved() {
        let mut d = dialog();
        assert!(d.changed_projects().is_empty());
        d.limit.set("91");
        assert_eq!(d.changed_projects(), ["home"]);
        assert_eq!(saved(&d, Some(91)).verify_timeout, Some(91));
    }

    #[test]
    fn a_draft_comes_back_over_what_is_stored_now() {
        let mut d = dialog();
        row(&mut d, "api").checks.field.set("typed");
        row(&mut d, "web")
            .setup
            .as_mut()
            .unwrap()
            .field
            .set("typed too");
        d.limit.set("91");
        let draft = d.draft();
        assert_ne!(draft, CommandsDraft::default());
        assert_eq!(dialog().draft(), CommandsDraft::default());

        // The file changed meanwhile: the field left alone follows it, the
        // typed one stays, and saving writes only that.
        let mut w = workflow();
        w.verify.insert(FEATURE_KEY.into(), vec!["./new.sh".into()]);
        let projects = vec![("home".to_owned(), "Home".to_owned(), w)];
        let mut back = CommandsDialog::new(2, scope(), &projects, CommandsTab::Checks);
        back.restore(draft, &projects);
        assert_eq!(*row(&mut back, "api").checks.field, *"typed");
        assert_eq!(*row(&mut back, FEATURE_KEY).checks.field, *"./new.sh");
        assert_eq!(*back.limit, *"91");
        assert!(!row(&mut back, FEATURE_KEY).checks.changed());
    }

    #[test]
    fn a_suggestion_shows_on_an_empty_field_and_fills_it() {
        let mut d = dialog();
        let suggested = Suggested {
            checks: vec!["a".into(), "b".into()],
            setup: vec!["c".into()],
            ..Suggested::default()
        };
        d.found("home", vec![("zed".into(), suggested)], &workflow());
        d.focus = Focus::Row("home".into(), "zed".into());
        let at = |d: &CommandsDialog| {
            (d.view().rows.into_iter())
                .find(|r| r.label == "zed")
                .unwrap()
        };
        assert_eq!(at(&d).suggestion.as_deref(), Some("a, b"));
        assert!(d.view().keys.contains(&("Alt+U", "use")));
        press(&mut d, alt(), Keysym::u);
        assert_eq!(at(&d).line.as_str(), "a\nb");
        // Filled, it offers nothing, and isn't overwritten.
        assert_eq!(at(&d).suggestion, None);
        row(&mut d, "zed").checks.field.set("mine");
        press(&mut d, alt(), Keysym::u);
        assert_eq!(at(&d).line.as_str(), "mine");
        // It isn't saved until Save.
        assert!(!workflow().verify.contains_key("zed"));
        // The other tab has its own.
        d.set_tab(CommandsTab::Setup);
        assert_eq!(at(&d).suggestion.as_deref(), Some("c"));
    }

    #[test]
    fn enter_saves_and_shift_enter_breaks_the_line() {
        let mut d = dialog();
        d.focus = Focus::Row("home".into(), "web".into());
        assert_eq!(press(&mut d, shift(), Keysym::Return), None);
        assert_eq!(*row(&mut d, "web").checks.field, *"\n");
        assert_eq!(
            press(&mut d, none(), Keysym::Return),
            Some(CommandsOutcome::Go)
        );
        assert_eq!(
            press(&mut d, none(), Keysym::Escape),
            Some(CommandsOutcome::Close)
        );
        // The time limit is one line.
        d.focus = Focus::Limit;
        press(&mut d, shift(), Keysym::Return);
        assert_eq!(*d.limit, *"90");
    }

    #[test]
    fn tab_and_arrows_go_round_the_fields() {
        let mut d = dialog();
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        press(&mut d, none(), Keysym::Tab);
        assert_eq!(d.focus, Focus::Row("home".into(), "web".into()));
        press(&mut d, shift(), Keysym::ISO_Left_Tab);
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        press(&mut d, ctrl(), Keysym::j);
        press(&mut d, ctrl(), Keysym::j);
        press(&mut d, ctrl(), Keysym::j);
        assert_eq!(d.focus, Focus::Limit);
        press(&mut d, none(), Keysym::Tab);
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        // Up and Down stay in a field of lines until its end.
        press(&mut d, none(), Keysym::Up);
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        press(&mut d, none(), Keysym::Down);
        press(&mut d, none(), Keysym::Up);
        press(&mut d, none(), Keysym::Up);
        assert_eq!(d.focus, Focus::Limit);
    }

    #[test]
    fn ctrl_page_keys_change_tab_and_focus_follows() {
        let mut d = dialog();
        d.focus = Focus::Limit;
        press(&mut d, ctrl(), Keysym::Page_Down);
        assert_eq!(d.tab, CommandsTab::Setup);
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        press(&mut d, ctrl(), Keysym::Page_Up);
        assert_eq!(d.tab, CommandsTab::Checks);
    }

    #[test]
    fn typing_clears_the_form_error() {
        let mut d = dialog();
        d.error = Some("wrong".into());
        press(&mut d, none(), Keysym::a);
        assert_eq!(d.error, None);
    }

    #[test]
    fn what_is_saved_runs_only_for_repositories_the_feature_touches() {
        use crate::feature::{
            PerProject,
            tests::{Temp, feature_over},
            verify,
        };
        let (a, b) = (Temp::new(), Temp::new());
        let touched = feature_over(&a, &["api"], &[]);
        let untouched = feature_over(&b, &["api"], &["api"]);
        let mut d = CommandsDialog::new(
            1,
            scope(),
            &[("home".into(), "Home".into(), Workflow::default())],
            CommandsTab::Checks,
        );
        d.found(
            "home",
            vec![("api".into(), Suggested::default())],
            &Workflow::default(),
        );
        row(&mut d, "api").checks.field.set("cargo test");
        let mut w = Workflow::default();
        d.apply("home", &mut w, None);
        let config = PerProject::all(w.verify);
        assert_eq!(verify::checks(&touched, &config).len(), 1);
        assert!(verify::checks(&untouched, &config).is_empty());
    }

    #[test]
    fn typing_doesnt_hide_that_the_file_wants_reloading() {
        let mut d = dialog();
        d.stale = true;
        d.error = Some("wrong".into());
        press(&mut d, none(), Keysym::a);
        assert_eq!(d.error.as_deref(), Some(STALE));
    }

    #[test]
    fn found_repositories_give_a_field_to_type_in() {
        // No repository known, on Setup: nothing has the keyboard.
        let empty = [("home".to_owned(), "Home".to_owned(), Workflow::default())];
        let mut d = CommandsDialog::new(1, scope(), &empty, CommandsTab::Setup);
        assert!(d.field_mut().is_none());
        d.found(
            "home",
            vec![("api".into(), Suggested::default())],
            &Workflow::default(),
        );
        assert_eq!(d.focus, Focus::Row("home".into(), "api".into()));
        assert!(d.field_mut().is_some());
        // On Checks, a time limit gone to on purpose stays.
        let mut d = CommandsDialog::new(1, scope(), &empty, CommandsTab::Checks);
        d.focus = Focus::Limit;
        d.found(
            "home",
            vec![("api".into(), Suggested::default())],
            &Workflow::default(),
        );
        assert_eq!(d.focus, Focus::Limit);
    }

    #[test]
    fn a_view_says_how_looking_went() {
        let mut d = dialog();
        d.looking = 1;
        assert_eq!(d.view().note.unwrap().0, "finding repositories\u{2026}");
        d.looking = 0;
        d.failed = Some("no git".into());
        assert_eq!(
            d.view().note.unwrap(),
            ("Couldn't look for repositories: no git".to_owned(), true)
        );
        d.failed = None;
        assert_eq!(d.view().note, None);
        // Setup with no repositories known: nothing shows, and it says why
        // rather than looking finished with none.
        let mut empty = CommandsDialog::new(
            1,
            scope(),
            &[("home".into(), "Home".into(), Workflow::default())],
            CommandsTab::Setup,
        );
        assert_eq!(
            empty.view().note.unwrap().0,
            "No repositories found in this project"
        );
        empty.nowhere = true;
        assert!(empty.view().note.unwrap().0.starts_with("Add a workspace"));
    }
}
