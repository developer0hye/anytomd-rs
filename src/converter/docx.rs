use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::format_heading;

pub struct DocxConverter;

// ---- Data types ----

/// The kind of block element a paragraph represents.
#[derive(Debug, Clone, PartialEq)]
enum ParagraphKind {
    Normal,
    Heading(u8), // level 1..=6
}

/// A resolved relationship entry from document.xml.rels.
#[derive(Debug, Clone)]
struct Relationship {
    target: String,
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

// ---- Styles parsing ----

/// Parse styles.xml to extract a mapping from style ID to heading level.
///
/// Recognizes two patterns:
/// 1. Direct match: styleId is "Heading1" through "Heading9" (case-insensitive digit extraction)
/// 2. Name-based: `<w:name w:val="heading N">` inside a style element (case-insensitive)
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
                    // Extract w:styleId attribute
                    current_style_id = None;
                    current_heading_level = None;
                    for attr in e.attributes().flatten() {
                        let local_name = attr.key.local_name();
                        let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                        if attr_local == "styleId" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            // Check direct heading pattern: "Heading1" through "Heading9"
                            if let Some(level) = extract_heading_level_from_id(&val) {
                                current_heading_level = Some(level);
                            }
                            current_style_id = Some(val);
                        }
                    }
                } else if local_str == "name" {
                    // <w:name w:val="heading N"> inside a style
                    if current_style_id.is_some() {
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

                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key {
                            "Id" => id = Some(val),
                            "Target" => target = Some(val),
                            _ => {}
                        }
                    }

                    if let (Some(id), Some(target)) = (id, target) {
                        rels.insert(id, Relationship { target });
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

// ---- Document body parsing ----

/// Parse the main document.xml body and produce Markdown output.
///
/// Returns (markdown, title, warnings).
fn parse_document(
    xml: &str,
    styles: &HashMap<String, u8>,
    relationships: &HashMap<String, Relationship>,
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
    // Hyperlink state
    let mut in_hyperlink = false;
    let mut current_hyperlink_url: Option<String> = None;
    let mut hyperlink_text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                match local_str {
                    "body" => {
                        in_body = true;
                    }
                    "p" if in_body => {
                        in_paragraph = true;
                        current_para_kind = ParagraphKind::Normal;
                        current_para_text.clear();
                    }
                    "pStyle" if in_paragraph => {
                        // <w:pStyle w:val="Heading1"/>
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_para_kind = resolve_paragraph_kind(&val, styles);
                            }
                        }
                    }
                    "hyperlink" if in_paragraph => {
                        in_hyperlink = true;
                        hyperlink_text.clear();
                        current_hyperlink_url = None;

                        // Look up r:id in relationships
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            // The attribute may be r:id or just id depending on namespace
                            if key == "r:id" || key.ends_with(":id") {
                                let rid = String::from_utf8_lossy(&attr.value).to_string();
                                current_hyperlink_url =
                                    resolve_hyperlink_url(&rid, relationships, &mut warnings);
                            }
                        }
                    }
                    "r" if in_paragraph => {
                        in_run = true;
                    }
                    "t" if in_run => {
                        in_text = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                match local_str {
                    "pStyle" if in_paragraph => {
                        for attr in e.attributes().flatten() {
                            let local_name = attr.key.local_name();
                            let attr_local = std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                            if attr_local == "val" {
                                let val = String::from_utf8_lossy(&attr.value);
                                current_para_kind = resolve_paragraph_kind(&val, styles);
                            }
                        }
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
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text && in_run {
                    let text = e.unescape().unwrap_or_default().to_string();
                    if in_hyperlink {
                        hyperlink_text.push_str(&text);
                    } else {
                        current_para_text.push_str(&text);
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
                    "p" if in_paragraph => {
                        // Finalize paragraph
                        finalize_paragraph(
                            &current_para_kind,
                            &current_para_text,
                            &mut output,
                            &mut title,
                        );
                        in_paragraph = false;
                        current_para_text.clear();
                    }
                    "hyperlink" if in_hyperlink => {
                        // Emit [text](url) or plain text
                        if let Some(ref url) = current_hyperlink_url {
                            current_para_text.push_str(&format!("[{}]({})", hyperlink_text, url));
                        } else {
                            current_para_text.push_str(&hyperlink_text);
                        }
                        in_hyperlink = false;
                        hyperlink_text.clear();
                        current_hyperlink_url = None;
                    }
                    "r" => {
                        in_run = false;
                        in_text = false;
                    }
                    "t" => {
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

    // Trim trailing newlines to a single trailing newline
    let markdown = output.trim_end().to_string();
    let markdown = if markdown.is_empty() {
        markdown
    } else {
        format!("{}\n", markdown)
    };

    (markdown, title, warnings)
}

/// Resolve paragraph kind from a style value, checking direct heading patterns first,
/// then falling back to the styles map.
fn resolve_paragraph_kind(style_val: &str, styles: &HashMap<String, u8>) -> ParagraphKind {
    // Phase 1: direct match on style value
    if let Some(level) = extract_heading_level_from_id(style_val) {
        let clamped = level.clamp(1, 6);
        return ParagraphKind::Heading(clamped);
    }
    // Phase 2: lookup in styles map
    if let Some(&level) = styles.get(style_val) {
        let clamped = level.clamp(1, 6);
        return ParagraphKind::Heading(clamped);
    }
    ParagraphKind::Normal
}

/// Resolve a hyperlink URL from a relationship ID.
/// Returns None and appends a warning if the relationship is not found.
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

/// Finalize a paragraph: emit heading or plain text into the output buffer.
fn finalize_paragraph(
    kind: &ParagraphKind,
    text: &str,
    output: &mut String,
    title: &mut Option<String>,
) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    match kind {
        ParagraphKind::Heading(level) => {
            output.push_str(&format_heading(*level, trimmed));
            output.push('\n');
            // Set title from first H1
            if *level == 1 && title.is_none() {
                *title = Some(trimmed.to_string());
            }
        }
        ParagraphKind::Normal => {
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
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor)?;

        // 1. Parse styles.xml (optional — graceful if missing)
        let styles = match read_zip_text(&mut archive, "word/styles.xml")? {
            Some(xml) => parse_styles(&xml),
            None => HashMap::new(),
        };

        // 2. Parse document.xml.rels (optional — hyperlinks degrade to plain text)
        let relationships = match read_zip_text(&mut archive, "word/_rels/document.xml.rels")? {
            Some(xml) => parse_relationships(&xml),
            None => HashMap::new(),
        };

        // 3. Parse document.xml (required)
        let document_xml = read_zip_text(&mut archive, "word/document.xml")?.ok_or_else(|| {
            ConvertError::MalformedDocument {
                reason: "missing word/document.xml".to_string(),
            }
        })?;

        let (markdown, title, warnings) = parse_document(&document_xml, &styles, &relationships);

        Ok(ConversionResult {
            markdown,
            title,
            warnings,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Helper: build minimal DOCX ZIP in memory ----

    /// Build a minimal DOCX file in memory from document XML, optional styles XML,
    /// and optional relationships XML.
    fn build_test_docx(
        document_xml: &str,
        styles_xml: Option<&str>,
        rels_xml: Option<&str>,
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

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    /// Wrap paragraph content in a minimal document.xml structure.
    fn wrap_body(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}</w:body></w:document>"#
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

    // ---- Tests ----

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
        // They should be separate paragraphs (double newline)
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
        // Use a custom style ID that maps to heading via styles.xml name
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
        // Headings with direct "HeadingN" pattern still work without styles.xml
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
        // Link text is preserved as plain text
        assert!(result.markdown.contains("Broken Link"));
        // Should not contain Markdown link syntax
        assert!(!result.markdown.contains('['));
        // Warning should be emitted
        assert!(!result.warnings.is_empty());
        assert_eq!(result.warnings[0].code, WarningCode::SkippedElement);
    }

    #[test]
    fn test_docx_line_break() {
        let body = r#"<w:p><w:r><w:t>Line one</w:t><w:br/><w:t>Line two</w:t></w:r></w:p>"#;
        let doc = wrap_body(&body);
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
        let doc = wrap_body(&body);
        let data = build_test_docx(&doc, None, None);
        let converter = DocxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Hello world"));
    }
}
