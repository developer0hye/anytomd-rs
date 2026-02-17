/// Errors that can occur during document conversion.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("unsupported format: {extension}")]
    UnsupportedFormat { extension: String },

    #[error("failed to read ZIP archive")]
    ZipError(#[from] zip::result::ZipError),

    #[error("failed to parse XML")]
    XmlError(#[from] quick_xml::Error),

    #[error("failed to read spreadsheet")]
    SpreadsheetError(#[from] calamine::Error),

    #[error("I/O error")]
    Io(#[from] std::io::Error),

    #[error("invalid UTF-8 content")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("malformed document: {reason}")]
    MalformedDocument { reason: String },
}
