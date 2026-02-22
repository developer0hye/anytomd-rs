mod common;

use anytomd::{ConversionOptions, convert_file};
use common::normalize;

/// Integration test: sample.xlsx end-to-end conversion via convert_file.
/// Fixture contains: two sheets, ASCII names, CJK cities, Korean names, emoji,
/// integers, and floats.
#[test]
fn test_xlsx_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.xlsx", &ConversionOptions::default()).unwrap();
    // Sheet 1 content
    assert!(result.markdown.contains("## Sheet1"));
    assert!(result.markdown.contains("Alice"));
    assert!(result.markdown.contains("東京"));
    assert!(result.markdown.contains("New York"));
    assert!(result.markdown.contains("다영"));
    assert!(result.markdown.contains("서울"));
    // Sheet 2 content
    assert!(result.markdown.contains("## Sheet2"));
    assert!(result.markdown.contains("Widget"));
    assert!(result.markdown.contains("9.99"));
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("✨"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_xlsx_golden_sample() {
    let result = convert_file("tests/fixtures/sample.xlsx", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.xlsx.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit "xlsx" extension.
#[test]
fn test_xlsx_convert_bytes_direct() {
    let data = std::fs::read("tests/fixtures/sample.xlsx").unwrap();
    let result = anytomd::convert_bytes(&data, "xlsx", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("## Sheet1"));
    assert!(result.markdown.contains("## Sheet2"));
    assert!(result.markdown.contains("Alice"));
    assert!(result.markdown.contains("🚀"));
}
