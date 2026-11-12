//! Inotify instances, of which the kernel gives each user a set number
//! (`fs.inotify.max_user_instances`, 128 by default). Every Alacritty that
//! reloads its config file takes one, and each terminal pane is an Alacritty,
//! so a few gerdrs use up most of them. With none left Zed refuses to start
//! and Alacritty shows an error bar, whoever took them.
//!
//! A thread looks now and then (off the event loop, as it reads every
//! process's open files), and gerdr says so before they run out: a toast
//! once, and the problems button while it lasts. Both lead to a dialog that
//! raises the limit in a new terminal tab, where sudo can ask for the
//! password. `[terminal] live_config_reload = false` is the other way out:
//! terminals started from then on take none.

use std::{path::Path, time::Duration};

use smithay::reexports::calloop::channel;

use crate::{Gerdr, confirm::Then, menu::MenuAction, toast::Level};

/// The limit, as the kernel has it now.
const LIMIT_FILE: &str = "/proc/sys/fs/inotify/max_user_instances";
/// How often the thread looks: raising the limit or closing apps shows
/// within this.
const INTERVAL: Duration = Duration::from_secs(30);
/// How full, in percent, counts as running low: leaves room to open a
/// project's terminals and its Zed.
const LOW_PERCENT: usize = 85;
/// How far back down, in percent, it must go to stop counting as low, so
/// that a count going up and down around `LOW_PERCENT` doesn't keep telling.
const FINE_PERCENT: usize = 75;
/// What the problems button keeps these under.
pub const PROBLEM: &str = "inotify";
/// The options that stop an Alacritty watching its config file.
const NO_RELOAD: [&str; 2] = ["-o", "general.live_config_reload=false"];
/// The file the raised limit is kept in, across reboots: named to be read
/// after the usual `60-` ones, as the last one read wins.
const SYSCTL_FILE: &str = "/etc/sysctl.d/90-gerdr-inotify.conf";
/// The least worth raising the limit to.
const RAISED_TO: usize = 1024;

/// How many instances there are, at one look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// The ones found in the user's processes.
    pub used: usize,
    pub limit: usize,
    /// The kernel refused another. `used` can fall short of what it counts
    /// (an open file shared by processes shows in each, one in a process
    /// that can't be read in none), so this is what says it's over.
    pub full: bool,
}

/// How close to the limit it is, in rising order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    #[default]
    Fine,
    Low,
    Full,
}

impl Pressure {
    /// What `usage` makes of it, from `self`.
    fn next(self, usage: &Usage) -> Self {
        let percent = usage.used * 100 / usage.limit.max(1);
        if usage.full {
            Self::Full
        } else if percent >= LOW_PERCENT || (self != Self::Fine && percent >= FINE_PERCENT) {
            Self::Low
        } else {
            Self::Fine
        }
    }
}

/// What gerdr last saw, for what it says.
#[derive(Debug, Default)]
pub struct Watch {
    usage: Option<Usage>,
    pressure: Pressure,
}

impl Watch {
    /// Whether the limit is worth raising now.
    pub fn is_low(&self) -> bool {
        self.pressure != Pressure::Fine
    }
}

impl Usage {
    /// What the toast and the problems button say at `pressure`, if there
    /// is anything to say.
    fn warning(&self, pressure: Pressure) -> Option<String> {
        let Usage { used, limit, .. } = self;
        match pressure {
            Pressure::Fine => None,
            Pressure::Low => Some(format!(
                "Running low on inotify instances: {used} of {limit} in use"
            )),
            Pressure::Full => Some(format!(
                "Out of inotify instances: all {limit} in use, so Zed can't start"
            )),
        }
    }

    /// What to raise the limit to: twice what it is when that is more.
    fn raised(&self) -> usize {
        (self.limit * 2).max(RAISED_TO)
    }
}

/// The shell line setting the limit to `limit` now and at every boot.
fn raise_command(limit: usize) -> String {
    format!(
        "echo fs.inotify.max_user_instances={limit} | sudo tee {SYSCTL_FILE} \
         && sudo sysctl -p {SYSCTL_FILE}"
    )
}

/// The title and the body of the dialog raising the limit.
fn raise_question(usage: &Usage) -> (String, String) {
    let Usage { used, limit, .. } = usage;
    let in_use = if usage.full {
        format!("All {limit}")
    } else {
        format!("{used} of {limit}")
    };
    (
        format!("Raise the inotify limit to {}?", usage.raised()),
        format!(
            "{in_use} inotify instances are in use. With none left, Zed\n\
             won't start and terminals show an error.\n\
             A new tab runs sudo, which asks for your password, to write\n\
             {SYSCTL_FILE}.\n\
             To free some instead, turn off terminal config reload in the\n\
             launcher: open terminals let go of theirs when gerdr restarts."
        ),
    )
}

