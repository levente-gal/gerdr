//! A markdown file rendered as HTML, as GitHub does (tables, task lists,
//! footnotes), by comrak: what the phone view shows of a plan or a file
//! (`phone`). gerdr paints a file itself everywhere else (`md_blocks`,
//! `doc_page`).
//!
//! The markdown is an agent's: raw HTML in it is left out (comrak's
//! `unsafe` stays off), and links to `javascript:` and the like are dropped.

/// A file bigger than this isn't rendered: what shows it says so.
pub(crate) const FILE_MAX: u64 = 512 * 1024;

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
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);
    crate::md_blocks::hard_item_breaks(root);
    let mut html = String::new();
    // Writing to a String can't fail.
    let _ = comrak::format_html(root, &options, &mut html);
    html
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
    fn lines_in_a_list_item_break_there() {
        let html = render("- [ ] M1\n  Files: a.py\n  Check: tests\n\nOne\nline.\n");
        assert!(
            html.contains("M1<br data-sourcepos=\"1:9-1:9\" />\nFiles: a.py<br"),
            "{html}"
        );
        assert!(html.contains("One\nline."), "{html}");
    }
}
