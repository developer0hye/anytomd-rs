# CLAUDE.md

## Project Overview

**anytomd-rs** is a pure Rust reimplementation of Microsoft's [MarkItDown](https://github.com/microsoft/markitdown) Python library. It converts various document formats (DOCX, PPTX, XLSX, PDF, HTML, CSV, JSON, etc.) into Markdown, targeting LLM consumption.

This project exists because MarkItDown — while excellent — is a Python library, making it painful to integrate into Rust applications. Bundling Python via PyInstaller adds ~50MB, breaks across platforms, and introduces dependency hell. anytomd-rs eliminates all of this: a single `cargo add anytomd-rs` with zero external runtime.

**Current phase: MVP (v0.1.0)** — DOCX, PPTX, XLSX, CSV, JSON, Plain Text.

See [PRD.md](./PRD.md) for full architecture, format-specific implementation details, and milestones.

---

## Reference-Driven Development

The original MarkItDown Python source is cloned locally at `reference/markitdown/` for analysis during development. This directory is excluded from Git via `.gitignore`.

**How to use the reference:**
- Before implementing a converter, **read the corresponding MarkItDown converter** to understand its parsing logic, edge case handling, and output format
  - Core converters: `reference/markitdown/packages/markitdown/src/markitdown/converters/`
  - Utility helpers: `reference/markitdown/packages/markitdown/src/markitdown/converter_utils/`
  - Main engine: `reference/markitdown/packages/markitdown/src/markitdown/_markitdown.py`
- Identify what content each converter extracts (headings, tables, images, links, metadata, etc.) and ensure the Rust implementation covers the same content
- Do NOT translate Python code line-by-line — understand the *intent*, then implement idiomatically in Rust
- Use MarkItDown's test fixtures and expected outputs as additional validation where applicable

**Workflow per converter:**
1. Read the Python converter (e.g., `_docx_converter.py`)
2. Note which document elements it extracts and how it formats them
3. Write Rust tests based on the same expected content coverage
4. Implement the Rust converter using native crates
5. Compare output against MarkItDown's output for the same test files to ensure content parity (not exact string match)

---

## Critical Principle: Native Rust Implementation

This is the single most important rule of this project:

**Every converter MUST be implemented in pure Rust. No Python. No C bindings. No subprocess calls. No external runtime dependencies.**

This means:
- DOCX/PPTX: Parse OOXML directly with `zip` + `quick-xml` — do NOT shell out to Python or wrap `mammoth`/`python-pptx`
- XLSX: Use `calamine` crate — do NOT wrap `openpyxl` or `pandas`
- PDF: Use `pdf-extract` or `lopdf` — do NOT wrap `pdfminer`
- HTML: Use `scraper` crate — do NOT wrap `BeautifulSoup`

If a pure Rust solution does not exist for a format, the correct response is to **implement it in Rust** or **defer the format to a later milestone** — never to introduce a non-Rust dependency.

The output does not need to be identical to MarkItDown's Markdown. The goal is **LLM-readable text extraction**, not pixel-perfect Markdown rendering. If an LLM can understand the output, it's good enough. Simpler output that captures the content is always preferred over complex formatting logic.

MarkItDown converts DOCX via a two-step process (DOCX → HTML via mammoth → Markdown via markdownify). anytomd-rs should convert **directly from OOXML XML to Markdown in a single step**. This is faster, simpler, and avoids intermediate representation overhead.

---

## Language Rules

**All project artifacts MUST be written in English.** No exceptions.

This applies to:
- Source code (variable names, function names, type names)
- Code comments and doc comments (`///`, `//`, `//!`)
- Commit messages
- Documentation files (README, PRD, CLAUDE.md, etc.)
- Error messages and user-facing strings
- Test names and test descriptions
- Issue titles and PR descriptions
- TODO/FIXME comments

---

## Toolchain

- **Before starting any work**, check the latest Rust stable version by searching the web (e.g., "latest Rust stable version") and ensure the project targets it
- Use the **latest Rust stable** release — update `rust-version` in `Cargo.toml` accordingly
- Set `rust-version = "<latest>"` in `Cargo.toml` under `[package]` to enforce MSRV
- Do NOT use nightly-only features — everything must compile on stable
- Run `rustup update stable` before starting work to ensure the toolchain is current

---

## Code Conventions

