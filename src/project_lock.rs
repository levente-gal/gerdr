//! Which projects a gerdr has open, so another gerdr leaves them be unless
//! started with `--attach`.
//!
//! Each gerdr holds a shared `flock` on `<dir>/<project id>.lock` for the
//! project it shows. The kernel lets go of it when gerdr ends, even by a
//! crash, so no lock outlives its gerdr. A project is open elsewhere when an
//! exclusive lock can't be had; gerdr's own shared lock counts too, so
//! callers leave out the project they show themselves.
//!
//! That test holds the exclusive lock for a moment, and every gerdr runs it
//! often (see `discover_services`). So a shared lock is retried while one
//! is held, and a test that finds the lock held tries again before it says
//! so: two tests at once mustn't look like a gerdr.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a lock is tried for while another gerdr's test holds it.
const TAKE_FOR: Duration = Duration::from_secs(1);

/// How many times a test tries the lock before it says it is held.
const TESTS: u32 = 3;

/// The folder of the locks: `$XDG_RUNTIME_DIR/gerdr/projects`; none
/// without a runtime folder, and then no project counts as open elsewhere.
pub fn dir() -> Option<PathBuf> {
    Some(crate::herdr::runtime_dir()?.join("gerdr/projects"))
}

/// The lock of the project shown, held until dropped.
#[derive(Debug)]
pub struct ProjectLock {
    id: String,
    _file: File,
}

impl ProjectLock {
    /// Hold project `id`'s lock in `dir`, shared: `--attach` gerdrs hold it
    /// alongside.
    pub fn take(dir: &Path, id: &str) -> io::Result<Self> {
        let file = open(dir, id)?;
        let until = Instant::now() + TAKE_FOR;
        // Only a test holds it exclusively, and only for a moment.
        while let Err(err) = flock(&file, libc::LOCK_SH | libc::LOCK_NB) {
            if err.kind() != io::ErrorKind::WouldBlock || Instant::now() > until {
                return Err(err);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(Self {
            id: id.to_owned(),
            _file: file,
        })
    }

    /// The project it is the lock of.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Whether a gerdr holds project `id`'s lock in `dir`, this one included.
pub fn is_open_elsewhere(dir: &Path, id: &str) -> bool {
    let Ok(file) = open(dir, id) else {
        return false;
    };
    // Closing the file lets go of the lock just taken. Another gerdr's
    // test holds it a moment only; a gerdr's shared lock stays.
    for test in 0..TESTS {
        if flock(&file, libc::LOCK_EX | libc::LOCK_NB).is_ok() {
            return false;
        }
        if test + 1 < TESTS {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    true
}

fn open(dir: &Path, id: &str) -> io::Result<File> {
    std::fs::create_dir_all(dir)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("{id}.lock")))
}

fn flock(file: &File, op: libc::c_int) -> io::Result<()> {
    // Safety: the fd is open for as long as `file` lives.
    match unsafe { libc::flock(file.as_raw_fd(), op) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gerdr-project-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Whether the lock is let go of within a second: a child another
    /// test starts meanwhile holds a copy of the lock's file until it runs
    /// its program, keeping the lock a moment longer.
    fn let_go(dir: &Path, id: &str) -> bool {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while is_open_elsewhere(dir, id) {
            if std::time::Instant::now() > until {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn a_held_lock_makes_the_project_open_elsewhere_until_dropped() {
        let dir = temp_dir("held");
        assert!(let_go(&dir, "aft"));
        let lock = ProjectLock::take(&dir, "aft").unwrap();
        assert!(is_open_elsewhere(&dir, "aft"));
        assert!(let_go(&dir, "other"));
        drop(lock);
        assert!(let_go(&dir, "aft"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attached_gerdrs_hold_the_lock_together() {
        let dir = temp_dir("shared");
        let first = ProjectLock::take(&dir, "aft").unwrap();
        let second = ProjectLock::take(&dir, "aft").unwrap();
        drop(first);
        assert!(is_open_elsewhere(&dir, "aft"));
        drop(second);
        assert!(let_go(&dir, "aft"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_is_taken_while_another_gerdr_tests_it() {
        let dir = temp_dir("tested");
        let test = open(&dir, "aft").unwrap();
        flock(&test, libc::LOCK_EX | libc::LOCK_NB).unwrap();
        let ends = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(test);
        });
        let lock = ProjectLock::take(&dir, "aft").unwrap();
        ends.join().unwrap();
        assert!(is_open_elsewhere(&dir, "aft"));
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
