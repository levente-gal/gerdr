//! A markdown file as plain blocks of text, for gerdr to paint itself: the
//! read-only copy of a stage's file on its page (see `feature::page`). The
//! file is parsed as `md_preview` parses it, by comrak with GitHub's
//! extensions (and its alerts), and raw HTML is left out as there: a block
//! of it keeps only its words, `<br>` its line break. A file's front matter
//! is left out. What is inside a block is its words, with runs of them
//! bold, italic, struck through, code or a link (`Inline`); an image alone
//! in its paragraph is a block of its own, one among words its alt text.
//! Each block knows the lines of its file it comes from (`lined`), so a
//! file's page beside its review follows it (see `doc_page`).

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{
    highlight::{self, GRAMMAR_LINES},
    syntax::Spans,
};
use comrak::{
    Arena, Options,
    nodes::{AlertType, AstNode, ListType, NodeValue, TableAlignment},
    parse_document,
};

/// What a list item starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemMark {
    Bullet,
    Number(usize),
    /// A task, `true` = ticked.
    Task(bool),
    /// None: a further paragraph of the item above.
    Under,
}

/// How a run of a block's words is set, other than plain. A run is set
/// one way: where they nest, the later one here wins (`Inline::over`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Italic,
    Bold,
    /// Struck through: what a plan or a list says is no longer so.
    Strike,
    /// Inline code, in the monospace font.
    Code,
    /// A link to a web page (`http` or `https`) or a file (a path, maybe
    /// with `#L12` or `:12`; see `file_target`). Other links (`mailto:`,
    /// a heading's `#anchor`) stay plain.
    Link(String),
}

impl Inline {
    fn rank(&self) -> u8 {
        match self {
            Inline::Italic => 0,
            Inline::Bold => 1,
            Inline::Strike => 2,
            Inline::Code => 3,
            Inline::Link(_) => 4,
        }
    }

    /// How words set as `self` inside words set as `on` are set.
    fn over<'a>(&'a self, on: Option<&'a Inline>) -> Option<&'a Inline> {
        match on {
            Some(on) if on.rank() > self.rank() => Some(on),
            _ => Some(self),
        }
    }
}

/// How a table's column sets its cells' words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
}

/// What a GitHub alert (`> [!NOTE]`) says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    /// The word heading it.
    pub fn title(self) -> &'static str {
        match self {
            Alert::Note => "Note",
            Alert::Tip => "Tip",
            Alert::Important => "Important",
            Alert::Warning => "Warning",
            Alert::Caution => "Caution",
        }
    }
}

/// A block's words, and the runs of them set other than plain: by byte,
/// in order, apart, none empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Styled {
    pub text: String,
    pub runs: Vec<(Range<usize>, Inline)>,
}

impl From<&str> for Styled {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl From<String> for Styled {
    fn from(text: String) -> Self {
        Self {
            text,
            runs: Vec::new(),
        }
    }
}

impl Styled {
    /// `text` onto the end, set as `on`; a run going on grows.
    pub fn push(&mut self, text: &str, on: Option<&Inline>) {
        let start = self.text.len();
        self.text.push_str(text);
        let Some(on) = on.filter(|_| !text.is_empty()) else {
            return;
        };
        match self.runs.last_mut() {
            Some((run, was)) if run.end == start && was == on => run.end = self.text.len(),
            _ => self.runs.push((start..self.text.len(), on.clone())),
        }
    }

    /// `other` onto the end, its runs with it.
    pub fn append(&mut self, other: &Styled) {
        for (range, on) in spans(&other.text, &other.runs) {
            self.push(&other.text[range], on);
        }
    }

    /// Bytes `range` of it, its runs cut to them.
    pub fn slice(&self, range: Range<usize>) -> Styled {
        let mut out = Styled::default();
        for (r, on) in spans(&self.text, &self.runs) {
            let (start, end) = (r.start.max(range.start), r.end.min(range.end));
            if start < end {
                out.push(&self.text[start..end], on);
            }
        }
        out
    }

