//! Integration tests for the ImageDescriber trait and its effect on DOCX/PPTX/XLSX/standalone image conversion.

use std::io::{Cursor, Write};
use std::sync::Arc;

use anytomd::{ConversionOptions, ConvertError, ImageDescriber};

/// A mock describer that returns a fixed description for any image.
struct MockDescriber {
    description: String,
}

impl ImageDescriber for MockDescriber {
    fn describe(
        &self,
        _image_bytes: &[u8],
        _mime_type: &str,
        _prompt: &str,
    ) -> Result<String, ConvertError> {
        Ok(self.description.clone())
    }
}

/// Build a minimal DOCX ZIP with an embedded image for integration tests.
fn build_docx_with_image() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    // [Content_Types].xml
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(ct.as_bytes()).unwrap();

    // _rels/.rels
    zip.start_file("_rels/.rels", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
    )
    .unwrap();

    // word/document.xml — contains an image reference with empty alt text
    let doc_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body><w:p><w:r><w:t>Before image.</w:t></w:r></w:p><w:p><w:r><w:drawing><wp:inline><wp:docPr descr=""/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p><w:p><w:r><w:t>After image.</w:t></w:r></w:p></w:body></w:document>"#;
    zip.start_file("word/document.xml", opts).unwrap();
    zip.write_all(doc_xml.as_bytes()).unwrap();

    // word/_rels/document.xml.rels
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
    zip.start_file("word/_rels/document.xml.rels", opts)
        .unwrap();
    zip.write_all(rels.as_bytes()).unwrap();

    // word/media/image1.png — fake image data
    zip.start_file("word/media/image1.png", opts).unwrap();
    zip.write_all(b"fake-png-data-for-integration-test")
        .unwrap();

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

/// Build a minimal PPTX ZIP with an embedded image for integration tests.
fn build_pptx_with_image() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    // [Content_Types].xml
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/></Types>"#;
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(ct.as_bytes()).unwrap();

    // ppt/presentation.xml
    let pres = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#;
    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(pres.as_bytes()).unwrap();

    // ppt/_rels/presentation.xml.rels
    let pres_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#;
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .unwrap();
    zip.write_all(pres_rels.as_bytes()).unwrap();

    // ppt/slides/slide1.xml — one image shape
    let slide = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Image Slide</a:t></a:r></a:p></p:txBody></p:sp><p:pic><p:nvPicPr><p:cNvPr id="10" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg1"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#;
    zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
    zip.write_all(slide.as_bytes()).unwrap();

    // ppt/slides/_rels/slide1.xml.rels
    let slide_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdImg1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#;
    zip.start_file("ppt/slides/_rels/slide1.xml.rels", opts)
        .unwrap();
    zip.write_all(slide_rels.as_bytes()).unwrap();

    // ppt/media/image1.png — fake image data
    zip.start_file("ppt/media/image1.png", opts).unwrap();
    zip.write_all(b"fake-png-data-for-pptx-test").unwrap();

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

