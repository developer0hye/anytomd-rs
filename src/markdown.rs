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

/// Unescape a table cell value, reversing the escaping from `escape_cell`.
///
/// Converts `<br>` back to newlines, `\|` to `|`, and `\\` to `\`.
fn unescape_cell(content: &str) -> String {
    content
        .replace("<br>", "\n")
        .replace("\\|", "|")
        .replace("\\\\", "\\")
}

/// Strip Markdown formatting from text, producing plain text output.
///
/// Processes the input line-by-line with code-block state tracking to
/// preserve code block content while stripping all other Markdown syntax.
pub fn strip_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_code_block = false;
    let mut consecutive_blank = 0u32;

    for line in md.lines() {
        // Code block fences
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        // Inside code blocks, preserve content as-is
        if in_code_block {
            consecutive_blank = 0;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let trimmed = line.trim();

        // Table separator rows (e.g., |---|---|)
        if trimmed.starts_with('|')
            && trimmed.ends_with('|')
            && trimmed
                .chars()
                .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
        {
            continue;
        }

        // Table rows: extract cell values, tab-separated
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let inner = &trimmed[1..trimmed.len() - 1];
            let cells = split_table_row(inner);
            let plain_cells: Vec<String> = cells
                .iter()
                .map(|c| strip_inline_markdown(&unescape_cell(c.trim())))
                .collect();
            let row = plain_cells.join("\t");
            if !row.trim().is_empty() {
                consecutive_blank = 0;
                out.push_str(&row);
                out.push('\n');
            }
            continue;
        }

        // Horizontal rules (---, ***, ___)
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            continue;
        }

        // Headings: strip # prefix
        if let Some(rest) = strip_heading_prefix(trimmed) {
            let plain = strip_inline_markdown(rest);
            consecutive_blank = 0;
            out.push_str(&plain);
            out.push('\n');
            continue;
        }

        // Blockquotes: strip > prefix (supports nested)
        if let Some(rest) = strip_blockquote_prefix(line) {
            let plain = strip_inline_markdown(rest);
            consecutive_blank = 0;
            out.push_str(&plain);
            out.push('\n');
            continue;
        }

        // List items: strip marker, preserve indent
        if let Some((indent, rest)) = strip_list_marker(line) {
            let plain = strip_inline_markdown(rest);
            consecutive_blank = 0;
            out.push_str(&indent);
            out.push_str(&plain);
            out.push('\n');
            continue;
        }

        // Blank line handling: collapse multiple blank lines to at most 1
        if trimmed.is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 1 {
                out.push('\n');
            }
            continue;
        }

        // Regular text: strip inline markdown
        consecutive_blank = 0;
        let plain = strip_inline_markdown(trimmed);
        out.push_str(&plain);
        out.push('\n');
    }

    // Trim trailing whitespace but preserve a single trailing newline if content exists
    let result = out.trim_end().to_string();
    if result.is_empty() {
        result
    } else {
        result + "\n"
    }
}

/// Strip a heading prefix (`# `, `## `, etc.) and return the remaining text.
fn strip_heading_prefix(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut level = 0usize;
    for &b in bytes {
        if b == b'#' {
            level += 1;
        } else {
            break;
        }
    }
    if (1..=6).contains(&level) && line.len() > level && bytes[level] == b' ' {
        Some(&line[level + 1..])
    } else {
        None
    }
}

/// Strip blockquote prefix (`> `) and return the remaining text.
/// Supports nested blockquotes (`>> `).
fn strip_blockquote_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('>') {
        let mut rest = trimmed;
        while rest.starts_with('>') {
            rest = rest[1..].trim_start();
        }
        Some(rest)
    } else {
        None
    }
}

/// Strip a list marker (`- `, `* `, `1. `) and return (indent, remaining text).
fn strip_list_marker(line: &str) -> Option<(String, &str)> {
    let stripped = line.trim_start();
    let indent_len = line.len() - stripped.len();
    let indent = " ".repeat(indent_len);

    // Unordered list markers
    let after_marker = stripped
        .strip_prefix("- ")
        .or_else(|| stripped.strip_prefix("* "));

    if let Some(rest) = after_marker {
        // Strip checkbox markers [x] or [ ]
        let rest = rest
            .strip_prefix("[x] ")
            .or_else(|| rest.strip_prefix("[X] "))
            .or_else(|| rest.strip_prefix("[ ] "))
            .unwrap_or(rest);
        return Some((indent, rest));
    }

    // Ordered list: digits followed by `. `
    if let Some(dot_pos) = stripped.find(". ") {
        let prefix = &stripped[..dot_pos];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            return Some((indent, &stripped[dot_pos + 2..]));
        }
    }

    None
}