### Rust Style
- Follow standard Rust conventions (`rustfmt` defaults, `clippy` clean)
- Use `thiserror` for error types — see `ConvertError` in PRD Section 7
- Prefer returning `Result<T, ConvertError>` over panicking
- Conversion should be **best-effort**: if a single element (e.g., one corrupted table) fails to parse, skip it and continue — do not fail the entire document

### Crate Structure
- `src/lib.rs` — public API (`convert_file`, `convert_bytes`)
- `src/converter/` — one module per format (`docx.rs`, `pptx.rs`, `xlsx.rs`, ...)
- Each converter implements the `Converter` trait (see PRD Section 3.2)
- `src/markdown.rs` — shared Markdown generation utilities (table builder, heading formatter)
- `src/detection.rs` — file format detection by extension and magic bytes
- `src/error.rs` — `ConvertError` enum

### Dependencies
- MVP dependencies: `zip`, `quick-xml`, `calamine`, `csv`, `serde_json`, `thiserror`
- Every dependency MUST be pure Rust (no C bindings) unless absolutely unavoidable
- Minimize dependency count — do not add a crate for something achievable in <50 lines

### Testing — TDD Required

**All features MUST be developed using Test-Driven Development (TDD):**
1. Write a failing test first that defines the expected behavior
2. Implement the minimum code to make the test pass
3. Refactor while keeping tests green

**Unit tests:**
- Every converter must have unit tests inside the module (`#[cfg(test)] mod tests`)
- Test individual parsing functions: heading extraction, table parsing, bold/italic detection, image extraction, hyperlink resolution, list parsing, etc.
- Cover edge cases: empty documents, single-cell tables, missing XML elements, malformed content, deeply nested structures, Unicode/CJK text
- Markdown utility functions (`markdown.rs`) must be fully unit-tested: table builder, heading formatter, list formatter, text escaping

**Integration tests:**
- Live in `tests/` with sample files in `tests/fixtures/`
- Test end-to-end conversion: file in → Markdown out
- One test file per format minimum (`test_docx.rs`, `test_pptx.rs`, `test_xlsx.rs`, etc.)
- Test against expected Markdown output patterns, not exact string matches
- Include a comparison test that verifies anytomd-rs output covers the same content as MarkItDown output for the same input file

**Test fixtures:**
- Sample documents live in `tests/fixtures/` (committed to the repo)
- Create minimal but representative test files for each format
- Include both simple cases (plain text only) and complex cases (tables + images + headings + lists)

**Test commands:**
```bash
cargo test              # Run all tests
cargo test --lib        # Unit tests only
cargo test --test '*'   # Integration tests only
```

---

## Development Workflow — Build-Test-Verify Loop

**After every minimal unit of work is completed, you MUST run the full verification loop before moving on to the next task.**

A "minimal unit of work" includes but is not limited to:
- Implementing a single parsing function (e.g., heading extraction from DOCX)
- Adding a new converter module
- Modifying the `Converter` trait or public API
- Adding or changing a dependency
- Refactoring existing code

**Verification loop (run every time):**
```bash
cargo build              # 1. Does it compile?
cargo test               # 2. Do all tests pass (including pre-existing ones)?
cargo clippy -- -D warnings  # 3. Any lint warnings?
```

**Rules:**
- Do NOT proceed to the next task if any step in the loop fails
- Fix the failure first, re-run the full loop, then continue
- If a new test was written (TDD), confirm it fails before implementation, then confirm it passes after
- After completing a full converter (e.g., entire DOCX support), also run `cargo fmt --check` and `cargo build --release` before considering it done
- This loop is non-negotiable — skipping it to "save time" leads to cascading failures

---

## CI — GitHub Actions

A GitHub Actions workflow (`.github/workflows/ci.yml`) **MUST be set up** and kept passing at all times. Every push and pull request must be validated.

**Required CI checks:**
```yaml
# The CI workflow must include ALL of the following steps:
- cargo fmt --check          # Format check — no unformatted code
- cargo clippy -- -D warnings  # Lint check — zero warnings
- cargo test                 # All unit + integration tests must pass
- cargo build --release      # Release build must succeed
```

**CI matrix:** Run on all three target platforms:
- `ubuntu-latest`
- `macos-latest`
- `windows-latest`

**Rules:**
- Never merge code that breaks CI
- If a new converter is added without tests, CI should be considered incomplete — add tests before merging
- CI must use the latest Rust stable toolchain (`dtolnay/rust-toolchain@stable`)
