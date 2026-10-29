//! Bringing an app's own profile, the one it keeps outside gerdr (its
//! logins, settings, extensions), into gerdr's shared profile of it, which
//! the projects' profiles share or start from (see `launch::instance_dir`).
//! The launcher offers a row per profile found; the one it replaces is kept
//! beside it.
//!
//! A browser's or VS Code's logins only open with the keyring key of the
//! same browser or editor, so a profile only goes to the same app.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use smithay::reexports::calloop::channel;

use crate::{Gerdr, desktop, home_dir, launch, paths::tilde, toast::Level};

/// The kinds of apps gerdr keeps a profile of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Chromium,
    Firefox,
    VsCode,
    Zed,
}

/// An app's own profile, found where it keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The app's name, as the user knows it.
    pub name: &'static str,
    pub kind: Kind,
    /// gerdr's key for the app: the program its profiles are named by.
    pub key: String,
    /// The profile's folder (Zed's data folder; its settings are in
    /// `~/.config/zed`).
    pub from: PathBuf,
    /// gerdr's shared profile it replaces.
    pub to: PathBuf,
}

/// An app whose own profile can come in.
struct Known {
    name: &'static str,
    kind: Kind,
    /// Its programs, gerdr's keys for it: the first installed is picked
    /// when gerdr has no profile of any yet.
    programs: &'static [&'static str],
    /// Where it keeps its profile, under the home folder; the first found
    /// counts. A Firefox keeps its profiles there.
    folders: &'static [&'static str],
}

const KNOWN: &[Known] = &[
    Known {
        name: "Chromium",
        kind: Kind::Chromium,
        programs: &["chromium-browser", "chromium"],
        folders: &[".config/chromium"],
    },
    Known {
        name: "Chrome",
        kind: Kind::Chromium,
        programs: &["google-chrome-stable", "google-chrome"],
        folders: &[".config/google-chrome"],
    },
    Known {
        name: "Brave",
        kind: Kind::Chromium,
        programs: &["brave-browser-stable", "brave-browser", "brave"],
        folders: &[".config/BraveSoftware/Brave-Browser"],
    },
    Known {
        name: "Vivaldi",
        kind: Kind::Chromium,
        programs: &["vivaldi-stable", "vivaldi"],
        folders: &[".config/vivaldi"],
    },
    Known {
        name: "Edge",
        kind: Kind::Chromium,
        programs: &["microsoft-edge-stable", "microsoft-edge"],
        folders: &[".config/microsoft-edge"],
    },
    Known {
        name: "Firefox",
        kind: Kind::Firefox,
        programs: &["firefox", "firefox-esr"],
        folders: &[".mozilla/firefox"],
    },
    Known {
        name: "LibreWolf",
        kind: Kind::Firefox,
        programs: &["librewolf"],
        folders: &[".librewolf", ".config/librewolf/librewolf"],
    },
    Known {
        name: "Cursor",
        kind: Kind::VsCode,
        programs: &["cursor"],
        folders: &[".config/Cursor"],
    },
    Known {
        name: "VS Code",
        kind: Kind::VsCode,
        programs: &["code"],
        folders: &[".config/Code"],
    },
    Known {
        name: "VS Code Insiders",
        kind: Kind::VsCode,
        programs: &["code-insiders"],
        folders: &[".config/Code - Insiders"],
    },
    Known {
        name: "VSCodium",
        kind: Kind::VsCode,
        programs: &["codium", "vscodium"],
        folders: &[".config/VSCodium"],
    },
    Known {
        name: "Windsurf",
        kind: Kind::VsCode,
        programs: &["windsurf"],
        folders: &[".config/Windsurf"],
    },
    Known {
        name: "Zed",
        kind: Kind::Zed,
        programs: &["zed", "zeditor", "zed-editor"],
        folders: &[".local/share/zed"],
    },
];

/// What of Zed's data folder comes in: what the user made, not what Zed
/// downloads again (its language servers, Node), which gerdr's keeps.
const ZED_IMPORTED: &[&str] = &["db", "extensions", "prompts", "threads"];

/// What of gerdr's Zed data folder stays behind, besides its sockets.
const ZED_LEFT: &[&str] = &["logs", "hang_traces"];

/// Whether an import runs: one at a time.
static IMPORTING: AtomicBool = AtomicBool::new(false);