    /// How its byte `at` is set.
    pub fn at(&self, at: usize) -> Option<&Inline> {
        (self.runs.iter())
            .find(|(range, _)| range.contains(&at))
            .map(|(_, on)| on)
    }
}

/// `text` cut where `runs` start and end, each part with how it is set:
/// all of it, in order. Runs past its end (`text` a line's start) are cut
/// to it.
pub fn spans<'a>(
    text: &str,
    runs: &'a [(Range<usize>, Inline)],
) -> impl Iterator<Item = (Range<usize>, Option<&'a Inline>)> + use<'a> {
    let (mut at, len) = (0, text.len());
    let mut runs = runs.iter().peekable();
    std::iter::from_fn(move || {
        if at >= len {
            return None;
        }
        let part = match runs.peek() {
            Some((range, on)) if range.start <= at => {
                runs.next();
                (at..range.end.min(len), Some(on))
            }
            Some((range, _)) => (at..range.start.min(len), None),
            None => (at..len, None),
        };
        at = part.0.end;
        Some(part)
    })
}

/// A block of a markdown file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocBlock {
    Heading {
        level: u8,
        text: Styled,
    },
    Para(Styled),
    /// A list item, `depth` lists in (0 = the outermost).
    Item {
        depth: u8,
        mark: ItemMark,
        text: Styled,
    },
    /// A code block's lines, and their colours for the language its fence
    /// names (one `Spans` a line; none when it names none gerdr knows).
    /// `lang` is what the fence names, empty for none; `None` for a block
    /// that isn't a file's (a tool's output), which has no head to copy it.
    Code {
        lang: Option<String>,
        lines: Vec<String>,
        syntax: Vec<Spans>,
    },
    /// A quote's blocks; a GitHub alert's, headed by its kind's word, or
    /// the title it gives.
    Quote {
        alert: Option<(Alert, String)>,
        blocks: Vec<DocBlock>,
    },
    /// A table's rows of cells, the first its header, and how each column
    /// sets its words.
    Table {
        align: Vec<Align>,
        rows: Vec<Vec<Styled>>,
    },
    /// Blocks other than paragraphs and lists in a list item (code, a
    /// table, a quote), indented to its words, `depth` lists in.
    Nested {
        depth: u8,
        blocks: Vec<DocBlock>,
    },
    /// An image alone in its paragraph: where it is (a web page's, a
    /// file's; see `blocks_in`), and its alt text.
    Image {
        url: String,
        alt: String,
    },
    Rule,
    /// What gerdr says in the file's place: it isn't the file's own text.
    Note(String),
}

/// A code block of `lines` in language `lang` (a fence's `rust`, `py`, `sh`;
/// see `DocBlock::Code`): coloured by `highlight`, plain for a language it
/// doesn't know or none.
pub fn code_block(lang: Option<&str>, lines: Vec<String>) -> DocBlock {
    let syntax = match lang {
        None | Some("") => None,
        Some(lang) => highlight::lines(
            lang,
            lines.iter().map(String::as_str),
            &mut GRAMMAR_LINES.clone(),
        ),
    };
    DocBlock::Code {
        lang: lang.map(str::to_owned),
        lines,
        syntax: syntax.unwrap_or_default(),
    }
}

/// `text`'s blocks, in order.
pub fn blocks(text: &str) -> Vec<DocBlock> {
    lined(text).0
}

/// The lines of its file each of a file's blocks comes from, first and
/// last, counted from 1 (as `blocks` gives them, one each).
pub type Lines = Vec<(usize, usize)>;

/// `text`'s blocks, in order, and the lines each comes from.
pub fn lined(text: &str) -> (Vec<DocBlock>, Lines) {
    let mut options = Options::default();
    let ext = &mut options.extension;
    ext.table = true;
    ext.strikethrough = true;
    ext.tasklist = true;
    ext.autolink = true;
    ext.footnotes = true;
    ext.alerts = true;
    ext.front_matter_delimiter = Some("---".into());
    let arena = Arena::new();
    let root = parse_document(&arena, text, &options);
    hard_item_breaks(root);
    let (mut out, mut lines) = (Vec::new(), Vec::new());
    for node in root.children() {
        block(node, 0, &mut out, &mut lines);
    }
    (out, lines)
}

/// The blocks put on `out` since `lines` was last filled come from `node`'s
/// lines.
fn fill<'a>(out: &[DocBlock], lines: &mut Lines, node: &'a AstNode<'a>) {
    let pos = node.data.borrow().sourcepos;
    let span = (pos.start.line, pos.end.line.max(pos.start.line));
    lines.resize(out.len(), span);
}