/// Strip inline Markdown formatting from a single line/text fragment.
fn strip_inline_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Images: ![alt](url) -> alt
        if chars[i] == '!'
            && i + 1 < len
            && chars[i + 1] == '['
            && let Some((alt, end)) = extract_link_or_image(&chars, i + 1)
        {
            result.push_str(&alt);
            i = end;
            continue;
        }

        // Links: [text](url) -> text
        if chars[i] == '['
            && let Some((link_text, end)) = extract_link_or_image(&chars, i)
        {
            result.push_str(&link_text);
            i = end;
            continue;
        }

        // Bold+italic (***text***), bold (**text**), italic (*text*)
        if chars[i] == '*' {
            let star_count = chars[i..].iter().take_while(|&&c| c == '*').count();
            if star_count >= 3
                && let Some(end) = find_closing_marker(&chars, i + 3, "***")
            {
                let inner: String = chars[i + 3..end].iter().collect();
                result.push_str(&inner);
                i = end + 3;
                continue;
            }
            if star_count >= 2
                && let Some(end) = find_closing_marker(&chars, i + 2, "**")
            {
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&inner);
                i = end + 2;
                continue;
            }
            if star_count >= 1
                && let Some(end) = find_closing_marker(&chars, i + 1, "*")
            {
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&inner);
                i = end + 1;
                continue;
            }
        }

        // Inline code: `code`
        if chars[i] == '`'
            && let Some(end) = find_closing_backtick(&chars, i + 1)
        {
            let inner: String = chars[i + 1..end].iter().collect();
            result.push_str(&inner);
            i = end + 1;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Extract the text part from a link or image pattern starting at `[`.
/// Returns (text, index_after_closing_paren).
fn extract_link_or_image(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start >= chars.len() || chars[start] != '[' {
        return None;
    }

    // Find closing ]
    let mut depth = 0;
    let mut bracket_end = None;
    for (j, &ch) in chars.iter().enumerate().skip(start) {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    bracket_end = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }

    let bracket_end = bracket_end?;

    // Must be followed by (url)
    if bracket_end + 1 >= chars.len() || chars[bracket_end + 1] != '(' {
        return None;
    }

    // Find closing )
    let mut paren_depth = 0;
    let mut paren_end = None;
    for (j, &ch) in chars.iter().enumerate().skip(bracket_end + 1) {
        match ch {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    paren_end = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }

    let paren_end = paren_end?;
    let text: String = chars[start + 1..bracket_end].iter().collect();
    Some((text, paren_end + 1))
}

/// Find the position of a closing marker (like `**` or `***`) starting from `start`.
fn find_closing_marker(chars: &[char], start: usize, marker: &str) -> Option<usize> {
    let marker_chars: Vec<char> = marker.chars().collect();
    let mlen = marker_chars.len();

    if start + mlen > chars.len() {
        return None;
    }

    for i in start..=(chars.len() - mlen) {
        let matches = chars[i..i + mlen]
            .iter()
            .zip(marker_chars.iter())
            .all(|(a, b)| a == b);
        if matches {
            // For ** marker, make sure the next char is not * (avoid matching inside ***)
            if marker == "**" && i + mlen < chars.len() && chars[i + mlen] == '*' {
                continue;
            }
            if marker == "*"
                && ((i > 0 && chars[i - 1] == '*')
                    || (i + mlen < chars.len() && chars[i + mlen] == '*'))
            {
                continue;
            }
            return Some(i);
        }
    }
    None
}

/// Find the position of a closing backtick for inline code.
fn find_closing_backtick(chars: &[char], start: usize) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == '`')
}

