use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::{build_table, format_heading, format_list_item, wrap_formatting};

pub struct DocxConverter;

// ---- Data types ----

/// The kind of block element a paragraph represents.
#[derive(Debug, Clone, PartialEq)]
enum ParagraphKind {
    Normal,
    Heading(u8),                           // level 1..=6
    ListItem { ordered: bool, level: u8 }, // list item from numbering
}

/// A resolved relationship entry from document.xml.rels.
#[derive(Debug, Clone)]
struct Relationship {
    target: String,
    rel_type: String,
}

/// A numbering level definition from numbering.xml.
#[derive(Debug, Clone)]
struct NumberingLevel {
    ordered: bool,
}

// ---- ZIP helpers ----

/// Read a UTF-8 text file from a ZIP archive, returning None if not found.
fn read_zip_text(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Option<String>, ConvertError> {
    let mut file = match archive.by_name(path) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ConvertError::ZipError(e)),
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(Some(buf))
}

/// Read raw bytes from a ZIP archive, returning None if not found.
fn read_zip_bytes(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Option<Vec<u8>>, ConvertError> {
    let mut file = match archive.by_name(path) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ConvertError::ZipError(e)),
    };
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(Some(buf))
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
    if lower.starts_with("heading") {
        let rest = &style_id[7..];
        rest.parse::<u8>().ok().filter(|&l| (1..=9).contains(&l))
    } else {
        None
    }
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

// ---- Relationships parsing ----

