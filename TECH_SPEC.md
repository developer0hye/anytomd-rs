# Technical Specification: anytomd

## 1. Overview

### 1.1 Project Name
**anytomd**

### 1.2 Purpose
A pure Rust library that converts various document formats into Markdown — a native Rust reimplementation of Microsoft's [MarkItDown](https://github.com/microsoft/markitdown) Python library.

### 1.3 Motivation

MarkItDown is a widely-used Python library for converting documents to Markdown, optimized for LLM consumption. However, integrating it into non-Python applications (e.g., Rust-based desktop apps, CLI tools, or WASM targets) requires bundling a Python runtime via PyInstaller or similar tools, which introduces:

- **~50MB+ binary size overhead** from bundled Python runtime and dependencies
- **Cross-platform build fragility** — PyInstaller builds break across macOS, Linux, and Windows
- **Process isolation overhead** — IPC between host application and Python sidecar
- **Dependency hell** — transitive Python dependencies (`mammoth`, `pdfminer`, `openpyxl`, etc.)

**anytomd** solves this by providing the same document-to-Markdown conversion entirely in Rust, as a single `cargo add anytomd` dependency with zero external runtime requirements.

### 1.4 Goals
- **Feature parity** with MarkItDown's core document conversion capabilities
- **Pure Rust** — no Python, no C bindings, no external processes
- **Cross-platform** — macOS, Linux, Windows without platform-specific dependencies
- **Library-first** — usable as a Rust crate (`lib.rs`), with an optional CLI binary
- **LLM-optimized output** — Markdown that preserves document structure for AI consumption

### 1.5 Non-Goals
- Pixel-perfect document rendering (we target LLM-readable Markdown, not visual fidelity)
- Audio transcription — out of scope (requires specialized models)
- Cloud service integrations (Azure Document Intelligence, YouTube API, etc.)
- MCP server implementation

---

## 2. Feature Scope

### 2.1 MarkItDown Feature Mapping

| MarkItDown Feature | Python Library Used | anytomd Approach | Priority |
|-------------------|--------------------|--------------------|----------|
| **DOCX → MD** | `mammoth` (DOCX→HTML→MD) | `zip` + `quick-xml` (direct OOXML→MD) | P0 (MVP) |
| **PPTX → MD** | `python-pptx` | `zip` + `quick-xml` (direct OOXML→MD) | P0 (MVP) |
| **XLSX → MD** | `pandas` + `openpyxl` | `calamine` | P0 (MVP) |
| **XLS → MD** | `xlrd` | `calamine` (supports legacy .xls) | P1 |
| **PDF → MD** | `pdfminer.six` + `pdfplumber` | `pdf-extract` or `lopdf` | P1 |
| **HTML → MD** | `BeautifulSoup` + custom markdownify | `scraper` + custom MD converter | P1 |
| **CSV → MD** | Python `csv` module | `csv` crate | P0 (MVP) |
| **JSON → MD** | built-in | `serde_json` (pretty-print as code block) | P0 (MVP) |
| **XML → MD** | built-in | `quick-xml` (pretty-print as code block) | P1 |
| **Plain Text → MD** | `read()` | `std::fs::read_to_string` (passthrough) | P0 (MVP) |
| **Images** | `Pillow` + EXIF extraction | `kamadak-exif` (EXIF metadata only) | P2 |
| **EPUB → MD** | `ebooklib` | `zip` + HTML converter (EPUB is XHTML in ZIP) | P2 |
| **Outlook MSG → MD** | `olefile` | OLE2 parsing (complex, low priority) | P3 |
| **Audio → MD** | `pydub` + `SpeechRecognition` | Out of scope (LLM-dependent) | — |
| **YouTube → MD** | `youtube-transcript-api` | Out of scope (cloud service) | — |
| **ZIP → MD** | `zipfile` (recursive) | `zip` crate (recursive conversion) | P1 |

### 2.2 Priority Definitions

| Priority | Meaning | Target |
|----------|---------|--------|
| P0 | MVP — must ship in v0.1.0 | DOCX, PPTX, XLSX, CSV, JSON, Plain Text |
| P1 | Core completeness — v0.2.0 | PDF, HTML, XLS, XML, ZIP |
| P2 | Extended formats — v0.3.0 | Images (EXIF), EPUB |
| P3 | Niche formats — future | Outlook MSG |

---

## 3. Architecture

### 3.1 Crate Structure