/// The profiles of the installed apps found in `home`.
pub fn sources(home: &Path) -> Vec<Source> {
    sources_with(home, desktop::installed)
}

/// See `sources`; `installed` tells the installed programs.
fn sources_with(home: &Path, installed: impl Fn(&str) -> bool) -> Vec<Source> {
    let mut sources = Vec::new();
    for known in KNOWN {
        let Some(folder) = known
            .folders
            .iter()
            .map(|folder| home.join(folder))
            .find(|folder| folder.is_dir())
        else {
            continue;
        };
        let installed: Vec<&str> = known
            .programs
            .iter()
            .copied()
            .filter(|program| installed(program))
            .collect();
        let Some(&first) = installed.first() else {
            continue;
        };
        let (key, from, to) = match known.kind {
            Kind::Zed => (
                "zed".to_owned(),
                folder,
                home.join(".local/share/gerdr/zed"),
            ),
            kind => {
                let key = installed
                    .iter()
                    .find(|program| launch::profile_dir(home, program).is_dir())
                    .unwrap_or(&first);
                let from = if kind == Kind::Firefox {
                    let Some(profile) = firefox_profile(&folder) else {
                        continue;
                    };
                    profile
                } else {
                    folder
                };
                ((*key).to_owned(), from, launch::profile_dir(home, key))
            }
        };
        sources.push(Source {
            name: known.name,
            kind: known.kind,
            key,
            from,
            to,
        });
    }
    sources
}

/// The default profile of the Firefox keeping its profiles in `folder`, by
/// its `profiles.ini`: the installation's, else the one marked default,
/// else the only one.
fn firefox_profile(folder: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(folder.join("profiles.ini")).ok()?;
    // Per section: its name, `Path`, `IsRelative`, `Default`.
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.to_owned(), Vec::new()));
        } else if let (Some((key, value)), Some((_, fields))) =
            (line.split_once('='), sections.last_mut())
        {
            fields.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let field = |fields: &[(String, String)], key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let profiles: Vec<&[(String, String)]> = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .map(|(_, fields)| fields.as_slice())
        .collect();
    // A path is the folder's unless its profile says `IsRelative=0`.
    let resolve = |path: String| {
        let absolute = profiles.iter().any(|fields| {
            field(fields, "Path").as_ref() == Some(&path)
                && field(fields, "IsRelative").as_deref() == Some("0")
        });
        if absolute {
            PathBuf::from(path)
        } else {
            folder.join(path)
        }
    };
    let installed = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .find_map(|(_, fields)| field(fields, "Default"));
    let marked = profiles
        .iter()
        .find(|fields| field(fields, "Default").as_deref() == Some("1"))
        .or(match profiles.as_slice() {
            [only] => Some(only),
            _ => None,
        })
        .and_then(|fields| field(fields, "Path"));
    [installed, marked]
        .into_iter()
        .flatten()
        .map(resolve)
        .find(|profile| profile.is_dir())
}

/// The pid a lock names: a link to `<host>-<pid>` (Chromium), to
/// `<ip>:+<pid>` (Firefox), or a file holding it (VS Code's `code.lock`).
fn lock_pid(lock: &Path) -> Option<u32> {
    let text = match std::fs::read_link(lock) {
        Ok(target) => target.to_string_lossy().into_owned(),
        Err(_) => std::fs::read_to_string(lock).ok()?,
    };
    let text = text.trim();
    let digits = text.len() - text.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    text[text.len() - digits..].parse().ok()
}

/// Whether an app runs on profile folder `dir`: a lock in it names a
/// running process, or a socket in it answers (Zed's).
fn running(dir: &Path) -> bool {
    let locked = ["SingletonLock", "lock", "code.lock"]
        .iter()
        .filter_map(|name| lock_pid(&dir.join(name)))
        .any(|pid| Path::new(&format!("/proc/{pid}")).exists());
    locked
        || std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().to_string_lossy().ends_with(".sock")
                    && std::os::unix::net::UnixStream::connect(entry.path()).is_ok()
            })
        })
}

/// The project whose profile of `key` an app runs on, if any (see
/// `launch::instance_dir`).
fn running_instance(home: &Path, key: &str) -> Option<String> {
    let instances = home.join(".local/share/gerdr/instances");
    std::fs::read_dir(instances)
        .ok()?
        .flatten()
        .find(|entry| running(&entry.path().join(key)))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
}

