use std::collections::HashMap;
use std::io::Cursor;

use calamine::{open_workbook_auto_from_rs, Data, Reader};
use chrono::{Datelike, Timelike};
use quick_xml::events::Event;
use zip::ZipArchive;

use crate::converter::ooxml_utils::{
    derive_rels_path, parse_relationships, resolve_image_placeholders, resolve_relative_path,
    ImageInfo, PendingImageResolution,
};
use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::{build_table, format_heading};
use crate::zip_utils::{read_zip_bytes, read_zip_text};

pub struct XlsxConverter;

/// Extract embedded images for a given sheet by following the OOXML relationship chain:
///
/// 1. `xl/worksheets/_rels/sheet{N}.xml.rels` → find drawing targets
/// 2. `xl/drawings/drawing{N}.xml` → find `<a:blip r:embed="rId..."/>` elements
/// 3. `xl/drawings/_rels/drawing{N}.xml.rels` → resolve image file paths
/// 4. `xl/media/image{N}.png` → read image bytes
///
/// Returns `(filename, bytes)` pairs for each image found.
fn extract_sheet_images(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    sheet_index: usize,
) -> Vec<(String, Vec<u8>)> {
    let mut images = Vec::new();

    // Step 1: Read sheet rels to find drawing references
    let sheet_rels_path = format!("xl/worksheets/_rels/sheet{}.xml.rels", sheet_index + 1);
    let sheet_rels_xml = match read_zip_text(archive, &sheet_rels_path) {
        Ok(Some(xml)) => xml,
        _ => return images,
    };

    let sheet_rels = parse_relationships(&sheet_rels_xml);

    // Find drawing targets (relationship target contains "drawing")
    let drawing_targets: Vec<String> = sheet_rels
        .values()
        .filter(|r| r.target.contains("drawing"))
        .map(|r| r.target.clone())
        .collect();

    for drawing_target in &drawing_targets {
        // Resolve the drawing path relative to xl/worksheets/
        // Handles both relative ("../drawings/drawing1.xml") and
        // absolute ("/xl/drawings/drawing1.xml") target paths.
        let drawing_path = if let Some(stripped) = drawing_target.strip_prefix('/') {
            stripped.to_string()
        } else if let Some(stripped) = drawing_target.strip_prefix("../") {
            format!("xl/{stripped}")
        } else {
            format!("xl/worksheets/{drawing_target}")
        };

        // Step 2: Read drawing XML and find blip references
        let drawing_xml = match read_zip_text(archive, &drawing_path) {
            Ok(Some(xml)) => xml,
            _ => continue,
        };

        let blip_rel_ids = parse_drawing_blips(&drawing_xml);

        if blip_rel_ids.is_empty() {
            continue;
        }

        // Step 3: Read drawing rels to resolve image paths
        let drawing_rels_path = derive_rels_path(&drawing_path);
        let drawing_rels_xml = match read_zip_text(archive, &drawing_rels_path) {
            Ok(Some(xml)) => xml,
            _ => continue,
        };
        let drawing_rels = parse_relationships(&drawing_rels_xml);

        // Step 4: Read image bytes for each blip
        for rel_id in &blip_rel_ids {
            if let Some(rel) = drawing_rels.get(rel_id) {
                let image_target = &rel.target;
                let image_path = if image_target.starts_with("../") {
                    // Resolve relative to drawing path's directory
                    let drawing_dir = drawing_path
                        .rfind('/')
                        .map(|pos| &drawing_path[..pos])
                        .unwrap_or("");
                    resolve_relative_path(drawing_dir, image_target)
                } else if let Some(stripped) = image_target.strip_prefix('/') {
                    stripped.to_string()
                } else {
                    let drawing_dir = drawing_path
                        .rfind('/')
                        .map(|pos| &drawing_path[..pos])
                        .unwrap_or("");
                    format!("{drawing_dir}/{image_target}")
                };

                let filename = image_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&image_path)
                    .to_string();

                if let Ok(Some(img_data)) = read_zip_bytes(archive, &image_path) {
                    images.push((filename, img_data));
                }
            }
        }
    }

    images
}

