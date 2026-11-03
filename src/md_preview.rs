//! A markdown file's rendered preview, in a web tab: a page of gerdr's own
//! browser extension (`assets/web-extension/preview.html`), which shows
//! what gerdr sends it over native messaging (see `web_control`). The file
//! is rendered here, by comrak, as GitHub does (tables, task lists,
//! footnotes), and styled by `github-markdown-css` in the page.
//!
//! The page's link names the file (`page_url`), so a tab brought back by
//! a restart shows it again: gerdr takes a tab as a preview once its
//! extension reports the page (`Gerdr::preview_page`). The file is read
//! again with every Git status read and after an edit in its review, and
//! sent again when it, the theme or its review changed.
//!
//! The markdown is an agent's, and the page has the extension's rights:
//! raw HTML in it is left out (comrak's `unsafe` stays off), and the page
//! puts what it gets in its article only. Its links open through gerdr.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use crate::{
    Gerdr, Theme,
    feature::{Stage, artifacts, store},
    web_control::{self, EXTENSION_ID},
};

/// The extension's page showing a preview.
pub const PAGE: &str = "preview.html";

/// The key showing the file under review's preview.
pub const PREVIEW_KEY: &str = "Ctrl+Shift+V";

/// A file bigger than this isn't rendered: the page says so.
const FILE_MAX: u64 = 512 * 1024;

/// Where a preview that can't open tells the user to look (see
/// `problems`).
pub(crate) const PROBLEM: &str = "preview";

/// Why a preview can't open.
pub const NO_EXTENSION: &str =
    "The preview needs gerdr's browser extension: a Chromium browser, not a Flatpak one";

/// The link of the page previewing `file`.
pub fn page_url(file: &Path) -> String {
    format!(
        "chrome-extension://{EXTENSION_ID}/{PAGE}?file={}",
        web_control::percent_encoded(&file.to_string_lossy())
    )
}

/// The file the preview page at `url` shows, if it is one.
pub fn file_of(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix(&format!("chrome-extension://{EXTENSION_ID}/{PAGE}?"))?;
    let rest = rest.split('#').next()?;
    let value = rest
        .split('&')
        .find_map(|pair| pair.strip_prefix("file="))?;
    let file = PathBuf::from(percent_decoded(value)?);
    file.is_absolute().then_some(file)
}

/// `text` with its `%XX` escapes decoded; `None` where they don't make
/// UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `text` rendered as GitHub renders markdown, each block marked with the
/// lines it comes from (`data-sourcepos="3:1-5:20"`). Raw HTML is left
/// out, and links to `javascript:` and the like are dropped.
pub fn render(text: &str) -> String {
    let mut options = comrak::Options::default();
    let ext = &mut options.extension;
    ext.table = true;
    ext.strikethrough = true;
    ext.tasklist = true;
    ext.autolink = true;
    ext.footnotes = true;
    ext.header_id_prefix = Some(String::new());
    options.render.sourcepos = true;
    options.render.github_pre_lang = true;
    comrak::markdown_to_html(text, &options)
}

/// What a preview's page shows: the file rendered, or why it can't be.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Shown {
    /// The page's title: `Preview · brief.md`.
    pub title: String,
    pub body: Result<String, String>,
    pub dark: bool,
    /// The file is a feature stage's: the page offers to review it.
    pub review: bool,
    /// Its review's comments, by the line they are on: the page marks
    /// their blocks.
    pub marks: Vec<(usize, String)>,
}

impl Shown {
    /// `file` (named `name`) read and rendered.
    fn of(file: &Path, name: &str, dark: bool, review: bool) -> Self {
        Shown {
            title: format!("Preview \u{b7} {name}"),
            body: read(file, name).map(|text| render(&text)),
            dark,
            review,
            marks: Vec::new(),
        }
    }

    /// Something that changes with what it shows.
    fn hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        Hash::hash(self, &mut hasher);
        hasher.finish()
    }
}

