use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::converter::{
    ConversionWarning, ImageDescriber, WarningCode, replace_image_alt_by_placeholder,
};

/// A resolved relationship entry from a .rels file.
#[derive(Debug, Clone)]
pub(crate) struct Relationship {
    pub(crate) target: String,
    pub(crate) rel_type: String,
}

/// Information about an image found during conversion.
#[derive(Debug, Clone)]
pub(crate) struct ImageInfo {
    pub(crate) placeholder: String,
    pub(crate) original_alt: String,
    pub(crate) filename: String,
    /// Lookup key for image bytes in `PendingImageResolution::bytes`.
    /// This can differ from `filename` when multiple images share a basename.
    pub(crate) bytes_key: String,
}

/// Collected image data from a converter's parse phase, ready for resolution.
///
/// Converters populate this during `convert_inner()` so that image description
/// (sync or async) can be performed separately from document parsing.
#[derive(Debug, Clone, Default)]
pub(crate) struct PendingImageResolution {
    pub(crate) infos: Vec<ImageInfo>,
    /// Raw image bytes keyed by `ImageInfo::bytes_key`.
    pub(crate) bytes: HashMap<String, Vec<u8>>,
}

/// Parse a .rels XML file to extract relationship ID -> Relationship mapping.
pub(crate) fn parse_relationships(xml: &str) -> HashMap<String, Relationship> {
    let mut rels = HashMap::new();
    let mut reader = Reader::from_str(xml);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                let local_str = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if local_str == "Relationship" {
                    let mut id = None;
                    let mut target = None;
                    let mut rel_type = String::new();

                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        let val = attr
                            .decode_and_unescape_value(reader.decoder())
                            .map(|v| v.into_owned())
                            .unwrap_or_else(|_| {
                                String::from_utf8_lossy(attr.value.as_ref()).to_string()
                            });
                        match key {
                            "Id" => id = Some(val),
                            "Target" => target = Some(val),
                            "Type" => rel_type = val,
                            _ => {}
                        }
                    }

                    if let (Some(id), Some(target)) = (id, target) {
                        rels.insert(id, Relationship { target, rel_type });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    rels
}

/// Derive the .rels path for a given file path.
///
/// Example: `ppt/slides/slide1.xml` -> `ppt/slides/_rels/slide1.xml.rels`
pub(crate) fn derive_rels_path(file_path: &str) -> String {
    if let Some(pos) = file_path.rfind('/') {
        let dir = &file_path[..pos];
        let filename = &file_path[pos + 1..];
        format!("{dir}/_rels/{filename}.rels")
    } else {
        format!("_rels/{file_path}.rels")
    }
}

/// Resolve a relative path target against a base directory path.
///
/// Example: base_dir=`xl/drawings`, target=`../media/image1.png`
///          -> `xl/media/image1.png`
pub(crate) fn resolve_relative_path(base_dir: &str, target: &str) -> String {
    let joined = if target.starts_with('/') {
        target.to_string()
    } else if base_dir.is_empty() {
        target.to_string()
    } else {
        format!("{base_dir}/{target}")
    };
    normalize_package_path(&joined)
}

/// Resolve a relative path target against a base file path.
///
/// Strips the filename from `base_file` to get the directory, then delegates
/// to `resolve_relative_path`.
///
/// Example: base_file=`ppt/slides/slide1.xml`, target=`../media/image1.png`
///          -> `ppt/media/image1.png`
pub(crate) fn resolve_relative_to_file(base_file: &str, target: &str) -> String {
    let base_dir = base_file
        .rfind('/')
        .map(|pos| &base_file[..pos])
        .unwrap_or("");
    let joined = if target.starts_with('/') {
        target.to_string()
    } else if base_dir.is_empty() {
        target.to_string()
    } else {
        format!("{base_dir}/{target}")
    };
    normalize_package_path(&joined)
}

fn normalize_package_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = out.pop();
            continue;
        }
        out.push(part);
    }
    out.join("/")
}

