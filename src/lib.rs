pub mod converter;
pub mod detection;
pub mod error;
pub mod markdown;
pub(crate) mod zip_utils;

pub use converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, ImageDescriber, WarningCode,
};
pub use error::ConvertError;

#[cfg(feature = "gemini")]
pub mod gemini {
    pub use crate::converter::gemini::GeminiDescriber;
}

use std::path::Path;

/// Convert a file at the given path to Markdown.
///
/// The format is auto-detected from magic bytes and file extension.
pub fn convert_file(
    path: impl AsRef<Path>,
    options: &ConversionOptions,
) -> Result<ConversionResult, ConvertError> {
    let path = path.as_ref();
    let data = std::fs::read(path)?;

    if data.len() > options.max_input_bytes {
        return Err(ConvertError::InputTooLarge {
            size: data.len(),
            limit: options.max_input_bytes,
        });
    }

    let header = &data[..data.len().min(16)];
    let format = detection::detect_format(path, header);

    // For ZIP-based formats, introspect to find the specific type
    let format = match format {
        Some("zip") => detection::detect_zip_format(&data),
        other => other,
    };

    let extension =
        format.unwrap_or_else(|| path.extension().and_then(|e| e.to_str()).unwrap_or(""));

    convert_bytes(&data, extension, options)
}

/// Convert raw bytes to Markdown with an explicit format extension.
pub fn convert_bytes(
    data: &[u8],
    extension: &str,
    options: &ConversionOptions,
) -> Result<ConversionResult, ConvertError> {
    if data.len() > options.max_input_bytes {
        return Err(ConvertError::InputTooLarge {
            size: data.len(),
            limit: options.max_input_bytes,
        });
    }

    use converter::csv_conv::CsvConverter;
    use converter::docx::DocxConverter;
    use converter::html::HtmlConverter;
    use converter::json_conv::JsonConverter;
    use converter::plain_text::PlainTextConverter;
    use converter::pptx::PptxConverter;
    use converter::xlsx::XlsxConverter;
    use converter::xml_conv::XmlConverter;

    let converters: Vec<Box<dyn Converter>> = vec![
        Box::new(DocxConverter),
        Box::new(PptxConverter),
        Box::new(XlsxConverter),
        Box::new(JsonConverter),
        Box::new(XmlConverter),
        Box::new(CsvConverter),
        Box::new(HtmlConverter),
        Box::new(PlainTextConverter),
    ];

    for conv in &converters {
        if conv.can_convert(extension, data) {
            return conv.convert(data, options);
        }
    }

    Err(ConvertError::UnsupportedFormat {
        extension: extension.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_bytes_input_too_large() {
        let data = vec![0u8; 1024];
        let options = ConversionOptions {
            max_input_bytes: 512,
            ..Default::default()
        };
        let result = convert_bytes(&data, "txt", &options);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("input too large"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_convert_bytes_at_exact_limit_succeeds() {
        let data = b"Hello, world!";
        let options = ConversionOptions {
            max_input_bytes: data.len(),
            ..Default::default()
        };
        let result = convert_bytes(data, "txt", &options);
        assert!(result.is_ok());
    }

    #[test]
    fn test_convert_file_input_too_large() {
        // Use existing sample.csv fixture with a tiny limit
        let path = std::path::Path::new("tests/fixtures/sample.csv");
        if !path.exists() {
            return; // Skip if fixture not available
        }
        let file_size = std::fs::metadata(path).unwrap().len() as usize;
        let options = ConversionOptions {
            max_input_bytes: file_size.saturating_sub(1).max(1),
            ..Default::default()
        };
        let result = convert_file(path, &options);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("input too large"),
            "error was: {err}"
        );
    }
}
