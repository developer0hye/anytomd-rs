use std::io::{Cursor, Read, Seek};

use zip::ZipArchive;

use crate::error::ConvertError;

/// Validate that the total uncompressed size of a ZIP archive does not exceed a budget.
///
/// Sums `entry.size()` from the ZIP central directory (no decompression needed).
/// Returns `InputTooLarge` if the total exceeds the budget.
pub(crate) fn validate_zip_budget<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    budget: usize,
) -> Result<(), ConvertError> {
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index_raw(i) {
            total = total.saturating_add(entry.size());
        }
    }
    if total > budget as u64 {
        return Err(ConvertError::InputTooLarge {
            size: total as usize,
            limit: budget,
        });
    }
    Ok(())
}

/// Read a UTF-8 text file from a ZIP archive, returning None if not found.
pub(crate) fn read_zip_text(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Option<String>, ConvertError> {
    let mut file = match archive.by_name(path) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ConvertError::ZipError(e)),
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(Some(buf))
}

/// Read raw bytes from a ZIP archive, returning None if not found.
pub(crate) fn read_zip_bytes(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    path: &str,
) -> Result<Option<Vec<u8>>, ConvertError> {
    let mut file = match archive.by_name(path) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ConvertError::ZipError(e)),
    };
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn build_test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(buf));
        let opts = SimpleFileOptions::default();
        for (name, data) in files {
            zip.start_file(name.to_string(), opts).unwrap();
            zip.write_all(data).unwrap();
        }
        let cursor = zip.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_read_zip_text_found() {
        let data = build_test_zip(&[("hello.txt", b"Hello, world!")]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        let result = read_zip_text(&mut archive, "hello.txt").unwrap();
        assert_eq!(result, Some("Hello, world!".to_string()));
    }

    #[test]
    fn test_read_zip_text_not_found() {
        let data = build_test_zip(&[("hello.txt", b"data")]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        let result = read_zip_text(&mut archive, "missing.txt").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_zip_bytes_found() {
        let binary = vec![0x89, b'P', b'N', b'G', 0x00, 0x01];
        let data = build_test_zip(&[("image.png", &binary)]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        let result = read_zip_bytes(&mut archive, "image.png").unwrap();
        assert_eq!(result, Some(binary));
    }

    #[test]
    fn test_read_zip_bytes_not_found() {
        let data = build_test_zip(&[("file.bin", b"data")]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        let result = read_zip_bytes(&mut archive, "missing.bin").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_validate_zip_budget_within_limit() {
        let data = build_test_zip(&[("a.txt", b"hello"), ("b.txt", b"world")]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        assert!(validate_zip_budget(&mut archive, 1000).is_ok());
    }

    #[test]
    fn test_validate_zip_budget_exceeded() {
        let big = vec![0u8; 500];
        let data = build_test_zip(&[("big.bin", &big)]);
        let mut archive = ZipArchive::new(Cursor::new(data.as_slice())).unwrap();
        let result = validate_zip_budget(&mut archive, 100);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("exceeds limit"));
    }
}
