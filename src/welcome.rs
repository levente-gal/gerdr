//! The welcome screen: shown when no project is open (first run, or the last
//! project was stopped). While one is open, the project menu switches
//! projects instead. A greeting, the recent projects to filter and pick
//! from, and a button to start a new one.
//!
//! Typing filters the projects; Up/Down (and Page Up/Down) move the
//! selection, down past the last project onto the button, and Tab jumps
//! between the projects and the button. Enter opens the selected project, or
//! on the button (or when nothing matches) asks for the new project's name,
//! filled in with what was typed; Ctrl+N asks for it right away. Delete
//! twice forgets the selected project (not the one shown behind). Esc backs
//! out one step at a time: the name, the forgetting, the filter, then the
//! screen itself, if a project is behind it.

use std::{collections::HashMap, time::Instant};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    anim::Hover,
    chrome::{self, ProjectRow, WelcomeHit, WelcomeView},
    herdr,
    project_list::{self, scrolled},
    projects,
    text_field::{Edit, TextField},
    tooltip::Tooltip,
};

/// The welcome screen's state, while it is shown.
#[derive(Debug, Default)]
pub struct Welcome {
    /// Opened from inside a project (Esc goes back to it), rather than shown
    /// because none is open.
    pub in_app: bool,
    /// What was typed to filter the projects.
    query: TextField,
    /// The projects, by id, most recently used first.
    projects: Vec<(String, ProjectRow)>,
    /// The projects `query` matches (indices into `projects`), best first,
    /// each with the characters matched in "name detail".
    matches: Vec<(usize, Vec<usize>)>,
    /// The selected match; `None` when the button is.
    selected: Option<usize>,
    /// The match selected before the button was, for Tab to go back to.
    last_row: usize,
    /// The first match shown.
    scroll: usize,
    /// What the pointer is over, in view rows, and its pill fading in.
    hover: Hover<WelcomeHit>,
    /// The new project's name typed so far, while asking for it.
    naming: Option<TextField>,
    /// The project Delete was pressed on once, by id.
    confirm_forget: Option<String>,
    /// herdr wasn't found on `PATH` when the screen opened.
    herdr_missing: bool,
}

/// What the welcome screen asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Open project `id`.
    Pick(String),
    /// Start a new project with this name.
    Create(String),
    /// Forget project `id` (its herdr session stays).
    Forget(String),
    /// Go back to the project behind it.
    Closed,
}

impl Welcome {
    fn new(projects: Vec<(String, ProjectRow)>, in_app: bool) -> Self {
        let mut welcome = Self {
            in_app,
            ..Self::default()
        };
        welcome.set_projects(projects);
        welcome
    }

    /// Replace the projects, keeping the query, and the selection on the
    /// same project if it is still there.
    fn set_projects(&mut self, projects: Vec<(String, ProjectRow)>) {
        let picked = self.picked().map(str::to_owned);
        let was = self.selected;
        self.projects = projects;
        self.refilter();
        if let Some(i) = picked.and_then(|id| {
            self.matches
                .iter()
                .position(|(m, _)| self.projects[*m].0 == id)
        }) {
            self.selected = Some(i);
        } else if let Some(was) = was
            && !self.matches.is_empty()
        {
            // The selected project went: select the one in its place.
            self.selected = Some(was.min(self.matches.len() - 1));
        }
    }

    /// Match the projects against the query again (by name, then detail),
    /// and select the best match, or the button if none.
    fn refilter(&mut self) {
        self.matches = project_list::matches(&self.query, &self.projects);
        self.selected = (!self.matches.is_empty()).then_some(0);
        self.last_row = 0;
        self.scroll = 0;
    }

