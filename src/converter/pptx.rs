use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::build_table;

pub struct PptxConverter;

// ---- Data types ----

/// A resolved relationship entry from a .rels file.
#[derive(Debug, Clone)]
struct Relationship {
    target: String,
    rel_type: String,
}

/// Information about a slide in presentation order.
#[derive(Debug, Clone)]
struct SlideInfo {
    number: usize,
    path: String,
}

/// The type of placeholder in a shape.
#[derive(Debug, Clone, PartialEq)]
enum PlaceholderType {
    Title,
    CenterTitle,
    SubTitle,
    Body,
    Other,
}

/// Content extracted from a single shape on a slide.
#[derive(Debug, Clone)]
enum ShapeContent {
    Title(String),
    Body(String),
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Image {
        rel_id: String,
    },
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

// ---- Path helpers ----

/// Derive the .rels path for a given file path.
///
/// Example: `ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`
fn derive_rels_path(file_path: &str) -> String {
    if let Some(pos) = file_path.rfind('/') {
        let dir = &file_path[..pos];
        let filename = &file_path[pos + 1..];
        format!("{dir}/_rels/{filename}.rels")
    } else {
        format!("_rels/{file_path}.rels")
    }
}

/// Resolve a relative path target against a base path.
///
/// Example: base=`ppt/slides/slide1.xml`, target=`../media/image1.png`
///          → `ppt/media/image1.png`
fn resolve_relative_path(base: &str, target: &str) -> String {
    if !target.starts_with("../") {
        // Absolute or same-directory target
        if let Some(pos) = base.rfind('/') {
            return format!("{}/{target}", &base[..pos]);
        }
        return target.to_string();
    }

    // Walk up for each "../" prefix
    let mut base_parts: Vec<&str> = base.split('/').collect();
    // Remove the filename from base
    base_parts.pop();

    let mut target_remaining = target;
    while let Some(rest) = target_remaining.strip_prefix("../") {
        base_parts.pop();
        target_remaining = rest;
    }

    if base_parts.is_empty() {
        target_remaining.to_string()
    } else {
        format!("{}/{target_remaining}", base_parts.join("/"))
    }
}

// ---- Relationships parsing ----

/// Parse a .rels XML file to extract relationship ID → Relationship mapping.
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

// ---- Slide order resolution ----

/// Parse presentation.xml and its rels to determine slide order.
///
/// Returns slides in presentation order (as defined by `<p:sldIdLst>`).
fn resolve_slide_order(
    pres_xml: &str,
    pres_rels: &HashMap<String, Relationship>,
) -> Vec<SlideInfo> {
    let mut reader = Reader::from_str(pres_xml);
    let mut rel_ids: Vec<String> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "sldId" {
                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        if key == "r:id" || key.ends_with(":id") {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            rel_ids.push(val);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    let mut slides = Vec::new();
    for (i, rid) in rel_ids.iter().enumerate() {
        if let Some(rel) = pres_rels.get(rid) {
            // Target is relative to ppt/ directory, e.g., "slides/slide1.xml"
            let path = if rel.target.starts_with("ppt/") {
                rel.target.clone()
            } else {
                format!("ppt/{}", rel.target)
            };
            slides.push(SlideInfo {
                number: i + 1,
                path,
            });
        }
    }

    slides
}

// ---- Slide content parsing ----

/// Parse a slide XML and extract shape contents in document order.
///
/// Returns (shapes, warnings).
fn parse_slide(xml: &str) -> (Vec<ShapeContent>, Vec<ConversionWarning>) {
    let mut reader = Reader::from_str(xml);
    let mut shapes: Vec<ShapeContent> = Vec::new();
    let mut warnings: Vec<ConversionWarning> = Vec::new();

    // Shape-level state
    let mut in_shape = false; // inside <p:sp>
    let mut in_graphic_frame = false; // inside <p:graphicFrame>
    let mut in_picture = false; // inside <p:pic>
    let mut placeholder_type: Option<PlaceholderType> = None;

    // Text body state
    let mut in_text_body = false;
    let mut in_paragraph = false;
    let mut in_run = false;
    let mut in_text = false;
    let mut current_paragraph = String::new();
    let mut shape_paragraphs: Vec<String> = Vec::new();

    // Table state
    let mut in_table = false;
    let mut in_table_row = false;
    let mut in_table_cell = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    // Track text state within table cells
    let mut in_cell_paragraph = false;
    let mut in_cell_run = false;
    let mut in_cell_text = false;

    // Image state
    let mut current_blip_rel_id: Option<String> = None;

    // Track depth for nested elements
    let mut shape_depth: u32 = 0;
    let mut graphic_frame_depth: u32 = 0;
    let mut picture_depth: u32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                match local_str {
                    "sp" if !in_shape && !in_graphic_frame && !in_picture => {
                        in_shape = true;
                        shape_depth = 1;
                        placeholder_type = None;
                        shape_paragraphs.clear();
                    }
                    "graphicFrame" if !in_shape && !in_graphic_frame && !in_picture => {
                        in_graphic_frame = true;
                        graphic_frame_depth = 1;
                    }
                    "pic" if !in_shape && !in_graphic_frame && !in_picture => {
                        in_picture = true;
                        picture_depth = 1;
                        current_blip_rel_id = None;
                    }
                    _ if in_shape => {
                        shape_depth += 1;
                        handle_shape_start(
                            local_str,
                            e,
                            &mut placeholder_type,
                            &mut in_text_body,
                            &mut in_paragraph,
                            &mut in_run,
                            &mut in_text,
                            &mut current_paragraph,
                        );
                    }
                    _ if in_graphic_frame => {
                        graphic_frame_depth += 1;
                        handle_graphic_frame_start(
                            local_str,
                            &mut in_table,
                            &mut in_table_row,
                            &mut in_table_cell,
                            &mut in_cell_paragraph,
                            &mut in_cell_run,
                            &mut in_cell_text,
                            &mut current_cell,
                            &mut current_row,
                            &mut table_rows,
                        );
                    }
                    _ if in_picture => {
                        picture_depth += 1;
                        handle_picture_start(local_str, e, &mut current_blip_rel_id);
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if in_shape {
                    handle_shape_empty(
                        local_str,
                        e,
                        &mut placeholder_type,
                        in_run,
                        &mut current_paragraph,
                    );
                } else if in_graphic_frame {
                    handle_graphic_frame_empty(local_str, in_cell_run, &mut current_cell);
                } else if in_picture {
                    handle_picture_start(local_str, e, &mut current_blip_rel_id);
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_shape && in_text && in_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    current_paragraph.push_str(&text);
                } else if in_graphic_frame && in_cell_text && in_cell_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    current_cell.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if in_shape {
                    shape_depth -= 1;

                    match local_str {
                        "t" => in_text = false,
                        "r" => {
                            in_run = false;
                            in_text = false;
                        }
                        "p" if in_paragraph => {
                            let para = current_paragraph.clone();
                            if !para.is_empty() {
                                shape_paragraphs.push(para);
                            }
                            current_paragraph.clear();
                            in_paragraph = false;
                        }
                        "txBody" => in_text_body = false,
                        _ => {}
                    }

                    if shape_depth == 0 {
                        // Finalize shape
                        let content = finalize_shape(&placeholder_type, &shape_paragraphs);
                        if let Some(c) = content {
                            shapes.push(c);
                        }
                        in_shape = false;
                        placeholder_type = None;
                        shape_paragraphs.clear();
                        in_text_body = false;
                        in_paragraph = false;
                        in_run = false;
                        in_text = false;
                    }
                } else if in_graphic_frame {
                    graphic_frame_depth -= 1;

                    match local_str {
                        "t" if in_table_cell => in_cell_text = false,
                        "r" if in_table_cell => {
                            in_cell_run = false;
                            in_cell_text = false;
                        }
                        "p" if in_cell_paragraph => {
                            in_cell_paragraph = false;
                        }
                        "tc" if in_table_cell => {
                            current_row.push(current_cell.trim().to_string());
                            current_cell.clear();
                            in_table_cell = false;
                            in_cell_paragraph = false;
                            in_cell_run = false;
                            in_cell_text = false;
                        }
                        "tr" if in_table_row => {
                            table_rows.push(current_row.clone());
                            current_row.clear();
                            in_table_row = false;
                        }
                        "tbl" if in_table => {
                            // Finalize table
                            if !table_rows.is_empty() {
                                let headers = table_rows[0].clone();
                                let data_rows = if table_rows.len() > 1 {
                                    table_rows[1..].to_vec()
                                } else {
                                    Vec::new()
                                };
                                shapes.push(ShapeContent::Table {
                                    headers,
                                    rows: data_rows,
                                });
                            }
                            table_rows.clear();
                            in_table = false;
                        }
                        _ => {}
                    }

                    if graphic_frame_depth == 0 {
                        in_graphic_frame = false;
                        in_table = false;
                        in_table_row = false;
                        in_table_cell = false;
                        in_cell_paragraph = false;
                        in_cell_run = false;
                        in_cell_text = false;
                    }
                } else if in_picture {
                    picture_depth -= 1;

                    if picture_depth == 0 {
                        if let Some(rel_id) = current_blip_rel_id.take() {
                            shapes.push(ShapeContent::Image { rel_id });
                        }
                        in_picture = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                warnings.push(ConversionWarning {
                    code: WarningCode::MalformedSegment,
                    message: format!("XML parse error in slide: {err}"),
                    location: None,
                });
                break;
            }
            _ => {}
        }
    }

    (shapes, warnings)
}

/// Handle a Start event inside a <p:sp> shape.
#[allow(clippy::too_many_arguments)]
fn handle_shape_start(
    local_str: &str,
    e: &quick_xml::events::BytesStart,
    placeholder_type: &mut Option<PlaceholderType>,
    in_text_body: &mut bool,
    in_paragraph: &mut bool,
    in_run: &mut bool,
    in_text: &mut bool,
    current_paragraph: &mut String,
) {
    match local_str {
        "ph" => {
            // <p:ph type="title"/> or <p:ph type="ctrTitle"/> etc.
            let mut ph_type = PlaceholderType::Other;
            for attr in e.attributes().flatten() {
                let local_name = attr.key.local_name();
                let key = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if key == "type" {
                    let val = String::from_utf8_lossy(&attr.value);
                    ph_type = match val.as_ref() {
                        "title" => PlaceholderType::Title,
                        "ctrTitle" => PlaceholderType::CenterTitle,
                        "subTitle" => PlaceholderType::SubTitle,
                        "body" => PlaceholderType::Body,
                        _ => PlaceholderType::Other,
                    };
                }
            }
            *placeholder_type = Some(ph_type);
        }
        "txBody" => {
            *in_text_body = true;
        }
        "p" if *in_text_body => {
            *in_paragraph = true;
            current_paragraph.clear();
        }
        "r" if *in_paragraph => {
            *in_run = true;
        }
        "t" if *in_run => {
            *in_text = true;
        }
        _ => {}
    }
}

/// Handle an Empty event inside a <p:sp> shape.
fn handle_shape_empty(
    local_str: &str,
    e: &quick_xml::events::BytesStart,
    placeholder_type: &mut Option<PlaceholderType>,
    in_run: bool,
    current_paragraph: &mut String,
) {
    match local_str {
        "ph" => {
            let mut ph_type = PlaceholderType::Other;
            for attr in e.attributes().flatten() {
                let local_name = attr.key.local_name();
                let key = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                if key == "type" {
                    let val = String::from_utf8_lossy(&attr.value);
                    ph_type = match val.as_ref() {
                        "title" => PlaceholderType::Title,
                        "ctrTitle" => PlaceholderType::CenterTitle,
                        "subTitle" => PlaceholderType::SubTitle,
                        "body" => PlaceholderType::Body,
                        _ => PlaceholderType::Other,
                    };
                }
            }
            *placeholder_type = Some(ph_type);
        }
        "br" if in_run => {
            current_paragraph.push('\n');
        }
        _ => {}
    }
}

/// Handle a Start event inside a <p:graphicFrame>.
#[allow(clippy::too_many_arguments)]
fn handle_graphic_frame_start(
    local_str: &str,
    in_table: &mut bool,
    in_table_row: &mut bool,
    in_table_cell: &mut bool,
    in_cell_paragraph: &mut bool,
    in_cell_run: &mut bool,
    in_cell_text: &mut bool,
    current_cell: &mut String,
    current_row: &mut Vec<String>,
    table_rows: &mut Vec<Vec<String>>,
) {
    match local_str {
        "tbl" => {
            *in_table = true;
            table_rows.clear();
        }
        "tr" if *in_table => {
            *in_table_row = true;
            current_row.clear();
        }
        "tc" if *in_table_row => {
            *in_table_cell = true;
            current_cell.clear();
        }
        "p" if *in_table_cell => {
            // Add space separator between paragraphs in the same cell
            if !current_cell.is_empty() {
                current_cell.push(' ');
            }
            *in_cell_paragraph = true;
        }
        "r" if *in_cell_paragraph => {
            *in_cell_run = true;
        }
        "t" if *in_cell_run => {
            *in_cell_text = true;
        }
        _ => {}
    }
}

/// Handle an Empty event inside a <p:graphicFrame>.
fn handle_graphic_frame_empty(local_str: &str, in_cell_run: bool, current_cell: &mut String) {
    if local_str == "br" && in_cell_run {
        current_cell.push(' ');
    }
}

/// Handle a Start/Empty event inside a <p:pic>.
fn handle_picture_start(
    local_str: &str,
    e: &quick_xml::events::BytesStart,
    current_blip_rel_id: &mut Option<String>,
) {
    if local_str == "blip" {
        for attr in e.attributes().flatten() {
            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
            if key == "r:embed" || key.ends_with(":embed") {
                let val = String::from_utf8_lossy(&attr.value).to_string();
                *current_blip_rel_id = Some(val);
            }
        }
    }
}

/// Finalize a shape into a ShapeContent based on its placeholder type and paragraphs.
fn finalize_shape(
    placeholder_type: &Option<PlaceholderType>,
    paragraphs: &[String],
) -> Option<ShapeContent> {
    if paragraphs.is_empty() {
        return None;
    }

    let text = paragraphs.join("\n");
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    match placeholder_type {
        Some(PlaceholderType::Title) | Some(PlaceholderType::CenterTitle) => {
            Some(ShapeContent::Title(text))
        }
        Some(PlaceholderType::SubTitle) => Some(ShapeContent::Body(text)),
        Some(PlaceholderType::Body) => Some(ShapeContent::Body(text)),
        Some(PlaceholderType::Other) | None => {
            // Shapes without a known placeholder type are treated as body text
            Some(ShapeContent::Body(text))
        }
    }
}

// ---- Notes parsing ----

/// Parse a notes slide XML and extract the body text.
///
/// Only extracts text from the body placeholder (ignores slide number placeholders).
fn parse_notes(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);

    let mut in_shape = false;
    let mut shape_depth: u32 = 0;
    let mut is_body_placeholder = false;

    let mut in_text_body = false;
    let mut in_paragraph = false;
    let mut in_run = false;
    let mut in_text = false;
    let mut current_paragraph = String::new();
    let mut paragraphs: Vec<String> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "sp" && !in_shape {
                    in_shape = true;
                    shape_depth = 1;
                    is_body_placeholder = false;
                    paragraphs.clear();
                } else if in_shape {
                    shape_depth += 1;
                    match local_str {
                        "ph" => {
                            for attr in e.attributes().flatten() {
                                let local_name = attr.key.local_name();
                                let key = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                                if key == "type" {
                                    let val = String::from_utf8_lossy(&attr.value);
                                    if val.as_ref() == "body" {
                                        is_body_placeholder = true;
                                    }
                                }
                            }
                        }
                        "txBody" => in_text_body = true,
                        "p" if in_text_body => {
                            in_paragraph = true;
                            current_paragraph.clear();
                        }
                        "r" if in_paragraph => in_run = true,
                        "t" if in_run => in_text = true,
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if in_shape {
                    if local_str == "ph" {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let key = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if key == "type" {
                                let val = String::from_utf8_lossy(&attr.value);
                                if val.as_ref() == "body" {
                                    is_body_placeholder = true;
                                }
                            }
                        }
                    } else if local_str == "br" && in_run {
                        current_paragraph.push('\n');
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_shape && in_text && in_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    current_paragraph.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if in_shape {
                    shape_depth -= 1;

                    match local_str {
                        "t" => in_text = false,
                        "r" => {
                            in_run = false;
                            in_text = false;
                        }
                        "p" if in_paragraph => {
                            if !current_paragraph.is_empty() {
                                paragraphs.push(current_paragraph.clone());
                            }
                            current_paragraph.clear();
                            in_paragraph = false;
                        }
                        "txBody" => in_text_body = false,
                        _ => {}
                    }

                    if shape_depth == 0 {
                        if is_body_placeholder && !paragraphs.is_empty() {
                            let text = paragraphs.join("\n").trim().to_string();
                            if !text.is_empty() {
                                return Some(text);
                            }
                        }
                        in_shape = false;
                        is_body_placeholder = false;
                        paragraphs.clear();
                        in_text_body = false;
                        in_paragraph = false;
                        in_run = false;
                        in_text = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    None
}

/// Find the notes slide path from a slide's relationships.
fn resolve_notes_path(slide_rels: &HashMap<String, Relationship>) -> Option<String> {
    for rel in slide_rels.values() {
        if rel.rel_type.contains("notesSlide") {
            return Some(rel.target.clone());
        }
    }
    None
}

// ---- Markdown rendering ----

/// Render a single slide's content as Markdown.
fn render_slide(
    number: usize,
    shapes: &[ShapeContent],
    notes: &Option<String>,
    image_filenames: &HashMap<String, String>,
) -> String {
    let mut out = String::new();

    // Find the title
    let title = shapes.iter().find_map(|s| {
        if let ShapeContent::Title(t) = s {
            Some(t.as_str())
        } else {
            None
        }
    });

    // Slide heading
    if let Some(title_text) = title {
        out.push_str(&format!("## Slide {number}: {title_text}\n\n"));
    } else {
        out.push_str(&format!("## Slide {number}\n\n"));
    }

    // Body content, tables, and images (skip title since it's already in heading)
    for shape in shapes {
        match shape {
            ShapeContent::Title(_) => {} // Already rendered as heading
            ShapeContent::Body(text) => {
                out.push_str(text);
                out.push_str("\n\n");
            }
            ShapeContent::Table { headers, rows } => {
                let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
                let row_refs: Vec<Vec<&str>> = rows
                    .iter()
                    .map(|r| r.iter().map(|s| s.as_str()).collect())
                    .collect();
                out.push_str(&build_table(&header_refs, &row_refs));
                out.push('\n');
            }
            ShapeContent::Image { rel_id } => {
                if let Some(filename) = image_filenames.get(rel_id) {
                    out.push_str(&format!("![]({filename})\n\n"));
                }
            }
        }
    }

    // Notes
    if let Some(notes_text) = notes {
        let lines: Vec<&str> = notes_text.lines().collect();
        if !lines.is_empty() {
            out.push_str(&format!("> Note: {}", lines[0]));
            for line in &lines[1..] {
                out.push_str(&format!("\n> {line}"));
            }
            out.push_str("\n\n");
        }
    }

    // Trim trailing whitespace
    out.trim_end().to_string()
}

// ---- Converter trait impl ----

impl Converter for PptxConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["pptx"]
    }

    fn convert(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor)?;

        let mut warnings: Vec<ConversionWarning> = Vec::new();
        let mut images: Vec<(String, Vec<u8>)> = Vec::new();

        // 1. Parse presentation.xml.rels (optional but needed for slide resolution)
        let pres_rels = match read_zip_text(&mut archive, "ppt/_rels/presentation.xml.rels")? {
            Some(xml) => parse_relationships(&xml),
            None => HashMap::new(),
        };

        // 2. Parse presentation.xml (required)
        let pres_xml = read_zip_text(&mut archive, "ppt/presentation.xml")?.ok_or_else(|| {
            ConvertError::MalformedDocument {
                reason: "missing ppt/presentation.xml".to_string(),
            }
        })?;

        // 3. Resolve slide order
        let slides = resolve_slide_order(&pres_xml, &pres_rels);

        if slides.is_empty() {
            return Ok(ConversionResult {
                markdown: String::new(),
                ..Default::default()
            });
        }

        // 4. Process each slide
        let mut slide_markdowns: Vec<String> = Vec::new();
        let mut document_title: Option<String> = None;
        let mut total_image_bytes: usize = 0;

        for slide_info in &slides {
            // Read slide XML
            let slide_xml = match read_zip_text(&mut archive, &slide_info.path)? {
                Some(xml) => xml,
                None => {
                    warnings.push(ConversionWarning {
                        code: WarningCode::SkippedElement,
                        message: format!("slide file not found: {}", slide_info.path),
                        location: Some(slide_info.path.clone()),
                    });
                    continue;
                }
            };

            // Parse slide content
            let (shapes, mut slide_warnings) = parse_slide(&slide_xml);
            warnings.append(&mut slide_warnings);

            // Read slide rels for notes and images
            let slide_rels_path = derive_rels_path(&slide_info.path);
            let slide_rels = match read_zip_text(&mut archive, &slide_rels_path)? {
                Some(xml) => parse_relationships(&xml),
                None => HashMap::new(),
            };

            // Parse notes
            let notes = if let Some(notes_target) = resolve_notes_path(&slide_rels) {
                let notes_path = resolve_relative_path(&slide_info.path, &notes_target);
                match read_zip_text(&mut archive, &notes_path)? {
                    Some(xml) => parse_notes(&xml),
                    None => None,
                }
            } else {
                None
            };

            // Resolve image filenames and optionally extract image data
            let mut image_filenames: HashMap<String, String> = HashMap::new();
            for shape in &shapes {
                if let ShapeContent::Image { rel_id } = shape {
                    if let Some(rel) = slide_rels.get(rel_id) {
                        let image_path = resolve_relative_path(&slide_info.path, &rel.target);
                        let filename = image_path.rsplit('/').next().unwrap_or(&image_path);
                        image_filenames.insert(rel_id.clone(), filename.to_string());

                        // Extract image data if requested
                        if options.extract_images
                            && total_image_bytes < options.max_total_image_bytes
                        {
                            if let Ok(Some(img_data)) = read_zip_bytes(&mut archive, &image_path) {
                                total_image_bytes += img_data.len();
                                if total_image_bytes <= options.max_total_image_bytes {
                                    images.push((filename.to_string(), img_data));
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
                }
            }

            // Set document title from first slide's title
            if document_title.is_none() {
                document_title = shapes.iter().find_map(|s| {
                    if let ShapeContent::Title(t) = s {
                        Some(t.clone())
                    } else {
                        None
                    }
                });
            }

            let slide_md = render_slide(slide_info.number, &shapes, &notes, &image_filenames);
            slide_markdowns.push(slide_md);
        }

        // Join slides with horizontal rule separator
        let markdown = slide_markdowns.join("\n\n---\n\n");
        let markdown = if markdown.is_empty() {
            markdown
        } else {
            format!("{markdown}\n")
        };

        Ok(ConversionResult {
            markdown,
            title: document_title,
            images,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Helper: build minimal PPTX ZIP in memory ----

    struct TestSlide<'a> {
        title: Option<&'a str>,
        body_texts: Vec<&'a str>,
        notes: Option<&'a str>,
        table: Option<TestTable<'a>>,
        images: Vec<&'a str>, // rel IDs for image references
    }

    struct TestTable<'a> {
        headers: Vec<&'a str>,
        rows: Vec<Vec<&'a str>>,
    }

    /// Build a minimal PPTX ZIP in memory.
    fn build_test_pptx(slides: &[TestSlide]) -> Vec<u8> {
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
        ct.push_str("</Types>");
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // Build presentation.xml with slide references
        let mut pres_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst>"#,
        );
        let mut pres_rels_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );

        for (i, slide) in slides.iter().enumerate() {
            let slide_num = i + 1;
            let rid = format!("rId{slide_num}");
            let slide_id = 256 + i;

            pres_xml.push_str(&format!(r#"<p:sldId id="{slide_id}" r:id="{rid}"/>"#));
            pres_rels_xml.push_str(&format!(
                r#"<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{slide_num}.xml"/>"#
            ));

            // Build slide XML
            let slide_xml = build_slide_xml(slide);
            zip.start_file(format!("ppt/slides/slide{slide_num}.xml"), opts)
                .unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();

            // Build slide rels if notes or images exist
            if slide.notes.is_some() || !slide.images.is_empty() {
                let mut slide_rels = String::from(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                );

                if slide.notes.is_some() {
                    slide_rels.push_str(&format!(
                        r#"<Relationship Id="rIdNotes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{slide_num}.xml"/>"#
                    ));
                }

                for (img_idx, _) in slide.images.iter().enumerate() {
                    let img_rid = format!("rIdImg{}", img_idx + 1);
                    slide_rels.push_str(&format!(
                        r#"<Relationship Id="{img_rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image{}.png"/>"#,
                        img_idx + 1
                    ));
                }

                slide_rels.push_str("</Relationships>");
                zip.start_file(format!("ppt/slides/_rels/slide{slide_num}.xml.rels"), opts)
                    .unwrap();
                zip.write_all(slide_rels.as_bytes()).unwrap();
            }

            // Build notes slide if present
            if let Some(notes_text) = slide.notes {
                let notes_xml = build_notes_xml(notes_text);
                zip.start_file(format!("ppt/notesSlides/notesSlide{slide_num}.xml"), opts)
                    .unwrap();
                zip.write_all(notes_xml.as_bytes()).unwrap();
            }
        }

        pres_xml.push_str("</p:sldIdLst></p:presentation>");
        pres_rels_xml.push_str("</Relationships>");

        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(pres_xml.as_bytes()).unwrap();

        zip.start_file("ppt/_rels/presentation.xml.rels", opts)
            .unwrap();
        zip.write_all(pres_rels_xml.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// Build the XML for a single slide.
    fn build_slide_xml(slide: &TestSlide) -> String {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree>"#,
        );

        // Title shape
        if let Some(title) = slide.title {
            xml.push_str(&format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp>"#
            ));
        }

        // Body text shapes
        for text in &slide.body_texts {
            xml.push_str(&format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
            ));
        }

        // Table
        if let Some(table) = &slide.table {
            xml.push_str(r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="3" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl>"#);

            // Header row
            xml.push_str("<a:tr>");
            for header in &table.headers {
                xml.push_str(&format!(
                    r#"<a:tc><a:txBody><a:p><a:r><a:t>{header}</a:t></a:r></a:p></a:txBody></a:tc>"#
                ));
            }
            xml.push_str("</a:tr>");

            // Data rows
            for row in &table.rows {
                xml.push_str("<a:tr>");
                for cell in row {
                    xml.push_str(&format!(
                        r#"<a:tc><a:txBody><a:p><a:r><a:t>{cell}</a:t></a:r></a:p></a:txBody></a:tc>"#
                    ));
                }
                xml.push_str("</a:tr>");
            }

            xml.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
        }

        // Image shapes
        for (idx, rel_id) in slide.images.iter().enumerate() {
            xml.push_str(&format!(
                r#"<p:pic><p:nvPicPr><p:cNvPr id="{}" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{rel_id}"/></p:blipFill></p:pic>"#,
                10 + idx
            ));
        }

        xml.push_str("</p:spTree></p:cSld></p:sld>");
        xml
    }

    /// Build the XML for a notes slide.
    fn build_notes_xml(text: &str) -> String {
        // Split text by newlines to create separate paragraphs
        let paragraphs: Vec<&str> = text.lines().collect();
        let mut para_xml = String::new();
        for p in &paragraphs {
            para_xml.push_str(&format!(r#"<a:p><a:r><a:t>{p}</a:t></a:r></a:p>"#));
        }

        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Slide Number"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>1</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody>{para_xml}</p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#
        )
    }

    // ---- Tests ----

    #[test]
    fn test_pptx_supported_extensions() {
        let converter = PptxConverter;
        assert_eq!(converter.supported_extensions(), &["pptx"]);
    }

    #[test]
    fn test_pptx_can_convert() {
        let converter = PptxConverter;
        assert!(converter.can_convert("pptx", &[]));
        assert!(!converter.can_convert("docx", &[]));
        assert!(!converter.can_convert("xlsx", &[]));
        assert!(!converter.can_convert("pdf", &[]));
    }

    #[test]
    fn test_pptx_invalid_data_returns_error() {
        let converter = PptxConverter;
        let result = converter.convert(b"not a valid pptx file", &ConversionOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_pptx_empty_presentation() {
        let data = build_test_pptx(&[]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "");
        assert!(result.title.is_none());
    }

    #[test]
    fn test_pptx_single_slide_title_and_body() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Hello World"),
            body_texts: vec!["This is the body text."],
            notes: None,
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## Slide 1: Hello World"));
        assert!(result.markdown.contains("This is the body text."));
    }

    #[test]
    fn test_pptx_multiple_slides_with_separator() {
        let data = build_test_pptx(&[
            TestSlide {
                title: Some("First"),
                body_texts: vec!["Body one."],
                notes: None,
                table: None,
                images: vec![],
            },
            TestSlide {
                title: Some("Second"),
                body_texts: vec!["Body two."],
                notes: None,
                table: None,
                images: vec![],
            },
        ]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## Slide 1: First"));
        assert!(result.markdown.contains("## Slide 2: Second"));
        assert!(result.markdown.contains("\n\n---\n\n"));
    }

    #[test]
    fn test_pptx_slide_without_title() {
        let data = build_test_pptx(&[TestSlide {
            title: None,
            body_texts: vec!["Just body text."],
            notes: None,
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## Slide 1\n"));
        // Should NOT have a colon when no title
        assert!(!result.markdown.contains("## Slide 1:"));
    }

    #[test]
    fn test_pptx_title_center_title() {
        // Build a PPTX with ctrTitle placeholder type
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Center Title</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        assert_eq!(shapes.len(), 1);
        match &shapes[0] {
            ShapeContent::Title(text) => assert_eq!(text, "Center Title"),
            other => panic!("expected Title, got {:?}", other),
        }
    }

    #[test]
    fn test_pptx_document_title_from_first_slide() {
        let data = build_test_pptx(&[
            TestSlide {
                title: Some("Presentation Title"),
                body_texts: vec![],
                notes: None,
                table: None,
                images: vec![],
            },
            TestSlide {
                title: Some("Second Slide"),
                body_texts: vec![],
                notes: None,
                table: None,
                images: vec![],
            },
        ]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.title, Some("Presentation Title".to_string()));
    }

    #[test]
    fn test_pptx_body_text_multiple_paragraphs() {
        // Build slide XML with multiple paragraphs in body
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>First paragraph</a:t></a:r></a:p><a:p><a:r><a:t>Second paragraph</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        assert_eq!(shapes.len(), 1);
        match &shapes[0] {
            ShapeContent::Body(text) => {
                assert!(text.contains("First paragraph"));
                assert!(text.contains("Second paragraph"));
                assert!(text.contains('\n'));
            }
            other => panic!("expected Body, got {:?}", other),
        }
    }

    #[test]
    fn test_pptx_body_text_multiple_runs_joined() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Hello </a:t></a:r><a:r><a:t>World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        match &shapes[0] {
            ShapeContent::Body(text) => assert_eq!(text, "Hello World"),
            other => panic!("expected Body, got {:?}", other),
        }
    }

    #[test]
    fn test_pptx_subtitle_treated_as_body() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Main Title</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="2" name="Subtitle"/><p:cNvSpPr/><p:nvPr><p:ph type="subTitle"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>The subtitle</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        assert_eq!(shapes.len(), 2);
        match &shapes[0] {
            ShapeContent::Title(text) => assert_eq!(text, "Main Title"),
            other => panic!("expected Title, got {:?}", other),
        }
        match &shapes[1] {
            ShapeContent::Body(text) => assert_eq!(text, "The subtitle"),
            other => panic!("expected Body, got {:?}", other),
        }
    }

    #[test]
    fn test_pptx_table_basic() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Data"),
            body_texts: vec![],
            notes: None,
            table: Some(TestTable {
                headers: vec!["Name", "Value"],
                rows: vec![vec!["Alpha", "100"], vec!["Beta", "200"]],
            }),
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| Name | Value |"));
        assert!(result.markdown.contains("|---|---|"));
        assert!(result.markdown.contains("| Alpha | 100 |"));
        assert!(result.markdown.contains("| Beta | 200 |"));
    }

    #[test]
    fn test_pptx_table_empty_cells() {
        let data = build_test_pptx(&[TestSlide {
            title: None,
            body_texts: vec![],
            notes: None,
            table: Some(TestTable {
                headers: vec!["A", "B", "C"],
                rows: vec![vec!["1", "", "3"]],
            }),
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| A | B | C |"));
        assert!(result.markdown.contains("| 1 |  | 3 |"));
    }

    #[test]
    fn test_pptx_notes_basic() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Slide"),
            body_texts: vec!["Content."],
            notes: Some("This is a speaker note."),
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("> Note: This is a speaker note."));
    }

    #[test]
    fn test_pptx_notes_multiline() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Slide"),
            body_texts: vec![],
            notes: Some("First line\nSecond line\nThird line"),
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("> Note: First line"));
        assert!(result.markdown.contains("> Second line"));
        assert!(result.markdown.contains("> Third line"));
    }

    #[test]
    fn test_pptx_notes_missing() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Slide"),
            body_texts: vec!["Text."],
            notes: None,
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(!result.markdown.contains("> Note:"));
    }

    #[test]
    fn test_pptx_unicode_cjk() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("다국어"),
            body_texts: vec!["한국어 테스트", "中文测试", "日本語テスト"],
            notes: None,
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어 테스트"));
        assert!(result.markdown.contains("中文测试"));
        assert!(result.markdown.contains("日本語テスト"));
        assert!(result.markdown.contains("다국어"));
    }

    #[test]
    fn test_pptx_emoji() {
        let data = build_test_pptx(&[TestSlide {
            title: Some("Emoji Test"),
            body_texts: vec!["Rocket: 🚀 Stars: ✨ Earth: 🌍"],
            notes: None,
            table: None,
            images: vec![],
        }]);
        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨"));
        assert!(result.markdown.contains("🌍"));
    }

    #[test]
    fn test_pptx_missing_presentation_xml() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // Just a content types file, no presentation.xml
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"></Types>").unwrap();

        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let converter = PptxConverter;
        let result = converter.convert(&data, &ConversionOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("missing ppt/presentation.xml"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_pptx_missing_slide_file_graceful() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"></Types>").unwrap();

        // presentation.xml references a slide that doesn't exist
        let pres_xml = r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#;
        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(pres_xml.as_bytes()).unwrap();

        let pres_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#;
        zip.start_file("ppt/_rels/presentation.xml.rels", opts)
            .unwrap();
        zip.write_all(pres_rels.as_bytes()).unwrap();

        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let converter = PptxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(!result.warnings.is_empty());
        assert_eq!(result.warnings[0].code, WarningCode::SkippedElement);
        assert!(result.warnings[0].message.contains("slide file not found"));
    }

    #[test]
    fn test_pptx_derive_rels_path() {
        assert_eq!(
            derive_rels_path("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
        assert_eq!(
            derive_rels_path("ppt/presentation.xml"),
            "ppt/_rels/presentation.xml.rels"
        );
        assert_eq!(derive_rels_path("file.xml"), "_rels/file.xml.rels");
    }

    #[test]
    fn test_pptx_resolve_relative_path() {
        assert_eq!(
            resolve_relative_path("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
        assert_eq!(
            resolve_relative_path("ppt/slides/slide1.xml", "../notesSlides/notesSlide1.xml"),
            "ppt/notesSlides/notesSlide1.xml"
        );
        assert_eq!(
            resolve_relative_path("ppt/slides/slide1.xml", "chart1.xml"),
            "ppt/slides/chart1.xml"
        );
    }

    #[test]
    fn test_pptx_image_reference_detected() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:pic><p:nvPicPr><p:cNvPr id="1" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        assert_eq!(shapes.len(), 1);
        match &shapes[0] {
            ShapeContent::Image { rel_id } => assert_eq!(rel_id, "rId2"),
            other => panic!("expected Image, got {:?}", other),
        }
    }

    #[test]
    fn test_pptx_line_break() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Line one</a:t><a:br/><a:t>Line two</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

        let (shapes, _) = parse_slide(slide_xml);
        match &shapes[0] {
            ShapeContent::Body(text) => assert!(text.contains("Line one\nLine two")),
            other => panic!("expected Body, got {:?}", other),
        }
    }
}
