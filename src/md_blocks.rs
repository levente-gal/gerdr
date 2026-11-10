//! A markdown file as plain blocks of text, for gerdr to paint itself: the
//! read-only copy of a stage's file on its page (see `feature::page`). The
//! file is parsed as `md_preview` parses it, by comrak with GitHub's
//! extensions, and raw HTML is left out as there. What is inside a block
//! flattens to its words: a link keeps its text, an image its alt text.
//! The page in a file's tab (`md_preview`) is still what shows it as GitHub
//! would.

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

/// A block of a markdown file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocBlock {
    Heading {
        level: u8,
        text: String,
    },
    Para(String),
    /// A list item, `depth` lists in (0 = the outermost).
    Item {
        depth: u8,
        mark: ItemMark,
        text: String,
    },
    /// A code block's lines.
    Code(Vec<String>),
    Quote(String),
    /// A table's rows with their cells joined, the first its header.
    Table(Vec<String>),
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
/// their own lines rather than running on into one. `md_preview` does the same.
pub fn hard_item_breaks<'a>(root: &'a AstNode<'a>) {
    for node in root.descendants() {
        let soft = matches!(node.data.borrow().value, NodeValue::SoftBreak);
        let in_item = (node.ancestors()).any(|a| {
            matches!(
                a.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            )
        });
        if soft && in_item {
            node.data.borrow_mut().value = NodeValue::LineBreak;
        }
    }
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
                if !text.is_empty() {
                    out.push(DocBlock::Quote(text));
                }
            }
        }
        NodeValue::Table(_) => {
            let rows = node.children().map(|row| {
                let cells: Vec<String> = row.children().map(inline).collect();
                cells.join(CELL_GAP)
            });
            out.push(DocBlock::Table(rows.collect()));
        }
        NodeValue::ThematicBreak => out.push(DocBlock::Rule),
        NodeValue::FootnoteDefinition(note) => {
            let text = node.children().map(inline).collect::<Vec<_>>().join(" ");
            out.push(DocBlock::Para(format!("[{}] {text}", note.name)));
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
                let text = String::new();
                out.push(DocBlock::Item { depth, mark, text });
            }
            block(child, depth.saturating_add(1), out);
        }
    }
    if let Some(mark) = mark {
        let text = String::new();
        out.push(DocBlock::Item { depth, mark, text });
    }
}

/// The words in `node`, flattened.
fn inline<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    words(node, &mut text);
    text.trim().to_owned()
}

/// `inline` onto `text`.
fn words<'a>(node: &'a AstNode<'a>, text: &mut String) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => text.push_str(t),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::SoftBreak => text.push(' '),
            // The page breaks its line there (`doc_text`).
            NodeValue::LineBreak => text.push('\n'),
            NodeValue::FootnoteReference(note) => {
                text.push_str(&format!("[{}]", note.name));
            }
            NodeValue::HtmlInline(_) | NodeValue::Raw(_) => {}
            _ => words(child, text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> DocBlock {
        DocBlock::Para(text.to_owned())
    }

    fn item(depth: u8, mark: ItemMark, text: &str) -> DocBlock {
        DocBlock::Item {
            depth,
            mark,
            text: text.to_owned(),
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
    fn inline_marks_flatten_to_their_words() {
        assert_eq!(
            blocks("A **bold** _word_ `code` [link](https://x.y) ![alt](i.png) ~~gone~~."),
            [para("A bold word code link alt gone.")]
        );
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
    fn a_table_is_rows_of_joined_cells_its_header_first() {
        assert_eq!(
            blocks("| a | b |\n|---|---|\n| 1 | **2** |\n"),
            [DocBlock::Table(vec![
                "a \u{b7} b".into(),
                "1 \u{b7} 2".into()
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
