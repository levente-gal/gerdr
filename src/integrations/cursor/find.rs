//! Finding a Cursor CLI chat: its `store.db` by id, else by folder and
//! time from the transcript folders Cursor makes per chat; on another
//! machine, by id only.

use std::{collections::HashSet, path::Path};

use crate::ai_work::{
    AgentKind, Chat,
    locate::{Form, Found, Homes, Span, found, plain_id, subdirs},
};

/// What a Cursor chat's folder holds beside its `store.db`.
const CURSOR_EXTRAS: [&str; 3] = ["meta.json", "prompt_history.json", "pasted_text.json"];

/// Cursor CLI chat `id`: `chats/<folder>/<id>/store.db`.
pub(crate) fn cursor(homes: &Homes, id: &str) -> Option<Found> {
    let folders = std::fs::read_dir(homes.cursor.join("chats")).ok()?;
    let main = (folders.flatten())
        .map(|dir| dir.path().join(id).join("store.db"))
        .find(|db| db.is_file())?;
    let folder = main.parent()?.to_path_buf();
    let mut chat = found(AgentKind::Cursor, id, main, Form::Store);
    chat.extra = (CURSOR_EXTRAS.iter())
        .map(|name| (name.to_string(), folder.join(name)))
        .filter(|(_, path)| path.is_file())
        .collect();
    Some(chat)
}

