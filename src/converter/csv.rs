//! CSV to Markdown table converter.
//!
//! Parses CSV data using the `csv` crate and renders it as a pipe-delimited
//! Markdown table. The first row is treated as the header.

use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;
use crate::markdown::{build_table, build_table_plain};

/// Converts CSV files to Markdown tables.
pub struct CsvConverter;

impl Converter for CsvConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["csv"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let (text, encoding_warning) = super::decode_text(data);

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());

        let mut records = reader.records();

        // First record becomes the header
        let header_record = match records.next() {
            Some(Ok(rec)) => rec,
            Some(Err(e)) => {
                return Err(ConvertError::MalformedDocument {
                    reason: format!("failed to parse CSV header: {e}"),
                });
            }
            None => {
                return Ok(ConversionResult {
                    markdown: String::new(),
                    ..Default::default()
                });
            }
        };

        let headers: Vec<String> = header_record.iter().map(|s| s.to_string()).collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        for result in records {
            match result {
                Ok(record) => {
                    let row: Vec<String> = record.iter().map(|s| s.to_string()).collect();
                    rows.push(row);
                }
                Err(e) => {
                    return Err(ConvertError::MalformedDocument {
                        reason: format!("failed to parse CSV row: {e}"),
                    });
                }
            }
        }

        let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
        let row_refs: Vec<Vec<&str>> = rows
            .iter()
            .map(|row| row.iter().map(|s| s.as_str()).collect())
            .collect();
        let markdown = build_table(&header_refs, &row_refs);
        let plain_text = build_table_plain(&header_refs, &row_refs);

        let mut warnings = Vec::new();
        if let Some(w) = encoding_warning {
            warnings.push(w);
        }

        Ok(ConversionResult {
            markdown,
            plain_text,
            warnings,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_simple_table() {
        let converter = CsvConverter;
        let input = b"A,B,C\n1,2,3\n4,5,6\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| A | B | C |"));
        assert!(result.markdown.contains("|---|---|---|"));
        assert!(result.markdown.contains("| 1 | 2 | 3 |"));
        assert!(result.markdown.contains("| 4 | 5 | 6 |"));
    }

    #[test]
    fn test_csv_single_row_header_only() {
        let converter = CsvConverter;
        let input = b"X,Y,Z\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| X | Y | Z |"));
        assert!(result.markdown.contains("|---|---|---|"));
        let lines: Vec<&str> = result.markdown.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_csv_single_column() {
        let converter = CsvConverter;
        let input = b"Name\nAlice\nBob\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| Name |"));
        assert!(result.markdown.contains("| Alice |"));
        assert!(result.markdown.contains("| Bob |"));
    }

    #[test]
    fn test_csv_empty_input() {
        let converter = CsvConverter;
        let result = converter
            .convert(b"", &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "");
    }

    #[test]
    fn test_csv_unicode_cjk() {
        let converter = CsvConverter;
        let input = "이름,나이\n홍길동,30\n田中,25\n".as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("홍길동"));
        assert!(result.markdown.contains("田中"));
        assert!(result.markdown.contains("이름"));
    }

    #[test]
    fn test_csv_emoji() {
        let converter = CsvConverter;
        let input = "Symbol,Meaning\n🚀,Rocket\n✨,Sparkle\n".as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨"));
    }

    #[test]
    fn test_csv_quoted_fields() {
        let converter = CsvConverter;
        let input = b"Name,City\nAlice,\"New York\"\nBob,\"San Francisco\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("New York"));
        assert!(result.markdown.contains("San Francisco"));
    }

    #[test]
    fn test_csv_short_rows_padded() {
        let converter = CsvConverter;
        let input = b"A,B,C\n1\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("| 1 |  |  |"));
    }

    #[test]
    fn test_csv_whitespace_in_cells() {
        let converter = CsvConverter;
        let input = b"Key,Value\n hello , world \n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains(" hello "));
        assert!(result.markdown.contains(" world "));
    }

    #[test]
    fn test_csv_supported_extensions() {
        let converter = CsvConverter;
        assert!(converter.supported_extensions().contains(&"csv"));
        assert!(!converter.supported_extensions().contains(&"txt"));
    }

    #[test]
    fn test_csv_can_convert() {
        let converter = CsvConverter;
        assert!(converter.can_convert("csv", &[]));
        assert!(!converter.can_convert("json", &[]));
    }

    #[test]
    fn test_csv_no_title_images_warnings() {
        let converter = CsvConverter;
        let result = converter
            .convert(b"A\n1\n", &ConversionOptions::default())
            .unwrap();
        assert!(result.title.is_none());
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_csv_pipe_in_cell_escaped() {
        let converter = CsvConverter;
        let input = b"Name,Command\nAlice,echo \"hello\" | grep h\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Pipe should be escaped so it doesn't break the Markdown table
        assert!(
            !result.markdown.contains("| echo \"hello\" | grep h |"),
            "raw pipe in cell should be escaped, got: {}",
            result.markdown
        );
        assert!(result.markdown.contains("grep h"));
    }

    #[test]
    fn test_csv_plain_text_tab_separated() {
        let converter = CsvConverter;
        let input = b"Name,Age,City\nAlice,30,Seoul\nBob,25,Tokyo\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.plain_text.contains("Name\tAge\tCity"));
        assert!(result.plain_text.contains("Alice\t30\tSeoul"));
        assert!(result.plain_text.contains("Bob\t25\tTokyo"));
        assert!(!result.plain_text.contains("|"));
        assert!(!result.plain_text.contains("---"));
    }

    #[test]
    fn test_csv_plain_text_pipe_in_cell_preserved() {
        let converter = CsvConverter;
        let input = b"Name,Command\nAlice,\"echo | grep\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // In plain text, pipes should NOT be escaped
        assert!(result.plain_text.contains("echo | grep"));
    }

    #[test]
    fn test_csv_non_utf8_decoded_with_warning() {
        let converter = CsvConverter;
        // Windows-1252 encoded CSV
        let input = b"Name,City\nAlice,Montr\xe9al\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("Montr\u{00e9}al"));
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_csv_multiline_quoted_field() {
        let converter = CsvConverter;
        let input = b"Name,Bio\nAlice,\"Line one\nLine two\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Newline inside quoted field should become <br> in markdown table
        assert!(
            result.markdown.contains("Line one<br>Line two"),
            "multiline cell should use <br>, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_escaped_quotes_in_field() {
        let converter = CsvConverter;
        // RFC 4180: doubled quotes inside quoted field
        let input = b"Name,Quote\nAlice,\"She said \"\"hello\"\"\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("She said \"hello\""),
            "escaped quotes should be unescaped, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_multiline_with_unicode() {
        let converter = CsvConverter;
        let input = "Name,Note\n홍길동,\"첫째 줄\n둘째 줄 🎉\"\n".as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("홍길동"));
        assert!(
            result.markdown.contains("첫째 줄<br>둘째 줄 🎉"),
            "CJK + emoji multiline should work, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_crlf_in_quoted_field() {
        let converter = CsvConverter;
        let input = b"A,B\nX,\"line1\r\nline2\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(
            result.markdown.contains("line1<br>line2"),
            "CRLF in quoted field should become <br>, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_pipe_and_newline_combined() {
        let converter = CsvConverter;
        let input = b"Cmd,Output\ntest,\"echo | grep\nhello\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Pipe should be escaped AND newline should become <br>
        assert!(
            result.markdown.contains("\\|"),
            "pipe should be escaped, got: {}",
            result.markdown
        );
        assert!(
            result.markdown.contains("<br>"),
            "newline should become <br>, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_backslash_in_quoted_field() {
        let converter = CsvConverter;
        let input = b"Path,Value\nroot,\"C:\\Users\\test\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Backslashes should be escaped in markdown table
        assert!(
            result.markdown.contains("C:\\\\Users\\\\test"),
            "backslashes should be escaped, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_empty_quoted_field() {
        let converter = CsvConverter;
        let input = b"A,B,C\n1,\"\",3\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Empty quoted field should render as empty cell
        assert!(
            result.markdown.contains("| 1 |  | 3 |"),
            "empty quoted field should be empty cell, got: {}",
            result.markdown
        );
    }

    #[test]
    fn test_csv_plain_text_multiline_preserved() {
        let converter = CsvConverter;
        let input = b"Name,Bio\nAlice,\"Line one\nLine two\"\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // In plain text, multiline cells appear with tab-separated columns
        assert!(result.plain_text.contains("Name\tBio"));
        assert!(
            result.plain_text.contains("Alice\t"),
            "plain text should have tab-separated cells, got: {}",
            result.plain_text
        );
        // The cell content includes the original newline in plain text
        assert!(
            result.plain_text.contains("Line one") && result.plain_text.contains("Line two"),
            "multiline content should be present in plain text, got: {}",
            result.plain_text
        );
    }
}