/// Make each line break inside a list item a hard one, so lines like a plan
/// milestone's `Files:`, `Covers:` and `Check:` under its `- [ ] Name` keep
/// their own lines rather than running on into one. Elsewhere, a line that
/// starts with such a label keeps its own line too, as a decision's
/// `Decided:`, `Why:` and `User said:` do. `md_preview` does the same.
pub fn hard_item_breaks<'a>(root: &'a AstNode<'a>) {
    for node in root.descendants() {
        let soft = matches!(node.data.borrow().value, NodeValue::SoftBreak);
        let in_item = (node.ancestors()).any(|a| {
            matches!(
                a.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            )
        });
        if soft && (in_item || node.next_sibling().is_some_and(starts_labelled)) {
            node.data.borrow_mut().value = NodeValue::LineBreak;
        }
    }
}

/// Whether inline `node` starts with a label and a colon (`Why:`,
/// `**User said:**`): a few words, the first capitalised.
fn starts_labelled<'a>(node: &'a AstNode<'a>) -> bool {
    let first = node
        .descendants()
        .find_map(|n| match &n.data.borrow().value {
            NodeValue::Text(text) => Some(text.to_string()),
            _ => None,
        });
    let Some(text) = first else {
        return false;
    };
    let Some((label, _)) = text.split_once(':') else {
        return false;
    };
    let worded = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'');
    label.len() <= 24
        && label.chars().next().is_some_and(char::is_uppercase)
        && label.chars().all(worded)
}

/// `node`'s blocks onto `out`, `depth` lists in, and their lines onto
/// `lines`.
fn block<'a>(node: &'a AstNode<'a>, depth: u8, out: &mut Vec<DocBlock>, lines: &mut Lines) {
    put(node, depth, out, lines);
    fill(out, lines, node);
}

/// `block`'s blocks, their lines but those of a list's items left to fill.
fn put<'a>(node: &'a AstNode<'a>, depth: u8, out: &mut Vec<DocBlock>, lines: &mut Lines) {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Heading(heading) => out.push(DocBlock::Heading {
            level: heading.level,
            text: inline(node),
        }),
        NodeValue::Paragraph => match images(node) {
            Some(images) => out.extend(images),
            None => out.push(DocBlock::Para(inline(node))),
        },
        NodeValue::List(list) => {
            for (i, item) in node.children().enumerate() {
                let value = item.data.borrow().value.clone();
                let mark = match value {
                    NodeValue::TaskItem(task) => ItemMark::Task(task.symbol.is_some()),
                    _ if list.list_type == ListType::Ordered => ItemMark::Number(list.start + i),
                    _ => ItemMark::Bullet,
                };
                list_item(item, depth, mark, out, lines);
            }
        }
        NodeValue::CodeBlock(code) => {
            let lines = code.literal.lines().map(str::to_owned).collect();
            let lang = code.info.split_whitespace().next().unwrap_or("");
            out.push(code_block(Some(lang), lines));
        }
        NodeValue::BlockQuote => out.push(DocBlock::Quote {
            alert: None,
            blocks: inner(node),
        }),
        NodeValue::Alert(alert) => {
            let kind = match alert.alert_type {
                AlertType::Note => Alert::Note,
                AlertType::Tip => Alert::Tip,
                AlertType::Important => Alert::Important,
                AlertType::Warning => Alert::Warning,
                AlertType::Caution => Alert::Caution,
            };
            let title = (alert.title.as_deref().map(str::trim))
                .filter(|t| !t.is_empty())
                .unwrap_or(kind.title());
            out.push(DocBlock::Quote {
                alert: Some((kind, title.to_owned())),
                blocks: inner(node),
            });
        }
        NodeValue::Table(table) => {
            let align = (table.alignments.iter())
                .map(|a| match a {
                    TableAlignment::Center => Align::Centre,
                    TableAlignment::Right => Align::Right,
                    _ => Align::Left,
                })
                .collect();
            let rows = node
                .children()
                .map(|row| row.children().map(inline).collect());
            out.push(DocBlock::Table {
                align,
                rows: rows.collect(),
            });
        }
        NodeValue::ThematicBreak => out.push(DocBlock::Rule),
        NodeValue::FootnoteDefinition(note) => {
            let mut text = Styled::from(format!("[{}] ", note.name));
            text.append(&joined(
                &node.children().map(inline).collect::<Vec<_>>(),
                " ",
            ));
            out.push(DocBlock::Para(text));
        }
        NodeValue::HtmlBlock(html) => {
            let words = html_words(&html.literal);
            if !words.is_empty() {
                out.push(DocBlock::Para(words.into()));
            }
        }
        _ => {}
    }
}

