mod common;

use anytomd::{ConversionOptions, convert_file};
use common::normalize;

/// Content coverage test: verify key elements are present in the converted output.
///
/// Fixture: tests/fixtures/sample.pptx
/// Contains:
/// - Slide 1: Title "Sample Presentation", body "Welcome to the presentation."
/// - Slide 2: Text "Data Overview", table (Name/Value/Status with 3 data rows),
///            speaker notes "Remember to explain the data table."
/// - Slide 3: Title "Multilingual", Korean text, emoji, speaker notes
#[test]
fn test_pptx_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.pptx", &ConversionOptions::default()).unwrap();

    // Title should be extracted from first slide's title
    assert_eq!(result.title, Some("Sample Presentation".to_string()));

    // Slide headings
    assert!(result.markdown.contains("## Slide 1: Sample Presentation"));
    assert!(result.markdown.contains("## Slide 2"));
    assert!(result.markdown.contains("## Slide 3: Multilingual"));

    // Body text
    assert!(result.markdown.contains("Welcome to the presentation."));
    assert!(result.markdown.contains("Data Overview"));

    // Table content
    assert!(result.markdown.contains("| Name | Value | Status |"));
    assert!(result.markdown.contains("| Alpha | 100 | Active |"));
    assert!(result.markdown.contains("| Beta | 200 | Inactive |"));
    assert!(result.markdown.contains("| Gamma | 300 | Active |"));

    // Speaker notes
    assert!(
        result
            .markdown
            .contains("> Note: Remember to explain the data table.")
    );
    assert!(
        result
            .markdown
            .contains("> Note: Test multilingual rendering.")
    );

    // Slide separators
    assert!(result.markdown.contains("\n\n---\n\n"));

    // Unicode: Korean
    assert!(result.markdown.contains("한국어 테스트"));

    // Emoji
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("✨"));
    assert!(result.markdown.contains("🌍"));

    // No warnings for well-formed PPTX
    assert!(result.warnings.is_empty());
}

/// Golden test: normalized comparison against expected output file.
#[test]
fn test_pptx_golden_sample() {
    let result = convert_file("tests/fixtures/sample.pptx", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.pptx.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Direct convert_bytes test: verify the converter works with explicit extension.
#[test]
fn test_pptx_convert_bytes_direct() {
    let data = std::fs::read("tests/fixtures/sample.pptx").unwrap();
    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("## Slide 1: Sample Presentation"));
    assert!(result.markdown.contains("한국어"));
    assert!(result.markdown.contains("🚀"));
}
