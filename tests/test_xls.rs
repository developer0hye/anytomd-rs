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
    assert!(result
        .markdown
        .contains("6ff4173b-42a5-4784-9b19-f49caff4d93d"));
    // Sheet 2 content
    assert!(result
        .markdown
        .contains("## 09060124-b5e7-4717-9d07-3c046eb"));
    assert!(result.markdown.contains("| ColA | ColB | ColC | ColD |"));
    assert!(result
        .markdown
        .contains("affc7dad-52dc-4b98-9b5d-51e65d8a8ad0"));
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