```
anytomd-rs/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API: convert(), detect_format()
│   ├── converter/
│   │   ├── mod.rs           # Converter trait definition
│   │   ├── docx.rs          # DOCX → Markdown
│   │   ├── pptx.rs          # PPTX → Markdown
│   │   ├── xlsx.rs          # XLSX → Markdown
│   │   ├── csv_conv.rs      # CSV → Markdown
│   │   ├── json_conv.rs     # JSON → Markdown
│   │   ├── plain_text.rs    # Plain text passthrough
│   │   ├── pdf.rs           # PDF → Markdown (P1)
│   │   ├── html.rs          # HTML → Markdown (P1)
│   │   └── ...
│   ├── markdown.rs          # Markdown generation utilities (tables, headings, lists)
│   ├── detection.rs         # File format detection (extension + magic bytes)
│   └── error.rs             # Error types
│
├── tests/                   # Integration tests with sample files
│   ├── fixtures/            # Sample DOCX, PPTX, XLSX, etc.
│   └── ...
│
└── examples/
    └── convert.rs           # CLI-style example
```

### 3.2 Core Trait

```rust
pub enum WarningCode {
    SkippedElement,
    UnsupportedFeature,
    ResourceLimitReached,
    MalformedSegment,
}

pub struct ConversionWarning {
    pub code: WarningCode,
    pub message: String,
    pub location: Option<String>,
}

pub struct ConversionOptions {
    /// Extract embedded images into `ConversionResult.images`
    pub extract_images: bool,
    /// Hard cap for total extracted image bytes per document
    pub max_total_image_bytes: usize,
    /// If true, return an error on recoverable parse failures
    pub strict: bool,
}

pub struct ConversionResult {
    /// Converted Markdown content
    pub markdown: String,
    /// Document title (if detected)
    pub title: Option<String>,
    /// Extracted images as (filename, bytes) pairs
    pub images: Vec<(String, Vec<u8>)>,
    /// Recoverable issues encountered during conversion
    pub warnings: Vec<ConversionWarning>,
}

pub trait Converter {
    /// Returns supported file extensions (e.g., ["docx"])
    fn supported_extensions(&self) -> &[&str];

    /// Check if this converter can handle the given bytes/extension
    fn can_convert(&self, extension: &str, _header_bytes: &[u8]) -> bool {
        self.supported_extensions().contains(&extension)
    }

    /// Convert file bytes to Markdown with conversion options
    fn convert(
        &self,
        data: &[u8],
        options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError>;
}
```

### 3.3 Public API

```rust
// Simple: auto-detect format and convert
let result = anytomd::convert_file("document.docx", &ConversionOptions::default())?;
println!("{}", result.markdown);

// From bytes with explicit format
let bytes = std::fs::read("document.docx")?;
let result = anytomd::convert_bytes(&bytes, "docx", &ConversionOptions::default())?;

// Access extracted images
for (filename, image_bytes) in &result.images {
    std::fs::write(format!("output/{}", filename), image_bytes)?;
}

// Inspect recoverable conversion warnings
for warning in &result.warnings {
    eprintln!("[{:?}] {}", warning.code, warning.message);
}
```

### 3.4 Package and Crate Naming

- Crates.io package name: `anytomd`
- Rust library crate name: `anytomd`
- Repository name: `anytomd-rs`
- All API examples use `anytomd::...` import path

### 3.5 Format Detection Precedence

Format detection must be deterministic and follow this order:

1. Magic bytes / file signature (highest priority)
2. Container introspection (e.g., ZIP internal paths like `word/document.xml`)
3. File extension
4. Explicit fallback to plain text (only if all checks fail)

If extension and magic bytes conflict, magic bytes win and a warning is recorded in `ConversionResult.warnings`.

---

## 4. Format-Specific Implementation Details

### 4.1 DOCX (P0)

DOCX is an OOXML format — a ZIP archive containing XML files.

**Key files inside DOCX:**
```
word/document.xml          — main document body
word/styles.xml            — heading/paragraph style definitions
word/numbering.xml         — list numbering definitions
word/media/*               — embedded images
word/_rels/document.xml.rels — relationship mappings (image refs)
```

**Extraction targets:**
| Element | XML Path | Markdown Output |
|---------|----------|----------------|
| Paragraph text | `<w:p>` → `<w:r>` → `<w:t>` | Plain text with line breaks |
| Headings | `<w:pStyle w:val="Heading1">` | `# Heading` |
| Bold | `<w:b/>` in `<w:rPr>` | `**bold**` |
| Italic | `<w:i/>` in `<w:rPr>` | `*italic*` |
| Tables | `<w:tbl>` → `<w:tr>` → `<w:tc>` | Pipe-delimited MD table |
| Hyperlinks | `<w:hyperlink>` + rels | `[text](url)` |
| Images | `<w:drawing>` + rels → media/ | Extract to `ConversionResult.images` |
| Lists | `<w:numPr>` + numbering.xml | `- item` or `1. item` |