/// Split a table row's inner content (between outer pipes) on unescaped `|`.
///
/// Escaped pipes (`\|`) are not treated as cell delimiters.
fn split_table_row(inner: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            // Escaped character — include both in current cell
            current.push(chars[i]);
            current.push(chars[i + 1]);
            i += 2;
        } else if chars[i] == '|' {
            cells.push(current);
            current = String::new();
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }
    cells.push(current);
    cells
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

    // --- strip_markdown unit tests ---

    #[test]
    fn test_strip_markdown_headings() {
        let md = "# Heading 1\n## Heading 2\n### Heading 3\n#### Heading 4\n##### Heading 5\n###### Heading 6\n";
        let result = strip_markdown(md);
        assert_eq!(
            result,
            "Heading 1\nHeading 2\nHeading 3\nHeading 4\nHeading 5\nHeading 6\n"
        );
    }

    #[test]
    fn test_strip_markdown_bold_italic() {
        let md = "This is **bold** and *italic* and ***both***.\n";
        let result = strip_markdown(md);
        assert_eq!(result, "This is bold and italic and both.\n");
    }

    #[test]
    fn test_strip_markdown_table() {
        let md = "| Name | Age |\n|---|---|\n| Alice | 30 |\n| Bob | 25 |\n";
        let result = strip_markdown(md);
        assert!(result.contains("Name\tAge"));
        assert!(result.contains("Alice\t30"));
        assert!(result.contains("Bob\t25"));
    }

    #[test]
    fn test_strip_markdown_table_escaped_cells() {
        let md = "| Col |\n|---|\n| a\\|b |\n| line1<br>line2 |\n| a\\\\b |\n";
        let result = strip_markdown(md);
        assert!(result.contains("a|b"));
        assert!(result.contains("line1\nline2"));
        assert!(result.contains("a\\b"));
    }

    #[test]
    fn test_strip_markdown_code_block() {
        let md = "Before\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nAfter\n";
        let result = strip_markdown(md);
        assert!(result.contains("fn main() {"));
        assert!(result.contains("    println!(\"hello\");"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
        // Fences should be stripped
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_strip_markdown_code_block_with_markdown_inside() {
        let md = "```\n# Not a heading\n**Not bold**\n```\n";
        let result = strip_markdown(md);
        assert!(result.contains("# Not a heading"));
        assert!(result.contains("**Not bold**"));
    }

    #[test]
    fn test_strip_markdown_lists() {
        let md = "- Item 1\n- Item 2\n  - Nested\n1. First\n2. Second\n";
        let result = strip_markdown(md);
        assert!(result.contains("Item 1"));
        assert!(result.contains("Item 2"));
        assert!(result.contains("  Nested"));
        assert!(result.contains("First"));
        assert!(result.contains("Second"));
        // Markers should be stripped
        assert!(!result.contains("- "));
        assert!(!result.contains("1. "));
    }

    #[test]
    fn test_strip_markdown_blockquotes() {
        let md = "> Single quote\n>> Nested quote\n";
        let result = strip_markdown(md);
        assert!(result.contains("Single quote"));
        assert!(result.contains("Nested quote"));
        assert!(!result.contains("> "));
    }

    #[test]
    fn test_strip_markdown_links_and_images() {
        let md = "Click [here](https://example.com) and see ![logo](logo.png).\n";
        let result = strip_markdown(md);
        assert!(result.contains("Click here and see logo."));
        assert!(!result.contains("https://example.com"));
        assert!(!result.contains("logo.png"));
    }

    #[test]
    fn test_strip_markdown_horizontal_rule() {
        let md = "Before\n\n---\n\nAfter\n";
        let result = strip_markdown(md);
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
        assert!(!result.contains("---"));
    }

    #[test]
    fn test_strip_markdown_checkboxes() {
        let md = "- [x] Done\n- [ ] Todo\n";
        let result = strip_markdown(md);
        assert!(result.contains("Done"));
        assert!(result.contains("Todo"));
        assert!(!result.contains("[x]"));
        assert!(!result.contains("[ ]"));
    }

    #[test]
    fn test_strip_markdown_inline_code() {
        let md = "Use `println!` to print.\n";
        let result = strip_markdown(md);
        assert_eq!(result, "Use println! to print.\n");
    }

    #[test]
    fn test_strip_markdown_mixed_document() {
        let md = "\
# Title

Some **bold** text with a [link](http://example.com).

| Col1 | Col2 |
|---|---|
| A | B |

- Item 1
- Item 2

> A quote

```
code here
```

End.
";
        let result = strip_markdown(md);
        assert!(result.contains("Title"));
        assert!(result.contains("Some bold text with a link."));
        assert!(result.contains("Col1\tCol2"));
        assert!(result.contains("A\tB"));
        assert!(result.contains("Item 1"));
        assert!(result.contains("Item 2"));
        assert!(result.contains("A quote"));
        assert!(result.contains("code here"));
        assert!(result.contains("End."));
        // No markdown markers
        assert!(!result.contains("# "));
        assert!(!result.contains("**"));
        assert!(!result.contains("|---|"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn test_strip_markdown_empty_input() {
        assert_eq!(strip_markdown(""), "");
    }

    #[test]
    fn test_strip_markdown_plain_text_passthrough() {
        let text = "Just plain text with no markdown.\n";
        assert_eq!(strip_markdown(text), text);
    }

    #[test]
    fn test_strip_markdown_unicode_preserved() {
        let md = "# 한국어 제목\n\n**中文粗体** and *日本語*\n\nEmoji: 🚀🎉\n";
        let result = strip_markdown(md);
        assert!(result.contains("한국어 제목"));
        assert!(result.contains("中文粗体"));
        assert!(result.contains("日本語"));
        assert!(result.contains("🚀🎉"));
    }

    #[test]
    fn test_strip_markdown_consecutive_blank_lines_collapsed() {
        let md = "Line 1\n\n\n\n\nLine 2\n";
        let result = strip_markdown(md);
        // Should have at most 2 consecutive blank lines
        assert!(!result.contains("\n\n\n"));
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_unescape_cell_basic() {
        assert_eq!(unescape_cell("a\\|b"), "a|b");
        assert_eq!(unescape_cell("a\\\\b"), "a\\b");
        assert_eq!(unescape_cell("line1<br>line2"), "line1\nline2");
    }
}
