//! Processes gerdr started: told apart from one that took their pid later,
//! and stopped by process group, never gerdr's own or init's.

use serde::{Deserialize, Serialize};
use std::process::Child;
use std::time::{Duration, Instant};

/// A process gerdr started, told apart from one that took its pid later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    /// When it started, in clock ticks after boot (see `start_time`).
    pub start: u64,
}

impl Process {
    /// `pid`, started a moment ago.
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            start: start_time(pid).unwrap_or_default(),
        }
    }

    /// Whether it still runs.
    pub fn runs(&self) -> bool {
        start_time(self.pid) == Some(self.start)
    }
}

/// When `pid` started, in clock ticks after boot; none when it doesn't run.
pub fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, in parentheses, may hold spaces: the start time is
    // the 20th field after it.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Send `signal` to every process of process group `group`. Groups 0 and 1
/// (the caller's and init's, as `kill` reads them) and bad ids (-1 from a
/// failed `getpgid`) are refused, and so is gerdr's own group. Whether it
/// was sent.
pub fn kill_group(group: i32, signal: i32) -> bool {
    // Safety: getpgrp takes nothing and can't fail.
    if group <= 1 || group == unsafe { libc::getpgrp() } {
        return false;
    }
    // Safety: kill takes no pointers; a group already gone is ESRCH.
    unsafe { libc::kill(-group, signal) == 0 }
}

/// Send `signal` to process `pid` alone. Pids that `kill` would read as a
/// group (0, and those past `i32::MAX`), init's and gerdr's own are refused.
/// Whether it was sent.
pub fn kill_pid(pid: u32, signal: i32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 1 || pid as u32 == std::process::id() {
        return false;
    }
    // Safety: kill takes no pointers; a pid already gone is ESRCH.
    unsafe { libc::kill(pid, signal) == 0 }
}

/// The process group `child` leads, started with `process_group(0)`.
pub fn group_of(child: &Child) -> i32 {
    i32::try_from(child.id()).unwrap_or(0)
}

/// Stop the process group `child` leads: SIGTERM, then SIGKILL once it
/// ended or after `wait`, so helpers it left go too. Reaps `child`.
pub fn stop_group(child: &mut Child, wait: Duration) {
    let group = group_of(child);
    kill_group(group, libc::SIGTERM);
    let start = Instant::now();
    while start.elapsed() < wait {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    kill_group(group, libc::SIGKILL);
    // Only reaping: it is gone either way.
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    #[test]
    fn a_running_process_has_a_start_time() {
        let me = std::process::id();
        assert!(start_time(me).is_some());
        assert_eq!(start_time(me), start_time(me));
        assert_eq!(start_time(u32::MAX), None);
    }

    #[test]
    fn a_process_runs_until_reaped() {
        let me = Process::new(std::process::id());
        assert!(me.runs());
        let moved = Process {
            start: me.start + 1,
            ..me
        };
        assert!(!moved.runs());
    }

    #[test]
    fn stop_group_ends_its_group() {
        // Its own group only: nothing outside it is signalled.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30"])
            .process_group(0)
            .spawn()
            .expect("sh runs");
        let at = Instant::now();
        stop_group(&mut child, Duration::from_secs(5));
        assert!(at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn kill_group_refuses_init_and_its_own() {
        // Signal 0 sends nothing, even when it is let through.
        assert!(!kill_group(0, 0));
        assert!(!kill_group(1, 0));
        assert!(!kill_group(-1, 0));
        // Safety: getpgrp takes nothing.
        assert!(!kill_group(unsafe { libc::getpgrp() }, 0));
    }

    #[test]
    fn kill_pid_refuses_groups_init_and_itself() {
        assert!(!kill_pid(0, 0));
        assert!(!kill_pid(1, 0));
        assert!(!kill_pid(u32::MAX, 0));
        assert!(!kill_pid(std::process::id(), 0));
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("sleep runs");
        assert!(kill_pid(child.id(), libc::SIGKILL));
        let _ = child.wait();
    }
}
