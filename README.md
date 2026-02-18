# anytomd

A pure Rust library that converts various document formats into Markdown — designed for LLM consumption.

[![CI](https://github.com/developer0hye/anytomd-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/developer0hye/anytomd-rs/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/anytomd.svg)](https://crates.io/crates/anytomd)
[![License](https://img.shields.io/crates/l/anytomd.svg)](LICENSE)

## Why?

[MarkItDown](https://github.com/microsoft/markitdown) is a great Python library for converting documents to Markdown. But integrating Python into Rust applications means bundling a Python runtime (~50 MB), dealing with cross-platform compatibility issues, and managing dependency hell.

**anytomd** solves this with a single `cargo add anytomd` — zero external runtime, no C bindings, no subprocess calls. Just pure Rust.

## Supported Formats

| Format | Extensions | Notes |
|--------|-----------|-------|
| DOCX | `.docx` | Headings, tables, lists, bold/italic, hyperlinks, images |
| PPTX | `.pptx` | Slides, tables, speaker notes, images |
| XLSX | `.xlsx` | Multi-sheet, date/time handling, images |
| XLS | `.xls` | Legacy Excel (via calamine) |
| HTML | `.html`, `.htm` | Full DOM: headings, tables, lists, links, blockquotes, code blocks |
| CSV | `.csv` | Converted to Markdown tables |
| JSON | `.json` | Pretty-printed in fenced code blocks |
| XML | `.xml` | Pretty-printed in fenced code blocks |
| Images | `.png`, `.jpg`, `.gif`, `.webp`, `.bmp`, `.tiff`, `.svg`, `.heic`, `.avif` | Optional LLM-based alt text via `ImageDescriber` |
| Plain Text | `.txt`, `.md`, `.rst`, `.log`, `.toml`, `.yaml`, `.ini`, etc. | Passthrough with encoding detection (UTF-8, UTF-16, Windows-1252) |

Format is auto-detected from magic bytes and file extension. ZIP-based formats (DOCX/PPTX/XLSX) are distinguished by inspecting internal archive structure.

## Installation

```sh
cargo add anytomd
```

To enable the built-in Gemini image describer:

```sh
cargo add anytomd --features gemini
```

## Quick Start

```rust
use anytomd::{convert_file, convert_bytes, ConversionOptions};

// Convert a file (format auto-detected from extension and magic bytes)
let options = ConversionOptions::default();
let result = convert_file("document.docx", &options).unwrap();
println!("{}", result.markdown);

// Convert raw bytes with an explicit format
let csv_data = b"Name,Age\nAlice,30\nBob,25";
let result = convert_bytes(csv_data, "csv", &options).unwrap();
println!("{}", result.markdown);
```

### Extracting Embedded Images

```rust
use anytomd::{convert_file, ConversionOptions};

let options = ConversionOptions {
    extract_images: true,
    ..Default::default()
};
let result = convert_file("presentation.pptx", &options).unwrap();

for (filename, bytes) in &result.images {
    std::fs::write(filename, bytes).unwrap();
}
```

### LLM-Based Image Descriptions

anytomd can generate alt text for images using any LLM backend via the `ImageDescriber` trait. A built-in Google Gemini implementation is available behind the `gemini` feature.

```rust
use std::sync::Arc;
use anytomd::{convert_file, ConversionOptions, ImageDescriber, ConvertError};

// Option 1: Use the built-in Gemini describer (requires `gemini` feature)
#[cfg(feature = "gemini")]
{
    use anytomd::gemini::GeminiDescriber;

    let describer = GeminiDescriber::from_env()  // reads GEMINI_API_KEY
        .unwrap()
        .with_model("gemini-3-flash-preview".to_string());

    let options = ConversionOptions {
        image_describer: Some(Arc::new(describer)),
        ..Default::default()
    };
    let result = convert_file("document.docx", &options).unwrap();
    // Images now have LLM-generated alt text: ![A chart showing quarterly revenue](chart.png)
}

// Option 2: Implement your own describer for any backend
struct MyDescriber;

impl ImageDescriber for MyDescriber {
    fn describe(
        &self,
        image_bytes: &[u8],
        mime_type: &str,
        prompt: &str,
    ) -> Result<String, ConvertError> {
        // Call your preferred LLM API here
        Ok("description of the image".to_string())
    }
}
```

## API

### `convert_file`

```rust
/// Convert a file at the given path to Markdown.
/// Format is auto-detected from magic bytes and file extension.
pub fn convert_file(
    path: impl AsRef<Path>,
    options: &ConversionOptions,
) -> Result<ConversionResult, ConvertError>
```

### `convert_bytes`

```rust
/// Convert raw bytes to Markdown with an explicit format extension.
pub fn convert_bytes(
    data: &[u8],
    extension: &str,
    options: &ConversionOptions,
) -> Result<ConversionResult, ConvertError>
```

### `ConversionOptions`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `extract_images` | `bool` | `false` | Extract embedded images into `result.images` |
| `max_total_image_bytes` | `usize` | 50 MB | Hard cap for total extracted image bytes |
| `max_input_bytes` | `usize` | 100 MB | Maximum input file size |
| `max_uncompressed_zip_bytes` | `usize` | 500 MB | ZIP bomb guard |
| `strict` | `bool` | `false` | Error on recoverable failures instead of warnings |
| `image_describer` | `Option<Arc<dyn ImageDescriber>>` | `None` | LLM backend for image alt text generation |

### `ConversionResult`

```rust
pub struct ConversionResult {
    pub markdown: String,                  // The converted Markdown
    pub title: Option<String>,             // Document title, if detected
    pub images: Vec<(String, Vec<u8>)>,    // Extracted images (filename, bytes)
    pub warnings: Vec<ConversionWarning>,  // Recoverable issues encountered
}
```

### Error Handling

Conversion is **best-effort** by default. If a single element fails to parse (e.g., a corrupted table), it is skipped and a warning is added to `result.warnings`. The rest of the document is still converted.

Set `strict: true` in `ConversionOptions` to turn recoverable failures into errors instead.

Warning codes: `SkippedElement`, `UnsupportedFeature`, `ResourceLimitReached`, `MalformedSegment`.

## Development

### Build and Test

```sh
cargo build && cargo test && cargo clippy -- -D warnings
```

With the Gemini feature:

```sh
cargo test --features gemini
cargo clippy --features gemini -- -D warnings
```

### Docker

A Docker environment is available for reproducible Linux builds:

```sh
docker compose run --rm verify    # Full loop: fmt + clippy + test + release build
docker compose run --rm test      # Run all tests
docker compose run --rm lint      # clippy + fmt check
docker compose run --rm shell     # Interactive bash
```

## License

Apache-2.0