/// How many inotify instances the processes under `proc` hold, as far as
/// it can tell: another user's aren't readable, and don't count anyway.
fn count_held(proc: &Path) -> usize {
    let Ok(processes) = std::fs::read_dir(proc) else {
        return 0;
    };
    processes
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter_map(|entry| std::fs::read_dir(entry.path().join("fd")).ok())
        .flat_map(|fds| fds.flatten())
        .filter(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|to| to.as_os_str() == "anon_inode:inotify")
        })
        .count()
}

/// Whether the kernel refuses this user another instance.
fn refused() -> bool {
    // SAFETY: no pointers go in, and the descriptor it gives is closed
    // straight away.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd >= 0 {
            libc::close(fd);
            return false;
        }
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EMFILE)
}

/// How many instances there are now. `None` if the kernel doesn't say what
/// the limit is.
fn read_usage() -> Option<Usage> {
    let limit = std::fs::read_to_string(LIMIT_FILE)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Usage {
        used: count_held(Path::new("/proc")),
        limit,
        full: refused(),
    })
}

/// Alacritty's options for a terminal: `colours`, and no watch on its config
/// file unless `reload`.
fn terminal_options(mut colours: Vec<String>, reload: bool) -> Vec<String> {
    if !reload {
        colours.extend(NO_RELOAD.map(String::from));
    }
    colours
}

impl Gerdr {
    /// The options every Alacritty gerdr starts gets: the look's colours,
    /// and what `[terminal]` says of its config file.
    pub(crate) fn terminal_options(&self) -> Vec<String> {
        terminal_options(
            self.theme().palette().alacritty_options(),
            self.settings.live_config_reload,
        )
    }

