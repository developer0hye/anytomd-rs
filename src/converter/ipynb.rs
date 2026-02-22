use crate::converter::{
    ConversionOptions, ConversionResult, ConversionWarning, Converter, WarningCode,
};
use crate::error::ConvertError;

pub struct IpynbConverter;

impl Converter for IpynbConverter {
    fn supported_extensions(&self) -> &[&str] {
        &["ipynb"]
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let text = std::str::from_utf8(data).map_err(|e| ConvertError::MalformedDocument {
            reason: format!("invalid UTF-8: {e}"),
        })?;

        let root: serde_json::Value =
            serde_json::from_str(text).map_err(|e| ConvertError::MalformedDocument {
                reason: format!("invalid JSON: {e}"),
            })?;

        let obj = root.as_object().ok_or(ConvertError::MalformedDocument {
            reason: "notebook root is not a JSON object".to_string(),
        })?;

        let cells =
            obj.get("cells")
                .and_then(|v| v.as_array())
                .ok_or(ConvertError::MalformedDocument {
                    reason: "notebook missing \"cells\" array".to_string(),
                })?;

        // Detect language from metadata
        let language = detect_language(obj);

        let mut sections: Vec<String> = Vec::new();
        let mut title: Option<String> = None;
        let mut warnings: Vec<ConversionWarning> = Vec::new();

        for (i, cell) in cells.iter().enumerate() {
            let cell_type = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("");
            let source = join_source(cell.get("source"));

            match cell_type {
                "markdown" => {
                    if title.is_none() {
                        title = extract_heading_title(&source);
                    }
                    if !source.is_empty() {
                        sections.push(source);
                    }
                }
                "code" => {
                    if !source.is_empty() {
                        sections.push(format!("```{language}\n{source}\n```"));
                    }
                }
                "raw" => {
                    if !source.is_empty() {
                        sections.push(format!("```\n{source}\n```"));
                    }
                }
                _ => {
                    warnings.push(ConversionWarning {
                        code: WarningCode::SkippedElement,
                        message: format!("unknown cell type: \"{cell_type}\""),
                        location: Some(format!("cell {i}")),
                    });
                }
            }
        }

        // metadata.title overrides heading-extracted title (matches MarkItDown)
        if let Some(meta_title) = obj
            .get("metadata")
            .and_then(|m| m.get("title"))
            .and_then(|t| t.as_str())
            && !meta_title.is_empty()
        {
            title = Some(meta_title.to_string());
        }

        let markdown = sections.join("\n\n");

        Ok(ConversionResult {
            markdown,
            title,
            warnings,
            ..Default::default()
        })
    }
}

/// Detect the notebook language from metadata.
///
/// Priority: `metadata.kernelspec.language` > `metadata.language_info.name` > `"python"`.
fn detect_language(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(metadata) = obj.get("metadata").and_then(|v| v.as_object()) {
        // Try kernelspec.language first
        if let Some(lang) = metadata
            .get("kernelspec")
            .and_then(|k| k.get("language"))
            .and_then(|l| l.as_str())
            && !lang.is_empty()
        {
            return lang.to_string();
        }
        // Fallback to language_info.name
        if let Some(lang) = metadata
            .get("language_info")
            .and_then(|li| li.get("name"))
            .and_then(|n| n.as_str())
            && !lang.is_empty()
        {
            return lang.to_string();
        }
    }
    "python".to_string()
}