/// The folder Cursor keeps folder `dir`'s things in under `projects/`: its
/// path with each run of other than letters and digits one `-`, none at
/// either end (`/home/me/.cache/x` is `home-me-cache-x`).
pub fn cursor_slug(dir: &Path) -> String {
    let path = dir.to_string_lossy();
    let words: Vec<&str> = (path.split(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    words.join("-")
}

/// Whether `name`, a folder under Cursor's `projects/`, is the one of
/// `slug`. Newer Cursors cut a long slug to 55 bytes and add `-` and 7 hex
/// digits of a hash gerdr doesn't know, so a cut one may be another
/// folder's of the same start: the pick by time sorts them out.
fn cursor_slug_is(name: &str, slug: &str) -> bool {
    const CUT: usize = 55;
    let cut = || {
        slug.len() > CUT
            && name.len() == CUT + 8
            && name.starts_with(&slug[..CUT])
            && name.as_bytes()[CUT] == b'-'
            && name[CUT + 1..].bytes().all(|b| b.is_ascii_hexdigit())
    };
    name == slug || cut()
}

/// Of Cursor transcripts `begun` (an id and when its folder was made), the
/// ones a run may have, nearest `span`'s time first: begun in `span`, not
/// `claimed`.
fn cursor_pick(span: Span, claimed: &HashSet<String>, begun: Vec<(String, u64)>) -> Vec<String> {
    let mut fit: Vec<(String, u64)> = (begun.into_iter())
        .filter(|(id, at)| {
            (span.from..=span.to).contains(at) && !claimed.contains(id) && plain_id(id)
        })
        .collect();
    fit.sort_by_key(|(id, at)| (at.abs_diff(span.near), id.clone()));
    fit.into_iter().map(|(id, _)| id).collect()
}

/// The Cursor chat of `chat`'s run, by folder and time: Cursor keeps a
/// transcript folder per chat, `projects/<slug>/agent-transcripts/<id>/`,
/// made with its first message; the chat is the `store.db` of that id.
pub fn cursor_by_time(
    homes: &Homes,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let slugs: Vec<String> = chat.folders.iter().map(|f| cursor_slug(f)).collect();
    let begun = (subdirs(&homes.cursor.join("projects")).into_iter())
        .filter(|dir| {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            slugs.iter().any(|slug| cursor_slug_is(name, slug))
        })
        .flat_map(|dir| subdirs(&dir.join("agent-transcripts")))
        .filter_map(|dir| {
            let id = dir.file_name()?.to_str()?.to_owned();
            let meta = dir.metadata().ok()?;
            let made = meta.created().or_else(|_| meta.modified()).ok()?;
            let at = made.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            Some((id, at))
        })
        .collect();
    (cursor_pick(span, claimed, begun).iter()).find_map(|id| cursor(homes, id))
}

/// The body of `script` for Cursor chat `id`.
pub(crate) fn cursor_script(id: &str) -> String {
    format!(
        r#"id='{id}'
for f in "$HOME"/.cursor/chats/*/"$id"/store.db; do
  [ -f "$f" ] || continue
  say M '' "$f"
  [ -f "$f-wal" ] && say W '' "$f-wal"
  d="${{f%/store.db}}"
  for x in {extras}; do [ -f "$d/$x" ] && say X "$x" "$d/$x"; done
  exit 0
done
"#,
        extras = CURSOR_EXTRAS.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::{
        locate::{
            find,
            tests::{chat, put},
        },
        write::tests::scratch,
    };

    #[test]
    fn a_cursor_chat_is_its_store_and_files() {
        let home = scratch("locate-cursor");
        let homes = Homes::under(&home);
        let folder = home.join(".cursor/chats/f00/c-1");
        put(&folder.join("store.db"), "");
        put(&folder.join("meta.json"), "{}");
        let c = chat(AgentKind::Cursor, Some("c-1"), 0, &[]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!(f.main, folder.join("store.db"));
        assert_eq!(f.form, Form::Store);
        assert_eq!(
            f.extra,
            vec![("meta.json".to_owned(), folder.join("meta.json"))]
        );
    }

    #[test]
    fn cursor_folders_are_named_as_cursor_does() {
        let slug = |dir: &str| cursor_slug(Path::new(dir));
        assert_eq!(
            slug("/home/lgal/Work/alphaocean/ao-app"),
            "home-lgal-Work-alphaocean-ao-app"
        );
        assert_eq!(slug("/home/me/.cache/x"), "home-me-cache-x");
        assert_eq!(slug("/tmp/c-1/-home-me/a_b.c/"), "tmp-c-1-home-me-a-b-c");
        let long = slug("/home/lgal/Work/alphaocean/ao-app/features/alpha-dashboard-redesign");
        assert!(cursor_slug_is(&long, &long));
        assert!(cursor_slug_is(
            "home-lgal-Work-alphaocean-ao-app-features-alpha-dashboa-b953f1b",
            &long
        ));
        assert!(!cursor_slug_is(
            "home-lgal-Work-alphaocean-ao-app-features-alpha-dashboa-xyz1234",
            &long
        ));
        assert!(!cursor_slug_is("home-me-cache-x-y", "home-me-cache-x"));
    }

    #[test]
    fn a_cursor_chat_is_found_by_folder_and_time() {
        let home = scratch("locate-cursor-time");
        let homes = Homes::under(&home);
        let work = Path::new("/work/my.app");
        let transcripts = home.join(".cursor/projects/work-my-app/agent-transcripts");
        for id in ["c-1", "c-2"] {
            std::fs::create_dir_all(transcripts.join(id)).unwrap();
        }
        put(&home.join(".cursor/chats/f00/c-2/store.db"), "");
        // Begun just now: c-1 has no store.db yet, so c-2 is the chat.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let c = chat(AgentKind::Cursor, None, now, &[work]);
        let f = cursor_by_time(&homes, &c, &HashSet::new(), Span::of(now)).unwrap();
        assert_eq!((f.id.as_str(), f.form), ("c-2", Form::Store));
        assert_eq!(
            cursor_by_time(&homes, &c, &HashSet::from(["c-2".into()]), Span::of(now)),
            None
        );
        let other = chat(AgentKind::Cursor, None, now, &[Path::new("/work/my")]);
        assert_eq!(
            cursor_by_time(&homes, &other, &HashSet::new(), Span::of(now)),
            None
        );
        let later = chat(AgentKind::Cursor, None, now + 3600, &[work]);
        assert_eq!(
            cursor_by_time(&homes, &later, &HashSet::new(), Span::of(now + 3600)),
            None
        );
    }

    #[test]
    fn the_cursor_chat_begun_nearest_its_run_is_picked() {
        let c = Span::of(1000);
        let begun = |list: &[(&str, u64)]| -> Vec<(String, u64)> {
            list.iter().map(|(id, at)| (id.to_string(), *at)).collect()
        };
        let all = begun(&[
            ("early", 939),
            ("before", 990),
            ("after", 1005),
            ("far", 1600),
            ("late", 1601),
            ("../x", 1000),
        ]);
        assert_eq!(
            cursor_pick(c, &HashSet::new(), all.clone()),
            ["after", "before", "far"]
        );
        let claimed = HashSet::from(["after".to_owned()]);
        assert_eq!(cursor_pick(c, &claimed, all), ["before", "far"]);
    }
}
