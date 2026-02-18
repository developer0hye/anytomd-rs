pub mod csv_conv;
pub mod docx;
pub mod html;
pub mod json_conv;
pub mod plain_text;
pub mod pptx;
pub mod xlsx;
pub mod xml_conv;

use crate::error::ConvertError;

/// Categories for recoverable conversion warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningCode {
    SkippedElement,
    UnsupportedFeature,
    ResourceLimitReached,
    MalformedSegment,
}

/// A recoverable issue encountered during conversion.
#[derive(Debug, Clone)]
pub struct ConversionWarning {
    pub code: WarningCode,
    pub message: String,
    pub location: Option<String>,
}

/// Options controlling conversion behavior.
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Extract embedded images into `ConversionResult.images`.
    pub extract_images: bool,
    /// Hard cap for total extracted image bytes per document.
    pub max_total_image_bytes: usize,
    /// If true, return an error on recoverable parse failures instead of warnings.
    pub strict: bool,
    /// Maximum input file size in bytes. Files larger than this are rejected.
    pub max_input_bytes: usize,
    /// Maximum total uncompressed size of entries in a ZIP-based document.
    pub max_uncompressed_zip_bytes: usize,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            extract_images: false,
            max_total_image_bytes: 50 * 1024 * 1024, // 50 MB
            strict: false,
            max_input_bytes: 100 * 1024 * 1024, // 100 MB
            max_uncompressed_zip_bytes: 500 * 1024 * 1024, // 500 MB
        }
    }
}

/// The result of converting a document to Markdown.
#[derive(Debug, Clone, Default)]
pub struct ConversionResult {
    /// Converted Markdown content.
    pub markdown: String,
    /// Document title, if detected.
    pub title: Option<String>,
    /// Extracted images as (filename, bytes) pairs.
    pub images: Vec<(String, Vec<u8>)>,
    /// Recoverable issues encountered during conversion.
    pub warnings: Vec<ConversionWarning>,
}

/// Decode raw bytes to a UTF-8 string, handling BOM detection and encoding fallback.
///
/// Returns the decoded text and an optional warning if non-UTF-8 encoding was used.
pub(crate) fn decode_text(data: &[u8]) -> (String, Option<ConversionWarning>) {
    // Fast path: valid UTF-8
    if let Ok(text) = std::str::from_utf8(data) {
        // Strip UTF-8 BOM if present
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        return (text.to_string(), None);
    }

    // BOM detection (UTF-16 LE/BE)
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(data) {
        let (decoded, _enc, had_errors) = encoding.decode(&data[bom_len..]);
        let warning = if had_errors {
            ConversionWarning {
                code: WarningCode::MalformedSegment,
                message: format!(
                    "replacement characters inserted during {} decoding",
                    encoding.name()
                ),
                location: None,
            }
        } else {
            ConversionWarning {
                code: WarningCode::UnsupportedFeature,
                message: format!("decoded from {} encoding", encoding.name()),
                location: None,
            }
        };
        return (decoded.into_owned(), Some(warning));
    }

    // Fallback: Windows-1252
    let (decoded, _enc, had_errors) = encoding_rs::WINDOWS_1252.decode(data);
    let warning = if had_errors {
        ConversionWarning {
            code: WarningCode::MalformedSegment,
            message: "replacement characters inserted during windows-1252 decoding".to_string(),
            location: None,
        }
    } else {
        ConversionWarning {
            code: WarningCode::UnsupportedFeature,
            message: "decoded from windows-1252 encoding (fallback)".to_string(),
            location: None,
        }
    };
    (decoded.into_owned(), Some(warning))
}

/// Trait implemented by each format-specific converter.
pub trait Converter {
    /// Returns the file extensions this converter supports (e.g., `["docx"]`).
    fn supported_extensions(&self) -> &[&str];

    /// Check if this converter can handle the given extension.
    fn can_convert(&self, extension: &str, _header_bytes: &[u8]) -> bool {
        self.supported_extensions().contains(&extension)
    }

    /// Convert file bytes to Markdown.
    fn convert(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_text_utf8_passthrough() {
        let (text, warning) = decode_text(b"Hello, world!");
        assert_eq!(text, "Hello, world!");
        assert!(warning.is_none());
    }

    #[test]
    fn test_decode_text_utf8_bom_stripped() {
        let mut input = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        input.extend_from_slice(b"BOM content");
        let (text, warning) = decode_text(&input);
        assert_eq!(text, "BOM content");
        assert!(warning.is_none());
    }

    #[test]
    fn test_decode_text_utf16_le_bom() {
        // UTF-16 LE BOM + "AB"
        let input: Vec<u8> = vec![0xFF, 0xFE, b'A', 0x00, b'B', 0x00];
        let (text, warning) = decode_text(&input);
        assert_eq!(text, "AB");
        assert!(warning.is_some());
    }

    #[test]
    fn test_decode_text_utf16_be_bom() {
        // UTF-16 BE BOM + "AB"
        let input: Vec<u8> = vec![0xFE, 0xFF, 0x00, b'A', 0x00, b'B'];
        let (text, warning) = decode_text(&input);
        assert_eq!(text, "AB");
        assert!(warning.is_some());
    }

    #[test]
    fn test_decode_text_windows_1252_fallback() {
        // 0xE9 = é in Windows-1252
        let input = b"caf\xe9";
        let (text, warning) = decode_text(input);
        assert_eq!(text, "café");
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert_eq!(w.code, WarningCode::UnsupportedFeature);
    }

    #[test]
    fn test_decode_text_cjk_utf8() {
        let input = "한국어 中文 日本語".as_bytes();
        let (text, warning) = decode_text(input);
        assert_eq!(text, "한국어 中文 日本語");
        assert!(warning.is_none());
    }
}
