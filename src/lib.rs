pub mod converter;
pub mod detection;
pub mod error;
pub mod markdown;

pub use converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
pub use error::ConvertError;

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
    use converter::csv_conv::CsvConverter;
    use converter::docx::DocxConverter;
    use converter::json_conv::JsonConverter;
    use converter::plain_text::PlainTextConverter;
    use converter::xlsx::XlsxConverter;

    let converters: Vec<Box<dyn Converter>> = vec![
        Box::new(DocxConverter),
        Box::new(XlsxConverter),
        Box::new(JsonConverter),
        Box::new(CsvConverter),
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
