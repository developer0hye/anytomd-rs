use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;

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
        let text = String::from_utf8(data.to_vec())?;

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
    fn test_json_invalid_utf8_returns_error() {
        let converter = JsonConverter;
        let input = vec![0xFF, 0xFE];
        let result = converter.convert(&input, &ConversionOptions::default());
        assert!(result.is_err());
    }
}
