//! HTML to Markdown converter.
//!
//! Parses HTML using the `scraper` crate (html5ever) and walks the DOM tree
//! to produce Markdown. Supports headings, paragraphs, tables, lists, links,
//! blockquotes, code blocks, bold/italic, and images.

use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown;

use ego_tree::iter::Edge;
use scraper::{Html, Node};

/// Upper bound on table columns / per-cell colspan.
///
/// Caps `colspan` so a malformed or hostile attribute cannot drive unbounded
/// allocation when a row is expanded to the grid width. Far larger than any real
/// table.
const MAX_TABLE_COLS: usize = 4096;

/// Upper bound on a cell's `rowspan`, limiting how many placeholder rows a single
/// vertical span can materialize.
const MAX_TABLE_ROWS: usize = 4096;

/// Upper bound on the total number of grid cells materialized while resolving
/// `rowspan` placeholders across an entire table.
///
/// `colspan` and `rowspan` are each clamped to 4096 independently, but their
/// product is not, so a single `<td rowspan="4096" colspan="4096">` followed by
/// thousands of empty `<tr>` rows could otherwise expand to ~16.7M cells
/// (gigabytes of `HtmlCell`). This cap bounds the *product*: normalization stops
/// emitting once the running total of output cells reaches this value
/// (best-effort truncation, matching the silent style of the per-cell clamps).
///
/// Every table renders as a single GFM grid, so all materialized cells are also
/// escaped and joined into the output. 100_000 is far larger than any real-world
/// table (e.g. 200x500) yet keeps both peak `HtmlCell` allocation and the
/// rendered table size bounded to a few MB worst case.
const MAX_TABLE_CELLS: usize = 100_000;

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
        let (md, plain, warnings) = walk_dom(&document);

        Ok(ConversionResult {
            markdown: md,
            plain_text: plain,
            title,
            warnings,
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
    /// Stack of table collectors; the top is the table currently being walked.
    /// A nested `<table>` inside a cell pushes a new frame.
    table_stack: Vec<TableCollector>,
    /// Recoverable issues encountered while walking (e.g. a table truncated by a
    /// resource cap). Surfaced in `ConversionResult.warnings` so best-effort
    /// truncation is observable rather than silent.
    warnings: Vec<ConversionWarning>,
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

/// One ordered piece of a table cell's content.
///
/// A cell may interleave text and nested tables (e.g. `text<table>..</table>more`).
/// Storing the pieces in document order — instead of one text string plus a
/// separate nested-table string — preserves their relative position when the
/// containing table is linearized. (Mirrors the DOCX cell-block model.)
#[derive(Debug, Clone)]
enum CellSegment {
    /// Accumulated text run.
    Text(String),
    /// A fully-rendered nested table: `(markdown, plain_text)`. A nested table
    /// cannot live inside a single GFM cell, so it is emitted as a standalone
    /// block when the containing table is linearized.
    Table {
        /// Rendered markdown of the nested table.
        md: String,
        /// Plain-text form of the nested table.
        plain: String,
    },
}

/// One buffered HTML table cell with span info and ordered content segments.
#[derive(Debug, Clone, Default)]
struct HtmlCell {
    /// Columns this cell spans (`colspan`), always at least 1.
    colspan: usize,
    /// Rows this cell spans (`rowspan`), always at least 1.
    rowspan: usize,
    /// Content in document order: text runs and nested tables interleaved.
    segments: Vec<CellSegment>,
}

impl HtmlCell {
    /// Whether this cell contains a rendered nested table.
    fn has_nested_table(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, CellSegment::Table { .. }))
    }

    /// Append text to the cell, extending the trailing `Text` segment if the last
    /// segment is text, otherwise starting a new one. Keeps consecutive characters
    /// in one segment so grid cells join cleanly.
    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(CellSegment::Text(t)) = self.segments.last_mut() {
            t.push_str(s);
        } else {
            self.segments.push(CellSegment::Text(s.to_string()));
        }
    }

    /// Append a single space to the trailing text segment (used to separate two
    /// block elements, e.g. two headings, inside one cell) only when the cell
    /// already ends with non-whitespace text. Never starts a new segment and
    /// never inserts space before a nested table.
    fn push_separator_space(&mut self) {
        if let Some(CellSegment::Text(t)) = self.segments.last_mut()
            && !t.is_empty()
            && !t.ends_with(char::is_whitespace)
        {
            t.push(' ');
        }
    }

    /// Join all text segments into one string (nested tables omitted). Used for
    /// the normal grid path where a cell becomes a single GFM cell. Trimmed.
    fn joined_text(&self) -> String {
        let mut s = String::new();
        for seg in &self.segments {
            if let CellSegment::Text(t) = seg {
                s.push_str(t);
            }
        }
        s.trim().to_string()
    }
}

/// One buffered HTML table row.
#[derive(Debug, Clone, Default)]
struct HtmlRow {
    /// Cells in document order.
    cells: Vec<HtmlCell>,
    /// True if this row came from `<thead>`.
    is_header_row: bool,
}

