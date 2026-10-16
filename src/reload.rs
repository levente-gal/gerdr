//! Reloading the projects file after it was edited by hand: reading it
//! back, and telling a running herdr session about what moved. Inside a
//! tab, gerdr's saved tree is what is shown once the project is attached
//! again; herdr needs to hear only which tab each pane is in, the order of
//! the tabs and workspaces, and the workspaces' names.

use std::collections::{HashMap, HashSet};

use crate::{
    Gerdr,
    herdr::{self, Herdr},
    layout::{Dir, SavedLeaf, SavedNode},
    projects::{self, Project},
    toast::Level,
};

/// A herdr workspace as it is now.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveWorkspace {
    pub id: String,
    pub label: String,
    /// Its tabs, in order, with their panes.
    pub tabs: Vec<(String, Vec<String>)>,
}

/// A tab of the file: tab `.1` of the project's workspace record `.0`.
type TabKey = (usize, usize);

/// What to tell herdr so it matches the file.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// The live tab each tab of the file is, if any; the others are new.
    pub tabs: HashMap<TabKey, String>,
    /// Panes to move into a tab of the file, in order.
    pub moves: Vec<(String, TabKey)>,
    /// Each workspace's tabs of the file holding herdr panes, in order.
    pub tab_order: Vec<(String, Vec<TabKey>)>,
    /// The workspaces, in the file's order.
    pub workspace_order: Vec<String>,
    /// Workspaces to rename.
    pub renames: Vec<(String, String)>,
    /// What can't be done.
    pub warnings: Vec<String>,
}

/// What it takes for `live` to match `project`.
pub fn plan(project: &Project, live: &[LiveWorkspace]) -> Plan {
    let mut plan = Plan::default();
    let where_is: HashMap<&str, &str> = live
        .iter()
        .flat_map(|ws| &ws.tabs)
        .flat_map(|(tab, panes)| panes.iter().map(move |p| (p.as_str(), tab.as_str())))
        .collect();
    let mut seen = HashSet::new();
    let mut taken = HashSet::new();
    for (w, record) in project.workspaces.iter().enumerate() {
        let Some(ws) = live.iter().find(|ws| ws.id == record.herdr_id) else {
            plan.warnings
                .push(format!("workspace {} isn't open in herdr", record.herdr_id));
            continue;
        };
        plan.workspace_order.push(ws.id.clone());
        if !record.label.is_empty() && record.label != ws.label {
            plan.renames.push((ws.id.clone(), record.label.clone()));
        }
        let mut order = Vec::new();
        for (t, tab) in record.tabs.iter().enumerate() {
            let key = (w, t);
            let panes: Vec<&str> = tab
                .layout
                .herdr_panes()
                .into_iter()
                .filter(|pane| {
                    if !where_is.contains_key(pane) {
                        plan.warnings.push(format!("no pane {pane} in herdr"));
                        false
                    } else if !seen.insert(*pane) {
                        plan.warnings.push(format!("pane {pane} is in two tabs"));
                        false
                    } else {
                        true
                    }
                })
                .collect();
            if panes.is_empty() {
                continue;
            }
            let here = tab
                .herdr_tab
                .as_ref()
                .filter(|id| ws.tabs.iter().any(|(tab, _)| tab == *id))
                .filter(|id| taken.insert(id.to_string()));
            if let Some(id) = here {
                plan.tabs.insert(key, id.clone());
            }
            for pane in panes {
                if here.map(String::as_str) != where_is.get(pane).copied() {
                    plan.moves.push((pane.to_owned(), key));
                }
            }
            order.push(key);
        }
        plan.tab_order.push((ws.id.clone(), order));
    }
    plan
}

/// Where a pane moved: its new id (another workspace's gives it one) and tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    pub pane_id: String,
    pub workspace_id: String,
}

