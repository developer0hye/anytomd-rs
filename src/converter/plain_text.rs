use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;

pub struct PlainTextConverter;

impl Converter for PlainTextConverter {
    fn supported_extensions(&self) -> &[&str] {
        &[
            "txt", "text", "log", "md", "markdown", "rst", "ini", "cfg", "conf", "toml", "yaml",
            "yml",
        ]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let text = String::from_utf8(data.to_vec())?;

        // Strip UTF-8 BOM if present
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);

        Ok(ConversionResult {
            markdown: text.to_string(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_simple_passthrough() {
        let converter = PlainTextConverter;
        let input = b"Hello, world!";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "Hello, world!");
    }

    #[test]
    fn test_plain_text_empty_input() {
        let converter = PlainTextConverter;
        let result = converter
            .convert(b"", &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "");
    }

    #[test]
    fn test_plain_text_multiline() {
        let converter = PlainTextConverter;
        let input = b"Line 1\nLine 2\nLine 3\n";
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "Line 1\nLine 2\nLine 3\n");
    }

    #[test]
    fn test_plain_text_utf8_bom_stripped() {
        let converter = PlainTextConverter;
        let mut input = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        input.extend_from_slice(b"BOM content");
        let result = converter
            .convert(&input, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "BOM content");
    }

    #[test]
    fn test_plain_text_unicode_cjk() {
        let converter = PlainTextConverter;
        let input = "한국어 中文 日本語".as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "한국어 中文 日本語");
    }

    #[test]
    fn test_plain_text_emoji() {
        let converter = PlainTextConverter;
        let input = "Hello 🌍🚀✨ World".as_bytes();
        let result = converter
            .convert(input, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "Hello 🌍🚀✨ World");
    }

    #[test]
    fn test_plain_text_invalid_utf8_returns_error() {
        let converter = PlainTextConverter;
        let input = vec![0xFF, 0xFE, 0x00];
        let result = converter.convert(&input, &ConversionOptions::default());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConvertError::Utf8Error(_)));
    }

    #[test]
    fn test_plain_text_supported_extensions() {
        let converter = PlainTextConverter;
        assert!(converter.supported_extensions().contains(&"txt"));
        assert!(converter.supported_extensions().contains(&"md"));
        assert!(converter.supported_extensions().contains(&"log"));
        assert!(converter.supported_extensions().contains(&"yaml"));
        assert!(!converter.supported_extensions().contains(&"docx"));
    }

    #[test]
    fn test_plain_text_can_convert() {
        let converter = PlainTextConverter;
        assert!(converter.can_convert("txt", &[]));
        assert!(converter.can_convert("md", &[]));
        assert!(!converter.can_convert("docx", &[]));
    }

    #[test]
    fn test_plain_text_no_title_extracted() {
        let converter = PlainTextConverter;
        let result = converter
            .convert(b"Some text", &ConversionOptions::default())
            .unwrap();
        assert!(result.title.is_none());
    }

    #[test]
    fn test_plain_text_no_images_extracted() {
        let converter = PlainTextConverter;
        let result = converter
            .convert(b"Some text", &ConversionOptions::default())
            .unwrap();
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_plain_text_no_warnings() {
        let converter = PlainTextConverter;
        let result = converter
            .convert(b"Some text", &ConversionOptions::default())
            .unwrap();
        assert!(result.warnings.is_empty());
    }
}