/// The blocks inside container `node` (a quote's).
fn inner<'a>(node: &'a AstNode<'a>) -> Vec<DocBlock> {
    let (mut blocks, mut lines) = (Vec::new(), Vec::new());
    for child in node.children() {
        block(child, 0, &mut blocks, &mut lines);
    }
    blocks
}

/// Paragraph `node`'s images as blocks of their own, when it has only
/// images (and the space between them).
fn images<'a>(node: &'a AstNode<'a>) -> Option<Vec<DocBlock>> {
    let mut images = Vec::new();
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Image(to) => images.push(DocBlock::Image {
                url: to.url.clone(),
                alt: inline(child).text,
            }),
            NodeValue::Text(t) if t.trim().is_empty() => {}
            NodeValue::SoftBreak | NodeValue::LineBreak => {}
            _ => return None,
        }
    }
    (!images.is_empty()).then_some(images)
}

/// The words a block of raw HTML shows (a `<summary>`'s, a `<p>`'s), its
/// tags, comments, scripts and styles left out; a line of them each line.
fn html_words(html: &str) -> String {
    let mut rest = html;
    let mut text = String::new();
    while let Some(at) = rest.find('<') {
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        let lower = rest.to_ascii_lowercase();
        let end = if lower.starts_with("<!--") {
            lower.find("-->").map(|e| e + 3)
        } else if let Some(tag) = ["<script", "<style"]
            .iter()
            .find(|t| lower.starts_with(**t))
        {
            let close = format!("</{}", &tag[1..]);
            (lower.find(&close)).and_then(|e| lower[e..].find('>').map(|g| e + g + 1))
        } else {
            lower.find('>').map(|e| e + 1)
        };
        let Some(end) = end else {
            rest = "";
            break;
        };
        if lower.starts_with("<br") {
            text.push('\n');
        }
        rest = &rest[end..];
    }
    text.push_str(rest);
    let text = (text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">"))
    .replace("&quot;", "\"")
    .replace("&amp;", "&");
    let lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.collect::<Vec<_>>().join("\n")
}

/// List item `item`'s blocks onto `out`: its first paragraph after `mark`,
/// the rest under it.
fn list_item<'a>(
    item: &'a AstNode<'a>,
    depth: u8,
    mark: ItemMark,
    out: &mut Vec<DocBlock>,
    lines: &mut Lines,
) {
    let mut mark = Some(mark);
    for child in item.children() {
        let value = child.data.borrow().value.clone();
        if matches!(value, NodeValue::Paragraph) && images(child).is_none() {
            let text = inline(child);
            let mark = mark.take().unwrap_or(ItemMark::Under);
            out.push(DocBlock::Item { depth, mark, text });
            fill(out, lines, child);
            continue;
        }
        // A list or a block opening the item leaves its mark on an empty
        // line.
        if let Some(mark) = mark.take() {
            let text = Styled::default();
            out.push(DocBlock::Item { depth, mark, text });
            fill(out, lines, child);
        }
        if matches!(value, NodeValue::List(_)) {
            block(child, depth.saturating_add(1), out, lines);
        } else {
            let (mut blocks, mut inner) = (Vec::new(), Vec::new());
            block(child, 0, &mut blocks, &mut inner);
            if !blocks.is_empty() {
                out.push(DocBlock::Nested { depth, blocks });
                fill(out, lines, child);
            }
        }
    }
    if let Some(mark) = mark {
        let text = Styled::default();
        out.push(DocBlock::Item { depth, mark, text });
        fill(out, lines, item);
    }
}

/// `parts` one after another, `gap` between them.
fn joined(parts: &[Styled], gap: &str) -> Styled {
    let mut out = Styled::default();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(gap, None);
        }
        out.append(part);
    }
    out
}

/// The words in `node`, with their runs, the space around them trimmed.
fn inline<'a>(node: &'a AstNode<'a>) -> Styled {
    let mut text = Styled::default();
    words(node, &mut text, None);
    let start = text.text.len() - text.text.trim_start().len();
    let end = text.text.trim_end().len().max(start);
    text.slice(start..end)
}

