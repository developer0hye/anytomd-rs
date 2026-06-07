#![cfg(not(target_arch = "wasm32"))]

mod common;

use anytomd::{ConversionOptions, convert_file};
use common::normalize;

/// Integration test: sample.html end-to-end conversion via convert_file.
/// Fixture contains headings, bold/italic, links, images, lists, tables,
/// code blocks, blockquotes, CJK text, and emoji.
#[test]
fn test_html_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.html", &ConversionOptions::default()).unwrap();

    // Title extracted from <title> tag
    assert_eq!(result.title, Some("Sample HTML Document".to_string()));

    // Headings
    assert!(result.markdown.contains("# Main Heading"));
    assert!(result.markdown.contains("## Links and Images"));
    assert!(result.markdown.contains("### Lists"));
    assert!(result.markdown.contains("## Data Table"));
    assert!(result.markdown.contains("## Code Block"));
    assert!(result.markdown.contains("## Blockquote"));
    assert!(result.markdown.contains("## Unicode and Emoji"));

    // Inline formatting
    assert!(result.markdown.contains("**bold**"));
    assert!(result.markdown.contains("*italic*"));
    assert!(result.markdown.contains("`inline code`"));

    // Links and images
    assert!(
        result
            .markdown
            .contains("[Example Site](https://example.com)")
    );
    assert!(result.markdown.contains("![Company Logo](logo.png)"));

    // Lists
    assert!(result.markdown.contains("- Apple"));
    assert!(result.markdown.contains("- Banana"));
    assert!(result.markdown.contains("  - Dark cherry"));
    assert!(result.markdown.contains("1. First step"));
    assert!(result.markdown.contains("2. Second step"));

    // Table
    assert!(result.markdown.contains("| Name | City | Score |"));
    assert!(result.markdown.contains("| Alice | Seoul | 95 |"));
    assert!(result.markdown.contains("| Bob | Tokyo | 88 |"));

    // Code block
    assert!(result.markdown.contains("```"));
    assert!(result.markdown.contains("fn main()"));

    // Blockquote
    assert!(result.markdown.contains("> "));
    assert!(
        result
            .markdown
            .contains("The only way to do great work is to love what you do.")
    );

    // Horizontal rule
    assert!(result.markdown.contains("---"));

    // Unicode / CJK
    assert!(result.markdown.contains("한국어 텍스트"));
    assert!(result.markdown.contains("안녕하세요"));
    assert!(result.markdown.contains("中文文本"));
    assert!(result.markdown.contains("日本語テキスト"));

    // Emoji
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("✨"));
    assert!(result.markdown.contains("🌍"));

    // Script and style should NOT appear
    assert!(!result.markdown.contains("console.log"));
    assert!(!result.markdown.contains("font-family"));
    assert!(!result.markdown.contains("<script"));
    assert!(!result.markdown.contains("<style"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_html_golden_sample() {
    let result = convert_file("tests/fixtures/sample.html", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.html.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit "html" extension.
#[test]
fn test_html_convert_bytes_direct() {
    let input = b"<html><body><h1>Hello</h1><p>World</p></body></html>";
    let result = anytomd::convert_bytes(input, "html", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("# Hello"));
    assert!(result.markdown.contains("World"));
}

/// Integration test: a colspan/rowspan layout table renders as one GFM grid.
///
/// Mirrors the structure of a layout table (full-width banner, label/value rows,
/// and a data grid) that previously collapsed to a single column. All columns
/// must survive as one empty-filled grid (no heading / `**Label:**` promotion).
#[test]
fn test_html_colspan_layout_grid() {
    let input = br#"<html><body><table>
        <tr><td colspan="3"><h2>Section Title</h2></td></tr>
        <tr><td>Field</td><td colspan="2">Value</td></tr>
        <tr><td>Col A</td><td>Col B</td><td>Col C</td></tr>
        <tr><td>1</td><td>2</td><td>3</td></tr>
    </table></body></html>"#;
    let result = anytomd::convert_bytes(input, "html", &ConversionOptions::default()).unwrap();

    assert!(
        result.markdown.contains("| Section Title |  |  |"),
        "banner not an empty-filled header: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("| Field | Value |  |"),
        "label/value not a grid row: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("| Col A | Col B | Col C |"),
        "grid columns missing: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("| 1 | 2 | 3 |"),
        "grid data missing: {}",
        result.markdown
    );
    // No heading / bold linearization.
    assert!(
        !result.markdown.contains("## Section"),
        "md: {}",
        result.markdown
    );
    assert!(
        !result.markdown.contains("**Field:**"),
        "md: {}",
        result.markdown
    );
}

/// Regression (review finding): a nested table in a simple outer table must
/// render as a standalone block, not be escaped into one cell; plain text must
/// not leak GFM pipes.
#[test]
fn test_html_nested_table_not_mangled_end_to_end() {
    let input =
        br#"<html><body><table><tr><td>a<table><tr><td>x</td><td>y</td></tr></table></td><td>b</td></tr></table></body></html>"#;
    let result = anytomd::convert_bytes(input, "html", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.contains("| x | y |"),
        "md: {}",
        result.markdown
    );
    assert!(
        !result.markdown.contains("\\|"),
        "escaped pipes: {}",
        result.markdown
    );
    assert!(
        !result.plain_text.contains('|'),
        "plain leaked pipes: {:?}",
        result.plain_text
    );
}

/// Regression (review finding): a huge colspan must not drive unbounded output.
#[test]
fn test_html_colspan_dos_bounded_end_to_end() {
    let input = br#"<html><body><table><tr><td colspan="1000000">X</td><td>Y</td></tr><tr><td colspan="1000000">P</td><td>Q</td></tr></table></body></html>"#;
    let result = anytomd::convert_bytes(input, "html", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.len() < 100_000,
        "not bounded: {} bytes",
        result.markdown.len()
    );
}

/// Regression (review should-fix): a genuine data table with one spanning row
/// must keep all columns as a single GFM table, not fragment into a header-only
/// table plus a relabeled `**x:** spans` line.
#[test]
fn test_html_data_table_spanning_row_keeps_columns() {
    let input = br#"<html><body><table>
        <tr><th>A</th><th>B</th><th>C</th></tr>
        <tr><td>x</td><td colspan="2">spans</td></tr>
    </table></body></html>"#;
    let result = anytomd::convert_bytes(input, "html", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.contains("| A | B | C |"),
        "md: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("| x | spans |  |"),
        "md: {}",
        result.markdown
    );
    assert!(
        !result.markdown.contains("**x:**"),
        "row was relabeled: {}",
        result.markdown
    );
}

/// Regression (best-effort-observable): an over-large table must surface a
/// ResourceLimitReached warning through the public API rather than truncating
/// silently.
#[test]
fn test_html_oversized_table_warns_end_to_end() {
    use anytomd::WarningCode;
    let cols = 1000usize;
    let rows = 200usize; // 200_000 cells > MAX_TABLE_CELLS
    let mut html = String::from("<html><body><table>");
    for _ in 0..rows {
        html.push_str("<tr>");
        for c in 0..cols {
            html.push_str("<td>");
            html.push_str(&c.to_string());
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    html.push_str("</table></body></html>");
    let result =
        anytomd::convert_bytes(html.as_bytes(), "html", &ConversionOptions::default()).unwrap();
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::ResourceLimitReached),
        "no truncation warning surfaced: {:?}",
        result.warnings
    );
}
