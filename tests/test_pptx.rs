#![cfg(not(target_arch = "wasm32"))]

mod common;

use anytomd::{ConversionOptions, convert_file};
use common::normalize;
use std::io::Cursor;

/// Content coverage test: verify key elements are present in the converted output.
///
/// Fixture: tests/fixtures/sample.pptx
/// Contains:
/// - Slide 1: Title "Sample Presentation", body "Welcome to the presentation."
/// - Slide 2: Text "Data Overview", table (Name/Value/Status with 3 data rows),
///   speaker notes "Remember to explain the data table."
/// - Slide 3: Title "Multilingual", Korean text, emoji, speaker notes
#[test]
fn test_pptx_convert_file_sample() {
    let result = convert_file("tests/fixtures/sample.pptx", &ConversionOptions::default()).unwrap();

    // Title should be extracted from first slide's title
    assert_eq!(result.title, Some("Sample Presentation".to_string()));

    // Slide headings
    assert!(result.markdown.contains("## Slide 1: Sample Presentation"));
    assert!(result.markdown.contains("## Slide 2"));
    assert!(result.markdown.contains("## Slide 3: Multilingual"));

    // Body text
    assert!(result.markdown.contains("Welcome to the presentation."));
    assert!(result.markdown.contains("Data Overview"));

    // Table content
    assert!(result.markdown.contains("| Name | Value | Status |"));
    assert!(result.markdown.contains("| Alpha | 100 | Active |"));
    assert!(result.markdown.contains("| Beta | 200 | Inactive |"));
    assert!(result.markdown.contains("| Gamma | 300 | Active |"));

    // Speaker notes
    assert!(
        result
            .markdown
            .contains("> Note: Remember to explain the data table.")
    );
    assert!(
        result
            .markdown
            .contains("> Note: Test multilingual rendering.")
    );

    // Slide separators
    assert!(result.markdown.contains("\n\n---\n\n"));

    // Unicode: Korean
    assert!(result.markdown.contains("한국어 테스트"));

    // Emoji
    assert!(result.markdown.contains("🚀"));
    assert!(result.markdown.contains("✨"));
    assert!(result.markdown.contains("🌍"));

    // No warnings for well-formed PPTX
    assert!(result.warnings.is_empty());
}

/// Golden test: normalized comparison against expected output file.
#[test]
fn test_pptx_golden_sample() {
    let result = convert_file("tests/fixtures/sample.pptx", &ConversionOptions::default()).unwrap();
    let expected = include_str!("fixtures/expected/sample.pptx.md");
    assert_eq!(normalize(&result.markdown), normalize(expected));
}

/// Direct convert_bytes test: verify the converter works with explicit extension.
#[test]
fn test_pptx_convert_bytes_direct() {
    let data = std::fs::read("tests/fixtures/sample.pptx").unwrap();
    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();
    assert!(result.markdown.contains("## Slide 1: Sample Presentation"));
    assert!(result.markdown.contains("한국어"));
    assert!(result.markdown.contains("🚀"));
}

/// Integration test: group shapes (programmatic PPTX with p:grpSp containing shapes).
///
/// Builds a PPTX in memory with a slide containing a group shape with two text shapes
/// and verifies both texts are extracted.
#[test]
fn test_pptx_group_shape_integration() {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#,
    )
    .unwrap();

    // presentation.xml
    let pres_xml = r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#;
    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(pres_xml.as_bytes()).unwrap();

    // presentation.xml.rels
    let pres_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#;
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .unwrap();
    zip.write_all(pres_rels.as_bytes()).unwrap();

    // Slide with a title shape + a group shape containing two text shapes
    let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Group Test</a:t></a:r></a:p></p:txBody></p:sp><p:grpSp><p:nvGrpSpPr><p:cNvPr id="10" name="Group 1"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="11" name="TB1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:txBody><a:p><a:r><a:t>Grouped text A</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="12" name="TB2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:txBody><a:p><a:r><a:t>Grouped text B</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp></p:spTree></p:cSld></p:sld>"#;
    zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
    zip.write_all(slide_xml.as_bytes()).unwrap();

    let cursor = zip.finish().unwrap();
    let data = cursor.into_inner();

    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.contains("## Slide 1: Group Test"),
        "markdown was: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("Grouped text A"),
        "markdown was: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("Grouped text B"),
        "markdown was: {}",
        result.markdown
    );
}