**MarkItDown comparison:**
- MarkItDown: DOCX → HTML (via mammoth) → Markdown (via markdownify) — two conversion steps
- anytomd: DOCX → Markdown directly from OOXML XML — single step, no intermediate HTML

### 4.2 PPTX (P0)

PPTX is also OOXML — similar ZIP+XML structure.

**Key files inside PPTX:**
```
ppt/presentation.xml       — slide order
ppt/slides/slide{N}.xml   — individual slide content
ppt/slides/_rels/slide{N}.xml.rels — image refs per slide
ppt/media/*                — embedded images
```

**Extraction targets:**
| Element | XML Path | Markdown Output |
|---------|----------|----------------|
| Slide title | `<a:t>` inside title placeholder | `## Slide {N}: Title` |
| Text body | `<a:t>` inside body placeholders | Paragraph text |
| Tables | `<a:tbl>` → `<a:tr>` → `<a:tc>` | Pipe-delimited MD table |
| Speaker notes | `notesSlide{N}.xml` → `<a:t>` | `> Note: ...` (blockquote) |
| Images | `<a:blip>` + rels → media/ | Extract to `ConversionResult.images` |

**Output structure per slide:**
```markdown
## Slide 1: Quarterly Revenue

Revenue grew 15% year-over-year.

| Region | Q3 2024 | Q3 2023 |
|--------|---------|---------|
| APAC   | $1.2M   | $1.0M   |

> Note: Emphasize APAC growth in presentation.

---

## Slide 2: Next Steps
...
```

### 4.3 XLSX (P0)

Use `calamine` crate for cell data extraction — it handles both `.xlsx` (OOXML) and `.xls` (legacy BIFF).

```rust
use calamine::{Reader, open_workbook, Xlsx};

fn convert_xlsx(data: &[u8]) -> Result<ConversionResult> {
    let cursor = std::io::Cursor::new(data);
    let mut workbook: Xlsx<_> = Xlsx::new(cursor)?;
    let mut markdown = String::new();

    for sheet_name in workbook.sheet_names().to_owned() {
        markdown.push_str(&format!("## {}\n\n", sheet_name));

        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            // First row as header
            // Remaining rows as table body
            // → Pipe-delimited Markdown table
        }
    }

    Ok(ConversionResult { markdown, ..Default::default() })
}
```

### 4.4 CSV (P0)

```rust
// CSV → Markdown table (pipe-delimited)
// Uses `csv` crate for robust parsing (handles quoting, escaping)
```

### 4.5 JSON (P0)

```rust
// JSON → Markdown code block
// Pretty-print with serde_json::to_string_pretty, wrap in ```json ... ```
```

### 4.6 Plain Text (P0)

```rust
// Direct passthrough — read as UTF-8 string, return as-is
// Detect encoding if not UTF-8 (optional: `encoding_rs` crate)
```

### 4.7 PDF (P1)

PDF text extraction is significantly harder than OOXML. Options:

| Crate | Approach | Pros | Cons |
|-------|----------|------|------|
| `pdf-extract` | Extracts text with layout inference | Good text quality | Larger dependency |
| `lopdf` | Low-level PDF parsing | Lightweight | Manual text extraction |

**Decision: Start with `pdf-extract` for P1.** If quality or maintenance becomes an issue, fall back to `lopdf` with custom text extraction. C-binding options are out of scope.

Scanned PDFs (image-based) cannot be handled without OCR — this is explicitly out of scope. The consuming application should use Gemini/GPT vision for scanned documents.

### 4.8 HTML (P1)

```rust
// HTML → Markdown using `scraper` for DOM parsing + custom converter
// Handle: headings, paragraphs, tables, lists, links, images, code blocks
// Similar to Python's markdownify but in Rust
```

### 4.9 LLM-Assisted Image Description

Embedded images in DOCX/PPTX can optionally be described by an external LLM. This provides richer Markdown output (e.g., `![A bar chart showing Q3 revenue by region](image_1.png)`) instead of generic filenames.

**Design: Trait-based injection**

The library does NOT make HTTP calls or manage API keys itself. Instead, callers provide an implementation of the `ImageDescriber` trait:

