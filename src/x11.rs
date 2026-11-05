//! X11 apps in gerdr's panes. Some apps speak only X11 (FreeCAD's Flatpak,
//! Java tools, Wine), and the host's X server would open their windows outside
//! gerdr. So each gerdr runs its own: `xwayland-satellite` serves an X display
//! and shows its windows as ordinary Wayland windows on gerdr's socket.
//!
//! All those windows come from the bridge's one client, so the pid of a window
//! no longer says whose it is (see `Gerdr::place`).

use std::{
    ffi::{OsStr, OsString},
    os::unix::{net::UnixStream, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use crate::{herdr, launch};

const BRIDGE: &str = "xwayland-satellite";

/// The first display tried. Far above what a desktop's own X servers take.
const FIRST_DISPLAY: u32 = 100;
const DISPLAYS_TRIED: u32 = 64;

/// A bridge that lived this long is started again when it stops. One that
/// stopped sooner would only stop again: the next app started tries it again.
const STEADY_AFTER: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Bridge {
    child: Option<Child>,
    display: Option<OsString>,
    started: Option<Instant>,
    /// Told the log that the bridge can't start: gerdr tries with every app.
    warned: bool,
}

impl Bridge {
    /// The X display that apps get, while a bridge runs.
    pub fn display(&self) -> Option<&OsStr> {
        self.display.as_deref()
    }

    /// Whether `pid` is the bridge's: a window of some X11 app.
    pub fn is_client(&self, pid: Option<u32>) -> bool {
        pid.is_some() && self.child.as_ref().map(Child::id) == pid
    }

    /// Start a bridge for Wayland display `wayland`, if none runs. Its display
    /// is known at once; X11 apps that connect meanwhile wait for the server.
    pub fn ensure(&mut self, wayland: &OsStr) {
        if self.child.is_some() {
            return;
        }
        // Next to gerdr's own number, so that shells outliving a gerdr keep a
        // display that matches their `WAYLAND_DISPLAY`.
        let Some(number) = free_display(Path::new("/tmp"), wayland) else {
            tracing::warn!("no free X display from :{FIRST_DISPLAY}");
            return;
        };
        let name = format!(":{number}");
        let mut cmd = Command::new(BRIDGE);
        cmd.arg(&name);
        herdr::scrub_env(&mut cmd, None);
        cmd.env("WAYLAND_DISPLAY", wayland)
            .env_remove("DISPLAY")
            .env_remove("NOTIFY_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0);
        if let Some(log) = launch::log_file(BRIDGE) {
            cmd.stderr(Stdio::from(log));
        }
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!("started {BRIDGE} on {name}");
                self.child = Some(child);
                self.display = Some(name.into());
                self.started = Some(Instant::now());
            }
            Err(err) if !self.warned => {
                tracing::warn!("X11 apps won't start: {BRIDGE}: {err}");
                self.warned = true;
            }
            Err(_) => {}
        }
    }

    /// See whether the bridge stopped, and start it again if it had been
    /// running well. Whether it did: X11 apps went with it. One that stops as
    /// soon as it starts goes on the "!" badge instead, since every app
    /// started tries it again.
    pub fn reap(&mut self, wayland: &OsStr) -> bool {
        let Some(child) = &mut self.child else {
            return false;
        };
        let steady = self.started.is_some_and(|t| t.elapsed() >= STEADY_AFTER);
        let status = match child.try_wait() {
            Ok(None) => {
                if steady {
                    crate::problems::clear(PROBLEM);
                }
                return false;
            }
            Ok(Some(status)) => status.to_string(),
            Err(err) => err.to_string(),
        };
        tracing::warn!("{BRIDGE} stopped: {status}");
        self.child = None;
        self.display = None;
        if steady {
            self.ensure(wayland);
        } else {
            crate::problems::report(
                PROBLEM,
                format!("{BRIDGE} stops as it starts, so X11 apps can't run"),
            );
        }
        steady
    }

    /// End the bridge, and with it the X server and its apps.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Safety: signals the process group gerdr started for the bridge.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
            let _ = child.wait();
        }
        self.display = None;
    }
}

/// The name problems of the bridge go by (see `problems`).
pub const PROBLEM: &str = "x11";

/// What to say of an app that quit before it showed a window, with no bridge
/// running: if it needs X11, this is why.
pub fn no_bridge_text(app: &str) -> String {
    format!("{app} closed without a window; X11 apps need {BRIDGE}")
}

/// The first X display not in use, starting from gerdr's own number: `gerdr-0`
/// looks at :100 first, `wayland-3` at :103. `tmp` holds the X lock files.
fn free_display(tmp: &Path, wayland: &OsStr) -> Option<u32> {
    let wayland = wayland.to_string_lossy();
    let digits = wayland.len() - wayland.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let own = wayland[wayland.len() - digits..]
        .parse::<u32>()
        .unwrap_or(0);
    (0..DISPLAYS_TRIED)
        .map(|i| FIRST_DISPLAY + (own + i) % DISPLAYS_TRIED)
        .find(|n| !display_taken(tmp, *n))
}

/// Whether a running X server holds display `number`. It answers on its socket;
/// not every server writes a lock file, and `xwayland-satellite` doesn't. A lock
/// file or socket left by one that crashed doesn't count: the next server
/// replaces it.
fn display_taken(tmp: &Path, number: u32) -> bool {
    let socket = tmp.join(format!(".X11-unix/X{number}"));
    if UnixStream::connect(socket).is_ok() {
        return true;
    }
    let Ok(lock) = std::fs::read_to_string(tmp.join(format!(".X{number}-lock"))) else {
        return false;
    };
    let Ok(pid) = lock.trim().parse::<i32>() else {
        // Not one the X server's own check would read: leave it alone.
        return true;
    };
    // Safety: signal 0 sends nothing, it only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-x11-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_display_follows_the_number_of_gerdrs_socket() {
        let tmp = scratch("number");
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(100));
        assert_eq!(free_display(&tmp, OsStr::new("wayland-3")), Some(103));
        assert_eq!(free_display(&tmp, OsStr::new("gerdr")), Some(100));
    }

    #[test]
    fn a_display_a_running_server_holds_is_skipped() {
        let tmp = scratch("held");
        let me = format!("{:>10}\n", std::process::id());
        std::fs::write(tmp.join(".X100-lock"), me).unwrap();
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(101));
    }

    #[test]
    fn a_display_a_server_answers_on_is_skipped() {
        let tmp = scratch("socket");
        std::fs::create_dir_all(tmp.join(".X11-unix")).unwrap();
        let _server = std::os::unix::net::UnixListener::bind(tmp.join(".X11-unix/X100")).unwrap();
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(101));
    }

    #[test]
    fn a_socket_left_by_a_crashed_server_frees_its_display() {
        let tmp = scratch("dead");
        std::fs::create_dir_all(tmp.join(".X11-unix")).unwrap();
        drop(std::os::unix::net::UnixListener::bind(tmp.join(".X11-unix/X100")).unwrap());
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(100));
    }

    #[test]
    fn a_lock_left_by_a_crashed_server_frees_its_display() {
        let tmp = scratch("stale");
        // Above the kernel's highest pid.
        std::fs::write(tmp.join(".X100-lock"), "2147483647\n").unwrap();
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(100));
    }

    #[test]
    fn a_lock_it_cannot_read_keeps_its_display() {
        let tmp = scratch("odd");
        std::fs::write(tmp.join(".X100-lock"), "?\n").unwrap();
        assert_eq!(free_display(&tmp, OsStr::new("gerdr-0")), Some(101));
    }
}