    /// The selected project's id.
    fn picked(&self) -> Option<&str> {
        let (m, _) = self.matches.get(self.selected?)?;
        Some(&self.projects[*m].0)
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` the one with
    /// Shift and the layout applied. `page` is how many rows are shown.
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
        let ctrl = mods.ctrl;
        // Any key but Delete again takes back a first Delete.
        let armed = self.confirm_forget.take();
        // The field typed in takes the keys that edit it first: the new
        // project's name while asking for it, else the query.
        let edit = match &mut self.naming {
            Some(name) => name.key(mods, sym, typed),
            None => self.query.key(mods, sym, typed),
        };
        match edit {
            Edit::Changed if self.naming.is_none() => {
                self.refilter();
                return None;
            }
            Edit::Changed | Edit::Moved => return None,
            Edit::Ignored => {}
        }
        match sym {
            Keysym::Escape => {
                if self.naming.is_some() {
                    self.naming = None;
                } else if armed.is_some() {
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.refilter();
                } else if self.in_app {
                    return Some(Outcome::Closed);
                }
            }
            Keysym::Return | Keysym::KP_Enter => match &self.naming {
                Some(name) => {
                    let name = name.trim();
                    if !name.is_empty() {
                        return Some(Outcome::Create(name.to_owned()));
                    }
                }
                None => match self.picked() {
                    Some(id) => return Some(Outcome::Pick(id.to_owned())),
                    None => self.start_naming(),
                },
            },
            Keysym::n if ctrl => {
                if self.naming.is_none() {
                    self.start_naming();
                }
            }
            Keysym::Delete | Keysym::KP_Delete if self.naming.is_none() => {
                let &(m, _) = self.selected.and_then(|i| self.matches.get(i))?;
                let (id, row) = &self.projects[m];
                if row.current {
                    // The project shown behind can't be forgotten.
                } else if armed.as_ref() == Some(id) {
                    return Some(Outcome::Forget(id.clone()));
                } else {
                    self.confirm_forget = Some(id.clone());
                }
            }
            // Arrows and Tab don't leave the name.
            _ if self.naming.is_some() => {}
            Keysym::Up | Keysym::KP_Up => self.step(-1, page),
            Keysym::Down | Keysym::KP_Down => self.step(1, page),
            Keysym::Page_Up | Keysym::KP_Page_Up => self.step(-(page.max(1) as isize), page),
            Keysym::Page_Down | Keysym::KP_Page_Down => self.step(page.max(1) as isize, page),
            Keysym::Tab | Keysym::ISO_Left_Tab => self.toggle_button(page),
            _ => {}
        }
        None
    }

    /// The text field a click can go in: the new project's name, while
    /// asking for it.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.naming.as_mut()
    }

    /// Ask for the new project's name, starting from what was typed.
    fn start_naming(&mut self) {
        self.naming = Some(self.query.trim().into());
    }

    /// Move the selection `by` places among the matches and then the button,
    /// scrolling to keep it shown.
    fn step(&mut self, by: isize, page: usize) {
        let button = self.matches.len();
        let at = self.selected.unwrap_or(button) as isize;
        let to = (at + by).clamp(0, button as isize) as usize;
        self.select((to < button).then_some(to), page);
    }

    /// Select the button, or from it the match selected before.
    fn toggle_button(&mut self, page: usize) {
        match self.selected {
            Some(_) => self.select(None, page),
            None if !self.matches.is_empty() => {
                self.select(Some(self.last_row.min(self.matches.len() - 1)), page);
            }
            None => {}
        }
    }

    fn select(&mut self, selected: Option<usize>, page: usize) {
        if let Some(i) = self.selected {
            self.last_row = i;
        }
        self.selected = selected;
        if let Some(i) = selected {
            self.scroll = scrolled(self.scroll, i, page);
        }
    }

    /// The first match shown.
    fn first_shown(&self, page: usize) -> usize {
        let scroll = match self.selected {
            Some(i) => scrolled(self.scroll, i, page),
            None => self.scroll,
        };
        scroll.min(self.matches.len().saturating_sub(page))
    }

    /// The matches shown, `page` at most, by index into `projects` with
    /// their hits.
    fn shown(&self, page: usize) -> &[(usize, Vec<usize>)] {
        let first = self.first_shown(page);
        let end = (first + page).min(self.matches.len());
        &self.matches[first..end]
    }

    /// The id of shown row `row`.
    fn row_id(&self, row: usize, page: usize) -> Option<&str> {
        let (m, _) = self.shown(page).get(row)?;
        Some(&self.projects[*m].0)
    }

    /// Select shown row `row`.
    fn select_row(&mut self, row: usize, page: usize) {
        let first = self.first_shown(page);
        if first + row < self.matches.len() {
            self.naming = None;
            self.select(Some(first + row), page);
        }
    }

    /// The pointer is over `hit` at `now`.
    fn set_hover(&mut self, hit: Option<WelcomeHit>, now: Instant) {
        self.hover.track(hit, now);
    }

    /// What is shown at `now`, with the parts gerdr knows filled in:
    /// `running` says whose sessions run, by id; `quit_keys` is what quits
    /// gerdr, for the bar at the bottom.
    fn view(
        &self,
        page: usize,
        greeting: String,
        note: Option<String>,
        running: &HashMap<String, bool>,
        quit_keys: Option<String>,
        now: Instant,
    ) -> WelcomeView {
        let first = self.first_shown(page);
        let shown = self.shown(page);
        let rows = shown
            .iter()
            .map(|(m, _)| {
                let (id, row) = &self.projects[*m];
                ProjectRow {
                    running: running.get(id).copied().or(row.running),
                    ..row.clone()
                }
            })
            .collect();
        let hits = shown.iter().map(|(_, hits)| hits.clone()).collect();
        let selected = self
            .selected
            .filter(|_| self.naming.is_none())
            .and_then(|i| i.checked_sub(first))
            .filter(|i| *i < shown.len());
        let confirm_forget = self
            .confirm_forget
            .as_ref()
            .and_then(|id| shown.iter().position(|(m, _)| &self.projects[*m].0 == id));
        let subtitle = if self.projects.is_empty() {
            "Start your first project."
        } else {
            "Pick up where you left off."
        };
        WelcomeView {
            greeting,
            subtitle: subtitle.into(),
            query: self.query.line(),
            rows,
            hits,
            selected,
            hover: self.hover.over(),
            hover_alpha: self.hover.alpha_over(now),
            naming: self.naming.as_ref().map(TextField::line),
            confirm_forget,
            note,
            in_app: self.in_app,
            quit_keys,
        }
    }
}

impl Gerdr {
    /// Show the welcome screen, and find out which projects' sessions run.
    pub fn open_welcome(&mut self, in_app: bool) {
        self.project_menu = None;
        self.welcome = Some(Welcome {
            herdr_missing: !herdr::is_installed(),
            ..Welcome::new(self.project_rows(), in_app)
        });
        self.check_running();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// A key while the welcome screen has the keyboard; what to do once the
    /// keyboard is free again, if anything.
    pub fn welcome_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let page = self.welcome_page();
        self.welcome.as_mut()?.key(mods, sym, typed, page)
    }

    /// Do what the welcome screen asked.
    pub fn apply_welcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Pick(id) => {
                self.welcome = None;
                // The project behind is already shown.
                if self.project.as_deref() != Some(id.as_str()) {
                    self.switch_project(&id);
                }
            }
            Outcome::Create(name) => {
                self.welcome = None;
                self.create_project(&name);
            }
            Outcome::Forget(id) => {
                self.forget_project(&id);
                let rows = self.project_rows();
                if let Some(welcome) = self.welcome.as_mut() {
                    welcome.set_projects(rows);
                }
            }
            Outcome::Closed => self.welcome = None,
        }
        // Gone, the client under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// A left click while the welcome screen is shown; `false` if it isn't.
    pub fn welcome_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let page = self.welcome_page();
        let size = self.output_size();
        let Some(welcome) = self.welcome.as_mut() else {
            return false;
        };
        welcome.confirm_forget = None;
        let outcome = match welcome_hit(welcome, size, page, pos) {
            Some(WelcomeHit::Row(row)) => {
                welcome.select_row(row, page);
                welcome
                    .row_id(row, page)
                    .map(|id| Outcome::Pick(id.to_owned()))
            }
            Some(WelcomeHit::Button) => match &welcome.naming {
                // The name's field: a click in it moves its caret.
                Some(_) => None,
                None => {
                    welcome.start_naming();
                    None
                }
            },
            None => None,
        };
        if let Some(outcome) = outcome {
            self.apply_welcome(outcome);
        }
        true
    }

    /// The pointer moved while the welcome screen is shown.
    pub fn welcome_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let page = self.welcome_page();
        let size = self.output_size();
        let welcome = self.welcome.as_mut()?;
        let hit = welcome_hit(welcome, size, page, pos);
        welcome.set_hover(hit, Instant::now());
        // A project's mark says whether it runs.
        let Some(WelcomeHit::Row(i)) = hit else {
            return None;
        };
        let view = self.welcome_view()?;
        if view.confirm_forget == Some(i) {
            return None;
        }
        let layout = chrome::welcome_layout(
            size,
            view.rows.len(),
            view.naming.is_some(),
            view.note.is_some(),
        );
        let mark = chrome::welcome_mark(*layout.rows.get(i)?);
        let running = view.rows.get(i)?.running;
        mark.to_f64()
            .contains(pos)
            .then(|| Tooltip::new(chrome::running_label(running), mark))
    }

    pub fn welcome_view(&self) -> Option<WelcomeView> {
        let welcome = self.welcome.as_ref()?;
        let hello = projects::greeting(projects::local_hour());
        let greeting = match std::env::var("USER").ok().filter(|u| !u.is_empty()) {
            Some(user) => format!("{hello}, {user}"),
            None => hello.into(),
        };
        // Without herdr, projects live only as long as gerdr.
        let note = welcome
            .herdr_missing
            .then(|| "herdr isn't installed — projects won't keep running".into());
        // The prefix and the key after it that quit, for the bar at the bottom.
        let help = self.keymap.help();
        let quit_keys = help
            .rows
            .iter()
            .find(|(_, label)| label == "quit")
            .map(|(keys, _)| format!("{} {keys}", help.prefix));
        Some(welcome.view(
            self.welcome_page(),
            greeting,
            note,
            &self.running,
            quit_keys,
            Instant::now(),
        ))
    }

    fn welcome_page(&self) -> usize {
        chrome::welcome_page(self.output_size())
    }
}

/// What `pos` (output coordinates) is over on the welcome screen.
fn welcome_hit(
    welcome: &Welcome,
    size: smithay::utils::Size<i32, Logical>,
    page: usize,
    pos: Point<f64, Logical>,
) -> Option<WelcomeHit> {
    let shown = welcome.shown(page).len();
    let layout =
        chrome::welcome_layout(size, shown, welcome.naming.is_some(), welcome.herdr_missing);
    if let Some(row) = layout.rows.iter().position(|r| r.to_f64().contains(pos)) {
        return Some(WelcomeHit::Row(row));
    }
    layout
        .button
        .to_f64()
        .contains(pos)
        .then_some(WelcomeHit::Button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, detail: &str) -> ProjectRow {
        ProjectRow {
            name: name.into(),
            detail: detail.into(),
            ago: "2h ago".into(),
            running: None,
            current: false,
        }
    }

    fn welcome(names: &[&str]) -> Welcome {
        Welcome::new(
            names
                .iter()
                .map(|n| (n.to_string(), row(n, &format!("~/{n}"))))
                .collect(),
            true,
        )
    }

    fn ctrl() -> ModifiersState {
        ModifiersState {
            ctrl: true,
            ..Default::default()
        }
    }

    fn press(w: &mut Welcome, sym: Keysym) -> Option<Outcome> {
        w.key(&ModifiersState::default(), sym, sym, 3)
    }

    fn type_in(w: &mut Welcome, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            assert_eq!(w.key(&ModifiersState::default(), sym, sym, 3), None);
        }
    }

    fn view(w: &Welcome) -> WelcomeView {
        w.view(
            3,
            String::new(),
            None,
            &HashMap::new(),
            None,
            Instant::now(),
        )
    }

    fn names(w: &Welcome) -> Vec<String> {
        view(w).rows.into_iter().map(|r| r.name).collect()
    }

    #[test]
    fn filters_best_first_and_keeps_recency_without_query() {
        let mut w = welcome(&["services-api", "gerdr", "garden"]);
        assert_eq!(names(&w), ["services-api", "gerdr", "garden"]);
        type_in(&mut w, "gerdr");
        assert_eq!(names(&w), ["gerdr"]);
        w.key(&ctrl(), Keysym::u, Keysym::u, 3);
        type_in(&mut w, "gr");
        let shown = names(&w);
        assert_eq!(shown.len(), 2);
        assert!(!shown.contains(&"services-api".to_string()));
        // The detail is searched too.
        w.key(&ctrl(), Keysym::u, Keysym::u, 3);
        type_in(&mut w, "~/serv");
        assert_eq!(names(&w), ["services-api"]);
    }

    #[test]
    fn the_view_carries_the_matched_characters_and_the_hover() {
        let mut w = welcome(&["services-api", "gerdr"]);
        type_in(&mut w, "ger");
        let v = view(&w);
        assert_eq!(names(&w), ["gerdr"]);
        assert_eq!(v.hits, vec![vec![0, 1, 2]]);
        assert!(v.in_app && v.quit_keys.is_none());
        // The hover pill fades in from when the pointer got there.
        let start = Instant::now();
        w.set_hover(Some(WelcomeHit::Row(0)), start);
        let at = |w: &Welcome, now| {
            let v = w.view(3, String::new(), None, &HashMap::new(), None, now);
            (v.hover, v.hover_alpha)
        };
        assert_eq!(at(&w, start), (Some(WelcomeHit::Row(0)), 0));
        assert_eq!(
            at(&w, start + crate::anim::FAST),
            (Some(WelcomeHit::Row(0)), 16)
        );
        w.set_hover(None, start);
        assert_eq!(at(&w, start), (None, 0));
    }

    #[test]
    fn enter_picks_or_names_from_the_query() {
        let mut w = welcome(&["alpha", "beta"]);
        press(&mut w, Keysym::Down);
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Pick("beta".into()))
        );
        type_in(&mut w, "zeta");
        assert_eq!(view(&w).selected, None);
        assert_eq!(press(&mut w, Keysym::Return), None);
        assert_eq!(w.naming.as_deref(), Some("zeta"));
        type_in(&mut w, " x");
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Create("zeta x".into()))
        );
    }

    #[test]
    fn ctrl_n_names_and_an_empty_name_is_ignored() {
        let mut w = welcome(&["alpha"]);
        assert_eq!(w.key(&ctrl(), Keysym::n, Keysym::n, 3), None);
        assert_eq!(w.naming.as_deref(), Some(""));
        type_in(&mut w, "  ");
        assert_eq!(press(&mut w, Keysym::Return), None);
        // Arrows and Tab don't leave the name.
        press(&mut w, Keysym::BackSpace);
        press(&mut w, Keysym::Down);
        type_in(&mut w, "new");
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Create("new".into()))
        );
        // Typing went to the name, not the filter.
        assert_eq!(&*w.query, "");
    }

    #[test]
    fn delete_twice_forgets() {
        let mut w = welcome(&["alpha", "beta"]);
        press(&mut w, Keysym::Down);
        assert_eq!(press(&mut w, Keysym::Delete), None);
        assert_eq!(view(&w).confirm_forget, Some(1));
        assert_eq!(
            press(&mut w, Keysym::Delete),
            Some(Outcome::Forget("beta".into()))
        );
    }

    #[test]
    fn another_key_takes_back_delete() {
        let mut w = welcome(&["alpha", "beta"]);
        press(&mut w, Keysym::Delete);
        assert!(w.confirm_forget.is_some());
        // A modifier on its own doesn't count.
        press(&mut w, Keysym::Shift_L);
        assert!(w.confirm_forget.is_some());
        press(&mut w, Keysym::Down);
        assert_eq!(w.confirm_forget, None);
        press(&mut w, Keysym::Up);
        assert_eq!(press(&mut w, Keysym::Delete), None);
        // Esc takes back the Delete, and nothing else.
        assert_eq!(press(&mut w, Keysym::Escape), None);
        assert_eq!(w.confirm_forget, None);
        assert_eq!(press(&mut w, Keysym::Delete), None);
    }

    #[test]
    fn the_current_project_is_never_forgotten() {
        let mut w = Welcome::new(
            vec![(
                "alpha".into(),
                ProjectRow {
                    current: true,
                    ..row("alpha", "")
                },
            )],
            true,
        );
        for _ in 0..3 {
            assert_eq!(press(&mut w, Keysym::Delete), None);
            assert_eq!(view(&w).confirm_forget, None);
        }
    }

    #[test]
    fn escape_backs_out_a_step_at_a_time() {
        let mut w = welcome(&["alpha"]);
        type_in(&mut w, "al");
        w.key(&ctrl(), Keysym::n, Keysym::n, 3);
        assert_eq!(press(&mut w, Keysym::Escape), None);
        assert_eq!((w.naming.as_deref(), &*w.query), (None, "al"));
        assert_eq!(press(&mut w, Keysym::Escape), None);
        assert_eq!(&*w.query, "");
        assert_eq!(press(&mut w, Keysym::Escape), Some(Outcome::Closed));
        // Without a project behind, there's nothing to go back to.
        w.in_app = false;
        assert_eq!(press(&mut w, Keysym::Escape), None);
    }

    #[test]
    fn tab_goes_to_the_button_and_back() {
        let mut w = welcome(&["alpha", "beta", "gamma"]);
        press(&mut w, Keysym::Down);
        press(&mut w, Keysym::Tab);
        assert_eq!(view(&w).selected, None);
        assert_eq!(press(&mut w, Keysym::Return), None);
        assert_eq!(w.naming.as_deref(), Some(""));
        press(&mut w, Keysym::Escape);
        press(&mut w, Keysym::Tab);
        assert_eq!(view(&w).selected, Some(1));
        // Down past the last project reaches the button too.
        press(&mut w, Keysym::Down);
        press(&mut w, Keysym::Down);
        assert_eq!(w.selected, None);
        press(&mut w, Keysym::Down);
        assert_eq!(w.selected, None);
        press(&mut w, Keysym::Up);
        assert_eq!(w.selected, Some(2));
    }

    #[test]
    fn without_projects_the_button_is_selected() {
        let mut w = welcome(&[]);
        let view = view(&w);
        assert_eq!(
            (view.selected, view.subtitle.as_str()),
            (None, "Start your first project.")
        );
        press(&mut w, Keysym::Down);
        press(&mut w, Keysym::Tab);
        assert_eq!(press(&mut w, Keysym::Return), None);
        assert!(w.naming.is_some());
    }

    #[test]
    fn selection_scrolls_by_page() {
        let names_list: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = names_list.iter().map(String::as_str).collect();
        let mut w = welcome(&refs);
        for _ in 0..4 {
            press(&mut w, Keysym::Down);
        }
        let view = view(&w);
        assert_eq!(names(&w), ["p2", "p3", "p4"]);
        assert_eq!(view.selected, Some(2));
        assert_eq!(w.row_id(0, 3), Some("p2"));
        press(&mut w, Keysym::Page_Down);
        assert_eq!(names(&w), ["p5", "p6", "p7"]);
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Pick("p7".into()))
        );
        press(&mut w, Keysym::Page_Up);
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Pick("p4".into()))
        );
        // On the button, the view stays where it was.
        press(&mut w, Keysym::Tab);
        assert_eq!(names(&w), ["p4", "p5", "p6"]);
        w.select_row(1, 3);
        assert_eq!(
            press(&mut w, Keysym::Return),
            Some(Outcome::Pick("p5".into()))
        );
    }

    #[test]
    fn forgetting_keeps_the_place() {
        let mut w = welcome(&["alpha", "beta", "gamma"]);
        press(&mut w, Keysym::Down);
        w.set_projects(vec![
            ("alpha".into(), row("alpha", "")),
            ("gamma".into(), row("gamma", "")),
        ]);
        let running = HashMap::from([("gamma".to_string(), true)]);
        let view = w.view(3, String::new(), None, &running, None, Instant::now());
        assert_eq!(view.selected, Some(1));
        assert_eq!(view.rows[1].running, Some(true));
        assert_eq!(view.rows[0].running, None);
    }

    #[test]
    fn ctrl_keys_edit_and_dont_type() {
        let mut w = welcome(&["alpha"]);
        type_in(&mut w, "foo bar");
        w.key(&ctrl(), Keysym::w, Keysym::w, 3);
        assert_eq!(&*w.query, "foo ");
        w.key(&ctrl(), Keysym::x, Keysym::x, 3);
        assert_eq!(&*w.query, "foo ");
        press(&mut w, Keysym::BackSpace);
        assert_eq!(&*w.query, "foo");
    }
}