    /// Look at how many instances there are, now and every `INTERVAL`, on
    /// a thread of its own, which ends with gerdr's end of the channel.
    pub fn watch_inotify(&mut self) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(usage) = event {
                state.inotify_read(usage);
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("inotify limit: {err}");
        }
        let looking = std::thread::Builder::new()
            .name("inotify-limit".into())
            .spawn(move || {
                loop {
                    let Some(usage) = read_usage() else {
                        // Nothing to warn of: without the file there's no
                        // telling how close the limit is.
                        return tracing::warn!("inotify limit: can't read {LIMIT_FILE}");
                    };
                    if tx.send(usage).is_err() {
                        return;
                    }
                    std::thread::sleep(INTERVAL);
                }
            });
        if let Err(err) = looking {
            tracing::warn!("inotify limit: {err}");
        }
    }

    /// Take in a look: a toast when it got worse, and the problems button
    /// says how it is until it is fine.
    fn inotify_read(&mut self, usage: Usage) {
        let was = self.inotify.pressure;
        let now = was.next(&usage);
        self.inotify = Watch {
            usage: Some(usage),
            pressure: now,
        };
        if now == was {
            return;
        }
        let Some(text) = usage.warning(now) else {
            crate::problems::clear(PROBLEM);
            let Usage { used, limit, .. } = usage;
            return self.show_success(format!(
                "Inotify instances are fine again: {used} of {limit} in use"
            ));
        };
        crate::problems::set(PROBLEM, vec![text.clone()]);
        if now > was {
            let level = match now {
                Pressure::Full => Level::Error,
                _ => Level::Warning,
            };
            let fix = ("How to fix".to_owned(), MenuAction::RaiseInotifyLimit);
            self.toast_with(text, level, Some(fix));
        }
    }

    /// Ask before raising the limit, saying what it does.
    pub fn ask_raise_inotify(&mut self) {
        let Some(usage) = self.inotify.usage else {
            return;
        };
        let (title, body) = raise_question(&usage);
        self.ask(title, body, "Raise limit", Then::RaiseInotifyLimit);
    }

    /// Raise the limit in a new tab, typed at its shell so that what runs
    /// shows and sudo can ask. With no workspace to open it in, the line goes
    /// on the clipboard. The next look sees it worked.
    pub fn raise_inotify_limit(&mut self) {
        let Some(usage) = self.inotify.usage else {
            return;
        };
        let command = raise_command(usage.raised());
        if self.workspace().is_none() {
            self.copy_text(command);
            let text = "No workspace to raise the limit in: its command is on the clipboard";
            return self.show_warning(text.to_owned());
        }
        let w = self.active_workspace;
        let dir = self.workspaces[w].dir.clone();
        self.new_tab_running(w, &dir, command);
    }

    /// Turn terminals reloading their config on or off: Settings'
    /// `[terminal] live_config_reload`, written there and read again.
    pub fn toggle_terminal_reload(&mut self) {
        let on = !self.settings.live_config_reload;
        if let Err(err) = crate::config::write_bool("terminal", "live_config_reload", on) {
            let text = format!("Couldn't change terminal config reload in Settings: {err}");
            let open = ("Open settings".to_owned(), MenuAction::OpenSettings);
            return self.toast_with(text, Level::Error, Some(open));
        }
        // Read again whole, so what else changed in the file counts too;
        // its problems were told when they came.
        let _ = self.apply_settings();
        let text = match self.settings.live_config_reload {
            true => "New terminals reload their config \u{b7} in Settings",
            false => {
                "New terminals stop reloading their config \u{b7} open ones when gerdr restarts"
            }
        };
        self.show_success(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: usize, limit: usize, full: bool) -> Usage {
        Usage { used, limit, full }
    }

    #[test]
    fn it_tells_when_it_gets_low_and_keeps_telling_until_well_below() {
        let fine = Pressure::Fine;
        assert_eq!(fine.next(&usage(108, 128, false)), Pressure::Fine);
        assert_eq!(fine.next(&usage(109, 128, false)), Pressure::Low);
        // Going down around the mark doesn't tell again.
        let low = Pressure::Low;
        assert_eq!(low.next(&usage(100, 128, false)), Pressure::Low);
        assert_eq!(low.next(&usage(95, 128, false)), Pressure::Fine);
        assert_eq!(Pressure::Full.next(&usage(100, 128, false)), Pressure::Low);
    }

    #[test]
    fn the_kernel_refusing_is_full_whatever_was_counted() {
        assert_eq!(Pressure::Fine.next(&usage(3, 128, true)), Pressure::Full);
        assert!(Pressure::Full > Pressure::Low && Pressure::Low > Pressure::Fine);
    }

    #[test]
    fn a_limit_of_nothing_does_not_divide_by_it() {
        assert_eq!(Pressure::Fine.next(&usage(0, 0, false)), Pressure::Fine);
    }

    #[test]
    fn it_raises_to_at_least_1024_and_past_a_limit_already_high() {
        assert_eq!(usage(0, 128, false).raised(), 1024);
        assert_eq!(usage(0, 1024, false).raised(), 2048);
    }

    #[test]
    fn the_command_sets_it_now_and_for_every_boot() {
        assert_eq!(
            raise_command(1024),
            "echo fs.inotify.max_user_instances=1024 \
             | sudo tee /etc/sysctl.d/90-gerdr-inotify.conf \
             && sudo sysctl -p /etc/sysctl.d/90-gerdr-inotify.conf"
        );
    }

    #[test]
    fn what_it_says_names_the_numbers() {
        let low = usage(120, 128, false);
        assert_eq!(
            low.warning(Pressure::Low).unwrap(),
            "Running low on inotify instances: 120 of 128 in use"
        );
        assert_eq!(low.warning(Pressure::Fine), None);
        // What was counted can fall short of what the kernel counts.
        let full = usage(120, 128, true);
        assert!(full.warning(Pressure::Full).unwrap().contains("all 128"));
        assert!(raise_question(&full).1.starts_with("All 128 "));
        assert!(raise_question(&low).1.starts_with("120 of 128 "));
        assert_eq!(raise_question(&low).0, "Raise the inotify limit to 1024?");
    }

    #[test]
    fn terminals_lose_the_config_watch_only_when_told() {
        let colours = vec!["-o".to_owned(), "colors.normal.red=\"#e06c75\"".to_owned()];
        assert_eq!(terminal_options(colours.clone(), true), colours);
        let off = terminal_options(colours.clone(), false);
        assert_eq!(off[..2], colours[..]);
        assert_eq!(off[2..], ["-o", "general.live_config_reload=false"]);
    }

    #[test]
    fn it_counts_the_inotify_files_of_processes_only() {
        let root = std::env::temp_dir().join(format!("gerdr-inotify-{}", std::process::id()));
        let add = |process: &str, fd: &str, to: &str| {
            let dir = root.join(process).join("fd");
            std::fs::create_dir_all(&dir).unwrap();
            std::os::unix::fs::symlink(to, dir.join(fd)).unwrap();
        };
        add("100", "3", "anon_inode:inotify");
        add("100", "4", "anon_inode:inotify");
        add("100", "5", "/some/file");
        add("200", "9", "anon_inode:inotify");
        add("self", "3", "anon_inode:inotify");
        assert_eq!(count_held(&root), 3);
        assert_eq!(count_held(&root.join("none")), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
