//! HTML to Markdown converter.
//!
//! Parses HTML using the `scraper` crate (html5ever) and walks the DOM tree
//! to produce Markdown. Supports headings, paragraphs, tables, lists, links,
//! blockquotes, code blocks, bold/italic, and images.

use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;
use crate::markdown;

use ego_tree::iter::Edge;
use scraper::{Html, Node};

/// Converts HTML files to Markdown.
pub struct HtmlConverter;

impl Converter for HtmlConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["html", "htm"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let text = String::from_utf8(data.to_vec())?;
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
        let document = Html::parse_document(text);

        let title = extract_title(&document);
        let (md, plain) = walk_dom(&document);

        Ok(ConversionResult {
            markdown: md,
            plain_text: plain,
            title,
            ..Default::default()
        })
    }
}

/// Extract document title: <title> first, fallback to first <h1>.
fn extract_title(document: &Html) -> Option<String> {
    use scraper::Selector;
    if let Ok(sel) = Selector::parse("title")
        && let Some(el) = document.select(&sel).next()
    {
        let t = el.text().collect::<String>().trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    if let Ok(sel) = Selector::parse("h1")
        && let Some(el) = document.select(&sel).next()
    {
        let t = el.text().collect::<String>().trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

// ---- State types ----

struct WalkerState {
    output: String,
    plain_output: String,
    list_stack: Vec<ListContext>,
    in_pre: bool,
    skip_depth: usize,
    blockquote_depth: usize,
    trailing_newlines: usize,
    plain_trailing_newlines: usize,
    pending_heading: Option<PendingHeading>,
    pending_link: Option<PendingLink>,
    table_collector: Option<TableCollector>,
}

struct ListContext {
    ordered: bool,
    item_count: usize,
}

struct PendingHeading {
    level: u8,
    start_pos: usize,
    plain_start_pos: usize,
}

struct PendingLink {
    href: String,
    start_pos: usize,
}

struct TableCollector {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_header: bool,
    in_cell: bool,
}

impl WalkerState {
    fn new() -> Self {
        Self {
            output: String::new(),
            plain_output: String::new(),
            list_stack: Vec::new(),
            in_pre: false,
            skip_depth: 0,
            blockquote_depth: 0,
            trailing_newlines: 0,
            plain_trailing_newlines: 0,
            pending_heading: None,
            pending_link: None,
            table_collector: None,
        }
    }

    // ---- Markdown buffer helpers ----

    fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.output.push_str(s);
        self.trailing_newlines = s.bytes().rev().take_while(|&b| b == b'\n').count();
    }

    fn push_char(&mut self, c: char) {
        self.output.push(c);
        if c == '\n' {
            self.trailing_newlines += 1;
        } else {
            self.trailing_newlines = 0;
        }
    }

    fn ensure_newline(&mut self) {
        if self.trailing_newlines < 1 && !self.output.is_empty() {
            self.push_char('\n');
        }
    }

    fn ensure_blank_line(&mut self) {
        if self.output.is_empty() {
            return;
        }
        if self.blockquote_depth > 0 {
            let prefix = "> ".repeat(self.blockquote_depth);
            self.ensure_newline();
            if self.trailing_newlines < 2 {
                self.push_str(&prefix);
                self.push_char('\n');
            }
        } else {
            while self.trailing_newlines < 2 {
                self.push_char('\n');
            }
        }
    }

    // ---- Plain text buffer helpers ----

    fn plain_push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.plain_output.push_str(s);
        self.plain_trailing_newlines = s.bytes().rev().take_while(|&b| b == b'\n').count();
    }

    fn plain_push_char(&mut self, c: char) {
        self.plain_output.push(c);
        if c == '\n' {
            self.plain_trailing_newlines += 1;
        } else {
            self.plain_trailing_newlines = 0;
        }
    }

    fn plain_ensure_newline(&mut self) {
        if self.plain_trailing_newlines < 1 && !self.plain_output.is_empty() {
            self.plain_push_char('\n');
        }
    }

    fn plain_ensure_blank_line(&mut self) {
        if self.plain_output.is_empty() {
            return;
        }
        while self.plain_trailing_newlines < 2 {
            self.plain_push_char('\n');
        }
    }

    // ---- Dual-buffer helpers ----

    fn both_push_str(&mut self, s: &str) {
        self.push_str(s);
        self.plain_push_str(s);
    }

    fn both_push_char(&mut self, c: char) {
        self.push_char(c);
        self.plain_push_char(c);
    }

    fn both_ensure_newline(&mut self) {
        self.ensure_newline();
        self.plain_ensure_newline();
    }

    fn both_ensure_blank_line(&mut self) {
        self.ensure_blank_line();
        self.plain_ensure_blank_line();
    }

    fn in_table_cell(&self) -> bool {
        self.table_collector.as_ref().is_some_and(|tc| tc.in_cell)
    }
}

// ---- DOM walker ----

fn walk_dom(document: &Html) -> (String, String) {
    let mut state = WalkerState::new();

    for edge in document.root_element().traverse() {
        match edge {
            Edge::Open(node) => handle_open(&mut state, &node),
            Edge::Close(node) => handle_close(&mut state, &node),
        }
    }

    // Final cleanup: trim trailing whitespace
    let md = state.output.trim().to_string();
    let md = if md.is_empty() { md } else { md + "\n" };

    let plain = state.plain_output.trim().to_string();
    let plain = if plain.is_empty() {
        plain
    } else {
        plain + "\n"
    };

    (md, plain)
}

// ---- Element handlers (open) ----

fn handle_open(state: &mut WalkerState, node: &ego_tree::NodeRef<Node>) {
    match node.value() {
        Node::Text(text) => handle_text(state, text),
        Node::Element(el) => {
            let tag = el.name().to_ascii_lowercase();
            match tag.as_str() {
                "script" | "style" | "head" => {
                    state.skip_depth += 1;
                }
                _ if state.skip_depth > 0 => {}
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    state.both_ensure_blank_line();
                    let level = tag[1..].parse::<u8>().unwrap_or(1);
                    state.pending_heading = Some(PendingHeading {
                        level,
                        start_pos: state.output.len(),
                        plain_start_pos: state.plain_output.len(),
                    });
                }
                "p" if !state.in_table_cell() => {
                    state.both_ensure_blank_line();
                }
                "a" => {
                    let href = el.attr("href").unwrap_or("").to_string();
                    state.pending_link = Some(PendingLink {
                        href,
                        start_pos: state.output.len(),
                    });
                }
                "img" => {
                    let alt = el.attr("alt").unwrap_or("");
                    let src = el.attr("src").unwrap_or("");
                    state.push_str(&format!("![{}]({})", alt, src));
                    state.plain_push_str(alt);
                }
                "strong" | "b" => {
                    state.push_str("**");
                    // plain text: no markers
                }
                "em" | "i" => {
                    state.push_str("*");
                    // plain text: no markers
                }
                "code" if !state.in_pre => {
                    state.push_str("`");
                    // plain text: no backtick
                }
                "pre" => {
                    state.in_pre = true;
                    state.both_ensure_blank_line();
                    state.push_str("```\n");
                    // plain text: no fence
                }
                "ul" => {
                    if !state.list_stack.is_empty() {
                        state.both_ensure_newline();
                    } else {
                        state.both_ensure_blank_line();
                    }
                    state.list_stack.push(ListContext {
                        ordered: false,
                        item_count: 0,
                    });
                }
                "ol" => {
                    if !state.list_stack.is_empty() {
                        state.both_ensure_newline();
                    } else {
                        state.both_ensure_blank_line();
                    }
                    state.list_stack.push(ListContext {
                        ordered: true,
                        item_count: 0,
                    });
                }
                "li" => {
                    let indent_level = state.list_stack.len().saturating_sub(1);
                    let indent = "  ".repeat(indent_level);
                    let prefix = if let Some(ctx) = state.list_stack.last_mut() {
                        ctx.item_count += 1;
                        if ctx.ordered {
                            format!("{}{}. ", indent, ctx.item_count)
                        } else {
                            format!("{}- ", indent)
                        }
                    } else {
                        format!("{}- ", indent)
                    };
                    state.push_str(&prefix);
                    // plain text: just indentation, no marker
                    state.plain_push_str(&indent);
                }
                "table" => {
                    state.both_ensure_blank_line();
                    state.table_collector = Some(TableCollector {
                        headers: Vec::new(),
                        rows: Vec::new(),
                        current_row: Vec::new(),
                        current_cell: String::new(),
                        in_header: false,
                        in_cell: false,
                    });
                }
                "thead" => {
                    if let Some(tc) = &mut state.table_collector {
                        tc.in_header = true;
                    }
                }
                "tbody" => {
                    if let Some(tc) = &mut state.table_collector {
                        tc.in_header = false;
                    }
                }
                "tr" => {
                    if let Some(tc) = &mut state.table_collector {
                        tc.current_row = Vec::new();
                    }
                }
                "th" | "td" => {
                    if let Some(tc) = &mut state.table_collector {
                        tc.current_cell = String::new();
                        tc.in_cell = true;
                    }
                }
                "blockquote" => {
                    state.blockquote_depth += 1;
                    state.ensure_newline();
                    state.plain_ensure_newline();
                }
                "hr" => {
                    state.ensure_blank_line();
                    state.push_str("---\n");
                    state.plain_ensure_blank_line();
                }
                "br" => {
                    if state.in_pre {
                        state.both_push_char('\n');
                    } else if state.in_table_cell() {
                        // In table cells, just add a space instead of a newline
                    } else {
                        state.both_push_char('\n');
                        // Add blockquote prefix after br (markdown only)
                        if state.blockquote_depth > 0 {
                            let prefix = "> ".repeat(state.blockquote_depth);
                            state.push_str(&prefix);
                        }
                    }
                }
                "input" => {
                    let input_type = el.attr("type").unwrap_or("");
                    if input_type == "checkbox" {
                        let checked = el.attr("checked").is_some();
                        if checked {
                            state.push_str("[x] ");
                        } else {
                            state.push_str("[ ] ");
                        }
                        // plain text: no checkbox markers
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

// ---- Element handlers (close) ----

fn handle_close(state: &mut WalkerState, node: &ego_tree::NodeRef<Node>) {
    if let Node::Element(el) = node.value() {
        let tag = el.name().to_ascii_lowercase();
        match tag.as_str() {
            "script" | "style" | "head" => {
                state.skip_depth = state.skip_depth.saturating_sub(1);
            }
            _ if state.skip_depth > 0 => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if let Some(pending) = state.pending_heading.take() {
                    // Markdown: format as heading
                    let text = state.output[pending.start_pos..].to_string();
                    state.output.truncate(pending.start_pos);
                    state.trailing_newlines = state
                        .output
                        .bytes()
                        .rev()
                        .take_while(|&b| b == b'\n')
                        .count();
                    let heading = markdown::format_heading(pending.level, text.trim());
                    state.push_str(&heading);

                    // Plain text: just the text with a newline
                    let plain_text = state.plain_output[pending.plain_start_pos..].to_string();
                    state.plain_output.truncate(pending.plain_start_pos);
                    state.plain_trailing_newlines = state
                        .plain_output
                        .bytes()
                        .rev()
                        .take_while(|&b| b == b'\n')
                        .count();
                    let trimmed = plain_text.trim();
                    if !trimmed.is_empty() {
                        state.plain_push_str(trimmed);
                        state.plain_push_char('\n');
                    }
                }
            }
            "p" if !state.in_table_cell() => {
                state.both_ensure_blank_line();
            }
            "a" => {
                if let Some(pending) = state.pending_link.take() {
                    // Markdown: format as link
                    let text = state.output[pending.start_pos..].to_string();
                    state.output.truncate(pending.start_pos);
                    state.trailing_newlines = state
                        .output
                        .bytes()
                        .rev()
                        .take_while(|&b| b == b'\n')
                        .count();
                    if pending.href.is_empty() {
                        state.push_str(text.trim());
                    } else {
                        state.push_str(&format!("[{}]({})", text.trim(), pending.href));
                    }

                    // Plain text: just the link text (already accumulated)
                    // No modification needed — text was pushed to plain_output during traversal
                }
            }
            "strong" | "b" => {
                state.push_str("**");
                // plain text: no closing marker
            }
            "em" | "i" => {
                state.push_str("*");
                // plain text: no closing marker
            }
            "code" if !state.in_pre => {
                state.push_str("`");
                // plain text: no closing backtick
            }
            "pre" => {
                state.ensure_newline();
                state.push_str("```\n");
                state.plain_ensure_newline();
                state.in_pre = false;
            }
            "ul" | "ol" => {
                state.list_stack.pop();
                if state.list_stack.is_empty() {
                    state.both_ensure_blank_line();
                }
            }
            "li" => {
                state.both_ensure_newline();
            }
            "table" => {
                if let Some(tc) = state.table_collector.take() {
                    let table_md = render_table(&tc, false);
                    state.push_str(&table_md);
                    let table_plain = render_table(&tc, true);
                    state.plain_push_str(&table_plain);
                }
            }
            "thead" => {
                // in_header handled by tbody open or next row
            }
            "tr" => {
                if let Some(tc) = &mut state.table_collector {
                    let row = std::mem::take(&mut tc.current_row);
                    if tc.in_header {
                        tc.headers = row;
                    } else {
                        tc.rows.push(row);
                    }
                }
            }
            "th" | "td" => {
                if let Some(tc) = &mut state.table_collector {
                    let cell = std::mem::take(&mut tc.current_cell);
                    tc.current_row.push(cell.trim().to_string());
                    tc.in_cell = false;
                }
            }
            "blockquote" => {
                state.blockquote_depth = state.blockquote_depth.saturating_sub(1);
                state.both_ensure_newline();
            }
            _ => {}
        }
    }
}

// ---- Text processing helpers ----

fn handle_text(state: &mut WalkerState, text: &scraper::node::Text) {
    if state.skip_depth > 0 {
        return;
    }

    let raw = text.text.as_ref();

    // Inside a table cell: accumulate into the cell buffer (shared for both outputs)
    if let Some(tc) = &mut state.table_collector {
        if tc.in_cell {
            tc.current_cell.push_str(raw);
            return;
        }
        // Text outside cells but inside table (e.g. whitespace between tags) — ignore
        return;
    }

    if state.in_pre {
        state.both_push_str(raw);
        return;
    }

    // Collapse whitespace
    let collapsed = collapse_whitespace(raw);

    if collapsed.is_empty() {
        return;
    }

    // Just whitespace — only add if output doesn't already end with whitespace/newline
    if collapsed == " " {
        if !state.output.is_empty() && state.trailing_newlines == 0 {
            let last = state.output.bytes().last().unwrap_or(b' ');
            if last != b' ' && last != b'\t' {
                state.push_char(' ');
            }
        }
        if !state.plain_output.is_empty() && state.plain_trailing_newlines == 0 {
            let last = state.plain_output.bytes().last().unwrap_or(b' ');
            if last != b' ' && last != b'\t' {
                state.plain_push_char(' ');
            }
        }
        return;
    }

    // Skip leading space if output already ends with whitespace
    let md_collapsed = if collapsed.starts_with(' ') && !state.output.is_empty() {
        let last = state.output.bytes().last().unwrap_or(b'\n');
        if last == b' ' || last == b'\t' {
            &collapsed[1..]
        } else {
            &collapsed
        }
    } else {
        &collapsed
    };

    let plain_collapsed = if collapsed.starts_with(' ') && !state.plain_output.is_empty() {
        let last = state.plain_output.bytes().last().unwrap_or(b'\n');
        if last == b' ' || last == b'\t' {
            &collapsed[1..]
        } else {
            &collapsed
        }
    } else {
        &collapsed
    };

    // Markdown: apply blockquote prefix at line starts
    if !md_collapsed.is_empty() {
        if state.blockquote_depth > 0 {
            let prefix = "> ".repeat(state.blockquote_depth);
            if state.trailing_newlines > 0 || state.output.is_empty() {
                state.push_str(&prefix);
            }
            let lines: Vec<&str> = md_collapsed.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    state.push_char('\n');
                    state.push_str(&prefix);
                }
                state.push_str(line);
            }
        } else {
            state.push_str(md_collapsed);
        }
    }

    // Plain text: no blockquote prefix
    if !plain_collapsed.is_empty() {
        state.plain_push_str(plain_collapsed);
    }
}

/// Collapse consecutive whitespace characters into a single space.
fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            if !prev_ws {
                result.push(' ');
            }
            prev_ws = true;
        } else {
            result.push(c);
            prev_ws = false;
        }
    }
    result
}