/// Buffers an HTML table while its elements are walked.
struct TableCollector {
    /// Completed rows, in document order.
    rows: Vec<HtmlRow>,
    /// In-progress row.
    current_row: HtmlRow,
    /// In-progress cell.
    current_cell: HtmlCell,
    /// Whether the current row is inside `<thead>`.
    in_header: bool,
    /// Whether a `<th>`/`<td>` is currently open.
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
            table_stack: Vec::new(),
            warnings: Vec::new(),
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
        self.table_stack.last().is_some_and(|tc| tc.in_cell)
    }
}

// ---- DOM walker ----

fn walk_dom(document: &Html) -> (String, String, Vec<ConversionWarning>) {
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

    (md, plain, state.warnings)
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
                    let level = tag[1..].parse::<u8>().unwrap_or(1);
                    // A heading inside a table cell keeps its text inline in the
                    // cell (no heading markers), but headings are block elements, so
                    // separate their text from any preceding cell content so two
                    // headings in one cell do not mash together (e.g. "FirstSecond").
                    if let Some(tc) = state.table_stack.last_mut()
                        && tc.in_cell
                    {
                        // Separate this heading's text from preceding cell text so
                        // two headings in one cell do not mash together.
                        tc.current_cell.push_separator_space();
                    }
                    if !state.in_table_cell() {
                        state.both_ensure_blank_line();
                        state.pending_heading = Some(PendingHeading {
                            level,
                            start_pos: state.output.len(),
                            plain_start_pos: state.plain_output.len(),
                        });
                    }
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
                    // Push a new table frame (supports nested tables).
                    state.table_stack.push(TableCollector {
                        rows: Vec::new(),
                        current_row: HtmlRow::default(),
                        current_cell: HtmlCell::default(),
                        in_header: false,
                        in_cell: false,
                    });
                }
                "thead" => {
                    if let Some(tc) = state.table_stack.last_mut() {
                        tc.in_header = true;
                    }
                }
                "tbody" => {
                    if let Some(tc) = state.table_stack.last_mut() {
                        tc.in_header = false;
                    }
                }
                "tr" => {
                    if let Some(tc) = state.table_stack.last_mut() {
                        tc.current_row = HtmlRow::default();
                        // Read back in `render_table` to pick the GFM header row,
                        // so a `<thead>` that does not appear first still wins.
                        tc.current_row.is_header_row = tc.in_header;
                    }
                }
                "th" | "td" => {
                    // Clamp colspan/rowspan so a hostile attribute (e.g.
                    // colspan="2000000000") cannot drive unbounded allocation when
                    // the row is expanded to the grid width.
                    let colspan = el
                        .attr("colspan")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_TABLE_COLS);
                    let rowspan = el
                        .attr("rowspan")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_TABLE_ROWS);
                    if let Some(tc) = state.table_stack.last_mut() {
                        tc.current_cell = HtmlCell {
                            colspan,
                            rowspan,
                            ..Default::default()
                        };
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
                if let Some(tc) = state.table_stack.pop() {
                    let table_md = render_table(&tc, false, &mut state.warnings);
                    let table_plain = render_table(&tc, true, &mut Vec::new());
                    if state.table_stack.last().is_some_and(|p| p.in_cell) {
                        // Nested table: append it as a Table segment so it keeps its
                        // document position relative to any text on either side. The
                        // containing table is then linearized, emitting the nested
                        // table as a standalone block rather than escaping it into a
                        // single grid cell.
                        let parent = state.table_stack.last_mut().unwrap();
                        parent.current_cell.segments.push(CellSegment::Table {
                            md: table_md.trim_end().to_string(),
                            plain: table_plain.trim_end().to_string(),
                        });
                    } else {
                        state.push_str(&table_md);
                        state.plain_push_str(&table_plain);
                    }
                }
            }
            "thead" => {
                if let Some(tc) = state.table_stack.last_mut() {
                    tc.in_header = false;
                }
            }
            "tr" => {
                if let Some(tc) = state.table_stack.last_mut() {
                    let row = std::mem::take(&mut tc.current_row);
                    tc.rows.push(row);
                }
            }
            "th" | "td" => {
                if let Some(tc) = state.table_stack.last_mut() {
                    // Trimming now happens at read time (`joined_text` / the
                    // linearize branch), matching how DOCX trims per-block; there is
                    // no longer a single `content` field to trim here.
                    let cell = std::mem::take(&mut tc.current_cell);
                    tc.current_row.cells.push(cell);
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

    // Inside a table cell: accumulate into the cell's ordered segments (shared for
    // both outputs). Extends the trailing text segment so text on either side of a
    // nested table keeps its document position relative to the table.
    if let Some(tc) = state.table_stack.last_mut() {
        if tc.in_cell {
            tc.current_cell.push_text(raw);
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

/// Resolve `rowspan` by inserting empty placeholder cells into lower rows.
///
/// HTML declares a vertical span on the top cell and omits the covered columns
/// from subsequent rows. To get a rectangular grid, each spanned column is
/// re-materialized as an empty cell in the rows it covers.
///
/// Carried vertical spans are tracked in a column-indexed `Vec<usize>` (the
/// remaining rows still covered at each column), never a hash map, so coverage
/// lookup is O(1), per-row bookkeeping is O(width), and output stays
/// deterministic. The grid is bounded on two axes: each output row is capped at
/// the table's provisional width (max summed `colspan` over rows, itself capped
/// at `MAX_TABLE_COLS`) so a stray span cannot push a row past it, and the total
/// number of materialized cells is capped at `MAX_TABLE_CELLS`. Both caps are
/// no-ops for realistic tables and only truncate hostile inputs (e.g. a single
/// `rowspan`x`colspan` cell that would otherwise expand quadratically).
/// Returns the normalized rows and `true` if the `MAX_TABLE_CELLS` cap dropped
/// cells (so the caller can emit a truncation warning).
fn normalize_html_rowspans(rows: &[HtmlRow]) -> (Vec<HtmlRow>, bool) {
    // Provisional grid width: the widest row by summed colspan, capped so an
    // extreme colspan cannot drive unbounded per-column allocation. This is the
    // same quantity `html_grid_width` derives from the *normalized* rows, but
    // computed up front from the raw rows so it can bound the work below.
    let width = rows
        .iter()
        .map(|r| {
            r.cells
                .iter()
                .map(|c| c.colspan.max(1))
                .sum::<usize>()
                .min(MAX_TABLE_COLS)
        })
        .max()
        .unwrap_or(0)
        .min(MAX_TABLE_COLS);

    let mut out: Vec<HtmlRow> = Vec::with_capacity(rows.len());
    if width == 0 {
        // No columns anywhere: preserve the (empty) rows verbatim.
        for row in rows {
            out.push(HtmlRow {
                cells: Vec::new(),
                is_header_row: row.is_header_row,
            });
        }
        return (out, false);
    }

    // Carried spans, indexed by column: remaining rows still covered at `col`.
    // O(1) coverage lookup, O(width) decrement per row, no hash map.
    let mut pending: Vec<usize> = vec![0; width];
    // Spans started by THIS row; applied only after the current row's carried
    // spans are decremented, so a cell never covers its own row.
    let mut new_spans: Vec<usize> = vec![0; width];
    // Running total of emitted cells across all output rows (memory bound).
    let mut total_cells = 0usize;

    for row in rows {
        new_spans.iter_mut().for_each(|s| *s = 0);
        let mut new_cells: Vec<HtmlCell> = Vec::new();
        let mut col = 0usize;
        let mut src = row.cells.iter();
        let mut next = src.next();

        // Materialize columns left-to-right until the row reaches the grid width.
        // The width cap is what makes the per-row work bounded; the original loop
        // terminated on `next == None` with the column not covered, which for
        // realistic tables coincides with `col == width`.
        while col < width {
            if total_cells >= MAX_TABLE_CELLS {
                break;
            }
            if pending[col] > 0 {
                // A vertical span from an earlier row covers this column.
                new_cells.push(HtmlCell {
                    colspan: 1,
                    rowspan: 1,
                    ..Default::default()
                });
                total_cells += 1;
                col += 1;
                continue;
            }
            match next {
                Some(cell) => {
                    let colspan = cell.colspan.max(1);
                    if cell.rowspan > 1 {
                        for k in 0..colspan {
                            let c = col + k;
                            if c < width {
                                new_spans[c] = cell.rowspan - 1;
                            }
                        }
                    }
                    let mut c = cell.clone();
                    c.rowspan = 1;
                    new_cells.push(c);
                    total_cells += 1;
                    col += colspan;
                    next = src.next();
                }
                // No carried span here and no source cells left: the row is
                // complete (matches the original loop's `None` break). Trailing
                // carried columns past the last real cell are still filled above
                // because `pending[col] > 0` is checked before `next`.
                None => break,
            }
        }

        // Every carried span covered exactly this row: decrement and drop the
        // exhausted ones, then layer in the spans started by this row.
        for (p, n) in pending.iter_mut().zip(new_spans.iter()) {
            *p = p.saturating_sub(1).max(*n);
        }

        out.push(HtmlRow {
            cells: new_cells,
            is_header_row: row.is_header_row,
        });

        if total_cells >= MAX_TABLE_CELLS {
            // Truncate the remaining rows: emit them empty so downstream row
            // count stays consistent without further allocation. This drops cell
            // content, so report it to the caller.
            for r in &rows[out.len()..] {
                out.push(HtmlRow {
                    cells: Vec::new(),
                    is_header_row: r.is_header_row,
                });
            }
            return (out, true);
        }
    }
    (out, false)
}

/// Authoritative grid width of an HTML table: the max over rows of summed colspan.
///
/// Bounded by `MAX_TABLE_COLS` so an extreme colspan cannot drive unbounded
/// column allocation downstream.
fn html_grid_width(rows: &[HtmlRow]) -> usize {
    rows.iter()
        .map(|r| r.cells.iter().map(|c| c.colspan.max(1)).sum::<usize>())
        .max()
        .unwrap_or(0)
        .min(MAX_TABLE_COLS)
}

/// Whether any cell in the row contains a nested table.
///
/// A nested table cannot live inside a single GFM cell, so a table containing one
/// is linearized (its cells' text and nested tables emitted as standalone blocks)
/// rather than rendered as a grid.
fn html_row_has_nested_table(row: &HtmlRow) -> bool {
    row.cells.iter().any(|c| c.has_nested_table())
}

/// Expand a row's cells across `grid_width` columns for GFM rendering (empty-fill).
fn expand_html_row(row: &HtmlRow, grid_width: usize) -> Vec<String> {
    let mut cols: Vec<String> = Vec::with_capacity(grid_width);
    for c in &row.cells {
        let span = c.colspan.max(1);
        cols.push(c.joined_text());
        for _ in 1..span {
            cols.push(String::new());
        }
    }
    cols.resize(grid_width, String::new());
    cols
}

/// Render a contiguous run of tiling rows as one GFM table (row 0 = header).
fn render_html_grid(rows: &[&HtmlRow], grid_width: usize, plain: bool) -> String {
    let expanded: Vec<Vec<String>> = rows
        .iter()
        .map(|r| expand_html_row(r, grid_width))
        .collect();
    let headers: Vec<&str> = expanded[0].iter().map(|s| s.as_str()).collect();
    let data: Vec<Vec<&str>> = expanded[1..]
        .iter()
        .map(|r| r.iter().map(|s| s.as_str()).collect())
        .collect();
    if plain {
        markdown::build_table_plain(&headers, &data)
    } else {
        markdown::build_table(&headers, &data)
    }
}

/// Render a completed table collector into a table string.
///
/// Every table — uniform, merged, or layout — renders as a single GFM table of
/// the authoritative grid width: horizontal spans are empty-filled, vertical
/// spans (`rowspan`) are materialized as blank placeholder cells, and the header
/// is the first row marked `is_header_row` (from `<thead>`) if any, else the first
/// row. This is deliberately uniform: a merged/layout table is kept as one table
/// rather than being split into headings and `**Label:** value` lines, so
/// structure stays consistent for downstream (LLM) consumers.
///
/// The one exception is a table that contains a nested table: a nested table
/// cannot live inside a single GFM cell, so such a table is linearized as a WHOLE
/// (per-table, not per-row) — each cell's text and any nested table are emitted as
/// standalone blocks in document order. This whole-table linearization is
/// intentional (consistency over prettiness): mixing a grid for some rows and
/// blocks for others would be less predictable than one uniform block.
///
/// `warnings` collects best-effort truncations (rows/cells dropped by a resource
/// cap) so they are observable rather than silent. Callers pass a throwaway sink
/// for the plain-text render so identical truncations are not counted twice.
fn render_table(tc: &TableCollector, plain: bool, warnings: &mut Vec<ConversionWarning>) -> String {
    let (rows, cells_truncated) = normalize_html_rowspans(&tc.rows);
    if rows.is_empty() {
        return String::new();
    }
    let grid_width = html_grid_width(&rows);
    if grid_width == 0 {
        return String::new();
    }

    // #2: the grid width is capped at MAX_TABLE_COLS, so a row whose real cells sum
    // to more than that has columns dropped. Detect it by comparing the rendered
    // width against the widest row's raw summed colspan.
    let raw_width = tc
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.colspan.max(1)).sum::<usize>())
        .max()
        .unwrap_or(0);
    let width_capped = raw_width > grid_width;

    // A nested table cannot be a grid cell: linearize the whole table, emitting
    // each cell's text and any nested table as standalone blocks in document
    // order. Iterating `segments` preserves text-before / text-after position
    // around a nested table; successive nested tables are separated by a blank
    // line so GFM does not fuse them.
    if rows.iter().any(html_row_has_nested_table) {
        let mut out = String::new();
        for row in &rows {
            for c in &row.cells {
                for seg in &c.segments {
                    match seg {
                        CellSegment::Text(t) => {
                            let text = t.trim();
                            if !text.is_empty() {
                                out.push_str(text);
                                out.push('\n');
                                if !plain {
                                    out.push('\n');
                                }
                            }
                        }
                        CellSegment::Table { md, plain: tp } => {
                            // Guarantee a blank line before the table so adjacent
                            // blocks (text or another nested table) never fuse.
                            if !plain && !out.is_empty() && !out.ends_with("\n\n") {
                                if out.ends_with('\n') {
                                    out.push('\n');
                                } else {
                                    out.push_str("\n\n");
                                }
                            }
                            if plain {
                                out.push_str(tp.trim_end());
                                out.push('\n');
                            } else {
                                out.push_str(md.trim_end());
                                out.push_str("\n\n");
                            }
                        }
                    }
                }
            }
        }
        if plain {
            out.push('\n');
        }
        if !plain {
            push_table_truncation_warnings(warnings, cells_truncated, width_capped, false);
        }
        return out;
    }

    // Common case: one empty-filled GFM grid. The header is the first row marked
    // `is_header_row` (a `<thead>` row, which may not appear first in the source,
    // e.g. `<tbody>` before `<thead>`); all OTHER rows render as the body in their
    // original order. Falls back to row 0 as the header when no row is marked.
    let header_idx = rows.iter().position(|r| r.is_header_row).unwrap_or(0);

    // Bound the rendered area (rows x grid_width) at MAX_TABLE_CELLS: since every
    // cell is escaped and emitted, an extreme grid would otherwise produce
    // gigabytes of markdown. Excess rows are dropped (best-effort truncation).
    let max_rows = (MAX_TABLE_CELLS / grid_width).max(1);
    let rows_dropped = rows.len() > max_rows;

    // Order the rows for rendering: header first, then every other row in source
    // order. Then apply the row cap to the ordered list.
    let mut ordered: Vec<&HtmlRow> = Vec::with_capacity(rows.len());
    ordered.push(&rows[header_idx]);
    for (i, r) in rows.iter().enumerate() {
        if i != header_idx {
            ordered.push(r);
        }
    }
    ordered.truncate(max_rows);

    if !plain {
        push_table_truncation_warnings(warnings, cells_truncated, width_capped, rows_dropped);
    }

    render_html_grid(&ordered, grid_width, plain)
}

/// Append one `ResourceLimitReached` warning per distinct table truncation.
///
/// Best-effort conversion truncates rather than failing on extreme tables; each
/// dropped-content case appends a structured warning so the loss is observable.
fn push_table_truncation_warnings(
    warnings: &mut Vec<ConversionWarning>,
    cells_truncated: bool,
    width_capped: bool,
    rows_dropped: bool,
) {
    if cells_truncated {
        warnings.push(ConversionWarning {
            code: WarningCode::ResourceLimitReached,
            message: format!(
                "table exceeded the {MAX_TABLE_CELLS}-cell limit; trailing rows were truncated"
            ),
            location: None,
        });
    }
    if width_capped {
        warnings.push(ConversionWarning {
            code: WarningCode::ResourceLimitReached,
            message: format!(
                "table row exceeded the {MAX_TABLE_COLS}-column limit; extra columns were dropped"
            ),
            location: None,
        });
    }
    if rows_dropped {
        warnings.push(ConversionWarning {
            code: WarningCode::ResourceLimitReached,
            message: format!(
                "table exceeded the {MAX_TABLE_CELLS}-cell render limit; excess rows were dropped"
            ),
            location: None,
        });
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
    fn test_html_table_colspan_preserved() {
        // A full-width colspan banner over a 2-column data grid. Both data
        // columns must survive (previously only the first was kept).
        let html = r#"<table>
            <tr><td colspan="2">Banner</td></tr>
            <tr><td>A</td><td>B</td></tr>
        </table>"#;
        let result = convert_html(html);
        assert!(result.markdown.contains("A"), "md: {}", result.markdown);
        assert!(result.markdown.contains("B"), "md: {}", result.markdown);
        assert!(
            result.markdown.contains("| A | B |"),
            "both columns missing: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_table_full_width_banner_is_grid_row() {
        // A full-width colspan row stays a grid row (empty-filled), not a heading
        // or bold paragraph.
        let html = r#"<table>
            <tr><td colspan="3">Section</td></tr>
            <tr><td>a</td><td>b</td><td>c</td></tr>
        </table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| Section |  |  |"),
            "banner not an empty-filled header: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("| a | b | c |"),
            "md: {}",
            result.markdown
        );
        assert!(!result.markdown.contains('#'), "md: {}", result.markdown);
        assert!(!result.markdown.contains("**"), "md: {}", result.markdown);
    }

    #[test]
    fn test_html_table_heading_in_banner_inline() {
        // A heading inside a full-width cell keeps its text inline in the grid cell
        // (no `##` promotion).
        let html = r#"<table>
            <tr><td colspan="3"><h2>Section</h2></td></tr>
            <tr><td>a</td><td>b</td><td>c</td></tr>
        </table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| Section |  |  |"),
            "md: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("## Section"),
            "md: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_table_label_value_kept_as_grid_row() {
        // A narrow-label + wide-value row stays a grid row (empty-filled), not a
        // `**Label:** value` line.
        let html = r#"<table>
            <tr><td>Date</td><td colspan="2">Monday</td></tr>
            <tr><td>a</td><td>b</td><td>c</td></tr>
        </table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| Date | Monday |  |"),
            "md: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("**Date:**"),
            "md: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_table_rowspan_preserved() {
        // rowspan on the first column: the second data row's other columns must
        // not shift left into the spanned column.
        let html = r#"<table>
            <tr><td rowspan="2">Merged</td><td>X1</td><td>Y1</td></tr>
            <tr><td>X2</td><td>Y2</td></tr>
        </table>"#;
        let result = convert_html(html);
        for needle in ["Merged", "X1", "Y1", "X2", "Y2"] {
            assert!(
                result.markdown.contains(needle),
                "missing {needle}: {}",
                result.markdown
            );
        }
        // Second row: spanned column is empty, X2/Y2 stay in columns 2 and 3.
        assert!(
            result.markdown.contains("|  | X2 | Y2 |"),
            "rowspan column not preserved: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_nested_table() {
        let html = r#"<table>
            <tr><td colspan="2">Outer</td></tr>
            <tr><td>cell<table><tr><td>inner1</td><td>inner2</td></tr></table></td><td>right</td></tr>
        </table>"#;
        let result = convert_html(html);
        for needle in ["Outer", "inner1", "inner2", "right"] {
            assert!(
                result.markdown.contains(needle),
                "missing {needle}: {}",
                result.markdown
            );
        }
        // The inner table renders as a real GFM table, not escaped into a cell.
        assert!(
            result.markdown.contains("| inner1 | inner2 |"),
            "inner table mangled: {}",
            result.markdown
        );
        // No escaped pipes leaking from a crammed nested table.
        assert!(
            !result.markdown.contains("\\|"),
            "escaped pipes leaked: {}",
            result.markdown
        );
        // Plain text must not contain raw GFM pipe rows from the inner table.
        assert!(
            !result.plain_text.contains("|---|"),
            "plain text leaked table syntax: {:?}",
            result.plain_text
        );
    }

    #[test]
    fn test_html_nested_table_in_simple_outer() {
        // A nested table inside an otherwise-uniform (no-span) outer table must
        // still render as a standalone block, not be flattened into one cell.
        let html = r#"<table><tr><td>a<table><tr><td>x</td><td>y</td></tr></table></td><td>b</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| x | y |"),
            "inner table mangled: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("<br>") && !result.markdown.contains("\\|"),
            "nested table crammed into a cell: {}",
            result.markdown
        );
        assert!(
            !result.plain_text.contains('|'),
            "plain text leaked pipes: {:?}",
            result.plain_text
        );
    }

    #[test]
    fn test_html_rowspan_colspan_no_empty_label() {
        // A rowspan placeholder followed by a colspan value must not be rendered as
        // an empty-label `**:**` line.
        let html = r#"<table><tr><td rowspan="2">R</td><td>a</td></tr><tr><td colspan="2">wide</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            !result.markdown.contains("**:**"),
            "empty-label artifact: {}",
            result.markdown
        );
        assert!(result.markdown.contains("wide"), "md: {}", result.markdown);
    }

    #[test]
    fn test_html_label_colons_preserved_in_grid_cell() {
        // A label cell keeps its text verbatim (colons included) as a grid cell.
        let html = r#"<table><tr><td>Ratio::</td><td colspan="2">val</td></tr><tr><td>p</td><td>q</td><td>r</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| Ratio:: | val |  |"),
            "label not a grid cell: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_multi_row_thead_preserved() {
        // A two-row <thead> must not silently drop the second header row.
        let html = r#"<table><thead><tr><th>H1a</th><th>H1b</th></tr><tr><th>H2a</th><th>H2b</th></tr></thead><tbody><tr><td>d1</td><td>d2</td></tr></tbody></table>"#;
        let result = convert_html(html);
        for needle in ["H1a", "H1b", "H2a", "H2b", "d1", "d2"] {
            assert!(
                result.markdown.contains(needle),
                "header/data dropped: missing {needle}: {}",
                result.markdown
            );
        }
    }

    #[test]
    fn test_html_multi_heading_banner_separated() {
        // Two headings in one banner cell must not mash into "FirstSecond".
        let html = r#"<table><tr><td colspan="2"><h1>First</h1><h3>Second</h3></td></tr><tr><td>a</td><td>b</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            !result.markdown.contains("FirstSecond"),
            "headings mashed: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("First Second"),
            "md: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_colspan_bounded_no_dos() {
        // A huge colspan in a tiling row must not allocate unbounded output.
        let html = r#"<table><tr><td colspan="1000000">X</td><td>Y</td></tr><tr><td colspan="1000000">P</td><td>Q</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.len() < 100_000,
            "output not bounded: {} bytes",
            result.markdown.len()
        );
        assert!(result.markdown.contains('X'));
    }

    #[test]
    fn test_html_rowspan_colspan_product_bounded_no_dos() {
        // A single <td rowspan=N colspan=N> followed by N-1 empty <tr> rows would,
        // before the MAX_TABLE_CELLS cap, materialize ~N*N placeholder cells
        // (~16.7M for N=4096) via normalize_html_rowspans. This exercises the
        // ACTUAL normalize/expand path (the colspan-only DoS test hits Fallback and
        // never expands). The result must be produced quickly and stay bounded.
        let n = 4096;
        let mut html = String::from("<table><tr>");
        html.push_str(&format!(r#"<td rowspan="{n}" colspan="{n}">RC</td>"#));
        html.push_str("</tr>");
        for _ in 1..n {
            html.push_str("<tr></tr>");
        }
        html.push_str("</table>");

        let start = std::time::Instant::now();
        let result = convert_html(&html);
        let elapsed = start.elapsed();

        // Without the MAX_TABLE_CELLS cap this would materialize ~16.7M cells
        // (~1.6 GB) and, since every table renders as one GFM grid, escape and
        // join all of them into gigabytes of markdown. The cap bounds the cell
        // count (~MAX_TABLE_CELLS), so both the materialized cells and the rendered
        // grid stay bounded. The speed assertion below is the real DoS guard (no
        // quadratic O(rows*width) scan).
        assert!(
            result.markdown.len() < 8_000_000,
            "output not bounded: {} bytes",
            result.markdown.len()
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "normalize too slow: {elapsed:?}"
        );
        assert!(
            result.markdown.contains("RC"),
            "content dropped: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_rowspan_colspan_same_cell_grid() {
        // A cell that spans 2 rows AND 2 columns simultaneously. The header row
        // shows the real cell, one in-cell colspan blank, then the trailing cell;
        // the data row shows two rowspan placeholders, then its own cell.
        let html = r#"<table><tr><td rowspan="2" colspan="2">RC</td><td>X</td></tr><tr><td>Y</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| RC |  | X |"),
            "header grid wrong: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("|  |  | Y |"),
            "data grid wrong: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_plain_linearized_flattened() {
        let html = r#"<table>
            <tr><td colspan="3">Section</td></tr>
            <tr><td>Field</td><td colspan="2">Value</td></tr>
            <tr><td>A</td><td>B</td><td>C</td></tr>
        </table>"#;
        let result = convert_html(html);
        // Plain text: no markdown markers in linearized parts.
        assert!(
            result.plain_text.contains("Section"),
            "plain: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Field"),
            "plain: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Value"),
            "plain: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains("**"),
            "plain has markers: {}",
            result.plain_text
        );
        // Grid region tab-separated.
        assert!(
            result.plain_text.contains("A\tB\tC"),
            "plain grid not tabbed: {}",
            result.plain_text
        );
    }

    #[test]
    fn test_html_simple_table_unchanged_deterministic() {
        // A simple uniform table must use the historical GFM path and be stable.
        let html = r#"<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>"#;
        let r1 = convert_html(html);
        let r2 = convert_html(html);
        assert_eq!(r1.markdown, r2.markdown);
        assert!(r1.markdown.contains("| a | b |"));
        assert!(r1.markdown.contains("| c | d |"));
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

    // ---- Table review-follow-up tests ----

    #[test]
    fn test_html_thead_after_tbody_is_header() {
        // Legal HTML: a <tbody> row may appear before the <thead> row. The GFM
        // header must be the <thead> row, not whichever row came first.
        let html = r#"<table><tbody><tr><td>d1</td><td>d2</td></tr></tbody><thead><tr><th>H1</th><th>H2</th></tr></thead></table>"#;
        let result = convert_html(html);
        // Header row (with separator immediately after) must be the thead row.
        assert!(
            result.markdown.contains("| H1 | H2 |\n|---|---|"),
            "thead row not used as header: {}",
            result.markdown
        );
        // The tbody row is the body, below the separator.
        assert!(
            result.markdown.contains("| d1 | d2 |"),
            "data row missing: {}",
            result.markdown
        );
        // Header must come before data.
        let h = result.markdown.find("H1").unwrap();
        let d = result.markdown.find("d1").unwrap();
        assert!(h < d, "header not above body: {}", result.markdown);
    }

    #[test]
    fn test_html_multi_row_thead_first_is_header_rest_body() {
        // A two-row <thead>: only the FIRST header row can be the GFM header; the
        // second header row must appear as the first body row (not dropped, not
        // promoted), followed by the tbody row. Nothing is lost.
        let html = r#"<table><thead><tr><th>H1a</th><th>H1b</th></tr><tr><th>H2a</th><th>H2b</th></tr></thead><tbody><tr><td>d1</td><td>d2</td></tr></tbody></table>"#;
        let result = convert_html(html);
        // First thead row is the header.
        assert!(
            result.markdown.contains("| H1a | H1b |\n|---|---|"),
            "first header row not the GFM header: {}",
            result.markdown
        );
        // Nothing lost.
        for needle in ["H1a", "H1b", "H2a", "H2b", "d1", "d2"] {
            assert!(
                result.markdown.contains(needle),
                "missing {needle}: {}",
                result.markdown
            );
        }
        // Body order: second header row, then the tbody row.
        let h2 = result.markdown.find("H2a").unwrap();
        let d1 = result.markdown.find("d1").unwrap();
        assert!(
            h2 < d1,
            "second header row not before tbody row: {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_nested_table_text_order_preserved() {
        // Text before AND after a nested table in one cell must keep document order
        // (outer, then the inner table, then after) and must NOT be glued
        // ("outerafter").
        let html = r#"<table><tr><td>outer<table><tr><td>x</td><td>y</td></tr></table>after</td><td>z</td></tr></table>"#;
        let result = convert_html(html);
        // The inner table is a standalone GFM block.
        assert!(
            result.markdown.contains("| x | y |"),
            "inner table not a standalone block: {}",
            result.markdown
        );
        // Not glued.
        assert!(
            !result.markdown.contains("outerafter"),
            "outer and after were glued: {}",
            result.markdown
        );
        // Source order: outer < inner table < after.
        let outer = result.markdown.find("outer").unwrap();
        let inner = result.markdown.find("| x | y |").unwrap();
        let after = result.markdown.find("after").unwrap();
        assert!(
            outer < inner && inner < after,
            "document order not preserved: {}",
            result.markdown
        );
        // Plain text keeps order too and leaks no pipes.
        let p_outer = result.plain_text.find("outer").unwrap();
        let p_after = result.plain_text.find("after").unwrap();
        assert!(
            p_outer < p_after,
            "plain order wrong: {:?}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains('|'),
            "plain leaked pipes: {:?}",
            result.plain_text
        );
    }

    #[test]
    fn test_html_sibling_nested_tables_not_fused() {
        // Two nested tables in one cell must render as two separate GFM tables,
        // separated by a blank line so GFM does not fuse them into one.
        let html = r#"<table><tr><td><table><tr><td>a1</td><td>a2</td></tr></table><table><tr><td>b1</td><td>b2</td></tr></table></td><td>z</td></tr></table>"#;
        let result = convert_html(html);
        assert!(
            result.markdown.contains("| a1 | a2 |"),
            "first inner table missing: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("| b1 | b2 |"),
            "second inner table missing: {}",
            result.markdown
        );
        // A blank line must separate the two tables. Each single-row inner table
        // renders as a header row plus a `|---|---|` separator (no data rows), so
        // the boundary is the first table's separator, a blank line, then the
        // second table's header — they are not fused into one GFM table.
        assert!(
            result.markdown.contains("|---|---|\n\n| b1 | b2 |"),
            "sibling nested tables fused (no blank line between): {}",
            result.markdown
        );
    }

    #[test]
    fn test_html_oversized_table_rows_dropped_warns() {
        // A table with far more cells than MAX_TABLE_CELLS (here: a wide grid with
        // many rows) must drop excess rows AND record a ResourceLimitReached
        // warning so the truncation is observable, not silent.
        let cols = 1000usize;
        let rows = 200usize; // cols * rows = 200_000 > MAX_TABLE_CELLS (100_000)
        let mut html = String::from("<table>");
        for _ in 0..rows {
            html.push_str("<tr>");
            for c in 0..cols {
                html.push_str("<td>");
                html.push_str(&c.to_string());
                html.push_str("</td>");
            }
            html.push_str("</tr>");
        }
        html.push_str("</table>");
        let result = convert_html(&html);
        assert!(
            !result.warnings.is_empty(),
            "no warning for truncated oversized table"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "warning code not ResourceLimitReached: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_html_rowspan_colspan_product_truncation_warns() {
        // The rowspan x colspan product cap in normalize_html_rowspans drops
        // trailing rows' cells; that truncation must surface a warning.
        let n = 4096;
        let mut html = String::from("<table><tr>");
        html.push_str(&format!(r#"<td rowspan="{n}" colspan="{n}">RC</td>"#));
        html.push_str("</tr>");
        for _ in 1..n {
            html.push_str("<tr></tr>");
        }
        html.push_str("</table>");
        let result = convert_html(&html);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "no ResourceLimitReached warning for product-capped table: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_html_overwide_row_columns_dropped_warns() {
        // A row whose real cells sum past MAX_TABLE_COLS (4096) is clipped by the
        // width cap, dropping trailing real cells; that loss must surface a
        // warning (#2). Here the first cell spans the whole cap, so the second
        // real cell ("dropped") is truncated away.
        let cap = 4096;
        let html = format!(
            r#"<table><tr><td colspan="{cap}">wide</td><td>dropped</td></tr><tr><td>a</td><td>b</td></tr></table>"#
        );
        let result = convert_html(&html);
        // The trailing real cell is gone (width-clipped).
        assert!(
            !result.markdown.contains("dropped"),
            "trailing cell unexpectedly survived: {}",
            &result.markdown[..result.markdown.len().min(200)]
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "no ResourceLimitReached warning for over-width table: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_html_normal_table_no_warnings() {
        // A realistic table must produce NO warnings (truncation warnings only fire
        // on resource-cap hits).
        let html = r#"<table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>"#;
        let result = convert_html(html);
        assert!(
            result.warnings.is_empty(),
            "unexpected warnings on a normal table: {:?}",
            result.warnings
        );
    }
}
