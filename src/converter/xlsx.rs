use std::io::Cursor;

use calamine::{open_workbook_auto_from_rs, Data, Reader};

use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;
use crate::markdown::{build_table, format_heading};

pub struct XlsxConverter;

/// Format a calamine cell value as a string for Markdown output.
///
/// Whole-number floats display as integers (e.g. `3.0` → `"3"`).
/// Booleans display as `TRUE` / `FALSE`.
/// Errors and empty cells produce an empty string.
fn format_cell(cell: &Data) -> String {
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
        Data::DateTime(dt) => format!("{dt}"),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(_) => String::new(),
    }
}

impl Converter for XlsxConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["xlsx"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let cursor = Cursor::new(data);
        let mut workbook = open_workbook_auto_from_rs(cursor)?;

        let sheet_names = workbook.sheet_names().to_owned();
        let mut sections = Vec::new();
        let mut warnings = Vec::new();

        for name in &sheet_names {
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

            let headers: Vec<String> = header_row.iter().map(format_cell).collect();
            let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();

            let mut data_rows: Vec<Vec<String>> = Vec::new();
            for row in rows_iter {
                let cells: Vec<String> = row.iter().map(format_cell).collect();
                data_rows.push(cells);
            }

            let row_refs: Vec<Vec<&str>> = data_rows
                .iter()
                .map(|row| row.iter().map(|s| s.as_str()).collect())
                .collect();

            let heading = format_heading(2, name);
            let table = build_table(&header_refs, &row_refs);
            sections.push(format!("{heading}{table}"));
        }

        let markdown = sections.join("\n");

        Ok(ConversionResult {
            markdown,
            warnings,
            ..Default::default()
        })
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

    /// Convert a 0-based column index to an Excel column letter (A-Z).
    fn col_letter(col: usize) -> char {
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
                    let col = col_letter(ci);
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
        assert_eq!(converter.supported_extensions(), &["xlsx"]);
    }

    #[test]
    fn test_xlsx_can_convert() {
        let converter = XlsxConverter;
        assert!(converter.can_convert("xlsx", &[]));
        assert!(!converter.can_convert("csv", &[]));
        assert!(!converter.can_convert("json", &[]));
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
}
