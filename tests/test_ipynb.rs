mod common;

use anytomd::{ConversionOptions, convert_bytes, convert_file};
use common::normalize;

/// Integration test: sample.ipynb end-to-end conversion via convert_file.
#[test]
fn test_ipynb_convert_file_sample() {
    let result =
        convert_file("tests/fixtures/sample.ipynb", &ConversionOptions::default()).unwrap();
    // Title extracted from first # heading
    assert_eq!(result.title.as_deref(), Some("Sample Notebook"));
    // Markdown cell content preserved
    assert!(result.markdown.contains("# Sample Notebook"));
    assert!(
        result
            .markdown
            .contains("sample Jupyter notebook for testing")
    );
    // Code cells wrapped in python fences
    assert!(result.markdown.contains("```python\n"));
    assert!(result.markdown.contains("def greet(name):"));
    assert!(result.markdown.contains("data = [1, 2, 3, 4, 5]"));
    // CJK and emoji preserved
    assert!(result.markdown.contains("한국어 中文 日本語"));
    assert!(result.markdown.contains("🚀"));
    // Raw cell in plain fence
    assert!(result.markdown.contains("```\nThis is raw text"));
    // Outputs not included
    assert!(!result.markdown.contains("Total: 15"));
    assert!(!result.markdown.contains("execute_result"));
    // No images or warnings for a clean notebook
    assert!(result.images.is_empty());
    assert!(result.warnings.is_empty());
}

/// Golden test: compare normalized output against expected file.
#[test]
fn test_ipynb_golden_sample() {
    let result =
        convert_file("tests/fixtures/sample.ipynb", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.ipynb.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Integration test: convert_bytes with ipynb extension.
#[test]
fn test_ipynb_convert_bytes_direct() {
    let nb = serde_json::json!({
        "nbformat": 4,
        "nbformat_minor": 2,
        "metadata": { "kernelspec": { "language": "python" } },
        "cells": [
            {
                "cell_type": "markdown",
                "metadata": {},
                "source": ["# Bytes Test"]
            },
            {
                "cell_type": "code",
                "metadata": {},
                "source": ["x = 42"]
            }
        ]
    });
    let data = serde_json::to_vec(&nb).unwrap();
    let result = convert_bytes(&data, "ipynb", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("# Bytes Test"));
    assert!(result.markdown.contains("```python\nx = 42\n```"));
    assert_eq!(result.title.as_deref(), Some("Bytes Test"));
}
