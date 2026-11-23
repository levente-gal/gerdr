//! A markdown file as plain blocks of text, for gerdr to paint itself: the
//! read-only copy of a stage's file on its page (see `feature::page`). The
//! file is parsed as `md_preview` parses it, by comrak with GitHub's
//! extensions, and raw HTML is left out as there. What is inside a block
//! is its words, with runs of them bold, code or a web link (`Inline`);
//! the rest flattens: emphasis to plain words, an image to its alt text.
//! The page in a file's tab (`md_preview`) is still what shows it as GitHub
//! would.

use std::ops::Range;

use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue},
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

/// How a run of a block's words is set, other than plain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Bold,
    /// Inline code, in the monospace font.
    Code,
    /// A link to a web page (`http` or `https`): other links stay plain.
    Link(String),
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
    /// A code block's lines.
    Code(Vec<String>),
    Quote(Styled),
    /// A table's rows with their cells joined, the first its header.
    Table(Vec<Styled>),
    Rule,
    /// What gerdr says in the file's place: it isn't the file's own text.
    Note(String),
}

/// What joins a table's cells.
const CELL_GAP: &str = " \u{b7} ";

/// `text`'s blocks, in order.
pub fn blocks(text: &str) -> Vec<DocBlock> {
    let mut options = Options::default();
    let ext = &mut options.extension;
    ext.table = true;
    ext.strikethrough = true;
    ext.tasklist = true;
    ext.autolink = true;
    ext.footnotes = true;
    let arena = Arena::new();
    let root = parse_document(&arena, text, &options);
    hard_item_breaks(root);
    let mut out = Vec::new();
    for node in root.children() {
        block(node, 0, &mut out);
    }
    out
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

/// `node`'s blocks onto `out`, `depth` lists in.
fn block<'a>(node: &'a AstNode<'a>, depth: u8, out: &mut Vec<DocBlock>) {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Heading(heading) => out.push(DocBlock::Heading {
            level: heading.level,
            text: inline(node),
        }),
        NodeValue::Paragraph => out.push(DocBlock::Para(inline(node))),
        NodeValue::List(list) => {
            for (i, item) in node.children().enumerate() {
                let value = item.data.borrow().value.clone();
                let mark = match value {
                    NodeValue::TaskItem(task) => ItemMark::Task(task.symbol.is_some()),
                    _ if list.list_type == ListType::Ordered => ItemMark::Number(list.start + i),
                    _ => ItemMark::Bullet,
                };
                list_item(item, depth, mark, out);
            }
        }
        NodeValue::CodeBlock(code) => {
            let lines = code.literal.lines().map(str::to_owned).collect();
            out.push(DocBlock::Code(lines));
        }
        NodeValue::BlockQuote => {
            for child in node.children() {
                let text = inline(child);
                if !text.text.is_empty() {
                    out.push(DocBlock::Quote(text));
                }
            }
        }
        NodeValue::Table(_) => {
            let rows = node.children().map(|row| {
                let cells: Vec<Styled> = row.children().map(inline).collect();
                joined(&cells, CELL_GAP)
            });
            out.push(DocBlock::Table(rows.collect()));
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
        _ => {}
    }
}

