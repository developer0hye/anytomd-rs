//! DOCX (Office Open XML) to Markdown converter.
//!
//! Parses DOCX files directly from their OOXML ZIP structure using `zip` + `quick-xml`,
//! without intermediate HTML conversion. Extracts headings, paragraphs, tables,
//! bold/italic, hyperlinks, lists, embedded images, and text boxes (`w:pict` /
//! `v:textbox` / `w:txbxContent`). Text boxes wrapped in `mc:AlternateContent` are
//! handled by skipping the `mc:Choice` branch and processing `mc:Fallback` (VML).

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use quick_xml::Reader;
use quick_xml::events::Event;
use zip::ZipArchive;

use crate::converter::comments::{self, Comment};
use crate::converter::ooxml_utils::{
    ImageInfo, PendingImageResolution, Relationship, parse_relationships,
    resolve_image_placeholders, resolve_relative_to_file,
};
use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::{
    build_table, build_table_plain, format_heading, format_list_item, format_list_item_plain,
    wrap_formatting,
};
use crate::zip_utils::{read_zip_bytes, read_zip_text, read_zip_text_lossy};

/// Converts DOCX files to Markdown.
pub struct DocxConverter;

// ---- Data types ----

/// The kind of block element a paragraph represents.
#[derive(Debug, Clone, PartialEq)]
enum ParagraphKind {
    Normal,
    Heading(u8), // level 1..=6
    ListItem {
        ordered: bool,
        level: u8,
        num_id: String,
    }, // list item from numbering
}

/// A numbering level definition from numbering.xml.
#[derive(Debug, Clone)]
struct NumberingLevel {
    ordered: bool,
}

// ---- Styles parsing ----