```rust
pub trait ImageDescriber: Send + Sync {
    fn describe(&self, image_bytes: &[u8], prompt: &str) -> Result<String, ConvertError>;
}
```

This is passed via `ConversionOptions`:

```rust
pub struct ConversionOptions {
    // ... existing fields ...
    /// Optional LLM-based image describer.
    pub image_describer: Option<Box<dyn ImageDescriber>>,
}
```

**Behavior:**
- If `image_describer` is `None` (default), images are referenced by filename only — no LLM call is made
- If provided, the describer is called for each extracted image with the image bytes and a default prompt
- If the describer returns an error, the image is still included with a generic filename and a warning is appended
- The library is agnostic to which LLM provider is used — the trait works with any backend (Gemini, OpenAI, local models, etc.)

**Default LLM provider: Google Gemini**

When building the built-in / example `ImageDescriber` implementation:
- Use **Google Gemini** as the LLM provider
- Default model: **`gemini-3-flash-preview`**
- Always refer to the [official Gemini API documentation](https://ai.google.dev/gemini-api/docs) for the latest API specs, authentication methods, and model availability before implementing or updating Gemini-related code

**API key management:**

The `ImageDescriber` trait has no concept of API keys — credential handling is entirely the implementor's responsibility. The built-in `GeminiDescriber` example should follow this pattern:

```rust
impl GeminiDescriber {
    /// Create with an explicit API key.
    pub fn new(api_key: String) -> Self { ... }

    /// Create from the `GEMINI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ConvertError> { ... }
}
```

- **Struct field** (`new`): The primary way — caller passes the key directly. Suitable for libraries and applications that manage secrets themselves.
- **Environment variable fallback** (`from_env`): Reads `GEMINI_API_KEY` from the environment. Convenient for CLI usage and examples.
- The library must **never** hardcode, log, or persist API keys.

**Model selection by context:**

| Context | Model | Rationale |
|---------|-------|-----------|
| Production / library default | `gemini-3-flash-preview` | Best quality for real-world image description |
| CI integration tests | `gemini-2.5-flash-lite` | Lowest cost; sufficient to verify API integration works end-to-end |

The `GeminiDescriber` must support model override via builder style (for example, `GeminiDescriber::new(api_key).with_model(model_name.to_string())`) so that CI can use a different model without changing library defaults. CI tests should:
- Only assert that the API returns a non-empty string (LLM output is non-deterministic)
- Be conditional on the `GEMINI_API_KEY` secret being available
- Be marked as allowed-to-fail to avoid blocking merges on transient API errors

---

## 5. Dependencies

### 5.1 MVP (P0) Dependencies

```toml
[dependencies]
zip = "2"              # ZIP archive reading (DOCX, PPTX)
quick-xml = "0.37"     # XML parsing (OOXML)
calamine = "0.26"      # XLSX/XLS reading
csv = "1"              # CSV parsing
serde_json = "1"       # JSON pretty-printing
thiserror = "2"        # Error types
```

### 5.2 P1 Dependencies (added later)

```toml
pdf-extract = "0.8"    # PDF text extraction
scraper = "0.21"       # HTML DOM parsing
```

### 5.3 Design Principle: Minimal Dependencies

Every dependency must be **pure Rust** (no C bindings). This ensures:
- `cargo build` works on all platforms without system library requirements
- WASM compilation target remains possible
- No dynamic linking issues

---

## 6. Markdown Output Conventions

### 6.1 General Rules

- UTF-8 encoded output
- Preserve multilingual Unicode text without corruption (including Korean, Chinese, Japanese, and other non-Latin scripts)
- Preserve emoji and symbol characters from source content when text is extracted
- Unix-style line endings (`\n`)
- Two newlines between major sections
- Trailing newline at end of document
- No trailing whitespace on lines

### 6.2 Table Format

```markdown
| Column A | Column B | Column C |
|----------|----------|----------|
| value 1  | value 2  | value 3  |
```

- Pipe-delimited with header separator row
- Cell content is trimmed
- Empty cells render as empty (not skipped)

### 6.3 Image References

Images embedded in DOCX/PPTX are extracted as binary data and referenced in Markdown:

```markdown
![image_1](image_1.png)
```

The actual image bytes are available in `ConversionResult.images`. The consuming application decides how to handle them (save to disk, send to LLM, etc.).

To prevent unbounded memory usage on large documents:
- Image extraction can be disabled via `ConversionOptions.extract_images = false`
- Total extracted image size must be capped by `ConversionOptions.max_total_image_bytes`
- If the cap is reached, remaining images are skipped and a warning is appended

### 6.4 Heading Hierarchy

- DOCX: Derived from paragraph styles (Heading 1 → `#`, Heading 2 → `##`, etc.)
- PPTX: Each slide title → `##`, slide number prepended
- XLSX: Each sheet name → `##`