/// Write into `project` what the moves changed: the panes' new ids, and the
/// tabs' herdr ids. A pane record follows its pane to another workspace.
pub fn patch(
    project: &mut Project,
    moved: &HashMap<String, Moved>,
    tabs: &HashMap<TabKey, String>,
) {
    for (w, record) in project.workspaces.iter_mut().enumerate() {
        for (t, tab) in record.tabs.iter_mut().enumerate() {
            rename_panes(&mut tab.layout, moved);
            if let Some(id) = tabs.get(&(w, t)) {
                tab.herdr_tab = Some(id.clone());
            }
        }
    }
    let mut travelling = Vec::new();
    for record in &mut project.workspaces {
        let herdr_id = record.herdr_id.clone();
        record.panes.retain_mut(|pane| {
            let Some(to) = moved.get(&pane.pane_id) else {
                return true;
            };
            pane.pane_id = to.pane_id.clone();
            if to.workspace_id == herdr_id {
                return true;
            }
            travelling.push((to.workspace_id.clone(), pane.clone()));
            false
        });
    }
    for (workspace_id, pane) in travelling {
        if let Some(record) = project
            .workspaces
            .iter_mut()
            .find(|r| r.herdr_id == workspace_id)
        {
            record.panes.push(pane);
        }
    }
}

fn rename_panes(node: &mut SavedNode, moved: &HashMap<String, Moved>) {
    match node {
        SavedNode::Leaf(SavedLeaf::Herdr { pane_id }) => {
            if let Some(to) = moved.get(pane_id.as_str()) {
                *pane_id = to.pane_id.clone();
            }
        }
        SavedNode::Leaf(_) => {}
        SavedNode::Split { first, second, .. } => {
            rename_panes(first, moved);
            rename_panes(second, moved);
        }
    }
}

/// The session's workspaces, tabs and panes, as herdr has them.
fn live(herdr: &mut Herdr) -> Result<Vec<LiveWorkspace>, String> {
    let mut out = Vec::new();
    for ws in herdr.workspace_list()? {
        let panes = herdr.pane_list(&ws.workspace_id)?;
        let tabs = herdr
            .tab_list(&ws.workspace_id)?
            .into_iter()
            .map(|tab| {
                let in_tab = panes
                    .iter()
                    .filter(|p| p.tab_id == tab)
                    .map(|p| p.pane_id.clone())
                    .collect();
                (tab, in_tab)
            })
            .collect();
        out.push(LiveWorkspace {
            id: ws.workspace_id,
            label: ws.label,
            tabs,
        });
    }
    Ok(out)
}

/// Make the running session `herdr` match `project`, and `project` match
/// the ids herdr gave what moved. Returns what couldn't be done.
pub fn apply(herdr: &mut Herdr, project: &mut Project) -> Vec<String> {
    let live = match live(herdr) {
        Ok(live) => live,
        Err(err) => return vec![err],
    };
    let plan = plan(project, &live);
    let mut problems = plan.warnings;
    let mut tabs = plan.tabs;
    // How many panes each live tab holds: one whose last pane leaves is gone.
    let mut count: HashMap<String, usize> = live
        .iter()
        .flat_map(|ws| &ws.tabs)
        .map(|(tab, panes)| (tab.clone(), panes.len()))
        .collect();
    let mut tab_of: HashMap<String, String> = live
        .iter()
        .flat_map(|ws| &ws.tabs)
        .flat_map(|(tab, panes)| panes.iter().map(move |p| (p.clone(), tab.clone())))
        .collect();
    let mut moved: HashMap<String, Moved> = HashMap::new();
    for (pane, key) in plan.moves {
        let workspace_id = &project.workspaces[key.0].herdr_id;
        let into = tabs
            .get(&key)
            .filter(|tab| count.get(*tab).is_some_and(|n| *n > 0));
        let result = match into {
            Some(tab) => herdr.pane_move(&pane, tab, None, Dir::Right, 0.5),
            None => herdr.pane_move_to_new_tab(&pane, workspace_id),
        };
        let info = match result {
            Ok(info) => info,
            Err(err) => {
                problems.push(format!("{pane}: {err}"));
                continue;
            }
        };
        if let Some(from) = tab_of.remove(&pane)
            && let Some(n) = count.get_mut(&from)
        {
            *n -= 1;
        }
        *count.entry(info.tab_id.clone()).or_default() += 1;
        tab_of.insert(info.pane_id.clone(), info.tab_id.clone());
        tabs.insert(key, info.tab_id);
        if info.pane_id != pane {
            moved.insert(
                pane,
                Moved {
                    pane_id: info.pane_id,
                    workspace_id: info.workspace_id,
                },
            );
        }
    }
    patch(project, &moved, &tabs);

    for (workspace_id, keys) in plan.tab_order {
        let want: Vec<&String> = keys.iter().filter_map(|key| tabs.get(key)).collect();
        let order = match herdr.tab_list(&workspace_id) {
            Ok(order) => order,
            Err(err) => {
                problems.push(err);
                continue;
            }
        };
        let now: Vec<&String> = order.iter().filter(|id| want.contains(id)).collect();
        if now != want {
            for tab in want {
                if let Err(err) = herdr.tab_move(&workspace_id, tab, None) {
                    problems.push(err);
                }
            }
        }
    }
    order_workspaces(
        herdr,
        &live,
        &plan.workspace_order,
        &plan.renames,
        &mut problems,
    );
    problems
}