/// Replace image placeholders in markdown and plain text with descriptions
/// from the describer, or fall back to the original alt text.
pub(crate) fn resolve_image_placeholders(
    markdown: &mut String,
    plain_text: &mut String,
    image_infos: &[ImageInfo],
    image_bytes: &HashMap<String, Vec<u8>>,
    describer: Option<&dyn ImageDescriber>,
    warnings: &mut Vec<ConversionWarning>,
) {
    if let Some(describer) = describer {
        for info in image_infos {
            if let Some(img_data) = image_bytes
                .get(&info.bytes_key)
                .or_else(|| image_bytes.get(&info.filename))
            {
                let mime = crate::converter::mime_from_image(&info.filename, img_data);
                let prompt = "Describe this image concisely for use as alt text.";
                match describer.describe(img_data, mime, prompt) {
                    Ok(description) => {
                        *markdown = replace_image_alt_by_placeholder(
                            markdown,
                            &info.placeholder,
                            &description,
                            &info.filename,
                        );
                        *plain_text = plain_text.replacen(&info.placeholder, &description, 1);
                    }
                    Err(e) => {
                        *markdown = replace_image_alt_by_placeholder(
                            markdown,
                            &info.placeholder,
                            &info.original_alt,
                            &info.filename,
                        );
                        *plain_text = plain_text.replacen(&info.placeholder, &info.original_alt, 1);
                        warnings.push(ConversionWarning {
                            code: WarningCode::SkippedElement,
                            message: format!(
                                "image description failed for '{}': {}",
                                info.filename, e
                            ),
                            location: Some(info.filename.clone()),
                        });
                    }
                }
            } else {
                *markdown = replace_image_alt_by_placeholder(
                    markdown,
                    &info.placeholder,
                    &info.original_alt,
                    &info.filename,
                );
                *plain_text = plain_text.replacen(&info.placeholder, &info.original_alt, 1);
            }
        }
    } else {
        for info in image_infos {
            *markdown = replace_image_alt_by_placeholder(
                markdown,
                &info.placeholder,
                &info.original_alt,
                &info.filename,
            );
            *plain_text = plain_text.replacen(&info.placeholder, &info.original_alt, 1);
        }
    }
}

