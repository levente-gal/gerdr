//! The background tasks gerdr follows, for the footer and its tasks panel
//! (see `chrome::footer`): T3 Code's server and its update checks, the
//! git operations running in the repositories, and the apps projects keep
//! running (see `keep`). Gathered anew for each frame from what gerdr
//! knows already; what reads files (the apps kept) is looked at every
//! `KEPT_EVERY`.
//!
//! The panel opens from the footer and the launcher. Up/Down move between
//! the tasks, Enter presses the selected one's first button, Esc closes.

use std::{path::Path, time::Duration};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Rectangle},
};

use crate::{
    Gerdr,
    chrome::{self, Dot, FooterHit, FooterView, TaskView, TasksHit, TasksView},
    git_ops, keep, launch,
    menu::MenuAction,
    t3_server,
    tooltip::Tooltip,
};

/// How often the apps projects keep running are looked at.
const KEPT_EVERY: Duration = Duration::from_secs(2);

/// A task running in the background.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub title: String,
    pub chip: String,
    pub detail: String,
    pub dot: Dot,
    /// Its buttons: label and what pressing it does.
    pub actions: Vec<(&'static str, MenuAction)>,
}

impl Task {
    /// Whether it runs: what is only offered (T3 Code's server stopped,
    /// to start) isn't counted nor shown in the footer.
    pub fn runs(&self) -> bool {
        self.dot != Dot::Idle
    }

    fn view(&self) -> TaskView {
        TaskView {
            title: self.title.clone(),
            chip: self.chip.clone(),
            detail: self.detail.clone(),
            dot: self.dot,
            buttons: self.actions.iter().map(|(l, _)| (*l).to_owned()).collect(),
        }
    }
}

/// The apps a project keeps running, as last looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub id: String,
    pub name: String,
    pub apps: Vec<String>,
}

/// The tasks panel's state, while it is open.
#[derive(Debug, Default)]
pub struct TasksPanel {
    pub selected: usize,
    /// The selected row's button Enter presses: Tab moves it.
    pub button: usize,
    /// The first row shown, when not all fit.
    pub first: usize,
    pub hover: Option<TasksHit>,
}

/// What the tasks panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Close,
    Run(MenuAction),
}