#[test]
fn test_docx_with_mock_describer_replaces_alt_text() {
    let data = build_docx_with_image();
    let options = ConversionOptions {
        image_describer: Some(Arc::new(MockDescriber {
            description: "A chart showing quarterly revenue growth".to_string(),
        })),
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&data, "docx", &options).unwrap();
    assert!(
        result
            .markdown
            .contains("![A chart showing quarterly revenue growth](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
    assert!(result.markdown.contains("Before image."));
    assert!(result.markdown.contains("After image."));
}

#[test]
fn test_docx_without_describer_has_empty_alt() {
    let data = build_docx_with_image();
    let result = anytomd::convert_bytes(&data, "docx", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.contains("![](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
}

#[test]
fn test_pptx_with_mock_describer_replaces_alt_text() {
    let data = build_pptx_with_image();
    let options = ConversionOptions {
        image_describer: Some(Arc::new(MockDescriber {
            description: "A diagram of the system architecture".to_string(),
        })),
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&data, "pptx", &options).unwrap();
    assert!(
        result
            .markdown
            .contains("![A diagram of the system architecture](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
    assert!(result.markdown.contains("Image Slide"));
}

#[test]
fn test_pptx_without_describer_has_empty_alt() {
    let data = build_pptx_with_image();
    let result = anytomd::convert_bytes(&data, "pptx", &ConversionOptions::default()).unwrap();
    assert!(
        result.markdown.contains("![](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
}

// ---- XLSX image describer integration tests ----

/// Build a minimal XLSX ZIP with an embedded image for integration tests.
///
/// Contains a single sheet "Sheet1" with one data row and an image (image1.png)
/// embedded via the drawing relationship chain.
fn build_xlsx_with_image() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default();

    // [Content_Types].xml
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(ct.as_bytes()).unwrap();

    // _rels/.rels
    zip.start_file("_rels/.rels", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
    )
    .unwrap();

    // xl/workbook.xml
    zip.start_file("xl/workbook.xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
    )
    .unwrap();

    // xl/_rels/workbook.xml.rels
    zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
    )
    .unwrap();

    // xl/worksheets/sheet1.xml — one row of data
    zip.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Alice</t></is></c></row></sheetData></worksheet>"#,
    )
    .unwrap();

    // xl/worksheets/_rels/sheet1.xml.rels — points to drawing
    zip.start_file("xl/worksheets/_rels/sheet1.xml.rels", opts)
        .unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#,
    )
    .unwrap();

    // xl/drawings/drawing1.xml — one blip
    zip.start_file("xl/drawings/drawing1.xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:twoCellAnchor><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="1" name="Picture 1"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic></xdr:twoCellAnchor></xdr:wsDr>"#,
    )
    .unwrap();

    // xl/drawings/_rels/drawing1.xml.rels — resolve to media
    zip.start_file("xl/drawings/_rels/drawing1.xml.rels", opts)
        .unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#,
    )
    .unwrap();

    // xl/media/image1.png — fake image data
    zip.start_file("xl/media/image1.png", opts).unwrap();
    zip.write_all(b"fake-png-data-for-xlsx-integration-test")
        .unwrap();

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

#[test]
fn test_xlsx_with_mock_describer_replaces_alt_text() {
    let data = build_xlsx_with_image();
    let options = ConversionOptions {
        image_describer: Some(Arc::new(MockDescriber {
            description: "A pie chart showing quarterly revenue".to_string(),
        })),
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&data, "xlsx", &options).unwrap();
    assert!(
        result
            .markdown
            .contains("![A pie chart showing quarterly revenue](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
    // Should still have the table data
    assert!(result.markdown.contains("Name"));
    assert!(result.markdown.contains("Alice"));
}

#[test]
fn test_xlsx_without_describer_has_empty_alt() {
    let data = build_xlsx_with_image();
    let options = ConversionOptions {
        extract_images: true,
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&data, "xlsx", &options).unwrap();
    assert!(
        result.markdown.contains("![](image1.png)"),
        "markdown was: {}",
        result.markdown
    );
}

// ---- Standalone image file integration tests ----

/// Minimal PNG file header for standalone image tests.
const PNG_HEADER: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[test]
fn test_image_file_with_mock_describer() {
    let options = ConversionOptions {
        image_describer: Some(Arc::new(MockDescriber {
            description: "A photo of a cat sitting on a windowsill".to_string(),
        })),
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&PNG_HEADER, "png", &options).unwrap();
    assert_eq!(
        result.markdown, "![A photo of a cat sitting on a windowsill](image.png)\n",
        "markdown was: {}",
        result.markdown
    );
}

#[test]
fn test_image_file_without_describer() {
    let result = anytomd::convert_bytes(&PNG_HEADER, "png", &ConversionOptions::default()).unwrap();
    assert_eq!(
        result.markdown, "![](image.png)\n",
        "markdown was: {}",
        result.markdown
    );
}

#[test]
fn test_image_file_with_extract_images() {
    let options = ConversionOptions {
        extract_images: true,
        ..Default::default()
    };
    let result = anytomd::convert_bytes(&PNG_HEADER, "png", &options).unwrap();
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].0, "image.png");
    assert_eq!(result.images[0].1, PNG_HEADER.to_vec());
}

#[test]
fn test_image_file_via_image_extension() {
    // The "image" generic extension (from detection.rs) should also work
    let result =
        anytomd::convert_bytes(&PNG_HEADER, "image", &ConversionOptions::default()).unwrap();
    assert_eq!(
        result.markdown, "![](image.png)\n",
        "markdown was: {}",
        result.markdown
    );
}

#[test]
fn test_image_file_jpeg_via_convert_bytes() {
    let jpeg_data: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
    let result = anytomd::convert_bytes(&jpeg_data, "jpg", &ConversionOptions::default()).unwrap();
    assert_eq!(
        result.markdown, "![](image.jpg)\n",
        "markdown was: {}",
        result.markdown
    );
}
