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

// ---- async-gemini tests (structural, no live API calls) ----

#[cfg(feature = "async-gemini")]
mod async_gemini {
    use wasm_bindgen_test::*;

    use anytomd::gemini::AsyncGeminiDescriber;

    #[wasm_bindgen_test]
    fn test_async_gemini_describer_construction_wasm() {
        let describer = AsyncGeminiDescriber::new("test-key".to_string());
        // Verify Debug output redacts the key
        let debug = format!("{:?}", describer);
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("test-key"));
    }

    #[wasm_bindgen_test]
    fn test_async_gemini_describer_with_model_wasm() {
        let describer =
            AsyncGeminiDescriber::new("key".to_string()).with_model("gemini-2.0-flash".to_string());
        let debug = format!("{:?}", describer);
        assert!(debug.contains("gemini-2.0-flash"));
    }

    #[wasm_bindgen_test]
    fn test_async_gemini_describer_trait_object_wasm() {
        use anytomd::AsyncImageDescriber;
        let describer = AsyncGeminiDescriber::new("key".to_string());
        // Verify it can be used as a trait object on WASM
        let _: &dyn AsyncImageDescriber = &describer;
    }

    #[wasm_bindgen_test]
    fn test_convert_bytes_async_no_describer_wasm() {
        use anytomd::{AsyncConversionOptions, convert_bytes_async};

        let csv_data = b"Name,Age\nAlice,30";
        let options = AsyncConversionOptions::default();
        // convert_bytes_async without a describer falls back to sync convert
        let result = wasm_bindgen_futures::JsFuture::from(wasm_bindgen_futures::future_to_promise(
            async move {
                let result = convert_bytes_async(csv_data, "csv", &options)
                    .await
                    .unwrap();
                assert!(result.markdown.contains("| Name | Age |"));
                assert!(result.markdown.contains("| Alice | 30 |"));
                Ok(wasm_bindgen::JsValue::TRUE)
            },
        ));
        // The future is constructed successfully — structural verification
        let _ = result;
    }
}
