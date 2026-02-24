//! Document format converters and shared conversion types.
//!
//! Each submodule implements the [`Converter`] trait for a specific format.
//! The public types ([`ConversionOptions`], [`ConversionResult`], [`ConversionWarning`])
//! are re-exported from the crate root.

pub mod code;
pub mod csv;
pub mod docx;
pub mod gemini;
pub mod html;
pub mod image;
pub mod ipynb;
pub mod json;
pub(crate) mod ooxml_utils;
pub mod plain_text;
pub mod pptx;
pub mod xlsx;
pub mod xml;

use std::sync::Arc;

#[cfg(feature = "async")]
use std::future::Future;
#[cfg(feature = "async")]
use std::pin::Pin;

use crate::error::ConvertError;

/// Trait for generating image descriptions using an LLM or other backend.
///
/// Implementors receive raw image bytes and return a textual description.
/// The built-in `GeminiDescriber` (behind the `gemini` feature) uses
/// Google Gemini, but any LLM backend can be plugged in.
pub trait ImageDescriber: Send + Sync {
    /// Describe the given image.
    ///
    /// - `image_bytes`: raw image data (PNG, JPEG, etc.)
    /// - `mime_type`: MIME type of the image (e.g., `"image/png"`)
    /// - `prompt`: instruction for the LLM (e.g., "Describe this image concisely")
    fn describe(
        &self,
        image_bytes: &[u8],
        mime_type: &str,
        prompt: &str,
    ) -> Result<String, ConvertError>;
}

/// Categories for recoverable conversion warnings.
///
/// Used in [`ConversionWarning`] to classify the type of issue encountered
/// during best-effort conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningCode {
    /// A document element was skipped because it could not be parsed.
    SkippedElement,
    /// The document uses a feature not yet supported by this converter.
    UnsupportedFeature,
    /// A resource limit was reached (e.g., max image bytes exceeded).
    ResourceLimitReached,
    /// Part of the document was malformed but conversion continued.
    MalformedSegment,
}

/// A recoverable issue encountered during conversion.
///
/// Warnings are collected in [`ConversionResult::warnings`] when the converter
/// encounters problems but can continue processing the rest of the document.
/// In strict mode ([`ConversionOptions::strict`] = `true`), these become errors instead.
#[derive(Debug, Clone)]
pub struct ConversionWarning {
    /// The category of this warning.
    pub code: WarningCode,
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Where in the document the issue occurred (e.g., "Sheet1, row 5").
    pub location: Option<String>,
}

/// Options controlling conversion behavior.
#[derive(Clone)]
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
    /// Optional image describer for LLM-based alt text generation.
    pub image_describer: Option<Arc<dyn ImageDescriber>>,
}

impl std::fmt::Debug for ConversionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversionOptions")
            .field("extract_images", &self.extract_images)
            .field("max_total_image_bytes", &self.max_total_image_bytes)
            .field("strict", &self.strict)
            .field("max_input_bytes", &self.max_input_bytes)
            .field(
                "max_uncompressed_zip_bytes",
                &self.max_uncompressed_zip_bytes,
            )
            .field(
                "image_describer",
                &self.image_describer.as_ref().map(|_| ".."),
            )
            .finish()
    }
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            extract_images: false,
            max_total_image_bytes: 50 * 1024 * 1024, // 50 MB
            strict: false,
            max_input_bytes: 100 * 1024 * 1024, // 100 MB
            max_uncompressed_zip_bytes: 500 * 1024 * 1024, // 500 MB
            image_describer: None,
        }
    }
}

/// The result of converting a document to Markdown.
#[derive(Debug, Clone, Default)]
pub struct ConversionResult {
    /// Converted Markdown content.
    pub markdown: String,
    /// Plain text content extracted directly from the source document,
    /// without any Markdown formatting. Tables are tab-separated,
    /// headings are just text, and code is raw content without fences.
    pub plain_text: String,
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

/// Infer a MIME type from an image filename extension and magic bytes.
///
/// Falls back to `"application/octet-stream"` if unrecognized.
pub(crate) fn mime_from_image(filename: &str, data: &[u8]) -> &'static str {
    // Check magic bytes first
    if data.len() >= 8 {
        if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return "image/png";
        }
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return "image/jpeg";
        }
        if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            return "image/gif";
        }
        if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
            return "image/webp";
        }
    }

    // Fallback to extension
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "svg" => "image/svg+xml",
        "heic" | "heif" => "image/heic",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

