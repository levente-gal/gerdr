//! Where a web page's window is now. A web page opens like an installed
//! web app, in a window of its own (see `launch::web_app`), and the browser
//! keeps its app windows' session in `Default/Sessions/Apps_<time>` of its
//! profile folder: a log of each tab's pages, by title and URL, and which
//! one it shows. The panes' windows carry the same titles, so each pane's
//! URL is read back now and then and takes the place of the link it was
//! launched with: a restart brings the page back where it was left.
//!
//! The browser writes the log a few seconds behind. It is read rather than
//! asked over DevTools: sites like Google's refuse to sign in a browser
//! serving DevTools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::launch;

/// A page the browser shows, by its window's title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub url: String,
}

/// A pane showing a web page: where it is, the link it was launched with
/// (or last read), and its window's title.
#[derive(Clone, Debug)]
pub struct WebPane {
    pub workspace: usize,
    pub tab: usize,
    pub pane: u32,
    pub command: String,
    pub title: String,
}

/// Where the panes' pages are now, by the pane, read off the event loop
/// (see `read_now`).
pub fn read_all(
    panes: Vec<WebPane>,
    instance: PathBuf,
    send: impl FnOnce(Vec<(WebPane, String)>) + Send + 'static,
) {
    std::thread::spawn(move || send(read_now(panes, &instance)));
}

/// Where the panes' pages are now, by the pane: the browser profile of each
/// pane's link in instance folder `instance` (see `launch::instance_dir`),
/// then the pages its app windows' session shows, matched to the panes by
/// title.
pub fn read_now(panes: Vec<WebPane>, instance: &Path) -> Vec<(WebPane, String)> {
    let mut by_profile: HashMap<PathBuf, Vec<WebPane>> = HashMap::new();
    for pane in panes {
        if let Some(profile) = launch::web_profile(&pane.command, instance) {
            by_profile.entry(profile).or_default().push(pane);
        }
    }
    let mut found = Vec::new();
    for (profile, panes) in by_profile {
        let Some(session) = session_file(&profile) else {
            continue;
        };
        match std::fs::read(&session) {
            Ok(bytes) => found.extend(matched(panes, &pages(&bytes))),
            Err(err) => tracing::debug!("pages of {}: {err}", session.display()),
        }
    }
    found
}

