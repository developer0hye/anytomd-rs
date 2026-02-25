//! JSON to Markdown converter.
//!
//! Pretty-prints JSON content inside a fenced code block with `json` syntax highlighting.

use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;

/// Converts JSON files to Markdown fenced code blocks.
pub struct JsonConverter;

impl Converter for JsonConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["json"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let (text, encoding_warning) = super::decode_text(data);

        // Parse and re-serialize for pretty-printing
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ConvertError::MalformedDocument {
                reason: format!("invalid JSON: {e}"),
            })?;

        let pretty =
            serde_json::to_string_pretty(&value).map_err(|e| ConvertError::MalformedDocument {
                reason: format!("failed to serialize JSON: {e}"),
            })?;

        let markdown = format!("```json\n{pretty}\n```\n");
        let plain_text = format!("{pretty}\n");
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
    fn test_json_simple_object() {
        let converter = JsonConverter;
        let input = br#"{"key": "value"}"#;
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```json\n"));
        assert!(result.markdown.ends_with("\n```\n"));
        assert!(result.markdown.contains("\"key\""));
        assert!(result.markdown.contains("\"value\""));
    }

    #[test]
    fn test_json_pretty_printed() {
        let converter = JsonConverter;
        let input = br#"{"a":1,"b":2}"#;
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        // Pretty-printed JSON should have indentation
        assert!(result.markdown.contains("  \"a\": 1"));
        assert!(result.markdown.contains("  \"b\": 2"));
    }

    #[test]
    fn test_json_array() {
        let converter = JsonConverter;
        let input = b"[1, 2, 3]";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```json\n"));
        assert!(result.markdown.contains("1"));
        assert!(result.markdown.contains("2"));
        assert!(result.markdown.contains("3"));
    }

    #[test]
    fn test_json_nested_object() {
        let converter = JsonConverter;
        let input = br#"{"outer": {"inner": "value"}}"#;
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("\"outer\""));
        assert!(result.markdown.contains("\"inner\""));
        assert!(result.markdown.contains("\"value\""));
    }

    #[test]
    fn test_json_unicode_cjk() {
        let converter = JsonConverter;
        let input = r#"{"name": "한국어 中文 日本語"}"#.as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어 中文 日本語"));
    }

    #[test]
    fn test_json_emoji() {
        let converter = JsonConverter;
        let input = r#"{"emoji": "🚀✨🌍"}"#.as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀✨🌍"));
    }

    #[test]
    fn test_json_invalid_returns_error() {
        let converter = JsonConverter;
        let input = b"{ invalid json }";
        let result = converter.convert(input, &ConversionOptions::default());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConvertError::MalformedDocument { .. }
        ));
    }

    #[test]
    fn test_json_empty_object() {
        let converter = JsonConverter;
        let input = b"{}";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("{}"));
    }

    #[test]
    fn test_json_empty_array() {
        let converter = JsonConverter;
        let input = b"[]";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("[]"));
    }

    #[test]
    fn test_json_supported_extensions() {
        let converter = JsonConverter;
        assert!(converter.supported_extensions().contains(&"json"));
        assert!(!converter.supported_extensions().contains(&"txt"));
    }

    #[test]
    fn test_json_can_convert() {
        let converter = JsonConverter;
        assert!(converter.can_convert("json", &[]));
        assert!(!converter.can_convert("csv", &[]));
    }

    #[test]
    fn test_json_no_title_or_images() {
        let converter = JsonConverter;
        let result = converter
            .convert(b"{}", &ConversionOptions::default())
            .unwrap();
        assert!(result.title.is_none());
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_json_plain_text_no_fences() {
        let converter = JsonConverter;
        let input = br#"{"name": "Alice"}"#;
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(!result.plain_text.contains("```"));
        assert!(result.plain_text.contains("\"name\""));
        assert!(result.plain_text.contains("\"Alice\""));
    }

    #[test]
    fn test_json_plain_text_pretty_printed() {
        let converter = JsonConverter;
        let input = br#"{"a":1,"b":2}"#;
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert!(result.plain_text.contains("  \"a\": 1"));
        assert!(!result.plain_text.contains("```"));
    }

    #[test]
    fn test_json_invalid_utf8_returns_error() {
        let converter = JsonConverter;
        let input = vec![0xFF, 0xFE];
        let result = converter.convert(&input, &ConversionOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_json_utf8_bom_is_accepted() {
        let converter = JsonConverter;
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(br#"{"k":1}"#);
        let result = converter
            .convert(&input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("\"k\""));
        assert!(result.markdown.contains("1"));
    }

    #[test]
    fn test_json_utf16_le_bom_is_accepted_with_warning() {
        let converter = JsonConverter;
        let mut input = vec![0xFF, 0xFE];
        for code_unit in "{\"k\":1}".encode_utf16() {
            input.extend_from_slice(&code_unit.to_le_bytes());
        }
        let result = converter
            .convert(&input, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("\"k\""));
        assert!(!result.warnings.is_empty(), "expected encoding warning");
    }
}