/// Build a PPTX fixture with group shapes for testing.
///
/// Contains:
/// - Slide 1: Title "Group Shape Demo", body text, group with two text shapes (Korean)
/// - Slide 2: Title "Nested Groups", nested group with text + table inside outer group
fn build_group_shapes_pptx() -> Vec<u8> {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#,
    ).unwrap();

    let pres_xml = r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(pres_xml.as_bytes()).unwrap();

    let pres_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#;
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .unwrap();
    zip.write_all(pres_rels.as_bytes()).unwrap();

    // Slide 1: Title + body + group with two text shapes
    let slide1 = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#,
        r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
        r#"<p:cSld><p:spTree>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Group Shape Demo</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Regular body text before group.</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="10" name="Group 1"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="11" name="TB-A"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Grouped callout: Project Alpha</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="12" name="TB-B"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Grouped callout: Project Beta with 한국어</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"</p:grpSp>"#,
        r#"</p:spTree></p:cSld></p:sld>"#,
    );
    zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
    zip.write_all(slide1.as_bytes()).unwrap();

    // Slide 2: Nested group + table in group
    let slide2 = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#,
        r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
        r#"<p:cSld><p:spTree>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Nested Groups</a:t></a:r></a:p></p:txBody></p:sp>"#,
        // Outer group
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="Outer"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="21" name="OT"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Outer group text</a:t></a:r></a:p></p:txBody></p:sp>"#,
        // Inner group
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="22" name="Inner"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="23" name="IT"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Deeply nested group text 🚀</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"</p:grpSp>"#,
        // Table in outer group
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="24" name="Tbl"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>"#,
        r#"<a:graphic><a:graphicData><a:tbl>"#,
        r#"<a:tr><a:tc><a:txBody><a:p><a:r><a:t>Key</a:t></a:r></a:p></a:txBody></a:tc>"#,
        r#"<a:tc><a:txBody><a:p><a:r><a:t>Value</a:t></a:r></a:p></a:txBody></a:tc></a:tr>"#,
        r#"<a:tr><a:tc><a:txBody><a:p><a:r><a:t>Name</a:t></a:r></a:p></a:txBody></a:tc>"#,
        r#"<a:tc><a:txBody><a:p><a:r><a:t>anytomd</a:t></a:r></a:p></a:txBody></a:tc></a:tr>"#,
        r#"</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        r#"</p:grpSp>"#,
        r#"</p:spTree></p:cSld></p:sld>"#,
    );
    zip.start_file("ppt/slides/slide2.xml", opts).unwrap();
    zip.write_all(slide2.as_bytes()).unwrap();

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

/// Integration test: group shapes from a programmatic PPTX fixture.
///
/// Tests the full end-to-end pipeline via `convert_bytes` with a realistic PPTX
/// containing title shapes, body text, group shapes with multiple children, nested
/// groups, a table inside a group, Korean text, and emoji.
#[test]
fn test_pptx_group_shape_convert_file() {
    let data = build_group_shapes_pptx();
    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();

    // Slide 1
    assert!(
        result.markdown.contains("## Slide 1: Group Shape Demo"),
        "markdown was: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("Regular body text before group."),
        "markdown was: {}",
        result.markdown
    );
    // Group shape child texts
    assert!(
        result.markdown.contains("Grouped callout: Project Alpha"),
        "markdown was: {}",
        result.markdown
    );
    assert!(
        result
            .markdown
            .contains("Grouped callout: Project Beta with 한국어"),
        "markdown was: {}",
        result.markdown
    );

    // Slide 2
    assert!(
        result.markdown.contains("## Slide 2: Nested Groups"),
        "markdown was: {}",
        result.markdown
    );
    // Outer group text
    assert!(
        result.markdown.contains("Outer group text"),
        "markdown was: {}",
        result.markdown
    );
    // Nested group text with emoji
    assert!(
        result.markdown.contains("Deeply nested group text 🚀"),
        "markdown was: {}",
        result.markdown
    );
    // Table inside group
    assert!(
        result.markdown.contains("| Key | Value |"),
        "table in group missing, markdown was: {}",
        result.markdown
    );
    assert!(
        result.markdown.contains("| Name | anytomd |"),
        "table data in group missing, markdown was: {}",
        result.markdown
    );

    // Title from first slide
    assert_eq!(result.title, Some("Group Shape Demo".to_string()),);
}

