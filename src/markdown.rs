/// Build a pipe-delimited Markdown table from headers and rows.
///
/// Each row is padded or truncated to match the header count.
/// Empty headers produce a table with no header labels but with a separator row.
pub fn build_table(headers: &[&str], rows: &[Vec<&str>]) -> String {
    let col_count = headers.len();
    if col_count == 0 {
        return String::new();
    }

    let mut out = String::new();

    // Header row
    out.push('|');
    for h in headers {
        out.push(' ');
        out.push_str(h);
        out.push_str(" |");
    }
    out.push('\n');

    // Separator row
    out.push('|');
    for _ in 0..col_count {
        out.push_str("---|");
    }
    out.push('\n');

    // Data rows
    for row in rows {
        out.push('|');
        for i in 0..col_count {
            out.push(' ');
            if let Some(cell) = row.get(i) {
                out.push_str(cell);
            }
            out.push_str(" |");
        }
        out.push('\n');
    }

    out
}

/// Format a Markdown heading at the given level (clamped to 1..=6).
pub fn format_heading(level: u8, text: &str) -> String {
    let level = level.clamp(1, 6);
    let hashes = "#".repeat(level as usize);
    format!("{} {}\n", hashes, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_table_basic() {
        let result = build_table(&["A", "B"], &[vec!["1", "2"], vec!["3", "4"]]);
        assert!(result.contains("| A | B |"));
        assert!(result.contains("|---|---|"));
        assert!(result.contains("| 1 | 2 |"));
        assert!(result.contains("| 3 | 4 |"));
    }

    #[test]
    fn test_build_table_empty_headers() {
        let result = build_table(&[], &[vec!["x"]]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_build_table_short_rows_padded() {
        let result = build_table(&["A", "B", "C"], &[vec!["1"]]);
        assert!(result.contains("| 1 |  |  |"));
    }

    #[test]
    fn test_build_table_no_rows() {
        let result = build_table(&["X", "Y"], &[]);
        assert!(result.contains("| X | Y |"));
        assert!(result.contains("|---|---|"));
        // No data rows
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_format_heading_levels_1_through_6() {
        assert_eq!(format_heading(1, "Title"), "# Title\n");
        assert_eq!(format_heading(2, "Sub"), "## Sub\n");
        assert_eq!(format_heading(3, "H3"), "### H3\n");
        assert_eq!(format_heading(4, "H4"), "#### H4\n");
        assert_eq!(format_heading(5, "H5"), "##### H5\n");
        assert_eq!(format_heading(6, "H6"), "###### H6\n");
    }

    #[test]
    fn test_format_heading_clamped_below() {
        assert_eq!(format_heading(0, "Zero"), "# Zero\n");
    }

    #[test]
    fn test_format_heading_clamped_above() {
        assert_eq!(format_heading(7, "Seven"), "###### Seven\n");
        assert_eq!(format_heading(255, "Max"), "###### Max\n");
    }
}