/// `inline` onto `text`, set as `on`; nested runs as `Inline::over` says.
/// An image among words is its alt text, as a link to it.
fn words<'a>(node: &'a AstNode<'a>, text: &mut Styled, on: Option<&Inline>) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => text.push(t, on),
            NodeValue::Code(code) => text.push(&code.literal, Inline::Code.over(on)),
            NodeValue::SoftBreak => text.push(" ", on),
            // The page breaks its line there (`doc_layout`).
            NodeValue::LineBreak => text.push("\n", None),
            NodeValue::FootnoteReference(note) => {
                text.push(&format!("[{}]", note.name), on);
            }
            NodeValue::HtmlInline(html) => {
                if html
                    .trim_start_matches('<')
                    .to_ascii_lowercase()
                    .starts_with("br")
                {
                    text.push("\n", None);
                }
            }
            NodeValue::Raw(_) => {}
            NodeValue::Emph => words(child, text, Inline::Italic.over(on)),
            NodeValue::Strong => words(child, text, Inline::Bold.over(on)),
            NodeValue::Strikethrough => words(child, text, Inline::Strike.over(on)),
            NodeValue::Link(to) | NodeValue::Image(to) if linkable(&to.url) => {
                words(child, text, Some(&Inline::Link(to.url.to_string())));
            }
            _ => words(child, text, on),
        }
    }
}

/// Whether a link to `url` is one: a web page's or a file's.
fn linkable(url: &str) -> bool {
    crate::is_web_link(url) || file_link(url).is_some()
}

/// Where link `url` points as a file's: its path, and what follows its `#`.
/// `None` for a web page's, another scheme's (`mailto:`), or a heading's
/// (`#anchor`).
fn file_link(url: &str) -> Option<(&str, &str)> {
    let url = url.trim();
    let url = url.strip_prefix("file://").unwrap_or(url);
    // `mailto:`, not `foo.rs:12`'s line.
    let scheme = url.split_once(':').is_some_and(|(s, rest)| {
        s.len() > 1
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-".contains(c))
            && !rest.starts_with(|c: char| c.is_ascii_digit())
    });
    if url.is_empty() || url.starts_with('#') || url.contains("://") || scheme {
        return None;
    }
    Some(url.split_once('#').unwrap_or((url, "")))
}

/// The file link `url` opens, and its line: `#L12` (GitHub's) or `:12`
/// after it, relative to `base` unless absolute. `None` for no file's.
pub fn file_target(url: &str, base: Option<&Path>) -> Option<(PathBuf, Option<u32>)> {
    let (path, fragment) = file_link(url)?;
    let line = |s: &str| s.parse::<u32>().ok().filter(|n| *n > 0);
    let from_fragment =
        (fragment.strip_prefix('L')).and_then(|l| line(l.split(['-', 'C']).next().unwrap_or(l)));
    let (path, at) = match path.rsplit_once(':') {
        Some((p, n)) if line(n).is_some() => (p, line(n)),
        _ => (path, None),
    };
    let path = PathBuf::from(unescaped(path));
    let path = match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path,
    };
    Some((path, from_fragment.or(at)))
}