/// `file`, named `name`, read: or why it can't be shown, as the page says
/// it.
fn read(file: &Path, name: &str) -> Result<String, String> {
    let opened = std::fs::File::open(file).and_then(|f| Ok((f.metadata()?.len(), f)));
    let (size, file_handle) = match opened {
        Ok(opened) => opened,
        Err(err) if err.kind() == ErrorKind::NotFound => return Err(format!("{name} is gone")),
        Err(err) => {
            tracing::warn!("previewing {}: {err}", file.display());
            return Err(format!("Couldn't read {name}: {err}"));
        }
    };
    if size > FILE_MAX {
        return Err(format!("{name} is too big to preview"));
    }
    let mut text = String::new();
    match file_handle.take(FILE_MAX + 1).read_to_string(&mut text) {
        Ok(_) => Ok(text),
        Err(err) => {
            tracing::warn!("previewing {}: {err}", file.display());
            Err(format!("Couldn't read {name}: {err}"))
        }
    }
}

/// The web tabs showing a preview, by pane; and how one shows beside its
/// file's review (see `md_dock`).
pub struct Previews {
    shown: HashMap<u32, Preview>,
    /// Ctrl+Shift+V took the preview away from beside the review, until
    /// the review closes.
    pub undocked: bool,
    /// The review's share of the output's width beside its preview.
    pub share: f64,
    /// The preview's pane and place beside the review, as the windows
    /// were laid out last.
    pub docked: Option<(u32, crate::chrome::Rect)>,
}

impl Default for Previews {
    fn default() -> Self {
        Previews {
            shown: HashMap::new(),
            undocked: false,
            share: crate::chrome::DOCK_SHARE,
            docked: None,
        }
    }
}

struct Preview {
    file: PathBuf,
    /// What was sent to its page last (`Shown::hash`).
    sent: Option<u64>,
    /// The line of its review's caret sent to its page last, as its
    /// comments (`marks_hash`).
    caret: Option<Option<usize>>,
    marks: Option<u64>,
    /// The line at the top of its review and its page, as told to the
    /// page or by it last (see `DiffView::top_line`).
    top: Option<f64>,
}

impl Preview {
    fn new(file: PathBuf) -> Self {
        Preview {
            file,
            sent: None,
            caret: None,
            marks: None,
            top: None,
        }
    }
}

/// Something that changes with `marks`.
fn marks_hash(marks: &[(usize, String)]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    Hash::hash(marks, &mut hasher);
    hasher.finish()
}

impl Previews {
    /// Whether `pane` shows a preview.
    pub fn has(&self, pane: u32) -> bool {
        self.shown.contains_key(&pane)
    }

    /// Whether `pane` shows `file`'s preview.
    pub fn shows(&self, pane: u32, file: &Path) -> bool {
        self.shown.get(&pane).is_some_and(|p| p.file == file)
    }

    /// The file `pane`'s preview shows.
    pub fn file(&self, pane: u32) -> Option<PathBuf> {
        self.shown.get(&pane).map(|p| p.file.clone())
    }

    /// The line at the top of `pane`'s preview and its review, as told
    /// to its page or by it last.
    pub fn top(&self, pane: u32) -> Option<f64> {
        self.shown.get(&pane)?.top
    }

    /// `pane`'s preview and its review stand with `line` at their top.
    pub fn set_top(&mut self, pane: u32, line: f64) {
        if let Some(preview) = self.shown.get_mut(&pane) {
            preview.top = Some(line);
        }
    }
}

impl Gerdr {
    /// Show `file`'s preview in workspace `w`: its tab if one shows it,
    /// else a new one. Without gerdr's extension, it can't: an error says
    /// so.
    pub fn open_preview(&mut self, w: usize, file: &Path) {
        let Some(ws) = self.workspaces.get(w) else {
            return;
        };
        let url = page_url(file);
        let dir = ws.dir.clone();
        if !self.steers(&url, &dir) {
            crate::problems::report(PROBLEM, NO_EXTENSION);
            return self.show_error(NO_EXTENSION.into());
        }
        crate::problems::clear(PROBLEM);
        self.open_page(w, url);
    }

    /// Show the file under review's preview, in the review's workspace:
    /// the review closes for it, its comments kept.
    pub fn open_doc_preview(&mut self) {
        let Some(wd) = self.wd() else {
            return;
        };
        let Some(doc) = wd.doc.as_ref() else {
            return;
        };
        let (w, file) = (wd.workspace, doc.path.clone());
        self.close_worktree_diff();
        self.open_preview(w, &file);
    }

