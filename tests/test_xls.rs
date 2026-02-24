#![cfg(not(target_arch = "wasm32"))]

mod common;

use anytomd::{ConversionOptions, convert_file};
use common::normalize;

/// Integration test: sample.xls end-to-end conversion via convert_file.
/// Fixture is the MarkItDown reference test.xls containing two sheets:
/// Sheet1 (Alpha/Beta/Gamma/Delta headers with 23 numeric rows, one UUID in Beta)
/// and a second sheet with ColA-ColD headers and 4 rows.
#[test]
fn test_xls_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.xls", &ConversionOptions::default()).unwrap();
    // Sheet 1 content
    assert!(result.markdown.contains("## Sheet1"));
    assert!(result.markdown.contains("| Alpha | Beta | Gamma | Delta |"));
    assert!(result.markdown.contains("| 89 | 82 | 100 | 12 |"));
    assert!(
        result
            .markdown
            .contains("6ff4173b-42a5-4784-9b19-f49caff4d93d")
    );
    // Sheet 2 content
    assert!(
        result
            .markdown
            .contains("## 09060124-b5e7-4717-9d07-3c046eb")
    );
    assert!(result.markdown.contains("| ColA | ColB | ColC | ColD |"));
    assert!(
        result
            .markdown
            .contains("affc7dad-52dc-4b98-9b5d-51e65d8a8ad0")
    );
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_xls_golden_sample() {
    let result = convert_file("tests/fixtures/sample.xls", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.xls.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit "xls" extension.
#[test]
fn test_xls_convert_bytes_direct() {
    let data = std::fs::read("tests/fixtures/sample.xls").unwrap();
    let result = anytomd::convert_bytes(&data, "xls", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("## Sheet1"));
    assert!(result.markdown.contains("| Alpha | Beta | Gamma | Delta |"));
    assert!(result.markdown.contains("| ColA | ColB | ColC | ColD |"));
}

/// Integration test: XLS with Korean/CJK Unicode content preserved.
/// Fixture: sample_unicode.xls Sheet1 has Korean names and CJK city names.
#[test]
fn test_xls_unicode_cjk_content() {
    let result = convert_file(
        "tests/fixtures/sample_unicode.xls",
        &ConversionOptions::default(),
    )
    .unwrap();
    // Korean names
    assert!(result.markdown.contains("홍길동"));
    assert!(result.markdown.contains("김다영"));
    // Japanese name
    assert!(result.markdown.contains("田中太郎"));
    // CJK city names
    assert!(result.markdown.contains("서울"));
    assert!(result.markdown.contains("東京"));
    assert!(result.markdown.contains("北京"));
    // Table structure preserved
    assert!(result.markdown.contains("| Name | Age | City |"));
}

/// Integration test: XLS with emoji content preserved.
/// Fixture: sample_unicode.xls Sheet2 has emoji in product names and notes.
#[test]
fn test_xls_emoji_content() {
    let result = convert_file(
        "tests/fixtures/sample_unicode.xls",
        &ConversionOptions::default(),
    )
    .unwrap();
    // Emoji in product names
    assert!(result.markdown.contains("🚀 Rocket Launch"));
    assert!(result.markdown.contains("🎉 Party Pack"));
    assert!(result.markdown.contains("📚 Book Set"));
    // Emoji in notes
    assert!(result.markdown.contains("Special offer ✨"));
    assert!(result.markdown.contains("한국어 포함 🇰🇷"));
    // Empty cell (Party Pack has no note)
    assert!(result.markdown.contains("| 🎉 Party Pack | 24.5 |  |"));
}

/// Golden test: compare normalized Unicode XLS output against expected file.
#[test]
fn test_xls_golden_unicode() {
    let result = convert_file(
        "tests/fixtures/sample_unicode.xls",
        &ConversionOptions::default(),
    )
    .unwrap();
    let expected = include_str!("fixtures/expected/sample_unicode.xls.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with Unicode XLS content.
#[test]
fn test_xls_convert_bytes_unicode() {
    let data = std::fs::read("tests/fixtures/sample_unicode.xls").unwrap();
    let result = anytomd::convert_bytes(&data, "xls", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("## Sheet1"));
    assert!(result.markdown.contains("홍길동"));
    assert!(result.markdown.contains("## Sheet2"));
    assert!(result.markdown.contains("🚀 Rocket Launch"));
}
