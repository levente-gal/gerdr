//! The project menu: a dropdown from the sidebar header, while a project is
//! shown, to find, switch to, start, rename, stop and forget projects.
//!
//! Typing filters the projects; under them, rows start a new project, named
//! as typed or in a folder zoxide knows that matches. Enter opens the
//! selected entry, Alt+1 to 9 a recent project, Ctrl+N the form for a new
//! one, Ctrl+R renames the selected project in place, Ctrl+S stops its
//! session, Delete twice forgets it. The form asks for the name, an
//! optional folder (typed, picked from zoxide's, or browsed for) and what
//! the folder opens as. Esc backs out one step at a time.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr, WorkspaceKind,
    chrome::{
        self, FormField, FormView, MenuEntry, MenuHit, ProjectMenuView, ProjectRow, RowButton,
    },
    fuzzy, home_dir,
    project_list::{self, scrolled},
    tilde,
};

/// How many of zoxide's folders the list offers as new projects.
const FOLDER_ROWS: usize = 3;
/// How many of zoxide's folders the form's folder field suggests.
const SUGGESTIONS: usize = 5;

/// The project menu's state, while it is open.
#[derive(Debug, Default)]
pub struct ProjectMenu {
    /// What the pointer is over.
    pub hover: Option<MenuHit>,
    /// The wheel's turn not yet a whole row, in rows.
    wheel: f64,
    /// What was typed to filter the projects.
    query: String,
    /// The projects, by id, most recently used first.
    projects: Vec<(String, ProjectRow)>,
    /// The folders zoxide knows, the highest ranked first.
    recent: Vec<PathBuf>,
    /// The folders a project already has a workspace in.
    taken: HashSet<PathBuf>,
    /// The home folder, written "~".
    home: Option<PathBuf>,
    /// The list's entries, in order.
    items: Vec<Item>,
    /// zoxide's folders the query matches, best first, for `Item::Folder`.
    folders: Vec<PathBuf>,
    /// The selected entry, by index into `items`; never a section.
    selected: usize,
    /// The first entry shown.
    scroll: usize,
    /// The project being renamed in place.
    renaming: Option<Renaming>,
    /// The project Delete was pressed on once, by id.
    confirm_forget: Option<String>,
    /// The form for a new project, while it replaces the list.
    form: Option<Form>,
}

/// An entry of the list, pointing into the menu's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// A section's label.
    Section(&'static str),
    /// A project, by index into `ProjectMenu::projects`.
    Project(usize),
    /// The row to start a new project.
    New,
    /// A folder, by index into `ProjectMenu::folders`.
    Folder(usize),
}

/// What an entry stands for, to find it again once the list changes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picked {
    Project(String),
    New,
    Folder(PathBuf),
}

/// A project's name being edited in place.
#[derive(Clone, Debug)]
struct Renaming {
    id: String,
    /// The name typed so far.
    name: String,
    /// The old name is still all selected: what is typed replaces it.
    whole: bool,
}

/// The form for a new project.
#[derive(Clone, Debug)]
struct Form {
    name: String,
    /// The folder as typed; empty for none.
    folder: String,
    kind: WorkspaceKind,
    focus: FormField,
    /// The name is the folder's: the user hasn't typed one.
    name_follows: bool,
    /// The folders offered for what was typed in the folder field; empty
    /// when the list is closed.
    suggestions: Vec<String>,
    /// The suggestion Up/Down moved to.
    suggestion: Option<usize>,
    error: Option<String>,
}

/// What the project menu asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Show project `id`.
    Switch(String),
    /// Start a new project and show it, opening `folder` in it as `kind`.
    Create {
        name: String,
        folder: Option<PathBuf>,
        kind: WorkspaceKind,
    },
    /// Rename project `id` (the first) to the second.
    Rename(String, String),
    /// Stop project `id`'s session; not the shown one.
    Stop(String),
    /// Forget project `id`; not the shown one.
    Forget(String),
    /// Ask zenity for the form's folder, starting in this one.
    Browse(Option<PathBuf>),
    /// Say this, keeping the menu as it is.
    Notice(String),
    /// Close the menu.
    Closed,
}

