use anytomd::{ConversionOptions, convert_bytes, convert_file};

fn opts() -> ConversionOptions {
    ConversionOptions::default()
}

/// Verify plain text output from CSV contains content but no markdown table syntax.
#[test]
fn test_plain_text_csv_no_table_markers() {
    let csv_data = b"Name,Age,City\nAlice,30,Seoul\nBob,25,Tokyo";
    let result = convert_bytes(csv_data, "csv", &opts()).unwrap();
    let plain = result.plain_text();

    // Content preserved
    assert!(plain.contains("Alice"), "missing Alice");
    assert!(plain.contains("Seoul"), "missing Seoul");
    assert!(plain.contains("Bob"), "missing Bob");
    assert!(plain.contains("Tokyo"), "missing Tokyo");

    // Markdown table markers stripped
    assert!(!plain.contains("|---"), "table separator not stripped");
    assert!(!plain.contains("| "), "table pipe markers not stripped");
}

/// Verify plain text output from HTML contains content but no markdown syntax.
#[test]
fn test_plain_text_html_no_markdown_markers() {
    let html = b"<html><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>";
    let result = convert_bytes(html, "html", &opts()).unwrap();
    let plain = result.plain_text();

    assert!(plain.contains("Title"), "missing title");
    assert!(plain.contains("Hello"), "missing text");
    assert!(plain.contains("world"), "missing bold text");
    assert!(!plain.contains("# "), "heading marker not stripped");
    assert!(!plain.contains("**"), "bold marker not stripped");
}

/// Verify DOCX fixture produces plain text with content preserved.
#[test]
fn test_plain_text_docx_content_preserved() {
    let result = convert_file("tests/fixtures/sample.docx", &opts()).unwrap();
    let plain = result.plain_text();

    // Must contain some text
    assert!(!plain.trim().is_empty(), "plain text should not be empty");

    // No heading markers
    assert!(
        !plain.contains("# ") || plain.contains("C# "),
        "heading markers should be stripped (allowing C# as content)"
    );
}

/// Verify XLSX fixture produces plain text with tab-separated values.
#[test]
fn test_plain_text_xlsx_tab_separated() {
    let result = convert_file("tests/fixtures/sample.xlsx", &opts()).unwrap();
    let plain = result.plain_text();

    assert!(!plain.trim().is_empty(), "plain text should not be empty");
    assert!(!plain.contains("|---"), "table separator not stripped");
    // Tab-separated output should contain tabs
    assert!(plain.contains('\t'), "table rows should be tab-separated");
}

/// Verify CJK and emoji characters are preserved in plain text output.
#[test]
fn test_plain_text_unicode_preserved() {
    let csv_data = "Name,City\n다영,서울\n太郎,東京\n🚀,🎉".as_bytes();
    let result = convert_bytes(csv_data, "csv", &opts()).unwrap();
    let plain = result.plain_text();

    assert!(plain.contains("다영"), "Korean not preserved");
    assert!(plain.contains("서울"), "Korean city not preserved");
    assert!(plain.contains("太郎"), "Japanese not preserved");
    assert!(plain.contains("東京"), "Japanese city not preserved");
    assert!(plain.contains("🚀"), "emoji not preserved");
    assert!(plain.contains("🎉"), "emoji not preserved");
}

/// Verify JSON (fenced code block) content is preserved.
#[test]
fn test_plain_text_json_code_block_preserved() {
    let json_data = br#"{"name": "Alice", "age": 30}"#;
    let result = convert_bytes(json_data, "json", &opts()).unwrap();
    let plain = result.plain_text();

    assert!(plain.contains("\"name\""), "JSON key not preserved");
    assert!(plain.contains("\"Alice\""), "JSON value not preserved");
    // Code fences stripped
    assert!(!plain.contains("```"), "code fence not stripped");
}

/// Verify that plain text from convert_file matches plain text from convert_bytes
/// for the same CSV content.
#[test]
fn test_plain_text_file_vs_bytes_consistent() {
    let result_file = convert_file("tests/fixtures/sample.csv", &opts()).unwrap();
    let data = std::fs::read("tests/fixtures/sample.csv").unwrap();
    let result_bytes = convert_bytes(&data, "csv", &opts()).unwrap();

    assert_eq!(result_file.plain_text(), result_bytes.plain_text());
}
