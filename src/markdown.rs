/// Escape special characters in a table cell so that pipes, backslashes,
/// and newlines do not break Markdown table structure.
fn escape_cell(content: &str) -> String {
    content
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace('\n', "<br>")
        .replace('\r', "")
}

/// Build a pipe-delimited Markdown table from headers and rows.
///
/// Each row is padded or truncated to match the header count.
/// Empty headers produce a table with no header labels but with a separator row.
/// Special characters in headers and cells are escaped via `escape_cell`.
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
        out.push_str(&escape_cell(h));
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
                out.push_str(&escape_cell(cell));
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

/// Wrap text with Markdown bold/italic markers.
///
/// Leading and trailing whitespace is preserved outside the markers for clean output.
/// Returns the text unchanged if neither bold nor italic.
/// Returns empty string if the input text (after trimming) is empty.
pub fn wrap_formatting(text: &str, bold: bool, italic: bool) -> String {
    if !bold && !italic {
        return text.to_string();
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let leading = &text[..text.len() - text.trim_start().len()];
    let trailing = &text[text.trim_end().len()..];

    let wrapped = match (bold, italic) {
        (true, true) => format!("***{trimmed}***"),
        (true, false) => format!("**{trimmed}**"),
        (false, true) => format!("*{trimmed}*"),
        (false, false) => unreachable!(),
    };

    format!("{leading}{wrapped}{trailing}")
}

/// Format a list item with indentation and marker.
///
/// `level` is 0-based indentation depth. `ordered` selects numbered vs bullet marker.
/// `counter` is the 1-based ordinal for ordered lists (ignored for unordered).
pub fn format_list_item(level: u8, ordered: bool, counter: usize, text: &str) -> String {
    let indent = "  ".repeat(level as usize);
    if ordered {
        format!("{indent}{counter}. {text}")
    } else {
        format!("{indent}- {text}")
    }
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

    #[test]
    fn test_wrap_formatting_bold() {
        assert_eq!(wrap_formatting("hello", true, false), "**hello**");
    }

    #[test]
    fn test_wrap_formatting_italic() {
        assert_eq!(wrap_formatting("hello", false, true), "*hello*");
    }

    #[test]
    fn test_wrap_formatting_bold_italic() {
        assert_eq!(wrap_formatting("hello", true, true), "***hello***");
    }

    #[test]
    fn test_wrap_formatting_none() {
        assert_eq!(wrap_formatting("hello", false, false), "hello");
    }

    #[test]
    fn test_wrap_formatting_empty_no_markers() {
        assert_eq!(wrap_formatting("", true, false), "");
        assert_eq!(wrap_formatting("", false, true), "");
        assert_eq!(wrap_formatting("", true, true), "");
    }

    #[test]
    fn test_format_list_item_unordered() {
        assert_eq!(format_list_item(0, false, 1, "Item"), "- Item");
    }

    #[test]
    fn test_format_list_item_ordered() {
        assert_eq!(format_list_item(0, true, 1, "First"), "1. First");
        assert_eq!(format_list_item(0, true, 3, "Third"), "3. Third");
    }

    #[test]
    fn test_format_list_item_nested() {
        assert_eq!(format_list_item(1, false, 1, "Nested"), "  - Nested");
        assert_eq!(format_list_item(2, false, 1, "Deep"), "    - Deep");
        assert_eq!(format_list_item(1, true, 2, "Sub"), "  2. Sub");
    }

    // --- escape_cell unit tests ---

    #[test]
    fn test_escape_cell_pipe() {
        assert_eq!(escape_cell("a|b"), "a\\|b");
    }

    #[test]
    fn test_escape_cell_multiple_pipes() {
        assert_eq!(escape_cell("a|b|c"), "a\\|b\\|c");
    }

    #[test]
    fn test_escape_cell_newline() {
        assert_eq!(escape_cell("line1\nline2"), "line1<br>line2");
    }

    #[test]
    fn test_escape_cell_crlf() {
        assert_eq!(escape_cell("line1\r\nline2"), "line1<br>line2");
    }

    #[test]
    fn test_escape_cell_backslash() {
        assert_eq!(escape_cell("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_cell_backslash_pipe() {
        assert_eq!(escape_cell("a\\|b"), "a\\\\\\|b");
    }

    #[test]
    fn test_escape_cell_empty_and_plain() {
        assert_eq!(escape_cell(""), "");
        assert_eq!(escape_cell("plain text"), "plain text");
    }

    // --- build_table integration tests with escaping ---

    #[test]
    fn test_build_table_pipe_in_cell_escaped() {
        let result = build_table(&["A", "B"], &[vec!["x|y", "z"]]);
        assert!(result.contains("| x\\|y | z |"));
    }

    #[test]
    fn test_build_table_pipe_in_header_escaped() {
        let result = build_table(&["A|1", "B"], &[vec!["x", "y"]]);
        assert!(result.contains("| A\\|1 | B |"));
    }

    #[test]
    fn test_build_table_newline_in_cell_replaced() {
        let result = build_table(&["A"], &[vec!["line1\nline2"]]);
        assert!(result.contains("| line1<br>line2 |"));
    }
}