/// Replace a single image placeholder with its description in the markdown.
///
/// Finds the first occurrence of `![{placeholder}]({filename})` and replaces the
/// placeholder with the description. Uses unique placeholders to avoid ambiguity
/// with duplicate filenames.
pub(crate) fn replace_image_alt_by_placeholder(
    markdown: &str,
    placeholder: &str,
    description: &str,
    filename: &str,
) -> String {
    let target = format!("![{placeholder}]({filename})");
    let replacement = format!("![{description}]({filename})");
    // Replace exactly ONE occurrence
    if let Some(pos) = markdown.find(&target) {
        let mut result = String::with_capacity(markdown.len());
        result.push_str(&markdown[..pos]);
        result.push_str(&replacement);
        result.push_str(&markdown[pos + target.len()..]);
        result
    } else {
        markdown.to_string()
    }
}

/// Async description future type for native targets.
///
/// Native async describers return `Send` futures so they can be scheduled on
/// multithreaded executors.
#[cfg(all(feature = "async", not(target_arch = "wasm32")))]
pub type AsyncDescribeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, ConvertError>> + Send + 'a>>;

/// Async description future type for WASM targets.
///
/// On WASM, futures backed by browser APIs are often not `Send`, so the async
/// image describer future omits the `Send` bound.
#[cfg(all(feature = "async", target_arch = "wasm32"))]
pub type AsyncDescribeFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ConvertError>> + 'a>>;

/// Async trait for generating image descriptions using an LLM or other backend.
///
/// This is the async counterpart of [`ImageDescriber`]. It uses
/// `Pin<Box<dyn Future>>` for dyn-compatibility (async fn in traits is not
/// dyn-safe).
///
/// Requires the `async` feature.
#[cfg(all(feature = "async", not(target_arch = "wasm32")))]
pub trait AsyncImageDescriber: Send + Sync {
    /// Describe the given image asynchronously.
    ///
    /// - `image_bytes`: raw image data (PNG, JPEG, etc.)
    /// - `mime_type`: MIME type of the image (e.g., `"image/png"`)
    /// - `prompt`: instruction for the LLM (e.g., "Describe this image concisely")
    fn describe<'a>(
        &'a self,
        image_bytes: &'a [u8],
        mime_type: &'a str,
        prompt: &'a str,
    ) -> AsyncDescribeFuture<'a>;
}

/// Async trait for generating image descriptions using an LLM or other backend.
///
/// This is the async counterpart of [`ImageDescriber`]. It uses
/// `Pin<Box<dyn Future>>` for dyn-compatibility (async fn in traits is not
/// dyn-safe).
///
/// On WASM targets this trait does not require `Send + Sync`, and the returned
/// future also omits `Send`.
#[cfg(all(feature = "async", target_arch = "wasm32"))]
pub trait AsyncImageDescriber {
    /// Describe the given image asynchronously.
    ///
    /// - `image_bytes`: raw image data (PNG, JPEG, etc.)
    /// - `mime_type`: MIME type of the image (e.g., `"image/png"`)
    /// - `prompt`: instruction for the LLM (e.g., "Describe this image concisely")
    fn describe<'a>(
        &'a self,
        image_bytes: &'a [u8],
        mime_type: &'a str,
        prompt: &'a str,
    ) -> AsyncDescribeFuture<'a>;
}

/// Conversion options for the async API.
///
/// Wraps the base [`ConversionOptions`] and adds an async image describer.
///
/// Requires the `async` feature.
#[cfg(feature = "async")]
#[derive(Default)]
pub struct AsyncConversionOptions {
    /// Base conversion options (resource limits, extract_images, strict mode).
    pub base: ConversionOptions,
    /// Optional async image describer for concurrent LLM-based alt text generation.
    pub async_image_describer: Option<Arc<dyn AsyncImageDescriber>>,
}