/// Parse document.xml.rels to extract a mapping from relationship ID to Relationship.
fn parse_relationships(xml: &str) -> HashMap<String, Relationship> {
    let mut rels = HashMap::new();
    let mut reader = Reader::from_str(xml);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "Relationship" {
                    let mut id = None;
                    let mut target = None;
                    let mut rel_type = String::new();

                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key {
                            "Id" => id = Some(val),
                            "Target" => target = Some(val),
                            "Type" => rel_type = val,
                            _ => {}
                        }
                    }

                    if let (Some(id), Some(target)) = (id, target) {
                        rels.insert(id, Relationship { target, rel_type });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    rels
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
                        if let (Some(ref abs_id), Some(lvl)) = (&current_abstract_id, current_lvl) {
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

// ---- Document body parsing ----

/// Parse the main document.xml body and produce Markdown output.
///
/// Returns (markdown, title, warnings).
fn parse_document(
    xml: &str,
    styles: &HashMap<String, u8>,
    relationships: &HashMap<String, Relationship>,
    numbering: &HashMap<(String, u8), NumberingLevel>,
) -> (String, Option<String>, Vec<ConversionWarning>) {
    let mut reader = Reader::from_str(xml);

    let mut warnings = Vec::new();
    let mut output = String::new();
    let mut title: Option<String> = None;

    // Paragraph-level state
    let mut in_body = false;
    let mut in_paragraph = false;
    let mut current_para_kind = ParagraphKind::Normal;
    let mut current_para_text = String::new();

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
    let mut hyperlink_text = String::new();

    // Paragraph properties state (for list detection)
    let mut in_para_properties = false;
    let mut in_num_pr = false;
    let mut current_num_id: Option<String> = None;
    let mut current_ilvl: Option<u8> = None;

    // List counter tracking: (numId, level) -> counter
    let mut list_counters: HashMap<(String, u8), usize> = HashMap::new();
    // Track if last paragraph was a list item (for single-newline separation)
    let mut last_was_list = false;

    // Table state
    let mut in_table = false;
    let mut in_table_row = false;
    let mut in_table_cell = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell_text = String::new();
    let mut cell_paragraph_count: usize = 0;

    // Drawing/Image state
    let mut in_drawing = false;
    let mut current_image_alt: Option<String> = None;
    let mut current_image_rel_id: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                match local_str {
                    "body" => {
                        in_body = true;
                    }
                    "tbl" if in_body => {
                        in_table = true;
                        table_rows.clear();
                    }
                    "tr" if in_table => {
                        in_table_row = true;
                        current_row.clear();
                    }
                    "tc" if in_table_row => {
                        in_table_cell = true;
                        current_cell_text.clear();
                        cell_paragraph_count = 0;
                    }
                    "p" if in_body => {
                        in_paragraph = true;
                        current_para_kind = ParagraphKind::Normal;
                        current_para_text.clear();
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
                        hyperlink_text.clear();
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
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

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
                        if in_hyperlink {
                            hyperlink_text.push('\n');
                        } else {
                            current_para_text.push('\n');
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
                if in_text && in_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    let formatted = wrap_formatting(&text, current_run_bold, current_run_italic);
                    if in_hyperlink {
                        hyperlink_text.push_str(&formatted);
                    } else {
                        current_para_text.push_str(&formatted);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                match local_str {
                    "body" => {
                        in_body = false;
                    }
                    "tbl" if in_table => {
                        // Render table
                        if !table_rows.is_empty() {
                            let first_row = &table_rows[0];
                            let headers: Vec<&str> = first_row.iter().map(|s| s.as_str()).collect();
                            let data_rows: Vec<Vec<&str>> = table_rows[1..]
                                .iter()
                                .map(|row| row.iter().map(|s| s.as_str()).collect())
                                .collect();
                            let table_md = build_table(&headers, &data_rows);
                            output.push_str(&table_md);
                            output.push('\n');
                        }
                        in_table = false;
                        table_rows.clear();
                        last_was_list = false;
                    }
                    "tr" if in_table_row => {
                        table_rows.push(current_row.clone());
                        current_row.clear();
                        in_table_row = false;
                    }
                    "tc" if in_table_cell => {
                        current_row.push(current_cell_text.trim().to_string());
                        current_cell_text.clear();
                        in_table_cell = false;
                    }
                    "p" if in_paragraph => {
                        // Resolve list item kind from numPr
                        if let (Some(num_id), Some(ilvl)) = (&current_num_id, current_ilvl) {
                            let key = (num_id.clone(), ilvl);
                            let ordered = numbering.get(&key).map(|nl| nl.ordered).unwrap_or(false); // default to bullet
                            current_para_kind = ParagraphKind::ListItem {
                                ordered,
                                level: ilvl,
                            };
                        }

                        if in_table_cell {
                            // In a table cell: accumulate text
                            if cell_paragraph_count > 0 && !current_para_text.is_empty() {
                                current_cell_text.push(' ');
                            }
                            current_cell_text.push_str(current_para_text.trim());
                            cell_paragraph_count += 1;
                        } else {
                            // Normal paragraph finalization
                            let is_list =
                                matches!(current_para_kind, ParagraphKind::ListItem { .. });
                            finalize_paragraph(
                                &current_para_kind,
                                &current_para_text,
                                &mut output,
                                &mut title,
                                &mut list_counters,
                                last_was_list,
                            );
                            last_was_list = is_list;
                        }
                        in_paragraph = false;
                        current_para_text.clear();
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
                        if let Some(ref url) = current_hyperlink_url {
                            current_para_text.push_str(&format!("[{}]({})", hyperlink_text, url));
                        } else {
                            current_para_text.push_str(&hyperlink_text);
                        }
                        in_hyperlink = false;
                        hyperlink_text.clear();
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
                        // Emit image markdown
                        if let Some(ref rel_id) = current_image_rel_id {
                            let filename = relationships
                                .get(rel_id)
                                .map(|r| {
                                    // Extract just the filename from path
                                    r.target.rsplit('/').next().unwrap_or(&r.target).to_string()
                                })
                                .unwrap_or_default();

                            if !filename.is_empty() {
                                let alt = current_image_alt.as_deref().unwrap_or("");
                                let img_md = format!("![{alt}]({filename})");
                                if in_hyperlink {
                                    hyperlink_text.push_str(&img_md);
                                } else {
                                    current_para_text.push_str(&img_md);
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

    (markdown, title, warnings)
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

/// Finalize a paragraph: emit heading, list item, or plain text into the output buffer.
fn finalize_paragraph(
    kind: &ParagraphKind,
    text: &str,
    output: &mut String,
    title: &mut Option<String>,
    list_counters: &mut HashMap<(String, u8), usize>,
    last_was_list: bool,
) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    match kind {
        ParagraphKind::Heading(level) => {
            if last_was_list {
                output.push('\n');
            }
            output.push_str(&format_heading(*level, trimmed));
            output.push('\n');
            if *level == 1 && title.is_none() {
                *title = Some(trimmed.to_string());
            }
        }
        ParagraphKind::ListItem { ordered, level } => {
            let counter = if *ordered {
                // For simplicity, use a global counter per level
                let key = ("__global__".to_string(), *level);
                let c = list_counters.entry(key).or_insert(0);
                *c += 1;
                *c
            } else {
                1
            };
            let item = format_list_item(*level, *ordered, counter, trimmed);
            output.push_str(&item);
            output.push('\n');
        }
        ParagraphKind::Normal => {
            if last_was_list {
                output.push('\n');
            }
            output.push_str(trimmed);
            output.push_str("\n\n");
        }
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

        let (markdown, title, mut warnings) =
            parse_document(&document_xml, &styles, &relationships, &numbering);

        // 5. Extract embedded images if requested or if describer needs them
        let need_image_bytes = options.extract_images || options.image_describer.is_some();
        let mut images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut image_data_for_describer: Vec<(String, Vec<u8>)> = Vec::new();
        if need_image_bytes {
            let mut total_image_bytes: usize = 0;
            for rel in relationships.values() {
                if !rel.rel_type.contains("image") {
                    continue;
                }
                if total_image_bytes >= options.max_total_image_bytes {
                    break;
                }
                let image_path = format!("word/{}", rel.target);
                if let Ok(Some(img_data)) = read_zip_bytes(&mut archive, &image_path) {
                    total_image_bytes += img_data.len();
                    if total_image_bytes <= options.max_total_image_bytes {
                        let filename = rel
                            .target
                            .rsplit('/')
                            .next()
                            .unwrap_or(&rel.target)
                            .to_string();
                        if options.extract_images {
                            images.push((filename.clone(), img_data.clone()));
                        }
                        if options.image_describer.is_some() {
                            image_data_for_describer.push((filename, img_data));
                        }
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

        // 6. Generate image descriptions if describer is set
        let mut markdown = markdown;
        if let Some(ref describer) = options.image_describer {
            for (filename, img_data) in &image_data_for_describer {
                let mime = crate::converter::mime_from_image(filename, img_data);
                let prompt = "Describe this image concisely for use as alt text.";
                match describer.describe(img_data, mime, prompt) {
                    Ok(description) => {
                        // Replace all occurrences of ![...](filename) with ![description](filename)
                        let pattern = format!("]({})", filename);
                        let mut result = String::new();
                        let mut remaining = markdown.as_str();
                        while let Some(pos) = remaining.find(&pattern) {
                            // Find the opening ![
                            let before = &remaining[..pos];
                            if let Some(bracket_pos) = before.rfind("![") {
                                result.push_str(&remaining[..bracket_pos]);
                                result.push_str(&format!("![{}]({})", description, filename));
                                remaining = &remaining[pos + pattern.len()..];
                            } else {
                                result.push_str(&remaining[..pos + pattern.len()]);
                                remaining = &remaining[pos + pattern.len()..];
                            }
                        }
                        result.push_str(remaining);
                        markdown = result;
                    }
                    Err(e) => {
                        warnings.push(ConversionWarning {
                            code: WarningCode::SkippedElement,
                            message: format!("image description failed for '{}': {}", filename, e),
                            location: Some(filename.clone()),
                        });
                    }
                }
            }
        }

        Ok(ConversionResult {
            markdown,
            title,
            images,
            warnings,
        })
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
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

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
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
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
        assert!(result
            .markdown
            .contains("First paragraph.\n\nSecond paragraph."));
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
        assert!(result
            .markdown
            .contains("[**Bold Link**](https://example.com)"));
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
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

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
}