/// List item `item`'s blocks onto `out`: its first paragraph after `mark`,
/// the rest under it.
fn list_item<'a>(item: &'a AstNode<'a>, depth: u8, mark: ItemMark, out: &mut Vec<DocBlock>) {
    let mut mark = Some(mark);
    for child in item.children() {
        if matches!(child.data.borrow().value, NodeValue::Paragraph) {
            let text = inline(child);
            let mark = mark.take().unwrap_or(ItemMark::Under);
            out.push(DocBlock::Item { depth, mark, text });
        } else {
            // A list opening the item leaves its mark on an empty line.
            if let Some(mark) = mark.take() {
                let text = Styled::default();
                out.push(DocBlock::Item { depth, mark, text });
            }
            block(child, depth.saturating_add(1), out);
        }
    }
    if let Some(mark) = mark {
        let text = Styled::default();
        out.push(DocBlock::Item { depth, mark, text });
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

/// `inline` onto `text`, set as `on`. A link sets all inside it, then
/// code, then bold.
fn words<'a>(node: &'a AstNode<'a>, text: &mut Styled, on: Option<&Inline>) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => text.push(t, on),
            NodeValue::Code(code) => {
                let on = match on {
                    Some(Inline::Link(_)) => on,
                    _ => Some(&Inline::Code),
                };
                text.push(&code.literal, on);
            }
            NodeValue::SoftBreak => text.push(" ", on),
            // The page breaks its line there (`doc_layout`).
            NodeValue::LineBreak => text.push("\n", None),
            NodeValue::FootnoteReference(note) => {
                text.push(&format!("[{}]", note.name), on);
            }
            NodeValue::HtmlInline(_) | NodeValue::Raw(_) => {}
            NodeValue::Strong => words(child, text, on.or(Some(&Inline::Bold))),
            NodeValue::Link(to) if crate::is_web_link(&to.url) => {
                words(child, text, Some(&Inline::Link(to.url.to_string())));
            }
            _ => words(child, text, on),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> DocBlock {
        DocBlock::Para(text.into())
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
    fn bold_code_and_web_links_keep_their_runs_the_rest_flattens() {
        let link = || Inline::Link("https://x.y".into());
        assert_eq!(
            blocks("A **bold** _word_ `code` [a link](https://x.y) ![alt](i.png) ~~gone~~."),
            [DocBlock::Para(styled(
                "A bold word code a link alt gone.",
                &[
                    ("bold", Inline::Bold),
                    ("code", Inline::Code),
                    ("a link", link()),
                ]
            ))]
        );
        // A link sets all of it; code beats bold; runs meet.
        assert_eq!(
            blocks("[**go** `x`](https://x.y) **b `c` d**"),
            [DocBlock::Para(styled(
                "go x b c d",
                &[
                    ("go x", link()),
                    ("b ", Inline::Bold),
                    ("c", Inline::Code),
                    (" d", Inline::Bold),
                ]
            ))]
        );
    }

    #[test]
    fn only_web_links_are_links() {
        let page = "See [mail](mailto:a@b.c), [file](file:///etc), [rel](docs/x.md), \
                    <https://y.z/q> and [js](javascript:alert(1)).";
        let Some(DocBlock::Para(text)) = blocks(page).into_iter().next() else {
            panic!("a paragraph");
        };
        assert_eq!(text.text, "See mail, file, rel, https://y.z/q and js.");
        assert_eq!(
            text.runs,
            styled(
                &text.text,
                &[("https://y.z/q", Inline::Link("https://y.z/q".into()))]
            )
            .runs
        );
    }

    #[test]
    fn a_table_keeps_its_cells_runs() {
        assert_eq!(
            blocks("| a | b |\n|---|---|\n| 1 | **2** |\n"),
            [DocBlock::Table(vec![
                "a \u{b7} b".into(),
                styled("1 \u{b7} 2", &[("2", Inline::Bold)]),
            ])]
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
            blocks("```rust\nlet a = 1;\n\n    b();\n```\n"),
            [DocBlock::Code(vec![
                "let a = 1;".into(),
                String::new(),
                "    b();".into(),
            ])]
        );
    }

    #[test]
    fn quotes_rules_and_footnotes_come_through() {
        assert_eq!(
            blocks("> said\n\n---\n\nSee[^n].\n\n[^n]: there\n"),
            [
                DocBlock::Quote("said".into()),
                DocBlock::Rule,
                para("See[n]."),
                para("[n] there"),
            ]
        );
    }

    #[test]
    fn raw_html_is_left_out() {
        assert_eq!(
            blocks("<script>alert(1)</script>\n\nSafe <b>word</b>.\n"),
            [para("Safe word.")]
        );
    }

    #[test]
    fn nothing_makes_no_blocks() {
        assert_eq!(blocks(""), []);
        assert_eq!(blocks("\n \n"), []);
    }
}
