//! Pausing an agent: its pane's processes stopped with SIGSTOP and let go
//! with SIGCONT, so it goes on where it was.
//!
//! The pane's shell stops first. A shell whose foreground job stops takes
//! the terminal back ("Stopped"), and the job would come back in the
//! background, cut off from its terminal. Stopped, the shell can't see
//! that, and the job is let go before it, so it never does. Each process
//! is kept with its start time: one that took its pid later is left alone.
//!
//! Only a pane's own tree is signalled, from its shell down, never a scan
//! of what else runs; `process::kill_pid` refuses gerdr and init.

use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel;

use crate::{
    Gerdr,
    chrome::Dot,
    herdr::{self, Herdr},
    menu::MenuAction,
    process::{self, Process},
    project_lock,
    tasks::Task,
    toast::Level,
};

/// An agent paused: its pane in its project's herdr session, and what was
/// stopped there. Kept in `state.json`, so a gerdr started again lets it go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paused {
    pub project: String,
    pub pane: String,
    pub procs: Vec<Process>,
}

impl Paused {
    /// Whether something stopped still runs: else there is nothing to let go.
    fn live(&self) -> bool {
        self.procs.iter().any(Process::runs)
    }
}

/// What a pass pausing the agents came to: those paused, and the projects
/// whose agents couldn't be read.
struct Found {
    paused: Vec<Paused>,
    unread: Vec<String>,
}

/// Whether a pass pausing the agents runs.
static PAUSING: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Where a pass sends what it paused, to the event loop.
    static DONE: RefCell<Option<channel::Sender<Found>>> = const { RefCell::new(None) };
}

/// The agents of every project in `sessions` (id, herdr session), but the
/// panes in `skip` (project, pane: views of what runs on other machines),
/// paused.
fn pause_all(sessions: Vec<(String, String)>, skip: &[(String, String)]) -> Found {
    let mut found = Found {
        paused: Vec::new(),
        unread: Vec::new(),
    };
    for (project, session) in sessions {
        if !herdr::is_running(&session) {
            continue;
        }
        let mut herdr = Herdr::on_socket(herdr::socket_path(&session));
        let agents = match herdr.agent_list() {
            Ok(agents) => agents,
            Err(err) => {
                tracing::warn!("listing the agents of {project}: {err}");
                found.unread.push(project);
                continue;
            }
        };
        for agent in agents {
            let view = (skip.iter()).any(|(p, pane)| *p == project && *pane == agent.pane_id);
            if agent.agent.is_none() || view {
                continue;
            }
            let info = match herdr.process_info(&agent.pane_id) {
                Ok(info) => info,
                Err(err) => {
                    tracing::warn!("what runs in pane {} of {project}: {err}", agent.pane_id);
                    found.unread.push(project.clone());
                    continue;
                }
            };
            let roots: Vec<u32> = match info.shell_pid {
                Some(shell) => vec![shell],
                None => (info.foreground_processes.iter()).map(|p| p.pid).collect(),
            }
            .into_iter()
            .filter_map(|pid| u32::try_from(pid).ok())
            .collect();
            let procs = freeze(&roots);
            if !procs.is_empty() {
                found.paused.push(Paused {
                    project: project.clone(),
                    pane: agent.pane_id,
                    procs,
                });
            }
        }
    }
    found.unread.dedup();
    found
}

impl Gerdr {
    /// The agents paused that are still there.
    fn paused_live(&self) -> impl Iterator<Item = &Paused> {
        self.projects.paused_agents.iter().filter(|p| p.live())
    }

    /// Whether some agents are paused.
    pub(crate) fn agents_paused(&self) -> bool {
        self.paused_live().next().is_some()
    }

    /// Whether the agent in herdr pane `pane` of the shown project is paused.
    pub(crate) fn pane_paused(&self, pane: &str) -> bool {
        let project = self.project.as_deref();
        (self.paused_live()).any(|p| Some(p.project.as_str()) == project && p.pane == pane)
    }

