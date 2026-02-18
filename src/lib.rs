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
    let (format, is_zip_magic) = match format {
        Some("zip") => (detection::detect_zip_format(&data), true),
        other => (other, false),
    };

    let extension = match format {
        Some(fmt) => fmt,
        None if is_zip_magic => {
            // ZIP magic bytes detected but not a known OOXML format — reject
            return Err(ConvertError::UnsupportedFormat {
                extension: "zip".to_string(),
            });
        }
        None => path.extension().and_then(|e| e.to_str()).unwrap_or(""),
    };

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
    use converter::image::ImageConverter;
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
        Box::new(ImageConverter),
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
    fn test_convert_file_non_ooxml_zip_returns_unsupported() {
        // Create a minimal valid ZIP file that is NOT an OOXML format
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip_writer = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default();
            zip_writer.start_file("hello.txt", options).unwrap();
            std::io::Write::write_all(&mut zip_writer, b"hello world").unwrap();
            zip_writer.finish().unwrap();
        }
        let zip_data = buf.into_inner();

        // Write to a temp file with .txt extension
        let dir = std::env::temp_dir().join("anytomd_test_zip_misroute");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("archive.txt");
        std::fs::write(&file_path, &zip_data).unwrap();

        let options = ConversionOptions::default();
        let result = convert_file(&file_path, &options);

        assert!(result.is_err(), "expected UnsupportedFormat error");
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("unsupported format"),
            "error was: {err}"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
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
