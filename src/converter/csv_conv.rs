use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;
use crate::markdown::build_table;

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
        let text = String::from_utf8(data.to_vec())?;
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);

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

        Ok(ConversionResult {
            markdown,
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
    fn test_csv_invalid_utf8_returns_error() {
        let converter = CsvConverter;
        let input = vec![0xFF, 0xFE];
        let result = converter.convert(&input, &ConversionOptions::default());
        assert!(result.is_err());
    }
}
