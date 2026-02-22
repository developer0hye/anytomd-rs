mod common;

use anytomd::{convert_file, ConversionOptions};
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
    assert!(result
        .markdown
        .contains("[Example Site](https://example.com)"));
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
    assert!(result
        .markdown
        .contains("The only way to do great work is to love what you do."));

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