/// Parse a drawing XML to extract blip relationship IDs from anchor elements.
fn parse_drawing_blips(xml: &str) -> Vec<String> {
    let mut rel_ids = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "blip" {
                    for attr in e.attributes().flatten() {
                        let key_local = attr.key.local_name();
                        let key_str = std::str::from_utf8(key_local.as_ref()).unwrap_or("");
                        if key_str == "embed" {
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

    rel_ids
}

/// Convert a 0-based column index to an Excel-style column letter (A, B, ..., Z, AA, ...).
fn col_letter(col: usize) -> String {
    let mut result = String::new();
    let mut n = col;
    loop {
        result.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    result
}

/// Format a calamine cell value as a string for Markdown output.
///
/// Whole-number floats display as integers (e.g. `3.0` → `"3"`).
/// Booleans display as `TRUE` / `FALSE`.
/// Empty cells produce an empty string.
/// Error cells display the error text (e.g. `#DIV/0!`) and emit a warning.
/// DateTime cells are formatted as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS`.
///
/// Note: calamine returns computed values for formula cells, not the formula text.
/// This means formulas like `=SUM(A1:A3)` appear as their computed numeric result.
fn format_cell(cell: &Data, location: &str, warnings: &mut Vec<ConversionWarning>) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            if f.is_finite() && f.fract() == 0.0 {
                format!("{:.0}", f)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Data::DateTime(dt) => {
            if let Some(ndt) = dt.as_datetime() {
                let (h, m, s) = (ndt.hour(), ndt.minute(), ndt.second());
                if h == 0 && m == 0 && s == 0 {
                    // Date-only: no time component
                    format!("{:04}-{:02}-{:02}", ndt.year(), ndt.month(), ndt.day())
                } else {
                    format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                        ndt.year(),
                        ndt.month(),
                        ndt.day(),
                        h,
                        m,
                        s
                    )
                }
            } else {
                // Fallback: use Display impl
                format!("{dt}")
            }
        }
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => {
            let error_text = format!("{e}");
            warnings.push(ConversionWarning {
                code: WarningCode::MalformedSegment,
                message: format!("cell contains error: {error_text}"),
                location: Some(location.to_string()),
            });
            error_text
        }
    }
}

// ---- Internal conversion (parse + image extraction, no resolution) ----

impl XlsxConverter {
    /// Parse the workbook and extract images without resolving placeholders.
    ///
    /// Returns the conversion result (with unresolved placeholders in markdown)
    /// and pending image data for later resolution (sync or async).
    pub(crate) fn convert_inner(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<(ConversionResult, PendingImageResolution), ConvertError> {
        // Pre-scan ZIP budget before passing to calamine
        if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(data)) {
            crate::zip_utils::validate_zip_budget(
                &mut archive,
                options.max_uncompressed_zip_bytes,
            )?;
        }

        let cursor = Cursor::new(data);
        let mut workbook = open_workbook_auto_from_rs(cursor)?;

        let sheet_names = workbook.sheet_names().to_owned();
        let mut sections = Vec::new();
        let mut warnings = Vec::new();

        // Track which sheet index each section corresponds to (for image attachment)
        let mut section_sheet_indices: Vec<usize> = Vec::new();

        for (sheet_idx, name) in sheet_names.iter().enumerate() {
            let range = match workbook.worksheet_range(name) {
                Ok(r) => r,
                Err(e) => {
                    warnings.push(ConversionWarning {
                        code: WarningCode::SkippedElement,
                        message: format!("failed to read sheet '{name}': {e}"),
                        location: Some(name.clone()),
                    });
                    continue;
                }
            };

            if range.is_empty() {
                continue;
            }

            let mut rows_iter = range.rows();
            let header_row = match rows_iter.next() {
                Some(row) => row,
                None => continue,
            };

            let headers: Vec<String> = header_row
                .iter()
                .enumerate()
                .map(|(ci, cell)| {
                    let loc = format!("{}!{}1", name, col_letter(ci));
                    format_cell(cell, &loc, &mut warnings)
                })
                .collect();
            let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();

            let mut data_rows: Vec<Vec<String>> = Vec::new();
            for (ri, row) in rows_iter.enumerate() {
                let cells: Vec<String> = row
                    .iter()
                    .enumerate()
                    .map(|(ci, cell)| {
                        let loc = format!("{}!{}{}", name, col_letter(ci), ri + 2);
                        format_cell(cell, &loc, &mut warnings)
                    })
                    .collect();
                data_rows.push(cells);
            }

            let row_refs: Vec<Vec<&str>> = data_rows
                .iter()
                .map(|row| row.iter().map(|s| s.as_str()).collect())
                .collect();

            let heading = format_heading(2, name);
            let table = build_table(&header_refs, &row_refs);
            sections.push(format!("{heading}{table}"));
            section_sheet_indices.push(sheet_idx);
        }

        // Extract embedded images if requested or if describer needs them
        let need_image_bytes = options.extract_images || options.image_describer.is_some();
        let mut images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut image_bytes_map: HashMap<String, Vec<u8>> = HashMap::new();
        let mut image_infos: Vec<ImageInfo> = Vec::new();
        let mut image_counter: usize = 0;

        if need_image_bytes {
            // Open a fresh ZipArchive (calamine consumed the original cursor)
            let mut archive = ZipArchive::new(Cursor::new(data))?;
            let mut total_image_bytes: usize = 0;

            for (section_idx, &sheet_idx) in section_sheet_indices.iter().enumerate() {
                let sheet_images = extract_sheet_images(&mut archive, sheet_idx);

                let mut image_lines = Vec::new();
                for (filename, img_data) in sheet_images {
                    total_image_bytes += img_data.len();
                    if total_image_bytes <= options.max_total_image_bytes {
                        let placeholder = format!("__img_{n}__", n = image_counter);
                        image_counter += 1;
                        image_infos.push(ImageInfo {
                            placeholder: placeholder.clone(),
                            original_alt: String::new(),
                            filename: filename.clone(),
                        });
                        image_lines.push(format!("![{placeholder}]({filename})"));
                        if options.extract_images {
                            images.push((filename.clone(), img_data.clone()));
                        }
                        image_bytes_map.insert(filename, img_data);
                    } else {
                        warnings.push(ConversionWarning {
                            code: WarningCode::ResourceLimitReached,
                            message: format!(
                                "total image bytes exceeded limit ({})",
                                options.max_total_image_bytes
                            ),
                            location: Some(filename),
                        });
                    }
                }

                if !image_lines.is_empty() {
                    sections[section_idx].push_str(&format!("\n{}", image_lines.join("\n")));
                }
            }
        }

        let markdown = sections.join("\n");

        let result = ConversionResult {
            markdown,
            images,
            warnings,
            ..Default::default()
        };

        let pending = PendingImageResolution {
            infos: image_infos,
            bytes: image_bytes_map,
        };

        Ok((result, pending))
    }
}

// ---- Converter trait impl ----

impl Converter for XlsxConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["xlsx", "xls"]
    }

    fn convert(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let (mut result, pending) = self.convert_inner(data, options)?;
        resolve_image_placeholders(
            &mut result.markdown,
            &pending.infos,
            &pending.bytes,
            options.image_describer.as_deref(),
            &mut result.warnings,
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper: build minimal XLSX bytes from sheet definitions --

    /// Cell value for test XLSX generation.
    enum TestCell {
        Str(&'static str),
        Num(f64),
        Bool(bool),
        Empty,
    }

    /// Convert a 0-based column index to an Excel column letter (A-Z) for test XML.
    fn test_col_letter(col: usize) -> char {
        (b'A' + col as u8) as char
    }

    /// Build a minimal XLSX file in memory from the given sheet definitions.
    fn build_test_xlsx(sheets: &[(&str, &[&[TestCell]])]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // [Content_Types].xml
        let mut ct = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
        );
        for (i, _) in sheets.iter().enumerate() {
            ct.push_str(&format!(
                "<Override PartName=\"/xl/worksheets/sheet{}.xml\" \
                 ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
                i + 1
            ));
        }
        ct.push_str("</Types>");
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
              <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
              <Relationship Id=\"rId1\" \
              Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
              Target=\"xl/workbook.xml\"/>\
              </Relationships>",
        )
        .unwrap();

        // xl/workbook.xml
        let mut wb = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
             <sheets>",
        );
        for (i, (name, _)) in sheets.iter().enumerate() {
            wb.push_str(&format!(
                "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                i + 1,
                i + 1
            ));
        }
        wb.push_str("</sheets></workbook>");
        zip.start_file("xl/workbook.xml", opts).unwrap();
        zip.write_all(wb.as_bytes()).unwrap();

        // xl/_rels/workbook.xml.rels
        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (i, _) in sheets.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" \
                 Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
                 Target=\"worksheets/sheet{}.xml\"/>",
                i + 1,
                i + 1
            ));
        }
        rels.push_str("</Relationships>");
        zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();

        // Each worksheet
        for (i, (_, rows)) in sheets.iter().enumerate() {
            let mut ws = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
                 <sheetData>",
            );
            for (ri, row) in rows.iter().enumerate() {
                ws.push_str(&format!("<row r=\"{}\">", ri + 1));
                for (ci, cell) in row.iter().enumerate() {
                    let col = test_col_letter(ci);
                    let r = ri + 1;
                    match cell {
                        TestCell::Str(s) => {
                            let escaped = s
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                                .replace('"', "&quot;");
                            ws.push_str(&format!(
                                "<c r=\"{col}{r}\" t=\"inlineStr\"><is><t>{escaped}</t></is></c>"
                            ));
                        }
                        TestCell::Num(f) => {
                            ws.push_str(&format!("<c r=\"{col}{r}\"><v>{f}</v></c>"));
                        }
                        TestCell::Bool(b) => {
                            let v = if *b { 1 } else { 0 };
                            ws.push_str(&format!("<c r=\"{col}{r}\" t=\"b\"><v>{v}</v></c>"));
                        }
                        TestCell::Empty => {}
                    }
                }
                ws.push_str("</row>");
            }
            ws.push_str("</sheetData></worksheet>");
            zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), opts)
                .unwrap();
            zip.write_all(ws.as_bytes()).unwrap();
        }

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    // -- Tests --

    #[test]
    fn test_xlsx_supported_extensions() {
        let converter = XlsxConverter;
        assert_eq!(converter.supported_extensions(), &["xlsx", "xls"]);
    }

    #[test]
    fn test_xlsx_can_convert() {
        let converter = XlsxConverter;
        assert!(converter.can_convert("xlsx", &[]));
        assert!(!converter.can_convert("csv", &[]));
        assert!(!converter.can_convert("json", &[]));
    }

    #[test]
    fn test_xls_supported_extension() {
        let converter = XlsxConverter;
        assert!(converter.can_convert("xls", &[]));
    }

    #[test]
    fn test_xls_not_confused_with_other_formats() {
        let converter = XlsxConverter;
        assert!(!converter.can_convert("csv", &[]));
        assert!(!converter.can_convert("json", &[]));
        assert!(!converter.can_convert("docx", &[]));
        assert!(!converter.can_convert("pptx", &[]));
    }

    #[test]
    fn test_xlsx_empty_workbook() {
        let data = build_test_xlsx(&[("Sheet1", &[])]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "");
    }

    #[test]
    fn test_xlsx_single_sheet_basic() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "Sheet1",
            &[
                &[Str("Name"), Str("Age")][..],
                &[Str("Alice"), Num(30.0)],
                &[Str("Bob"), Num(25.0)],
            ],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## Sheet1"));
        assert!(result.markdown.contains("| Name | Age |"));
        assert!(result.markdown.contains("| Alice | 30 |"));
        assert!(result.markdown.contains("| Bob | 25 |"));
    }

    #[test]
    fn test_xlsx_multiple_sheets() {
        use TestCell::*;
        let data = build_test_xlsx(&[
            ("First", &[&[Str("A")][..], &[Str("1")]]),
            ("Second", &[&[Str("B")][..], &[Str("2")]]),
        ]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## First"));
        assert!(result.markdown.contains("| A |"));
        assert!(result.markdown.contains("| 1 |"));
        assert!(result.markdown.contains("## Second"));
        assert!(result.markdown.contains("| B |"));
        assert!(result.markdown.contains("| 2 |"));
    }

    #[test]
    fn test_xlsx_empty_sheet_skipped() {
        use TestCell::*;
        let data = build_test_xlsx(&[("HasData", &[&[Str("X")][..], &[Str("1")]]), ("Empty", &[])]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## HasData"));
        assert!(!result.markdown.contains("## Empty"));
    }

    #[test]
    fn test_xlsx_header_only_sheet() {
        use TestCell::*;
        let data = build_test_xlsx(&[("Sheet1", &[&[Str("Col1"), Str("Col2")][..]])]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("## Sheet1"));
        assert!(result.markdown.contains("| Col1 | Col2 |"));
        assert!(result.markdown.contains("|---|---|"));
        // Verify no data rows after separator
        let sep_pos = result.markdown.find("|---|---|").unwrap();
        let after_sep = &result.markdown[sep_pos + "|---|---|".len()..];
        assert!(
            !after_sep.trim().contains('|'),
            "expected no data rows after separator"
        );
    }

    #[test]
    fn test_xlsx_numeric_cells() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "Numbers",
            &[
                &[Str("Int"), Str("Float"), Str("Whole")][..],
                &[Num(42.0), Num(3.14), Num(100.0)],
            ],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| 42 |"));
        assert!(result.markdown.contains("3.14"));
        assert!(result.markdown.contains("| 100 |"));
    }

    #[test]
    fn test_xlsx_bool_cells() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "Bools",
            &[&[Str("Value")][..], &[Bool(true)], &[Bool(false)]],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| TRUE |"));
        assert!(result.markdown.contains("| FALSE |"));
    }

    #[test]
    fn test_xlsx_unicode_cjk() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "CJK",
            &[
                &[Str("한국어"), Str("中文"), Str("日本語")][..],
                &[Str("서울"), Str("北京"), Str("東京")],
            ],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("中文"));
        assert!(result.markdown.contains("日本語"));
        assert!(result.markdown.contains("서울"));
        assert!(result.markdown.contains("北京"));
        assert!(result.markdown.contains("東京"));
    }

    #[test]
    fn test_xlsx_emoji() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "Emoji",
            &[&[Str("Icon")][..], &[Str("🚀")], &[Str("✨")], &[Str("🌍")]],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨"));
        assert!(result.markdown.contains("🌍"));
    }

    #[test]
    fn test_xlsx_mixed_types() {
        use TestCell::*;
        let data = build_test_xlsx(&[(
            "Mixed",
            &[
                &[Str("Str"), Str("Num"), Str("Bool"), Str("Empty")][..],
                &[Str("hello"), Num(42.0), Bool(true), Empty],
            ],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| hello | 42 | TRUE |  |"));
    }

    #[test]
    fn test_xlsx_uneven_row_lengths() {
        use TestCell::*;
        // Row 1 has 3 cols (header), row 2 has 2 cols, row 3 has 4 cols
        // calamine pads shorter rows with Empty, truncates longer rows to header width
        let data = build_test_xlsx(&[(
            "Sheet1",
            &[
                &[Str("A"), Str("B"), Str("C")][..],
                &[Str("1"), Str("2")],
                &[Str("x"), Str("y"), Str("z")],
            ],
        )]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Header row should be intact
        assert!(result.markdown.contains("| A | B | C |"));
        // Short row: calamine returns fewer cells, build_table pads
        assert!(result.markdown.contains("1"));
        assert!(result.markdown.contains("2"));
        // Full row should be intact
        assert!(result.markdown.contains("| x | y | z |"));
    }

    #[test]
    fn test_xlsx_zip_budget_exceeded_returns_error() {
        use TestCell::*;
        let data = build_test_xlsx(&[("Sheet1", &[&[Str("A")][..], &[Str("1")]])]);
        let converter = XlsxConverter;
        let options = ConversionOptions {
            max_uncompressed_zip_bytes: 1, // impossibly small
            ..Default::default()
        };
        let result = converter.convert(&data, &options);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("exceeds limit"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_xlsx_no_title_images() {
        use TestCell::*;
        let data = build_test_xlsx(&[("Sheet1", &[&[Str("A")][..], &[Str("1")]])]);
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.title.is_none());
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_xlsx_invalid_data_returns_error() {
        let converter = XlsxConverter;
        let result = converter.convert(b"not a valid xlsx file", &ConversionOptions::default());
        assert!(result.is_err());
    }

    // -- col_letter tests --

    #[test]
    fn test_col_letter_single() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(1), "B");
        assert_eq!(col_letter(25), "Z");
    }

    #[test]
    fn test_col_letter_multi() {
        assert_eq!(col_letter(26), "AA");
        assert_eq!(col_letter(27), "AB");
        assert_eq!(col_letter(51), "AZ");
        assert_eq!(col_letter(52), "BA");
        assert_eq!(col_letter(701), "ZZ");
        assert_eq!(col_letter(702), "AAA");
    }

    // -- Error cell tests --

    #[test]
    fn test_xlsx_format_cell_error_displays_text() {
        let mut warnings = Vec::new();
        let cell = Data::Error(calamine::CellErrorType::Div0);
        let result = format_cell(&cell, "Sheet1!A1", &mut warnings);
        assert!(
            result.contains("DIV"),
            "expected error text containing 'DIV', got: {result}"
        );
    }

    #[test]
    fn test_xlsx_format_cell_error_na() {
        let mut warnings = Vec::new();
        let cell = Data::Error(calamine::CellErrorType::NA);
        let result = format_cell(&cell, "Sheet1!B2", &mut warnings);
        assert!(
            result.contains("N/A"),
            "expected error text containing 'N/A', got: {result}"
        );
    }

    #[test]
    fn test_xlsx_format_cell_error_emits_warning() {
        let mut warnings = Vec::new();
        let cell = Data::Error(calamine::CellErrorType::Div0);
        format_cell(&cell, "Sheet1!C3", &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, WarningCode::MalformedSegment);
        assert_eq!(warnings[0].location.as_deref(), Some("Sheet1!C3"));
        assert!(warnings[0].message.contains("error"));
    }

    // -- DateTime formatting tests --

    #[test]
    fn test_xlsx_format_cell_datetime_date_only() {
        use calamine::ExcelDateTimeType;
        let mut warnings = Vec::new();
        // Excel serial date for 2024-01-15 = 45306
        let dt = Data::DateTime(calamine::ExcelDateTime::new(
            45306.0,
            ExcelDateTimeType::DateTime,
            false,
        ));
        let result = format_cell(&dt, "Sheet1!A1", &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(result, "2024-01-15");
    }

    #[test]
    fn test_xlsx_format_cell_datetime_full() {
        use calamine::ExcelDateTimeType;
        let mut warnings = Vec::new();
        // 45306.5 = 2024-01-15 12:00:00
        let dt = Data::DateTime(calamine::ExcelDateTime::new(
            45306.5,
            ExcelDateTimeType::DateTime,
            false,
        ));
        let result = format_cell(&dt, "Sheet1!A1", &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(result, "2024-01-15 12:00:00");
    }

    #[test]
    fn test_xlsx_format_cell_datetime_with_time() {
        use calamine::ExcelDateTimeType;
        let mut warnings = Vec::new();
        // 45306.0 + 14h30m15s = 45306 + (14*3600+30*60+15)/86400
        let fractional = (14.0 * 3600.0 + 30.0 * 60.0 + 15.0) / 86400.0;
        let dt = Data::DateTime(calamine::ExcelDateTime::new(
            45306.0 + fractional,
            ExcelDateTimeType::DateTime,
            false,
        ));
        let result = format_cell(&dt, "Sheet1!A1", &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(result, "2024-01-15 14:30:15");
    }

    #[test]
    fn test_xlsx_format_cell_datetime_time_only() {
        use calamine::ExcelDateTimeType;
        let mut warnings = Vec::new();
        // Time-only: fractional day < 1.0, e.g. 0.5 = 12:00:00
        let dt = Data::DateTime(calamine::ExcelDateTime::new(
            0.5,
            ExcelDateTimeType::TimeDelta,
            false,
        ));
        let result = format_cell(&dt, "Sheet1!A1", &mut warnings);
        assert!(warnings.is_empty());
        // Could be either "1899-12-30 12:00:00" or "12:00:00" depending on calamine behavior
        assert!(
            result.contains("12:00:00"),
            "expected time 12:00:00 in output, got: {result}"
        );
    }

    // -- Image extraction tests --

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

    /// Build a minimal XLSX file with an embedded image in sheet 1.
    ///
    /// The ZIP contains:
    /// - Standard XLSX structure (workbook, sheet with data)
    /// - `xl/worksheets/_rels/sheet1.xml.rels` → points to `../drawings/drawing1.xml`
    /// - `xl/drawings/drawing1.xml` → contains `<a:blip r:embed="rId1"/>`
    /// - `xl/drawings/_rels/drawing1.xml.rels` → points to `../media/image1.png`
    /// - `xl/media/image1.png` → fake image bytes
    fn build_test_xlsx_with_image(
        sheets: &[(&str, &[&[TestCell]])],
        image_filename: &str,
        image_data: &[u8],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // [Content_Types].xml — add image content type
        let mut ct = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
        );
        for (i, _) in sheets.iter().enumerate() {
            ct.push_str(&format!(
                "<Override PartName=\"/xl/worksheets/sheet{}.xml\" \
                 ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
                i + 1
            ));
        }
        ct.push_str("</Types>");
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
              <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
              <Relationship Id=\"rId1\" \
              Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
              Target=\"xl/workbook.xml\"/>\
              </Relationships>",
        )
        .unwrap();

        // xl/workbook.xml
        let mut wb = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
             <sheets>",
        );
        for (i, (name, _)) in sheets.iter().enumerate() {
            wb.push_str(&format!(
                "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                i + 1,
                i + 1
            ));
        }
        wb.push_str("</sheets></workbook>");
        zip.start_file("xl/workbook.xml", opts).unwrap();
        zip.write_all(wb.as_bytes()).unwrap();

        // xl/_rels/workbook.xml.rels
        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (i, _) in sheets.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" \
                 Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
                 Target=\"worksheets/sheet{}.xml\"/>",
                i + 1,
                i + 1
            ));
        }
        rels.push_str("</Relationships>");
        zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();

        // Each worksheet
        for (i, (_, rows)) in sheets.iter().enumerate() {
            let mut ws = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
                 <sheetData>",
            );
            for (ri, row) in rows.iter().enumerate() {
                ws.push_str(&format!("<row r=\"{}\">", ri + 1));
                for (ci, cell) in row.iter().enumerate() {
                    let col = test_col_letter(ci);
                    let r = ri + 1;
                    match cell {
                        TestCell::Str(s) => {
                            let escaped = s
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                                .replace('"', "&quot;");
                            ws.push_str(&format!(
                                "<c r=\"{col}{r}\" t=\"inlineStr\"><is><t>{escaped}</t></is></c>"
                            ));
                        }
                        TestCell::Num(f) => {
                            ws.push_str(&format!("<c r=\"{col}{r}\"><v>{f}</v></c>"));
                        }
                        TestCell::Bool(b) => {
                            let v = if *b { 1 } else { 0 };
                            ws.push_str(&format!("<c r=\"{col}{r}\" t=\"b\"><v>{v}</v></c>"));
                        }
                        TestCell::Empty => {}
                    }
                }
                ws.push_str("</row>");
            }
            ws.push_str("</sheetData></worksheet>");
            zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), opts)
                .unwrap();
            zip.write_all(ws.as_bytes()).unwrap();
        }

        // Sheet 1 rels — point to drawing1.xml
        zip.start_file("xl/worksheets/_rels/sheet1.xml.rels", opts)
            .unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
              <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
              <Relationship Id=\"rId1\" \
              Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" \
              Target=\"../drawings/drawing1.xml\"/>\
              </Relationships>",
        )
        .unwrap();

        // xl/drawings/drawing1.xml — contains a blip reference
        let drawing_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
             <xdr:twoCellAnchor>\
             <xdr:pic>\
             <xdr:nvPicPr><xdr:cNvPr id=\"1\" name=\"Picture 1\"/><xdr:cNvPicPr/></xdr:nvPicPr>\
             <xdr:blipFill><a:blip r:embed=\"rId1\"/></xdr:blipFill>\
             </xdr:pic>\
             </xdr:twoCellAnchor>\
             </xdr:wsDr>"
        );
        zip.start_file("xl/drawings/drawing1.xml", opts).unwrap();
        zip.write_all(drawing_xml.as_bytes()).unwrap();

        // xl/drawings/_rels/drawing1.xml.rels — resolve blip rId1 to media file
        let drawing_rels = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" \
             Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" \
             Target=\"../media/{image_filename}\"/>\
             </Relationships>"
        );
        zip.start_file("xl/drawings/_rels/drawing1.xml.rels", opts)
            .unwrap();
        zip.write_all(drawing_rels.as_bytes()).unwrap();

        // xl/media/image1.png — actual image bytes
        zip.start_file(format!("xl/media/{image_filename}"), opts)
            .unwrap();
        zip.write_all(image_data).unwrap();

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_xlsx_image_extraction_disabled_by_default() {
        use TestCell::*;
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let result = converter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // Default options: extract_images=false, image_describer=None
        assert!(result.images.is_empty());
        assert!(
            !result.markdown.contains("!["),
            "markdown should not contain image refs by default: {}",
            result.markdown
        );
    }

    #[test]
    fn test_xlsx_image_extraction_with_extract_images() {
        use TestCell::*;
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].0, "image1.png");
        assert_eq!(result.images[0].1, b"fake-png-data");
    }

    #[test]
    fn test_xlsx_image_in_markdown() {
        use TestCell::*;
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("![](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
        // Image refs should appear after the table
        assert!(result.markdown.contains("## Sheet1"));
    }

    #[test]
    fn test_xlsx_image_describer_replaces_alt_text() {
        use TestCell::*;
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "A chart showing sales data".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result
                .markdown
                .contains("![A chart showing sales data](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
        // Without extract_images, images vec should be empty
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_xlsx_image_describer_error_keeps_original() {
        use TestCell::*;
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(FailingDescriber)),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        // Original empty alt text should be preserved
        assert!(
            result.markdown.contains("![](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
        // Should have a warning about the failure
        assert!(result
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::SkippedElement
                && w.message.contains("image description failed")));
    }

    #[test]
    fn test_xlsx_image_byte_budget_enforced() {
        use TestCell::*;
        // Create an image that exceeds a small budget
        let large_image = vec![0u8; 1000];
        let data = build_test_xlsx_with_image(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            &large_image,
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            extract_images: true,
            max_total_image_bytes: 500, // budget smaller than image
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        // Image should not be extracted (exceeds budget)
        assert!(result.images.is_empty());
        // Should have a ResourceLimitReached warning
        assert!(result
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::ResourceLimitReached));
    }

    // -- Helper function unit tests --

    #[test]
    fn test_parse_relationships_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Target="../drawings/drawing1.xml"
             Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing"/>
            </Relationships>"#;
        let rels = parse_relationships(xml);
        assert_eq!(
            rels.get("rId1").map(|r| r.target.as_str()),
            Some("../drawings/drawing1.xml")
        );
    }

    #[test]
    fn test_parse_drawing_blips_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
                      xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                      xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
            <xdr:twoCellAnchor>
            <xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic>
            </xdr:twoCellAnchor>
            <xdr:oneCellAnchor>
            <xdr:pic><xdr:blipFill><a:blip r:embed="rId2"/></xdr:blipFill></xdr:pic>
            </xdr:oneCellAnchor>
            </xdr:wsDr>"#;
        let blips = parse_drawing_blips(xml);
        assert_eq!(blips, vec!["rId1", "rId2"]);
    }

    #[test]
    fn test_derive_rels_path() {
        assert_eq!(
            derive_rels_path("xl/drawings/drawing1.xml"),
            "xl/drawings/_rels/drawing1.xml.rels"
        );
        assert_eq!(
            derive_rels_path("xl/worksheets/sheet1.xml"),
            "xl/worksheets/_rels/sheet1.xml.rels"
        );
        assert_eq!(derive_rels_path("file.xml"), "_rels/file.xml.rels");
    }

    #[test]
    fn test_resolve_relative_path_parent_dir() {
        assert_eq!(
            resolve_relative_path("xl/drawings", "../media/image1.png"),
            "xl/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_same_dir() {
        assert_eq!(
            resolve_relative_path("xl/drawings", "image1.png"),
            "xl/drawings/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_empty_base() {
        assert_eq!(
            resolve_relative_path("", "media/image1.png"),
            "media/image1.png"
        );
    }

    // -- Absolute path (openpyxl-style) tests --

    /// Build a minimal XLSX with absolute paths in rels (openpyxl style).
    ///
    /// Uses `/xl/drawings/drawing1.xml` and `/xl/media/image1.png` instead of
    /// relative `../drawings/...` and `../media/...` paths.
    fn build_test_xlsx_with_image_absolute_paths(
        sheets: &[(&str, &[&[TestCell]])],
        image_filename: &str,
        image_data: &[u8],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();

        // [Content_Types].xml
        let mut ct = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
        );
        for (i, _) in sheets.iter().enumerate() {
            ct.push_str(&format!(
                "<Override PartName=\"/xl/worksheets/sheet{}.xml\" \
                 ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
                i + 1
            ));
        }
        ct.push_str("</Types>");
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(ct.as_bytes()).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
              <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
              <Relationship Id=\"rId1\" \
              Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
              Target=\"xl/workbook.xml\"/>\
              </Relationships>",
        )
        .unwrap();

        // xl/workbook.xml
        let mut wb = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
             <sheets>",
        );
        for (i, (name, _)) in sheets.iter().enumerate() {
            wb.push_str(&format!(
                "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                i + 1,
                i + 1
            ));
        }
        wb.push_str("</sheets></workbook>");
        zip.start_file("xl/workbook.xml", opts).unwrap();
        zip.write_all(wb.as_bytes()).unwrap();

        // xl/_rels/workbook.xml.rels
        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (i, _) in sheets.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" \
                 Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
                 Target=\"worksheets/sheet{}.xml\"/>",
                i + 1,
                i + 1
            ));
        }
        rels.push_str("</Relationships>");
        zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();

        // Each worksheet
        for (i, (_, rows)) in sheets.iter().enumerate() {
            let mut ws = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
                 <sheetData>",
            );
            for (ri, row) in rows.iter().enumerate() {
                ws.push_str(&format!("<row r=\"{}\">", ri + 1));
                for (ci, cell) in row.iter().enumerate() {
                    let col = test_col_letter(ci);
                    let r = ri + 1;
                    match cell {
                        TestCell::Str(s) => {
                            let escaped = s
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                                .replace('"', "&quot;");
                            ws.push_str(&format!(
                                "<c r=\"{col}{r}\" t=\"inlineStr\"><is><t>{escaped}</t></is></c>"
                            ));
                        }
                        TestCell::Num(f) => {
                            ws.push_str(&format!("<c r=\"{col}{r}\"><v>{f}</v></c>"));
                        }
                        TestCell::Bool(b) => {
                            let v = if *b { 1 } else { 0 };
                            ws.push_str(&format!("<c r=\"{col}{r}\" t=\"b\"><v>{v}</v></c>"));
                        }
                        TestCell::Empty => {}
                    }
                }
                ws.push_str("</row>");
            }
            ws.push_str("</sheetData></worksheet>");
            zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), opts)
                .unwrap();
            zip.write_all(ws.as_bytes()).unwrap();
        }

        // Sheet 1 rels — absolute path to drawing (openpyxl style)
        zip.start_file("xl/worksheets/_rels/sheet1.xml.rels", opts)
            .unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
              <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
              <Relationship Id=\"rId1\" \
              Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" \
              Target=\"/xl/drawings/drawing1.xml\"/>\
              </Relationships>",
        )
        .unwrap();

        // xl/drawings/drawing1.xml
        zip.start_file("xl/drawings/drawing1.xml", opts).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
             <xdr:twoCellAnchor>\
             <xdr:pic>\
             <xdr:nvPicPr><xdr:cNvPr id=\"1\" name=\"Picture 1\"/><xdr:cNvPicPr/></xdr:nvPicPr>\
             <xdr:blipFill><a:blip r:embed=\"rId1\"/></xdr:blipFill>\
             </xdr:pic>\
             </xdr:twoCellAnchor>\
             </xdr:wsDr>",
        )
        .unwrap();

        // xl/drawings/_rels/drawing1.xml.rels — absolute path to media (openpyxl style)
        let drawing_rels = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" \
             Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" \
             Target=\"/xl/media/{image_filename}\"/>\
             </Relationships>"
        );
        zip.start_file("xl/drawings/_rels/drawing1.xml.rels", opts)
            .unwrap();
        zip.write_all(drawing_rels.as_bytes()).unwrap();

        // xl/media/image1.png
        zip.start_file(format!("xl/media/{image_filename}"), opts)
            .unwrap();
        zip.write_all(image_data).unwrap();

        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_xlsx_image_extraction_absolute_paths() {
        use TestCell::*;
        let data = build_test_xlsx_with_image_absolute_paths(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].0, "image1.png");
        assert_eq!(result.images[0].1, b"fake-png-data");
    }

    #[test]
    fn test_xlsx_image_in_markdown_absolute_paths() {
        use TestCell::*;
        let data = build_test_xlsx_with_image_absolute_paths(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result.markdown.contains("![](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
    }

    #[test]
    fn test_xlsx_image_describer_absolute_paths() {
        use TestCell::*;
        let data = build_test_xlsx_with_image_absolute_paths(
            &[("Sheet1", &[&[Str("Name")][..], &[Str("Alice")]])],
            "image1.png",
            b"fake-png-data",
        );
        let converter = XlsxConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "A chart from openpyxl".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&data, &options).unwrap();
        assert!(
            result
                .markdown
                .contains("![A chart from openpyxl](image1.png)"),
            "markdown was: {}",
            result.markdown
        );
    }
}