/// `text` with its `%XX` escapes (a link's `%20`) as the bytes they are.
fn unescaped(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `text`'s blocks, as `blocks`, for a file in folder `dir`: its links and
/// images to relative paths made absolute in it, so they open (and show)
/// wherever the blocks go.
pub fn blocks_in(text: &str, dir: &Path) -> Vec<DocBlock> {
    lined_in(text, dir).0
}

/// `blocks_in`, with the lines each block comes from (see `lined`).
pub fn lined_in(text: &str, dir: &Path) -> (Vec<DocBlock>, Lines) {
    let (mut blocks, lines) = lined(text);
    rebase(&mut blocks, dir);
    (blocks, lines)
}

fn rebase(blocks: &mut [DocBlock], dir: &Path) {
    let to = |url: &mut String| {
        if let Some((path, fragment)) = file_link(url)
            && Path::new(path).is_relative()
        {
            let path = dir.join(unescaped(path)).to_string_lossy().into_owned();
            *url = match fragment {
                "" => path,
                f => format!("{path}#{f}"),
            };
        }
    };
    let runs = |text: &mut Styled| {
        for (_, on) in &mut text.runs {
            if let Inline::Link(url) = on {
                to(url);
            }
        }
    };
    for block in blocks {
        match block {
            DocBlock::Heading { text, .. } | DocBlock::Para(text) | DocBlock::Item { text, .. } => {
                runs(text)
            }
            DocBlock::Quote { blocks, .. } | DocBlock::Nested { blocks, .. } => rebase(blocks, dir),
            DocBlock::Table { rows, .. } => rows.iter_mut().flatten().for_each(runs),
            DocBlock::Image { url, .. } => to(url),
            DocBlock::Code { .. } | DocBlock::Rule | DocBlock::Note(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> DocBlock {
        DocBlock::Para(text.into())
    }

    #[test]
    fn each_block_knows_the_lines_it_comes_from() {
        let md = "---\ntitle: x\n---\n# Head\n\nOne\ntwo\n\n- a\n- b\n  - c\n\n  ```\n  ls\n  ```\n\n> q\n> r\n\n| h |\n|---|\n| d |\n";
        let (blocks, lines) = lined(md);
        assert_eq!(blocks.len(), lines.len());
        assert_eq!(
            lines,
            [
                (4, 4),
                (6, 7),
                (9, 9),
                (10, 10),
                (11, 11),
                (13, 15),
                (17, 18),
                (20, 22)
            ]
        );
        assert!(matches!(blocks[5], DocBlock::Nested { .. }));
    }

    /// `text` with `runs`, each the bytes of its first `&str` found in it.
    fn styled(text: &str, runs: &[(&str, Inline)]) -> Styled {
        let runs = (runs.iter())
            .map(|(part, on)| {
                let at = text.find(part).expect("in the text");
                (at..at + part.len(), on.clone())
            })
            .collect();
        Styled {
            text: text.to_owned(),
            runs,
        }
    }

    fn item(depth: u8, mark: ItemMark, text: &str) -> DocBlock {
        DocBlock::Item {
            depth,
            mark,
            text: text.into(),
        }
    }

    #[test]
    fn headings_and_paragraphs_keep_their_order() {
        assert_eq!(
            blocks("# One\n\nFirst\nline.\n\n## Two\n\nNext."),
            [
                DocBlock::Heading {
                    level: 1,
                    text: "One".into()
                },
                para("First line."),
                DocBlock::Heading {
                    level: 2,
                    text: "Two".into()
                },
                para("Next."),
            ]
        );
    }

    #[test]
    fn a_line_starting_with_a_label_keeps_its_own_line() {
        let log = "## D1: Format\nDecided: CSV.\nWhy: the tool reads CSV.\n**User said:** \"CSV only\"\n\nWrapped prose\ngoes on: here.\n";
        let said = "Decided: CSV.\nWhy: the tool reads CSV.\nUser said: \"CSV only\"";
        assert_eq!(
            blocks(log)[1..],
            [
                DocBlock::Para(styled(said, &[("User said:", Inline::Bold)])),
                para("Wrapped prose goes on: here."),
            ]
        );
    }

    #[test]
    fn inline_runs_keep_how_they_are_set() {
        let link = || Inline::Link("https://x.y".into());
        assert_eq!(
            blocks("A **bold** _word_ `code` [a link](https://x.y) ![alt](i.png) ~~gone~~."),
            [DocBlock::Para(styled(
                "A bold word code a link alt gone.",
                &[
                    ("bold", Inline::Bold),
                    ("word", Inline::Italic),
                    ("code", Inline::Code),
                    ("a link", link()),
                    ("alt", Inline::Link("i.png".into())),
                    ("gone", Inline::Strike),
                ]
            ))]
        );
        // A link sets all of it; code beats bold, bold italic; runs meet.
        assert_eq!(
            blocks("[**go** `x`](https://x.y) **b `c` d** *e **f***"),
            [DocBlock::Para(styled(
                "go x b c d e f",
                &[
                    ("go x", link()),
                    ("b ", Inline::Bold),
                    ("c", Inline::Code),
                    (" d", Inline::Bold),
                    ("e ", Inline::Italic),
                    ("f", Inline::Bold),
                ]
            ))]
        );
    }

    #[test]
    fn web_and_file_links_are_links_others_not() {
        let page = "See [mail](mailto:a@b.c), [file](file:///etc), [rel](docs/x.md), \
                    [line](src/a.rs:12), [top](#top), <https://y.z/q> and [js](javascript:alert(1)).";
        let Some(DocBlock::Para(text)) = blocks(page).into_iter().next() else {
            panic!("a paragraph");
        };
        assert_eq!(
            text.text,
            "See mail, file, rel, line, top, https://y.z/q and js."
        );
        let link = |url: &str| Inline::Link(url.into());
        let want = [
            ("file", link("file:///etc")),
            ("rel", link("docs/x.md")),
            ("line", link("src/a.rs:12")),
            ("https://y.z/q", link("https://y.z/q")),
        ];
        assert_eq!(text.runs, styled(&text.text, &want).runs);
    }

    #[test]
    fn a_file_link_opens_its_file_at_its_line() {
        let base = Path::new("/r");
        let at = |url| file_target(url, Some(base));
        assert_eq!(at("docs/x.md"), Some(("/r/docs/x.md".into(), None)));
        assert_eq!(at("src/a.rs#L12"), Some(("/r/src/a.rs".into(), Some(12))));
        assert_eq!(at("src/a.rs#L3-L9"), Some(("/r/src/a.rs".into(), Some(3))));
        assert_eq!(at("a.rs:40"), Some(("/r/a.rs".into(), Some(40))));
        assert_eq!(at("x.md#usage"), Some(("/r/x.md".into(), None)));
        assert_eq!(at("my%20notes.md"), Some(("/r/my notes.md".into(), None)));
        assert_eq!(at("file:///etc/hosts"), Some(("/etc/hosts".into(), None)));
        for not in ["https://x.y", "mailto:a@b.c", "#top", "javascript:x()", ""] {
            assert_eq!(at(not), None, "{not}");
        }
    }

    #[test]
    fn a_files_relative_links_and_images_are_made_absolute_in_its_folder() {
        let md =
            "[a](b.md#L2) [w](https://x.y) [abs](/etc/x)\n\n![shot](img/s.png)\n\n> [c](c.md)\n";
        let got = blocks_in(md, Path::new("/d"));
        let DocBlock::Para(text) = &got[0] else {
            panic!("{got:?}")
        };
        let urls: Vec<_> = (text.runs.iter())
            .map(|(_, on)| match on {
                Inline::Link(url) => url.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(urls, ["/d/b.md#L2", "https://x.y", "/etc/x"]);
        assert_eq!(
            got[1],
            DocBlock::Image {
                url: "/d/img/s.png".into(),
                alt: "shot".into()
            }
        );
        let DocBlock::Quote { blocks, .. } = &got[2] else {
            panic!("{got:?}")
        };
        assert_eq!(
            blocks[0],
            DocBlock::Para(styled("c", &[("c", Inline::Link("/d/c.md".into()))]))
        );
    }

    #[test]
    fn a_table_keeps_its_cells_runs_and_alignment() {
        assert_eq!(
            blocks("| a | b | c |\n|---|:-:|--:|\n| 1 | **2** | 3<br>4 |\n"),
            [DocBlock::Table {
                align: vec![Align::Left, Align::Centre, Align::Right],
                rows: vec![
                    vec!["a".into(), "b".into(), "c".into()],
                    vec![
                        "1".into(),
                        styled("2", &[("2", Inline::Bold)]),
                        "3\n4".into()
                    ],
                ],
            }]
        );
    }

    #[test]
    fn spans_cover_all_of_the_text_in_order() {
        let text = styled("ab cd ef", &[("cd", Inline::Bold)]);
        let parts: Vec<_> = spans(&text.text, &text.runs).collect();
        assert_eq!(
            parts,
            [(0..3, None), (3..5, Some(&Inline::Bold)), (5..8, None)]
        );
        assert_eq!(text.slice(4..7), styled("d e", &[("d", Inline::Bold)]));
        assert_eq!(spans("", &[]).count(), 0);
    }

    #[test]
    fn lists_nest_and_number_from_their_start() {
        assert_eq!(
            blocks("- a\n  - b\n\n    more\n- c\n\n3. d\n4. e\n"),
            [
                item(0, ItemMark::Bullet, "a"),
                item(1, ItemMark::Bullet, "b"),
                item(1, ItemMark::Under, "more"),
                item(0, ItemMark::Bullet, "c"),
                item(0, ItemMark::Number(3), "d"),
                item(0, ItemMark::Number(4), "e"),
            ]
        );
    }

    #[test]
    fn lines_in_an_item_keep_their_own_lines() {
        assert_eq!(
            blocks("- [ ] M1\n  Files: a.py\n  Check: tests\n\nOne\nline.  \nTwo.\n"),
            [
                item(0, ItemMark::Task(false), "M1\nFiles: a.py\nCheck: tests"),
                para("One line.\nTwo."),
            ]
        );
    }

    #[test]
    fn tasks_say_whether_they_are_ticked() {
        assert_eq!(
            blocks("- [ ] open\n- [x] done\n"),
            [
                item(0, ItemMark::Task(false), "open"),
                item(0, ItemMark::Task(true), "done"),
            ]
        );
    }

    #[test]
    fn code_keeps_its_lines_as_they_are() {
        assert_eq!(
            blocks("```\nlet a = 1;\n\n    b();\n```\n"),
            [DocBlock::Code {
                lang: Some(String::new()),
                lines: vec!["let a = 1;".into(), String::new(), "    b();".into()],
                syntax: Vec::new(),
            }]
        );
    }

    /// A fence's language colours its lines; one gerdr doesn't know, or
    /// none, leaves them plain.
    #[test]
    fn a_fences_language_colours_its_code() {
        let syntax = |md: &str| match blocks(md).pop() {
            Some(DocBlock::Code { syntax, .. }) => syntax,
            other => panic!("a code block, not {other:?}"),
        };
        let rust = syntax("```rust\nfn main() {}\n```\n");
        assert_eq!(rust.len(), 1);
        assert!(
            rust[0].contains(&(0..2, crate::syntax::Kind::Keyword)),
            "{rust:?}"
        );
        assert!(syntax("```\nfn main() {}\n```\n").is_empty());
        assert!(syntax("```nolang\nfn main() {}\n```\n").is_empty());
    }

    #[test]
    fn quotes_rules_and_footnotes_come_through() {
        assert_eq!(
            blocks("> said\n\n---\n\nSee[^n].\n\n[^n]: there\n"),
            [
                DocBlock::Quote {
                    alert: None,
                    blocks: vec![para("said")],
                },
                DocBlock::Rule,
                para("See[n]."),
                para("[n] there"),
            ]
        );
    }

    /// A quote keeps the blocks in it: code, a list, a quote.
    #[test]
    fn a_quote_keeps_its_blocks() {
        let quote = |blocks| DocBlock::Quote {
            alert: None,
            blocks,
        };
        assert_eq!(
            blocks("> Run:\n>\n> ```\n> ls\n> ```\n>\n> - one\n>\n> > deeper\n"),
            [quote(vec![
                para("Run:"),
                code_block(Some(""), vec!["ls".into()]),
                item(0, ItemMark::Bullet, "one"),
                quote(vec![para("deeper")]),
            ])]
        );
    }

    #[test]
    fn an_alert_says_its_kind() {
        assert_eq!(
            blocks("> [!WARNING]\n> Mind the gap.\n\n> [!TIP] Shortcut\n> Go.\n"),
            [
                DocBlock::Quote {
                    alert: Some((Alert::Warning, "Warning".into())),
                    blocks: vec![para("Mind the gap.")],
                },
                DocBlock::Quote {
                    alert: Some((Alert::Tip, "Shortcut".into())),
                    blocks: vec![para("Go.")],
                },
            ]
        );
    }

    #[test]
    fn code_in_a_list_item_is_nested_under_it() {
        assert_eq!(
            blocks("1. Build:\n\n   ```sh\n   cargo build\n   ```\n2. Done\n"),
            [
                item(0, ItemMark::Number(1), "Build:"),
                DocBlock::Nested {
                    depth: 0,
                    blocks: vec![code_block(Some("sh"), vec!["cargo build".into()])],
                },
                item(0, ItemMark::Number(2), "Done"),
            ]
        );
    }

    #[test]
    fn an_image_alone_is_a_block_among_words_its_alt() {
        assert_eq!(
            blocks("![one](a.png) ![two](https://x.y/b.png)\n\nSee ![it](c.png).\n"),
            [
                DocBlock::Image {
                    url: "a.png".into(),
                    alt: "one".into()
                },
                DocBlock::Image {
                    url: "https://x.y/b.png".into(),
                    alt: "two".into()
                },
                DocBlock::Para(styled("See it.", &[("it", Inline::Link("c.png".into()))])),
            ]
        );
    }

    #[test]
    fn front_matter_is_left_out() {
        assert_eq!(
            blocks("---\nname: x\ndescription: y\n---\n\n# Title\n"),
            [DocBlock::Heading {
                level: 1,
                text: "Title".into()
            }]
        );
    }

    #[test]
    fn raw_html_is_left_out_but_its_words() {
        assert_eq!(
            blocks("<script>alert(1)</script>\n\nSafe <b>word</b>.\n"),
            [para("Safe word.")]
        );
        assert_eq!(
            blocks(
                "<details>\n<summary>More &amp; less</summary>\n<!-- hid -->\n\nInside.\n\n</details>\n"
            ),
            [para("More & less"), para("Inside.")]
        );
        assert_eq!(blocks("a<br>b<br/>c\n"), [para("a\nb\nc")]);
    }

    #[test]
    fn nothing_makes_no_blocks() {
        assert_eq!(blocks(""), []);
        assert_eq!(blocks("\n \n"), []);
    }
}
