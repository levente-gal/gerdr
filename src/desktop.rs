//! The apps installed on the system, as the desktop's app menu lists them:
//! the desktop entries (`.desktop` files) under `applications` in the XDG
//! data folders. Entries hidden from the menu, for this desktop, or meant
//! to run in a terminal are left out, and so are those whose program isn't
//! there. An entry in an earlier folder stands in for one with the same id
//! in a later folder. Their icons come from the icon theme (see `icons`).

use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use crate::{home_dir, icons::IconIndex, launch::App};

/// How deep the `applications` folders are searched.
const MAX_DEPTH: usize = 4;

/// The installed apps, by name.
pub fn installed_apps() -> Vec<App> {
    let desktops = current_desktops();
    let locales = locales();
    let data_dirs = data_dirs();
    let icons = IconIndex::new(&data_dirs);
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for root in data_dirs.iter().map(|dir| dir.join("applications")) {
        let mut files = Vec::new();
        desktop_files(&root, 0, &mut files);
        files.sort();
        for path in files {
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            // The id: the path below the folder, with "-" for "/".
            if !seen.insert(rel.to_string_lossy().replace('/', "-")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            apps.extend(app(&text, &path, &desktops, &locales, &icons));
        }
    }
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps
}

/// The `.desktop` files in `dir` and the folders below it.
fn desktop_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                desktop_files(&path, depth + 1, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            files.push(path);
        }
    }
}

/// The app desktop entry `text` (from `path`) describes, if it should be
/// listed, with its icon from `icons`.
fn app(
    text: &str,
    path: &Path,
    desktops: &[String],
    locales: &[String],
    icons: &IconIndex,
) -> Option<App> {
    let entry = desktop_entry(text);
    let get = |key: &str| entry.get(key).map(String::as_str);
    let localized = |key: &str| {
        let value = locales
            .iter()
            .find_map(|locale| entry.get(&format!("{key}[{locale}]")))
            .or(entry.get(key))?;
        Some(unescape(value)).filter(|value| !value.trim().is_empty())
    };
    let set = |key: &str| get(key) == Some("true");
    if get("Type") != Some("Application") || set("NoDisplay") || set("Hidden") || set("Terminal") {
        return None;
    }
    let listed = |key: &str| {
        get(key).map(|list| {
            list.split(';')
                .any(|d| desktops.contains(&d.to_lowercase()))
        })
    };
    if listed("OnlyShowIn") == Some(false) || listed("NotShowIn") == Some(true) {
        return None;
    }
    if get("TryExec").is_some_and(|program| !installed(&unescape(program))) {
        return None;
    }
    let name = localized("Name")?;
    let args = exec_args(&unescape(get("Exec")?), &name, path)?;
    if !installed(args.first()?) {
        return None;
    }
    let detail = localized("GenericName")
        .or_else(|| localized("Comment"))
        .unwrap_or_default();
    let command = args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let icon = get("Icon").and_then(|icon| icons.find(&unescape(icon)));
    Some(App {
        name,
        command,
        detail,
        icon,
    })
}

/// The keys of the `[Desktop Entry]` group, each with its first value.
fn desktop_entry(text: &str) -> HashMap<String, String> {
    let mut entry = HashMap::new();
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
        } else if in_group
            && !line.starts_with('#')
            && let Some((key, value)) = line.split_once('=')
        {
            entry
                .entry(key.trim().to_owned())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    entry
}

/// A string value with its escapes (`\s`, `\n`, `\t`, `\r`, `\\`) undone.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

/// The arguments of an `Exec` value (already unescaped): split at spaces
/// outside double quotes, with `%c` (the name), `%k` (the file) and `%%`
/// filled in, and the other field codes (files, URLs, the icon) dropped, as
/// nothing is opened with the app. `None` when the quotes don't close.
fn exec_args(exec: &str, name: &str, path: &Path) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut arg: Option<String> = None;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => args.extend(arg.take()),
            '"' => {
                let arg = arg.get_or_insert_default();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => arg.push(chars.next()?),
                        c => arg.push(c),
                    }
                }
            }
            '%' => match chars.next() {
                Some('%') => arg.get_or_insert_default().push('%'),
                Some('c') => arg.get_or_insert_default().push_str(name),
                Some('k') => arg
                    .get_or_insert_default()
                    .push_str(&path.to_string_lossy()),
                _ => {}
            },
            c => arg.get_or_insert_default().push(c),
        }
    }
    args.extend(arg);
    // Flatpak's marks around the files it forwards.
    args.retain(|arg| !matches!(arg.as_str(), "@@" | "@@u" | "@@f"));
    Some(args)
}