    /// Take `pane`, whose tab the extension says is at `url`, as a
    /// preview if `url` is one, its file sent to it; one gone elsewhere is
    /// not one any more.
    pub(crate) fn preview_page(&mut self, pane: u32, url: &str) {
        let Some(file) = file_of(url) else {
            self.previews.shown.remove(&pane);
            return;
        };
        let same = self
            .previews
            .shown
            .get(&pane)
            .is_some_and(|p| p.file == file);
        if !same {
            self.previews.shown.insert(pane, Preview::new(file));
        }
        self.send_preview(pane);
    }

    /// `pane`'s preview page loaded (again), empty: its file is sent to it.
    pub(crate) fn preview_ready(&mut self, pane: u32) {
        if let Some(preview) = self.previews.shown.get_mut(&pane) {
            preview.sent = None;
            preview.caret = None;
            preview.top = None;
        }
        self.send_preview(pane);
        self.sync_preview_marks();
    }

    /// `pane`'s tab closed.
    pub(crate) fn preview_closed(&mut self, pane: u32) {
        self.previews.shown.remove(&pane);
    }

    /// Send every preview whose file, theme or review changed again; the
    /// previews of panes gone are dropped.
    pub fn refresh_previews(&mut self) {
        let panes: Vec<u32> = self.previews.shown.keys().copied().collect();
        for pane in panes {
            if self.pane_place(pane).is_none() {
                self.previews.shown.remove(&pane);
                continue;
            }
            self.send_preview(pane);
        }
    }

    /// Send `pane`'s preview to its page, unless it was sent as it is.
    fn send_preview(&mut self, pane: u32) {
        let Some(preview) = self.previews.shown.get(&pane) else {
            return;
        };
        let file = preview.file.clone();
        let name = file.file_name().unwrap_or(file.as_os_str());
        let name = name.to_string_lossy().into_owned();
        let dark = self.theme() == Theme::Dark;
        let review = self.doc_at(&file).is_some();
        let mut shown = Shown::of(&file, &name, dark, review);
        shown.marks = self.review_marks(&file).map(|(_, m)| m).unwrap_or_default();
        let marks = marks_hash(&shown.marks);
        let hash = shown.hash();
        if preview.sent == Some(hash) {
            return;
        }
        let sent = self.preview_to_browser(pane, &shown);
        if let Some(preview) = self.previews.shown.get_mut(&pane) {
            // Not sent: tried again with the next read.
            preview.sent = sent.then_some(hash);
            // Not sent, it is all sent with the next read: not every frame.
            preview.marks = Some(marks);
        }
    }

    /// Tell every preview where its file's review has its caret, and send
    /// it again if the review's comments changed; one whose file isn't
    /// under review marks nothing.
    pub(crate) fn sync_preview_marks(&mut self) {
        let wanted: Vec<(u32, Option<usize>, u64, bool, bool)> = (self.previews.shown.iter())
            .map(|(&pane, p)| {
                let (caret, marks) = self.review_marks(&p.file).unwrap_or_default();
                let hash = marks_hash(&marks);
                (
                    pane,
                    caret,
                    hash,
                    p.caret != Some(caret),
                    p.marks != Some(hash),
                )
            })
            .collect();
        for (pane, caret, _, caret_moved, marks_changed) in wanted {
            if marks_changed {
                self.send_preview(pane);
            }
            if caret_moved
                && self.caret_to_browser(pane, caret)
                && let Some(preview) = self.previews.shown.get_mut(&pane)
            {
                preview.caret = Some(caret);
            }
        }
    }

    /// The feature stage whose file `file` is: its project, slug and stage.
    pub(crate) fn doc_at(&self, file: &Path) -> Option<(String, String, Stage)> {
        let name = file.file_name()?.to_str()?;
        let stage = Stage::ALL
            .into_iter()
            .find(|&stage| artifacts::name(stage) == Some(name))?;
        let dir = file.parent()?;
        self.features.all.iter().find_map(|(project, features)| {
            let feature = features
                .iter()
                .find(|f| store::dir(None, project, &f.slug) == dir)?;
            Some((project.clone(), feature.slug.clone(), stage))
        })
    }

    /// The page of `pane`'s preview asked to review its file.
    pub(crate) fn preview_review(&mut self, pane: u32) {
        let Some(file) = self.previews.shown.get(&pane).map(|p| p.file.clone()) else {
            return;
        };
        match self.doc_at(&file) {
            Some((project, slug, stage)) => self.open_doc_review(&project, &slug, stage),
            None => self.show_warning("That file isn't a feature's to review".into()),
        }
    }