/// Async version of [`resolve_image_placeholders`].
///
/// Describes all images concurrently using `futures_util::future::join_all`,
/// then replaces placeholders in the markdown. Falls back to original alt text
/// on error, just like the sync version.
#[cfg(feature = "async")]
pub(crate) async fn resolve_image_placeholders_async(
    markdown: &mut String,
    plain_text: &mut String,
    image_infos: &[ImageInfo],
    image_bytes: &HashMap<String, Vec<u8>>,
    describer: &dyn crate::converter::AsyncImageDescriber,
    warnings: &mut Vec<ConversionWarning>,
) {
    use futures_util::future::join_all;

    let prompt = "Describe this image concisely for use as alt text.";

    // Build futures for all images that have bytes available
    let futures: Vec<_> = image_infos
        .iter()
        .map(|info| {
            let bytes_opt = image_bytes
                .get(&info.bytes_key)
                .or_else(|| image_bytes.get(&info.filename));
            async move {
                if let Some(img_data) = bytes_opt {
                    let mime = crate::converter::mime_from_image(&info.filename, img_data);
                    match describer.describe(img_data, mime, prompt).await {
                        Ok(description) => (info, Some(description), None),
                        Err(e) => (info, None, Some(e)),
                    }
                } else {
                    (info, None, None)
                }
            }
        })
        .collect();

    let results = join_all(futures).await;

    for (info, description, error) in results {
        if let Some(desc) = description {
            *markdown = replace_image_alt_by_placeholder(
                markdown,
                &info.placeholder,
                &desc,
                &info.filename,
            );
            *plain_text = plain_text.replacen(&info.placeholder, &desc, 1);
        } else {
            *markdown = replace_image_alt_by_placeholder(
                markdown,
                &info.placeholder,
                &info.original_alt,
                &info.filename,
            );
            *plain_text = plain_text.replacen(&info.placeholder, &info.original_alt, 1);
            if let Some(e) = error {
                warnings.push(ConversionWarning {
                    code: WarningCode::SkippedElement,
                    message: format!("image description failed for '{}': {}", info.filename, e),
                    location: Some(info.filename.clone()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_relationships_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let rels = parse_relationships(xml);
        assert_eq!(rels.len(), 2);

        let r1 = rels.get("rId1").unwrap();
        assert_eq!(r1.target, "media/image1.png");
        assert!(r1.rel_type.contains("image"));

        let r2 = rels.get("rId2").unwrap();
        assert_eq!(r2.target, "https://example.com");
        assert!(r2.rel_type.contains("hyperlink"));
    }

    #[test]
    fn test_parse_relationships_unescapes_target_entities() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com?a=1&amp;b=2" TargetMode="External"/></Relationships>"#;
        let rels = parse_relationships(xml);
        let r1 = rels.get("rId1").expect("missing rId1");
        assert_eq!(r1.target, "https://example.com?a=1&b=2");
    }

    #[test]
    fn test_parse_relationships_empty() {
        let xml = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#;
        let rels = parse_relationships(xml);
        assert!(rels.is_empty());
    }

    #[test]
    fn test_parse_relationships_missing_id() {
        let xml = r#"<Relationships><Relationship Type="foo" Target="bar"/></Relationships>"#;
        let rels = parse_relationships(xml);
        assert!(rels.is_empty());
    }

    #[test]
    fn test_parse_relationships_missing_target() {
        let xml = r#"<Relationships><Relationship Id="rId1" Type="foo"/></Relationships>"#;
        let rels = parse_relationships(xml);
        assert!(rels.is_empty());
    }

    #[test]
    fn test_derive_rels_path_with_directory() {
        assert_eq!(
            derive_rels_path("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
        assert_eq!(
            derive_rels_path("xl/drawings/drawing1.xml"),
            "xl/drawings/_rels/drawing1.xml.rels"
        );
    }

    #[test]
    fn test_derive_rels_path_no_directory() {
        assert_eq!(derive_rels_path("file.xml"), "_rels/file.xml.rels");
    }

    #[test]
    fn test_resolve_relative_path_same_dir() {
        assert_eq!(
            resolve_relative_path("xl/drawings", "image1.png"),
            "xl/drawings/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_parent_dir() {
        assert_eq!(
            resolve_relative_path("xl/drawings", "../media/image1.png"),
            "xl/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_current_dir_segment() {
        assert_eq!(
            resolve_relative_path("xl/drawings", "./media/image1.png"),
            "xl/drawings/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_empty_base() {
        assert_eq!(resolve_relative_path("", "image1.png"), "image1.png");
    }

    #[test]
    fn test_resolve_relative_to_file_same_dir() {
        assert_eq!(
            resolve_relative_to_file("ppt/slides/slide1.xml", "image1.png"),
            "ppt/slides/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_to_file_parent_dir() {
        assert_eq!(
            resolve_relative_to_file("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_to_file_current_dir_segment() {
        assert_eq!(
            resolve_relative_to_file("word/document.xml", "./media/image1.png"),
            "word/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_to_file_no_dir() {
        assert_eq!(
            resolve_relative_to_file("slide.xml", "image1.png"),
            "image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_to_file_absolute_path() {
        assert_eq!(
            resolve_relative_to_file("ppt/slides/slide1.xml", "/ppt/media/image1.png"),
            "ppt/media/image1.png"
        );
    }

    #[test]
    fn test_resolve_relative_path_excessive_parent_stops_at_root() {
        // base "a" has 1 segment. "../../etc/passwd" tries 2 levels up.
        // After reaching root, extra "../" are clamped.
        assert_eq!(resolve_relative_path("a", "../../etc/passwd"), "etc/passwd");
    }

    #[test]
    fn test_resolve_relative_to_file_excessive_parent_stops_at_root() {
        // base_file "a/b.xml" → directory segments: ["a"].
        // After reaching root, extra "../" are clamped.
        assert_eq!(
            resolve_relative_to_file("a/b.xml", "../../etc/passwd"),
            "etc/passwd"
        );
    }

    #[test]
    fn test_resolve_relative_path_exact_parent_count() {
        // Exactly matching "../" count should resolve cleanly
        assert_eq!(
            resolve_relative_path("a/b", "../media/image.png"),
            "a/media/image.png"
        );
    }

    #[test]
    fn test_resolve_image_placeholders_no_describer() {
        let mut md = "![__img_0__](cat.png)\n![__img_1__](dog.png)".to_string();
        let mut pt = "__img_0__\n__img_1__".to_string();
        let infos = vec![
            ImageInfo {
                placeholder: "__img_0__".to_string(),
                original_alt: "A cat".to_string(),
                filename: "cat.png".to_string(),
                bytes_key: "__img_0__".to_string(),
            },
            ImageInfo {
                placeholder: "__img_1__".to_string(),
                original_alt: "A dog".to_string(),
                filename: "dog.png".to_string(),
                bytes_key: "__img_1__".to_string(),
            },
        ];
        let image_bytes = HashMap::new();
        let mut warnings = Vec::new();
        resolve_image_placeholders(&mut md, &mut pt, &infos, &image_bytes, None, &mut warnings);
        assert!(md.contains("![A cat](cat.png)"));
        assert!(md.contains("![A dog](dog.png)"));
        assert!(pt.contains("A cat"));
        assert!(pt.contains("A dog"));
        assert!(!pt.contains("__img_"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_resolve_image_placeholders_with_describer() {
        use crate::converter::ImageDescriber;
        use crate::error::ConvertError;

        struct MockDescriber;
        impl ImageDescriber for MockDescriber {
            fn describe(
                &self,
                _image_bytes: &[u8],
                _mime_type: &str,
                _prompt: &str,
            ) -> Result<String, ConvertError> {
                Ok("LLM description".to_string())
            }
        }

        let mut md = "![__img_0__](cat.png)".to_string();
        let mut pt = "__img_0__".to_string();
        let infos = vec![ImageInfo {
            placeholder: "__img_0__".to_string(),
            original_alt: "A cat".to_string(),
            filename: "cat.png".to_string(),
            bytes_key: "__img_0__".to_string(),
        }];
        let mut image_bytes = HashMap::new();
        image_bytes.insert("cat.png".to_string(), vec![0x89, b'P', b'N', b'G']);
        let mut warnings = Vec::new();
        let describer = MockDescriber;
        resolve_image_placeholders(
            &mut md,
            &mut pt,
            &infos,
            &image_bytes,
            Some(&describer),
            &mut warnings,
        );
        assert!(md.contains("![LLM description](cat.png)"));
        assert_eq!(pt, "LLM description");
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_resolve_image_placeholders_describer_error_fallback() {
        use crate::converter::ImageDescriber;
        use crate::error::ConvertError;

        struct FailingDescriber;
        impl ImageDescriber for FailingDescriber {
            fn describe(
                &self,
                _image_bytes: &[u8],
                _mime_type: &str,
                _prompt: &str,
            ) -> Result<String, ConvertError> {
                Err(ConvertError::ImageDescriptionError {
                    reason: "API error".to_string(),
                })
            }
        }

        let mut md = "![__img_0__](cat.png)".to_string();
        let mut pt = "__img_0__".to_string();
        let infos = vec![ImageInfo {
            placeholder: "__img_0__".to_string(),
            original_alt: "A cat".to_string(),
            filename: "cat.png".to_string(),
            bytes_key: "__img_0__".to_string(),
        }];
        let mut image_bytes = HashMap::new();
        image_bytes.insert("cat.png".to_string(), vec![0x89, b'P', b'N', b'G']);
        let mut warnings = Vec::new();
        let describer = FailingDescriber;
        resolve_image_placeholders(
            &mut md,
            &mut pt,
            &infos,
            &image_bytes,
            Some(&describer),
            &mut warnings,
        );
        assert!(md.contains("![A cat](cat.png)"));
        assert_eq!(pt, "A cat");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("image description failed"));
    }

    #[test]
    fn test_resolve_image_placeholders_bytes_key_disambiguates_same_filename() {
        use crate::converter::ImageDescriber;
        use crate::error::ConvertError;

        struct ByteKeyDescriber;
        impl ImageDescriber for ByteKeyDescriber {
            fn describe(
                &self,
                image_bytes: &[u8],
                _mime_type: &str,
                _prompt: &str,
            ) -> Result<String, ConvertError> {
                match image_bytes.first().copied() {
                    Some(b'A') => Ok("DESC_A".to_string()),
                    Some(b'B') => Ok("DESC_B".to_string()),
                    _ => Ok("DESC_UNKNOWN".to_string()),
                }
            }
        }

        let mut md = "![__img_0__](image1.png)\n![__img_1__](image1.png)".to_string();
        let mut pt = "__img_0__\n__img_1__".to_string();
        let infos = vec![
            ImageInfo {
                placeholder: "__img_0__".to_string(),
                original_alt: "".to_string(),
                filename: "image1.png".to_string(),
                bytes_key: "k1".to_string(),
            },
            ImageInfo {
                placeholder: "__img_1__".to_string(),
                original_alt: "".to_string(),
                filename: "image1.png".to_string(),
                bytes_key: "k2".to_string(),
            },
        ];

        let mut image_bytes = HashMap::new();
        image_bytes.insert("k1".to_string(), b"A-image".to_vec());
        image_bytes.insert("k2".to_string(), b"B-image".to_vec());

        let mut warnings = Vec::new();
        let describer = ByteKeyDescriber;
        resolve_image_placeholders(
            &mut md,
            &mut pt,
            &infos,
            &image_bytes,
            Some(&describer),
            &mut warnings,
        );

        assert!(md.contains("![DESC_A](image1.png)"));
        assert!(md.contains("![DESC_B](image1.png)"));
        assert!(pt.contains("DESC_A"));
        assert!(pt.contains("DESC_B"));
        assert!(warnings.is_empty());
    }

    // ---- Async resolve tests (require tokio dev-dependency) ----

    #[cfg(all(feature = "async", not(target_arch = "wasm32")))]
    mod async_tests {
        use super::*;
        use crate::converter::AsyncImageDescriber;
        use crate::error::ConvertError;
        use std::future::Future;
        use std::pin::Pin;

        struct MockAsyncDescriber;
        impl AsyncImageDescriber for MockAsyncDescriber {
            fn describe<'a>(
                &'a self,
                _image_bytes: &'a [u8],
                _mime_type: &'a str,
                _prompt: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<String, ConvertError>> + Send + 'a>>
            {
                Box::pin(async { Ok("async description".to_string()) })
            }
        }

        struct FailingAsyncDescriber;
        impl AsyncImageDescriber for FailingAsyncDescriber {
            fn describe<'a>(
                &'a self,
                _image_bytes: &'a [u8],
                _mime_type: &'a str,
                _prompt: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<String, ConvertError>> + Send + 'a>>
            {
                Box::pin(async {
                    Err(ConvertError::ImageDescriptionError {
                        reason: "async API error".to_string(),
                    })
                })
            }
        }

        #[tokio::test]
        async fn test_resolve_image_placeholders_async_with_describer() {
            let mut md = "![__img_0__](cat.png)\n![__img_1__](dog.png)".to_string();
            let mut pt = "__img_0__\n__img_1__".to_string();
            let infos = vec![
                ImageInfo {
                    placeholder: "__img_0__".to_string(),
                    original_alt: "A cat".to_string(),
                    filename: "cat.png".to_string(),
                    bytes_key: "__img_0__".to_string(),
                },
                ImageInfo {
                    placeholder: "__img_1__".to_string(),
                    original_alt: "A dog".to_string(),
                    filename: "dog.png".to_string(),
                    bytes_key: "__img_1__".to_string(),
                },
            ];
            let mut image_bytes = HashMap::new();
            image_bytes.insert("cat.png".to_string(), vec![0x89, b'P', b'N', b'G']);
            image_bytes.insert("dog.png".to_string(), vec![0xFF, 0xD8, 0xFF]);
            let mut warnings = Vec::new();
            let describer = MockAsyncDescriber;
            resolve_image_placeholders_async(
                &mut md,
                &mut pt,
                &infos,
                &image_bytes,
                &describer,
                &mut warnings,
            )
            .await;
            assert!(md.contains("![async description](cat.png)"));
            assert!(md.contains("![async description](dog.png)"));
            assert!(pt.contains("async description"));
            assert!(!pt.contains("__img_"));
            assert!(warnings.is_empty());
        }

        #[tokio::test]
        async fn test_resolve_image_placeholders_async_error_fallback() {
            let mut md = "![__img_0__](cat.png)".to_string();
            let mut pt = "__img_0__".to_string();
            let infos = vec![ImageInfo {
                placeholder: "__img_0__".to_string(),
                original_alt: "A cat".to_string(),
                filename: "cat.png".to_string(),
                bytes_key: "__img_0__".to_string(),
            }];
            let mut image_bytes = HashMap::new();
            image_bytes.insert("cat.png".to_string(), vec![0x89, b'P', b'N', b'G']);
            let mut warnings = Vec::new();
            let describer = FailingAsyncDescriber;
            resolve_image_placeholders_async(
                &mut md,
                &mut pt,
                &infos,
                &image_bytes,
                &describer,
                &mut warnings,
            )
            .await;
            assert!(md.contains("![A cat](cat.png)"));
            assert_eq!(pt, "A cat");
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].message.contains("image description failed"));
        }

        #[tokio::test]
        async fn test_resolve_image_placeholders_async_missing_bytes() {
            let mut md = "![__img_0__](cat.png)".to_string();
            let mut pt = "__img_0__".to_string();
            let infos = vec![ImageInfo {
                placeholder: "__img_0__".to_string(),
                original_alt: "A cat".to_string(),
                filename: "cat.png".to_string(),
                bytes_key: "__img_0__".to_string(),
            }];
            let image_bytes = HashMap::new(); // no bytes
            let mut warnings = Vec::new();
            let describer = MockAsyncDescriber;
            resolve_image_placeholders_async(
                &mut md,
                &mut pt,
                &infos,
                &image_bytes,
                &describer,
                &mut warnings,
            )
            .await;
            // Falls back to original alt text
            assert!(md.contains("![A cat](cat.png)"));
            assert_eq!(pt, "A cat");
            assert!(warnings.is_empty());
        }
    }
}
