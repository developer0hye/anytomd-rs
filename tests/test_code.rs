mod common;

use anytomd::{ConversionOptions, convert_bytes, convert_file};
use common::normalize;

/// Integration test: sample.py end-to-end conversion via convert_file.
#[test]
fn test_code_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.py", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.starts_with("```python\n"));
    assert!(result.markdown.ends_with("\n```\n"));
    assert!(result.markdown.contains("class Greeter:"));
    assert!(result.markdown.contains("def add("));
    assert!(result.markdown.contains("한국어"));
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("世界"));
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_code_golden_sample() {
    let result = convert_file("tests/fixtures/sample.py", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.py.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with explicit extension.
#[test]
fn test_code_convert_bytes_direct() {
    let input = b"fn main() {\n    println!(\"hello\");\n}\n";
    let result = convert_bytes(input, "rs", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.starts_with("```rust\n"));
    assert!(result.markdown.ends_with("\n```\n"));
    assert!(result.markdown.contains("fn main()"));
}