/// Parse styles.xml to extract a mapping from style ID to heading level.
fn parse_styles(xml: &str) -> HashMap<String, u8> {
    let mut styles = HashMap::new();
    let mut reader = Reader::from_str(xml);

    let mut current_style_id: Option<String> = None;
    let mut current_heading_level: Option<u8> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "style" {
                    current_style_id = None;
                    current_heading_level = None;
                    for attr in e.attributes().flatten() {
                        let local_name = attr.key.local_name();
                        let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                        if attr_local == "styleId" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            if let Some(level) = extract_heading_level_from_id(&val) {
                                current_heading_level = Some(level);
                            }
                            current_style_id = Some(val);
                        }
                    }
                } else if local_str == "name" && current_style_id.is_some() {
                    for attr in e.attributes().flatten() {
                        let local_name = attr.key.local_name();
                        let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                        if attr_local == "val" {
                            let val = String::from_utf8_lossy(&attr.value);
                            if let Some(level) = extract_heading_level_from_name(&val) {
                                current_heading_level = Some(level);
                            }
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if local_str == "style" {
                    if let (Some(id), Some(level)) =
                        (current_style_id.take(), current_heading_level.take())
                    {
                        styles.insert(id, level);
                    }
                    current_style_id = None;
                    current_heading_level = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    styles
}

/// Extract heading level from a style ID like "Heading1", "Heading2", etc.
fn extract_heading_level_from_id(style_id: &str) -> Option<u8> {
    let lower = style_id.to_ascii_lowercase();
    lower
        .strip_prefix("heading")
        .and_then(|rest| rest.parse::<u8>().ok())
        .filter(|&l| (1..=9).contains(&l))
}

/// Extract heading level from a style name like "heading 1", "Heading 2", etc.
fn extract_heading_level_from_name(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let trimmed = lower.trim();
    if let Some(rest) = trimmed.strip_prefix("heading") {
        rest.trim()
            .parse::<u8>()
            .ok()
            .filter(|&l| (1..=9).contains(&l))
    } else {
        None
    }
}

// ---- Numbering parsing ----

/// Parse numbering.xml to extract numbering definitions.
///
/// Returns a mapping from (numId, level) to NumberingLevel.
/// Handles the indirection: numId → abstractNumId → level definitions.
fn parse_numbering(xml: &str) -> HashMap<(String, u8), NumberingLevel> {
    let mut reader = Reader::from_str(xml);

    // abstractNumId -> Vec<(level, ordered)>
    let mut abstract_defs: HashMap<String, Vec<(u8, bool)>> = HashMap::new();
    // numId -> abstractNumId
    let mut num_to_abstract: HashMap<String, String> = HashMap::new();

    let mut current_abstract_id: Option<String> = None;
    let mut current_lvl: Option<u8> = None;
    let mut in_abstract_num = false;
    let mut in_lvl = false;
    let mut in_num = false;
    let mut current_num_id: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                match local_str {
                    "abstractNum" => {
                        in_abstract_num = true;
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if k == "abstractNumId" {
                                let id = String::from_utf8_lossy(&attr.value).to_string();
                                current_abstract_id = Some(id.clone());
                                abstract_defs.entry(id).or_default();
                            }
                        }
                    }
                    "lvl" if in_abstract_num => {
                        in_lvl = true;
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if k == "ilvl" {
                                current_lvl =
                                    String::from_utf8_lossy(&attr.value).parse::<u8>().ok();
                            }
                        }
                    }
                    "numFmt" if in_lvl => {
                        if let (Some(abs_id), Some(lvl)) = (&current_abstract_id, current_lvl) {
                            for attr in e.attributes().flatten() {
                                let local_name = attr.key.local_name();
                                let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                                if k == "val" {
                                    let fmt = String::from_utf8_lossy(&attr.value).to_string();
                                    let ordered = is_ordered_format(&fmt);
                                    abstract_defs
                                        .entry(abs_id.clone())
                                        .or_default()
                                        .push((lvl, ordered));
                                }
                            }
                        }
                    }
                    "num" => {
                        in_num = true;
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if k == "numId" {
                                current_num_id =
                                    Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                    "abstractNumId" if in_num => {
                        if let Some(ref num_id) = current_num_id {
                            for attr in e.attributes().flatten() {
                                let local_name = attr.key.local_name();
                                let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                                if k == "val" {
                                    let abs_id = String::from_utf8_lossy(&attr.value).to_string();
                                    num_to_abstract.insert(num_id.clone(), abs_id);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                match local_str {
                    "abstractNum" => {
                        in_abstract_num = false;
                        current_abstract_id = None;
                    }
                    "lvl" => {
                        in_lvl = false;
                        current_lvl = None;
                    }
                    "num" => {
                        in_num = false;
                        current_num_id = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // Build final mapping: (numId, level) -> NumberingLevel
    let mut result: HashMap<(String, u8), NumberingLevel> = HashMap::new();
    for (num_id, abs_id) in &num_to_abstract {
        if let Some(levels) = abstract_defs.get(abs_id) {
            for &(lvl, ordered) in levels {
                result.insert((num_id.clone(), lvl), NumberingLevel { ordered });
            }
        }
    }

    result
}

/// Determine if a numFmt value represents an ordered (numbered) list.
fn is_ordered_format(fmt: &str) -> bool {
    matches!(
        fmt,
        "decimal" | "upperRoman" | "lowerRoman" | "upperLetter" | "lowerLetter" | "decimalZero"
    )
}

// ---- Run segment merging ----

/// A segment of text within a run, with formatting info.
#[derive(Debug, Clone)]
struct RunSegment {
    text: String,
    bold: bool,
    italic: bool,
}

/// Saved paragraph-level state for text box context save/restore.
///
/// When entering `<w:txbxContent>`, the current paragraph state is saved and reset
/// so that inner `<w:p>` elements can be processed normally. On exit, the state is
/// restored to continue the outer paragraph.
#[derive(Debug, Clone)]
struct SavedParagraphState {
    in_paragraph: bool,
    in_run: bool,
    in_text: bool,
    in_run_properties: bool,
    current_para_kind: ParagraphKind,
    current_para_runs: Vec<RunSegment>,
    current_para_runs_plain: Vec<RunSegment>,
    current_run_bold: bool,
    current_run_italic: bool,
    in_hyperlink: bool,
    current_hyperlink_url: Option<String>,
    hyperlink_runs: Vec<RunSegment>,
    hyperlink_runs_plain: Vec<RunSegment>,
    in_para_properties: bool,
    in_num_pr: bool,
    current_num_id: Option<String>,
    current_ilvl: Option<u8>,
    /// Whether the enclosing table frame had an open cell/row when the text box
    /// was entered. Text-box content is a separate flow, so these are cleared on
    /// entry (so the content does not leak into the surrounding cell) and
    /// restored on exit.
    table_in_cell: bool,
    table_in_row: bool,
}

// ---- Table model ----

/// Vertical-merge state of a table cell (`<w:vMerge>`).
#[derive(Debug, Clone, Default, PartialEq)]
enum VMerge {
    /// Not vertically merged.
    #[default]
    None,
    /// Top cell of a vertical merge (`<w:vMerge w:val="restart"/>`).
    Restart,
    /// Continuation cell of a vertical merge (a bare `<w:vMerge/>`).
    Continue,
}

/// One paragraph of content buffered inside a table cell.
///
/// Cells may contain multiple paragraphs; keeping them as separate blocks lets
/// the renderer preserve paragraph breaks when linearizing a table that contains
/// a nested table. Markdown form retains bold/italic/link/image syntax; plain
/// form has none.
#[derive(Debug, Clone)]
struct DocxCellBlock {
    /// Markdown form of the paragraph (untrimmed, as produced by the run merge).
    md: String,
    /// Plain-text form of the paragraph (untrimmed, no markdown markers).
    plain: String,
    /// True when this block is a fully-rendered nested table (multi-line GFM).
    /// Such blocks cannot be collapsed into a single grid cell, so any table that
    /// contains one is linearized and the nested table emitted as a standalone block.
    is_table: bool,
}

/// A buffered table cell with horizontal/vertical span info and content blocks.
#[derive(Debug, Clone)]
struct DocxCell {
    /// Grid columns this cell spans (`<w:gridSpan w:val>`); always at least 1.
    grid_span: usize,
    /// Vertical-merge state (`<w:vMerge>`).
    v_merge: VMerge,
    /// Content paragraphs, in document order.
    blocks: Vec<DocxCellBlock>,
}

impl Default for DocxCell {
    fn default() -> Self {
        Self {
            grid_span: 1,
            v_merge: VMerge::None,
            blocks: Vec::new(),
        }
    }
}

/// A buffered table row.
#[derive(Debug, Clone, Default)]
struct DocxRow {
    /// Cells in document order.
    cells: Vec<DocxCell>,
}

/// A fully buffered table plus its authoritative grid width.
///
/// Tables are buffered in full before rendering because layout classification
/// needs every row in hand. A stack of these supports nested tables: an inner
/// `<w:tbl>` pushes a new frame and pops on its end tag.
#[derive(Debug, Clone, Default)]
struct DocxTable {
    /// Completed rows, in document order.
    rows: Vec<DocxRow>,
    /// Grid column count from `<w:tblGrid>`/`<w:gridCol>` (0 if absent/unparsed).
    grid_width: usize,
    /// In-progress row being accumulated.
    current_row: DocxRow,
    /// In-progress cell being accumulated.
    current_cell: DocxCell,
    /// Whether a `<w:tr>` is currently open in this frame.
    in_row: bool,
    /// Whether a `<w:tc>` is currently open in this frame.
    in_cell: bool,
}

/// Flatten a cell's paragraph blocks into one Markdown string.
///
/// Reproduces the historical join: paragraphs are trimmed and concatenated, with
/// a single space inserted before each non-empty paragraph after the first; the
/// whole result is then trimmed.
fn join_cell_blocks_md(cell: &DocxCell) -> String {
    let mut s = String::new();
    for (i, b) in cell.blocks.iter().enumerate() {
        if i > 0 && !b.md.is_empty() {
            s.push(' ');
        }
        s.push_str(b.md.trim());
    }
    s.trim().to_string()
}

/// Flatten a cell's paragraph blocks into one plain-text string.
///
/// Mirrors [`join_cell_blocks_md`] but uses the plain form and checks plain
/// emptiness for space insertion (matching the historical plain-text join).
fn join_cell_blocks_plain(cell: &DocxCell) -> String {
    let mut s = String::new();
    for (i, b) in cell.blocks.iter().enumerate() {
        if i > 0 && !b.plain.is_empty() {
            s.push(' ');
        }
        s.push_str(b.plain.trim());
    }
    s.trim().to_string()
}

/// Expand a row's cells across `grid_width` columns for GFM rendering.
///
/// A cell with `grid_span = k` places its (joined) text in the first of its `k`
/// columns and leaves the other `k - 1` empty (empty-fill). Vertical-merge
/// continuation cells render empty. The `plain` flag selects the plain-text
/// join. The result always has exactly `grid_width` entries. Because
/// `docx_grid_width` treats `tblGrid` as a floor (it is at least the widest row),
/// the trailing `resize` only ever PADS short rows — it never truncates a real
/// cell. The per-cell span clamp remains only as defense against a single
/// malformed span on a non-widest row.
fn expand_row_to_grid(row: &DocxRow, grid_width: usize, plain: bool) -> Vec<String> {
    let mut cols: Vec<String> = Vec::with_capacity(grid_width);
    for c in &row.cells {
        // Clamp the span to the columns still available so a malformed span value
        // can never push the row past `grid_width`.
        let remaining = grid_width.saturating_sub(cols.len());
        let span = c.grid_span.max(1).min(remaining.max(1));
        let content = if plain {
            join_cell_blocks_plain(c)
        } else {
            join_cell_blocks_md(c)
        };
        // A genuine vertical-merge continuation cell is empty in the source, so it
        // renders empty. But if a continuation cell carries text (an orphan merge
        // with no matching restart, or malformed input), keep it rather than
        // silently dropping content.
        let text = if c.v_merge == VMerge::Continue && content.is_empty() {
            String::new()
        } else {
            content
        };
        cols.push(text);
        for _ in 1..span {
            cols.push(String::new());
        }
    }
    cols.resize(grid_width, String::new());
    cols
}

/// Build a GFM table (markdown + plain) from a slice of rows, expanding spans.
///
/// The first row supplies the header; remaining rows are data. Each row is
/// expanded to `grid_width` columns via [`expand_row_to_grid`].
fn render_docx_grid(rows: &[&DocxRow], grid_width: usize) -> (String, String) {
    let md_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| expand_row_to_grid(r, grid_width, false))
        .collect();
    let plain_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| expand_row_to_grid(r, grid_width, true))
        .collect();

    let headers: Vec<&str> = md_rows[0].iter().map(|s| s.as_str()).collect();
    let data: Vec<Vec<&str>> = md_rows[1..]
        .iter()
        .map(|r| r.iter().map(|s| s.as_str()).collect())
        .collect();
    let md = build_table(&headers, &data);

    let headers_p: Vec<&str> = plain_rows[0].iter().map(|s| s.as_str()).collect();
    let data_p: Vec<Vec<&str>> = plain_rows[1..]
        .iter()
        .map(|r| r.iter().map(|s| s.as_str()).collect())
        .collect();
    let plain = build_table_plain(&headers_p, &data_p);

    (md, plain)
}

/// Render a buffered table to `(markdown, plain_text)`.
///
/// Every table — uniform, merged, or layout — renders as a single GFM table of
/// the authoritative grid width: horizontal spans are empty-filled, vertical-merge
/// continuations are blank, and the first row is the header. This is deliberately
/// uniform: a merged/layout table is kept as one table rather than being split
/// into headings and `**Label:** value` lines, so structure stays consistent for
/// downstream (LLM) consumers.
///
/// The one exception is a table that contains a nested table: a nested table
/// cannot live inside a single GFM cell, so the whole table is linearized — each
/// cell's paragraphs and any nested table are emitted as standalone blocks. This
/// is per-table (not per-row) by deliberate maintainer decision: consistency over
/// prettiness. Only bugs within that path are fixed; the linearization is kept.
///
/// `warnings` collects [`WarningCode::ResourceLimitReached`] entries if the table
/// is so large that rows (grid path) or blocks (linearize path) are dropped to
/// honor `MAX_TABLE_CELLS`, or columns are dropped to honor `MAX_TABLE_COLS`.
fn render_docx_table(table: &DocxTable, warnings: &mut Vec<ConversionWarning>) -> (String, String) {
    if table.rows.is_empty() {
        return (String::new(), String::new());
    }

    let raw_width = docx_raw_grid_width(table);
    let grid_width = docx_grid_width(table);
    if grid_width == 0 {
        return (String::new(), String::new());
    }

    // Common case: no nested tables → one empty-filled GFM grid. Bound the
    // rendered area (rows × grid_width) at MAX_TABLE_CELLS: every cell is escaped
    // and emitted, so an extreme grid would otherwise produce gigabytes of
    // markdown. Excess rows are dropped — and, because dropping content must be
    // observable, a ResourceLimitReached warning is appended.
    if !table.rows.iter().any(docx_row_has_nested_table) {
        // Capping the width drops trailing columns from the widest row(s); that
        // must be observable too (mirrors the HTML converter's width-cap
        // warning).
        if raw_width > grid_width {
            warnings.push(ConversionWarning {
                code: WarningCode::ResourceLimitReached,
                message: format!(
                    "table row exceeded the {MAX_TABLE_COLS}-column limit; extra columns were dropped"
                ),
                location: None,
            });
        }
        let max_rows = (MAX_TABLE_CELLS / grid_width).max(1);
        if table.rows.len() > max_rows {
            warnings.push(ConversionWarning {
                code: WarningCode::ResourceLimitReached,
                message: format!(
                    "table truncated to {max_rows} rows (limit {MAX_TABLE_CELLS} cells)"
                ),
                location: None,
            });
        }
        let rows: Vec<&DocxRow> = table.rows.iter().take(max_rows).collect();
        return render_docx_grid(&rows, grid_width);
    }

    // A nested table cannot be a grid cell: linearize the whole table, emitting
    // each cell's paragraphs and nested tables as standalone blocks in order.
    // Linearization is per-table (whole table), not per-row, by deliberate
    // maintainer decision (consistency over prettiness); only the bugs below are
    // fixed within this path.
    //
    // Within the table, blocks are separated by one blank line in the markdown
    // stream and one newline in the plain stream so adjacent paragraphs/tables do
    // not fuse. A block is skipped when it has no meaningful content in EITHER
    // stream (both md and plain trim empty), so a block with non-empty md but
    // empty plain (e.g. an empty nested table) cannot inject a phantom blank line
    // into plain or a degenerate empty GFM table into md.
    // The emitted blocks are bounded at MAX_TABLE_CELLS, the same budget the
    // grid path enforces on its rendered cells: without it, a single nested
    // table anywhere in an extreme-row-count table would divert rendering to
    // this branch and bypass the cap entirely. Dropping blocks is observable
    // via a ResourceLimitReached warning.
    let mut md = String::new();
    let mut plain = String::new();
    let mut emitted_blocks = 0usize;
    let mut truncated = false;
    'rows: for row in &table.rows {
        for c in &row.cells {
            for b in &c.blocks {
                let md_block = b.md.trim_end();
                let plain_block = b.plain.trim_end();
                // Skip unless the block carries content in at least one stream.
                if md_block.trim_start().is_empty() && plain_block.trim_start().is_empty() {
                    continue;
                }
                if emitted_blocks >= MAX_TABLE_CELLS {
                    truncated = true;
                    break 'rows;
                }
                emitted_blocks += 1;
                // Never emit a nested table whose rendered markdown is empty:
                // it would otherwise contribute a degenerate empty GFM table.
                if !(b.is_table && md_block.is_empty()) {
                    md.push_str(md_block);
                    md.push_str("\n\n");
                }
                if !(b.is_table && plain_block.is_empty()) {
                    plain.push_str(plain_block);
                    plain.push('\n');
                }
            }
        }
    }
    if truncated {
        warnings.push(ConversionWarning {
            code: WarningCode::ResourceLimitReached,
            message: format!(
                "table linearization truncated to {MAX_TABLE_CELLS} blocks (limit {MAX_TABLE_CELLS} cells)"
            ),
            location: None,
        });
    }

    // Normalize each stream to end in exactly one newline, matching the contract
    // of `build_table`/`build_table_plain` on the normal-grid path: the per-block
    // `"\n\n"` terminator leaves a trailing blank line on the last block, but the
    // outer caller appends one more `\n` after the table. Trimming to a single
    // trailing newline here means the caller's `\n` yields exactly one blank line
    // before the following block — identical to a normal table (no extra blank
    // line, the FINDING #11 regression).
    let md = trim_to_single_trailing_newline(&md);
    let plain = trim_to_single_trailing_newline(&plain);

    (md, plain)
}

/// Trim trailing whitespace and, if any content remains, re-append exactly one
/// `\n`. An all-empty string stays empty. Matches the trailing-newline contract
/// of the normal-grid table builders so callers can append a single `\n`
/// uniformly regardless of which render path produced the table.
fn trim_to_single_trailing_newline(s: &str) -> String {
    let trimmed = s.trim_end();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

// ---- Table classification ----

/// Uncapped grid width of a table: the maximum of the declared `tblGrid` count
/// and the widest row's summed spans. Callers compare this against the
/// `MAX_TABLE_COLS`-capped width to detect (and warn about) dropped columns.
fn docx_raw_grid_width(t: &DocxTable) -> usize {
    let widest_row = t
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.grid_span.max(1)).sum::<usize>())
        .max()
        .unwrap_or(0);
    t.grid_width.max(widest_row)
}

/// Authoritative grid width of a table.
///
/// The `<w:tblGrid>`/`<w:gridCol>` count is a FLOOR, not a ceiling: a row whose
/// cells (by summed `grid_span`) are wider than the declared grid still keeps
/// every cell. The width is therefore the maximum of the `tblGrid` count and the
/// widest row, so `expand_row_to_grid` only ever pads — never truncates — a real
/// cell. (Word occasionally emits a `tblGrid` narrower than an actual row; taking
/// the ceiling there silently dropped trailing cells.) The grid is still bounded
/// by `MAX_TABLE_COLS` so an extreme span/gridCol count cannot drive unbounded
/// column allocation downstream; [`render_docx_table`] warns when that bound
/// actually drops columns.
fn docx_grid_width(t: &DocxTable) -> usize {
    docx_raw_grid_width(t).min(MAX_TABLE_COLS)
}

/// Whether any cell in the row contains a nested-table block.
///
/// A nested table cannot live inside a single GFM cell, so a table containing one
/// is linearized (its cells' paragraphs and nested tables emitted as standalone
/// blocks) rather than rendered as a grid.
fn docx_row_has_nested_table(row: &DocxRow) -> bool {
    row.cells
        .iter()
        .any(|c| c.blocks.iter().any(|b| b.is_table))
}

/// Whether a buffered table has any content worth rendering.
///
/// A table with no cells, or whose every cell block is blank in both the markdown
/// and plain streams, renders only to an empty/skeleton GFM table (e.g.
/// `|  |\n|---|`). Such a table is suppressed entirely when it appears nested
/// inside a cell, so it contributes no degenerate phantom table or stray blank
/// line to the enclosing output.
fn docx_table_has_content(t: &DocxTable) -> bool {
    t.rows.iter().any(|r| {
        r.cells.iter().any(|c| {
            c.blocks
                .iter()
                .any(|b| !b.md.trim().is_empty() || !b.plain.trim().is_empty())
        })
    })
}

/// Merge adjacent segments with the same formatting, then apply `wrap_formatting`
/// once per merged group.
fn merge_and_format_runs(runs: &[RunSegment]) -> String {
    if runs.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    let mut i = 0;
    while i < runs.len() {
        let bold = runs[i].bold;
        let italic = runs[i].italic;
        let mut merged_text = runs[i].text.clone();
        let mut j = i + 1;
        while j < runs.len() && runs[j].bold == bold && runs[j].italic == italic {
            merged_text.push_str(&runs[j].text);
            j += 1;
        }
        result.push_str(&wrap_formatting(&merged_text, bold, italic));
        i = j;
    }

    result
}

/// Concatenate run segments into plain text without any bold/italic formatting.
fn merge_runs_plain(runs: &[RunSegment]) -> String {
    let mut result = String::new();
    for run in runs {
        result.push_str(&run.text);
    }
    result
}

// ---- Document body parsing ----

/// Parse the main document.xml body and produce Markdown and plain text output.
///
/// Returns (markdown, plain_text, title, warnings, image_infos).
/// Images are emitted with unique placeholder alt text `__img_N__`.
/// `image_counter` is incremented for each image to ensure uniqueness.
fn parse_document(
    xml: &str,
    styles: &HashMap<String, u8>,
    relationships: &HashMap<String, Relationship>,
    numbering: &HashMap<(String, u8), NumberingLevel>,
    image_counter: &mut usize,
) -> (
    String,
    String,
    Option<String>,
    Vec<ConversionWarning>,
    Vec<ImageInfo>,
) {
    let mut reader = Reader::from_str(xml);

    let mut warnings = Vec::new();
    let mut output = String::new();
    let mut plain_output = String::new();
    let mut title: Option<String> = None;

    // Paragraph-level state
    let mut in_body = false;
    let mut in_paragraph = false;
    let mut current_para_kind = ParagraphKind::Normal;
    let mut current_para_runs: Vec<RunSegment> = Vec::new();
    // Plain text counterpart: tracks text without markdown link/image syntax
    let mut current_para_runs_plain: Vec<RunSegment> = Vec::new();

    // Run-level state
    let mut in_run = false;
    let mut in_text = false;

    // Run properties state (bold/italic)
    let mut in_run_properties = false;
    let mut current_run_bold = false;
    let mut current_run_italic = false;

    // Hyperlink state
    let mut in_hyperlink = false;
    let mut current_hyperlink_url: Option<String> = None;
    let mut hyperlink_runs: Vec<RunSegment> = Vec::new();
    let mut hyperlink_runs_plain: Vec<RunSegment> = Vec::new();

    // Paragraph properties state (for list detection)
    let mut in_para_properties = false;
    let mut in_num_pr = false;
    let mut current_num_id: Option<String> = None;
    let mut current_ilvl: Option<u8> = None;

    // List counter tracking: (numId, level) -> counter
    let mut list_counters: HashMap<(String, u8), usize> = HashMap::new();
    // Track if last paragraph was a list item (for single-newline separation)
    let mut last_was_list = false;

    // Table state: a stack of buffered tables supports nesting. The top frame
    // is the table currently being parsed; an inner `<w:tbl>` pushes a new frame.
    let mut table_stack: Vec<DocxTable> = Vec::new();

    // Drawing/Image state
    let mut in_drawing = false;
    let mut current_image_alt: Option<String> = None;
    let mut current_image_rel_id: Option<String> = None;

    // Image info tracking for placeholder-based replacement
    let mut image_infos: Vec<ImageInfo> = Vec::new();

    // mc:AlternateContent state: skip Choice, process Fallback
    let mut in_mc_choice = false;
    let mut mc_choice_depth: u32 = 0;

    // Text box state: w:pict > v:shape > v:textbox > w:txbxContent
    let mut in_pict = false;
    let mut in_textbox_content = false;
    let mut saved_paragraph_state: Option<SavedParagraphState> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                // mc:AlternateContent handling: skip Choice, process Fallback
                if in_mc_choice {
                    mc_choice_depth += 1;
                    continue;
                }
                match local_str {
                    "AlternateContent" => {
                        // Just a wrapper — content inside is either Choice or Fallback
                        continue;
                    }
                    "Choice" => {
                        in_mc_choice = true;
                        mc_choice_depth = 1;
                        continue;
                    }
                    "Fallback" => {
                        // Process Fallback content normally — just skip this tag
                        continue;
                    }
                    _ => {}
                }

                // Text box handling: w:pict > ... > w:txbxContent
                match local_str {
                    "pict" if in_run => {
                        in_pict = true;
                        continue;
                    }
                    "txbxContent" if in_pict => {
                        // Save current paragraph state and reset for inner paragraphs
                        saved_paragraph_state = Some(SavedParagraphState {
                            in_paragraph,
                            in_run,
                            in_text,
                            in_run_properties,
                            current_para_kind: current_para_kind.clone(),
                            current_para_runs: current_para_runs.clone(),
                            current_para_runs_plain: current_para_runs_plain.clone(),
                            current_run_bold,
                            current_run_italic,
                            in_hyperlink,
                            current_hyperlink_url: current_hyperlink_url.clone(),
                            hyperlink_runs: hyperlink_runs.clone(),
                            hyperlink_runs_plain: hyperlink_runs_plain.clone(),
                            in_para_properties,
                            in_num_pr,
                            current_num_id: current_num_id.clone(),
                            current_ilvl,
                            table_in_cell: table_stack.last().is_some_and(|f| f.in_cell),
                            table_in_row: table_stack.last().is_some_and(|f| f.in_row),
                        });
                        // A text box is a separate content flow: detach it from any
                        // enclosing table cell so its paragraphs (and nested tables)
                        // render to the main output rather than leaking into the cell.
                        if let Some(frame) = table_stack.last_mut() {
                            frame.in_cell = false;
                            frame.in_row = false;
                        }
                        // Reset paragraph-level state for text box content
                        in_paragraph = false;
                        in_run = false;
                        in_text = false;
                        in_run_properties = false;
                        current_para_kind = ParagraphKind::Normal;
                        current_para_runs.clear();
                        current_para_runs_plain.clear();
                        current_run_bold = false;
                        current_run_italic = false;
                        in_hyperlink = false;
                        current_hyperlink_url = None;
                        hyperlink_runs.clear();
                        hyperlink_runs_plain.clear();
                        in_para_properties = false;
                        in_num_pr = false;
                        current_num_id = None;
                        current_ilvl = None;
                        in_textbox_content = true;
                        continue;
                    }
                    // VML elements inside w:pict are transparent containers
                    "shape" | "rect" | "roundrect" | "textbox" | "group" if in_pict => {
                        continue;
                    }
                    _ => {}
                }

                // Table-property elements (gridCol/gridSpan/vMerge) may be serialized
                // in long form (Start+End) rather than self-closing, so handle them here
                // as well as in the Event::Empty arm.
                if let Some(frame) = table_stack.last_mut()
                    && apply_table_property(local_str, e, frame)
                {
                    continue;
                }

                match local_str {
                    "body" => {
                        in_body = true;
                    }
                    "tbl" if in_body => {
                        // Push a new table frame (supports nested tables).
                        table_stack.push(DocxTable::default());
                    }
                    "tr" if !table_stack.is_empty() => {
                        let frame = table_stack.last_mut().unwrap();
                        frame.in_row = true;
                        frame.current_row = DocxRow::default();
                    }
                    "tc" if table_stack.last().is_some_and(|f| f.in_row) => {
                        let frame = table_stack.last_mut().unwrap();
                        frame.in_cell = true;
                        frame.current_cell = DocxCell::default();
                    }
                    "p" if in_body => {
                        in_paragraph = true;
                        current_para_kind = ParagraphKind::Normal;
                        current_para_runs.clear();
                        current_para_runs_plain.clear();
                        current_num_id = None;
                        current_ilvl = None;
                    }
                    "pPr" if in_paragraph => {
                        in_para_properties = true;
                    }
                    "pStyle" if in_para_properties => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_para_kind = resolve_paragraph_kind(&val, styles);
                            }
                        }
                    }
                    "numPr" if in_para_properties => {
                        in_num_pr = true;
                    }
                    "ilvl" if in_num_pr => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                current_ilvl =
                                    String::from_utf8_lossy(&attr.value).parse::<u8>().ok();
                            }
                        }
                    }
                    "numId" if in_num_pr => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                // numId "0" means no numbering
                                if val != "0" {
                                    current_num_id = Some(val);
                                }
                            }
                        }
                    }
                    "hyperlink" if in_paragraph => {
                        in_hyperlink = true;
                        hyperlink_runs.clear();
                        hyperlink_runs_plain.clear();
                        current_hyperlink_url = None;

                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            if key == "r:id" || key.ends_with(":id") {
                                let rid = String::from_utf8_lossy(&attr.value).to_string();
                                current_hyperlink_url =
                                    resolve_hyperlink_url(&rid, relationships, &mut warnings);
                            }
                        }
                    }
                    "r" if in_paragraph => {
                        in_run = true;
                        current_run_bold = false;
                        current_run_italic = false;
                    }
                    "rPr" if in_run => {
                        in_run_properties = true;
                    }
                    "b" if in_run_properties => {
                        // Bold: <w:b/> or <w:b w:val="true"/>
                        // Check for explicit false
                        current_run_bold = !is_val_false(e);
                    }
                    "i" if in_run_properties => {
                        current_run_italic = !is_val_false(e);
                    }
                    "t" if in_run => {
                        in_text = true;
                    }
                    "drawing" if in_run => {
                        in_drawing = true;
                        current_image_alt = None;
                        current_image_rel_id = None;
                    }
                    "docPr" if in_drawing => {
                        // <wp:docPr descr="Alt text"/>
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if k == "descr" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if !val.is_empty() {
                                    current_image_alt = Some(val);
                                }
                            }
                        }
                    }
                    "blip" if in_drawing => {
                        // <a:blip r:embed="rId5"/>
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            if key == "r:embed" || key.ends_with(":embed") {
                                current_image_rel_id =
                                    Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                if in_mc_choice {
                    continue;
                }
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if let Some(frame) = table_stack.last_mut()
                    && apply_table_property(local_str, e, frame)
                {
                    continue;
                }

                match local_str {
                    "pStyle" if in_para_properties => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_para_kind = resolve_paragraph_kind(&val, styles);
                            }
                        }
                    }
                    "ilvl" if in_num_pr => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                current_ilvl =
                                    String::from_utf8_lossy(&attr.value).parse::<u8>().ok();
                            }
                        }
                    }
                    "numId" if in_num_pr => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if val != "0" {
                                    current_num_id = Some(val);
                                }
                            }
                        }
                    }
                    "b" if in_run_properties => {
                        current_run_bold = !is_val_false(e);
                    }
                    "i" if in_run_properties => {
                        current_run_italic = !is_val_false(e);
                    }
                    "br" if in_run => {
                        let seg = RunSegment {
                            text: "\n".to_string(),
                            bold: false,
                            italic: false,
                        };
                        if in_hyperlink {
                            hyperlink_runs.push(seg.clone());
                            hyperlink_runs_plain.push(seg);
                        } else {
                            current_para_runs.push(seg.clone());
                            current_para_runs_plain.push(seg);
                        }
                    }
                    "hyperlink" if in_paragraph => {
                        // Self-closing hyperlink (unlikely but handle gracefully)
                    }
                    "docPr" if in_drawing => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if k == "descr" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if !val.is_empty() {
                                    current_image_alt = Some(val);
                                }
                            }
                        }
                    }
                    "blip" if in_drawing => {
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            if key == "r:embed" || key.ends_with(":embed") {
                                current_image_rel_id =
                                    Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_mc_choice {
                    continue;
                }
                if in_text && in_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    let seg = RunSegment {
                        text,
                        bold: current_run_bold,
                        italic: current_run_italic,
                    };
                    if in_hyperlink {
                        hyperlink_runs.push(seg.clone());
                        hyperlink_runs_plain.push(seg);
                    } else {
                        current_para_runs.push(seg.clone());
                        current_para_runs_plain.push(seg);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                // mc:Choice depth tracking
                if in_mc_choice {
                    mc_choice_depth -= 1;
                    if mc_choice_depth == 0 {
                        in_mc_choice = false;
                    }
                    continue;
                }

                // mc:AlternateContent and mc:Fallback end tags — just skip
                if local_str == "AlternateContent" || local_str == "Fallback" {
                    continue;
                }

                // Text box end handling
                if local_str == "txbxContent" && in_textbox_content {
                    // Flush any pending paragraph inside the text box
                    // (the normal "p" end handler will have already flushed it,
                    // but guard against edge cases)
                    in_textbox_content = false;
                    // Restore saved paragraph state
                    if let Some(saved) = saved_paragraph_state.take() {
                        in_paragraph = saved.in_paragraph;
                        in_run = saved.in_run;
                        in_text = saved.in_text;
                        in_run_properties = saved.in_run_properties;
                        current_para_kind = saved.current_para_kind;
                        current_para_runs = saved.current_para_runs;
                        current_para_runs_plain = saved.current_para_runs_plain;
                        current_run_bold = saved.current_run_bold;
                        current_run_italic = saved.current_run_italic;
                        in_hyperlink = saved.in_hyperlink;
                        current_hyperlink_url = saved.current_hyperlink_url;
                        hyperlink_runs = saved.hyperlink_runs;
                        hyperlink_runs_plain = saved.hyperlink_runs_plain;
                        in_para_properties = saved.in_para_properties;
                        in_num_pr = saved.in_num_pr;
                        current_num_id = saved.current_num_id;
                        current_ilvl = saved.current_ilvl;
                        // Re-attach to the enclosing table cell, if any.
                        if let Some(frame) = table_stack.last_mut() {
                            frame.in_cell = saved.table_in_cell;
                            frame.in_row = saved.table_in_row;
                        }
                    }
                    continue;
                }
                if local_str == "pict" && in_pict {
                    in_pict = false;
                    continue;
                }
                // VML end tags inside w:pict are transparent
                if in_pict
                    && matches!(
                        local_str,
                        "shape" | "rect" | "roundrect" | "textbox" | "group"
                    )
                {
                    continue;
                }

                match local_str {
                    "body" => {
                        in_body = false;
                    }
                    "tbl" if !table_stack.is_empty() => {
                        let table = table_stack.pop().unwrap();
                        // A content-empty nested/inner table would render only to a
                        // degenerate skeleton (e.g. `|  |\n|---|`); skip it entirely
                        // so it never injects a phantom table into markdown or a
                        // stray blank line into plain text. Skip the render too:
                        // a suppressed table contributes no output, so it must not
                        // contribute a truncation warning either (a warning with
                        // nothing visibly dropped would be a false positive).
                        let has_content = docx_table_has_content(&table);
                        let (table_md, table_plain) = if has_content {
                            render_docx_table(&table, &mut warnings)
                        } else {
                            (String::new(), String::new())
                        };
                        if table_stack.last().is_some_and(|f| f.in_cell) {
                            // Nested table: attach its rendered output to the
                            // enclosing cell as a block so it renders in place.
                            if has_content && !table_md.is_empty() {
                                table_stack.last_mut().unwrap().current_cell.blocks.push(
                                    DocxCellBlock {
                                        md: table_md,
                                        plain: table_plain,
                                        is_table: true,
                                    },
                                );
                            }
                        } else if has_content && !table_md.is_empty() {
                            // Outermost table: write to the document output.
                            output.push_str(&table_md);
                            output.push('\n');
                            plain_output.push_str(&table_plain);
                            plain_output.push('\n');
                        }
                        last_was_list = false;
                    }
                    "tr" if table_stack.last().is_some_and(|f| f.in_row) => {
                        let frame = table_stack.last_mut().unwrap();
                        let row = std::mem::take(&mut frame.current_row);
                        frame.rows.push(row);
                        frame.in_row = false;
                    }
                    "tc" if table_stack.last().is_some_and(|f| f.in_cell) => {
                        let frame = table_stack.last_mut().unwrap();
                        let cell = std::mem::take(&mut frame.current_cell);
                        frame.current_row.cells.push(cell);
                        frame.in_cell = false;
                    }
                    "p" if in_paragraph => {
                        // Resolve list item kind from numPr
                        if let (Some(num_id), Some(ilvl)) = (&current_num_id, current_ilvl) {
                            let key = (num_id.clone(), ilvl);
                            let ordered = numbering.get(&key).map(|nl| nl.ordered).unwrap_or(false); // default to bullet
                            current_para_kind = ParagraphKind::ListItem {
                                ordered,
                                level: ilvl,
                                num_id: num_id.clone(),
                            };
                        }

                        // Merge runs into final paragraph text (markdown with formatting)
                        let current_para_text = merge_and_format_runs(&current_para_runs);
                        // Plain text: no bold/italic markers, no link/image syntax
                        let current_para_text_plain = merge_runs_plain(&current_para_runs_plain);

                        if table_stack.last().is_some_and(|f| f.in_cell) {
                            // In a table cell: buffer this paragraph as a content block.
                            let frame = table_stack.last_mut().unwrap();
                            frame.current_cell.blocks.push(DocxCellBlock {
                                md: current_para_text.clone(),
                                plain: current_para_text_plain.clone(),
                                is_table: false,
                            });
                        } else {
                            // Normal paragraph finalization
                            let is_list =
                                matches!(current_para_kind, ParagraphKind::ListItem { .. });
                            finalize_paragraph(
                                &current_para_kind,
                                &current_para_text,
                                &current_para_text_plain,
                                &mut output,
                                &mut plain_output,
                                &mut title,
                                &mut list_counters,
                                last_was_list,
                            );
                            last_was_list = is_list;
                        }
                        in_paragraph = false;
                        current_para_runs.clear();
                        current_para_runs_plain.clear();
                        current_num_id = None;
                        current_ilvl = None;
                    }
                    "pPr" => {
                        in_para_properties = false;
                    }
                    "numPr" => {
                        in_num_pr = false;
                    }
                    "hyperlink" if in_hyperlink => {
                        let link_text = merge_and_format_runs(&hyperlink_runs);
                        let link_text_plain = merge_runs_plain(&hyperlink_runs_plain);
                        let link_md = if let Some(ref url) = current_hyperlink_url {
                            format!("[{}]({})", link_text, url)
                        } else {
                            link_text
                        };
                        current_para_runs.push(RunSegment {
                            text: link_md,
                            bold: false,
                            italic: false,
                        });
                        // Plain text: just the link text, no URL
                        current_para_runs_plain.push(RunSegment {
                            text: link_text_plain,
                            bold: false,
                            italic: false,
                        });
                        in_hyperlink = false;
                        hyperlink_runs.clear();
                        hyperlink_runs_plain.clear();
                        current_hyperlink_url = None;
                    }
                    "rPr" => {
                        in_run_properties = false;
                    }
                    "r" => {
                        in_run = false;
                        in_text = false;
                        current_run_bold = false;
                        current_run_italic = false;
                    }
                    "t" => {
                        in_text = false;
                    }
                    "drawing" if in_drawing => {
                        // Emit image markdown with unique placeholder
                        if let Some(ref rel_id) = current_image_rel_id {
                            let filename = relationships
                                .get(rel_id)
                                .map(|r| {
                                    // Extract just the filename from path
                                    r.target.rsplit('/').next().unwrap_or(&r.target).to_string()
                                })
                                .unwrap_or_default();

                            if !filename.is_empty() {
                                let original_alt =
                                    current_image_alt.as_deref().unwrap_or("").to_string();
                                let placeholder = format!("__img_{n}__", n = *image_counter);
                                *image_counter += 1;
                                image_infos.push(ImageInfo {
                                    placeholder: placeholder.clone(),
                                    original_alt,
                                    filename: filename.clone(),
                                    bytes_key: rel_id.clone(),
                                });
                                let img_md = format!("![{placeholder}]({filename})");
                                let seg = RunSegment {
                                    text: img_md,
                                    bold: false,
                                    italic: false,
                                };
                                // Plain text: just the placeholder (no image markdown syntax)
                                let seg_plain = RunSegment {
                                    text: placeholder,
                                    bold: false,
                                    italic: false,
                                };
                                if in_hyperlink {
                                    hyperlink_runs.push(seg);
                                    hyperlink_runs_plain.push(seg_plain);
                                } else {
                                    current_para_runs.push(seg);
                                    current_para_runs_plain.push(seg_plain);
                                }
                            } else {
                                warnings.push(ConversionWarning {
                                    code: WarningCode::SkippedElement,
                                    message: format!(
                                        "image relationship '{rel_id}' not found in rels"
                                    ),
                                    location: Some(rel_id.clone()),
                                });
                            }
                        }
                        in_drawing = false;
                        current_image_alt = None;
                        current_image_rel_id = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // Trim trailing newlines to a single trailing newline
    let markdown = output.trim_end().to_string();
    let markdown = if markdown.is_empty() {
        markdown
    } else {
        format!("{}\n", markdown)
    };

    let plain_text = plain_output.trim_end().to_string();
    let plain_text = if plain_text.is_empty() {
        plain_text
    } else {
        format!("{}\n", plain_text)
    };

    (markdown, plain_text, title, warnings, image_infos)
}

/// Upper bound on table columns / cell span.
///
/// Caps `gridSpan`/`gridCol` so a malformed or hostile document with a huge span
/// value cannot drive unbounded allocation when a row is expanded to the grid
/// width. Far larger than any real table.
const MAX_TABLE_COLS: usize = 4096;

/// Upper bound on the number of rendered table cells (rows × grid width).
///
/// `MAX_TABLE_COLS` bounds the width, but the row count is otherwise unbounded:
/// a hostile document with millions of `<w:tr>` rows would materialize and escape
/// a grid of arbitrary size. This caps the product so worst-case output stays a
/// few MB. The same budget bounds the blocks emitted by the nested-table
/// linearization path, so routing a table through that path cannot bypass the
/// cap. Mirrors the HTML converter's value. Far larger than any real table.
const MAX_TABLE_CELLS: usize = 100_000;

/// Parse a `<w:tblGrid>`/`<w:tcPr>` child element (`gridCol`, `gridSpan`,
/// `vMerge`) into the current table frame.
///
/// Word may serialize these as self-closing (`Event::Empty`) or as separately
/// closed (`Event::Start` + `Event::End`) elements, so both event arms call this.
/// Returns `true` if the element was a recognized table-property element.
fn apply_table_property(
    local_str: &str,
    e: &quick_xml::events::BytesStart,
    frame: &mut DocxTable,
) -> bool {
    match local_str {
        // `<w:gridCol>` inside `<w:tblGrid>` defines one grid column; counting
        // them yields the authoritative table width.
        "gridCol" if !frame.in_row => {
            if frame.grid_width < MAX_TABLE_COLS {
                frame.grid_width += 1;
            }
            true
        }
        // `<w:gridSpan w:val="N"/>` in `<w:tcPr>`: horizontal merge.
        "gridSpan" if frame.in_cell => {
            if let Some(n) = read_w_val(e).and_then(|v| v.parse::<usize>().ok()) {
                frame.current_cell.grid_span = n.clamp(1, MAX_TABLE_COLS);
            }
            true
        }
        // `<w:vMerge .../>` in `<w:tcPr>`: vertical merge. `w:val="restart"` starts
        // a merge; a bare `<w:vMerge/>` continues one. An explicit falsey value
        // (`0`/`false`) means "not merged".
        "vMerge" if frame.in_cell => {
            let v = match read_w_val(e).as_deref() {
                Some("restart") => VMerge::Restart,
                Some(other) if other == "0" || other.eq_ignore_ascii_case("false") => VMerge::None,
                _ => VMerge::Continue,
            };
            frame.current_cell.v_merge = v;
            true
        }
        _ => false,
    }
}

/// Read the `w:val` attribute of an element, if present (namespace-agnostic).
fn read_w_val(e: &quick_xml::events::BytesStart) -> Option<String> {
    for attr in e.attributes().flatten() {
        let local_name = attr.key.local_name();
        let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
        if k == "val" {
            return Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    None
}

/// Check if a `w:val` attribute on an element is explicitly false ("0" or "false").
fn is_val_false(e: &quick_xml::events::BytesStart) -> bool {
    for attr in e.attributes().flatten() {
        let local_name = attr.key.local_name();
        let k = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
        if k == "val" {
            let v = String::from_utf8_lossy(&attr.value);
            return v == "0" || v.eq_ignore_ascii_case("false");
        }
    }
    false
}

/// Resolve paragraph kind from a style value.
fn resolve_paragraph_kind(style_val: &str, styles: &HashMap<String, u8>) -> ParagraphKind {
    if let Some(level) = extract_heading_level_from_id(style_val) {
        let clamped = level.clamp(1, 6);
        return ParagraphKind::Heading(clamped);
    }
    if let Some(&level) = styles.get(style_val) {
        let clamped = level.clamp(1, 6);
        return ParagraphKind::Heading(clamped);
    }
    ParagraphKind::Normal
}

/// Resolve a hyperlink URL from a relationship ID.
fn resolve_hyperlink_url(
    rid: &str,
    relationships: &HashMap<String, Relationship>,
    warnings: &mut Vec<ConversionWarning>,
) -> Option<String> {
    match relationships.get(rid) {
        Some(rel) => Some(rel.target.clone()),
        None => {
            warnings.push(ConversionWarning {
                code: WarningCode::SkippedElement,
                message: format!("hyperlink relationship '{rid}' not found in rels"),
                location: Some(rid.to_string()),
            });
            None
        }
    }
}

/// Finalize a paragraph: emit heading, list item, or plain text into the output buffers.
#[allow(clippy::too_many_arguments)]
fn finalize_paragraph(
    kind: &ParagraphKind,
    text: &str,
    text_plain: &str,
    output: &mut String,
    plain_output: &mut String,
    title: &mut Option<String>,
    list_counters: &mut HashMap<(String, u8), usize>,
    last_was_list: bool,
) {
    let trimmed = text.trim();
    let trimmed_plain = text_plain.trim();
    if trimmed.is_empty() {
        return;
    }

    match kind {
        ParagraphKind::Heading(level) => {
            if last_was_list {
                output.push('\n');
                plain_output.push('\n');
            }
            output.push_str(&format_heading(*level, trimmed));
            output.push('\n');
            // Plain text: just the text, no # markers
            plain_output.push_str(trimmed_plain);
            plain_output.push_str("\n\n");
            if *level == 1 && title.is_none() {
                *title = Some(trimmed_plain.to_string());
            }
        }
        ParagraphKind::ListItem {
            ordered,
            level,
            num_id,
        } => {
            let counter = if *ordered {
                let key = (num_id.clone(), *level);
                let c = list_counters.entry(key).or_insert(0);
                *c += 1;
                *c
            } else {
                1
            };
            let item = format_list_item(*level, *ordered, counter, trimmed);
            output.push_str(&item);
            output.push('\n');
            // Plain text: indented text without bullet/number markers
            let item_plain = format_list_item_plain(*level, trimmed_plain);
            plain_output.push_str(&item_plain);
            plain_output.push('\n');
        }
        ParagraphKind::Normal => {
            if last_was_list {
                output.push('\n');
                plain_output.push('\n');
            }
            output.push_str(trimmed);
            output.push_str("\n\n");
            plain_output.push_str(trimmed_plain);
            plain_output.push_str("\n\n");
        }
    }
}

// ---- Comment extraction ----

/// A raw comment parsed from `word/comments.xml`, before assembly into a
/// rendered [`Comment`].
#[derive(Debug, Clone)]
struct RawComment {
    author: String,
    date: String,
    body: String,
    /// The `w14:paraId` of the comment's last paragraph, used to link a comment
    /// to its reply metadata in `commentsExtended.xml`.
    para_id: Option<String>,
}

/// Parse `word/comments.xml` into a map from comment id to its raw content.
///
/// Captures `w:author`, `w:date`, the concatenated body text (paragraphs joined
/// by newline, later collapsed), and the `w14:paraId` of the final paragraph.
fn parse_comments_xml(xml: &str) -> HashMap<String, RawComment> {
    let mut reader = Reader::from_str(xml);
    let mut comments: HashMap<String, RawComment> = HashMap::new();

    let mut cur_id: Option<String> = None;
    let mut cur_author = String::new();
    let mut cur_date = String::new();
    let mut cur_body = String::new();
    let mut last_para_id: Option<String> = None;
    let mut para_count: usize = 0;
    let mut in_text = false;
    // Depth of nested tables; w14:paraId is only meaningful on the comment's
    // top-level paragraphs (commentsExtended threads on the last of those), so
    // paraIds inside table cells must be ignored.
    let mut table_depth: u32 = 0;

    loop {
        match reader.read_event() {
            // Only Event::Start sets in_text: a self-closing `<w:t/>` arrives as
            // Event::Empty with no matching End, so handling it here would leave
            // in_text stuck true and leak later text into the body.
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                match local_str {
                    "comment" => {
                        cur_id = None;
                        cur_author = String::new();
                        cur_date = String::new();
                        cur_body = String::new();
                        last_para_id = None;
                        para_count = 0;
                        table_depth = 0;
                        if let Some(v) =
                            crate::converter::ooxml_utils::attr_value_unescaped(e, "id")
                        {
                            cur_id = Some(v);
                        }
                        if let Some(v) =
                            crate::converter::ooxml_utils::attr_value_unescaped(e, "author")
                        {
                            cur_author = v;
                        }
                        if let Some(v) =
                            crate::converter::ooxml_utils::attr_value_unescaped(e, "date")
                        {
                            cur_date = v;
                        }
                    }
                    "tbl" if cur_id.is_some() => table_depth += 1,
                    "p" if cur_id.is_some() => {
                        // Separate paragraphs with a newline (collapsed later).
                        if para_count > 0 {
                            cur_body.push('\n');
                        }
                        para_count += 1;
                        // Capture w14:paraId of top-level paragraphs only (last wins).
                        if table_depth == 0
                            && let Some(v) =
                                crate::converter::ooxml_utils::attr_value_unescaped(e, "paraId")
                        {
                            last_para_id = Some(v);
                        }
                    }
                    "t" if cur_id.is_some() => in_text = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                // A self-closing top-level `<w:p .../>` still counts paraId.
                if local_str == "p"
                    && cur_id.is_some()
                    && table_depth == 0
                    && let Some(v) =
                        crate::converter::ooxml_utils::attr_value_unescaped(e, "paraId")
                {
                    last_para_id = Some(v);
                }
            }
            Ok(Event::Text(ref e)) if in_text => {
                cur_body.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                match local_str {
                    "t" => in_text = false,
                    "tbl" if table_depth > 0 => table_depth -= 1,
                    "comment" => {
                        if let Some(id) = cur_id.take() {
                            comments.insert(
                                id,
                                RawComment {
                                    author: std::mem::take(&mut cur_author),
                                    date: std::mem::take(&mut cur_date),
                                    body: std::mem::take(&mut cur_body),
                                    para_id: last_para_id.take(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    comments
}

/// Parse `word/commentsExtended.xml`, returning the set of `w15:paraId` values
/// that are replies (i.e., have a `w15:paraIdParent`).
fn parse_comments_extended(xml: &str) -> HashSet<String> {
    let mut reader = Reader::from_str(xml);
    let mut reply_para_ids: HashSet<String> = HashSet::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if local_str == "commentEx" {
                    let mut para_id: Option<String> = None;
                    let mut has_parent = false;
                    for attr in e.attributes().flatten() {
                        let k = attr.key.local_name();
                        let k = std::str::from_utf8(k.as_ref()).unwrap_or("");
                        match k {
                            "paraId" => {
                                para_id = Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                            "paraIdParent" => has_parent = true,
                            _ => {}
                        }
                    }
                    if has_parent && let Some(pid) = para_id {
                        reply_para_ids.insert(pid);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    reply_para_ids
}

/// Collect commented-on text ranges from a single content part (document body,
/// header, footer, footnotes, or endnotes).
///
/// Returns the comment ids in first-appearance order (keyed on whichever of
/// `commentRangeStart` or `commentReference` appears first) and a map from id to
/// the concatenated source text inside its range. Mirrors the body's extraction
/// rules: skips `mc:Choice` branches, includes text-box/table text, and takes
/// hyperlink display text (images contribute nothing).
fn collect_ranges_in_part(xml: &str) -> (Vec<String>, HashMap<String, String>) {
    let mut reader = Reader::from_str(xml);

    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut text: HashMap<String, String> = HashMap::new();
    // Set of currently-open range ids; text events append to all of them.
    let mut open: HashSet<String> = HashSet::new();

    // Skip mc:Choice branches for CONTENT (mirror body behavior), but range
    // markers are still honored inside them (see below).
    let mut in_mc_choice = false;
    let mut mc_choice_depth: u32 = 0;
    // Text is only captured inside a run, matching the body parser — a loose
    // `<w:t>` outside `<w:r>` is not rendered, so it must not appear in source.
    let mut in_run = false;
    let mut in_text = false;

    // Per-range byte bound: a malformed range that is never closed (missing or
    // misplaced commentRangeEnd) must not absorb the whole part. 4 bytes/char
    // guarantees at least SOURCE_CAP chars survive for the later char-cap.
    let cap_bytes = comments::SOURCE_CAP.saturating_mul(4);

    // Append `s` to a range buffer, never exceeding `cap_bytes` total; the slice
    // is cut on a char boundary so the buffer stays valid UTF-8.
    let push_capped = |buf: &mut String, s: &str| {
        if buf.len() >= cap_bytes {
            return;
        }
        let mut room = cap_bytes - buf.len();
        if room >= s.len() {
            buf.push_str(s);
        } else {
            while room > 0 && !s.is_char_boundary(room) {
                room -= 1;
            }
            buf.push_str(&s[..room]);
        }
    };

    // Handle a comment range/reference marker. Returns true if `local_str` was a
    // marker. Markers are honored even inside a skipped mc:Choice branch so that
    // a range whose end lands there still closes (rather than leaking forever).
    let handle_marker = |local_str: &str,
                         e: &quick_xml::events::BytesStart,
                         order: &mut Vec<String>,
                         seen: &mut HashSet<String>,
                         open: &mut HashSet<String>| {
        // Record an id in first-appearance order.
        let note_id = |order: &mut Vec<String>, seen: &mut HashSet<String>, id: String| {
            if seen.insert(id.clone()) {
                order.push(id);
            }
        };
        match local_str {
            "commentRangeStart" => {
                if let Some(id) = range_id(e) {
                    note_id(order, seen, id.clone());
                    open.insert(id);
                }
                true
            }
            "commentRangeEnd" => {
                if let Some(id) = range_id(e) {
                    open.remove(&id);
                }
                true
            }
            "commentReference" => {
                if let Some(id) = range_id(e) {
                    note_id(order, seen, id);
                }
                true
            }
            _ => false,
        }
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if handle_marker(local_str, e, &mut order, &mut seen, &mut open) {
                    continue;
                }
                if in_mc_choice {
                    mc_choice_depth += 1;
                    continue;
                }
                match local_str {
                    "Choice" => {
                        in_mc_choice = true;
                        mc_choice_depth = 1;
                    }
                    "r" => in_run = true,
                    "t" => in_text = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if handle_marker(local_str, e, &mut order, &mut seen, &mut open) {
                    continue;
                }
                if in_mc_choice {
                    continue;
                }
                if local_str == "br" && in_run && !open.is_empty() {
                    for id in &open {
                        push_capped(text.entry(id.clone()).or_default(), " ");
                    }
                }
            }
            Ok(Event::Text(ref e)) if in_text && in_run && !open.is_empty() => {
                let t = e.unescape().unwrap_or_default();
                for id in &open {
                    push_capped(text.entry(id.clone()).or_default(), &t);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if in_mc_choice {
                    mc_choice_depth -= 1;
                    if mc_choice_depth == 0 {
                        in_mc_choice = false;
                    }
                    continue;
                }
                match local_str {
                    "t" => in_text = false,
                    "r" => {
                        in_run = false;
                        in_text = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    (order, text)
}

/// Extract the `w:id` attribute from a comment range/reference element.
fn range_id(e: &quick_xml::events::BytesStart) -> Option<String> {
    for attr in e.attributes().flatten() {
        let k = attr.key.local_name();
        let k = std::str::from_utf8(k.as_ref()).unwrap_or("");
        if k == "id" {
            return Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    None
}

/// Assemble the final ordered list of [`Comment`]s for a DOCX document.
///
/// `part_orders` holds the per-part `(order, text)` results in the fixed scan
/// sequence (body, headers, footers, footnotes, endnotes). Comments are ordered
/// by first anchor appearance across parts (first part wins on duplicate ids);
/// comments with no anchor anywhere are appended last in `comments.xml` order.
/// A warning is pushed for any anchor referencing an unknown comment id.
fn assemble_docx_comments(
    raw: &HashMap<String, RawComment>,
    reply_para_ids: &HashSet<String>,
    part_results: &[(Vec<String>, HashMap<String, String>)],
    warnings: &mut Vec<ConversionWarning>,
) -> Vec<Comment> {
    let mut anchored_order: Vec<String> = Vec::new();
    let mut anchored_seen: HashSet<String> = HashSet::new();
    let mut source_by_id: HashMap<String, String> = HashMap::new();

    for (order, text) in part_results {
        for id in order {
            if anchored_seen.insert(id.clone()) {
                anchored_order.push(id.clone());
            }
            // Merge discontinuous ranges for the same id across parts with " … ".
            if let Some(t) = text.get(id)
                && !t.is_empty()
            {
                match source_by_id.get_mut(id) {
                    Some(existing) => {
                        existing.push_str(" … ");
                        existing.push_str(t);
                    }
                    None => {
                        source_by_id.insert(id.clone(), t.clone());
                    }
                }
            }
        }
    }

    let mut result: Vec<Comment> = Vec::new();

    // 1. Anchored comments, in appearance order.
    for id in &anchored_order {
        match raw.get(id) {
            Some(rc) => result.push(build_comment(rc, reply_para_ids, source_by_id.get(id))),
            None => warnings.push(ConversionWarning {
                code: WarningCode::MalformedSegment,
                message: format!("comment range references unknown comment id '{id}'"),
                location: Some(id.clone()),
            }),
        }
    }

    // 2. Orphan comments (no anchor), in comments.xml id order (numeric when possible).
    let mut orphan_ids: Vec<&String> = raw
        .keys()
        .filter(|id| !anchored_seen.contains(*id))
        .collect();
    // Sort by (numeric value, original string) so the comparator is a total
    // order even with a mix of numeric and non-numeric ids: all parseable ids
    // sort numerically and ahead of any non-numeric ones (None > Some), with the
    // string as a stable tiebreaker.
    orphan_ids.sort_by_key(|id| {
        (
            id.parse::<u64>().ok().is_none(),
            id.parse::<u64>().ok(),
            (*id).clone(),
        )
    });
    for id in orphan_ids {
        if let Some(rc) = raw.get(id) {
            result.push(build_comment(rc, reply_para_ids, None));
        }
    }

    result
}

/// Build a rendered [`Comment`] from a raw comment and optional source text.
fn build_comment(
    rc: &RawComment,
    reply_para_ids: &HashSet<String>,
    source: Option<&String>,
) -> Comment {
    let is_reply = rc
        .para_id
        .as_ref()
        .is_some_and(|pid| reply_para_ids.contains(pid));
    let source = source
        .map(|s| comments::cap_text(&comments::collapse_ws(s), comments::SOURCE_CAP))
        .unwrap_or_default();
    Comment {
        author: comments::format_author(&rc.author, &rc.date),
        body: comments::collapse_ws(&rc.body),
        source,
        is_reply,
    }
}

/// Read all DOCX comments for a document, scanning the body and any
/// headers/footers/footnotes/endnotes for commented-on text ranges.
///
/// Returns an empty vec when there are no comments. `body_xml` is the already
/// in-memory `word/document.xml`; other parts are read from `archive`.
fn extract_docx_comments(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    body_xml: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<Vec<Comment>, ConvertError> {
    // Comment parts are read leniently (lossy UTF-8): a malformed sub-part must
    // not abort the whole document conversion (best-effort principle).
    let comments_xml = match read_zip_text_lossy(archive, "word/comments.xml")? {
        Some(xml) => xml,
        None => return Ok(Vec::new()),
    };
    let raw = parse_comments_xml(&comments_xml);
    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let reply_para_ids = match read_zip_text_lossy(archive, "word/commentsExtended.xml")? {
        Some(xml) => parse_comments_extended(&xml),
        None => HashSet::new(),
    };

    // Discover extra content parts in fixed-category order: headers, footers,
    // then footnotes/endnotes. Within a category, sort by filename.
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|f| f.name().to_string()))
        .collect();
    let mut headers: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with("word/header") && n.ends_with(".xml"))
        .cloned()
        .collect();
    headers.sort();
    let mut footers: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with("word/footer") && n.ends_with(".xml"))
        .cloned()
        .collect();
    footers.sort();

    // Fixed scan order: body, headers, footers, footnotes, endnotes.
    let mut part_results: Vec<(Vec<String>, HashMap<String, String>)> = Vec::new();
    part_results.push(collect_ranges_in_part(body_xml));
    for path in headers.iter().chain(footers.iter()) {
        if let Some(xml) = read_zip_text_lossy(archive, path)? {
            part_results.push(collect_ranges_in_part(&xml));
        }
    }
    for path in ["word/footnotes.xml", "word/endnotes.xml"] {
        if let Some(xml) = read_zip_text_lossy(archive, path)? {
            part_results.push(collect_ranges_in_part(&xml));
        }
    }

    Ok(assemble_docx_comments(
        &raw,
        &reply_para_ids,
        &part_results,
        warnings,
    ))
}

// ---- Internal conversion (parse + image extraction, no resolution) ----

impl DocxConverter {
    /// Parse the document and extract images without resolving placeholders.
    ///
    /// Returns the conversion result (with unresolved placeholders in markdown),
    /// pending image data for later resolution (sync or async), and any extracted
    /// comments (empty unless `options.extract_comments` is set). Comments are
    /// appended to the output by the caller, after image placeholders resolve.
    pub(crate) fn convert_inner(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<(ConversionResult, PendingImageResolution, Vec<Comment>), ConvertError> {
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor)?;

        crate::zip_utils::validate_zip_budget(&mut archive, options.max_uncompressed_zip_bytes)?;

        // 1. Parse styles.xml (optional)
        let styles = match read_zip_text(&mut archive, "word/styles.xml")? {
            Some(xml) => parse_styles(&xml),
            None => HashMap::new(),
        };

        // 2. Parse document.xml.rels (optional)
        let relationships = match read_zip_text(&mut archive, "word/_rels/document.xml.rels")? {
            Some(xml) => parse_relationships(&xml),
            None => HashMap::new(),
        };

        // 3. Parse numbering.xml (optional)
        let numbering = match read_zip_text(&mut archive, "word/numbering.xml")? {
            Some(xml) => parse_numbering(&xml),
            None => HashMap::new(),
        };

        // 4. Parse document.xml (required)
        let document_xml = read_zip_text(&mut archive, "word/document.xml")?.ok_or_else(|| {
            ConvertError::MalformedDocument {
                reason: "missing word/document.xml".to_string(),
            }
        })?;

        let mut image_counter: usize = 0;
        let (markdown, plain_text, title, mut warnings, image_infos) = parse_document(
            &document_xml,
            &styles,
            &relationships,
            &numbering,
            &mut image_counter,
        );

        // 5. Extract embedded images if requested or if describer needs them
        let need_image_bytes = options.extract_images || options.image_describer.is_some();
        let mut images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut image_bytes_map: HashMap<String, Vec<u8>> = HashMap::new();
        if need_image_bytes {
            let mut total_image_bytes: usize = 0;
            for (rel_id, rel) in &relationships {
                if !rel.rel_type.contains("image") {
                    continue;
                }
                if total_image_bytes >= options.max_total_image_bytes {
                    break;
                }
                let image_path = resolve_relative_to_file("word/document.xml", &rel.target);
                if let Ok(Some(img_data)) = read_zip_bytes(&mut archive, &image_path) {
                    total_image_bytes += img_data.len();
                    if total_image_bytes <= options.max_total_image_bytes {
                        let filename = image_path
                            .rsplit('/')
                            .next()
                            .unwrap_or(&image_path)
                            .to_string();
                        if options.extract_images {
                            images.push((filename.clone(), img_data.clone()));
                        }
                        image_bytes_map.insert(rel_id.clone(), img_data);
                    } else {
                        warnings.push(ConversionWarning {
                            code: WarningCode::ResourceLimitReached,
                            message: format!(
                                "total image bytes exceeded limit ({})",
                                options.max_total_image_bytes
                            ),
                            location: Some(image_path),
                        });
                    }
                }
            }
        }

        // 6. Extract comments if requested (DOCX body + headers/footers/notes).
        let doc_comments = if options.extract_comments {
            extract_docx_comments(&mut archive, &document_xml, &mut warnings)?
        } else {
            Vec::new()
        };

        let result = ConversionResult {
            markdown,
            plain_text,
            title,
            images,
            warnings,
        };

        let pending = PendingImageResolution {
            infos: image_infos,
            bytes: image_bytes_map,
        };

        Ok((result, pending, doc_comments))
    }
}

// ---- Converter trait impl ----

impl Converter for DocxConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["docx"]
    }

    fn convert(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let (mut result, pending, doc_comments) = self.convert_inner(data, options)?;
        resolve_image_placeholders(
            &mut result.markdown,
            &mut result.plain_text,
            &pending.infos,
            &pending.bytes,
            options.image_describer.as_deref(),
            &mut result.warnings,
        );
        comments::append_comments(&mut result.markdown, &mut result.plain_text, &doc_comments);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Helper: build minimal DOCX ZIP in memory ----

    /// Build a minimal DOCX file in memory from document XML, optional styles XML,
    /// optional relationships XML, and optional numbering XML.
    fn build_test_docx(
        document_xml: &str,
        styles_xml: Option<&str>,
        rels_xml: Option<&str>,
    ) -> Vec<u8> {
        build_test_docx_with_numbering(document_xml, styles_xml, rels_xml, None)
    }

    fn build_test_docx_with_numbering(
        document_xml: &str,
        styles_xml: Option<&str>,
        rels_xml: Option<&str>,
        numbering_xml: Option<&str>,
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // [Content_Types].xml
        let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
        ct.push_str(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        );
        ct.push_str(
            r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        );
        ct.push_str(r#"<Default Extension="xml" ContentType="application/xml"/>"#);
        ct.push_str(
            r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
        );
        ct.push_str("</Types>");
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        )
        .unwrap();

        // word/document.xml
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        // word/styles.xml (optional)
        if let Some(styles) = styles_xml {
            zip.start_file("word/styles.xml", opts).unwrap();
            zip.write_all(styles.as_bytes()).unwrap();
        }

        // word/_rels/document.xml.rels (optional)
        if let Some(rels) = rels_xml {
            zip.start_file("word/_rels/document.xml.rels", opts)
                .unwrap();
            zip.write_all(rels.as_bytes()).unwrap();
        }

        // word/numbering.xml (optional)
        if let Some(numbering) = numbering_xml {
            zip.start_file("word/numbering.xml", opts).unwrap();
            zip.write_all(numbering.as_bytes()).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// Wrap paragraph content in a minimal document.xml structure.
    fn wrap_body(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><w:body>{body}</w:body></w:document>"#
        )
    }

    /// Build a simple paragraph XML element.
    fn para(text: &str) -> String {
        format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
    }

    /// Build a heading paragraph XML element with a direct style ID.
    fn heading_para(text: &str, level: u8) -> String {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
        )
    }

    /// Build a bold paragraph.
    fn bold_para(text: &str) -> String {
        format!(r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>{text}</w:t></w:r></w:p>"#)
    }

    /// Build an italic paragraph.
    fn italic_para(text: &str) -> String {
        format!(r#"<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>{text}</w:t></w:r></w:p>"#)
    }

    /// Build a bold+italic paragraph.
    fn bold_italic_para(text: &str) -> String {
        format!(r#"<w:p><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>{text}</w:t></w:r></w:p>"#)
    }

    // ---- Existing tests (unchanged) ----

    #[test]
    fn test_docx_supported_extensions() {
        let converter = DocxConverter;
        assert_eq!(converter.supported_extensions(), &["docx"]);
    }

    #[test]
    fn test_docx_can_convert() {
        let converter = DocxConverter;
        assert!(converter.can_convert("docx", &[]));
        assert!(!converter.can_convert("xlsx", &[]));
        assert!(!converter.can_convert("pdf", &[]));
        assert!(!converter.can_convert("txt", &[]));
    }

    #[test]
    fn test_docx_invalid_data_returns_error() {
        let converter = DocxConverter;
        let result = converter.convert(b"not a valid docx file", &ConversionOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_docx_single_paragraph() {
        let doc = wrap_body(&para("Hello, world!"));
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown.trim(), "Hello, world!");
    }

    #[test]
    fn test_docx_multiple_paragraphs() {
        let body = format!("{}{}", para("First paragraph."), para("Second paragraph."));
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("First paragraph."));
        assert!(result.markdown.contains("Second paragraph."));
        assert!(
            result
                .markdown
                .contains("First paragraph.\n\nSecond paragraph.")
        );
    }

    #[test]
    fn test_docx_empty_document() {
        let doc = wrap_body("");
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "");
    }

    #[test]
    fn test_docx_unicode_cjk() {
        let body = format!(
            "{}{}{}",
            para("한국어 테스트"),
            para("中文测试"),
            para("日本語テスト")
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어 테스트"));
        assert!(result.markdown.contains("中文测试"));
        assert!(result.markdown.contains("日本語テスト"));
    }

    #[test]
    fn test_docx_emoji() {
        let body = para("Rocket: 🚀 Stars: ✨ Earth: 🌍");
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨"));
        assert!(result.markdown.contains("🌍"));
    }

    #[test]
    fn test_docx_heading_levels() {
        let body = format!(
            "{}{}{}{}{}{}",
            heading_para("Heading 1", 1),
            heading_para("Heading 2", 2),
            heading_para("Heading 3", 3),
            heading_para("Heading 4", 4),
            heading_para("Heading 5", 5),
            heading_para("Heading 6", 6),
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("# Heading 1\n"));
        assert!(result.markdown.contains("## Heading 2\n"));
        assert!(result.markdown.contains("### Heading 3\n"));
        assert!(result.markdown.contains("#### Heading 4\n"));
        assert!(result.markdown.contains("##### Heading 5\n"));
        assert!(result.markdown.contains("###### Heading 6\n"));
    }

    #[test]
    fn test_docx_heading_from_styles_xml() {
        let body = r#"<w:p><w:pPr><w:pStyle w:val="CustomTitle"/></w:pPr><w:r><w:t>My Title</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let styles = r#"<?xml version="1.0" encoding="UTF-8"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="CustomTitle"><w:name w:val="heading 1"/></w:style></w:styles>"#;
        let data = build_test_docx(&doc, Some(styles), None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("# My Title\n"));
    }

    #[test]
    fn test_docx_first_heading1_becomes_title() {
        let body = format!(
            "{}{}{}",
            heading_para("Document Title", 1),
            para("Some text."),
            heading_para("Another H1", 1),
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.title, Some("Document Title".to_string()));
    }

    #[test]
    fn test_docx_missing_styles_xml_graceful() {
        let body = format!("{}{}", heading_para("Title", 1), para("Body text."),);
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("# Title\n"));
        assert!(result.markdown.contains("Body text."));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_docx_hyperlink() {
        let body =
            r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>Example</w:t></w:r></w:hyperlink></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("[Example](https://example.com)"));
    }

    #[test]
    fn test_docx_hyperlink_missing_rel() {
        let body = r#"<w:p><w:hyperlink r:id="rId99"><w:r><w:t>Broken Link</w:t></w:r></w:hyperlink></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Broken Link"));
        assert!(!result.markdown.contains('['));
        assert!(!result.warnings.is_empty());
        assert_eq!(result.warnings[0].code, WarningCode::SkippedElement);
    }

    #[test]
    fn test_docx_line_break() {
        let body = r#"<w:p><w:r><w:t>Line one</w:t><w:br/><w:t>Line two</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Line one\nLine two"));
    }

    #[test]
    fn test_docx_multiple_runs_joined() {
        let body = r#"<w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:t>world</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Hello world"));
    }

    // ---- Bold/Italic tests ----

    #[test]
    fn test_docx_bold_text() {
        let doc = wrap_body(&bold_para("Bold text"));
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("**Bold text**"));
    }

    #[test]
    fn test_docx_italic_text() {
        let doc = wrap_body(&italic_para("Italic text"));
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("*Italic text*"));
    }

    #[test]
    fn test_docx_bold_italic_nested() {
        let doc = wrap_body(&bold_italic_para("Bold and italic"));
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("***Bold and italic***"));
    }

    #[test]
    fn test_docx_bold_val_false_not_bold() {
        // <w:b w:val="0"/> means NOT bold
        let body = r#"<w:p><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t>Not bold</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Not bold"));
        assert!(!result.markdown.contains("**"));
    }

    #[test]
    fn test_docx_mixed_formatting_runs() {
        // Normal + bold + normal in one paragraph
        let body = r#"<w:p><w:r><w:t xml:space="preserve">Normal </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">bold </w:t></w:r><w:r><w:t>normal</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Normal **bold** normal"));
    }

    #[test]
    fn test_docx_bold_in_hyperlink() {
        let body = r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:rPr><w:b/></w:rPr><w:t>Bold Link</w:t></w:r></w:hyperlink></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result
                .markdown
                .contains("[**Bold Link**](https://example.com)")
        );
    }

    #[test]
    fn test_docx_empty_run_no_markers() {
        // Empty bold run should not produce bare **
        let body =
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t></w:t></w:r><w:r><w:t>text</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(!result.markdown.contains("****"));
        assert!(result.markdown.contains("text"));
    }

    #[test]
    fn test_docx_adjacent_bold_runs_merged() {
        // Two consecutive bold runs should produce **Hello World** not **Hello** **World**
        let body = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>World</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("**Hello World**"),
            "expected '**Hello World**' but markdown was: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("** **"),
            "should not have separate markers"
        );
    }

    #[test]
    fn test_docx_adjacent_italic_runs_merged() {
        let body = r#"<w:p><w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>World</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("*Hello World*"),
            "expected '*Hello World*' but markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_formatting_change_between_runs() {
        // Bold run then italic run should NOT merge
        let body = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">bold </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>italic</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("**bold** *italic*"),
            "expected '**bold** *italic*' but markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_split_word_across_bold_runs() {
        // Word split across two bold runs (common in spell-check/revision tracking)
        // Should produce **Hello** not **Hel****lo**
        let body = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Hel</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>lo</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("**Hello**"),
            "expected '**Hello**' but markdown was: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("****"),
            "should not have adjacent markers"
        );
    }

    // ---- Table tests ----

    #[test]
    fn test_docx_table_basic() {
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| H1 | H2 |"));
        assert!(result.markdown.contains("|---|---|"));
        assert!(result.markdown.contains("| A | B |"));
    }

    #[test]
    fn test_docx_table_empty_cells() {
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p></w:p></w:tc></w:tr><w:tr><w:tc><w:p></w:p></w:tc><w:tc><w:p><w:r><w:t>D</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| A |"));
        assert!(result.markdown.contains("| D |"));
    }

    #[test]
    fn test_docx_table_with_formatting() {
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Normal</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("**Bold**"));
        assert!(result.markdown.contains("Normal"));
    }

    #[test]
    fn test_docx_table_between_paragraphs() {
        let body = format!(
            "{}{}{}",
            para("Before table."),
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            para("After table.")
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Before table."));
        assert!(result.markdown.contains("| Cell |"));
        assert!(result.markdown.contains("After table."));
    }

    #[test]
    fn test_docx_table_unicode() {
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>한국어</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>中文</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("中文"));
    }

    #[test]
    fn test_docx_table_gridspan_banner_preserves_all_columns() {
        // A full-width gridSpan banner over a two-column data row. The banner must
        // not collapse the table to one column: both "A" and "B" must survive.
        // (This previously truncated every row to its first cell.) The whole table
        // renders as one empty-filled GFM grid.
        let body = r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Merged</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // The banner is the header row of the grid, empty-filled to full width.
        assert!(
            result.markdown.contains("| Merged |  |"),
            "banner not an empty-filled header: {}",
            result.markdown
        );
        // The data row keeps both columns.
        assert!(
            result.markdown.contains("| A | B |"),
            "columns truncated: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_table_depth_stack_nested_smoke() {
        // A table nested inside a cell of an outer table. The parser tracks table
        // depth via a stack, so both the outer cell text and the inner table's
        // cells must survive (the old single-flag machine could not nest).
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>outer</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("outer"),
            "outer cell text missing: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("inner"),
            "inner table text missing: {}",
            result.markdown
        );
    }

    // ---- Table classifier unit tests ----

    /// Build a cell with the given markdown text, grid span, and merge state.
    fn cell(text: &str, grid_span: usize, v_merge: VMerge) -> DocxCell {
        DocxCell {
            grid_span,
            v_merge,
            blocks: vec![DocxCellBlock {
                md: text.to_string(),
                plain: text.to_string(),
                is_table: false,
            }],
        }
    }

    /// Build a row from cells.
    fn row(cells: Vec<DocxCell>) -> DocxRow {
        DocxRow { cells }
    }

    #[test]
    fn test_docx_grid_width_from_gridcol() {
        let t = DocxTable {
            grid_width: 3,
            rows: vec![row(vec![cell("a", 1, VMerge::None)])],
            ..Default::default()
        };
        assert_eq!(docx_grid_width(&t), 3);
    }

    #[test]
    fn test_docx_grid_width_fallback_max_span() {
        let t = DocxTable {
            grid_width: 0,
            rows: vec![
                row(vec![cell("a", 2, VMerge::None)]),
                row(vec![
                    cell("a", 1, VMerge::None),
                    cell("b", 1, VMerge::None),
                    cell("c", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        assert_eq!(docx_grid_width(&t), 3);
    }

    #[test]
    fn test_docx_grid_width_tblgrid_is_floor_not_ceiling() {
        // tblGrid declares 2 columns but a row actually has 3 single-span cells.
        // The grid width must widen to 3 so no cell is dropped.
        let t = DocxTable {
            grid_width: 2,
            rows: vec![row(vec![
                cell("A", 1, VMerge::None),
                cell("B", 1, VMerge::None),
                cell("C", 1, VMerge::None),
            ])],
            ..Default::default()
        };
        assert_eq!(docx_grid_width(&t), 3);
    }

    #[test]
    fn test_docx_grid_width_uniform_match_unchanged() {
        // When tblGrid equals the widest row, the width is byte-identical (the
        // floor change must not perturb ordinary uniform tables).
        let t = DocxTable {
            grid_width: 3,
            rows: vec![
                row(vec![
                    cell("a", 1, VMerge::None),
                    cell("b", 1, VMerge::None),
                    cell("c", 1, VMerge::None),
                ]),
                row(vec![
                    cell("d", 1, VMerge::None),
                    cell("e", 1, VMerge::None),
                    cell("f", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        assert_eq!(docx_grid_width(&t), 3);
    }

    #[test]
    fn test_docx_render_three_cells_in_two_col_grid_keeps_all() {
        // 3 single-span cells A,B,C in a 2-col tblGrid: all three survive.
        let t = DocxTable {
            grid_width: 2,
            rows: vec![
                row(vec![
                    cell("H1", 1, VMerge::None),
                    cell("H2", 1, VMerge::None),
                ]),
                row(vec![
                    cell("A", 1, VMerge::None),
                    cell("B", 1, VMerge::None),
                    cell("C", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        assert!(md.contains("| A | B | C |"), "trailing cell dropped: {md}");
    }

    #[test]
    fn test_docx_render_gridspan_overflow_keeps_trailing_cell() {
        // A [gridSpan=2 X, Y] row in a 2-col grid sums to 3 > grid_width: Y must
        // not be truncated. Grid widens to 3; X empty-fills its second column.
        let t = DocxTable {
            grid_width: 2,
            rows: vec![row(vec![
                cell("X", 2, VMerge::None),
                cell("Y", 1, VMerge::None),
            ])],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        // Single row => header only; X spans cols 1-2 (empty-filled), Y in col 3.
        assert!(md.contains("| X |  | Y |"), "Y truncated: {md}");
    }

    // ---- Grid render unit tests ----

    #[test]
    fn test_docx_expand_row_to_grid_hspan() {
        let r = row(vec![cell("X", 2, VMerge::None), cell("Y", 1, VMerge::None)]);
        assert_eq!(expand_row_to_grid(&r, 3, false), vec!["X", "", "Y"]);
    }

    #[test]
    fn test_docx_expand_row_vmerge_continue_empty() {
        // A genuine vertical-merge continuation cell is empty in the source, so it
        // renders empty.
        let r = row(vec![DocxCell {
            grid_span: 1,
            v_merge: VMerge::Continue,
            blocks: vec![],
        }]);
        assert_eq!(expand_row_to_grid(&r, 1, false), vec![""]);
    }

    #[test]
    fn test_docx_expand_row_vmerge_continue_with_text_preserved() {
        // An orphan continuation cell that carries text (no matching restart, or
        // malformed input) keeps its content rather than silently dropping it.
        let r = row(vec![cell("kept", 1, VMerge::Continue)]);
        assert_eq!(expand_row_to_grid(&r, 1, false), vec!["kept"]);
    }

    #[test]
    fn test_docx_render_full_width_banner_is_grid_row() {
        // A full-width "banner" row is kept as a grid row (empty-filled), not
        // linearized into a heading or bold paragraph.
        let t = DocxTable {
            grid_width: 3,
            rows: vec![
                row(vec![cell("Section Header", 3, VMerge::None)]),
                row(vec![
                    cell("A", 1, VMerge::None),
                    cell("B", 1, VMerge::None),
                    cell("C", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        // One GFM table: the banner is the header row, empty-filled.
        assert!(md.contains("| Section Header |  |  |"), "md: {md}");
        assert!(md.contains("| A | B | C |"), "md: {md}");
        // No heading/bold linearization.
        assert!(!md.contains('#'), "md: {md}");
        assert!(!md.contains("**"), "md: {md}");
    }

    #[test]
    fn test_docx_render_label_value_kept_as_grid_row() {
        // A narrow-label + wide-value row stays a grid row (empty-filled), not a
        // `**Label:** value` line.
        let t = DocxTable {
            grid_width: 3,
            rows: vec![row(vec![
                cell("Date", 1, VMerge::None),
                cell("Monday", 2, VMerge::None),
            ])],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        assert!(md.contains("| Date | Monday |  |"), "md: {md}");
        assert!(!md.contains("**Date:**"), "md: {md}");
    }

    #[test]
    fn test_docx_render_data_table_spanning_row_keeps_columns() {
        // A data table with a header row and a spanning row keeps all columns as
        // one GFM table (the regression the review flagged).
        let t = DocxTable {
            grid_width: 3,
            rows: vec![
                row(vec![
                    cell("A", 1, VMerge::None),
                    cell("B", 1, VMerge::None),
                    cell("C", 1, VMerge::None),
                ]),
                row(vec![
                    cell("x", 1, VMerge::None),
                    cell("spans", 2, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        assert!(md.contains("| A | B | C |"), "md: {md}");
        assert!(md.contains("| x | spans |  |"), "md: {md}");
    }

    #[test]
    fn test_docx_render_grid_vmerge_continue_blank() {
        // vMerge restart label + continuation: continuation cell renders empty.
        let t = DocxTable {
            grid_width: 2,
            rows: vec![
                row(vec![
                    cell("Label", 1, VMerge::Restart),
                    cell("Head", 1, VMerge::None),
                ]),
                row(vec![
                    cell("", 1, VMerge::Continue),
                    cell("Val", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        assert!(md.contains("| Label | Head |"), "md: {md}");
        // The continuation cell is empty in the data row.
        assert!(md.contains("|  | Val |"), "md: {md}");
    }

    #[test]
    fn test_docx_render_pipe_in_cell_escaped() {
        // A literal pipe inside a grid cell must be escaped by build_table.
        let t = DocxTable {
            grid_width: 2,
            rows: vec![
                row(vec![
                    cell("h1", 1, VMerge::None),
                    cell("h2", 1, VMerge::None),
                ]),
                row(vec![
                    cell("a|b", 1, VMerge::None),
                    cell("c", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (md, _plain) = render_docx_table(&t, &mut Vec::new());
        assert!(md.contains("a\\|b"), "pipe not escaped: {md}");
    }

    #[test]
    fn test_docx_plain_grid_tab_separated() {
        // Plain text for a merged/layout table is tab-separated, empty-filled.
        let t = DocxTable {
            grid_width: 3,
            rows: vec![
                row(vec![cell("Section", 3, VMerge::None)]),
                row(vec![
                    cell("A", 1, VMerge::None),
                    cell("B", 1, VMerge::None),
                    cell("C", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        let (_md, plain) = render_docx_table(&t, &mut Vec::new());
        assert!(plain.contains("Section"), "plain: {plain}");
        assert!(!plain.contains("**"), "plain has markers: {plain}");
        assert!(plain.contains("A\tB\tC"), "plain not tabbed: {plain}");
    }

    #[test]
    fn test_docx_render_deterministic() {
        // Byte-stable output: two renders of the same table must be identical.
        let t = DocxTable {
            grid_width: 3,
            rows: vec![
                row(vec![cell("Banner", 3, VMerge::None)]),
                row(vec![
                    cell("Field", 1, VMerge::None),
                    cell("Value", 2, VMerge::None),
                ]),
                row(vec![
                    cell("A", 1, VMerge::None),
                    cell("B", 1, VMerge::None),
                    cell("C", 1, VMerge::None),
                ]),
            ],
            ..Default::default()
        };
        assert_eq!(
            render_docx_table(&t, &mut Vec::new()),
            render_docx_table(&t, &mut Vec::new())
        );
    }

    #[test]
    fn test_docx_grid_width_bounded() {
        // A pathological gridSpan cannot drive an unbounded grid width.
        let t = DocxTable {
            grid_width: 0,
            rows: vec![row(vec![cell("x", 100_000_000, VMerge::None)])],
            ..Default::default()
        };
        assert!(docx_grid_width(&t) <= MAX_TABLE_COLS);
        // Expanding the row must not allocate beyond the bound.
        let cols = expand_row_to_grid(&t.rows[0], docx_grid_width(&t), false);
        assert!(cols.len() <= MAX_TABLE_COLS);
    }

    #[test]
    fn test_docx_linearize_path_blocks_capped_with_warning() {
        // A nested table anywhere routes the whole table through the linearize
        // path; the blocks it emits must honor MAX_TABLE_CELLS just like grid
        // cells do, and dropping blocks must be observable.
        let mut rows: Vec<DocxRow> = Vec::with_capacity(MAX_TABLE_CELLS + 2);
        rows.push(row(vec![DocxCell {
            grid_span: 1,
            v_merge: VMerge::None,
            blocks: vec![DocxCellBlock {
                md: "| i |\n|---|".to_string(),
                plain: "i".to_string(),
                is_table: true,
            }],
        }]));
        for i in 0..=MAX_TABLE_CELLS {
            rows.push(row(vec![cell(&format!("r{i}"), 1, VMerge::None)]));
        }
        let t = DocxTable {
            grid_width: 1,
            rows,
            ..Default::default()
        };
        let mut warnings = Vec::new();
        let (md, plain) = render_docx_table(&t, &mut warnings);
        assert!(md.contains("r0"), "early block missing");
        let last = format!("r{MAX_TABLE_CELLS}");
        assert!(
            !md.contains(&last),
            "linearized markdown not truncated: {} bytes",
            md.len()
        );
        assert!(
            !plain.contains(&last),
            "linearized plain not truncated: {} bytes",
            plain.len()
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "expected ResourceLimitReached warning, got: {warnings:?}"
        );
    }

    #[test]
    fn test_docx_overwide_row_columns_dropped_with_warning() {
        // A row with more real cells than MAX_TABLE_COLS loses its trailing
        // cells to the width cap; that drop must be observable (mirrors the
        // HTML converter's width-cap warning).
        let cells: Vec<DocxCell> = (0..MAX_TABLE_COLS + 5)
            .map(|i| cell(&format!("c{i}"), 1, VMerge::None))
            .collect();
        let t = DocxTable {
            grid_width: 0,
            rows: vec![row(cells)],
            ..Default::default()
        };
        let mut warnings = Vec::new();
        let (md, _plain) = render_docx_table(&t, &mut warnings);
        assert!(md.contains("c0"), "leading cell missing");
        assert!(
            !md.contains(&format!("c{MAX_TABLE_COLS}")),
            "trailing cell beyond the width cap must be dropped"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "expected ResourceLimitReached warning, got: {warnings:?}"
        );

        // A table within the width bound must not warn.
        let t_ok = DocxTable {
            grid_width: 3,
            rows: vec![row(vec![
                cell("a", 1, VMerge::None),
                cell("b", 1, VMerge::None),
                cell("c", 1, VMerge::None),
            ])],
            ..Default::default()
        };
        let mut ok_warnings = Vec::new();
        let _ = render_docx_table(&t_ok, &mut ok_warnings);
        assert!(
            ok_warnings.is_empty(),
            "unexpected warning: {ok_warnings:?}"
        );
    }

    #[test]
    fn test_docx_nested_table_rendered_fully() {
        // A 2x2 inner table inside a single outer cell. All four inner cells must
        // appear, rendered as a GFM table nested within the outer cell's position.
        let inner = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>i1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>i2</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>i3</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>i4</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let body = format!(
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>outer-text</w:t></w:r></w:p>{inner}</w:tc></w:tr></w:tbl>"#
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let result = DocxConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        for needle in ["outer-text", "i1", "i2", "i3", "i4"] {
            assert!(
                result.markdown.contains(needle),
                "missing {needle}: {}",
                result.markdown
            );
        }
        // The inner table is rendered as a GFM table (has a separator row).
        assert!(
            result.markdown.contains("| i1 | i2 |"),
            "{}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_nested_table_forces_linearize() {
        // The outer row holds a nested table, so it must linearize: the outer
        // cell text is not crammed into a one-column GFM cell with <br> noise.
        let inner =
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let body = format!(
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>before</w:t></w:r></w:p>{inner}</w:tc></w:tr></w:tbl>"#
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let result = DocxConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("before"), "{}", result.markdown);
        assert!(result.markdown.contains("inner"), "{}", result.markdown);
        // No <br>-joined cramming of the outer paragraph with the inner table.
        assert!(
            !result.markdown.contains("before<br>"),
            "outer cell was crammed: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_empty_nested_table_no_phantom_table_end_to_end() {
        // An outer cell holds real text followed by a nested table whose only cell
        // is empty. Driven through the REAL parser (not a hand-built block): the
        // empty inner table renders to a degenerate skeleton (`|  |\n|---|`) and is
        // suppressed entirely. With the empty nested table gone, the outer 1x1
        // table is a clean grid `| OuterText |`; crucially there is NO second,
        // phantom empty table and the markdown/plain streams agree on the content.
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>OuterText</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let result = DocxConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("OuterText"),
            "md: {:?}",
            result.markdown
        );
        // The empty inner table contributed no skeleton: a single separator row
        // (the outer table's own), and no empty `|  |` data cell from the inner one.
        assert_eq!(
            result.markdown.matches("---").count(),
            1,
            "phantom nested separator in md: {:?}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("|  |"),
            "phantom empty nested cell in md: {:?}",
            result.markdown
        );
        // plain_text agrees on the content (the inner empty table left no residue).
        assert!(
            result.plain_text.contains("OuterText"),
            "plain: {:?}",
            result.plain_text
        );
        assert_eq!(
            result.markdown.matches("OuterText").count(),
            1,
            "md: {:?}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_linearize_skips_block_empty_in_both_streams() {
        // A block empty in BOTH md and plain is skipped entirely; a sibling
        // nested-table block keeps the table on the linearize path.
        let t = DocxTable {
            grid_width: 1,
            rows: vec![row(vec![DocxCell {
                grid_span: 1,
                v_merge: VMerge::None,
                blocks: vec![
                    DocxCellBlock {
                        md: "   ".to_string(),
                        plain: "   ".to_string(),
                        is_table: false,
                    },
                    DocxCellBlock {
                        md: "| i |\n|---|\n| i |\n".to_string(),
                        plain: "i\n".to_string(),
                        is_table: true,
                    },
                ],
            }])],
            ..Default::default()
        };
        let (md, plain) = render_docx_table(&t, &mut Vec::new());
        // Only the nested table content is present; the blank block contributed
        // nothing to either stream.
        assert!(
            md.starts_with("| i |"),
            "leading blank block leaked: {md:?}"
        );
        assert!(
            plain.starts_with("i\n"),
            "leading blank block leaked: {plain:?}"
        );
    }

    // ---- List tests ----

    #[test]
    fn test_docx_unordered_list() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item 1</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item 2</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("- Item 1"));
        assert!(result.markdown.contains("- Item 2"));
    }

    #[test]
    fn test_docx_ordered_list() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>First</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Second</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("1. First"));
        assert!(result.markdown.contains("2. Second"));
    }

    #[test]
    fn test_docx_nested_list() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Parent</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Child</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("- Parent"));
        assert!(result.markdown.contains("  - Child"));
    }

    #[test]
    fn test_docx_mixed_list_and_paragraph() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = format!(
            "{}{}{}",
            para("Before list."),
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>List item</w:t></w:r></w:p>"#,
            para("After list.")
        );
        let doc = wrap_body(&body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Before list."));
        assert!(result.markdown.contains("- List item"));
        assert!(result.markdown.contains("After list."));
    }

    #[test]
    fn test_docx_list_with_bold() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>Bold item</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("- **Bold item**"));
    }

    #[test]
    fn test_docx_two_separate_ordered_lists_restart_numbering() {
        // Two ordered lists with different numId, separated by a normal paragraph.
        // The second list should restart numbering at 1.
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#;
        let body = format!(
            "{}{}{}{}{}{}{}",
            // First ordered list (numId=1)
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Alpha</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Beta</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Gamma</w:t></w:r></w:p>"#,
            // Normal paragraph separating the lists
            para("Separator paragraph."),
            // Second ordered list (numId=2)
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>First</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Second</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Third</w:t></w:r></w:p>"#,
        );
        let doc = wrap_body(&body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // First list: 1, 2, 3
        assert!(
            result.markdown.contains("1. Alpha"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("2. Beta"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("3. Gamma"),
            "markdown was: {}",
            result.markdown
        );
        // Second list: should restart at 1, not continue at 4
        assert!(
            result.markdown.contains("1. First"),
            "expected '1. First' but markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("2. Second"),
            "expected '2. Second' but markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("3. Third"),
            "expected '3. Third' but markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_parse_numbering_missing_graceful() {
        // No numbering.xml — numPr should fall back to bullet
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Fallback item</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Falls back to bullet (unordered) when numbering.xml is missing
        assert!(result.markdown.contains("- Fallback item"));
    }

    // ---- Image tests ----

    #[test]
    fn test_docx_inline_image() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr=""/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("![](image1.png)"));
    }

    #[test]
    fn test_docx_image_with_alt_text() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="A nice photo"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/photo.jpg"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("![A nice photo](photo.jpg)"));
    }

    #[test]
    fn test_docx_image_missing_rel_graceful() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Missing"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId99"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Image should be skipped with a warning
        assert!(!result.warnings.is_empty());
        assert!(result.warnings[0].message.contains("not found"));
    }

    // ---- Numbering parser unit tests ----

    #[test]
    fn test_parse_numbering_bullet() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let result = parse_numbering(xml);
        assert_eq!(
            result.get(&("1".to_string(), 0)).map(|n| n.ordered),
            Some(false)
        );
    }

    #[test]
    fn test_parse_numbering_decimal() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let result = parse_numbering(xml);
        assert_eq!(
            result.get(&("1".to_string(), 0)).map(|n| n.ordered),
            Some(true)
        );
    }

    // ---- Resource limit tests ----

    #[test]
    fn test_docx_zip_budget_exceeded_returns_error() {
        let doc = wrap_body(&para("Hello"));
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        // Set budget to 1 byte — any real DOCX will exceed this
        let options = ConversionOptions {
            max_uncompressed_zip_bytes: 1,
            ..Default::default()
        };
        let result = converter.convert(&data, &options);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("input too large"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_docx_relationship_type_captured() {
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let result = parse_relationships(rels);
        assert_eq!(
            result.get("rId1").map(|r| r.rel_type.as_str()),
            Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships/image")
        );
        assert_eq!(
            result.get("rId2").map(|r| r.rel_type.as_str()),
            Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink")
        );
    }

    // ---- Image extraction tests ----

    /// Helper: build a DOCX with an embedded image file.
    fn build_test_docx_with_image(
        document_xml: &str,
        rels_xml: &str,
        image_path: &str,
        image_data: &[u8],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // [Content_Types].xml
        let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        )
        .unwrap();

        // word/document.xml
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        // word/_rels/document.xml.rels
        zip.start_file("word/_rels/document.xml.rels", opts)
            .unwrap();
        zip.write_all(rels_xml.as_bytes()).unwrap();

        // Image file
        zip.start_file(image_path, opts).unwrap();
        zip.write_all(image_data).unwrap();

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_docx_image_extraction_enabled() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Test image"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data-for-test";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(!result.images.is_empty(), "expected extracted images");
        assert_eq!(result.images[0].0, "image1.png");
        assert_eq!(result.images[0].1, fake_png);
    }

    #[test]
    fn test_docx_image_extraction_disabled_by_default() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Test"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_docx_image_extraction_respects_budget() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Big"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = vec![0u8; 1024]; // 1 KB image
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", &fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            extract_images: true,
            max_total_image_bytes: 100, // Budget smaller than image
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        // Image should not be extracted (exceeds budget)
        assert!(result.images.is_empty());
        // Should have a ResourceLimitReached warning
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached),
            "expected ResourceLimitReached warning, got: {:?}",
            result.warnings
        );
    }

    // ---- Image describer tests ----

    use crate::converter::ImageDescriber;
    use std::sync::Arc;

    struct MockDescriber {
        description: String,
    }

    impl ImageDescriber for MockDescriber {
        fn describe(
            &self,
            _image_bytes: &[u8],
            _mime_type: &str,
            _prompt: &str,
        ) -> Result<String, ConvertError> {
            Ok(self.description.clone())
        }
    }

    struct FailingDescriber;

    impl ImageDescriber for FailingDescriber {
        fn describe(
            &self,
            _image_bytes: &[u8],
            _mime_type: &str,
            _prompt: &str,
        ) -> Result<String, ConvertError> {
            Err(ConvertError::ImageDescriptionError {
                reason: "API error".to_string(),
            })
        }
    }

    #[test]
    fn test_docx_image_describer_replaces_alt_text() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr=""/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "A beautiful sunset over the ocean".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result
                .markdown
                .contains("![A beautiful sunset over the ocean](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
        // Images should not be in result.images since extract_images is false
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_docx_image_describer_with_extract_images() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr=""/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            extract_images: true,
            image_describer: Some(Arc::new(MockDescriber {
                description: "Described image".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(result.markdown.contains("![Described image](image1.png)"));
        assert!(!result.images.is_empty());
    }

    #[test]
    fn test_docx_image_describer_absolute_target_path() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Original alt"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="/word/media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "Described image".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("![Described image](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_image_describer_dot_slash_target_path() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Original alt"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="./media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "Described image".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("![Described image](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
    }

    /// Helper: build a DOCX with multiple embedded image files.
    fn build_test_docx_with_images(
        document_xml: &str,
        rels_xml: &str,
        images: &[(&str, &[u8])], // (zip_path, data)
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ).unwrap();

        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        zip.start_file("word/_rels/document.xml.rels", opts)
            .unwrap();
        zip.write_all(rels_xml.as_bytes()).unwrap();

        for (path, data) in images {
            zip.start_file(*path, opts).unwrap();
            zip.write_all(data).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// A mock describer that returns descriptions based on the image bytes.
    struct MockDescriberByContent;

    impl ImageDescriber for MockDescriberByContent {
        fn describe(
            &self,
            image_bytes: &[u8],
            _mime_type: &str,
            _prompt: &str,
        ) -> Result<String, ConvertError> {
            // Return different descriptions based on the content
            let content = String::from_utf8_lossy(image_bytes);
            if content.contains("cat") {
                Ok("A photo of a cat".to_string())
            } else if content.contains("dog") {
                Ok("A photo of a dog".to_string())
            } else {
                Ok("Unknown image".to_string())
            }
        }
    }

    #[test]
    fn test_docx_duplicate_image_filenames_independent_descriptions() {
        // Two images in the same document, both referencing different relationship IDs
        // that point to files with the SAME filename (media/image1.png) but different content.
        // In practice DOCX files with duplicate filenames come from different relationship IDs.
        // Here we use two different rel IDs pointing to different paths (media/img_a.png and
        // media/img_b.png) but the filenames extracted are different. To truly test duplicate
        // filenames, we need the same Target in two rels (which doesn't happen in practice).
        //
        // Instead, test the real scenario: two images with the same filename in the markdown
        // output. We simulate this by having two rels with the same target path.
        let body = format!(
            "{}{}{}",
            r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="First image"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            para("Text between images"),
            r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Second image"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId3"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        );
        let doc = wrap_body(&body);
        // Both rels point to the same filename
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-cat-png-data";
        let data = build_test_docx_with_images(&doc, rels, &[("word/media/image1.png", fake_png)]);

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriberByContent)),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        // Both images should have descriptions, and the text between them must be preserved
        let md = &result.markdown;
        assert!(
            md.contains("![A photo of a cat](image1.png)"),
            "expected first image described, markdown was: {}",
            md
        );
        assert!(
            md.contains("Text between images"),
            "expected text between images preserved, markdown was: {}",
            md
        );
        // Count occurrences of the described image — should be exactly 2
        let count = md.matches("![A photo of a cat](image1.png)").count();
        assert_eq!(
            count, 2,
            "expected 2 described images, found {} in: {}",
            count, md
        );
    }

    #[test]
    fn test_docx_duplicate_basenames_different_paths_independent_descriptions() {
        let body = format!(
            "{}{}{}",
            r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="First image"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            para("Text between images"),
            r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Second image"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId3"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        );
        let doc = wrap_body(&body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/a/image1.png"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/b/image1.png"/></Relationships>"#;
        let data = build_test_docx_with_images(
            &doc,
            rels,
            &[
                ("word/media/a/image1.png", b"fake-cat-png-data"),
                ("word/media/b/image1.png", b"fake-dog-png-data"),
            ],
        );

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriberByContent)),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        let md = &result.markdown;
        assert!(
            md.contains("![A photo of a cat](image1.png)"),
            "expected cat description, markdown was: {}",
            md
        );
        assert!(
            md.contains("![A photo of a dog](image1.png)"),
            "expected dog description, markdown was: {}",
            md
        );
    }

    #[test]
    fn test_docx_image_describer_error_keeps_original_alt() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="Original alt"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
        let fake_png = b"fake-png-data";
        let data = build_test_docx_with_image(&doc, rels, "word/media/image1.png", fake_png);

        let converter = DocxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(FailingDescriber)),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("![Original alt](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::SkippedElement
                    && w.message.contains("image description failed")),
            "expected SkippedElement warning for image description failure"
        );
    }

    // ---- Plain text output tests ----

    #[test]
    fn test_docx_plain_text_paragraphs_and_headings() {
        let body = format!(
            "{}{}{}",
            heading_para("My Title", 1),
            para("Normal paragraph."),
            heading_para("Section", 2),
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown should have # markers
        assert!(result.markdown.contains("# My Title"));
        assert!(result.markdown.contains("## Section"));
        // Plain text: no # markers, just text
        assert!(
            !result.plain_text.contains('#'),
            "plain_text should not contain # heading markers, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("My Title"),
            "plain_text should contain heading text, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Normal paragraph."),
            "plain_text should contain paragraph text, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Section"),
            "plain_text should contain second heading text, was: {}",
            result.plain_text
        );
    }

    #[test]
    fn test_docx_plain_text_no_bold_italic_markers() {
        let body = format!(
            "{}{}{}",
            bold_para("Bold text"),
            italic_para("Italic text"),
            bold_italic_para("Both"),
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown has formatting markers
        assert!(result.markdown.contains("**Bold text**"));
        assert!(result.markdown.contains("*Italic text*"));
        assert!(result.markdown.contains("***Both***"));
        // Plain text: no * markers at all
        assert!(
            !result.plain_text.contains('*'),
            "plain_text should not contain * markers, was: {}",
            result.plain_text
        );
        assert!(result.plain_text.contains("Bold text"));
        assert!(result.plain_text.contains("Italic text"));
        assert!(result.plain_text.contains("Both"));
    }

    #[test]
    fn test_docx_plain_text_hyperlink_no_markdown_syntax() {
        let body = r#"<w:p><w:hyperlink r:id="rId1"><w:r><w:t>Example Link</w:t></w:r></w:hyperlink></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown has link syntax
        assert!(
            result
                .markdown
                .contains("[Example Link](https://example.com)")
        );
        // Plain text: just the text, no brackets or URL
        assert!(
            result.plain_text.contains("Example Link"),
            "plain_text should contain link text, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains('['),
            "plain_text should not contain [ bracket, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains("https://example.com"),
            "plain_text should not contain URL, was: {}",
            result.plain_text
        );
    }

    #[test]
    fn test_docx_plain_text_table_tab_separated() {
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown has pipe-delimited table
        assert!(result.markdown.contains("| H1 | H2 |"));
        assert!(result.markdown.contains("|---|---|"));
        // Plain text: tab-separated, no pipes or separator row
        assert!(
            result.plain_text.contains("H1\tH2"),
            "plain_text should have tab-separated headers, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("A\tB"),
            "plain_text should have tab-separated data, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains('|'),
            "plain_text should not contain pipe characters, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains("---"),
            "plain_text should not contain separator row, was: {}",
            result.plain_text
        );
    }

    #[test]
    fn test_docx_plain_text_image_no_markdown_syntax() {
        let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr descr="A photo"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
        let doc = wrap_body(body);
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/photo.jpg"/></Relationships>"#;
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown has image syntax: ![alt](filename)
        assert!(result.markdown.contains("photo.jpg"));
        assert!(result.markdown.contains("!["));
        // Plain text: just the placeholder, no ![] or () syntax
        assert!(
            !result.plain_text.contains("!["),
            "plain_text should not contain ![ image syntax, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains("photo.jpg"),
            "plain_text should not contain image filename, was: {}",
            result.plain_text
        );
    }

    #[test]
    fn test_docx_plain_text_list_no_markers() {
        let numbering = r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
        let body = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item 1</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item 2</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx_with_numbering(&doc, None, None, Some(numbering));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown has bullet markers
        assert!(result.markdown.contains("- Item 1"));
        assert!(result.markdown.contains("- Item 2"));
        // Plain text: no bullet markers
        assert!(
            !result.plain_text.contains("- "),
            "plain_text should not contain bullet markers, was: {}",
            result.plain_text
        );
        assert!(result.plain_text.contains("Item 1"));
        assert!(result.plain_text.contains("Item 2"));
    }

    #[test]
    fn test_docx_plain_text_table_bold_cells_no_formatting() {
        // Table with bold text in cells — plain_text should not contain **
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Header</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Value</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold Cell</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Normal</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown should have bold markers
        assert!(
            result.markdown.contains("**Bold Cell**"),
            "markdown should contain bold markers, was: {}",
            result.markdown
        );
        // Plain text should NOT have bold markers
        assert!(
            !result.plain_text.contains("**"),
            "plain_text should not contain ** markers, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Bold Cell"),
            "plain_text should contain the cell text"
        );
    }

    #[test]
    fn test_docx_plain_text_table_hyperlink_cells_no_markdown() {
        // Table with hyperlinked text in cells — plain_text should not contain [](url)
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let body = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:hyperlink r:id="rId1"><w:r><w:t>Click Here</w:t></w:r></w:hyperlink></w:p></w:tc></w:tr></w:tbl>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Markdown should have link syntax
        assert!(
            result
                .markdown
                .contains("[Click Here](https://example.com)"),
            "markdown should contain link syntax, was: {}",
            result.markdown
        );
        // Plain text should NOT have link syntax
        assert!(
            !result.plain_text.contains('['),
            "plain_text should not contain [ from link syntax, was: {}",
            result.plain_text
        );
        assert!(
            !result.plain_text.contains("https://example.com"),
            "plain_text should not contain URL, was: {}",
            result.plain_text
        );
        assert!(
            result.plain_text.contains("Click Here"),
            "plain_text should contain the link text"
        );
    }

    #[test]
    fn test_docx_title_no_markdown_formatting() {
        // Heading 1 with bold text — title should be plain text, no **
        let body = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>Bold Title</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(
            result.title.as_deref(),
            Some("Bold Title"),
            "title should not contain markdown bold markers, was: {:?}",
            result.title
        );
        assert!(
            !result.title.as_deref().unwrap_or("").contains("**"),
            "title must not contain ** markers"
        );
    }

    #[test]
    fn test_docx_title_hyperlink_no_markdown_syntax() {
        // Heading 1 with a hyperlink — title should be plain text, no [](url)
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let body = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:hyperlink r:id="rId1"><w:r><w:t>Linked Title</w:t></w:r></w:hyperlink></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, Some(rels));
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(
            result.title.as_deref(),
            Some("Linked Title"),
            "title should not contain markdown link syntax, was: {:?}",
            result.title
        );
        assert!(
            !result.title.as_deref().unwrap_or("").contains('['),
            "title must not contain [ from link syntax"
        );
    }

    // ---- mc:AlternateContent tests ----

    #[test]
    fn test_docx_alternate_content_fallback_used() {
        // mc:AlternateContent with Choice (DrawingML) and Fallback (VML).
        // Fallback text should appear; Choice text should NOT.
        let body = r#"<mc:AlternateContent><mc:Choice Requires="wps"><w:p><w:r><w:t>Choice text (should be hidden)</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>Fallback text visible</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("Fallback text visible"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("Choice text"),
            "Choice text should be skipped, markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_alternate_content_choice_skipped() {
        // mc:AlternateContent with only Choice (no Fallback) — nothing should appear
        let body = r#"<w:p><w:r><w:t>Before AC</w:t></w:r></w:p><mc:AlternateContent><mc:Choice Requires="wps"><w:p><w:r><w:t>Hidden</w:t></w:r></w:p></mc:Choice></mc:AlternateContent><w:p><w:r><w:t>After AC</w:t></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Before AC"));
        assert!(result.markdown.contains("After AC"));
        assert!(
            !result.markdown.contains("Hidden"),
            "Choice content should be skipped, markdown was: {}",
            result.markdown
        );
    }

    // ---- Text box tests ----

    #[test]
    fn test_docx_textbox_basic() {
        // Simple text box: w:pict > v:shape > v:textbox > w:txbxContent > w:p
        let body = r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>Text box content</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("Text box content"),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_textbox_with_formatting() {
        // Bold and italic text inside a text box
        let body = r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold in box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("**Bold in box**"),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_textbox_multiple_paragraphs() {
        // Two paragraphs inside one text box
        let body = r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>First TB para</w:t></w:r></w:p><w:p><w:r><w:t>Second TB para</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("First TB para"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("Second TB para"),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_textbox_via_alternate_content() {
        // Full mc:AlternateContent > Fallback > w:pict > v:shape > v:textbox path
        let body = r#"<mc:AlternateContent><mc:Choice Requires="wps"><w:p><w:r><w:t>DrawingML choice</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>VML text box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p></mc:Fallback></mc:AlternateContent>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("VML text box"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            !result.markdown.contains("DrawingML choice"),
            "Choice content should be hidden, markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_textbox_between_paragraphs() {
        // Text box surrounded by normal paragraphs — verify document flow
        let body = format!(
            "{}{}{}",
            para("Before text box."),
            r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>Inside box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#,
            para("After text box.")
        );
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("Before text box."),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("Inside box"),
            "markdown was: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("After text box."),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_textbox_unicode() {
        // CJK and emoji in text box
        let body = r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>한국어 🚀 中文</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("中文"));
    }

    #[test]
    fn test_docx_textbox_empty() {
        // Empty text box — should not crash or produce garbage output
        let body = r#"<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#;
        let doc = wrap_body(body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Should not contain any text from the empty text box
        assert!(
            result.markdown.trim().is_empty(),
            "expected empty output, got: {}",
            result.markdown
        );
    }

    // ---- Comment extraction tests ----

    /// Build a DOCX with an arbitrary set of extra parts (e.g. comments.xml,
    /// headers, footnotes) in addition to document.xml.
    fn build_test_docx_with_parts(document_xml: &str, extra_parts: &[(&str, &str)]) -> Vec<u8> {
        let byte_parts: Vec<(&str, Vec<u8>)> = extra_parts
            .iter()
            .map(|(p, c)| (*p, c.as_bytes().to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = byte_parts.iter().map(|(p, c)| (*p, c.as_slice())).collect();
        build_test_docx_with_parts_bytes(document_xml, &refs)
    }

    /// Like `build_test_docx_with_parts`, but extra parts are raw bytes (so a
    /// part can contain invalid UTF-8 for best-effort/lenient-decode tests).
    fn build_test_docx_with_parts_bytes(
        document_xml: &str,
        extra_parts: &[(&str, &[u8])],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        )
        .unwrap();

        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        for (path, content) in extra_parts {
            zip.start_file(*path, opts).unwrap();
            zip.write_all(content).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// Build a minimal `word/comments.xml` from (id, author, date, body) tuples.
    /// Each comment's single paragraph carries a `w14:paraId` equal to `pid_<id>`.
    fn comments_xml(entries: &[(&str, &str, &str, &str)]) -> String {
        let mut s = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml">"#,
        );
        for (id, author, date, body) in entries {
            s.push_str(&format!(
                r#"<w:comment w:id="{id}" w:author="{author}" w:date="{date}"><w:p w14:paraId="pid_{id}"><w:r><w:t xml:space="preserve">{body}</w:t></w:r></w:p></w:comment>"#
            ));
        }
        s.push_str("</w:comments>");
        s
    }

    // -- unit: parse_comments_xml --

    #[test]
    fn test_parse_comments_xml_basic() {
        let xml = comments_xml(&[(
            "1",
            "Jane Smith",
            "2024-01-15T09:30:00Z",
            "Please revise this.",
        )]);
        let parsed = parse_comments_xml(&xml);
        let c = parsed.get("1").expect("comment 1");
        assert_eq!(c.author, "Jane Smith");
        assert_eq!(c.date, "2024-01-15T09:30:00Z");
        assert_eq!(c.body, "Please revise this.");
        assert_eq!(c.para_id.as_deref(), Some("pid_1"));
    }

    #[test]
    fn test_parse_comments_xml_no_author_empty() {
        let xml = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="3"><w:p><w:r><w:t>Body</w:t></w:r></w:p></w:comment></w:comments>"#;
        let parsed = parse_comments_xml(xml);
        let c = parsed.get("3").expect("comment 3");
        assert_eq!(c.author, "");
        assert_eq!(c.body, "Body");
    }

    #[test]
    fn test_parse_comments_xml_multi_paragraph_joined() {
        let xml = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="1" w:author="A"><w:p><w:r><w:t>Line one</w:t></w:r></w:p><w:p><w:r><w:t>Line two</w:t></w:r></w:p></w:comment></w:comments>"#;
        let parsed = parse_comments_xml(xml);
        // Paragraphs separated by newline (collapsed downstream).
        assert_eq!(parsed.get("1").unwrap().body, "Line one\nLine two");
    }

    // -- unit: parse_comments_extended --

    #[test]
    fn test_parse_comments_extended_replies() {
        let xml = r#"<?xml version="1.0"?><w15:commentsEx xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"><w15:commentEx w15:paraId="pid_1" w15:done="0"/><w15:commentEx w15:paraId="pid_2" w15:paraIdParent="pid_1" w15:done="0"/></w15:commentsEx>"#;
        let replies = parse_comments_extended(xml);
        assert!(replies.contains("pid_2"));
        assert!(!replies.contains("pid_1"));
    }

    // -- unit: collect_ranges_in_part --

    #[test]
    fn test_collect_ranges_basic_and_order() {
        let body = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="2"/><w:r><w:t>second anchor</w:t></w:r><w:commentRangeEnd w:id="2"/><w:r><w:commentReference w:id="2"/></w:r></w:p><w:p><w:commentRangeStart w:id="1"/><w:r><w:t>first by id</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        );
        let (order, text) = collect_ranges_in_part(&body);
        // Order is by appearance: id 2 appears before id 1.
        assert_eq!(order, vec!["2".to_string(), "1".to_string()]);
        assert_eq!(text.get("2").map(|s| s.as_str()), Some("second anchor"));
        assert_eq!(text.get("1").map(|s| s.as_str()), Some("first by id"));
    }

    #[test]
    fn test_collect_ranges_zero_length_empty() {
        let body = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        );
        let (order, text) = collect_ranges_in_part(&body);
        assert_eq!(order, vec!["1".to_string()]);
        // Zero-length range -> no captured text.
        assert!(text.get("1").map(|s| s.is_empty()).unwrap_or(true));
    }

    #[test]
    fn test_collect_ranges_skips_mc_choice() {
        // The range spans an mc:AlternateContent; Choice text must be excluded.
        let body = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><mc:AlternateContent><mc:Choice Requires="wps"><w:r><w:t>CHOICE</w:t></w:r></mc:Choice><mc:Fallback><w:r><w:t>FALLBACK</w:t></w:r></mc:Fallback></mc:AlternateContent><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        );
        let (_order, text) = collect_ranges_in_part(&body);
        let captured = text.get("1").cloned().unwrap_or_default();
        assert!(captured.contains("FALLBACK"), "got: {captured}");
        assert!(!captured.contains("CHOICE"), "got: {captured}");
    }

    // -- integration --

    /// A document body with one commented range on id 1.
    fn body_with_comment_1() -> String {
        wrap_body(
            r#"<w:p><w:r><w:t xml:space="preserve">Intro paragraph. </w:t></w:r><w:commentRangeStart w:id="1"/><w:r><w:t>the quick brown fox</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        )
    }

    #[test]
    fn test_docx_comments_end_to_end() {
        let doc = body_with_comment_1();
        let cx = comments_xml(&[(
            "1",
            "Jane Smith",
            "2024-01-15T09:30:00Z",
            "Please revise this.",
        )]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("# Comments"),
            "md: {}",
            result.markdown
        );
        assert!(result.markdown.contains("## 1"));
        assert!(
            result
                .markdown
                .contains("- **author**: Jane Smith (2024-01-15T09:30:00Z)")
        );
        assert!(
            result
                .markdown
                .contains("- **comment**: Please revise this.")
        );
        assert!(
            result
                .markdown
                .contains("- **source**: the quick brown fox")
        );
        // Body content is preserved above the section.
        assert!(result.markdown.contains("Intro paragraph."));
        let body_pos = result.markdown.find("Intro paragraph.").unwrap();
        let cmt_pos = result.markdown.find("# Comments").unwrap();
        assert!(body_pos < cmt_pos, "comments must be appended at the end");
    }

    #[test]
    fn test_docx_comments_absent_when_flag_off() {
        let doc = body_with_comment_1();
        let cx = comments_xml(&[("1", "Jane", "2024-01-15T09:30:00Z", "Hi")]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        // Default options: extract_comments = false.
        let result = DocxConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(!result.markdown.contains("# Comments"));
        assert!(!result.plain_text.contains("Comments"));
    }

    #[test]
    fn test_docx_comments_plain_text_stripped() {
        let doc = body_with_comment_1();
        let cx = comments_xml(&[("1", "Jane Smith", "2024-01-15T09:30:00Z", "Revise.")]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(result.plain_text.contains("Comments\n"));
        assert!(
            result
                .plain_text
                .contains("author: Jane Smith (2024-01-15T09:30:00Z)")
        );
        assert!(result.plain_text.contains("comment: Revise."));
        assert!(result.plain_text.contains("source: the quick brown fox"));
        // No markdown markers in the appended plain-text section.
        assert!(!result.plain_text.contains("**"));
        assert!(!result.plain_text.contains("# Comments"));
    }

    #[test]
    fn test_docx_comments_unknown_author() {
        let doc = body_with_comment_1();
        // No author attribute on the comment.
        let cx = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="1"><w:p><w:r><w:t>Anonymous note</w:t></w:r></w:p></w:comment></w:comments>"#;
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("- **author**: Unknown"),
            "md: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_comments_reply_marked() {
        let doc = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>anchor text</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r><w:commentRangeStart w:id="2"/><w:r><w:t>more</w:t></w:r><w:commentRangeEnd w:id="2"/><w:r><w:commentReference w:id="2"/></w:r></w:p>"#,
        );
        let cx = comments_xml(&[
            ("1", "Alice", "2024-01-01T00:00:00Z", "Top level"),
            ("2", "Bob", "2024-01-02T00:00:00Z", "Agreed"),
        ]);
        // comment 2 (paraId pid_2) is a reply to comment 1 (pid_1).
        let cex = r#"<?xml version="1.0"?><w15:commentsEx xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"><w15:commentEx w15:paraId="pid_1"/><w15:commentEx w15:paraId="pid_2" w15:paraIdParent="pid_1"/></w15:commentsEx>"#;
        let data = build_test_docx_with_parts(
            &doc,
            &[
                ("word/comments.xml", &cx),
                ("word/commentsExtended.xml", cex),
            ],
        );
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(result.markdown.contains("- **comment**: Top level"));
        assert!(
            result.markdown.contains("- **comment**: (reply) Agreed"),
            "reply not marked, md: {}",
            result.markdown
        );
    }

    #[test]
    fn test_docx_comments_source_capped_at_200() {
        let long = "x".repeat(300);
        let doc = wrap_body(&format!(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>{long}</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#
        ));
        let cx = comments_xml(&[("1", "A", "2024-01-01T00:00:00Z", "note")]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        // The source line is capped to 200 x's + ellipsis (the full 300-char run
        // still appears in the body paragraph, so we check the source line only).
        let expected = format!("- **source**: {}…", "x".repeat(200));
        assert!(
            result.markdown.contains(&expected),
            "md: {}",
            result.markdown
        );
        let source_line = result
            .markdown
            .lines()
            .find(|l| l.starts_with("- **source**:"))
            .expect("source line");
        // 200 x's capped: line is "- **source**: " + 200 x + "…".
        assert_eq!(source_line.matches('x').count(), 200, "line: {source_line}");
    }

    #[test]
    fn test_docx_comments_orphan_listed_last() {
        // Comment 1 is anchored; comment 9 has no range anywhere (orphan).
        let doc = body_with_comment_1();
        let cx = comments_xml(&[
            ("1", "Anchored", "2024-01-01T00:00:00Z", "Has anchor"),
            ("9", "Orphan", "2024-01-02T00:00:00Z", "No anchor"),
        ]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        let anchored = result.markdown.find("Has anchor").unwrap();
        let orphan = result.markdown.find("No anchor").unwrap();
        assert!(
            anchored < orphan,
            "orphan must come last, md: {}",
            result.markdown
        );
        // Orphan's source is empty.
        assert!(
            result
                .markdown
                .contains("- **comment**: No anchor\n- **source**: \n")
        );
    }

    #[test]
    fn test_docx_comments_header_anchor_after_body() {
        // Comment 1 anchored in body, comment 2 anchored only in a header.
        let doc = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>body anchor</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        );
        let header = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="2"/><w:r><w:t>header anchor</w:t></w:r><w:commentRangeEnd w:id="2"/><w:r><w:commentReference w:id="2"/></w:r></w:p>"#,
        );
        let cx = comments_xml(&[
            ("1", "BodyAuthor", "2024-01-01T00:00:00Z", "Body comment"),
            (
                "2",
                "HeaderAuthor",
                "2024-01-02T00:00:00Z",
                "Header comment",
            ),
        ]);
        let data = build_test_docx_with_parts(
            &doc,
            &[("word/comments.xml", &cx), ("word/header1.xml", &header)],
        );
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        let body_c = result.markdown.find("Body comment").unwrap();
        let header_c = result.markdown.find("Header comment").unwrap();
        assert!(
            body_c < header_c,
            "body comment must precede header comment"
        );
        assert!(result.markdown.contains("- **source**: header anchor"));
    }

    #[test]
    fn test_docx_comments_unknown_range_id_warns() {
        // Body references comment id 5, but comments.xml only has id 1.
        let doc = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="5"/><w:r><w:t>x</w:t></w:r><w:commentRangeEnd w:id="5"/><w:r><w:commentReference w:id="5"/></w:r></w:p>"#,
        );
        let cx = comments_xml(&[("1", "A", "2024-01-01T00:00:00Z", "real")]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::MalformedSegment
                    && w.message.contains("unknown comment id")),
            "expected malformed-segment warning, warnings: {:?}",
            result.warnings
        );
        // Comment 1 is an orphan but still emitted.
        assert!(result.markdown.contains("- **comment**: real"));
    }

    #[test]
    fn test_docx_comments_none_present_no_section() {
        // extract_comments on, but the document has no comments.xml.
        let doc = wrap_body(&para("Just text."));
        let data = build_test_docx_with_parts(&doc, &[]);
        let options = ConversionOptions {
            extract_comments: true,
            ..Default::default()
        };
        let result = DocxConverter.convert(&data, &options).unwrap();
        assert!(!result.markdown.contains("# Comments"));
    }

    // ---- Regression tests for review findings ----

    fn extract_opts() -> ConversionOptions {
        ConversionOptions {
            extract_comments: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_comments_xml_self_closing_t_no_leak() {
        // Finding 5: a self-closing <w:t/> (Empty event) must not leave in_text
        // stuck true and capture stray text from later in the same comment.
        let cx = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="1" w:author="A"><w:p><w:r><w:t/></w:r><w:r><w:t>real</w:t></w:r></w:p></w:comment></w:comments>"#;
        let parsed = parse_comments_xml(cx);
        assert_eq!(parsed.get("1").unwrap().body, "real");
    }

    #[test]
    fn test_parse_comments_xml_unescapes_author() {
        // Finding 8: author/date attributes must be XML-unescaped.
        let cx = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="1" w:author="R&amp;D &lt;Team&gt;"><w:p><w:r><w:t>x</w:t></w:r></w:p></w:comment></w:comments>"#;
        let parsed = parse_comments_xml(cx);
        assert_eq!(parsed.get("1").unwrap().author, "R&D <Team>");
    }

    #[test]
    fn test_parse_comments_xml_para_id_ignores_table_cells() {
        // Finding 9: w14:paraId must come from the last TOP-LEVEL paragraph, not
        // a nested table-cell paragraph.
        let cx = r#"<?xml version="1.0"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w:comment w:id="1" w:author="A"><w:p w14:paraId="TOP">text</w:p><w:tbl><w:tr><w:tc><w:p w14:paraId="CELL"><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:comment></w:comments>"#;
        let parsed = parse_comments_xml(cx);
        assert_eq!(parsed.get("1").unwrap().para_id.as_deref(), Some("TOP"));
    }

    #[test]
    fn test_collect_ranges_end_inside_mc_choice_closes() {
        // Findings 1/3: a commentRangeEnd buried in a skipped mc:Choice must still
        // close the range, so later text does not leak into the source.
        let body = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>anchor</w:t></w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:commentRangeEnd w:id="1"/></mc:Choice><mc:Fallback><w:r><w:t>fb</w:t></w:r></mc:Fallback></mc:AlternateContent><w:r><w:commentReference w:id="1"/></w:r></w:p><w:p><w:r><w:t>UNRELATED LATER TEXT</w:t></w:r></w:p>"#,
        );
        let (_order, text) = collect_ranges_in_part(&body);
        let src = text.get("1").cloned().unwrap_or_default();
        assert!(src.contains("anchor"), "src: {src}");
        assert!(
            !src.contains("UNRELATED"),
            "range leaked past its end: {src}"
        );
    }

    #[test]
    fn test_collect_ranges_unclosed_range_is_bounded() {
        // Finding 1: a range with no commentRangeEnd at all must not absorb an
        // unbounded amount of the part (bounded to SOURCE_CAP*4 bytes).
        let huge = "x".repeat(5000);
        let body = wrap_body(&format!(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>anchor</w:t></w:r><w:r><w:commentReference w:id="1"/></w:r></w:p><w:p><w:r><w:t>{huge}</w:t></w:r></w:p>"#
        ));
        let (_order, text) = collect_ranges_in_part(&body);
        let src = text.get("1").cloned().unwrap_or_default();
        // Bounded well under the 5000-char tail (cap is SOURCE_CAP*4 = 800 bytes).
        assert!(
            src.len() <= comments::SOURCE_CAP * 4 + 16,
            "unbounded: {} bytes",
            src.len()
        );
    }

    #[test]
    fn test_collect_ranges_ignores_loose_text_outside_run() {
        // Finding 6: text outside a <w:r> is not rendered in the body, so it must
        // not appear in the commented-on source either.
        let body = wrap_body(
            r#"<w:p><w:commentRangeStart w:id="1"/><w:t>loose no run</w:t><w:r><w:t>real</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#,
        );
        let (_order, text) = collect_ranges_in_part(&body);
        let src = text.get("1").cloned().unwrap_or_default();
        assert_eq!(
            src, "real",
            "loose text outside a run leaked into source: {src}"
        );
    }

    #[test]
    fn test_docx_comments_author_entity_end_to_end() {
        // Finding 8 end-to-end: "R&D" renders as R&D, not R&amp;D.
        let doc = body_with_comment_1();
        let cx = comments_xml(&[("1", "R&amp;D Team", "2024-01-01T00:00:00Z", "note")]);
        let data = build_test_docx_with_parts(&doc, &[("word/comments.xml", &cx)]);
        let result = DocxConverter.convert(&data, &extract_opts()).unwrap();
        assert!(
            result.markdown.contains("- **author**: R&D Team"),
            "md: {}",
            result.markdown
        );
        assert!(!result.markdown.contains("R&amp;D"));
    }

    #[test]
    fn test_docx_comments_malformed_part_does_not_abort() {
        // Finding 4: a non-UTF-8 comments.xml must not abort conversion; the body
        // still converts (best-effort).
        let doc = wrap_body(&para("Body survives."));
        // comments.xml referencing valid structure but with an invalid byte.
        let mut cx = comments_xml(&[("1", "A", "2024-01-01T00:00:00Z", "ok")]).into_bytes();
        cx.push(0xFF); // invalid UTF-8 trailing byte
        let data = build_test_docx_with_parts_bytes(&doc, &[("word/comments.xml", &cx)]);
        let result = DocxConverter.convert(&data, &extract_opts()).unwrap();
        assert!(result.markdown.contains("Body survives."));
        // Lossy decode still recovers the well-formed comment.
        assert!(result.markdown.contains("# Comments"));
    }
}