/// Join the `source` field of a cell.
///
/// `source` can be either a string or an array of strings per the nbformat spec.
fn join_source(source: Option<&serde_json::Value>) -> String {
    match source {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(""),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Extract the first `# Heading` from markdown content.
fn extract_heading_title(markdown: &str) -> Option<String> {
    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_notebook(cells: &[serde_json::Value], metadata: serde_json::Value) -> Vec<u8> {
        let nb = serde_json::json!({
            "nbformat": 4,
            "nbformat_minor": 2,
            "metadata": metadata,
            "cells": cells
        });
        serde_json::to_vec(&nb).unwrap()
    }

    fn make_cell(cell_type: &str, source: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "cell_type": cell_type,
            "metadata": {},
            "source": source
        })
    }

    fn default_metadata() -> serde_json::Value {
        serde_json::json!({
            "kernelspec": {
                "language": "python"
            }
        })
    }

    #[test]
    fn test_ipynb_markdown_cell_passthrough() {
        let cells = vec![make_cell("markdown", &["# Hello\n", "\n", "World"])];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("# Hello"));
        assert!(result.markdown.contains("World"));
    }

    #[test]
    fn test_ipynb_code_cell_fenced_block() {
        let cells = vec![make_cell("code", &["print('hello')"])];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("```python\nprint('hello')\n```"));
    }

    #[test]
    fn test_ipynb_raw_cell_fenced_block() {
        let cells = vec![make_cell("raw", &["raw content here"])];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("```\nraw content here\n```"));
    }

    #[test]
    fn test_ipynb_mixed_cells() {
        let cells = vec![
            make_cell("markdown", &["# Title"]),
            make_cell("code", &["x = 1"]),
            make_cell("raw", &["raw data"]),
            make_cell("markdown", &["## Section"]),
        ];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        let md = &result.markdown;
        assert!(md.contains("# Title"));
        assert!(md.contains("```python\nx = 1\n```"));
        assert!(md.contains("```\nraw data\n```"));
        assert!(md.contains("## Section"));
        // Verify ordering
        let title_pos = md.find("# Title").unwrap();
        let code_pos = md.find("```python").unwrap();
        let raw_pos = md.find("```\nraw data").unwrap();
        let section_pos = md.find("## Section").unwrap();
        assert!(title_pos < code_pos);
        assert!(code_pos < raw_pos);
        assert!(raw_pos < section_pos);
    }

    #[test]
    fn test_ipynb_empty_notebook() {
        let data = make_notebook(&[], default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.is_empty());
        assert!(result.title.is_none());
    }

    #[test]
    fn test_ipynb_title_from_heading() {
        let cells = vec![
            make_cell("markdown", &["Some text without heading"]),
            make_cell("markdown", &["# My Notebook Title\n", "\n", "Body"]),
        ];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.title.as_deref(), Some("My Notebook Title"));
    }

    #[test]
    fn test_ipynb_title_from_metadata() {
        let cells = vec![make_cell("markdown", &["# Heading Title"])];
        let metadata = serde_json::json!({
            "title": "Metadata Title",
            "kernelspec": { "language": "python" }
        });
        let data = make_notebook(&cells, metadata);
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        // metadata.title overrides heading
        assert_eq!(result.title.as_deref(), Some("Metadata Title"));
    }

    #[test]
    fn test_ipynb_kernel_language_detected() {
        let cells = vec![make_cell("code", &["val x = 1"])];
        let metadata = serde_json::json!({
            "kernelspec": { "language": "scala" }
        });
        let data = make_notebook(&cells, metadata);
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("```scala\n"));
    }

    #[test]
    fn test_ipynb_language_info_fallback() {
        let cells = vec![make_cell("code", &["puts 'hi'"])];
        let metadata = serde_json::json!({
            "language_info": { "name": "ruby" }
        });
        let data = make_notebook(&cells, metadata);
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("```ruby\n"));
    }

    #[test]
    fn test_ipynb_default_language_python() {
        let cells = vec![make_cell("code", &["x = 1"])];
        let metadata = serde_json::json!({});
        let data = make_notebook(&cells, metadata);
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("```python\n"));
    }

    #[test]
    fn test_ipynb_unicode_cjk() {
        let cells = vec![
            make_cell("markdown", &["# 한국어 제목"]),
            make_cell("code", &["# 中文注释\nprint('日本語')"]),
        ];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어 제목"));
        assert!(result.markdown.contains("中文注释"));
        assert!(result.markdown.contains("日本語"));
    }

    #[test]
    fn test_ipynb_emoji() {
        let cells = vec![
            make_cell("markdown", &["# Emoji Test 🚀"]),
            make_cell("code", &["x = '✨🌍'"]),
        ];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀"));
        assert!(result.markdown.contains("✨🌍"));
    }

    #[test]
    fn test_ipynb_invalid_json_returns_error() {
        let data = b"{ not valid json }";
        let result = IpynbConverter.convert(data, &ConversionOptions::default());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConvertError::MalformedDocument { .. }
        ));
    }

    #[test]
    fn test_ipynb_missing_cells_returns_error() {
        let data = br#"{"metadata": {}}"#;
        let result = IpynbConverter.convert(data, &ConversionOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("cells"), "error was: {err}");
    }

    #[test]
    fn test_ipynb_source_as_string() {
        // nbformat allows source as a single string (not array)
        let nb = serde_json::json!({
            "nbformat": 4,
            "nbformat_minor": 2,
            "metadata": { "kernelspec": { "language": "python" } },
            "cells": [{
                "cell_type": "code",
                "metadata": {},
                "source": "x = 42"
            }]
        });
        let data = serde_json::to_vec(&nb).unwrap();
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("x = 42"));
    }

    #[test]
    fn test_ipynb_supported_extensions() {
        let conv = IpynbConverter;
        assert!(conv.supported_extensions().contains(&"ipynb"));
        assert_eq!(conv.supported_extensions().len(), 1);
    }

    #[test]
    fn test_ipynb_can_convert() {
        let conv = IpynbConverter;
        assert!(conv.can_convert("ipynb", &[]));
        assert!(!conv.can_convert("json", &[]));
        assert!(!conv.can_convert("py", &[]));
    }

    #[test]
    fn test_ipynb_no_images_or_warnings() {
        let cells = vec![
            make_cell("markdown", &["# Clean"]),
            make_cell("code", &["x = 1"]),
        ];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_ipynb_outputs_ignored() {
        // Code cell with outputs — outputs should NOT appear in markdown
        let nb = serde_json::json!({
            "nbformat": 4,
            "nbformat_minor": 2,
            "metadata": { "kernelspec": { "language": "python" } },
            "cells": [{
                "cell_type": "code",
                "metadata": {},
                "source": ["print('hello')"],
                "outputs": [{
                    "output_type": "stream",
                    "name": "stdout",
                    "text": ["hello\n"]
                }]
            }]
        });
        let data = serde_json::to_vec(&nb).unwrap();
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("print('hello')"));
        // The output "hello\n" should NOT be in the markdown as standalone text
        // (it will be inside the fenced code block as part of print('hello') though)
        assert!(!result.markdown.contains("output_type"));
        assert!(!result.markdown.contains("stdout"));
    }

    #[test]
    fn test_ipynb_unknown_cell_type_warning() {
        let cells = vec![make_cell("custom_type", &["some content"])];
        let data = make_notebook(&cells, default_metadata());
        let result = IpynbConverter
            .convert(&data, &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, WarningCode::SkippedElement);
        assert!(result.warnings[0].message.contains("custom_type"));
    }
}