/// Put the workspaces of the running session `herdr` in `project`'s order,
/// under its labels, and nothing else: for a session that started after
/// the file was edited, whose panes were made again.
pub fn apply_order(herdr: &mut Herdr, project: &Project) -> Vec<String> {
    let live = match live(herdr) {
        Ok(live) => live,
        Err(err) => return vec![err],
    };
    let plan = plan(project, &live);
    let mut problems = Vec::new();
    order_workspaces(
        herdr,
        &live,
        &plan.workspace_order,
        &plan.renames,
        &mut problems,
    );
    problems
}

fn order_workspaces(
    herdr: &mut Herdr,
    live: &[LiveWorkspace],
    want: &[String],
    renames: &[(String, String)],
    problems: &mut Vec<String>,
) {
    let now: Vec<&String> = live
        .iter()
        .map(|ws| &ws.id)
        .filter(|id| want.contains(id))
        .collect();
    if now != want.iter().collect::<Vec<_>>() {
        for ws in want {
            if let Err(err) = herdr.workspace_move(ws, None) {
                problems.push(err);
            }
        }
    }
    for (workspace_id, label) in renames {
        if let Err(err) = herdr.workspace_rename(workspace_id, label) {
            problems.push(err);
        }
    }
}

impl Gerdr {
    /// Read the projects file again, after it was edited outside gerdr, and
    /// make the running sessions match it. The project shown is let go of
    /// and shown again, from the file.
    pub fn reload_projects(&mut self) {
        // A file that can't be read changes nothing, the project shown stays.
        if let Err(err) = projects::Store::read() {
            return self.toast(format!("projects.json: {err}"), Level::Error);
        }
        let shown = self.project.clone();
        if shown.is_some() {
            self.detach_project();
            self.project = None;
        }
        // Read again: letting go saves what changed on screen, unless the
        // file was edited.
        let mut store = match projects::Store::read() {
            Ok(store) => store,
            Err(err) => {
                self.toast(format!("projects.json: {err}"), Level::Error);
                if let Some(id) = shown {
                    self.show_project(&id, false);
                }
                return;
            }
        };
        let mut problems = Vec::new();
        for project in &mut store.projects {
            // Only what was edited: the rest is as gerdr left it.
            let edited = self.projects.project(&project.id) != Some(project);
            if !edited {
                continue;
            }
            // One not running takes the file's order when it starts.
            if !herdr::is_running(&project.session) {
                self.order_pending.insert(project.id.clone());
                continue;
            }
            let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
            problems.extend(
                apply(&mut herdr, project)
                    .into_iter()
                    .map(|problem| format!("{}: {problem}", project.name)),
            );
        }
        for problem in &problems {
            tracing::warn!("reloading projects.json: {problem}");
        }
        self.projects = store;
        self.projects.save();
        match shown {
            Some(id) if self.projects.project(&id).is_some() => self.show_project(&id, false),
            Some(_) => self.open_welcome(false),
            None => {}
        }
        self.refresh_project_menu();
        self.refresh_switcher();
        match problems.as_slice() {
            [] => self.toast("Reloaded projects.json".into(), Level::Info),
            [one] => self.toast(format!("Reloaded projects.json; {one}"), Level::Warning),
            [first, rest @ ..] => self.toast(
                format!("Reloaded projects.json; {first} (and {} more)", rest.len()),
                Level::Warning,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{Kind, PaneRecord, RunState, TabRecord, VenvSetting, WorkspaceRecord};

    fn herdr(id: &str) -> SavedNode {
        SavedNode::Leaf(SavedLeaf::Herdr { pane_id: id.into() })
    }

    fn split(first: SavedNode, second: SavedNode) -> SavedNode {
        SavedNode::Split {
            dir: Dir::Right,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn tab(id: Option<&str>, layout: SavedNode) -> TabRecord {
        TabRecord {
            herdr_tab: id.map(str::to_owned),
            layout,
            label: None,
        }
    }

    fn workspace(id: &str, label: &str, tabs: Vec<TabRecord>) -> WorkspaceRecord {
        WorkspaceRecord {
            herdr_id: id.into(),
            label: label.into(),
            kind: Kind::Project,
            dir: "/w".into(),
            venv: VenvSetting::Auto,
            tabs,
            panes: Vec::new(),
        }
    }

    fn project(workspaces: Vec<WorkspaceRecord>) -> Project {
        Project {
            id: "p".into(),
            name: "P".into(),
            session: "gerdr-p".into(),
            created: 0,
            last_used: 0,
            state: RunState::Kept,
            workspaces,
            before_start: String::new(),
            after_stop: String::new(),
        }
    }

    fn live(id: &str, label: &str, tabs: &[(&str, &[&str])]) -> LiveWorkspace {
        LiveWorkspace {
            id: id.into(),
            label: label.into(),
            tabs: tabs
                .iter()
                .map(|(tab, panes)| {
                    (
                        tab.to_string(),
                        panes.iter().map(|p| p.to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn as_it_is_nothing_moves() {
        let file = project(vec![workspace(
            "w1",
            "api",
            vec![
                tab(
                    Some("w1:t1"),
                    split(herdr("w1:p1"), SavedNode::Leaf(SavedLeaf::Zed)),
                ),
                tab(Some("w1:t2"), herdr("w1:p2")),
            ],
        )]);
        let plan = plan(
            &file,
            &[live(
                "w1",
                "api",
                &[("w1:t1", &["w1:p1"]), ("w1:t2", &["w1:p2"])],
            )],
        );
        assert!(plan.moves.is_empty() && plan.renames.is_empty() && plan.warnings.is_empty());
        assert_eq!(plan.tab_order, [("w1".to_string(), vec![(0, 0), (0, 1)])]);
        assert_eq!(plan.workspace_order, ["w1"]);
    }

    #[test]
    fn a_pane_put_in_another_tab_moves_there() {
        let file = project(vec![workspace(
            "w1",
            "api",
            vec![
                tab(Some("w1:t1"), herdr("w1:p1")),
                tab(Some("w1:t2"), split(herdr("w1:p2"), herdr("w1:p3"))),
            ],
        )]);
        let now = live(
            "w1",
            "api",
            &[("w1:t1", &["w1:p1", "w1:p3"]), ("w1:t2", &["w1:p2"])],
        );
        let plan = plan(&file, &[now]);
        assert_eq!(plan.moves, [("w1:p3".to_string(), (0, 1))]);
        assert_eq!(plan.tabs[&(0, 1)], "w1:t2");
    }

    #[test]
    fn a_tab_herdr_doesnt_have_is_new() {
        let file = project(vec![workspace(
            "w1",
            "api",
            vec![
                tab(Some("w1:t1"), herdr("w1:p1")),
                tab(None, herdr("w1:p2")),
            ],
        )]);
        let plan = plan(
            &file,
            &[live("w1", "api", &[("w1:t1", &["w1:p1", "w1:p2"])])],
        );
        assert_eq!(plan.moves, [("w1:p2".to_string(), (0, 1))]);
        assert!(!plan.tabs.contains_key(&(0, 1)));
    }

    #[test]
    fn swapped_panes_both_move() {
        let file = project(vec![workspace(
            "w1",
            "api",
            vec![
                tab(Some("w1:t1"), herdr("w1:p2")),
                tab(Some("w1:t2"), herdr("w1:p1")),
            ],
        )]);
        let plan = plan(
            &file,
            &[live(
                "w1",
                "api",
                &[("w1:t1", &["w1:p1"]), ("w1:t2", &["w1:p2"])],
            )],
        );
        assert_eq!(
            plan.moves,
            [("w1:p2".to_string(), (0, 0)), ("w1:p1".to_string(), (0, 1))]
        );
    }

    #[test]
    fn order_and_names_follow_the_file() {
        let file = project(vec![
            workspace(
                "w2",
                "web",
                vec![
                    tab(Some("w2:t2"), herdr("w2:p2")),
                    tab(Some("w2:t1"), herdr("w2:p1")),
                ],
            ),
            workspace("w1", "api", vec![tab(Some("w1:t1"), herdr("w1:p1"))]),
        ]);
        let plan = plan(
            &file,
            &[
                live("w1", "api", &[("w1:t1", &["w1:p1"])]),
                live(
                    "w2",
                    "site",
                    &[("w2:t1", &["w2:p1"]), ("w2:t2", &["w2:p2"])],
                ),
            ],
        );
        assert!(plan.moves.is_empty());
        assert_eq!(plan.workspace_order, ["w2", "w1"]);
        assert_eq!(plan.tab_order[0], ("w2".to_string(), vec![(0, 0), (0, 1)]));
        assert_eq!(plan.tabs[&(0, 0)], "w2:t2");
        assert_eq!(plan.renames, [("w2".to_string(), "web".to_string())]);
    }

    #[test]
    fn a_pane_put_in_another_workspace_moves_there() {
        let file = project(vec![
            workspace("w1", "api", vec![tab(Some("w1:t1"), herdr("w1:p1"))]),
            workspace(
                "w2",
                "web",
                vec![tab(Some("w2:t1"), split(herdr("w2:p1"), herdr("w1:p2")))],
            ),
        ]);
        let plan = plan(
            &file,
            &[
                live("w1", "api", &[("w1:t1", &["w1:p1", "w1:p2"])]),
                live("w2", "web", &[("w2:t1", &["w2:p1"])]),
            ],
        );
        assert_eq!(plan.moves, [("w1:p2".to_string(), (1, 0))]);
    }

    #[test]
    fn unknown_and_doubled_panes_are_warned_about() {
        let file = project(vec![
            workspace(
                "w1",
                "api",
                vec![
                    tab(Some("w1:t1"), split(herdr("w1:p1"), herdr("w1:p9"))),
                    tab(Some("w1:t2"), herdr("w1:p1")),
                ],
            ),
            workspace("w7", "gone", vec![tab(Some("w7:t1"), herdr("w7:p1"))]),
        ]);
        let plan = plan(&file, &[live("w1", "api", &[("w1:t1", &["w1:p1"])])]);
        assert!(plan.moves.is_empty());
        assert_eq!(
            plan.warnings,
            [
                "no pane w1:p9 in herdr",
                "pane w1:p1 is in two tabs",
                "workspace w7 isn't open in herdr"
            ]
        );
        // The tab left with no pane herdr has isn't ordered.
        assert_eq!(plan.tab_order, [("w1".to_string(), vec![(0, 0)])]);
    }

    #[test]
    fn patching_follows_the_moves() {
        let mut file = project(vec![
            workspace("w1", "api", vec![tab(Some("w1:t1"), herdr("w1:p1"))]),
            workspace(
                "w2",
                "web",
                vec![tab(None, split(herdr("w2:p1"), herdr("w1:p2")))],
            ),
        ]);
        file.workspaces[0].panes = vec![
            PaneRecord {
                pane_id: "w1:p1".into(),
                ..PaneRecord::default()
            },
            PaneRecord {
                pane_id: "w1:p2".into(),
                command: Some("make serve".into()),
                ..PaneRecord::default()
            },
        ];
        let moved = HashMap::from([(
            "w1:p2".to_string(),
            Moved {
                pane_id: "w2:p4".into(),
                workspace_id: "w2".into(),
            },
        )]);
        let tabs = HashMap::from([((1, 0), "w2:t3".to_string())]);
        patch(&mut file, &moved, &tabs);
        let web = &file.workspaces[1];
        assert_eq!(web.tabs[0].herdr_tab.as_deref(), Some("w2:t3"));
        assert_eq!(web.tabs[0].layout.herdr_panes(), ["w2:p1", "w2:p4"]);
        assert_eq!(web.panes.len(), 1);
        assert_eq!(web.panes[0].pane_id, "w2:p4");
        assert_eq!(web.panes[0].command.as_deref(), Some("make serve"));
        assert_eq!(file.workspaces[0].panes.len(), 1);
    }
}
