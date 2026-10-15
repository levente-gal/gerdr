//! Where a web page's window is now. A web page opens like an installed
//! web app, in a window of its own (see `launch::web_app`), and the browser
//! is told to serve the DevTools protocol on a free port of localhost. It
//! writes the port to `DevToolsActivePort` in its profile folder, and
//! `/json/list` there names each page's window by title with the URL it
//! shows. The panes' windows carry the same titles, so each pane's URL is
//! read back now and then and takes the place of the link it was launched
//! with: a restart brings the page back where it was left.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::launch;

/// How long a read of the page list may take: the browser answers on
/// localhost at once, or not at all.
const TIMEOUT: Duration = Duration::from_millis(500);

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
pub fn read_all(panes: Vec<WebPane>, send: impl FnOnce(Vec<(WebPane, String)>) + Send + 'static) {
    std::thread::spawn(move || send(read_now(panes)));
}

/// Where the panes' pages are now, by the pane: the browser profile of each
/// pane's link, then the pages the browser of each profile shows, matched
/// to the panes by title. Waits on each browser up to `TIMEOUT`.
pub fn read_now(panes: Vec<WebPane>) -> Vec<(WebPane, String)> {
    {
        let mut by_profile: HashMap<PathBuf, Vec<WebPane>> = HashMap::new();
        for pane in panes {
            if let Some(profile) = launch::web_profile(&pane.command) {
                by_profile.entry(profile).or_default().push(pane);
            }
        }
        let mut found = Vec::new();
        for (profile, panes) in by_profile {
            let Some(port) = port(&profile) else { continue };
            let pages = match pages(port) {
                Ok(pages) => pages,
                Err(err) => {
                    tracing::debug!("pages of {}: {err}", profile.display());
                    continue;
                }
            };
            found.extend(matched(panes, &pages));
        }
        found
    }
}

/// The port the browser with `profile` serves DevTools on, if it runs.
fn port(profile: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    text.lines().next()?.trim().parse().ok()
}

/// The pages the browser on `port` shows.
fn pages(port: u16) -> Result<Vec<Page>, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /json/list HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8_lossy(&response);
    let body = response.split_once("\r\n\r\n").map(|(_, body)| body);
    parse(body.ok_or("no body")?)
}

/// The pages in the browser's `/json/list`: the targets of type `page`.
fn parse(body: &str) -> Result<Vec<Page>, String> {
    let targets: Vec<serde_json::Value> = serde_json::from_str(body).map_err(|e| e.to_string())?;
    Ok(targets
        .iter()
        .filter(|t| t["type"].as_str() == Some("page"))
        .filter_map(|t| {
            Some(Page {
                title: t["title"].as_str()?.to_owned(),
                url: t["url"].as_str()?.to_owned(),
            })
        })
        .collect())
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
    use super::{matched, parse, Page, WebPane};

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

    #[test]
    fn parses_page_targets_only() {
        let body = r#"[
            {"type": "page", "title": "Docs", "url": "https://a.b/docs"},
            {"type": "service_worker", "title": "sw", "url": "https://a.b/sw.js"},
            {"type": "page", "title": "", "url": "chrome://newtab/"}
        ]"#;
        assert_eq!(
            parse(body).unwrap(),
            vec![
                page("Docs", "https://a.b/docs"),
                page("", "chrome://newtab/")
            ]
        );
        assert!(parse("nope").is_err());
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
