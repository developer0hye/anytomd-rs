use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

pub struct XmlConverter;

/// Strip UTF-8 BOM (EF BB BF) from the beginning of the text if present.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// Pretty-print XML with 2-space indentation using quick-xml's Reader/Writer.
fn prettify_xml(input: &str) -> Result<String, ConvertError> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text_start = true;
    reader.config_mut().trim_text_end = true;

    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(event) => {
                writer.write_event(event.into_owned()).map_err(|e| {
                    ConvertError::MalformedDocument {
                        reason: format!("failed to write XML event: {e}"),
                    }
                })?;
            }
            Err(e) => {
                return Err(ConvertError::MalformedDocument {
                    reason: format!("invalid XML: {e}"),
                });
            }
        }
    }

    let output = writer.into_inner();
    String::from_utf8(output).map_err(|e| ConvertError::MalformedDocument {
        reason: format!("XML output is not valid UTF-8: {e}"),
    })
}

impl Converter for XmlConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["xml"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let text = String::from_utf8(data.to_vec())?;
        let text = strip_bom(&text);

        if text.trim().is_empty() {
            return Err(ConvertError::MalformedDocument {
                reason: "empty XML input".to_string(),
            });
        }

        let pretty = prettify_xml(text)?;
        let markdown = format!("```xml\n{pretty}\n```\n");

        Ok(ConversionResult {
            markdown,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(input: &[u8]) -> Result<ConversionResult, ConvertError> {
        XmlConverter.convert(input, &ConversionOptions::default())
    }

    #[test]
    fn test_xml_simple_element() {
        let result = convert(b"<root><child>text</child></root>").unwrap();
        assert!(result.markdown.contains("<root>"));
        assert!(result.markdown.contains("<child>text</child>"));
        assert!(result.markdown.contains("</root>"));
    }

    #[test]
    fn test_xml_pretty_printed_indentation() {
        let result = convert(b"<root><a><b>deep</b></a></root>").unwrap();
        // Extract the XML content between code fences
        let xml = result.markdown.strip_prefix("```xml\n").unwrap();
        let xml = xml.strip_suffix("\n```\n").unwrap();
        let lines: Vec<&str> = xml.lines().collect();
        // <root> at indent 0, <a> at indent 2, <b>deep</b> at indent 4
        assert!(lines.iter().any(|l| l == &"<root>"));
        assert!(lines.iter().any(|l| l == &"  <a>"));
        assert!(lines.iter().any(|l| l == &"    <b>deep</b>"));
        assert!(lines.iter().any(|l| l == &"  </a>"));
        assert!(lines.iter().any(|l| l == &"</root>"));
    }

    #[test]
    fn test_xml_with_declaration() {
        let input = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><root/>";
        let result = convert(input).unwrap();
        assert!(
            result
                .markdown
                .contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>")
        );
    }

    #[test]
    fn test_xml_with_attributes() {
        let input = b"<elem attr=\"val\" id=\"1\">content</elem>";
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("attr=\"val\""));
        assert!(result.markdown.contains("id=\"1\""));
        assert!(result.markdown.contains("content"));
    }

    #[test]
    fn test_xml_self_closing_tags() {
        let input = b"<root><br/><hr/></root>";
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("<br/>"));
        assert!(result.markdown.contains("<hr/>"));
    }

    #[test]
    fn test_xml_with_comments() {
        let input = b"<root><!-- this is a comment --><child/></root>";
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("<!-- this is a comment -->"));
    }

    #[test]
    fn test_xml_with_cdata() {
        let input = b"<root><![CDATA[some <raw> data]]></root>";
        let result = convert(input).unwrap();
        assert!(
            result.markdown.contains("some <raw> data")
                || result.markdown.contains("<![CDATA[some <raw> data]]>")
        );
    }

    #[test]
    fn test_xml_with_namespaces() {
        let input = b"<ns:root xmlns:ns=\"http://example.com\"><ns:child>text</ns:child></ns:root>";
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("ns:root"));
        assert!(result.markdown.contains("ns:child"));
        assert!(result.markdown.contains("xmlns:ns"));
    }

    #[test]
    fn test_xml_nested_elements() {
        let input = b"<a><b><c><d>deep</d></c></b></a>";
        let result = convert(input).unwrap();
        let xml = result.markdown.strip_prefix("```xml\n").unwrap();
        let xml = xml.strip_suffix("\n```\n").unwrap();
        let lines: Vec<&str> = xml.lines().collect();
        assert!(lines.iter().any(|l| l == &"      <d>deep</d>"));
    }

    #[test]
    fn test_xml_unicode_cjk() {
        let input = "<root><ko>한국어</ko><zh>中文</zh><ja>日本語</ja></root>".as_bytes();
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("中文"));
        assert!(result.markdown.contains("日本語"));
    }

    #[test]
    fn test_xml_emoji() {
        let input = "<root><emoji>🚀✨🌍</emoji></root>".as_bytes();
        let result = convert(input).unwrap();
        assert!(result.markdown.contains("🚀✨🌍"));
    }

    #[test]
    fn test_xml_empty_input() {
        let result = convert(b"");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConvertError::MalformedDocument { .. }
        ));
    }

    #[test]
    fn test_xml_whitespace_only() {
        let result = convert(b"   \n\t  ");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConvertError::MalformedDocument { .. }
        ));
    }

    #[test]
    fn test_xml_invalid_xml_returns_error() {
        // Truly malformed XML: unclosed angle bracket
        let result = convert(b"<root attr=");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConvertError::MalformedDocument { .. }
        ));
    }

    #[test]
    fn test_xml_invalid_utf8_returns_error() {
        let result = convert(&[0xFF, 0xFE]);
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_output_starts_with_code_fence() {
        let result = convert(b"<root/>").unwrap();
        assert!(result.markdown.starts_with("```xml\n"));
    }

    #[test]
    fn test_xml_output_ends_with_code_fence() {
        let result = convert(b"<root/>").unwrap();
        assert!(result.markdown.ends_with("\n```\n"));
    }

    #[test]
    fn test_xml_supported_extensions() {
        let converter = XmlConverter;
        assert!(converter.supported_extensions().contains(&"xml"));
    }

    #[test]
    fn test_xml_can_convert() {
        let converter = XmlConverter;
        assert!(converter.can_convert("xml", &[]));
        assert!(!converter.can_convert("json", &[]));
        assert!(!converter.can_convert("html", &[]));
    }

    #[test]
    fn test_xml_no_title_or_images() {
        let result = convert(b"<root/>").unwrap();
        assert!(result.title.is_none());
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_xml_utf8_bom_stripped() {
        let mut input = Vec::from(b"\xEF\xBB\xBF".as_slice());
        input.extend_from_slice(b"<root>bom</root>");
        let result = convert(&input).unwrap();
        assert!(result.markdown.contains("<root>bom</root>"));
        // BOM should not appear in the output
        assert!(!result.markdown.contains('\u{FEFF}'));
    }
}