#[cfg(feature = "async")]
impl std::fmt::Debug for AsyncConversionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncConversionOptions")
            .field("base", &self.base)
            .field(
                "async_image_describer",
                &self.async_image_describer.as_ref().map(|_| ".."),
            )
            .finish()
    }
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

    // ---- MIME detection tests ----

    #[test]
    fn test_mime_from_image_png_magic_bytes() {
        let png_header = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(mime_from_image("image.png", &png_header), "image/png");
        // Magic bytes take priority over extension
        assert_eq!(mime_from_image("image.jpg", &png_header), "image/png");
    }

    #[test]
    fn test_mime_from_image_jpeg_magic_bytes() {
        let jpeg_header = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert_eq!(mime_from_image("photo.jpg", &jpeg_header), "image/jpeg");
    }

    #[test]
    fn test_mime_from_image_gif_magic_bytes() {
        assert_eq!(mime_from_image("anim.gif", b"GIF89a.."), "image/gif");
        assert_eq!(mime_from_image("old.gif", b"GIF87a.."), "image/gif");
    }

    #[test]
    fn test_mime_from_image_webp_magic_bytes() {
        let webp = b"RIFF\x00\x00\x00\x00WEBP";
        assert_eq!(mime_from_image("photo.webp", webp), "image/webp");
    }

    #[test]
    fn test_mime_from_image_extension_fallback() {
        let empty = b"unknown";
        assert_eq!(mime_from_image("file.png", empty), "image/png");
        assert_eq!(mime_from_image("file.jpg", empty), "image/jpeg");
        assert_eq!(mime_from_image("file.jpeg", empty), "image/jpeg");
        assert_eq!(mime_from_image("file.gif", empty), "image/gif");
        assert_eq!(mime_from_image("file.webp", empty), "image/webp");
        assert_eq!(mime_from_image("file.bmp", empty), "image/bmp");
        assert_eq!(mime_from_image("file.tiff", empty), "image/tiff");
        assert_eq!(mime_from_image("file.svg", empty), "image/svg+xml");
        assert_eq!(mime_from_image("file.heic", empty), "image/heic");
        assert_eq!(mime_from_image("file.heif", empty), "image/heic");
        assert_eq!(mime_from_image("file.avif", empty), "image/avif");
        assert_eq!(
            mime_from_image("file.xyz", empty),
            "application/octet-stream"
        );
    }

    // ---- ConversionOptions tests ----

    #[test]
    fn test_conversion_options_default_has_no_describer() {
        let opts = ConversionOptions::default();
        assert!(opts.image_describer.is_none());
    }

    #[test]
    fn test_conversion_options_debug_format() {
        let opts = ConversionOptions::default();
        let debug = format!("{:?}", opts);
        assert!(debug.contains("ConversionOptions"));
        assert!(debug.contains("image_describer: None"));
    }

    #[test]
    fn test_conversion_options_clone_with_describer() {
        use crate::error::ConvertError;

        struct MockDescriber;
        impl ImageDescriber for MockDescriber {
            fn describe(
                &self,
                _image_bytes: &[u8],
                _mime_type: &str,
                _prompt: &str,
            ) -> Result<String, ConvertError> {
                Ok("mock".to_string())
            }
        }

        let opts = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber)),
            ..Default::default()
        };
        let cloned = opts.clone();
        assert!(cloned.image_describer.is_some());
    }

    // ---- replace_image_alt_by_placeholder tests ----

    #[test]
    fn test_replace_image_alt_placeholder_match() {
        let md = "![__img_0__](cat.png)";
        let result = replace_image_alt_by_placeholder(md, "__img_0__", "A cute cat", "cat.png");
        assert_eq!(result, "![A cute cat](cat.png)");
    }

    #[test]
    fn test_replace_image_alt_placeholder_no_match() {
        let md = "![__img_0__](cat.png)";
        let result = replace_image_alt_by_placeholder(md, "__img_99__", "description", "cat.png");
        assert_eq!(result, md);
    }

    #[test]
    fn test_replace_image_alt_placeholder_only_first_occurrence() {
        let md = "![__img_0__](cat.png) and ![__img_0__](cat.png)";
        let result = replace_image_alt_by_placeholder(md, "__img_0__", "A cat", "cat.png");
        assert_eq!(result, "![A cat](cat.png) and ![__img_0__](cat.png)");
    }

    #[test]
    fn test_replace_image_alt_placeholder_same_filename_different_placeholders() {
        let md = "![__img_0__](logo.png)\n![__img_1__](logo.png)";
        let result = replace_image_alt_by_placeholder(md, "__img_1__", "Second logo", "logo.png");
        assert!(result.contains("![__img_0__](logo.png)"));
        assert!(result.contains("![Second logo](logo.png)"));
    }

    #[test]
    fn test_conversion_options_debug_with_describer() {
        use crate::error::ConvertError;

        struct MockDescriber;
        impl ImageDescriber for MockDescriber {
            fn describe(
                &self,
                _image_bytes: &[u8],
                _mime_type: &str,
                _prompt: &str,
            ) -> Result<String, ConvertError> {
                Ok("mock".to_string())
            }
        }

        let opts = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber)),
            ..Default::default()
        };
        let debug = format!("{:?}", opts);
        assert!(debug.contains("image_describer: Some"));
    }
}
