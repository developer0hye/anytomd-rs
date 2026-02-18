use std::io::{Read, Seek};

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
