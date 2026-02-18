use anytomd::{convert_file, ConversionOptions};

/// Normalize whitespace for golden test comparison:
/// trim each line, collapse consecutive blank lines, strip trailing newline.
fn normalize(s: &str) -> String {
    let lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    let mut result = String::new();
    let mut prev_blank = false;
    for line in &lines {
        let is_blank = line.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        prev_blank = is_blank;
    }
    result.trim_end().to_string()
}

/// Content coverage test: verify key elements are present in the converted output.
///
/// Fixture: tests/fixtures/sample.docx
/// Contains: H1 "Sample Document", body paragraph, H2 "Section One",
///           paragraph with hyperlink to example.com, Korean text, emoji, H3 "Subsection".
#[test]
fn test_docx_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.docx", &ConversionOptions::default()).unwrap();

    // Title should be extracted from first H1
    assert_eq!(result.title, Some("Sample Document".to_string()));

    // Headings
    assert!(result.markdown.contains("# Sample Document"));
    assert!(result.markdown.contains("## Section One"));
    assert!(result.markdown.contains("### Subsection"));

    // Body paragraphs
    assert!(result.markdown.contains("This is a simple paragraph."));
    assert!(result
        .markdown
        .contains("Final paragraph with mixed content."));

    // Hyperlink
    assert!(result.markdown.contains("[Example](https://example.com)"));

    // Unicode: Korean
    assert!(result.markdown.contains("한국어 테스트"));

    // Emoji
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("✨"));
    assert!(result.markdown.contains("🌍"));
}

/// Golden test: normalized comparison against expected output file.
#[test]
fn test_docx_golden_sample() {
    let result = convert_file("tests/fixtures/sample.docx", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.docx.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Direct convert_bytes test: verify the converter works with explicit extension.
#[test]
fn test_docx_convert_bytes_direct() {
    let data = std::fs::read("tests/fixtures/sample.docx").unwrap();
    let result = anytomd::convert_bytes(&data, "docx", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("# Sample Document"));
    assert!(result.markdown.contains("한국어"));
}
