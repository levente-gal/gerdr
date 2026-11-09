//! What holds a TCP port on this machine: the process listening on it, as
//! `/proc` tells, so a message about a taken port can say what to stop.
//!
//! Only this user's processes can be told: another user's sockets show in
//! `/proc/net/tcp`, but their open files can't be read. Then nothing is named.

use std::{fs, path::Path};

/// A process listening on a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: i32,
    /// Its command line, the program by its file name only.
    pub command: String,
    /// The folder it runs in.
    pub dir: Option<std::path::PathBuf>,
}

/// The longest command line said, in characters.
const COMMAND_CHARS: usize = 48;
/// The longest folder said, in characters.
const DIR_CHARS: usize = 40;

/// The process listening on `port` in this machine's network, if one of this
/// user's is. Reads `/proc` only, a few milliseconds' worth.
pub fn of(port: u16) -> Option<Holder> {
    let mut sockets = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        // Unreadable: no socket of that kind can be told.
        let text = fs::read_to_string(table).unwrap_or_default();
        sockets.extend(listening_sockets(&text, port));
    }
    if sockets.is_empty() {
        return None;
    }
    let names: Vec<String> = sockets.iter().map(|inode| format!("socket:[{inode}]")).collect();
    // A folder that can't be listed has no process to name.
    let mut pids: Vec<i32> = (fs::read_dir("/proc").into_iter().flatten().flatten())
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    pids.sort_unstable();
    let pid = pids.into_iter().find(|pid| {
        // Another user's: its open files can't be read, so it isn't told.
        let fds = fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten();
        fds.filter_map(|fd| fs::read_link(fd.path()).ok())
            .any(|link| link.to_str().is_some_and(|l| names.iter().any(|n| n == l)))
    })?;
    let raw = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let command = command_line(&raw)
        .or_else(|| Some(fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_owned()))
        .filter(|c| !c.is_empty())?;
    let dir = fs::read_link(format!("/proc/{pid}/cwd")).ok();
    Some(Holder { pid, command, dir })
}

/// The inodes of the sockets listening on `port` in `table`, the text of a
/// `/proc/net/tcp` or `tcp6`.
fn listening_sockets(table: &str, port: u16) -> Vec<u64> {
    const LISTEN: &str = "0A";
    (table.lines().skip(1))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let local = fields.get(1)?;
            let at = u16::from_str_radix(local.rsplit_once(':')?.1, 16).ok()?;
            let inode = fields.get(9)?.parse().ok()?;
            (at == port && *fields.get(3)? == LISTEN && inode != 0).then_some(inode)
        })
        .collect()
}

/// A `/proc/<pid>/cmdline` as it reads to a person: the program by its file
/// name, then its arguments, cut at `COMMAND_CHARS`.
fn command_line(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let mut args = text.split('\0').filter(|a| !a.is_empty());
    let program = args.next()?;
    let program = Path::new(program).file_name().map_or(program.into(), |n| n.to_string_lossy());
    let line = std::iter::once(program.as_ref()).chain(args).collect::<Vec<_>>().join(" ");
    Some(match line.char_indices().nth(COMMAND_CHARS) {
        Some((cut, _)) => format!("{}\u{2026}", line[..cut].trim_end()),
        None => line,
    })
}

impl Holder {
    /// How a message names it: `python manage.py runserver (pid 4242,
    /// ~/app)`.
    pub fn named(&self) -> String {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        match &self.dir {
            Some(dir) => format!(
                "{} (pid {}, {})",
                self.command,
                self.pid,
                crate::paths::display(dir, home.as_deref(), DIR_CHARS)
            ),
            None => format!("{} (pid {})", self.command, self.pid),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};

    use super::*;

    #[test]
    fn only_a_listening_socket_on_that_port_is_read() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 00000000:2329 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0\n\
            1: 0100007F:2329 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 5151 1 0\n\
            2: 00000000:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 6161 1 0\n";
        assert_eq!(listening_sockets(table, 9001), [4242]);
        assert_eq!(listening_sockets(table, 8081), [6161]);
        assert!(listening_sockets(table, 80).is_empty());
    }

    #[test]
    fn a_command_line_reads_by_the_program_name_and_is_cut() {
        let raw = b"/home/x/.venv/bin/python\0manage.py\0runserver\x000.0.0.0:9001\0";
        assert_eq!(
            command_line(raw).as_deref(),
            Some("python manage.py runserver 0.0.0.0:9001")
        );
        let long = format!("/bin/node\0{}\0", "a".repeat(80));
        let cut = command_line(long.as_bytes()).unwrap();
        assert_eq!(cut.chars().count(), COMMAND_CHARS + 1);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(command_line(b""), None);
    }

    #[test]
    fn a_port_this_process_listens_on_is_held_by_it() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let holder = of(port).expect("this test listens on it");
        assert_eq!(holder.pid, std::process::id() as i32);
        assert!(holder.dir.is_some());
        assert!(holder.named().contains(&format!("(pid {}", holder.pid)), "{}", holder.named());
        drop(held);
        assert_eq!(of(port), None);
    }
}