/// Bring `source` into gerdr's shared profile of its app, with the one it
/// replaces moved aside: where it went, if there was one. Refused while the
/// app runs on it, or, for an editor (whose projects' profiles link to the
/// shared one), on a project's.
pub fn import(home: &Path, source: &Source) -> Result<Option<PathBuf>, String> {
    let name = source.name;
    if running(&source.from) {
        return Err(format!("{name} is running: quit it first"));
    }
    if matches!(source.kind, Kind::VsCode | Kind::Zed)
        && let Some(project) = running_instance(home, &source.key)
    {
        return Err(format!("gerdr's {name} runs in {project}: close it first"));
    }
    let new = suffixed(&source.to, ".new");
    let _ = std::fs::remove_dir_all(&new);
    let done = fill(home, source, &new).and_then(|()| swap(source, &new));
    if done.is_err() {
        let _ = std::fs::remove_dir_all(&new);
    }
    done.map_err(|err| format!("Couldn't import {name}'s profile: {err}"))
}

/// Copy what of `source` gerdr keeps into the new profile `new`.
fn fill(home: &Path, source: &Source, new: &Path) -> std::io::Result<()> {
    let from = &source.from;
    let all = |_: &std::ffi::OsStr| false;
    match source.kind {
        Kind::Chromium | Kind::Firefox => {
            let skipped =
                |name: &std::ffi::OsStr| launch::BROWSER_UNCOPIED.iter().any(|s| name == *s);
            launch::copy_tree(from, new, &skipped)
        }
        // What the projects' profiles link to (see
        // `launch::ready_vs_code_profile`); its extensions are in a folder
        // of their own, which gerdr's use too.
        Kind::VsCode => {
            std::fs::create_dir_all(new)?;
            if from.join("User").is_dir() {
                launch::copy_tree(&from.join("User"), &new.join("User"), &all)?;
            }
            if from.join("machineid").is_file() {
                std::fs::copy(from.join("machineid"), new.join("machineid"))?;
            }
            Ok(())
        }
        Kind::Zed => {
            std::fs::create_dir_all(new)?;
            for name in ZED_IMPORTED {
                if from.join(name).is_dir() {
                    launch::copy_tree(&from.join(name), &new.join(name), &all)?;
                }
            }
            let config = home.join(".config/zed");
            if config.is_dir() {
                launch::copy_tree(&config, &new.join("config"), &all)?;
            }
            Ok(())
        }
    }
}

/// Put `new` in the place of `source`'s shared profile, which moves aside:
/// where to. What of Zed's it downloaded comes along, not to fetch it again.
fn swap(source: &Source, new: &Path) -> std::io::Result<Option<PathBuf>> {
    let to = &source.to;
    if !to.exists() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(new, to)?;
        return Ok(None);
    }
    if source.kind == Kind::Zed {
        for entry in std::fs::read_dir(to)?.flatten() {
            let name = entry.file_name();
            let left = ZED_LEFT.iter().any(|left| name == *left)
                || name.to_string_lossy().ends_with(".sock");
            if !left && !new.join(&name).exists() {
                std::fs::rename(entry.path(), new.join(&name))?;
            }
        }
    }
    let aside = suffixed(to, &format!(".before-import-{}", stamp()));
    std::fs::rename(to, &aside)?;
    if let Err(err) = std::fs::rename(new, to) {
        let _ = std::fs::rename(&aside, to);
        return Err(err);
    }
    Ok(Some(aside))
}