/// Build a single-slide PPTX whose table uses a horizontal merge.
fn build_merged_table_pptx() -> Vec<u8> {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#,
    ).unwrap();

    let pres_xml = r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#;
    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(pres_xml.as_bytes()).unwrap();

    let pres_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#;
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .unwrap();
    zip.write_all(pres_rels.as_bytes()).unwrap();

    // Header row uses a 2-column horizontal merge (gridSpan + hMerge placeholder);
    // the data row has two ordinary cells.
    let slide1 = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#,
        r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
        r#"<p:cSld><p:spTree>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>"#,
        r#"<p:txBody><a:p><a:r><a:t>Merged Table</a:t></a:r></a:p></p:txBody></p:sp>"#,
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="3" name="Tbl"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>"#,
        r#"<a:graphic><a:graphicData><a:tbl>"#,
        r#"<a:tr><a:tc gridSpan="2"><a:txBody><a:p><a:r><a:t>Wide Header</a:t></a:r></a:p></a:txBody></a:tc>"#,
        r#"<a:tc hMerge="1"><a:txBody><a:p><a:r><a:t>Wide Header</a:t></a:r></a:p></a:txBody></a:tc></a:tr>"#,
        r#"<a:tr><a:tc><a:txBody><a:p><a:r><a:t>L</a:t></a:r></a:p></a:txBody></a:tc>"#,
        r#"<a:tc><a:txBody><a:p><a:r><a:t>R</a:t></a:r></a:p></a:txBody></a:tc></a:tr>"#,
        r#"</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        r#"</p:spTree></p:cSld></p:sld>"#,
    );
    zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
    zip.write_all(slide1.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

/// Integration test: a PPTX table with a horizontal merge keeps all columns and
/// does not duplicate the spanning cell's text into the merged placeholder.
#[test]
fn test_pptx_merged_table_columns() {
    let data = build_merged_table_pptx();
    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();

    // Header row: spanning text in the first column, empty placeholder in the second.
    assert!(
        result.markdown.contains("| Wide Header |  |"),
        "merged header columns wrong: {}",
        result.markdown
    );
    // Data row keeps both columns.
    assert!(
        result.markdown.contains("| L | R |"),
        "data columns missing: {}",
        result.markdown
    );
    // The spanning text must appear exactly once (not duplicated into the placeholder).
    assert_eq!(
        result.markdown.matches("Wide Header").count(),
        1,
        "spanning text duplicated: {}",
        result.markdown
    );
}

/// End-to-end PPTX comment extraction through the public `convert_bytes` API.
///
/// Builds a one-slide presentation with a legacy comment part and verifies the
/// appended `# Comments` section uses the slide label as `source`.
#[test]
fn test_pptx_extract_comments_end_to_end() {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#).unwrap();

    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#).unwrap();
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .unwrap();
    zip.write_all(br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#).unwrap();

    zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Roadmap</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#).unwrap();

    zip.start_file("ppt/slides/_rels/slide1.xml.rels", opts)
        .unwrap();
    zip.write_all(br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdC" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments/comment1.xml"/></Relationships>"#).unwrap();

    zip.start_file("ppt/commentAuthors.xml", opts).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cmAuthor id="0" name="Dana" initials="D"/></p:cmAuthorLst>"#).unwrap();
    zip.start_file("ppt/comments/comment1.xml", opts).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cm authorId="0" dt="2024-02-02T08:00:00Z" idx="1"><p:pos x="100" y="100"/><p:text>Reorder these milestones.</p:text></p:cm></p:cmLst>"#).unwrap();

    let data = zip.finish().unwrap().into_inner();

    // Without the flag: no Comments section.
    let plain = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();
    assert!(!plain.markdown.contains("# Comments"));

    let options = ConversionOptions {
        extract_comments: true,
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&data, "pptx", &options).unwrap();
    assert!(
        result.markdown.contains("# Comments"),
        "md: {}",
        result.markdown
    );
    assert!(
        result
            .markdown
            .contains("- **author**: Dana (2024-02-02T08:00:00Z)")
    );
    assert!(
        result
            .markdown
            .contains("- **comment**: Reorder these milestones.")
    );
    assert!(result.markdown.contains("- **source**: Slide 1: Roadmap"));
    // Slide content precedes the appended section.
    let slide = result.markdown.find("## Slide 1").unwrap();
    let section = result.markdown.find("# Comments").unwrap();
    assert!(slide < section);
}
