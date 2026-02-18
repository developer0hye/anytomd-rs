pub mod csv_conv;
pub mod docx;
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
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            extract_images: false,
            max_total_image_bytes: 50 * 1024 * 1024, // 50 MB
            strict: false,
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
