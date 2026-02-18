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

/// Integration test: sample.csv end-to-end conversion via convert_file.
/// Fixture contains ASCII, CJK, quoted fields, and Korean names.
#[test]
fn test_csv_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.csv", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("Alice"));
    assert!(result.markdown.contains("東京"));
    assert!(result.markdown.contains("서울"));
    assert!(result.markdown.contains("New York"));
    assert!(result.markdown.contains("다영"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_csv_golden_sample() {
    let result = convert_file("tests/fixtures/sample.csv", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.csv.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit "csv" extension.
#[test]
fn test_csv_convert_bytes_direct() {
    let input = b"X,Y\n1,2\n";
    let result = anytomd::convert_bytes(input, "csv", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("| X | Y |"));
    assert!(result.markdown.contains("| 1 | 2 |"));
}
