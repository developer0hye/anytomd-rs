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

/// Integration test: sample.xml end-to-end conversion via convert_file.
/// Fixture contains: XML declaration, nested elements, attributes, CJK text,
/// emoji, XML comment, and self-closing element.
#[test]
fn test_xml_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.xml", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.starts_with("```xml\n"));
    assert!(result.markdown.contains("Sample XML Document"));
    assert!(result.markdown.contains("한국어 텍스트"));
    assert!(result.markdown.contains("中文文本"));
    assert!(result.markdown.contains("日本語テキスト"));
    assert!(result.markdown.contains("🚀 Rocket launch! ✨🌍"));
    assert!(result.markdown.contains("<!-- This is a comment -->"));
    assert!(result.markdown.contains("<separator/>"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_xml_golden_sample() {
    let result = convert_file("tests/fixtures/sample.xml", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.xml.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit "xml" extension.
#[test]
fn test_xml_convert_bytes() {
    let input = b"<root><child>hello</child></root>";
    let result = anytomd::convert_bytes(input, "xml", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.starts_with("```xml\n"));
    assert!(result.markdown.contains("<child>hello</child>"));
    assert!(result.markdown.ends_with("\n```\n"));
}

/// Integration test: XML conversion produces no title, images, or warnings.
#[test]
fn test_xml_no_metadata() {
    let result = convert_file("tests/fixtures/sample.xml", &ConversionOptions::default()).unwrap();
    assert!(result.title.is_none());
    assert!(result.images.is_empty());
    assert!(result.warnings.is_empty());
}