/// The app windows' session the browser with `profile` writes now, or last
/// wrote: the newest `Apps_` file of its sessions.
fn session_file(profile: &Path) -> Option<PathBuf> {
    std::fs::read_dir(profile.join("Default/Sessions"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("Apps_"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
}

/// The commands of a session file the pages are read from (see Chromium's
/// `session_service_commands.cc`).
const SET_TAB_WINDOW: u8 = 0;
const UPDATE_TAB_NAVIGATION: u8 = 6;
const SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
const TAB_CLOSED: u8 = 16;
const WINDOW_CLOSED: u8 = 17;

/// A tab of a session file: its pages by their place in its history, the
/// one it shows, and its window.
#[derive(Default)]
struct Tab {
    pages: HashMap<i32, Page>,
    /// The place last written, for a tab not told which it shows.
    last: Option<i32>,
    shown: Option<i32>,
    window: Option<i32>,
}

/// The pages the tabs still open in session file `bytes` show. The file is
/// `SNSS`, its version, then commands, each its size (of the rest) in two
/// bytes, its id in one, then its payload. A command cut short, as the
/// browser writes, ends it.
fn pages(bytes: &[u8]) -> Vec<Page> {
    let Some(mut rest) = bytes.strip_prefix(b"SNSS").and_then(|r| r.get(4..)) else {
        return Vec::new();
    };
    let mut tabs: HashMap<i32, Tab> = HashMap::new();
    while let [lo, hi, after @ ..] = rest {
        let size = u16::from_le_bytes([*lo, *hi]) as usize;
        let Some((&id, payload)) = after.get(..size).and_then(<[u8]>::split_first) else {
            break;
        };
        rest = &after[size..];
        match id {
            UPDATE_TAB_NAVIGATION => {
                if let Some((tab, index, page)) = navigation(payload) {
                    let tab = tabs.entry(tab).or_default();
                    tab.pages.insert(index, page);
                    tab.last = Some(index);
                }
            }
            SET_SELECTED_NAVIGATION_INDEX => {
                if let (Some(tab), Some(index)) = (int(payload, 0), int(payload, 4)) {
                    tabs.entry(tab).or_default().shown = Some(index);
                }
            }
            SET_TAB_WINDOW => {
                if let (Some(window), Some(tab)) = (int(payload, 0), int(payload, 4)) {
                    tabs.entry(tab).or_default().window = Some(window);
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = int(payload, 0) {
                    tabs.remove(&tab);
                }
            }
            WINDOW_CLOSED => {
                if let Some(window) = int(payload, 0) {
                    tabs.retain(|_, tab| tab.window != Some(window));
                }
            }
            _ => {}
        }
    }
    tabs.into_values()
        .filter_map(|mut tab| tab.pages.remove(&tab.shown.or(tab.last)?))
        .collect()
}

/// The tab, place in its history and page of an update of a tab's
/// navigation: a pickle, its payload's size in four bytes, then the tab's
/// id and the place, the URL in UTF-8 and the title in UTF-16, each its
/// length then itself, padded to four bytes.
fn navigation(pickle: &[u8]) -> Option<(i32, i32, Page)> {
    let tab = int(pickle, 4)?;
    let index = int(pickle, 8)?;
    let len = usize::try_from(int(pickle, 12)?).ok()?;
    let url = std::str::from_utf8(pickle.get(16..16 + len)?)
        .ok()?
        .to_owned();
    let at = 16 + len.next_multiple_of(4);
    let len = usize::try_from(int(pickle, at)?).ok()?;
    let units: Vec<u16> = pickle
        .get(at + 4..at + 4 + 2 * len)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|unit| u16::from_le_bytes(*unit))
        .collect();
    let title = String::from_utf16_lossy(&units);
    Some((tab, index, Page { title, url }))
}

/// The little-endian `i32` at `at` of `bytes`.
fn int(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Whether a window titled `window` shows the page titled `page`: a web
/// app's window takes the page's title, some browsers with their name
/// after it.
fn same_title(window: &str, page: &str) -> bool {
    !page.is_empty()
        && (window == page
            || window
                .strip_prefix(page)
                .is_some_and(|rest| rest.starts_with(" - ") || rest.starts_with(" – ")))
}

/// Each pane of `panes` with the URL of the one page of `pages` its title
/// matches, when it is the only pane matching it; one whose URL is the link
/// it has is left out.
fn matched(panes: Vec<WebPane>, pages: &[Page]) -> Vec<(WebPane, String)> {
    let mut found = Vec::new();
    for (i, pane) in panes.iter().enumerate() {
        let mut matching = pages
            .iter()
            .filter(|page| same_title(&pane.title, &page.title));
        let (Some(page), None) = (matching.next(), matching.next()) else {
            continue;
        };
        let others = panes
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && same_title(&other.title, &page.title));
        if others || pane.command.trim() == page.url {
            continue;
        }
        found.push((pane.clone(), page.url.clone()));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{Page, WebPane, matched, pages};

    fn pane(pane: u32, command: &str, title: &str) -> WebPane {
        WebPane {
            workspace: 0,
            tab: 0,
            pane,
            command: command.to_owned(),
            title: title.to_owned(),
        }
    }

    fn page(title: &str, url: &str) -> Page {
        Page {
            title: title.to_owned(),
            url: url.to_owned(),
        }
    }

    /// A session file of `commands`, each its id and payload.
    fn session(commands: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"SNSS".to_vec();
        bytes.extend(3i32.to_le_bytes());
        for (id, payload) in commands {
            bytes.extend(u16::try_from(payload.len() + 1).unwrap().to_le_bytes());
            bytes.push(*id);
            bytes.extend(payload);
        }
        bytes
    }

    fn ints(ints: &[i32]) -> Vec<u8> {
        ints.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes
    }

    /// An update of `tab`'s navigation at `index` to `url` titled `title`.
    fn navigation(tab: i32, index: i32, url: &str, title: &str) -> (u8, Vec<u8>) {
        let mut body = ints(&[tab, index, url.len() as i32]);
        body.extend(padded(url.as_bytes().to_vec()));
        let units: Vec<u16> = title.encode_utf16().collect();
        body.extend(ints(&[units.len() as i32]));
        body.extend(padded(units.iter().flat_map(|u| u.to_le_bytes()).collect()));
        // What follows the title is not read.
        body.extend(ints(&[0, 0]));
        let mut pickle = ints(&[body.len() as i32]);
        pickle.extend(body);
        (6, pickle)
    }

    fn sorted(mut pages: Vec<Page>) -> Vec<Page> {
        pages.sort_by(|a, b| a.url.cmp(&b.url));
        pages
    }

    #[test]
    fn reads_the_page_each_open_tab_shows() {
        let bytes = session(&[
            (0, ints(&[1, 10])),
            (0, ints(&[1, 11])),
            (0, ints(&[2, 12])),
            (0, ints(&[3, 13])),
            navigation(10, 0, "https://a.b", "A"),
            (7, ints(&[10, 1])),
            navigation(10, 1, "https://a.b/next", "Next – Ä"),
            navigation(10, 2, "https://a.b/gone", "Gone"),
            // Back, then on to another page in the same place.
            (7, ints(&[11, 0])),
            navigation(11, 0, "https://c.d", "C"),
            navigation(11, 1, "https://c.d/1", "C1"),
            (7, ints(&[11, 0])),
            navigation(12, 0, "https://e.f", "Closed tab"),
            (16, ints(&[12, 0, 0, 0])),
            navigation(13, 0, "https://g.h", "Closed window"),
            (17, ints(&[3, 0, 0, 0])),
            // Unknown commands are passed over.
            (14, ints(&[10, 7])),
            navigation(14, 0, "https://i.j", "Untold"),
            navigation(14, 1, "https://i.j/2", "Untold 2"),
        ]);
        let expected = vec![
            page("Next – Ä", "https://a.b/next"),
            page("C", "https://c.d"),
            page("Untold 2", "https://i.j/2"),
        ];
        assert_eq!(sorted(pages(&bytes)), expected);
        // A command cut short, as the browser writes, ends the file.
        let mut cut = bytes.clone();
        cut.extend(session(&[navigation(10, 1, "https://a.b/later", "L")])[8..20].iter());
        assert_eq!(sorted(pages(&cut)), expected);
        assert!(pages(b"nope").is_empty());
    }

    #[test]
    fn matches_panes_to_pages_by_title() {
        let panes = vec![
            pane(1, "https://a.b", "Docs"),
            pane(2, "https://c.d", "Mail - Brave"),
            pane(3, "https://e.f", "Same"),
            pane(4, "https://g.h", "Same"),
            pane(5, "https://i.j/x", "Still"),
            pane(6, "https://k.l", ""),
        ];
        let pages = [
            page("Docs", "https://a.b/docs"),
            page("Mail", "https://c.d/inbox"),
            page("Same", "https://e.f/1"),
            page("Still", "https://i.j/x"),
            page("", "chrome://newtab/"),
        ];
        let found: Vec<(u32, String)> = matched(panes, &pages)
            .into_iter()
            .map(|(pane, url)| (pane.pane, url))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, "https://a.b/docs".to_owned()),
                (2, "https://c.d/inbox".to_owned())
            ]
        );
    }

    #[test]
    fn two_pages_with_one_title_match_nothing() {
        let panes = vec![pane(1, "https://a.b", "Docs")];
        let pages = [page("Docs", "https://a.b/1"), page("Docs", "https://a.b/2")];
        assert!(matched(panes, &pages).is_empty());
    }
}