    /// herdr's list of the shown project's agents, but a paused one's status
    /// unknown: herdr's is from before it stopped, and would show it at work.
    pub(crate) fn mark_paused_agents(&mut self) {
        let project = self.project.clone();
        let paused: Vec<String> = (self.paused_live())
            .filter(|p| Some(&p.project) == project.as_ref())
            .map(|p| p.pane.clone())
            .collect();
        for agent in &mut self.session.agents {
            if paused.contains(&agent.pane_id) {
                agent.agent_status = herdr::AgentStatus::Unknown;
            }
        }
    }

    /// Pause every agent of every project, or let them go on while some are
    /// paused.
    pub(crate) fn toggle_agents_paused(&mut self) {
        if PAUSING.load(Ordering::Relaxed) {
            return self.show_notice("Pausing the agents; try again in a moment".into());
        }
        if self.agents_paused() {
            self.resume_agents();
        } else {
            self.pause_agents();
        }
    }

    /// Pause the agents of every project no other gerdr shows, off the
    /// event loop: herdr is asked for each. Those on other machines go on.
    fn pause_agents(&mut self) {
        let Some(done) = self.pause_channel() else {
            return;
        };
        let lock_dir = project_lock::dir();
        let shown = self.project.as_deref();
        // The lock test counts this gerdr's own lock: the shown project is ours.
        let sessions: Vec<(String, String)> = (self.projects.projects.iter())
            .filter(|p| {
                shown == Some(p.id.as_str())
                    || !(lock_dir.as_ref())
                        .is_some_and(|dir| project_lock::is_open_elsewhere(dir, &p.id))
            })
            .map(|p| (p.id.clone(), p.session.clone()))
            .collect();
        let shown = self.project.clone().unwrap_or_default();
        let skip: Vec<(String, String)> = (self.machines.views.values())
            .map(|pane| (shown.clone(), pane.clone()))
            .collect();
        PAUSING.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            let found = pause_all(sessions, &skip);
            // The loop is gone only as gerdr quits.
            let _ = done.send(found);
        });
    }

    /// The channel a pass sends what it paused on, made once.
    fn pause_channel(&mut self) -> Option<channel::Sender<Found>> {
        if let Some(tx) = DONE.with(|done| done.borrow().clone()) {
            return Some(tx);
        }
        let (tx, rx) = channel::channel::<Found>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(found) = event {
                state.agents_paused_now(found);
            }
        });
        if let Err(err) = inserted {
            tracing::warn!("pausing the agents: {err}");
            self.show_error(format!("Couldn't pause the agents: {err}"));
            return None;
        }
        DONE.with(|done| *done.borrow_mut() = Some(tx.clone()));
        Some(tx)
    }

    /// A pass paused `found`'s agents: kept, and said.
    fn agents_paused_now(&mut self, found: Found) {
        PAUSING.store(false, Ordering::Relaxed);
        let n = found.paused.len();
        self.projects.paused_agents.retain(Paused::live);
        self.projects.paused_agents.extend(found.paused);
        self.projects.save();
        self.mark_paused_agents();
        if !found.unread.is_empty() {
            let names: Vec<String> = (found.unread.iter())
                .map(|id| {
                    self.projects
                        .project(id)
                        .map_or_else(|| id.clone(), |p| p.name.clone())
                })
                .collect();
            self.show_warning(format!(
                "Couldn't read the agents of {}: they go on",
                names.join(", ")
            ));
        }
        if n == 0 {
            return self.show_notice("No agent runs to pause".into());
        }
        let text = match n {
            1 => "Paused 1 agent".to_owned(),
            n => format!("Paused {n} agents"),
        };
        let button = ("Resume".to_owned(), MenuAction::ToggleAgentsPaused);
        self.toast_with(text, Level::Success, Some(button));
    }

    /// Let every paused agent go on.
    fn resume_agents(&mut self) {
        let paused = std::mem::take(&mut self.projects.paused_agents);
        self.projects.save();
        let n = paused.iter().filter(|p| thaw(&p.procs) > 0).count();
        match n {
            0 => self.show_notice("The paused agents ended meanwhile".into()),
            1 => self.show_success("Resumed 1 agent".into()),
            n => self.show_success(format!("Resumed {n} agents")),
        }
    }

    /// The footer's chip while agents are paused, with a Resume button.
    pub(crate) fn paused_task(&self) -> Option<Task> {
        let n = self.paused_live().count();
        (n > 0).then(|| Task {
            title: "Agents paused".to_owned(),
            chip: match n {
                1 => "1 agent paused".to_owned(),
                n => format!("{n} agents paused"),
            },
            detail: "Stopped where they were until resumed".to_owned(),
            dot: Dot::Blocked,
            actions: vec![("Resume", MenuAction::ToggleAgentsPaused)],
        })
    }

    /// A stage agent can't start while the agents are paused: say so, with
    /// a Resume button. Whether it can.
    pub(crate) fn starts_while_paused(&mut self) -> bool {
        if !self.agents_paused() {
            return true;
        }
        let button = ("Resume".to_owned(), MenuAction::ToggleAgentsPaused);
        self.toast_with(
            "Agents are paused: resume them, then start it".into(),
            Level::Warning,
            Some(button),
        );
        false
    }
}

