mod common;

use anytomd::{convert_file, ConversionOptions};
use common::normalize;

/// Integration test: sample.txt end-to-end conversion via convert_file.
#[test]
fn test_plain_text_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.txt", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("Hello, world!"));
    assert!(result.markdown.contains("한국어 中文 日本語"));
    assert!(result.markdown.contains("🚀✨🌍"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_plain_text_golden_sample() {
    let result = convert_file("tests/fixtures/sample.txt", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.txt.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_file with a .md extension is detected as plain text.
#[test]
fn test_plain_text_md_extension_detected() {
    // Use convert_bytes directly since we don't have a .md fixture file
    let input = b"# Heading\n\nSome markdown content.";
    let result = anytomd::convert_bytes(input, "md", &ConversionOptions::default()).unwrap();
    assert_eq!(result.markdown, "# Heading\n\nSome markdown content.");
}

/// Integration test: unsupported format still returns error.
#[test]
fn test_unsupported_format_returns_error() {
    let result = anytomd::convert_bytes(b"data", "xyz", &ConversionOptions::default());
    assert!(result.is_err());
}
