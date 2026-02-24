#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;

use anytomd::{ConversionOptions, convert_bytes};

// ---- convert_bytes tests (core library on WASM) ----

#[wasm_bindgen_test]
fn test_convert_csv_wasm() {
    let csv_data = b"Name,Age,City\nAlice,30,Seoul\nBob,25,Tokyo";
    let options = ConversionOptions::default();
    let result = convert_bytes(csv_data, "csv", &options).unwrap();
    assert!(result.markdown.contains("| Name | Age | City |"));
    assert!(result.markdown.contains("| Alice | 30 | Seoul |"));
    assert!(result.markdown.contains("| Bob | 25 | Tokyo |"));
}

#[wasm_bindgen_test]
fn test_convert_html_wasm() {
    let html_data = b"<html><body><h1>Hello</h1><p>World</p></body></html>";
    let options = ConversionOptions::default();
    let result = convert_bytes(html_data, "html", &options).unwrap();
    assert!(result.markdown.contains("# Hello"));
    assert!(result.markdown.contains("World"));
}

#[wasm_bindgen_test]
fn test_convert_json_wasm() {
    let json_data = br#"{"key": "value", "number": 42}"#;
    let options = ConversionOptions::default();
    let result = convert_bytes(json_data, "json", &options).unwrap();
    assert!(result.markdown.contains("key"));
    assert!(result.markdown.contains("value"));
    assert!(result.markdown.contains("42"));
}

#[wasm_bindgen_test]
fn test_convert_xml_wasm() {
    let xml_data = b"<root><item>Hello</item></root>";
    let options = ConversionOptions::default();
    let result = convert_bytes(xml_data, "xml", &options).unwrap();
    assert!(result.markdown.contains("Hello"));
}

#[wasm_bindgen_test]
fn test_convert_plain_text_wasm() {
    let text_data = b"Hello, WASM world!";
    let options = ConversionOptions::default();
    let result = convert_bytes(text_data, "txt", &options).unwrap();
    assert!(result.markdown.contains("Hello, WASM world!"));
    assert!(result.plain_text.contains("Hello, WASM world!"));
}

#[wasm_bindgen_test]
fn test_convert_code_wasm() {
    let code_data = b"fn main() {\n    println!(\"Hello\");\n}";
    let options = ConversionOptions::default();
    let result = convert_bytes(code_data, "rs", &options).unwrap();
    assert!(result.markdown.contains("```rust"));
    assert!(result.markdown.contains("fn main()"));
}

#[wasm_bindgen_test]
fn test_convert_docx_wasm() {
    // Embed the sample DOCX fixture at compile time (no filesystem at WASM runtime)
    let docx_bytes = include_bytes!("fixtures/sample.docx");
    let options = ConversionOptions::default();
    let result = convert_bytes(docx_bytes, "docx", &options).unwrap();
    assert!(
        result.markdown.contains("Sample Document"),
        "DOCX should contain heading: {}",
        &result.markdown[..result.markdown.len().min(200)]
    );
}

#[wasm_bindgen_test]
fn test_convert_pptx_wasm() {
    let pptx_bytes = include_bytes!("fixtures/sample.pptx");
    let options = ConversionOptions::default();
    let result = convert_bytes(pptx_bytes, "pptx", &options).unwrap();
    assert!(
        !result.markdown.is_empty(),
        "PPTX conversion should produce output"
    );
}

#[wasm_bindgen_test]
fn test_convert_xlsx_wasm() {
    let xlsx_bytes = include_bytes!("fixtures/sample.xlsx");
    let options = ConversionOptions::default();
    let result = convert_bytes(xlsx_bytes, "xlsx", &options).unwrap();
    assert!(
        !result.markdown.is_empty(),
        "XLSX conversion should produce output"
    );
}

#[wasm_bindgen_test]
fn test_convert_unsupported_format_wasm() {
    let data = b"some data";
    let options = ConversionOptions::default();
    let result = convert_bytes(data, "xyz_unsupported", &options);
    assert!(result.is_err(), "unsupported format should return an error");
}

#[wasm_bindgen_test]
fn test_convert_unicode_wasm() {
    let csv_data =
        "Name,City\n\u{B2E4}\u{C601},\u{C11C}\u{C6B8}\n\u{592A}\u{90CE},\u{6771}\u{4EAC}"
            .as_bytes();
    let options = ConversionOptions::default();
    let result = convert_bytes(csv_data, "csv", &options).unwrap();
    assert!(result.markdown.contains("\u{B2E4}\u{C601}"));
    assert!(result.markdown.contains("\u{C11C}\u{C6B8}"));
    assert!(result.markdown.contains("\u{592A}\u{90CE}"));
    assert!(result.markdown.contains("\u{6771}\u{4EAC}"));
}