/// How long ago `secs` seconds is, shortly: "5m", "2h 5m", "3d 4h".
pub fn uptime(secs: u64) -> String {
    let (m, h, d) = (secs / 60 % 60, secs / 3600 % 24, secs / 86400);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// The repository whose shared git folder (or folder) is `key`, named.
fn repo_name(key: &Path) -> String {
    let dir = if key.file_name().is_some_and(|n| n == ".git") {
        key.parent().unwrap_or(key)
    } else {
        key
    };
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The git operations running now as tasks: one per repository.
fn git_tasks(turns: &[(std::path::PathBuf, String)]) -> Vec<Task> {
    turns
        .iter()
        .map(|(key, what)| {
            let repo = repo_name(key);
            let what = what.strip_prefix("a ").unwrap_or(what);
            Task {
                title: format!("Git: {what} in {repo}"),
                chip: format!("{repo}: {what}"),
                detail: key.display().to_string(),
                dot: Dot::Working,
                actions: vec![("Repositories", MenuAction::Repos)],
            }
        })
        .collect()
}

/// A profile being imported as a task (see `profile_import`).
fn import_task(app: Option<(&'static str, bool)>) -> Option<Task> {
    let (name, closing) = app?;
    let detail = if closing {
        format!("{name} closes for it, and starts again after")
    } else {
        "Copying what gerdr keeps of it".to_owned()
    };
    Some(Task {
        title: format!("Importing {name}'s profile"),
        chip: format!("Importing {name}"),
        detail,
        dot: Dot::Working,
        actions: Vec::new(),
    })
}

/// The apps kept running as tasks: one per project.
fn kept_tasks(kept: &[Kept]) -> Vec<Task> {
    kept.iter()
        .map(|k| Task {
            title: format!("{} kept running for {}", k.apps.join(", "), k.name),
            chip: format!("{} · {}", k.apps.join(", "), k.name),
            detail: "Its windows come back when the project opens again".to_owned(),
            dot: Dot::Done,
            actions: vec![("Stop", MenuAction::StopKept(k.id.clone()))],
        })
        .collect()
}

impl Gerdr {
    /// Look at the apps projects keep running every `KEPT_EVERY`.
    pub fn watch_tasks(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        self.look_at_kept();
        let tick = |_, _: &mut (), state: &mut Gerdr| {
            state.look_at_kept();
            TimeoutAction::ToDuration(KEPT_EVERY)
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(KEPT_EVERY), tick)
        {
            tracing::warn!("background tasks: {err}");
        }
    }

    /// Look again at what reads files: the apps kept, T3 Code's server
    /// gerdr started and its log.
    pub fn look_at_kept(&mut self) {
        self.t3_started = t3_server::started();
        self.t3_log = launch::log_path(t3_server::LOG).exists();
        self.kept = self
            .projects
            .projects
            .iter()
            .filter_map(|p| {
                let apps = keep::app_names(&p.id);
                (!apps.is_empty()).then(|| Kept {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    apps,
                })
            })
            .collect();
    }

    /// T3 Code's server and its update check, as tasks: none if T3 Code
    /// never ran here.
    fn t3_tasks(&self) -> Vec<Task> {
        let started = self.t3_started.clone();
        if self.t3.home.is_none() && started.is_none() {
            return Vec::new();
        }
        let mut tasks = Vec::new();
        let log = launch::log_path(t3_server::LOG);
        let has_log = self.t3_log;
        let server = self.t3.server.as_ref().filter(|_| self.t3.alive);
        let task = if self.t3.stopping {
            Task {
                title: "T3 Code server".to_owned(),
                chip: "T3 Code stopping".to_owned(),
                detail: "Stopping…".to_owned(),
                dot: Dot::Working,
                actions: Vec::new(),
            }
        } else if let Some(server) = server {
            let mut detail = Vec::new();
            if let Some(version) = &server.version {
                detail.push(version.clone());
            }
            detail.push(server.origin.clone());
            match &started {
                Some(started) => detail.push(format!(
                    "up {}",
                    uptime(crate::clock::now_secs().saturating_sub(started.at))
                )),
                None => detail.push("started outside gerdr".to_owned()),
            }
            let mut actions = vec![
                ("Open", MenuAction::OpenT3),
                ("Restart", MenuAction::RestartT3),
                ("Stop", MenuAction::StopT3),
            ];
            if started.is_some() && has_log {
                actions.push(("Log", MenuAction::ShowLog(log.clone())));
            }
            // This version's app didn't take a workspace's project filter
            // (see `t3_unscoped`): kept in sight until another runs.
            let broken = self
                .t3
                .unscoped_in
                .as_ref()
                .filter(|(version, _)| *version == server.version);
            if let Some((_, reason)) = broken {
                let why = crate::t3_agents::unscoped_words(reason);
                detail.insert(0, format!("Project filter broken: {why}"));
                actions.push(("gerdr log", MenuAction::OpenLog));
            }
            Task {
                title: "T3 Code server".to_owned(),
                chip: if broken.is_some() {
                    "T3 Code: filter broken".to_owned()
                } else {
                    "T3 Code".to_owned()
                },
                detail: detail.join(" · "),
                dot: if broken.is_some() {
                    Dot::Failed
                } else {
                    Dot::Done
                },
                actions,
            }
        } else if started.is_some() || self.t3.started_at.is_some() {
            Task {
                title: "T3 Code server".to_owned(),
                chip: "T3 Code starting".to_owned(),
                detail: "Starting…".to_owned(),
                dot: Dot::Working,
                actions: vec![
                    ("Stop", MenuAction::StopT3),
                    ("Log", MenuAction::ShowLog(log.clone())),
                ],
            }
        } else {
            let mut actions = vec![("Start", MenuAction::StartT3)];
            if has_log {
                actions.push(("Log", MenuAction::ShowLog(log.clone())));
            }
            Task {
                title: "T3 Code server".to_owned(),
                chip: "T3 Code stopped".to_owned(),
                detail: "Not running".to_owned(),
                dot: Dot::Idle,
                actions,
            }
        };
        tasks.push(task);
        if self.t3.checking {
            tasks.push(Task {
                title: "T3 Code update".to_owned(),
                chip: "Checking T3 Code".to_owned(),
                detail: "Looking for a newer version gerdr reads".to_owned(),
                dot: Dot::Working,
                actions: Vec::new(),
            });
        } else if let Some(version) = &self.t3.restart_at {
            let busy = self.t3_busy();
            let threads = if busy == 1 { "thread" } else { "threads" };
            tasks.push(Task {
                title: format!("T3 Code {version} waits to start"),
                chip: format!("T3 Code {version} waits"),
                detail: format!("Restarts once {busy} {threads} are done"),
                dot: Dot::Blocked,
                actions: vec![("Restart now", MenuAction::RestartT3)],
            });
        }
        tasks
    }

    /// Every task running in the background, in the footer's order.
    pub fn tasks(&self) -> Vec<Task> {
        let mut tasks = self.t3_tasks();
        tasks.extend(git_tasks(&git_ops::turns()));
        tasks.extend(import_task(crate::profile_import::importing()));
        tasks.extend(kept_tasks(&self.kept));
        tasks.extend(self.feature_tasks());
        tasks.extend(self.machine_tasks());
        tasks.extend(self.sandbox_left_tasks());
        tasks.extend(self.take_tasks());
        tasks.extend(self.ai_work_tasks());
        tasks
    }

    /// How many tasks run: what the footer and the launcher count.
    pub fn running_tasks(tasks: &[Task]) -> usize {
        tasks.iter().filter(|t| t.runs()).count()
    }

    /// How high the footer is: nothing when the config file hides it.
    pub fn footer_height(&self) -> i32 {
        if self.show_footer {
            chrome::status_bar_height(self.density)
        } else {
            0
        }
    }

    /// Where the footer is, in window coordinates.
    pub fn footer_rect(&self) -> chrome::Rect {
        let size = self.output_size();
        let h = self.footer_height();
        Rectangle::new((0, size.h - h).into(), (size.w, h).into())
    }

    /// Whether `pos` is on the footer.
    pub fn on_footer(&self, pos: Point<f64, Logical>) -> bool {
        self.show_footer && self.footer_rect().to_f64().contains(pos)
    }

    /// The footer: a chip per task that runs.
    pub fn footer_view(&self, tasks: &[Task]) -> Option<FooterView> {
        self.show_footer.then(|| FooterView {
            tasks: tasks.iter().filter(|t| t.runs()).map(Task::view).collect(),
            hover: self.footer_hover,
        })
    }

    /// Which of `tasks` the footer's chip `chip` is.
    fn footer_task(tasks: &[Task], chip: usize) -> Option<usize> {
        tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.runs())
            .nth(chip)
            .map(|(i, _)| i)
    }

    /// What of the footer is at `pos`, if any.
    pub fn footer_hit(&self, pos: Point<f64, Logical>) -> Option<FooterHit> {
        if !self.on_footer(pos) {
            return None;
        }
        let rect = self.footer_rect();
        let view = self.footer_view(&self.tasks())?;
        let local = Point::from((pos.x, pos.y - rect.loc.y as f64));
        chrome::footer_layout(&view, rect.size).hit(local)
    }

    /// A press on the footer: a task or the count opens the tasks panel; a
    /// machine's chip opens the machines panel on it.
    pub fn footer_press(&mut self, pos: Point<f64, Logical>) {
        match self.footer_hit(pos) {
            Some(FooterHit::Task(chip)) => {
                let tasks = self.tasks();
                let i = Self::footer_task(&tasks, chip).unwrap_or(0);
                let machine = tasks.get(i).and_then(|t| match t.actions.first() {
                    Some((_, MenuAction::OpenMachines(name))) => Some(name.clone()),
                    _ => None,
                });
                match machine {
                    Some(name) => self.open_machines(Some(&name)),
                    None => self.open_tasks(i),
                }
            }
            Some(FooterHit::Count) => self.open_tasks(0),
            None => {}
        }
    }

    /// The pointer moved: light what of the footer it is on, and offer
    /// what a chip is for.
    pub fn footer_motion(&mut self, pos: Option<Point<f64, Logical>>) -> Option<Tooltip> {
        let hit = pos.and_then(|pos| self.footer_hit(pos));
        self.footer_hover = hit;
        let hit = hit?;
        let rect = self.footer_rect();
        let tasks = self.tasks();
        let view = self.footer_view(&tasks)?;
        let layout = chrome::footer_layout(&view, rect.size);
        let shift = |r: chrome::Rect| Rectangle::new(r.loc + Point::from((0, rect.loc.y)), r.size);
        match hit {
            FooterHit::Task(chip) => {
                let task = tasks.get(Self::footer_task(&tasks, chip)?)?;
                let text = format!("{}: {}", task.title, task.detail);
                Some(Tooltip::new(&text, shift(*layout.chips.get(chip)?)))
            }
            FooterHit::Count => Some(Tooltip::new("Background tasks", shift(layout.count))),
        }
    }

    /// Open the tasks panel at task `selected`.
    /// The other panels close: it would cover them.
    pub fn open_tasks(&mut self, selected: usize) {
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.tasks_panel = Some(TasksPanel {
            selected,
            ..TasksPanel::default()
        });
        self.reveal_task();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the tasks panel.
    pub fn close_tasks(&mut self) {
        self.tasks_panel = None;
        self.refresh_pointer_focus();
    }

    /// The key hints under the tasks, for task `task` selected with its
    /// button `button` focused.
    fn tasks_hints(task: Option<&Task>, button: usize) -> Vec<(String, String)> {
        let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
        let mut hints = vec![hint("Up / Down", "move")];
        if let Some(task) = task {
            if task.actions.len() > 1 {
                hints.push(hint("Tab", "next button"));
            }
            if let Some((label, _)) = task.actions.get(button.min(task.actions.len() - 1)) {
                hints.push(hint("Enter", &label.to_lowercase()));
            }
        }
        hints.push(hint("Esc", "close"));
        hints
    }

    pub fn tasks_view(&self, tasks: &[Task]) -> Option<TasksView> {
        let panel = self.tasks_panel.as_ref()?;
        let selected = (!tasks.is_empty()).then(|| panel.selected.min(tasks.len() - 1));
        Some(TasksView {
            rows: tasks.iter().map(Task::view).collect(),
            first: panel.first,
            selected,
            button: panel.button,
            hover: panel.hover,
            hints: Self::tasks_hints(selected.and_then(|i| tasks.get(i)), panel.button),
        })
    }

    fn tasks_layout(&self) -> Option<(Vec<Task>, TasksView, chrome::TasksLayout)> {
        let tasks = self.tasks();
        let view = self.tasks_view(&tasks)?;
        let layout = chrome::tasks_layout(&view, self.output_size(), self.footer_height());
        Some((tasks, view, layout))
    }

    /// How many tasks the panel shows at once.
    fn tasks_page(&self, count: usize) -> usize {
        chrome::tasks_shown(count, self.output_size(), self.footer_height()).max(1)
    }

    /// Scroll the tasks panel so the selected task shows.
    fn reveal_task(&mut self) {
        let count = self.tasks().len();
        let page = self.tasks_page(count);
        if let Some(panel) = self.tasks_panel.as_mut() {
            panel.selected = panel.selected.min(count.saturating_sub(1));
            if panel.selected < panel.first {
                panel.first = panel.selected;
            } else if panel.selected >= panel.first + page {
                panel.first = panel.selected + 1 - page;
            }
            panel.first = panel.first.min(count.saturating_sub(page));
        }
    }

    /// A key while the tasks panel has the keyboard: Up/Down (Ctrl+K/J,
    /// k/j), PgUp/PgDn, Home/End move between the tasks; Tab/Shift+Tab
    /// (Right/Left) between the selected one's buttons; Enter presses the
    /// focused button; Esc closes.
    pub fn tasks_key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        let tasks = self.tasks();
        let count = tasks.len();
        let page = self.tasks_page(count);
        let panel = self.tasks_panel.as_mut()?;
        let last = count.saturating_sub(1);
        panel.selected = panel.selected.min(last);
        let row = panel.selected;
        let buttons = tasks.get(row).map_or(0, |t| t.actions.len());
        let plain = !mods.ctrl && !mods.alt && !mods.logo;
        let mut to = None;
        match sym {
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::Up | Keysym::KP_Up => to = Some(row.saturating_sub(1)),
            Keysym::k if plain || (mods.ctrl && !mods.alt) => to = Some(row.saturating_sub(1)),
            Keysym::Down | Keysym::KP_Down => to = Some((row + 1).min(last)),
            Keysym::j if plain || (mods.ctrl && !mods.alt) => to = Some((row + 1).min(last)),
            Keysym::Page_Up | Keysym::KP_Page_Up => to = Some(row.saturating_sub(page)),
            Keysym::Page_Down | Keysym::KP_Page_Down => to = Some((row + page).min(last)),
            Keysym::Home | Keysym::KP_Home => to = Some(0),
            Keysym::End | Keysym::KP_End => to = Some(last),
            Keysym::Tab
            | Keysym::ISO_Left_Tab
            | Keysym::Left
            | Keysym::Right
            | Keysym::h
            | Keysym::l
                if plain || matches!(sym, Keysym::Tab | Keysym::ISO_Left_Tab) =>
            {
                let back = matches!(sym, Keysym::ISO_Left_Tab | Keysym::Left | Keysym::h)
                    || (sym == Keysym::Tab && mods.shift);
                let at = panel.button.min(buttons.saturating_sub(1));
                panel.button = if back {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(buttons.saturating_sub(1))
                };
            }
            Keysym::Return | Keysym::KP_Enter => {
                let actions = &tasks.get(row)?.actions;
                let at = panel.button.min(actions.len().checked_sub(1)?);
                return Some(Outcome::Run(actions[at].1.clone()));
            }
            _ => {}
        }
        if let Some(to) = to.filter(|to| *to != row) {
            panel.selected = to;
            panel.button = 0;
            self.reveal_task();
        }
        None
    }

    /// Do what the tasks panel asked: an action leaves it open, to see
    /// what it did.
    pub fn apply_tasks(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_tasks(),
            Outcome::Run(action) => {
                // What opens elsewhere closes the panel.
                if matches!(
                    action,
                    MenuAction::OpenT3
                        | MenuAction::ShowLog(_)
                        | MenuAction::SetupLog(..)
                        | MenuAction::Repos
                        | MenuAction::OpenMachines(_)
                ) {
                    self.close_tasks();
                }
                self.menu_action(action);
            }
        }
    }

    /// A left click while the tasks panel is open: off it closes it, on a
    /// row selects it, on a button presses it, on a key hint presses its
    /// key.
    pub fn tasks_click(&mut self, pos: Point<f64, Logical>) {
        let Some((tasks, view, layout)) = self.tasks_layout() else {
            return;
        };
        let outcome = match layout.hit(pos) {
            None => Some(Outcome::Close),
            Some(TasksHit::Panel) => None,
            Some(TasksHit::Hint(i)) => {
                let keys = view.hints.get(i).map(|(k, _)| k.clone());
                if let Some(keys) = keys {
                    self.press_hint(crate::input::Overlay::Tasks, &keys, pos);
                }
                None
            }
            Some(TasksHit::Row(i)) => {
                if let Some(panel) = self.tasks_panel.as_mut()
                    && panel.selected != i
                {
                    panel.selected = i;
                    panel.button = 0;
                }
                None
            }
            Some(TasksHit::Button(i, b)) => {
                if let Some(panel) = self.tasks_panel.as_mut() {
                    panel.selected = i;
                    panel.button = b;
                }
                tasks
                    .get(i)
                    .and_then(|t| t.actions.get(b))
                    .map(|(_, action)| Outcome::Run(action.clone()))
            }
        };
        if let Some(outcome) = outcome {
            self.apply_tasks(outcome);
        }
    }

    /// The pointer moved while the tasks panel is open.
    pub fn tasks_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = self
            .tasks_layout()
            .and_then(|(_, _, layout)| layout.hit(pos));
        if let Some(panel) = self.tasks_panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel or touchpad over the tasks panel: `rows` rows on.
    pub fn tasks_scroll(&mut self, rows: i32) {
        let count = self.tasks().len();
        let page = self.tasks_page(count);
        if let Some(panel) = self.tasks_panel.as_mut() {
            let first = panel.first as i64 + rows as i64;
            panel.first = first.clamp(0, count.saturating_sub(page) as i64) as usize;
        }
    }

    /// The rows of the tasks panel, as laid out now: for how far a notch
    /// of the wheel goes.
    pub fn tasks_rows(&self) -> Vec<chrome::Rect> {
        self.tasks_layout()
            .map_or_else(Vec::new, |(_, _, layout)| layout.rows)
    }

    /// The buttons of the tasks panel, while it is open.
    pub fn tasks_controls(&self) -> Vec<chrome::Rect> {
        self.tasks_layout()
            .map_or_else(Vec::new, |(_, _, layout)| layout.controls())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_reads_shortly() {
        assert_eq!(uptime(42), "42s");
        assert_eq!(uptime(5 * 60 + 3), "5m");
        assert_eq!(uptime(2 * 3600 + 5 * 60), "2h 5m");
        assert_eq!(uptime(3 * 86400 + 4 * 3600 + 59), "3d 4h");
    }

    #[test]
    fn git_operations_are_named_by_repository() {
        let turns = vec![
            ("/home/me/gerdr/.git".into(), "a fetch".to_owned()),
            ("/home/me/plain".into(), "a push".to_owned()),
        ];
        let tasks = git_tasks(&turns);
        assert_eq!(tasks[0].chip, "gerdr: fetch");
        assert_eq!(tasks[1].title, "Git: push in plain");
        assert!(tasks.iter().all(|t| t.dot == Dot::Working));
    }

    #[test]
    fn kept_apps_stop_by_project() {
        let kept = vec![Kept {
            id: "gerdr".into(),
            name: "gerdr".into(),
            apps: vec!["Cursor".into()],
        }];
        let tasks = kept_tasks(&kept);
        assert_eq!(tasks[0].chip, "Cursor · gerdr");
        assert_eq!(
            tasks[0].actions,
            vec![("Stop", MenuAction::StopKept("gerdr".into()))]
        );
    }
}