impl ProjectMenu {
    /// An empty menu; `home` is written "~".
    fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            ..Self::default()
        }
    }

    /// Replace the projects (most recently used first) and the folders
    /// zoxide knows (`taken` are those a project has), keeping the query,
    /// and the selection on the same entry if it is still there.
    fn set_lists(
        &mut self,
        projects: Vec<(String, ProjectRow)>,
        recent: Vec<PathBuf>,
        taken: HashSet<PathBuf>,
    ) {
        let picked = self.items.get(self.selected).map(|&item| self.picked(item));
        let was = self.selected;
        self.projects = projects;
        self.recent = recent;
        self.taken = taken;
        self.rebuild();
        let has = |id: &Option<String>| {
            id.as_ref()
                .is_some_and(|id| self.projects.iter().any(|(p, _)| p == id))
        };
        if !has(&self.confirm_forget) {
            self.confirm_forget = None;
        }
        if !has(&self.renaming.as_ref().map(|r| r.id.clone())) {
            self.renaming = None;
        }
        if let Some(form) = self.form.as_mut()
            && !form.suggestions.is_empty()
        {
            form.suggest(&self.recent, self.home.as_deref());
        }
        match picked {
            None => self.select_first(),
            Some(picked) => {
                match self
                    .items
                    .iter()
                    .position(|&item| self.picked(item) == picked)
                {
                    Some(i) => self.selected = i,
                    // It went: select the entry in its place.
                    None => self.selected = self.selectable_near(was),
                }
            }
        }
    }

    /// Build the list's entries again, for the query.
    fn rebuild(&mut self) {
        let matches = project_list::matches(&self.query, &self.projects);
        self.folders = if self.query.trim().is_empty() {
            Vec::new()
        } else {
            best_dirs(
                &self.query,
                self.recent.iter().filter(|dir| !self.taken.contains(*dir)),
                self.home.as_deref(),
                FOLDER_ROWS,
            )
        };
        let (current, recent): (Vec<usize>, Vec<usize>) = matches
            .into_iter()
            .partition(|&m| self.projects[m].1.current);
        let mut items = Vec::new();
        if !current.is_empty() {
            items.push(Item::Section("Current"));
            items.extend(current.into_iter().map(Item::Project));
        }
        if !recent.is_empty() {
            items.push(Item::Section("Recent"));
            items.extend(recent.into_iter().map(Item::Project));
        }
        items.push(Item::New);
        items.extend((0..self.folders.len()).map(Item::Folder));
        self.items = items;
    }

    /// Build the list again for a changed query, selecting the first entry.
    fn refilter(&mut self) {
        self.rebuild();
        self.select_first();
    }

    /// Select the first recent project, else the shown one, else the
    /// button to start one.
    fn select_first(&mut self) {
        let position =
            |want: fn(&Self, Item) -> bool| self.items.iter().position(|&item| want(self, item));
        self.selected =
            position(|menu, item| matches!(item, Item::Project(p) if !menu.projects[p].1.current))
                .or_else(|| position(|_, item| matches!(item, Item::Project(_))))
                .or_else(|| position(|_, item| item == Item::New))
                .unwrap_or(0);
        self.scroll = 0;
    }

    /// What `item` stands for.
    fn picked(&self, item: Item) -> Picked {
        match item {
            Item::Project(p) => Picked::Project(self.projects[p].0.clone()),
            Item::Folder(f) => Picked::Folder(self.folders[f].clone()),
            Item::New | Item::Section(_) => Picked::New,
        }
    }

    /// The selectable entry at or after `i`, else the last one before it.
    fn selectable_near(&self, i: usize) -> usize {
        let selectable = |&j: &usize| !matches!(self.items[j], Item::Section(_));
        (i..self.items.len())
            .find(selectable)
            .or_else(|| (0..i.min(self.items.len())).rev().find(selectable))
            .unwrap_or(0)
    }

    /// The selectable entries' indices, in order.
    fn selectable(&self) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&i| !matches!(self.items[i], Item::Section(_)))
            .collect()
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` the one with
    /// Shift and the layout applied. `page` is how many entries are shown.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        page: usize,
    ) -> Option<Outcome> {
        if sym.is_modifier_key() {
            return None;
        }
        if self.form.is_some() {
            return self.form_key(mods, sym, typed);
        }
        // Any key but Delete again takes back a first Delete.
        let armed = self.confirm_forget.take();
        if self.renaming.is_some() {
            return self.rename_key(mods, sym, typed);
        }
        let ctrl = mods.ctrl;
        match sym {
            Keysym::Escape => {
                if armed.is_some() {
                    // Esc took back the Delete.
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.refilter();
                } else {
                    return Some(Outcome::Closed);
                }
            }
            Keysym::Return | Keysym::KP_Enter => return self.enter(self.selected),
            Keysym::n if ctrl => self.open_form(),
            Keysym::r if ctrl => self.start_renaming(self.selected),
            Keysym::s if ctrl => return self.stop(self.selected),
            Keysym::Delete | Keysym::KP_Delete => return self.forget(self.selected, armed),
            Keysym::Up | Keysym::KP_Up => self.step(-1, page),
            Keysym::Down | Keysym::KP_Down => self.step(1, page),
            Keysym::Page_Up | Keysym::KP_Page_Up => self.step(-(page.max(1) as isize), page),
            Keysym::Page_Down | Keysym::KP_Page_Down => self.step(page.max(1) as isize, page),
            Keysym::Home | Keysym::KP_Home => self.step(isize::MIN / 2, page),
            Keysym::End | Keysym::KP_End => self.step(isize::MAX / 2, page),
            _ if mods.alt && !ctrl && !mods.logo => {
                let digit = sym.key_char().and_then(|c| c.to_digit(10))?;
                return self.digit_project(digit).map(Outcome::Switch);
            }
            _ => {
                if type_in(&mut self.query, mods, sym, typed) {
                    self.refilter();
                }
            }
        }
        None
    }

    /// A key while a project is being renamed.
    fn rename_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<Outcome> {
        let renaming = self.renaming.as_mut()?;
        match sym {
            Keysym::Escape => self.renaming = None,
            Keysym::Return | Keysym::KP_Enter => {
                let name = renaming.name.trim();
                let taken = self.projects.iter().any(|(p, row)| {
                    *p != renaming.id && row.name.to_lowercase() == name.to_lowercase()
                });
                // The field stays open to pick another.
                if taken {
                    return Some(Outcome::Notice(format!("A project named {name} exists.")));
                }
                let Renaming { id, name, .. } = self.renaming.take()?;
                let name = name.trim();
                let old = self.projects.iter().find(|(p, _)| *p == id)?;
                if !name.is_empty() && name != old.1.name {
                    return Some(Outcome::Rename(id, name.to_owned()));
                }
            }
            // Moving the caret lets go of the old name.
            Keysym::Left | Keysym::Right | Keysym::Home | Keysym::End => renaming.whole = false,
            _ if renaming.whole => {
                let mut name = String::new();
                if type_in(&mut name, mods, sym, typed) {
                    renaming.name = name;
                    renaming.whole = false;
                }
            }
            _ => {
                type_in(&mut renaming.name, mods, sym, typed);
            }
        }
        None
    }

    /// Enter on entry `i`: open a project, or the form for a new one.
    fn enter(&mut self, i: usize) -> Option<Outcome> {
        match *self.items.get(i)? {
            Item::Section(_) => None,
            Item::Project(p) => {
                let (id, row) = &self.projects[p];
                Some(if row.current {
                    Outcome::Closed
                } else {
                    Outcome::Switch(id.clone())
                })
            }
            Item::New => {
                self.open_form();
                None
            }
            Item::Folder(f) => {
                let folder = tilde(&self.folders[f], self.home.as_deref());
                let name = basename(&folder, self.home.as_deref());
                self.hover = None;
                self.form = Some(Form {
                    name,
                    folder,
                    name_follows: true,
                    ..Form::new(FormField::Folder)
                });
                None
            }
        }
    }

    /// Open the form, named as the query.
    fn open_form(&mut self) {
        let name = self.query.trim().to_owned();
        self.hover = None;
        self.form = Some(Form {
            name_follows: name.is_empty(),
            name,
            ..Form::new(FormField::Name)
        });
    }

    /// Start renaming entry `i`, if it is a project.
    fn start_renaming(&mut self, i: usize) {
        if let Some(Item::Project(p)) = self.items.get(i) {
            let (id, row) = &self.projects[*p];
            self.renaming = Some(Renaming {
                id: id.clone(),
                name: row.name.clone(),
                whole: true,
            });
        }
    }

    /// Stop entry `i`'s project, if it is one other than the shown one and
    /// may be running.
    fn stop(&self, i: usize) -> Option<Outcome> {
        let Item::Project(p) = *self.items.get(i)? else {
            return None;
        };
        let (id, row) = &self.projects[p];
        can_stop(row).then(|| Outcome::Stop(id.clone()))
    }

    /// Delete (or the forget button) on entry `i`'s project: the second time
    /// in a row (`armed` is the first's) forgets it. Not the shown one.
    fn forget(&mut self, i: usize, armed: Option<String>) -> Option<Outcome> {
        let Item::Project(p) = *self.items.get(i)? else {
            return None;
        };
        let (id, row) = &self.projects[p];
        if row.current {
            // The project shown can't be forgotten here.
        } else if armed.as_ref() == Some(id) {
            return Some(Outcome::Forget(id.clone()));
        } else {
            self.confirm_forget = Some(id.clone());
        }
        None
    }

    /// The project Alt+`digit` opens: the `digit`th recent one.
    fn digit_project(&self, digit: u32) -> Option<String> {
        let n = usize::try_from(digit)
            .ok()
            .filter(|d| (1..=9).contains(d))?;
        self.items
            .iter()
            .filter_map(|&item| match item {
                Item::Project(p) if !self.projects[p].1.current => Some(&self.projects[p].0),
                _ => None,
            })
            .nth(n - 1)
            .cloned()
    }

    /// Move the selection `by` places among the selectable entries,
    /// scrolling to keep it shown.
    fn step(&mut self, by: isize, page: usize) {
        let selectable = self.selectable();
        let Some(at) = selectable.iter().position(|&i| i == self.selected) else {
            return;
        };
        let to = (at as isize)
            .saturating_add(by)
            .clamp(0, selectable.len() as isize - 1);
        self.select(selectable[to as usize], page);
    }

    /// Select entry `i`, scrolling to keep it shown, with its section's
    /// label when it is the section's first.
    fn select(&mut self, i: usize, page: usize) {
        self.selected = i;
        let mut scroll = scrolled(self.scroll, i, page.max(1));
        if scroll == i && i > 0 && page > 1 && matches!(self.items[i - 1], Item::Section(_)) {
            scroll = i - 1;
        }
        self.scroll = scroll;
    }

    /// The first entry shown.
    fn first_shown(&self, page: usize) -> usize {
        let page = page.max(1);
        scrolled(self.scroll, self.selected, page).min(self.items.len().saturating_sub(page))
    }

    /// Scroll the list `rows` entries (down positive), moving the selection
    /// only as far as needed to keep it shown.
    fn scroll_by(&mut self, rows: i32, page: usize) {
        if self.form.is_some() {
            return;
        }
        let page = page.max(1);
        let last = self.items.len().saturating_sub(page);
        let first = self
            .first_shown(page)
            .saturating_add_signed(rows as isize)
            .min(last);
        let end = (first + page).min(self.items.len());
        let selectable = |&i: &usize| !matches!(self.items[i], Item::Section(_));
        let selected = if self.selected < first {
            (first..end).find(selectable)
        } else if self.selected >= end {
            (first..end).rev().find(selectable)
        } else {
            Some(self.selected)
        };
        if let Some(selected) = selected {
            self.selected = selected;
            self.scroll = first;
        }
    }

    /// A click on `hit`; `page` is how many entries are shown.
    fn click(&mut self, hit: MenuHit, page: usize) -> Option<Outcome> {
        if self.form.is_some() {
            return self.form_click(hit);
        }
        let armed = self.confirm_forget.take();
        let first = self.first_shown(page);
        let entry = |i: usize| (first + i < self.items.len()).then_some(first + i);
        // A click anywhere but in the name being edited ends the renaming.
        let renamed = self.renaming.as_ref().map(|r| r.id.clone());
        let on_renamed = |menu: &Self, i: usize| match menu.items[i] {
            Item::Project(p) => renamed.as_ref() == Some(&menu.projects[p].0),
            _ => false,
        };
        match hit {
            MenuHit::Entry(i) => {
                let i = entry(i)?;
                if on_renamed(self, i) {
                    return None;
                }
                self.renaming = None;
                if matches!(self.items[i], Item::Section(_)) {
                    return None;
                }
                self.select(i, page);
                self.enter(i)
            }
            MenuHit::Button(i, button) => {
                let i = entry(i)?;
                if !on_renamed(self, i) {
                    self.renaming = None;
                }
                self.select(i, page);
                match button {
                    RowButton::Rename => {
                        self.start_renaming(i);
                        None
                    }
                    RowButton::Stop => self.stop(i),
                    RowButton::Forget => self.forget(i, armed),
                }
            }
            MenuHit::Query
            | MenuHit::Panel
            | MenuHit::Field(_)
            | MenuHit::Kind(_)
            | MenuHit::Suggestion(_) => {
                self.renaming = None;
                None
            }
        }
    }

    /// A key while the form is shown.
    fn form_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<Outcome> {
        let form = self.form.as_mut()?;
        let ctrl = mods.ctrl;
        match sym {
            Keysym::Escape => {
                if form.suggestions.is_empty() {
                    self.form = None;
                } else {
                    form.close_suggestions();
                }
            }
            Keysym::o if ctrl => return Some(Outcome::Browse(form.start(self.home.as_deref()))),
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let back =
                    sym == Keysym::ISO_Left_Tab || typed == Keysym::ISO_Left_Tab || mods.shift;
                form.tab(back);
            }
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(s) = form.suggestion {
                    form.take_suggestion(s, self.home.as_deref());
                    return None;
                }
                return match form.focus {
                    FormField::Browse => Some(Outcome::Browse(form.start(self.home.as_deref()))),
                    FormField::Cancel => {
                        self.form = None;
                        None
                    }
                    _ => self.create(),
                };
            }
            Keysym::Up | Keysym::KP_Up if !form.suggestions.is_empty() => {
                form.suggestion = form.suggestion.and_then(|s| s.checked_sub(1));
            }
            Keysym::Down | Keysym::KP_Down if !form.suggestions.is_empty() => {
                form.suggestion = Some(
                    form.suggestion
                        .map_or(0, |s| s + 1)
                        .min(form.suggestions.len() - 1),
                );
            }
            Keysym::Left | Keysym::Right | Keysym::space
                if form.focus == FormField::Kind && !ctrl && !mods.alt && !mods.logo =>
            {
                form.toggle_kind();
            }
            _ => match form.focus {
                FormField::Name if type_in(&mut form.name, mods, sym, typed) => {
                    form.name_follows = form.name.trim().is_empty();
                    form.error = None;
                }
                FormField::Folder if type_in(&mut form.folder, mods, sym, typed) => {
                    form.folder_changed(self.home.as_deref());
                    form.suggest(&self.recent, self.home.as_deref());
                }
                _ => {}
            },
        }
        None
    }

    /// A click on `hit` while the form is shown.
    fn form_click(&mut self, hit: MenuHit) -> Option<Outcome> {
        let form = self.form.as_mut()?;
        match hit {
            MenuHit::Field(FormField::Browse) => {
                form.focus = FormField::Browse;
                form.close_suggestions();
                Some(Outcome::Browse(form.start(self.home.as_deref())))
            }
            MenuHit::Field(FormField::Cancel) => {
                self.form = None;
                None
            }
            MenuHit::Field(FormField::Create) => {
                form.focus = FormField::Create;
                form.close_suggestions();
                self.create()
            }
            MenuHit::Field(FormField::Kind) if form.folder.trim().is_empty() => None,
            MenuHit::Field(field) => {
                if form.focus != field {
                    form.focus = field;
                    form.close_suggestions();
                }
                None
            }
            MenuHit::Kind(kind) => {
                if !form.folder.trim().is_empty() {
                    form.kind = kind;
                    form.focus = FormField::Kind;
                    form.close_suggestions();
                }
                None
            }
            MenuHit::Suggestion(s) => {
                if s < form.suggestions.len() {
                    form.take_suggestion(s, self.home.as_deref());
                }
                None
            }
            MenuHit::Query | MenuHit::Panel | MenuHit::Entry(_) | MenuHit::Button(..) => None,
        }
    }

    /// Create the project the form describes, or say on its error line why
    /// it can't be.
    fn create(&mut self) -> Option<Outcome> {
        let form = self.form.as_mut()?;
        let name = form.name.trim();
        let folder = form.folder.trim();
        let (error, focus) = if name.is_empty() {
            ("Give the project a name.".to_owned(), FormField::Name)
        } else if self
            .projects
            .iter()
            .any(|(_, row)| row.name.trim().to_lowercase() == name.to_lowercase())
        {
            (format!("A project named {name} exists."), FormField::Name)
        } else {
            match expand(folder, self.home.as_deref()) {
                Some(dir) if !dir.is_dir() => {
                    (format!("No folder at {folder}."), FormField::Folder)
                }
                dir => {
                    return Some(Outcome::Create {
                        name: name.to_owned(),
                        folder: dir,
                        kind: form.kind,
                    });
                }
            }
        };
        form.error = Some(error);
        form.focus = focus;
        form.close_suggestions();
        None
    }

    /// zenity gave back `dir` for the form's folder.
    fn browsed(&mut self, dir: &Path) {
        if let Some(form) = self.form.as_mut() {
            form.folder = tilde(dir, self.home.as_deref());
            form.folder_changed(self.home.as_deref());
            form.close_suggestions();
        }
    }

    /// What is shown: `page` entries at most; `running` says whose sessions
    /// run, by id.
    fn view(&self, page: usize, running: &HashMap<String, bool>) -> ProjectMenuView {
        if let Some(form) = &self.form {
            return ProjectMenuView {
                query: String::new(),
                entries: Vec::new(),
                selected: None,
                hover: self.hover,
                form: Some(form.view()),
                hints: hints(&[
                    ("Tab", "next"),
                    ("Ctrl+O", "browse"),
                    ("Enter", "create"),
                    ("Esc", "back"),
                ]),
            };
        }
        let first = self.first_shown(page);
        let end = (first + page.max(1)).min(self.items.len());
        // The recent projects are numbered from the top, shown or not.
        let mut recent: u8 = 0;
        let entries = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, &item)| {
                if let Item::Project(p) = item
                    && !self.projects[p].1.current
                {
                    recent = recent.saturating_add(1);
                }
                if !(first..end).contains(&i) {
                    return None;
                }
                Some(match item {
                    Item::Section(label) => MenuEntry::Section(label.into()),
                    Item::Project(p) => {
                        let (id, row) = &self.projects[p];
                        let digit = (!row.current && recent <= 9).then_some(recent);
                        MenuEntry::Project {
                            row: ProjectRow {
                                running: running.get(id).copied().or(row.running),
                                ..row.clone()
                            },
                            digit,
                            renaming: self
                                .renaming
                                .as_ref()
                                .filter(|r| r.id == *id)
                                .map(|r| r.name.clone()),
                            name_selected: self
                                .renaming
                                .as_ref()
                                .is_some_and(|r| r.id == *id && r.whole),
                            confirm_forget: self.confirm_forget.as_ref() == Some(id),
                        }
                    }
                    Item::New => MenuEntry::New(match self.query.trim() {
                        "" => "New project…".into(),
                        query => format!("New project \"{query}\"…"),
                    }),
                    Item::Folder(f) => {
                        MenuEntry::Folder(tilde(&self.folders[f], self.home.as_deref()))
                    }
                })
            })
            .collect();
        let hints = if self.renaming.is_some() {
            hints(&[("Enter", "save"), ("Esc", "cancel")])
        } else {
            hints(&[
                ("Enter", "open"),
                ("Ctrl+N", "new"),
                ("Ctrl+R", "rename"),
                ("Ctrl+S", "stop"),
                ("Del", "forget"),
                ("Esc", "close"),
            ])
        };
        ProjectMenuView {
            query: self.query.clone(),
            entries,
            selected: self.selected.checked_sub(first).filter(|i| first + i < end),
            hover: self.hover,
            form: None,
            hints,
        }
    }
}

impl Form {
    /// An empty form, with `focus`.
    fn new(focus: FormField) -> Self {
        Self {
            name: String::new(),
            folder: String::new(),
            kind: WorkspaceKind::Project,
            focus,
            name_follows: true,
            suggestions: Vec::new(),
            suggestion: None,
            error: None,
        }
    }

    /// Move the focus to the next field, or the one before if `back`; with
    /// no folder, past the choice of what it opens as.
    fn tab(&mut self, back: bool) {
        const ORDER: [FormField; 6] = [
            FormField::Name,
            FormField::Folder,
            FormField::Browse,
            FormField::Kind,
            FormField::Cancel,
            FormField::Create,
        ];
        let at = ORDER.iter().position(|f| *f == self.focus).unwrap_or(0);
        let step = if back { ORDER.len() - 1 } else { 1 };
        let mut to = (at + step) % ORDER.len();
        if ORDER[to] == FormField::Kind && self.folder.trim().is_empty() {
            to = (to + step) % ORDER.len();
        }
        self.focus = ORDER[to];
        self.close_suggestions();
    }

    /// Open the folder as the other kind.
    fn toggle_kind(&mut self) {
        self.kind = match self.kind {
            WorkspaceKind::Project => WorkspaceKind::Terminal,
            WorkspaceKind::Terminal => WorkspaceKind::Project,
        };
    }

    /// The folder changed: the name follows it, if it still does.
    fn folder_changed(&mut self, home: Option<&Path>) {
        if self.name_follows {
            self.name = basename(&self.folder, home);
        }
        self.error = None;
    }

    /// Offer zoxide's folders (`recent`) that match what was typed in the
    /// folder field.
    fn suggest(&mut self, recent: &[PathBuf], home: Option<&Path>) {
        let chosen = self
            .suggestion
            .and_then(|s| self.suggestions.get(s))
            .cloned();
        let typed = self.folder.trim();
        self.suggestions = if typed.is_empty() {
            Vec::new()
        } else {
            // A path typed in full matches as zoxide's are written.
            let typed = tilde(Path::new(typed), home);
            best_dirs(&typed, recent.iter(), home, SUGGESTIONS)
                .iter()
                .map(|dir| tilde(dir, home))
                .collect()
        };
        // The one moved to stays chosen if it is still offered.
        self.suggestion = chosen.and_then(|c| self.suggestions.iter().position(|s| *s == c));
    }

    /// Close the list of suggested folders.
    fn close_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggestion = None;
    }

    /// Fill the folder field with suggestion `s`.
    fn take_suggestion(&mut self, s: usize, home: Option<&Path>) {
        if let Some(folder) = self.suggestions.get(s) {
            self.folder = folder.clone();
            self.folder_changed(home);
        }
        self.close_suggestions();
    }

    /// Where Browse starts: the typed folder if there is one.
    fn start(&self, home: Option<&Path>) -> Option<PathBuf> {
        expand(self.folder.trim(), home).filter(|dir| dir.is_dir())
    }

    /// What the form shows.
    fn view(&self) -> FormView {
        FormView {
            name: self.name.clone(),
            folder: self.folder.clone(),
            kind: self.kind,
            focus: self.focus,
            suggestions: self.suggestions.clone(),
            suggestion: self.suggestion,
            error: self.error.clone(),
        }
    }
}

/// Whether the menu offers to stop `row`'s session: not the shown one's, nor
/// one known to be stopped.
fn can_stop(row: &ProjectRow) -> bool {
    !row.current && row.running != Some(false)
}

/// Edit `text` as `welcome` does: Backspace deletes a character, Ctrl+W (or
/// Ctrl+Backspace) a word, Ctrl+U all of it, and a key that types a
/// character adds it. `false` for any other key, which leaves it alone.
fn type_in(text: &mut String, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> bool {
    let ctrl = mods.ctrl;
    let change: Box<dyn FnOnce(&mut String)> = match sym {
        Keysym::BackSpace | Keysym::w if ctrl => Box::new(|text: &mut String| {
            let word = text
                .trim_end()
                .rfind(char::is_whitespace)
                .map_or(0, |i| i + 1);
            text.truncate(word);
        }),
        Keysym::u if ctrl => Box::new(String::clear),
        Keysym::BackSpace => Box::new(|text: &mut String| {
            text.pop();
        }),
        _ => match typed
            .key_char()
            .filter(|c| !c.is_control() && !(ctrl || mods.alt || mods.logo))
        {
            Some(c) => Box::new(move |text: &mut String| text.push(c)),
            None => return false,
        },
    };
    change(text);
    true
}

/// The folder `typed` names, `~` being `home`, and a relative one taken from
/// `home`; `None` when nothing was typed.
fn expand(typed: &str, home: Option<&Path>) -> Option<PathBuf> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let Some(home) = home else {
        return Some(PathBuf::from(typed));
    };
    Some(match typed.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => home.join(typed),
    })
}

/// The last part of the folder `typed`, for a project's name.
fn basename(typed: &str, home: Option<&Path>) -> String {
    expand(typed, home)
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// The `max` folders of `dirs` that `query` matches best, written "~/…",
/// best first; equal scores keep their order.
fn best_dirs<'a>(
    query: &str,
    dirs: impl Iterator<Item = &'a PathBuf>,
    home: Option<&Path>,
    max: usize,
) -> Vec<PathBuf> {
    let mut found: Vec<(i32, &PathBuf)> = dirs
        .filter_map(|dir| Some((fuzzy::score(query, &tilde(dir, home))?.0, dir)))
        .collect();
    // A stable sort.
    found.sort_by_key(|(score, _)| Reverse(*score));
    found
        .into_iter()
        .take(max)
        .map(|(_, dir)| dir.clone())
        .collect()
}

/// The footer's hints, owned.
fn hints(hints: &[(&str, &str)]) -> Vec<(String, String)> {
    hints
        .iter()
        .map(|(key, label)| ((*key).to_owned(), (*label).to_owned()))
        .collect()
}

impl Gerdr {
    /// Open the project menu, and find out which projects' sessions run.
    pub fn open_project_menu(&mut self) {
        self.switcher = None;
        self.stop_switcher_repeat();
        self.terminal_menu = false;
        self.menu_hover = None;
        self.project_menu = Some(ProjectMenu::new(home_dir()));
        self.refresh_project_menu();
        self.check_running();
        self.query_zoxide();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Open the project menu, or close it if it is open.
    pub fn toggle_project_menu(&mut self) {
        if self.project_menu.is_some() {
            self.apply_project_menu(Outcome::Closed);
        } else {
            self.open_project_menu();
        }
    }

    /// A key while the project menu has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn project_menu_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let page = self.project_menu_page();
        self.project_menu.as_mut()?.key(mods, sym, typed, page)
    }

    /// Do what the project menu asked.
    pub fn apply_project_menu(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Switch(id) => {
                self.project_menu = None;
                // The project shown is already shown.
                if self.project.as_deref() != Some(id.as_str()) {
                    self.switch_project(&id);
                }
            }
            Outcome::Create { name, folder, kind } => {
                self.project_menu = None;
                self.create_project_with(&name, folder, kind);
            }
            Outcome::Rename(id, name) => {
                self.rename_project(&id, &name);
                self.refresh_project_menu();
            }
            Outcome::Stop(id) => {
                self.stop_project(&id);
                self.refresh_project_menu();
            }
            Outcome::Forget(id) => {
                self.forget_project(&id);
                self.refresh_project_menu();
            }
            Outcome::Browse(start) => self.pick_folder_for_menu(start),
            Outcome::Notice(text) => self.show_notice(text),
            Outcome::Closed => self.project_menu = None,
        }
        // Gone, the client under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// A left click while the project menu is open.
    pub fn project_menu_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.project_menu_view() else {
            return;
        };
        let page = self.project_menu_page();
        let outcome = match chrome::project_menu_hit(&view, self.output_size(), pos) {
            Some(hit) => match self.project_menu.as_mut() {
                Some(menu) => menu.click(hit, page),
                None => None,
            },
            None => Some(Outcome::Closed),
        };
        if let Some(outcome) = outcome {
            self.apply_project_menu(outcome);
        }
    }

    /// The pointer moved while the project menu is open.
    pub fn project_menu_hover(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.project_menu_view() else {
            return;
        };
        let hit = chrome::project_menu_hit(&view, self.output_size(), pos);
        if let Some(menu) = self.project_menu.as_mut() {
            menu.hover = hit;
        }
    }

    /// The wheel turned `rows` rows (down positive, parts of a row adding
    /// up) over the project menu.
    pub fn project_menu_scroll(&mut self, rows: f64) {
        let page = self.project_menu_page();
        if let Some(menu) = self.project_menu.as_mut() {
            menu.wheel += rows;
            let whole = menu.wheel.trunc();
            menu.wheel -= whole;
            if whole != 0.0 {
                menu.scroll_by(whole as i32, page);
            }
        }
    }

    /// What the project menu shows, while it is open.
    pub fn project_menu_view(&self) -> Option<ProjectMenuView> {
        let menu = self.project_menu.as_ref()?;
        Some(menu.view(self.project_menu_page(), &self.running))
    }

    /// zenity gave back `dir` for the form's folder.
    pub fn project_menu_folder(&mut self, dir: PathBuf) {
        if let Some(menu) = self.project_menu.as_mut() {
            menu.browsed(&dir);
        }
    }

    /// The projects, or which of them run, changed: show them again.
    pub fn refresh_project_menu(&mut self) {
        if self.project_menu.is_none() {
            return;
        }
        let rows = self
            .project_rows()
            .into_iter()
            .map(|(id, row)| {
                let running = self.running.get(&id).copied().or(row.running);
                (id, ProjectRow { running, ..row })
            })
            .collect();
        let taken = self
            .projects
            .projects
            .iter()
            .flat_map(|p| p.workspaces.iter().map(|w| w.dir.clone()))
            .collect();
        let recent = self.recent_dirs.clone();
        if let Some(menu) = self.project_menu.as_mut() {
            menu.set_lists(rows, recent, taken);
        }
    }

    /// How many entries the project menu's list shows.
    fn project_menu_page(&self) -> usize {
        chrome::project_menu_page(self.output_size())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home folder that has a `src` folder in it.
    fn home() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn row(name: &str, current: bool) -> (String, ProjectRow) {
        let row = ProjectRow {
            name: name.into(),
            detail: format!("~/x/{name}"),
            ago: "2h".into(),
            running: None,
            current,
        };
        (name.into(), row)
    }

    /// A menu with `aft` shown and `names` after it, most recent first, and
    /// zoxide knowing a few folders, `~/Sites/blog` taken by a project.
    fn menu(names: &[&str]) -> ProjectMenu {
        let home = home();
        let mut menu = ProjectMenu::new(Some(home.clone()));
        let mut rows = vec![row("aft", true)];
        rows.extend(names.iter().map(|n| row(n, false)));
        let recent = [
            "Sites/blogging",
            "Sites/blog",
            "blob",
            "blot",
            "blow",
            "src",
        ]
        .iter()
        .map(|d| home.join(d))
        .collect();
        menu.set_lists(rows, recent, HashSet::from([home.join("Sites/blog")]));
        menu
    }

    fn ctrl() -> ModifiersState {
        ModifiersState {
            ctrl: true,
            ..Default::default()
        }
    }

    fn alt() -> ModifiersState {
        ModifiersState {
            alt: true,
            ..Default::default()
        }
    }

    fn press(m: &mut ProjectMenu, sym: Keysym) -> Option<Outcome> {
        m.key(&ModifiersState::default(), sym, sym, 10)
    }

    fn chord(m: &mut ProjectMenu, mods: ModifiersState, sym: Keysym) -> Option<Outcome> {
        m.key(&mods, sym, sym, 10)
    }

    fn type_text(m: &mut ProjectMenu, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            assert_eq!(m.key(&ModifiersState::default(), sym, sym, 10), None);
        }
    }

    fn view(m: &ProjectMenu, page: usize) -> ProjectMenuView {
        m.view(page, &HashMap::new())
    }

    /// The entries in view, as text: "# Current" for a section.
    fn labels(m: &ProjectMenu, page: usize) -> Vec<String> {
        view(m, page)
            .entries
            .into_iter()
            .map(|entry| match entry {
                MenuEntry::Section(label) => format!("# {label}"),
                MenuEntry::Project { row, .. } => row.name,
                MenuEntry::New(label) | MenuEntry::Folder(label) => label,
            })
            .collect()
    }

    /// The selected entry, as text.
    fn selected(m: &ProjectMenu, page: usize) -> String {
        let i = view(m, page).selected.expect("an entry is selected");
        labels(m, page)[i].clone()
    }

    fn form(m: &ProjectMenu) -> FormView {
        view(m, 10).form.expect("the form is shown")
    }

    #[test]
    fn entries_come_in_sections_and_filter() {
        let mut m = menu(&["gerdr", "blog"]);
        assert_eq!(
            labels(&m, 10),
            [
                "# Current",
                "aft",
                "# Recent",
                "gerdr",
                "blog",
                "New project…"
            ]
        );
        // Without the shown project, and with zoxide's folders no project has.
        type_text(&mut m, "blo");
        assert_eq!(
            labels(&m, 10),
            [
                "# Recent",
                "blog",
                "New project \"blo\"…",
                "~/Sites/blogging",
                "~/blob",
                "~/blot",
            ]
        );
        m.key(&ctrl(), Keysym::u, Keysym::u, 10);
        type_text(&mut m, "aft");
        assert_eq!(labels(&m, 10), ["# Current", "aft", "New project \"aft\"…"]);
        assert_eq!(selected(&m, 10), "aft");
        type_text(&mut m, "zzz");
        assert_eq!(labels(&m, 10), ["New project \"aftzzz\"…"]);
        assert_eq!(selected(&m, 10), "New project \"aftzzz\"…");
    }

    #[test]
    fn the_selection_skips_sections() {
        let mut m = menu(&["gerdr", "blog"]);
        assert_eq!(selected(&m, 10), "gerdr");
        press(&mut m, Keysym::Up);
        assert_eq!(selected(&m, 10), "aft");
        press(&mut m, Keysym::Up);
        assert_eq!(selected(&m, 10), "aft");
        press(&mut m, Keysym::Down);
        assert_eq!(selected(&m, 10), "gerdr");
        press(&mut m, Keysym::End);
        assert_eq!(selected(&m, 10), "New project…");
        press(&mut m, Keysym::Down);
        assert_eq!(selected(&m, 10), "New project…");
        press(&mut m, Keysym::Home);
        assert_eq!(selected(&m, 10), "aft");
        press(&mut m, Keysym::Page_Down);
        assert_eq!(selected(&m, 10), "New project…");
    }

    #[test]
    fn the_list_scrolls_with_the_selection_and_shows_its_section() {
        let mut m = menu(&["gerdr", "blog"]);
        let key = |m: &mut ProjectMenu, sym| m.key(&ModifiersState::default(), sym, sym, 3);
        key(&mut m, Keysym::End);
        assert_eq!(labels(&m, 3), ["gerdr", "blog", "New project…"]);
        key(&mut m, Keysym::Up);
        key(&mut m, Keysym::Up);
        assert_eq!(labels(&m, 3), ["# Recent", "gerdr", "blog"]);
        assert_eq!(view(&m, 3).selected, Some(1));
    }

    #[test]
    fn the_wheel_scrolls_and_keeps_the_selection_in_view() {
        let names: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut m = menu(&refs);
        assert_eq!(selected(&m, 4), "p0");
        m.scroll_by(4, 4);
        assert_eq!(labels(&m, 4), ["p1", "p2", "p3", "p4"]);
        assert_eq!(selected(&m, 4), "p1");
        m.scroll_by(100, 4);
        assert_eq!(labels(&m, 4), ["p5", "p6", "p7", "New project…"]);
        assert_eq!(selected(&m, 4), "p5");
        m.scroll_by(-100, 4);
        assert_eq!(labels(&m, 4)[0], "# Current");
        assert_eq!(selected(&m, 4), "p0");
    }

    #[test]
    fn alt_digits_open_recent_projects() {
        let mut m = menu(&["gerdr", "blog"]);
        let digits: Vec<Option<u8>> = view(&m, 10)
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Project { digit, .. } => Some(*digit),
                _ => None,
            })
            .collect();
        assert_eq!(digits, [None, Some(1), Some(2)]);
        assert_eq!(
            chord(&mut m, alt(), Keysym::_2),
            Some(Outcome::Switch("blog".into()))
        );
        assert_eq!(
            chord(&mut m, alt(), Keysym::_1),
            Some(Outcome::Switch("gerdr".into()))
        );
        assert_eq!(chord(&mut m, alt(), Keysym::_3), None);
        // Alt doesn't type.
        assert_eq!(chord(&mut m, alt(), Keysym::a), None);
        assert_eq!(m.query, "");
    }

    #[test]
    fn enter_opens_each_kind_of_entry() {
        let mut m = menu(&["gerdr", "blog"]);
        assert_eq!(
            press(&mut m, Keysym::Return),
            Some(Outcome::Switch("gerdr".into()))
        );
        press(&mut m, Keysym::Home);
        assert_eq!(press(&mut m, Keysym::Return), Some(Outcome::Closed));
        press(&mut m, Keysym::End);
        assert_eq!(press(&mut m, Keysym::Return), None);
        let f = form(&m);
        assert_eq!((f.name.as_str(), f.focus), ("", FormField::Name));
        press(&mut m, Keysym::Escape);
        type_text(&mut m, "blo");
        press(&mut m, Keysym::End);
        assert_eq!(selected(&m, 10), "~/blot");
        assert_eq!(press(&mut m, Keysym::Return), None);
        let f = form(&m);
        assert_eq!(
            (f.name.as_str(), f.folder.as_str(), f.focus),
            ("blot", "~/blot", FormField::Folder)
        );
        assert!(f.suggestions.is_empty());
        assert_eq!(view(&m, 10).hints[0], ("Tab".into(), "next".into()));
    }

    #[test]
    fn ctrl_n_names_the_project_as_typed() {
        let mut m = menu(&["gerdr"]);
        type_text(&mut m, " blo ");
        assert_eq!(chord(&mut m, ctrl(), Keysym::n), None);
        assert_eq!(form(&m).name, "blo");
        // A name from the query doesn't follow the folder.
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/src");
        assert_eq!(form(&m).name, "blo");
    }

    #[test]
    fn renaming_saves_cancels_and_ignores_an_empty_name() {
        let mut m = menu(&["gerdr", "blog"]);
        chord(&mut m, ctrl(), Keysym::r);
        let renaming = |m: &ProjectMenu| {
            view(m, 10).entries.iter().find_map(|e| match e {
                MenuEntry::Project { renaming, .. } => renaming.clone(),
                _ => None,
            })
        };
        assert_eq!(renaming(&m).as_deref(), Some("gerdr"));
        assert_eq!(view(&m, 10).hints[0], ("Enter".into(), "save".into()));
        // The old name is selected: typing replaces it.
        type_text(&mut m, "x");
        type_text(&mut m, " y");
        assert_eq!(renaming(&m).as_deref(), Some("x y"));
        // Arrows don't move the selection meanwhile.
        press(&mut m, Keysym::Down);
        assert_eq!(
            press(&mut m, Keysym::Return),
            Some(Outcome::Rename("gerdr".into(), "x y".into()))
        );
        assert_eq!(renaming(&m), None);

        chord(&mut m, ctrl(), Keysym::r);
        assert_eq!(press(&mut m, Keysym::Escape), None);
        assert_eq!(renaming(&m), None);
        // Esc took back the renaming only.
        assert_eq!(press(&mut m, Keysym::Escape), Some(Outcome::Closed));

        chord(&mut m, ctrl(), Keysym::r);
        press(&mut m, Keysym::Right);
        press(&mut m, Keysym::BackSpace);
        assert_eq!(renaming(&m).as_deref(), Some("gerd"));
        chord(&mut m, ctrl(), Keysym::u);
        type_text(&mut m, "  ");
        assert_eq!(press(&mut m, Keysym::Return), None);
        assert_eq!(renaming(&m), None);
        // The shown project can be renamed too.
        press(&mut m, Keysym::Home);
        chord(&mut m, ctrl(), Keysym::r);
        type_text(&mut m, "aft2");
        assert_eq!(
            press(&mut m, Keysym::Return),
            Some(Outcome::Rename("aft".into(), "aft2".into()))
        );
    }

    #[test]
    fn delete_twice_forgets_and_another_key_takes_it_back() {
        let mut m = menu(&["gerdr", "blog"]);
        press(&mut m, Keysym::Down);
        assert_eq!(press(&mut m, Keysym::Delete), None);
        let confirm = |m: &ProjectMenu| {
            view(m, 10).entries.iter().position(
                |e| matches!(e, MenuEntry::Project { confirm_forget, .. } if *confirm_forget),
            )
        };
        assert_eq!(confirm(&m), Some(4));
        // A modifier on its own doesn't count.
        press(&mut m, Keysym::Shift_L);
        assert_eq!(
            press(&mut m, Keysym::Delete),
            Some(Outcome::Forget("blog".into()))
        );
        press(&mut m, Keysym::Delete);
        press(&mut m, Keysym::Up);
        assert_eq!(confirm(&m), None);
        assert_eq!(press(&mut m, Keysym::Delete), None);
        assert_eq!(confirm(&m), Some(3));
        // Not the shown project.
        press(&mut m, Keysym::Home);
        assert_eq!(press(&mut m, Keysym::Delete), None);
        assert_eq!(press(&mut m, Keysym::Delete), None);
        assert_eq!(confirm(&m), None);
    }

    #[test]
    fn stop_is_only_for_other_projects_that_may_run() {
        let mut m = menu(&["gerdr", "blog"]);
        assert_eq!(
            chord(&mut m, ctrl(), Keysym::s),
            Some(Outcome::Stop("gerdr".into()))
        );
        press(&mut m, Keysym::Home);
        assert_eq!(chord(&mut m, ctrl(), Keysym::s), None);
        let mut rows = vec![row("aft", true), row("gerdr", false), row("blog", false)];
        rows[2].1.running = Some(false);
        m.set_lists(rows, Vec::new(), HashSet::new());
        press(&mut m, Keysym::End);
        press(&mut m, Keysym::Up);
        assert_eq!(selected(&m, 10), "blog");
        assert_eq!(chord(&mut m, ctrl(), Keysym::s), None);
    }

    #[test]
    fn escape_backs_out_of_the_list_a_step_at_a_time() {
        let mut m = menu(&["gerdr"]);
        type_text(&mut m, "ge");
        press(&mut m, Keysym::Delete);
        assert_eq!(press(&mut m, Keysym::Escape), None);
        assert_eq!(
            (m.confirm_forget.as_deref(), m.query.as_str()),
            (None, "ge")
        );
        assert_eq!(press(&mut m, Keysym::Escape), None);
        assert_eq!(m.query, "");
        assert_eq!(press(&mut m, Keysym::Escape), Some(Outcome::Closed));
    }

    #[test]
    fn escape_backs_out_of_the_form_a_step_at_a_time() {
        let mut m = menu(&["gerdr"]);
        type_text(&mut m, "x");
        chord(&mut m, ctrl(), Keysym::n);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "sr");
        assert!(form(&m).suggestions.contains(&"~/src".to_string()));
        assert_eq!(press(&mut m, Keysym::Escape), None);
        assert!(form(&m).suggestions.is_empty());
        assert_eq!(press(&mut m, Keysym::Escape), None);
        assert_eq!(view(&m, 10).form, None);
        assert_eq!(m.query, "x");
    }

    #[test]
    fn tab_goes_round_the_form_skipping_the_kind_without_a_folder() {
        use FormField::*;
        let mut m = menu(&[]);
        chord(&mut m, ctrl(), Keysym::n);
        let mut order = vec![form(&m).focus];
        for _ in 0..5 {
            press(&mut m, Keysym::Tab);
            order.push(form(&m).focus);
        }
        assert_eq!(order, [Name, Folder, Browse, Cancel, Create, Name]);
        press(&mut m, Keysym::ISO_Left_Tab);
        assert_eq!(form(&m).focus, Create);
        // Shift+Tab, as the key's own symbol gives it.
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        m.key(&shift, Keysym::Tab, Keysym::ISO_Left_Tab, 10);
        assert_eq!(form(&m).focus, Cancel);
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/src");
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::Tab);
        assert_eq!(form(&m).focus, Kind);
        press(&mut m, Keysym::Right);
        assert_eq!(form(&m).kind, WorkspaceKind::Terminal);
        press(&mut m, Keysym::space);
        assert_eq!(form(&m).kind, WorkspaceKind::Project);
        press(&mut m, Keysym::Tab);
        assert_eq!(form(&m).focus, Cancel);
        assert_eq!(press(&mut m, Keysym::Return), None);
        assert_eq!(view(&m, 10).form, None);
    }

    #[test]
    fn the_name_follows_the_folder_until_typed() {
        let mut m = menu(&[]);
        chord(&mut m, ctrl(), Keysym::n);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/Sites/blog");
        assert_eq!(form(&m).name, "blog");
        m.browsed(&home().join("Work/aft"));
        assert_eq!(
            (form(&m).name.as_str(), form(&m).folder.as_str()),
            ("aft", "~/Work/aft")
        );
        press(&mut m, Keysym::ISO_Left_Tab);
        type_text(&mut m, "2");
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::BackSpace);
        assert_eq!(
            (form(&m).name.as_str(), form(&m).folder.as_str()),
            ("aft2", "~/Work/af")
        );
        // Cleared, it follows again.
        press(&mut m, Keysym::ISO_Left_Tab);
        chord(&mut m, ctrl(), Keysym::u);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "t");
        assert_eq!(form(&m).name, "aft");
    }

    #[test]
    fn a_suggestion_is_taken_by_enter_or_click() {
        let mut m = menu(&[]);
        chord(&mut m, ctrl(), Keysym::n);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "sr");
        assert_eq!(form(&m).suggestions, ["~/src"]);
        assert_eq!(form(&m).suggestion, None);
        press(&mut m, Keysym::Down);
        press(&mut m, Keysym::Down);
        assert_eq!(form(&m).suggestion, Some(0));
        assert_eq!(press(&mut m, Keysym::Return), None);
        let f = form(&m);
        assert_eq!(
            (f.folder.as_str(), f.name.as_str(), f.suggestions.len()),
            ("~/src", "src", 0)
        );
        // At most five, best first.
        chord(&mut m, ctrl(), Keysym::u);
        type_text(&mut m, "~/b");
        assert_eq!(form(&m).suggestions.len(), 5);
        assert_eq!(form(&m).suggestions[0], "~/blob");
        assert_eq!(m.click(MenuHit::Suggestion(1), 10), None);
        assert_eq!(form(&m).folder, "~/blot");
        assert!(form(&m).suggestions.is_empty());
    }

    #[test]
    fn each_validation_error_shows() {
        let mut m = menu(&["gerdr"]);
        chord(&mut m, ctrl(), Keysym::n);
        assert_eq!(press(&mut m, Keysym::Return), None);
        assert_eq!(form(&m).error.as_deref(), Some("Give the project a name."));
        type_text(&mut m, "GERDR ");
        assert_eq!(form(&m).error, None);
        assert_eq!(press(&mut m, Keysym::Return), None);
        assert_eq!(
            form(&m).error.as_deref(),
            Some("A project named GERDR exists.")
        );
        chord(&mut m, ctrl(), Keysym::u);
        type_text(&mut m, "new");
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/nope");
        assert_eq!(press(&mut m, Keysym::Return), None);
        let f = form(&m);
        assert_eq!(f.error.as_deref(), Some("No folder at ~/nope."));
        assert_eq!(f.focus, FormField::Folder);
    }

    #[test]
    fn create_with_and_without_a_folder() {
        let mut m = menu(&["gerdr"]);
        chord(&mut m, ctrl(), Keysym::n);
        type_text(&mut m, " new ");
        assert_eq!(
            press(&mut m, Keysym::Return),
            Some(Outcome::Create {
                name: "new".into(),
                folder: None,
                kind: WorkspaceKind::Project,
            })
        );
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/src");
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::Tab);
        press(&mut m, Keysym::Left);
        let create = Some(Outcome::Create {
            name: "new".into(),
            folder: Some(home().join("src")),
            kind: WorkspaceKind::Terminal,
        });
        assert_eq!(press(&mut m, Keysym::Return), create);
        assert_eq!(m.click(MenuHit::Field(FormField::Create), 10), create);
    }

    #[test]
    fn browse_starts_in_the_folder_typed() {
        let mut m = menu(&[]);
        chord(&mut m, ctrl(), Keysym::n);
        assert_eq!(
            chord(&mut m, ctrl(), Keysym::o),
            Some(Outcome::Browse(None))
        );
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/src");
        let browse = Some(Outcome::Browse(Some(home().join("src"))));
        assert_eq!(chord(&mut m, ctrl(), Keysym::o), browse);
        press(&mut m, Keysym::Tab);
        assert_eq!(form(&m).focus, FormField::Browse);
        assert_eq!(press(&mut m, Keysym::Return), browse);
        assert_eq!(m.click(MenuHit::Field(FormField::Browse), 10), browse);
    }

    #[test]
    fn clicks_open_rename_stop_and_forget() {
        let mut m = menu(&["gerdr", "blog"]);
        assert_eq!(
            m.click(MenuHit::Entry(4), 10),
            Some(Outcome::Switch("blog".into()))
        );
        assert_eq!(m.click(MenuHit::Entry(0), 10), None);
        assert_eq!(m.click(MenuHit::Button(3, RowButton::Forget), 10), None);
        assert_eq!(m.confirm_forget.as_deref(), Some("gerdr"));
        assert_eq!(
            m.click(MenuHit::Button(3, RowButton::Forget), 10),
            Some(Outcome::Forget("gerdr".into()))
        );
        // A click elsewhere takes back the first.
        m.click(MenuHit::Button(3, RowButton::Forget), 10);
        m.click(MenuHit::Panel, 10);
        assert_eq!(m.click(MenuHit::Button(3, RowButton::Forget), 10), None);
        assert_eq!(
            m.click(MenuHit::Button(4, RowButton::Stop), 10),
            Some(Outcome::Stop("blog".into()))
        );
        m.click(MenuHit::Button(1, RowButton::Rename), 10);
        assert_eq!(m.renaming.as_ref().map(|r| r.id.as_str()), Some("aft"));
        // Clicking in the name keeps it; elsewhere ends it.
        m.click(MenuHit::Entry(1), 10);
        assert!(m.renaming.is_some());
        m.click(MenuHit::Query, 10);
        assert!(m.renaming.is_none());
        assert_eq!(m.click(MenuHit::Entry(5), 10), None);
        assert!(m.form.is_some());
        m.click(MenuHit::Field(FormField::Cancel), 10);
        assert!(m.form.is_none());
    }

    #[test]
    fn refreshing_keeps_the_selection_on_its_project() {
        let mut m = menu(&["gerdr", "blog", "notes"]);
        press(&mut m, Keysym::Down);
        assert_eq!(selected(&m, 10), "blog");
        let mut rows = vec![row("aft", true), row("blog", false), row("gerdr", false)];
        rows[1].1.running = Some(true);
        m.set_lists(rows.clone(), Vec::new(), HashSet::new());
        assert_eq!(selected(&m, 10), "blog");
        // Forgotten, the entry in its place is selected.
        rows.remove(1);
        m.set_lists(rows, Vec::new(), HashSet::new());
        assert_eq!(selected(&m, 10), "gerdr");
        // The running status shows from gerdr's.
        let running = HashMap::from([("gerdr".to_string(), false)]);
        let dots: Vec<Option<bool>> = m
            .view(10, &running)
            .entries
            .into_iter()
            .filter_map(|e| match e {
                MenuEntry::Project { row, .. } => Some(row.running),
                _ => None,
            })
            .collect();
        assert_eq!(dots, [None, Some(false)]);
    }

    #[test]
    fn ctrl_keys_edit_the_query_and_dont_type() {
        let mut m = menu(&["gerdr"]);
        type_text(&mut m, "foo bar");
        chord(&mut m, ctrl(), Keysym::w);
        assert_eq!(m.query, "foo ");
        chord(&mut m, ctrl(), Keysym::x);
        assert_eq!(m.query, "foo ");
        press(&mut m, Keysym::BackSpace);
        assert_eq!(m.query, "foo");
        let logo = ModifiersState {
            logo: true,
            ..Default::default()
        };
        chord(&mut m, logo, Keysym::a);
        assert_eq!(m.query, "foo");
    }

    #[test]
    fn folders_expand_from_home() {
        let home = home();
        assert_eq!(expand("  ", Some(&home)), None);
        assert_eq!(expand("~", Some(&home)), Some(home.clone()));
        assert_eq!(expand("~/src", Some(&home)), Some(home.join("src")));
        assert_eq!(expand("src", Some(&home)), Some(home.join("src")));
        assert_eq!(expand("/tmp", Some(&home)), Some(PathBuf::from("/tmp")));
        assert_eq!(basename("~/Sites/blog/", Some(&home)), "blog");
    }

    #[test]
    fn renaming_to_a_name_in_use_keeps_the_field_open() {
        let mut m = menu(&["gerdr", "blog"]);
        chord(&mut m, ctrl(), Keysym::r);
        type_text(&mut m, "BLOG");
        assert_eq!(
            press(&mut m, Keysym::Return),
            Some(Outcome::Notice("A project named BLOG exists.".into()))
        );
        let renaming = view(&m, 10).entries.iter().find_map(|e| match e {
            MenuEntry::Project { renaming, .. } => renaming.clone(),
            _ => None,
        });
        assert_eq!(renaming.as_deref(), Some("BLOG"));
    }

    #[test]
    fn a_refresh_keeps_the_suggestion_moved_to() {
        let mut m = menu(&[]);
        chord(&mut m, ctrl(), Keysym::n);
        press(&mut m, Keysym::Tab);
        type_text(&mut m, "~/b");
        press(&mut m, Keysym::Down);
        press(&mut m, Keysym::Down);
        let chosen = form(&m).suggestions[1].clone();
        let (projects, recent, taken) = (m.projects.clone(), m.recent.clone(), m.taken.clone());
        m.set_lists(projects, recent, taken);
        let f = form(&m);
        assert_eq!(f.suggestion.map(|s| f.suggestions[s].clone()), Some(chosen));
    }
}
