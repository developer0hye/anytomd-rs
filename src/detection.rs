use std::path::Path;

/// Magic bytes signatures for supported formats.
const ZIP_MAGIC: &[u8] = &[0x50, 0x4B, 0x03, 0x04];
const PDF_MAGIC: &[u8] = b"%PDF";

/// Detect the document format from a file path and optional header bytes.
///
/// Priority: magic bytes → file extension.
/// For ZIP-based formats (DOCX, PPTX, XLSX), the caller should use
/// `detect_zip_format` on the full file data for accurate detection.
pub fn detect_format(path: &Path, header_bytes: &[u8]) -> Option<&'static str> {
    // 1. Magic bytes / file signature
    if header_bytes.len() >= 4 {
        if header_bytes.starts_with(ZIP_MAGIC) {
            // Cannot distinguish DOCX/PPTX/XLSX from magic bytes alone;
            // return "zip" — caller should use detect_zip_format for specifics.
            return Some("zip");
        }
        if header_bytes.starts_with(PDF_MAGIC) {
            return Some("pdf");
        }
    }

    // 2. JSON heuristic: starts with { or [
    if let Some(&first) = header_bytes.iter().find(|b| !b.is_ascii_whitespace()) {
        if first == b'{' || first == b'[' {
            return Some("json");
        }
    }

    // 3. File extension
    detect_by_extension(path)
}

/// Detect the specific format of a ZIP-based file by inspecting its internal paths.
///
/// Returns "docx", "pptx", or "xlsx" based on the presence of characteristic
/// internal files. Returns None if the ZIP does not match a known format.
pub fn detect_zip_format(data: &[u8]) -> Option<&'static str> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;

    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index_raw(i) {
            let name = file.name();
            if name.starts_with("word/") {
                return Some("docx");
            }
            if name.starts_with("ppt/") {
                return Some("pptx");
            }
            if name.starts_with("xl/") {
                return Some("xlsx");
            }
        }
    }

    None
}

/// Detect format by file extension alone.
fn detect_by_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" => Some("docx"),
        "pptx" => Some("pptx"),
        "xlsx" => Some("xlsx"),
        "xls" => Some("xls"),
        "csv" => Some("csv"),
        "json" => Some("json"),
        "pdf" => Some("pdf"),
        "html" | "htm" => Some("html"),
        "xml" => Some("xml"),
        "txt" | "text" | "log" | "md" | "markdown" | "rst" | "ini" | "cfg" | "conf" | "toml"
        | "yaml" | "yml" => Some("txt"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "svg" | "heic"
        | "heif" | "avif" => Some("image"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_format_docx_by_extension() {
        let path = PathBuf::from("document.docx");
        assert_eq!(detect_format(&path, &[]), Some("docx"));
    }

    #[test]
    fn test_detect_format_pptx_by_extension() {
        let path = PathBuf::from("slides.pptx");
        assert_eq!(detect_format(&path, &[]), Some("pptx"));
    }

    #[test]
    fn test_detect_format_xlsx_by_extension() {
        let path = PathBuf::from("data.xlsx");
        assert_eq!(detect_format(&path, &[]), Some("xlsx"));
    }

    #[test]
    fn test_detect_format_csv_by_extension() {
        let path = PathBuf::from("data.csv");
        assert_eq!(detect_format(&path, &[]), Some("csv"));
    }

    #[test]
    fn test_detect_format_json_by_extension() {
        let path = PathBuf::from("config.json");
        assert_eq!(detect_format(&path, &[]), Some("json"));
    }

    #[test]
    fn test_detect_format_txt_by_extension() {
        let path = PathBuf::from("readme.txt");
        assert_eq!(detect_format(&path, &[]), Some("txt"));
    }

    #[test]
    fn test_detect_format_text_variants() {
        for ext in &[
            "log", "md", "markdown", "rst", "ini", "cfg", "conf", "toml", "yaml", "yml",
        ] {
            let path = PathBuf::from(format!("file.{}", ext));
            assert_eq!(
                detect_format(&path, &[]),
                Some("txt"),
                "expected 'txt' for .{}",
                ext
            );
        }
    }

    #[test]
    fn test_detect_format_pdf_by_extension() {
        let path = PathBuf::from("paper.pdf");
        assert_eq!(detect_format(&path, &[]), Some("pdf"));
    }

    #[test]
    fn test_detect_format_html_by_extension() {
        let path = PathBuf::from("page.html");
        assert_eq!(detect_format(&path, &[]), Some("html"));
        let path2 = PathBuf::from("page.htm");
        assert_eq!(detect_format(&path2, &[]), Some("html"));
    }

    #[test]
    fn test_detect_format_unknown_returns_none() {
        let path = PathBuf::from("file.xyz");
        assert_eq!(detect_format(&path, &[]), None);
    }

    #[test]
    fn test_detect_format_no_extension_returns_none() {
        let path = PathBuf::from("Makefile");
        assert_eq!(detect_format(&path, &[]), None);
    }

    #[test]
    fn test_detect_format_zip_magic_bytes_override_extension() {
        let path = PathBuf::from("data.csv");
        let zip_header = [0x50, 0x4B, 0x03, 0x04];
        // ZIP magic bytes should win over .csv extension
        assert_eq!(detect_format(&path, &zip_header), Some("zip"));
    }

    #[test]
    fn test_detect_format_pdf_magic_bytes_override_extension() {
        let path = PathBuf::from("file.txt");
        let pdf_header = b"%PDF-1.7";
        assert_eq!(detect_format(&path, pdf_header), Some("pdf"));
    }

    #[test]
    fn test_detect_format_json_heuristic_object() {
        let path = PathBuf::from("data.bin");
        let json_bytes = b"  { \"key\": \"value\" }";
        assert_eq!(detect_format(&path, json_bytes), Some("json"));
    }

    #[test]
    fn test_detect_format_json_heuristic_array() {
        let path = PathBuf::from("data.bin");
        let json_bytes = b"[1, 2, 3]";
        assert_eq!(detect_format(&path, json_bytes), Some("json"));
    }

    #[test]
    fn test_detect_format_png_by_extension() {
        let path = PathBuf::from("photo.png");
        assert_eq!(detect_format(&path, &[]), Some("image"));
    }

    #[test]
    fn test_detect_format_jpg_by_extension() {
        let path = PathBuf::from("photo.jpg");
        assert_eq!(detect_format(&path, &[]), Some("image"));
    }

    #[test]
    fn test_detect_format_jpeg_by_extension() {
        let path = PathBuf::from("photo.jpeg");
        assert_eq!(detect_format(&path, &[]), Some("image"));
    }

    #[test]
    fn test_detect_format_svg_by_extension() {
        let path = PathBuf::from("icon.svg");
        assert_eq!(detect_format(&path, &[]), Some("image"));
    }

    #[test]
    fn test_detect_format_image_variants() {
        for ext in &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "svg", "heic", "heif",
            "avif",
        ] {
            let path = PathBuf::from(format!("file.{}", ext));
            assert_eq!(
                detect_format(&path, &[]),
                Some("image"),
                "expected 'image' for .{}",
                ext
            );
        }
    }
}
