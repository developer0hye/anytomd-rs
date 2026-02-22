use std::collections::HashMap;

use crate::converter::ooxml_utils::{
    ImageInfo, PendingImageResolution, resolve_image_placeholders,
};
use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode, mime_from_image,
};
use crate::error::ConvertError;

pub struct ImageConverter;

/// Derive a file extension from a MIME type string.
///
/// Returns the extension without a dot (e.g., `"png"`, `"jpg"`).
/// Falls back to an empty string if the MIME type is unrecognized.
fn ext_from_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/heic" => "heic",
        "image/avif" => "avif",
        _ => "",
    }
}

// ---- Internal conversion (parse + image extraction, no resolution) ----

impl ImageConverter {
    /// Build image markdown with a placeholder and extract image bytes
    /// without resolving the description.
    ///
    /// Returns the conversion result (with unresolved placeholder in markdown)
    /// and pending image data for later resolution (sync or async).
    pub(crate) fn convert_inner(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<(ConversionResult, PendingImageResolution), ConvertError> {
        let mut warnings = Vec::new();

        let mime = mime_from_image("image", data);
        let ext = ext_from_mime(mime);
        let filename = if ext.is_empty() {
            "image".to_string()
        } else {
            format!("image.{}", ext)
        };

        // Check byte budget
        if data.len() > options.max_total_image_bytes {
            warnings.push(ConversionWarning {
                code: WarningCode::ResourceLimitReached,
                message: format!(
                    "image size ({} bytes) exceeds limit ({})",
                    data.len(),
                    options.max_total_image_bytes
                ),
                location: Some(filename.clone()),
            });
            return Ok((
                ConversionResult {
                    markdown: String::new(),
                    title: None,
                    images: Vec::new(),
                    warnings,
                },
                PendingImageResolution::default(),
            ));
        }

        let placeholder = "__img_0__".to_string();
        let markdown = format!("![{placeholder}]({filename})\n");

        let image_infos = vec![ImageInfo {
            placeholder,
            original_alt: String::new(),
            filename: filename.clone(),
        }];

        let mut image_bytes_map = HashMap::new();
        image_bytes_map.insert(filename.clone(), data.to_vec());

        // Extract image data if requested
        let images = if options.extract_images {
            vec![(filename, data.to_vec())]
        } else {
            Vec::new()
        };

        let result = ConversionResult {
            markdown,
            title: None,
            images,
            warnings,
        };

        let pending = PendingImageResolution {
            infos: image_infos,
            bytes: image_bytes_map,
        };

        Ok((result, pending))
    }
}

// ---- Converter trait impl ----

impl Converter for ImageConverter {
    fn supported_extensions(&self) -> &[&str] {
        &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "svg", "heic", "heif",
            "avif", "image",
        ]
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
    use crate::converter::ImageDescriber;
    use std::sync::Arc;

    // Minimal PNG header (8 bytes)
    const PNG_HEADER: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // Minimal JPEG header (3 bytes, padded)
    const JPEG_HEADER: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];

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
                reason: "API timeout".to_string(),
            })
        }
    }

    #[test]
    fn test_image_supported_extensions() {
        let converter = ImageConverter;
        let exts = converter.supported_extensions();
        for expected in &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "svg", "heic", "heif",
            "avif", "image",
        ] {
            assert!(exts.contains(expected), "missing extension: {}", expected);
        }
    }

    #[test]
    fn test_image_simple_png() {
        let converter = ImageConverter;
        let result = converter
            .convert(&PNG_HEADER, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image.png)\n");
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_image_simple_jpeg() {
        let converter = ImageConverter;
        let result = converter
            .convert(&JPEG_HEADER, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image.jpg)\n");
    }

    #[test]
    fn test_image_unknown_format() {
        let converter = ImageConverter;
        let data = b"unknown-format-data";
        let result = converter
            .convert(data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image)\n");
    }

    #[test]
    fn test_image_describer_replaces_alt_text() {
        let converter = ImageConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(MockDescriber {
                description: "A sunset over the ocean".to_string(),
            })),
            ..Default::default()
        };
        let result = converter.convert(&PNG_HEADER, &options).unwrap();
        assert_eq!(result.markdown, "![A sunset over the ocean](image.png)\n");
    }

    #[test]
    fn test_image_describer_error_keeps_empty_alt() {
        let converter = ImageConverter;
        let options = ConversionOptions {
            image_describer: Some(Arc::new(FailingDescriber)),
            ..Default::default()
        };
        let result = converter.convert(&PNG_HEADER, &options).unwrap();
        assert_eq!(result.markdown, "![](image.png)\n");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::SkippedElement
                    && w.message.contains("image description failed"))
        );
    }

    #[test]
    fn test_image_extract_images_flag() {
        let converter = ImageConverter;
        let options = ConversionOptions {
            extract_images: true,
            ..Default::default()
        };
        let result = converter.convert(&PNG_HEADER, &options).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].0, "image.png");
        assert_eq!(result.images[0].1, PNG_HEADER.to_vec());
    }

    #[test]
    fn test_image_extract_images_default_false() {
        let converter = ImageConverter;
        let result = converter
            .convert(&PNG_HEADER, &ConversionOptions::default())
            .unwrap();
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_image_byte_budget_exceeded() {
        let converter = ImageConverter;
        let options = ConversionOptions {
            max_total_image_bytes: 4, // Less than PNG_HEADER (8 bytes)
            ..Default::default()
        };
        let result = converter.convert(&PNG_HEADER, &options).unwrap();
        assert_eq!(result.markdown, "");
        assert!(result.images.is_empty());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::ResourceLimitReached)
        );
    }

    #[test]
    fn test_image_empty_input() {
        let converter = ImageConverter;
        let result = converter
            .convert(&[], &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image)\n");
    }

    #[test]
    fn test_ext_from_mime_known_types() {
        assert_eq!(ext_from_mime("image/png"), "png");
        assert_eq!(ext_from_mime("image/jpeg"), "jpg");
        assert_eq!(ext_from_mime("image/gif"), "gif");
        assert_eq!(ext_from_mime("image/webp"), "webp");
        assert_eq!(ext_from_mime("image/bmp"), "bmp");
        assert_eq!(ext_from_mime("image/tiff"), "tiff");
        assert_eq!(ext_from_mime("image/svg+xml"), "svg");
        assert_eq!(ext_from_mime("image/heic"), "heic");
        assert_eq!(ext_from_mime("image/avif"), "avif");
    }

    #[test]
    fn test_ext_from_mime_unknown() {
        assert_eq!(ext_from_mime("application/octet-stream"), "");
        assert_eq!(ext_from_mime("text/plain"), "");
    }

    #[test]
    fn test_image_gif_magic_bytes() {
        let converter = ImageConverter;
        let data = b"GIF89a\x00\x00";
        let result = converter
            .convert(data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image.gif)\n");
    }

    #[test]
    fn test_image_webp_magic_bytes() {
        let converter = ImageConverter;
        let data = b"RIFF\x00\x00\x00\x00WEBP";
        let result = converter
            .convert(data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "![](image.webp)\n");
    }
}