/// `path` with `suffix` added to its name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The local time now, `20261029-1532`, to tell profiles moved aside by.
fn stamp() -> String {
    // SAFETY: `localtime_r` writes only to the `tm` it is given, and an
    // all-zero `tm` is a valid one.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return crate::projects::now().to_string();
        }
        tm
    };
    format!(
        "{}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// What an import hands the event loop: its source, and where the profile
/// it replaced went, or why it failed.
pub type Imported = (Source, Result<Option<PathBuf>, String>);

impl Gerdr {
    /// Take in what imports of profiles (see `import_profile`) end with.
    pub fn watch_profile_imports(&mut self) {
        let (tx, rx) = channel::channel::<Imported>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg((source, done)) = event {
                state.profile_imported(&source, done);
            }
        });
        match inserted {
            Ok(_) => self.profile_imports = Some(tx),
            Err(err) => tracing::warn!("profile imports: {err}"),
        }
    }

    /// Bring `source` into gerdr's shared profile of its app, off the event
    /// loop (a profile can be large).
    pub fn import_profile(&mut self, source: Source) {
        let (Some(tx), Some(home)) = (self.profile_imports.clone(), home_dir()) else {
            return;
        };
        if IMPORTING.swap(true, Ordering::AcqRel) {
            return self.toast("A profile is being imported already".into(), Level::Warning);
        }
        self.toast(format!("Importing {}'s profile…", source.name), Level::Info);
        let spawned = std::thread::Builder::new()
            .name("import profile".into())
            .spawn(move || {
                let done = import(&home, &source);
                IMPORTING.store(false, Ordering::Release);
                let _ = tx.send((source, done));
            });
        if let Err(err) = spawned {
            IMPORTING.store(false, Ordering::Release);
            self.toast(format!("Couldn't import: {err}"), Level::Error);
        }
    }

    /// Tell how the import of `source` went.
    fn profile_imported(&mut self, source: &Source, done: Result<Option<PathBuf>, String>) {
        let name = source.name;
        let aside = match done {
            Ok(aside) => aside,
            Err(why) => return self.toast(why, Level::Error),
        };
        let mut text = match source.kind {
            Kind::Chromium | Kind::Firefox => format!(
                "Imported {name}'s profile: projects without a {name} profile of their own start from it"
            ),
            Kind::VsCode | Kind::Zed => format!("Imported {name}'s profile into gerdr's {name}"),
        };
        if let Some(aside) = aside {
            let aside = tilde(&aside, home_dir().as_deref());
            text.push_str(&format!(". The old one is in {aside}"));
        }
        self.toast(text, Level::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder for a test.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gerdr-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn sources_are_installed_apps_with_a_profile() {
        let home = scratch("sources");
        std::fs::create_dir_all(home.join(".config/chromium")).unwrap();
        std::fs::create_dir_all(home.join(".config/Cursor")).unwrap();
        std::fs::create_dir_all(home.join(".local/share/gerdr/profiles/chromium")).unwrap();
        let found = sources_with(&home, |program| {
            ["chromium", "chromium-browser", "brave"].contains(&program)
        });
        // Cursor isn't installed, Brave keeps no profile.
        assert_eq!(found.len(), 1);
        let chromium = &found[0];
        assert_eq!((chromium.name, chromium.kind), ("Chromium", Kind::Chromium));
        // The key gerdr has a profile of wins over the first installed.
        assert_eq!(chromium.key, "chromium");
        assert_eq!(chromium.from, home.join(".config/chromium"));
        assert_eq!(
            chromium.to,
            home.join(".local/share/gerdr/profiles/chromium")
        );
        let first = sources_with(&home, |program| program == "chromium-browser");
        assert_eq!(first[0].key, "chromium-browser");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn firefox_profile_is_the_installs_default() {
        let root = scratch("firefox");
        std::fs::create_dir_all(root.join("abc.default")).unwrap();
        std::fs::create_dir_all(root.join("xyz.default-release")).unwrap();
        write(
            &root.join("profiles.ini"),
            "[Profile1]\nName=default\nIsRelative=1\nPath=abc.default\nDefault=1\n\n\
             [Profile0]\nName=default-release\nIsRelative=1\nPath=xyz.default-release\n\n\
             [Install4F96D1932A9F858E]\nDefault=xyz.default-release\nLocked=1\n",
        );
        assert_eq!(
            firefox_profile(&root),
            Some(root.join("xyz.default-release"))
        );
        write(
            &root.join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=abc.default\nDefault=1\n[Profile1]\nPath=xyz.default-release\n",
        );
        assert_eq!(firefox_profile(&root), Some(root.join("abc.default")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_live_lock_means_running() {
        let dir = scratch("lock");
        assert!(!running(&dir));
        let me = std::process::id();
        std::os::unix::fs::symlink(format!("host-{me}"), dir.join("SingletonLock")).unwrap();
        assert_eq!(lock_pid(&dir.join("SingletonLock")), Some(me));
        assert!(running(&dir));
        std::fs::remove_file(dir.join("SingletonLock")).unwrap();
        // A pid past any the kernel hands out.
        std::os::unix::fs::symlink("127.0.0.1:+4194305", dir.join("lock")).unwrap();
        assert!(!running(&dir));
        write(&dir.join("code.lock"), &me.to_string());
        assert!(running(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn source(home: &Path, kind: Kind, from: &str, to: &str) -> Source {
        Source {
            name: "App",
            kind,
            key: "app".into(),
            from: home.join(from),
            to: home.join(to),
        }
    }

    #[test]
    fn a_browser_comes_in_without_caches_and_the_old_one_stays() {
        let home = scratch("browser");
        write(&home.join("own/Default/Cookies"), "new cookies");
        write(&home.join("own/Local State"), "{}");
        write(&home.join("own/Default/Cache/data"), "cache");
        write(&home.join("gerdr/Default/Cookies"), "old cookies");
        let source = source(&home, Kind::Chromium, "own", "gerdr");
        let aside = import(&home, &source).unwrap().unwrap();
        let read = |path: &str| std::fs::read_to_string(home.join(path)).unwrap();
        assert_eq!(read("gerdr/Default/Cookies"), "new cookies");
        assert!(home.join("gerdr/Local State").exists());
        assert!(!home.join("gerdr/Default/Cache").exists());
        assert_eq!(
            std::fs::read_to_string(aside.join("Default/Cookies")).unwrap(),
            "old cookies"
        );
        assert!(!home.join("gerdr.new").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_running_source_is_refused_and_nothing_changes() {
        let home = scratch("refused");
        write(&home.join("own/Default/Cookies"), "new");
        write(&home.join("gerdr/Default/Cookies"), "old");
        let me = std::process::id();
        std::os::unix::fs::symlink(format!("host-{me}"), home.join("own/SingletonLock")).unwrap();
        let source = source(&home, Kind::Chromium, "own", "gerdr");
        assert!(import(&home, &source).unwrap_err().contains("running"));
        assert_eq!(
            std::fs::read_to_string(home.join("gerdr/Default/Cookies")).unwrap(),
            "old"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_vs_code_brings_its_user_folder_and_machine_id() {
        let home = scratch("vs-code");
        write(&home.join("own/User/settings.json"), "{}");
        write(&home.join("own/machineid"), "id");
        write(&home.join("own/Cache/data"), "cache");
        let source = source(&home, Kind::VsCode, "own", "gerdr");
        assert_eq!(import(&home, &source), Ok(None));
        let mut names: Vec<String> = std::fs::read_dir(home.join("gerdr"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["User", "machineid"]);
        assert!(home.join("gerdr/User/settings.json").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_vs_code_running_in_a_project_is_refused() {
        let home = scratch("vs-code-busy");
        write(&home.join("own/User/settings.json"), "{}");
        let me = std::process::id();
        write(
            &home.join(".local/share/gerdr/instances/aft/app/code.lock"),
            &me.to_string(),
        );
        let source = source(&home, Kind::VsCode, "own", "gerdr");
        assert_eq!(
            import(&home, &source),
            Err("gerdr's App runs in aft: close it first".into())
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn zed_brings_its_settings_and_keeps_what_it_downloaded() {
        let home = scratch("zed");
        write(&home.join("own/extensions/installed/x"), "ext");
        write(&home.join("own/db/0-stable/db.sqlite"), "db");
        write(&home.join("own/node/bin"), "node");
        write(&home.join("own/logs/Zed.log"), "log");
        write(&home.join(".config/zed/settings.json"), "{}");
        write(&home.join("gerdr/node/bin"), "gerdr's node");
        write(&home.join("gerdr/config/settings.json"), "old");
        write(&home.join("gerdr/logs/Zed.log"), "old log");
        let source = source(&home, Kind::Zed, "own", "gerdr");
        let aside = import(&home, &source).unwrap().unwrap();
        let read = |path: &str| std::fs::read_to_string(home.join(path)).unwrap();
        assert_eq!(read("gerdr/config/settings.json"), "{}");
        assert_eq!(read("gerdr/extensions/installed/x"), "ext");
        assert_eq!(read("gerdr/db/0-stable/db.sqlite"), "db");
        // gerdr's downloads stay; its logs go with the old profile.
        assert_eq!(read("gerdr/node/bin"), "gerdr's node");
        assert!(!home.join("gerdr/logs").exists());
        assert!(aside.join("logs/Zed.log").exists());
        assert!(aside.join("config/settings.json").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