/// How many times the tree is walked again for children started while it
/// was being stopped.
const WALKS: usize = 4;

/// The children of `pid`, as its threads list them.
fn children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    (tasks.flatten())
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|list| {
            (list.split_whitespace())
                .filter_map(|pid| pid.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// `roots` and everything under them, parents before their children.
fn tree(roots: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut next = roots.to_vec();
    while !next.is_empty() {
        let mut deeper = Vec::new();
        for pid in next {
            if !out.contains(&pid) {
                out.push(pid);
                deeper.extend(children(pid));
            }
        }
        next = deeper;
    }
    out
}

/// Stop `roots` (a pane's shell, or what runs in it without one) and all
/// under them, parents first. What was stopped, to let go with `thaw`.
pub fn freeze(roots: &[u32]) -> Vec<Process> {
    // Not from gerdr or init down: all else would stop.
    let roots: Vec<u32> = (roots.iter().copied())
        .filter(|pid| *pid > 1 && *pid != std::process::id())
        .collect();
    let mut stopped: Vec<Process> = Vec::new();
    for _ in 0..WALKS {
        let fresh: Vec<u32> = (tree(&roots).into_iter())
            .filter(|pid| !stopped.iter().any(|p| p.pid == *pid))
            .collect();
        if fresh.is_empty() {
            break;
        }
        for pid in fresh {
            let p = Process::new(pid);
            if p.start != 0 && process::kill_pid(pid, libc::SIGSTOP) {
                stopped.push(p);
            }
        }
    }
    stopped
}

/// Let `stopped` go on, children before their parents: the shell last.
/// One whose pid another process took since is left alone. How many were.
pub fn thaw(stopped: &[Process]) -> usize {
    (stopped.iter().rev())
        .filter(|p| p.runs() && process::kill_pid(p.pid, libc::SIGCONT))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// The state letter of `pid`: `T` stopped, `S` sleeping.
    fn state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.trim_start().chars().next()
    }

    /// Wait for `pid` to come to state `want`.
    fn comes_to(pid: u32, want: char) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if state(pid) == Some(want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_tree_stops_from_its_root_down_and_goes_on_again() {
        // Its own group and its own children only: nothing else is signalled.
        let mut sh = Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .expect("sh runs");
        let root = sh.id();
        let until = Instant::now() + Duration::from_secs(5);
        while tree(&[root]).len() < 3 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let stopped = freeze(&[root]);
        assert_eq!(stopped.len(), 3, "{stopped:?}");
        assert_eq!(stopped[0].pid, root, "the root stops first");
        assert!(stopped.iter().all(|p| comes_to(p.pid, 'T')));
        assert_eq!(thaw(&stopped), 3);
        assert!(stopped.iter().all(|p| comes_to(p.pid, 'S')));
        process::kill_group(process::group_of(&sh), libc::SIGKILL);
        let _ = sh.wait();
    }

    #[test]
    fn what_ended_is_neither_stopped_nor_let_go() {
        assert!(freeze(&[u32::MAX]).is_empty());
        let gone = Process {
            pid: std::process::id(),
            start: 1,
        };
        assert_eq!(thaw(&[gone]), 0);
        // Nothing under gerdr or init is walked.
        assert!(freeze(&[1]).is_empty());
    }
}