/// Render a completed table collector into a table string.
///
/// When `plain` is true, produces tab-separated output; otherwise produces
/// a pipe-delimited Markdown table.
fn render_table(tc: &TableCollector, plain: bool) -> String {
    // If no explicit headers (no <thead>), use first row as headers
    let (headers, data_rows) = if tc.headers.is_empty() && !tc.rows.is_empty() {
        (tc.rows[0].clone(), &tc.rows[1..])
    } else {
        (tc.headers.clone(), tc.rows.as_slice())
    };

    if headers.is_empty() {
        return String::new();
    }

    let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
    let row_refs: Vec<Vec<&str>> = data_rows
        .iter()
        .map(|row| row.iter().map(|s| s.as_str()).collect())
        .collect();
    if plain {
        markdown::build_table_plain(&header_refs, &row_refs)
    } else {
        markdown::build_table(&header_refs, &row_refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::ConversionOptions;

    fn convert_html(html: &str) -> ConversionResult {
        let converter = HtmlConverter;
        converter
            .convert(html.as_bytes(), &ConversionOptions::default())
            .unwrap()
    }

    #[test]
    fn test_html_supported_extensions() {
        let converter = HtmlConverter;
        let exts = converter.supported_extensions();
        assert_eq!(exts, &["html", "htm"]);
    }

    #[test]
    fn test_html_can_convert() {
        let converter = HtmlConverter;
        assert!(converter.can_convert("html", &[]));
        assert!(converter.can_convert("htm", &[]));
        assert!(!converter.can_convert("txt", &[]));
        assert!(!converter.can_convert("docx", &[]));
    }

    #[test]
    fn test_html_empty_document() {
        let result = convert_html("");
        assert!(result.markdown.is_empty());
    }

    #[test]
    fn test_html_headings_h1_through_h6() {
        let html = r#"<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("# H1"));
        assert!(result.markdown.contains("## H2"));
        assert!(result.markdown.contains("### H3"));
        assert!(result.markdown.contains("#### H4"));
        assert!(result.markdown.contains("##### H5"));
        assert!(result.markdown.contains("###### H6"));
    }

    #[test]
    fn test_html_paragraph_basic() {
        let html = "<p>First paragraph</p><p>Second paragraph</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("First paragraph"));
        assert!(result.markdown.contains("Second paragraph"));
        // Should have blank line between paragraphs
        assert!(
            result
                .markdown
                .contains("First paragraph\n\nSecond paragraph")
        );
    }

    #[test]
    fn test_html_bold_and_italic() {
        let html = "<p><strong>bold</strong> and <em>italic</em></p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("**bold**"));
        assert!(result.markdown.contains("*italic*"));
    }

    #[test]
    fn test_html_b_and_i_tags() {
        let html = "<p><b>bold</b> and <i>italic</i></p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("**bold**"));
        assert!(result.markdown.contains("*italic*"));
    }

    #[test]
    fn test_html_inline_code() {
        let html = "<p>Use <code>cargo build</code> to compile.</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("`cargo build`"));
    }

    #[test]
    fn test_html_code_block() {
        let html = "<pre><code>fn main() {\n    println!(\"hello\");\n}</code></pre>";
        let result = convert_html(html);
        assert!(result.markdown.contains("```\n"));
        assert!(result.markdown.contains("fn main()"));
        assert!(result.markdown.contains("println!"));
    }

    #[test]
    fn test_html_link_basic() {
        let html = r#"<a href="https://example.com">Example</a>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("[Example](https://example.com)"));
    }

    #[test]
    fn test_html_link_no_href() {
        let html = "<a>just text</a>";
        let result = convert_html(html);
        assert!(result.markdown.contains("just text"));
        assert!(!result.markdown.contains("["));
    }

    #[test]
    fn test_html_image_basic() {
        let html = r#"<img src="photo.jpg" alt="A photo">"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("![A photo](photo.jpg)"));
    }

    #[test]
    fn test_html_image_no_alt() {
        let html = r#"<img src="photo.jpg">"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("![](photo.jpg)"));
    }

    #[test]
    fn test_html_unordered_list() {
        let html = "<ul><li>Apple</li><li>Banana</li><li>Cherry</li></ul>";
        let result = convert_html(html);
        assert!(result.markdown.contains("- Apple"));
        assert!(result.markdown.contains("- Banana"));
        assert!(result.markdown.contains("- Cherry"));
    }

    #[test]
    fn test_html_ordered_list() {
        let html = "<ol><li>First</li><li>Second</li><li>Third</li></ol>";
        let result = convert_html(html);
        assert!(result.markdown.contains("1. First"));
        assert!(result.markdown.contains("2. Second"));
        assert!(result.markdown.contains("3. Third"));
    }

    #[test]
    fn test_html_nested_list() {
        let html = r#"<ul>
            <li>Outer
                <ul>
                    <li>Inner A</li>
                    <li>Inner B</li>
                </ul>
            </li>
            <li>Outer 2</li>
        </ul>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("- Outer"));
        assert!(result.markdown.contains("  - Inner A"));
        assert!(result.markdown.contains("  - Inner B"));
        assert!(result.markdown.contains("- Outer 2"));
    }

    #[test]
    fn test_html_table_basic() {
        let html = r#"<table>
            <thead><tr><th>Name</th><th>Age</th></tr></thead>
            <tbody>
                <tr><td>Alice</td><td>30</td></tr>
                <tr><td>Bob</td><td>25</td></tr>
            </tbody>
        </table>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("| Name | Age |"));
        assert!(result.markdown.contains("|---|---|"));
        assert!(result.markdown.contains("| Alice | 30 |"));
        assert!(result.markdown.contains("| Bob | 25 |"));
    }

    #[test]
    fn test_html_table_no_thead() {
        let html = r#"<table>
            <tr><td>Name</td><td>Age</td></tr>
            <tr><td>Alice</td><td>30</td></tr>
        </table>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("| Name | Age |"));
        assert!(result.markdown.contains("| Alice | 30 |"));
    }

    #[test]
    fn test_html_table_empty_cells() {
        let html = r#"<table>
            <thead><tr><th>A</th><th>B</th><th>C</th></tr></thead>
            <tbody><tr><td>1</td><td></td><td>3</td></tr></tbody>
        </table>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("| 1 |  | 3 |"));
    }

    #[test]
    fn test_html_blockquote() {
        let html = "<blockquote>Quoted text</blockquote>";
        let result = convert_html(html);
        assert!(result.markdown.contains("> Quoted text"));
    }

    #[test]
    fn test_html_nested_blockquote() {
        let html = "<blockquote><blockquote>Deeply quoted</blockquote></blockquote>";
        let result = convert_html(html);
        assert!(result.markdown.contains("> > Deeply quoted"));
    }

    #[test]
    fn test_html_horizontal_rule() {
        let html = "<p>Above</p><hr><p>Below</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("---"));
        assert!(result.markdown.contains("Above"));
        assert!(result.markdown.contains("Below"));
    }

    #[test]
    fn test_html_line_break() {
        let html = "<p>Line one<br>Line two</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("Line one\nLine two"));
    }

    #[test]
    fn test_html_script_stripped() {
        let html = "<p>Visible</p><script>alert('xss');</script><p>Also visible</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("Visible"));
        assert!(result.markdown.contains("Also visible"));
        assert!(!result.markdown.contains("alert"));
        assert!(!result.markdown.contains("script"));
    }

    #[test]
    fn test_html_style_stripped() {
        let html = "<style>body { color: red; }</style><p>Content</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("Content"));
        assert!(!result.markdown.contains("color"));
        assert!(!result.markdown.contains("red"));
    }

    #[test]
    fn test_html_title_from_title_tag() {
        let html =
            "<html><head><title>My Page Title</title></head><body><p>Content</p></body></html>";
        let result = convert_html(html);
        assert_eq!(result.title, Some("My Page Title".to_string()));
    }

    #[test]
    fn test_html_title_fallback_h1() {
        let html = "<html><body><h1>Main Heading</h1><p>Content</p></body></html>";
        let result = convert_html(html);
        assert_eq!(result.title, Some("Main Heading".to_string()));
    }

    #[test]
    fn test_html_unicode_cjk() {
        let html = "<p>한국어 中文 日本語</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("中文"));
        assert!(result.markdown.contains("日本語"));
    }

    #[test]
    fn test_html_emoji() {
        let html = "<p>Hello 🌍🚀✨ World</p>";
        let result = convert_html(html);
        assert!(result.markdown.contains("🌍"));
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨"));
    }

    #[test]
    fn test_html_whitespace_collapse() {
        let html = "<p>  Multiple   spaces   here  </p>";
        let result = convert_html(html);
        // Whitespace should be collapsed
        assert!(!result.markdown.contains("  "));
        assert!(result.markdown.contains("Multiple spaces here"));
    }

    #[test]
    fn test_html_pre_whitespace_preserved() {
        let html = "<pre>  indented\n    more indented\n</pre>";
        let result = convert_html(html);
        assert!(result.markdown.contains("  indented"));
        assert!(result.markdown.contains("    more indented"));
    }

    #[test]
    fn test_html_heading_with_inline_formatting() {
        let html = "<h2><em>Italic Title</em></h2>";
        let result = convert_html(html);
        assert!(result.markdown.contains("## *Italic Title*"));
    }

    #[test]
    fn test_html_checkbox_input() {
        let html = r#"<ul>
            <li><input type="checkbox" checked> Done</li>
            <li><input type="checkbox"> Not done</li>
        </ul>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("[x] Done"));
        assert!(result.markdown.contains("[ ] Not done"));
    }

    // ---- Plain text output tests ----

    #[test]
    fn test_html_plain_text_no_heading_markers() {
        let html = "<h1>Title</h1><h2>Subtitle</h2>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("Title"));
        assert!(result.plain_text.contains("Subtitle"));
        assert!(!result.plain_text.contains("# "));
        assert!(!result.plain_text.contains("## "));
    }

    #[test]
    fn test_html_plain_text_no_bold_italic_markers() {
        let html = "<p><strong>bold</strong> and <em>italic</em></p>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("bold"));
        assert!(result.plain_text.contains("italic"));
        assert!(!result.plain_text.contains("**"));
        assert!(!result.plain_text.contains("*italic*"));
    }

    #[test]
    fn test_html_plain_text_link_text_only() {
        let html = r#"<a href="https://example.com">Example</a>"#;
        let result = convert_html(html);
        assert!(result.plain_text.contains("Example"));
        assert!(!result.plain_text.contains("[Example]"));
        assert!(!result.plain_text.contains("https://example.com"));
    }

    #[test]
    fn test_html_plain_text_image_alt_text_only() {
        let html = r#"<img src="photo.jpg" alt="A photo">"#;
        let result = convert_html(html);
        assert!(result.plain_text.contains("A photo"));
        assert!(!result.plain_text.contains("!["));
        assert!(!result.plain_text.contains("photo.jpg"));
    }

    #[test]
    fn test_html_plain_text_no_code_fences() {
        let html = "<pre><code>fn main() {}</code></pre>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("fn main() {}"));
        assert!(!result.plain_text.contains("```"));
    }

    #[test]
    fn test_html_plain_text_no_inline_backtick() {
        let html = "<p>Use <code>cargo</code> to build.</p>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("cargo"));
        assert!(!result.plain_text.contains("`cargo`"));
    }

    #[test]
    fn test_html_plain_text_table_tab_separated() {
        let html = r#"<table>
            <thead><tr><th>Name</th><th>Age</th></tr></thead>
            <tbody><tr><td>Alice</td><td>30</td></tr></tbody>
        </table>"#;
        let result = convert_html(html);
        assert!(result.plain_text.contains("Name\tAge"));
        assert!(result.plain_text.contains("Alice\t30"));
        assert!(!result.plain_text.contains("|"));
    }

    #[test]
    fn test_html_plain_text_list_no_markers() {
        let html = "<ul><li>Apple</li><li>Banana</li></ul>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("Apple"));
        assert!(result.plain_text.contains("Banana"));
        assert!(!result.plain_text.contains("- "));
    }

    #[test]
    fn test_html_plain_text_no_blockquote_prefix() {
        let html = "<blockquote>Quoted text</blockquote>";
        let result = convert_html(html);
        assert!(result.plain_text.contains("Quoted text"));
        assert!(!result.plain_text.contains("> "));
    }

    #[test]
    fn test_html_plain_text_empty_document() {
        let result = convert_html("");
        assert!(result.plain_text.is_empty());
    }

    #[test]
    fn test_html_malformed_html_best_effort() {
        let html = "<p>Unclosed paragraph<p>Another<b>Bold without close";
        let result = convert_html(html);
        assert!(result.markdown.contains("Unclosed paragraph"));
        assert!(result.markdown.contains("Another"));
        assert!(result.markdown.contains("Bold without close"));
    }
}
