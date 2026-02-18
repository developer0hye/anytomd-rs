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

**Workflow per converter (integrates with TDD):**
1. Read the Python converter (e.g., `_docx_converter.py`)
2. Note which document elements it extracts and how it formats them
3. Create test fixtures for the format (see "Test fixtures" under Testing)
4. **Write failing Rust tests first** based on the same expected content coverage (TDD red phase)
5. Implement the Rust converter using native crates to make tests pass (TDD green phase)
6. Refactor while keeping tests green (TDD refactor phase)
7. Compare output against MarkItDown's output for the same test files to ensure content parity (not exact string match)

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

Text extraction quality must include robust Unicode handling: converters must preserve multilingual content (Korean, Chinese, Japanese, and other scripts) and emoji without corruption.

MarkItDown converts DOCX via a two-step process (DOCX → HTML via mammoth → Markdown via markdownify). anytomd-rs should convert **directly from OOXML XML to Markdown in a single step**. This is faster, simpler, and avoids intermediate representation overhead.

---

## LLM Integration — Gemini

anytomd-rs supports optional LLM-based image description via the `ImageDescriber` trait (see PRD §4.9). The library itself does not make HTTP calls or manage API keys — callers inject their own implementation.

**Default LLM provider: Google Gemini**

- Use **Google Gemini** as the default LLM provider for built-in / example implementations
- Default model: **`gemini-3-flash-preview`**
- When developing or updating any Gemini-related code (API calls, authentication, model parameters, request/response formats), **always consult the [official Gemini API documentation](https://ai.google.dev/gemini-api/docs)** for the latest specs — do NOT rely on cached knowledge or outdated examples
- The `ImageDescriber` trait is provider-agnostic: Gemini is the default, but any LLM backend can be used
- **API key management:** The `ImageDescriber` trait has no key concept. The built-in `GeminiDescriber` accepts a key via struct field (`new(api_key)`) and also provides an env-var fallback (`from_env()` reads `GEMINI_API_KEY`). Never hardcode, log, or persist API keys in library code.

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

## Git Configuration

- All commits must use the local git config `user.name` and `user.email` for both author and committer. Verify with `git config user.name` and `git config user.email` before committing.
- All commits must include `Signed-off-by` line to pass DCO check (always use `git commit -s`). The `Signed-off-by` name must match the commit author.
- The expected git `user.name` is `Yonghye Kwon`. If the local git config `user.name` does not match, you **MUST** ask the user to confirm their identity before the first commit or push in the session. Once confirmed, do not ask again for the rest of the session.

## Branching & PR Workflow

- Always create a new branch before starting any task (never work directly on `main`)
- **All changes — including documentation-only edits (`*.md`) — MUST go through a PR.** Never commit directly to `main`.
- **MUST:** create a worktree first, then create/edit files inside that worktree directory
- Branch naming convention: `<type>/<short-description>` (e.g., `feat/add-docx-converter`, `fix/table-parsing-bug`, `ci/add-linting`)
- Once the task is complete, push the branch and create a Pull Request to `main`
- Each branch should contain a single, focused unit of work
- Do not start a new task on the same branch — create a new branch for each task
- **When working on an existing PR** (e.g., fixing issues, adding changes), push commits directly to that PR's branch instead of creating a new PR. Only create a separate PR if explicitly requested. For cross-repository (fork) PRs, add the contributor's fork as a remote (e.g., `git remote add <user> <fork-url>`) and push to that remote's branch.
- **MUST (for PR branch changes):** use `git worktree` to work on multiple branches simultaneously in separate directories
  - For local-only cleanup or inspection tasks that do not create PR changes, `git checkout`/`git switch` may be used when explicitly requested
  - Create a worktree: `git worktree add ../anytomd-rs-<branch-name> -b <type>/<short-description>`
  - Work inside the worktree directory, not the main repo
  - **All PR-related commands (`gh pr create`, `gh pr merge`, `gh pr checks`, `git push`, etc.) must be run from inside the worktree directory**, not the main repo directory. Commands like `gh pr create` use the current directory's branch as the head branch — running them from the main repo (on `main`) will fail with "head branch is the same as base branch".
  - **Do NOT remove a worktree immediately after completing a task.** If you delete the worktree while your working directory is still inside it, all subsequent commands will fail because the path no longer exists. Only remove a worktree after you have confirmed the user wants it removed, or when starting a new task (at which point you will create a new worktree and move into that directory).
  - **When removing a worktree, you MUST return to the main repo directory first.** You cannot remove a worktree while your working directory is inside it. Always run: `cd /Users/yhkwon/Documents/Projects/anytomd-rs && git worktree remove ../anytomd-rs-<branch-name>`

### PR Merge Procedure

Follow these steps in order when merging a PR. Do not skip any step.

**Step 1. Review PR description**
- If the PR description is empty or lacks detail, rewrite it using `gh pr edit <number> --body "..."`.
- A good PR description includes: what changed and why, a list of key changes, and relevant context (e.g., related issues).
- Review the actual commits and code diff to write an accurate description.

**Step 2. Search for related issues**
- Search for related issues using `gh issue list` and the PR's topic/keywords.
- If related issues exist, mention them in the PR description (e.g., "Related: #70, #55").
- Do not use auto-close keywords (`Closes`, `Fixes`) unless explicitly instructed — only reference issues for context. Only mention issues that are directly related to the PR's changes.

**Step 3. Check for conflicts with main**
- Check if `main` has advanced since the PR was created.
- If so, run `git merge-tree` to check for conflicts. If conflicts exist, rebase or merge `main` into the PR branch to resolve them, then push again.

**Step 4. Wait for CI checks to pass**
- Run `gh pr checks <number> --watch` and wait for all CI checks to pass.
- If CI fails, do **not** merge. Report the failure and stop.

**Step 5. Final review before merge**
- Run `gh pr diff <number>` and review every changed file one more time.
- Check for: accidental debug code, hardcoded paths, missing test coverage, unused imports, TODO comments that should have been resolved, secrets or credentials.
- Verify the PR description accurately reflects the final diff (it may have drifted if fixup commits were added).
- This step is mandatory — never skip it, even if CI is green. CI catches compilation and lint issues, but not logic errors or design oversights.

**Step 6. Merge the PR**
- **NEVER use the `--delete-branch` flag.** The worktree is still using the branch, so `--delete-branch` attempts to checkout `main` locally, which fails because `main` is already checked out in the main repo directory.
- Merge command: `gh pr merge <number> --merge`

**Step 7. Update local main**
- Move to the main repo directory and run `git pull`.
  ```bash
  cd /Users/yhkwon/Documents/Projects/anytomd-rs && git pull
  ```
- New worktrees branch off local `main`, so skipping this step causes missing commits in new branches.

---

## Toolchain

- Use the Rust version pinned by `rust-version` in `Cargo.toml` as the project MSRV contract
- Keep CI and local development on stable Rust compatible with that pinned version
- Do not bump `rust-version` in unrelated feature PRs
- Review new Rust stable releases on a regular cadence (for example monthly) and bump `rust-version` only in a dedicated chore PR
- Do NOT use nightly-only features — everything must compile on stable
- `rustup update stable` is optional local maintenance, not a prerequisite for every task

---

## Docker Development Environment

A Docker-based development environment is provided for reproducible builds and tests on Linux, independent of the host OS. This is especially useful for:
- Cross-platform verification without needing native Linux
- Ensuring consistent build results across developer machines
- Running the full verification loop in an isolated environment
- CI-like local validation before pushing

### Prerequisites

- Docker Engine 20.10+ and Docker Compose V2 (`docker compose`)
- On macOS, Docker Desktop or OrbStack

### Quick Start

```bash
# Full verification loop (fmt + clippy + test + release build)
docker compose run --rm verify

# Run tests only
docker compose run --rm test

# Lint only (clippy + fmt check)
docker compose run --rm lint

# Debug build
docker compose run --rm build

# Release build
docker compose run --rm release

# Interactive shell inside the container
docker compose run --rm shell
```

### Available Services

| Service            | Command                                  | Description                              |
|--------------------|------------------------------------------|------------------------------------------|
| `verify`           | `docker compose run --rm verify`         | Full verification loop (fmt + clippy + test + release) |
| `test`             | `docker compose run --rm test`           | Run all tests                            |
| `test-lib`         | `docker compose run --rm test-lib`       | Unit tests only                          |
| `test-integration` | `docker compose run --rm test-integration` | Integration tests only                 |
| `lint`             | `docker compose run --rm lint`           | clippy + fmt check                       |
| `fmt`              | `docker compose run --rm fmt`            | Auto-format code with rustfmt            |
| `build`            | `docker compose run --rm build`          | Debug build                              |
| `release`          | `docker compose run --rm release`        | Release build                            |
| `shell`            | `docker compose run --rm shell`          | Interactive bash shell                   |

### How It Works

- **Source mounting:** The project directory is bind-mounted into the container at `/app`, so code edits on the host are immediately reflected
- **Dependency caching:** `cargo-chef` is used in the Dockerfile to cache compiled dependencies in a separate Docker layer — rebuilds after source-only changes are fast
- **Volume persistence:** Cargo registry, git database, and the `target/` directory are stored in named Docker volumes (`cargo-registry`, `cargo-git`, `cargo-target`), so incremental compilation works across runs
- **Rust version:** The Dockerfile's `RUST_VERSION` ARG should be kept in sync with `rust-version` in `Cargo.toml`

### When to Use Docker vs Native Cargo

| Scenario                                    | Recommended       |
|---------------------------------------------|--------------------|
| Day-to-day development on your own machine  | Native `cargo`     |
| Verifying Linux-specific behavior           | Docker             |
| Pre-push CI simulation                      | Docker `verify`    |
| Debugging a CI failure on Linux             | Docker `shell`     |
| Quick iteration (edit-compile-test cycle)   | Native `cargo`     |
| Ensuring no host-specific dependencies leak | Docker             |

### Cleanup

```bash
# Remove containers and volumes (frees disk space, loses cached builds)
docker compose down -v

# Rebuild from scratch (e.g., after changing Rust version)
docker compose build --no-cache
```

### Rules

- The `RUST_VERSION` ARG in the Dockerfile MUST match the `rust-version` in `Cargo.toml`
- When bumping `rust-version`, update the Dockerfile ARG in the same commit
- Docker is an **optional** development tool — native `cargo` commands remain the primary workflow
- CI uses GitHub Actions (not Docker) — Docker is for local development convenience only

---

## Code Conventions

### Rust Style
- Follow standard Rust conventions (`rustfmt` defaults, `clippy` clean)
- Use `thiserror` for error types — see `ConvertError` in PRD Section 7
- Prefer returning `Result<T, ConvertError>` over panicking
- Conversion should be **best-effort**: if a single element (e.g., one corrupted table) fails to parse, skip it and continue — do not fail the entire document
- Best-effort behavior must be observable: append structured warnings to `ConversionResult.warnings` instead of silently dropping parse failures

### Crate Structure
- `src/lib.rs` — public API (`convert_file`, `convert_bytes`)
- `src/converter/` — one module per format (`docx.rs`, `pptx.rs`, `xlsx.rs`, ...)
- Each converter implements the `Converter` trait (see PRD Section 3.2)
- Public API must include conversion options (resource limits, strict mode) and warning output
- `src/markdown.rs` — shared Markdown generation utilities (table builder, heading formatter)
- `src/detection.rs` — file format detection by extension and magic bytes
- `src/error.rs` — `ConvertError` enum

### Dependencies
- MVP dependencies: `zip`, `quick-xml`, `calamine`, `csv`, `serde_json`, `thiserror`
- Every dependency MUST be pure Rust (no C bindings)
- Minimize dependency count — do not add a crate for something achievable in <50 lines
- **Before adding or upgrading any external crate**, check its latest stable version on [crates.io](https://crates.io/) (e.g., via `cargo search <crate>` or web search) and use that version. Do not blindly copy version numbers from old examples or memory — always verify the current latest stable release at the time of use.
- **Version fallback strategy:** If the latest stable version causes build or test failures (e.g., MSRV incompatibility, breaking API changes, dependency conflicts), downgrade to the previous minor or patch version and retry. When using a non-latest version, add a comment above the dependency in `Cargo.toml` explaining the reason and the blocked version (e.g., `# pinned: foo 3.x requires MSRV 1.80, tracking https://...`).
- **Dependency freshness:** When touching a converter or module that uses an external crate, check whether a newer stable version is available. If so, attempt the upgrade as part of the work. This keeps dependencies from going stale over time — upgrades should happen opportunistically, not only when something breaks.

### Testing — TDD Required

**All features MUST be developed using Test-Driven Development (TDD):**
1. Write a failing test first that defines the expected behavior
2. Implement the minimum code to make the test pass
3. Refactor while keeping tests green

**Bug fixes MUST also follow TDD:**
1. Write a failing test that reproduces the bug
2. Confirm the test fails with the current code
3. Fix the bug to make the test pass
4. Never fix a bug without a regression test

**Test integrity rules:**
- NEVER delete or modify an existing passing test to make code "pass" — fix the code instead
- NEVER use `#[ignore]` to skip failing tests as a workaround — either fix the test or fix the code
- If a test is genuinely obsolete (e.g., API was intentionally redesigned), document the reason in the commit message when removing it

**Test naming convention:**
- Use descriptive snake_case names that read as behavior specifications
- Pattern: `test_<what>_<condition>_<expected>` or `test_<what>_<scenario>`
- Examples:
  - `test_heading_extraction_h1_through_h6`
  - `test_table_parsing_empty_cells_preserved`
  - `test_bold_and_italic_nested`
  - `test_convert_file_nonexistent_path_returns_error`
  - `test_docx_unicode_cjk_text`

**Unit tests:**
- Every converter must have unit tests inside the module (`#[cfg(test)] mod tests`)
- Test individual parsing functions: heading extraction, table parsing, bold/italic detection, image extraction, hyperlink resolution, list parsing, etc.
- Cover edge cases: empty documents, single-cell tables, missing XML elements, malformed content, deeply nested structures, multilingual Unicode text (Korean/Chinese/Japanese), and emoji
- Markdown utility functions (`markdown.rs`) must be fully unit-tested: table builder, heading formatter, list formatter, text escaping
- Every public function and every non-trivial private function must have at least one test

**Integration tests:**
- Live in `tests/` with sample files in `tests/fixtures/`
- Test end-to-end conversion: file in → Markdown out
- One test file per format minimum (`test_docx.rs`, `test_pptx.rs`, `test_xlsx.rs`, etc.)
- Include golden tests for at least one representative fixture per format:
  - Golden files live in `tests/fixtures/expected/` (e.g., `tests/fixtures/expected/sample.docx.md`)
  - Before comparison, normalize both actual and expected output: collapse consecutive whitespace/newlines, trim lines, strip trailing newline — so that insignificant formatting differences do not cause false failures
  - After normalization, compare with exact string match (`assert_eq!`)
  - When a converter's output intentionally changes, update the golden file and document the reason in the commit message
- For content coverage tests (separate from golden tests), use pattern matching (`contains`, regex) to verify that key content elements (headings, table data, links, etc.) are present — this is more resilient to formatting changes than exact matching
- Include a comparison test that verifies anytomd-rs output covers the same content as MarkItDown output for the same input file (use content coverage assertions, not exact match)

**Test fixtures:**
- Sample documents live in `tests/fixtures/` (committed to the repo)
- Create minimal but representative test files for each format
- Include both simple cases (plain text only) and complex cases (tables + images + headings + lists)
- **How to create fixtures for binary formats (DOCX, PPTX, XLSX):**
  - DOCX/PPTX: Build programmatically in Rust test setup using `zip` + XML templates, or create manually in LibreOffice/Google Docs and export — commit the resulting files
  - XLSX: Create manually in LibreOffice/Google Sheets and export, or use a Python one-liner with `openpyxl` to generate — commit the resulting files
  - Keep fixtures as small as possible — only include the elements needed for the specific test
  - Document what each fixture contains in a comment at the top of the test that uses it

**Test commands:**
```bash
cargo test              # Run all tests
cargo test --lib        # Unit tests only
cargo test --test '*'   # Integration tests only
```

---

## Development Workflow — Build-Test-Verify Loop

**For code changes in `src/` or `tests/`, run the full verification loop before moving to the next task.**

For documentation-only changes (`*.md`), running the full Rust loop is recommended but optional.

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
- Do NOT delete, `#[ignore]`, or weaken tests to make the loop pass — fix the code
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
- CI must use a stable Rust toolchain compatible with `rust-version` in `Cargo.toml`
