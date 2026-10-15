//! Paths and titles as the chrome shows them: from the home folder as `~`,
//! and cut in the middle when too long, a whole folder at a time, so the
//! start and the end stay readable ("~/Work/…/service/api").

use crate::herdr::PaneState;
use crate::home_dir;
use std::path::Path;

/// How long `short_path` lets a path get, in characters.
const SHORT_PATH_CHARS: usize = 28;

/// `path` with the home folder `home` written `~`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `path` as the chrome shows it: from the home folder `home` as `~`, and
/// at most `max_chars` long when that is possible without cutting into a
/// folder's name: the folders in the middle give way to "…", the start and
/// the last folders stay ("~/Work/…/service/api"). The last folder is
/// always whole, so a long name alone may still run over.
pub fn display(path: &Path, home: Option<&Path>, max_chars: usize) -> String {
    let (start, rest) = match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => ("~", rest),
        None => ("", path.strip_prefix("/").unwrap_or(path)),
    };
    shorten(start, rest, max_chars)
}

/// A terminal title without the spinner or status glyph agents put in front.
pub(crate) fn tidy_title(title: &str) -> String {
    let glyph = |c: char| c.is_whitespace() || matches!(c, '\u{2190}'..='\u{2BFF}' | '·' | '•');
    title.trim_start_matches(glyph).trim_end().to_string()
}

/// The name of the repository pane `p` is in, and its worktree's folder.
pub(crate) fn repo_root(p: &PaneState) -> Option<(&str, &std::path::Path)> {
    p.repo
        .as_deref()
        .zip(p.worktree.as_ref().map(|w| w.path.as_path()))
}

/// `path`, short: from the repository named `repo` when it is inside that
/// repository's worktree ("gerdr/src/bin"), else as `display` shows it,
/// `SHORT_PATH_CHARS` long at most either way.
pub(crate) fn short_path(path: &std::path::Path, root: Option<(&str, &std::path::Path)>) -> String {
    let in_repo = root.and_then(|(repo, worktree)| Some((repo, path.strip_prefix(worktree).ok()?)));
    match in_repo {
        Some((repo, rest)) => shorten(repo, rest, SHORT_PATH_CHARS),
        None => display(path, home_dir().as_deref(), SHORT_PATH_CHARS),
    }
}

/// `start` ("~", a repository's name, or "" for the root) followed by the
/// folders of `rest`, "/" between them, cut to `max_chars` by replacing
/// folders after the first with "…" (see `display`).
fn shorten(start: &str, rest: &Path, max_chars: usize) -> String {
    let names: Vec<String> = rest
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if names.is_empty() {
        return if start.is_empty() {
            "/".into()
        } else {
            start.into()
        };
    }
    let join = |tail: &[String]| {
        let mut parts = vec![start.to_string()];
        parts.extend(tail.iter().cloned());
        parts.join("/")
    };
    let whole = join(&names);
    if whole.chars().count() <= max_chars {
        return whole;
    }
    let len = |s: &str| s.chars().count();
    // The most folders at the end that fit after "start/…/", at least one.
    let room = max_chars.saturating_sub(len(start) + len("/…/"));
    let mut keep = 1;
    let mut used = len(&names[names.len() - 1]);
    while keep < names.len() - 1 {
        let next = len(&names[names.len() - keep - 1]) + 1;
        if used + next > room {
            break;
        }
        used += next;
        keep += 1;
    }
    if keep == names.len() {
        // One folder, too long on its own: nothing to cut from the middle.
        return whole;
    }
    let tail = &names[names.len() - keep..];
    format!("{}/…/{}", start, tail.join("/"))
}

#[cfg(test)]
mod tests {
    use super::{display, short_path};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[test]
    fn display_writes_home_as_tilde_and_keeps_short_paths_whole() {
        let home = Some(Path::new("/home/x"));
        assert_eq!(
            display(Path::new("/home/x/Work/api"), home, 40),
            "~/Work/api"
        );
        assert_eq!(display(Path::new("/home/x"), home, 40), "~");
        assert_eq!(
            display(Path::new("/usr/local/share"), home, 40),
            "/usr/local/share"
        );
        assert_eq!(display(Path::new("/home/x/Work"), None, 40), "/home/x/Work");
    }

    #[test]
    fn display_cuts_whole_folders_in_the_middle() {
        let home = Some(Path::new("/home/x"));
        let long = Path::new("/home/x/Work/clients/acme/service/api");
        assert_eq!(display(long, home, 40), "~/Work/clients/acme/service/api");
        assert_eq!(display(long, home, 22), "~/…/acme/service/api");
        assert_eq!(display(long, home, 18), "~/…/service/api");
        assert_eq!(display(long, home, 8), "~/…/api");
        // The last folder is never cut into, even when it alone is too long.
        assert_eq!(display(long, home, 3), "~/…/api");
        // The first folder gives way only after all the others.
        assert_eq!(display(Path::new("/home/x/Work/api"), home, 9), "~/…/api");
        // The ellipsis counts as one character: 18 fits "~/…/service/api".
        assert_eq!(display(long, home, 15), "~/…/service/api");
        assert_eq!(display(long, home, 14), "~/…/api");
        // A lone folder longer than the room stays whole: nothing to cut.
        assert_eq!(
            display(Path::new("/home/x/a-very-long-folder-name"), home, 8),
            "~/a-very-long-folder-name"
        );
    }

    #[test]
    fn display_handles_the_root() {
        assert_eq!(display(Path::new("/"), None, 40), "/");
        assert_eq!(display(Path::new("/tmp"), None, 40), "/tmp");
        assert_eq!(
            display(Path::new("/var/lib/flatpak/app/org.x"), None, 16),
            "/…/app/org.x"
        );
        // Home itself is the root of nothing: no "~" without a home.
        assert_eq!(
            display(Path::new("/home"), Some(Path::new("/home/x")), 40),
            "/home"
        );
    }

    #[test]
    fn display_shows_what_it_can_of_a_non_utf8_name() {
        let raw = Path::new(OsStr::from_bytes(b"/home/x/caf\xe9/menu"));
        assert_eq!(
            display(raw, Some(Path::new("/home/x")), 40),
            "~/caf\u{FFFD}/menu"
        );
    }

    #[test]
    fn short_path_starts_from_the_repository_or_the_home() {
        let root = Some(("gerdr", Path::new("/work/gerdr")));
        assert_eq!(short_path(Path::new("/work/gerdr"), root), "gerdr");
        assert_eq!(
            short_path(Path::new("/work/gerdr/src/bin/tool"), root),
            "gerdr/src/bin/tool"
        );
        assert_eq!(
            short_path(Path::new("/work/gerdr/target/release/build/deps/out"), root),
            "gerdr/…/build/deps/out"
        );
        assert_eq!(
            short_path(Path::new("/elsewhere/dir"), root),
            "/elsewhere/dir"
        );
        assert_eq!(short_path(Path::new("/"), None), "/");
        assert_eq!(
            short_path(Path::new("/usr/local/share/fonts/truetype/dejavu"), None),
            "/…/fonts/truetype/dejavu"
        );
    }
}