    /// A link clicked in `pane`'s preview: a web page opens in a tab of
    /// the preview's workspace.
    pub(crate) fn preview_link(&mut self, pane: u32, url: &str) {
        if !self.previews.has(pane) {
            return;
        }
        if !crate::is_web_link(url) {
            return self.show_warning("The preview opens only web links".into());
        }
        let w = self
            .pane_place(pane)
            .map_or(self.active_workspace, |(w, _)| w);
        self.open_page(w, url.to_owned());
    }

    /// The workspace and tab `pane` is in, shown or on its way.
    fn pane_place(&self, pane: u32) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            let t = ws.tabs.iter().position(|tab| {
                tab.panes.iter().any(|p| p.id == pane)
                    || tab.expected.iter().any(|e| e.pane == pane)
            })?;
            Some((w, t))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_and_script_links_are_left_out() {
        let html = render(
            "<script>alert(1)</script>\n\n[x](javascript:alert(1)) <img src=x onerror=alert(1)>\n",
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("onerror"), "{html}");
    }

    #[test]
    fn tables_and_task_lists_render_with_their_lines() {
        let html =
            render("# Goal\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n- [x] done\n- [ ] not yet\n");
        assert!(html.contains("<table"), "{html}");
        assert!(html.contains("type=\"checkbox\""), "{html}");
        assert!(html.contains("data-sourcepos=\"1:1-1:6\""), "{html}");
        assert!(html.contains("data-sourcepos=\"3:1-5:9\""), "{html}");
        assert!(html.contains("id=\"goal\""), "{html}");
    }

    #[test]
    fn the_page_link_names_the_file() {
        let file = Path::new("/home/a b/.local/share/gerdr/features/p/f/brief.md");
        let url = page_url(file);
        assert!(url.starts_with(&format!(
            "chrome-extension://{EXTENSION_ID}/preview.html?file="
        )));
        assert!(!url.contains(' '));
        assert_eq!(file_of(&url).as_deref(), Some(file));
        assert_eq!(file_of(&format!("{url}#goal")).as_deref(), Some(file));
        assert_eq!(file_of("https://example.com/preview.html?file=%2Fa"), None);
        let relative = format!("chrome-extension://{EXTENSION_ID}/preview.html?file=a.md");
        assert_eq!(file_of(&relative), None);
    }

    #[test]
    fn a_file_too_big_or_gone_says_so() {
        let dir = std::env::temp_dir().join(format!("gerdr-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.md");
        std::fs::write(&big, "a".repeat(FILE_MAX as usize + 1)).unwrap();
        let gone = dir.join("gone.md");
        let shown = Shown::of(&big, "big.md", true, false);
        assert_eq!(shown.body, Err("big.md is too big to preview".into()));
        let shown = Shown::of(&gone, "gone.md", true, false);
        assert_eq!(shown.body, Err("gone.md is gone".into()));
        std::fs::write(&gone, "# Hi\n").unwrap();
        let shown = Shown::of(&gone, "gone.md", false, true);
        assert_eq!(shown.title, "Preview \u{b7} gone.md");
        assert!(shown.body.unwrap().contains("<h1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_largest_file_fits_one_message() {
        // Every byte an escape in JSON, as `<` is in HTML: the worst case.
        let text = "<".repeat(FILE_MAX as usize);
        let html = render(&text);
        let shown = |dark, review, marks| Shown {
            title: "P".into(),
            body: Ok(String::new()),
            dark,
            review,
            marks,
        };
        let dark = shown(true, false, Vec::new());
        assert_eq!(web_control::preview_message(1, &dark, Ok(&html)), None);
        let why = web_control::preview_message(1, &dark, Err("big.md is too big"));
        assert!(why.is_some_and(|m| m.len() <= web_control::MESSAGE_MAX));
        let marked = shown(false, true, vec![(3, "Say why".into())]);
        let fits = web_control::preview_message(1, &marked, Ok("<p>hi</p>")).unwrap();
        let fits: serde_json::Value = serde_json::from_slice(&fits).unwrap();
        assert_eq!(fits["type"], "preview");
        assert_eq!(fits["html"], "<p>hi</p>");
        assert_eq!(fits["review"], true);
        assert_eq!(fits["marks"], serde_json::json!([[3, "Say why"]]));
        assert!(fits.get("error").is_none());
    }
}