---

## 7. Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("unsupported format: {extension}")]
    UnsupportedFormat { extension: String },

    #[error("failed to read ZIP archive")]
    ZipError(#[from] zip::result::ZipError),

    #[error("failed to parse XML")]
    XmlError(#[from] quick_xml::Error),

    #[error("failed to read spreadsheet")]
    SpreadsheetError(#[from] calamine::Error),

    #[error("I/O error")]
    Io(#[from] std::io::Error),

    #[error("invalid UTF-8 content")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("malformed document: {reason}")]
    MalformedDocument { reason: String },
}
```

**Philosophy:** Conversion should be **best-effort by default**. If a specific element fails to parse (e.g., a corrupted table), skip it and append a structured warning to `ConversionResult.warnings`.

**Strict mode:** When `ConversionOptions.strict = true`, recoverable parse failures should return `ConvertError` instead of warnings.

---

## 8. Testing Strategy

### 8.1 Test Fixtures

Create sample documents for each format in `tests/fixtures/`:
- `simple.docx` — paragraphs, headings, bold/italic
- `tables.docx` — tables with various column counts
- `images.docx` — embedded images
- `simple.pptx` — multi-slide with text and notes
- `data.xlsx` — multi-sheet with numbers and text
- `data.csv` — standard CSV with headers
- `data.json` — nested JSON object
- `plain.txt` — plain text file
- Add multilingual fixtures (Korean/Chinese/Japanese + emoji) and verify end-to-end text preservation

### 8.2 Test Approach

```rust
#[test]
fn test_docx_headings_normalized_output() {
    let result = anytomd::convert_file(
        "tests/fixtures/simple.docx",
        &ConversionOptions::default(),
    )
    .unwrap();
    let actual = normalize_markdown(&result.markdown);
    let expected = include_str!("expected/simple.normalized.md");
    assert_eq!(actual, expected);
}

#[test]
fn test_docx_table_keeps_empty_cells() {
    let result = anytomd::convert_file(
        "tests/fixtures/tables.docx",
        &ConversionOptions::default(),
    )
    .unwrap();
    assert!(result.markdown.contains("| name | value | note |"));
    assert!(result.markdown.contains("| a | 1 |  |"));
}

#[test]
fn test_resource_limit_adds_warning() {
    let options = ConversionOptions {
        max_total_image_bytes: 1024,
        ..Default::default()
    };
    let result = anytomd::convert_file("tests/fixtures/images.docx", &options).unwrap();
    assert!(result.warnings.iter().any(|w| matches!(w.code, WarningCode::ResourceLimitReached)));
}
```

### 8.3 Comparison Testing

For validation, compare anytomd output against MarkItDown output on the same input files. Markdown does not need exact string equality, but extracted content parity must be measurable:

- Normalize whitespace and punctuation, then compare token sets
- Require token recall >= 95% for MVP formats (DOCX/PPTX/XLSX/CSV/JSON/TXT)
- Compare structural signals (heading count, table count, hyperlink count) with per-format tolerances
- Add at least one fixture per format where output is manually reviewed and locked as a golden baseline

### 8.4 CI Gemini Live API Tests

CI includes optional live Gemini API integration tests that verify the `GeminiDescriber` works end-to-end with a real API call.

**Trigger policy:**

Gemini tests consume real API quota, so they are gated to prevent abuse from external PRs:

| CI trigger | Gemini tests run? | Reason |
|------------|-------------------|--------|
| `push` (any branch) | Yes, automatically | Only repo owner/collaborators can push |
| `pull_request` (default) | No | External PRs are untrusted — must be gated |
| `pull_request` with `ci:gemini` label | Yes | Owner explicitly approved after code review |

The owner must **review the PR diff before adding the `ci:gemini` label** — the label grants the PR's code access to the `GEMINI_API_KEY` secret.

**Structure:**
- Live tests live in `tests/test_gemini_live.rs` (or similar), gated behind the `gemini` feature
- Each test reads the `GEMINI_API_KEY` environment variable — if absent, the test is skipped (not failed)
- Tests use model `gemini-2.5-flash-lite` to minimize API cost
- Assertions check that the API returns a non-empty description string — they do NOT check the content (LLM output is non-deterministic)
- Live tests are marked as allowed-to-fail in CI to avoid blocking merges on transient API issues (rate limits, outages)

**Example test pattern:**
```rust
#[test]
fn test_gemini_live_describe_image() {
    let api_key = match std::env::var("GEMINI_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("GEMINI_API_KEY not set, skipping live test");
            return;
        }
    };
    let describer = GeminiDescriber::new(api_key).with_model("gemini-2.5-flash-lite".to_string());
    let image_bytes = include_bytes!("fixtures/sample_image.png");
    let result = describer.describe(image_bytes, "image/png", "Describe this image.");
    assert!(result.is_ok());
    assert!(!result.unwrap().is_empty());
}
```

---

## 9. Milestones

### v0.1.0 — MVP
- [ ] Project setup (Cargo.toml, CI)
- [ ] Converter trait + deterministic format detection (magic bytes first)
- [ ] Conversion options + warning contract (`ConversionOptions`, `ConversionResult.warnings`)
- [ ] DOCX converter (paragraphs, headings, hyperlinks; best-effort mode)
- [ ] PPTX converter (slide titles + body text)
- [ ] XLSX converter (multi-sheet, core cell types → Markdown tables)
- [ ] CSV converter (→ Markdown table)
- [ ] JSON converter (→ code block)
- [ ] Plain text converter (passthrough)
- [ ] Integration tests + normalized golden tests for all P0 formats
- [ ] README with usage examples
- [ ] Publish to crates.io

### v0.2.0 — Core Completeness
- [ ] DOCX advanced formatting (bold/italic/lists/tables/images)
- [ ] PPTX advanced content (tables/speaker notes/images)
- [ ] XLSX formula/date/error cell normalization
- [ ] PDF converter (text extraction)
- [ ] HTML converter (→ Markdown)
- [ ] XLS legacy format support (via calamine)
- [ ] XML converter (→ code block)
- [ ] ZIP recursive conversion
- [ ] Improved table formatting (column alignment, escaping)
- [ ] Encoding detection for non-UTF-8 files
- [ ] Resource-limit guards (max file size/page count/uncompressed ZIP budget)

### v0.3.0 — Extended Formats
- [ ] Image EXIF metadata extraction
- [ ] EPUB converter
- [ ] Markdown output normalization (consistent whitespace, line endings)
- [ ] Optional CLI binary (`cargo install anytomd`)

### Future
- [ ] `ImageDescriber` trait + Gemini-based example implementation
- [ ] Outlook MSG support
- [ ] WASM compilation target
- [ ] Streaming conversion for large files
- [ ] Plugin system for custom converters

---

## 10. Comparison: anytomd vs MarkItDown

| Aspect | MarkItDown (Python) | anytomd (Rust) |
|--------|--------------------|--------------------|
| Runtime | Python 3.10+ | None (native binary) |
| Install | `pip install markitdown` | `cargo add anytomd` |
| Binary size impact | ~50MB (PyInstaller) | Single-digit MB (target/profile dependent) |
| DOCX approach | DOCX → HTML → MD (2 steps) | DOCX → MD directly (1 step) |
| PDF approach | pdfminer + pdfplumber | pdf-extract (pure Rust) |
| XLSX approach | pandas + openpyxl | calamine |
| LLM features | Built-in (image caption, audio transcription) | Optional trait-based image description (Gemini default) |
| Cloud integrations | Azure, YouTube, Wikipedia, Bing | None (pure local conversion) |
| WASM support | No | Possible (P0 deps are all pure Rust) |
| Cross-platform build | PyInstaller fragility | `cargo build` (no external runtime) |

---

## 11. Non-Functional Requirements

### 11.1 Performance Targets (P0 formats)

- Convert a 10MB DOCX/PPTX/XLSX document in <= 2 seconds on a modern laptop (single-thread baseline)
- Keep peak memory usage <= 4x input file size for non-image-heavy documents
- Keep startup overhead minimal (library-first, no runtime bootstrap)

### 11.2 Safety Limits

- Reject files larger than configurable `max_input_bytes`
- Abort ZIP-based parsing if uncompressed size exceeds configurable budget
- Cap parser recursion / nesting depth for XML/HTML
- Cap PDF page count processed per conversion request

### 11.3 Determinism

- Given identical input bytes and options, output Markdown must be byte-stable
- Warning ordering must be stable and reproducible across platforms

---

## 12. License

Apache License 2.0 (matching the repository)
