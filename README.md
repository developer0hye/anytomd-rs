# anytomd

A pure Rust library that converts various document formats into Markdown — designed for LLM consumption.

[![CI](https://github.com/developer0hye/anytomd-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/developer0hye/anytomd-rs/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/anytomd.svg)](https://crates.io/crates/anytomd)
[![License](https://img.shields.io/crates/l/anytomd.svg)](LICENSE)

## Why?

[MarkItDown](https://github.com/microsoft/markitdown) is a great Python library for converting documents to Markdown. But integrating Python into Rust applications means bundling a Python runtime (~50MB), dealing with cross-platform compatibility issues, and managing dependency hell.

**anytomd** solves this with a single `cargo add anytomd` — zero external runtime, no C bindings, no subprocess calls. Just pure Rust.

## Supported Formats

| Format | Extensions | Notes |
|--------|-----------|-------|
| DOCX | `.docx` | Headings, tables, lists, bold/italic, hyperlinks, images |
| PPTX | `.pptx` | Slide content, tables, text formatting |
| XLSX | `.xlsx` | Multi-sheet support, merged cells, date handling |
| HTML | `.html`, `.htm` | Headings, tables, lists, links, text formatting |
| CSV | `.csv` | Converted to Markdown tables |
| JSON | `.json` | Pretty-printed as code blocks |
| XML | `.xml` | Pretty-printed as code blocks |
| Plain Text | `.txt`, `.md`, `.rst`, `.log`, etc. | Passed through with encoding detection |

## Installation

```sh
cargo add anytomd
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

## API

### `ConversionOptions`

Controls conversion behavior:

```rust
let options = ConversionOptions {
    extract_images: true,               // Extract embedded images (default: false)
    max_total_image_bytes: 50 * 1024 * 1024, // Image extraction cap (default: 50 MB)
    max_input_bytes: 100 * 1024 * 1024,      // Max input file size (default: 100 MB)
    max_uncompressed_zip_bytes: 500 * 1024 * 1024, // ZIP bomb guard (default: 500 MB)
    strict: false,                      // Error on recoverable failures (default: false)
};
```

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

## Comparison with MarkItDown

| | anytomd | MarkItDown |
|---|---------|------------|
| Language | Pure Rust | Python |
| Runtime dependency | None | Python interpreter |
| DOCX conversion | Direct OOXML-to-Markdown | DOCX → HTML → Markdown (two-step) |
| Binary size | Single static binary | ~50 MB with bundled Python |
| Integration | `cargo add anytomd` | PyO3/subprocess/bundled runtime |

## License

Apache-2.0
