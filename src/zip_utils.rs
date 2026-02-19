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