/// `arg` as one word for `sh`.
pub fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Whether `program` is an executable file: at that path, or in `$PATH`.
fn installed(program: &str) -> bool {
    let executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return executable(Path::new(program));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| executable(&dir.join(program)))
}

/// The XDG data folders, the one to look in first first.
fn data_dirs() -> Vec<PathBuf> {
    let absolute = |dir: &PathBuf| dir.is_absolute();
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(absolute);
    let home = home.or_else(|| Some(home_dir()?.join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty());
    let dirs = dirs.unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut all: Vec<PathBuf> = home
        .into_iter()
        .chain(dirs.split(':').map(PathBuf::from).filter(absolute))
        .collect();
    let mut seen = HashSet::new();
    all.retain(|dir| seen.insert(dir.clone()));
    all
}

/// The host's desktops, lower case, for `OnlyShowIn` and `NotShowIn`.
fn current_desktops() -> Vec<String> {
    let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    desktops
        .split(':')
        .filter(|d| !d.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The locales to look for translated names in, the closest first: for
/// "de_AT.UTF-8", "de_AT" then "de".
fn locales() -> Vec<String> {
    let var = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
    let Some(locale) = var else { return Vec::new() };
    let locale = locale.split(['.', '@']).next().unwrap_or_default();
    if matches!(locale, "" | "C" | "POSIX") {
        return Vec::new();
    }
    let mut locales = vec![locale.to_owned()];
    if let Some((lang, _)) = locale.split_once('_') {
        locales.push(lang.to_owned());
    }
    locales
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{app, exec_args, shell_quote, unescape};
    use crate::icons::IconIndex;

    fn args(exec: &str) -> Option<Vec<String>> {
        exec_args(&unescape(exec), "Files", Path::new("/a/b.desktop"))
    }

    #[test]
    fn exec_lines() {
        assert_eq!(args("firefox %u").unwrap(), ["firefox"]);
        assert_eq!(
            args("app --name=%c -k %k %%x %F").unwrap(),
            ["app", "--name=Files", "-k", "/a/b.desktop", "%x"]
        );
        assert_eq!(
            args(r#"sh -c "echo \\"hi\\" \\$HOME""#).unwrap(),
            ["sh", "-c", r#"echo "hi" $HOME"#]
        );
        assert_eq!(args(r#"app "" x"#).unwrap(), ["app", "", "x"]);
        assert_eq!(args(r#"app "open"#), None);
        let flatpak = "/usr/bin/flatpak run --branch=stable --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@";
        assert_eq!(
            args(flatpak).unwrap().last().map(String::as_str),
            Some("org.mozilla.firefox")
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("--x=1"), "--x=1");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn entries() {
        let desktops = ["gnome".to_owned()];
        let locales = ["de_AT".to_owned(), "de".to_owned()];
        let entry = |extra: &str| {
            let text = format!(
                "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nComment=Browse\nExec=sh %U\n{extra}\n[Desktop Action new]\nExec=other\n"
            );
            app(
                &text,
                Path::new("/a/b.desktop"),
                &desktops,
                &locales,
                &IconIndex::default(),
            )
        };
        let files = entry("").unwrap();
        assert_eq!(
            (
                files.name.as_str(),
                files.command.as_str(),
                files.detail.as_str()
            ),
            ("Dateien", "sh", "Browse")
        );
        assert!(entry("NoDisplay=true").is_none());
        assert!(entry("Terminal=true").is_none());
        assert!(entry("OnlyShowIn=KDE;").is_none());
        assert!(entry("OnlyShowIn=KDE;GNOME;").is_some());
        assert!(entry("NotShowIn=GNOME;").is_none());
        assert!(entry("TryExec=/nonexistent/app").is_none());
    }
}
