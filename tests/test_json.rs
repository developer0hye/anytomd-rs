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

/// Integration test: sample.json end-to-end conversion via convert_file.
#[test]
fn test_json_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.json", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.starts_with("```json\n"));
    assert!(result.markdown.contains("Sample Document"));
    assert!(result.markdown.contains("한국어"));
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("\"tags\""));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_json_golden_sample() {
    let result = convert_file("tests/fixtures/sample.json", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.json.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: JSON detected by content heuristic (starts with {).
#[test]
fn test_json_detected_by_heuristic() {
    let input = br#"{"detected": true}"#;
    let result = anytomd::convert_bytes(input, "json", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("\"detected\": true"));
}
