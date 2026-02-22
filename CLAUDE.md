# CLAUDE.md

## Project Overview

**anytomd** (repository: `anytomd-rs`) is a pure Rust reimplementation of Microsoft's [MarkItDown](https://github.com/microsoft/markitdown) Python library. It converts various document formats (DOCX, PPTX, XLSX, PDF, HTML, CSV, JSON, etc.) into Markdown, targeting LLM consumption. A single `cargo add anytomd` with zero external runtime.

**Current phase:** 9 converters (DOCX, PPTX, XLSX, XLS, HTML, CSV, JSON, XML, Images, Plain Text) + async image description + Gemini integration. See [TECH_SPEC.md](./TECH_SPEC.md) for full architecture and milestones.

---

## Reference-Driven Development

MarkItDown Python source is at `reference/markitdown/` (gitignored) for analysis during development.

**Reference paths:**
- Converters: `reference/markitdown/packages/markitdown/src/markitdown/converters/`
- Utilities: `reference/markitdown/packages/markitdown/src/markitdown/converter_utils/`
- Main engine: `reference/markitdown/packages/markitdown/src/markitdown/_markitdown.py`

**Per-converter workflow:** Read the Python converter → identify extracted elements → create test fixtures → TDD (red/green/refactor) → compare output against MarkItDown for content parity (not exact match).

Do NOT translate Python line-by-line — understand the *intent*, then implement idiomatically in Rust.

---

## Critical Principle: Native Rust Implementation

**Every converter MUST be pure Rust. No Python. No C bindings. No subprocess calls. No external runtime.**

- DOCX/PPTX: `zip` + `quick-xml` (direct OOXML→Markdown, no intermediate HTML)
- XLSX: `calamine` — PDF: `pdf-extract`/`lopdf` — HTML: `scraper`
- If no pure Rust solution exists, **implement in Rust** or **defer** — never add a non-Rust dependency

**Output goal:** LLM-readable text extraction, not pixel-perfect Markdown. Simpler output that captures content is preferred. Must preserve Unicode (CJK, emoji) without corruption.

---

## LLM Integration — Gemini

Optional LLM-based image description via the `ImageDescriber` trait (Technical Spec §4.9). The library makes no HTTP calls — callers inject their own implementation. Trait is provider-agnostic; Gemini is the default.

- Default model: **`gemini-3-flash-preview`** (production) / **`gemini-2.5-flash-lite`** (CI, cost savings)
- Always consult the [official Gemini API docs](https://ai.google.dev/gemini-api/docs) — do NOT rely on cached knowledge
- `GeminiDescriber`: `new(api_key)` or `from_env()` (reads `GEMINI_API_KEY`). Never hardcode/log/persist API keys.
- `AsyncGeminiDescriber`: async counterpart using `reqwest` (behind `async-gemini` feature). Same API: `new()`, `from_env()`, `with_model()`.

### Async Image Description

The library supports concurrent image description via feature flags:

| Feature | Dependencies | Description |
|---------|-------------|-------------|
| `async` | `futures-util` | `AsyncImageDescriber` trait, `AsyncConversionOptions`, `convert_file_async()`, `convert_bytes_async()` |
| `async-gemini` | `async` + `reqwest` | `AsyncGeminiDescriber` for concurrent Gemini API calls |

**Architecture:** Two-phase conversion — `convert_inner()` parses the document and collects image placeholders, then `resolve_image_placeholders_async()` resolves all descriptions concurrently via `futures_util::future::join_all`. No `tokio` in library deps — caller provides the runtime.

**Key types:**
- `AsyncImageDescriber` trait: uses `Pin<Box<dyn Future>>` for dyn-compatibility (async fn in traits not dyn-safe in Rust 1.90)
- `AsyncConversionOptions`: wraps `ConversionOptions` with `async_image_describer: Option<Arc<dyn AsyncImageDescriber>>`

### CI Gemini Testing

Gemini CI tests do NOT run on every PR to prevent API quota abuse.

| Event | Runs? | Reason |
|-------|-------|--------|
| `push` (any branch) | Yes | Owner/collaborators only — trusted |
| `pull_request` (default) | No | External PRs — gated |
| `pull_request` + `ci:gemini` label | Yes | Owner explicitly approved after code review |

**Key rules:**
- `GEMINI_API_KEY` stored as GitHub Actions repository secret
- CI condition: `if: github.event_name == 'push' || contains(github.event.pull_request.labels.*.name, 'ci:gemini')`
- Fork PRs with `ci:gemini`: use `pull_request_target` with `ref: ${{ github.event.pull_request.head.sha }}` — only after code review
- CI tests use `GeminiDescriber::new(api_key).with_model("gemini-2.5-flash-lite".to_string())` and only assert non-empty response (LLM output is non-deterministic)
- Gemini tests must be **additive** — existing tests must pass without the secret
- Gemini test failures (rate limits, transient errors) must NOT block CI — allowed-to-fail
- **Never add `ci:gemini` label without reviewing the PR diff first**

---

## Language Rules

**All project artifacts MUST be written in English.** No exceptions — source code, comments, commit messages, docs, error messages, test names, issues, and PRs.

---

## Git Configuration

- All commits must use the local git config `user.name` and `user.email` for both author and committer. Verify with `git config user.name` and `git config user.email` before committing.
- All commits must include `Signed-off-by` line to pass DCO check (always use `git commit -s`). The `Signed-off-by` name must match the commit author.
- The expected git `user.name` is `Yonghye Kwon`. If the local git config `user.name` does not match, you **MUST** ask the user to confirm their identity before the first commit or push in the session. Once confirmed, do not ask again for the rest of the session.

## Branching & PR Workflow

- **All changes MUST go through a PR** — never commit directly to `main`, including doc-only edits
- Branch naming: `<type>/<short-description>` (e.g., `feat/add-docx-converter`, `fix/table-parsing-bug`)
- One focused unit of work per branch. For existing PRs, push to that branch instead of creating a new PR.
- For fork PRs: `git remote add <user> <fork-url>` and push to that remote's branch

**Worktree workflow (mandatory for PR branch changes):**
- Create: `git worktree add ../anytomd-rs-<branch-name> -b <type>/<short-description>`
- Work and run all PR commands (`gh pr create`, `git push`, etc.) **from inside the worktree**, not the main repo
- Do NOT remove a worktree while your working directory is inside it — return to main repo first: `cd /Users/yhkwon/Documents/Projects/anytomd-rs && git worktree remove ../anytomd-rs-<branch-name>`
- Do NOT remove a worktree immediately after completing a task — only when starting a new task or user confirms
- `git checkout`/`git switch` may be used only for local-only inspection tasks (no PR changes)

### PR Merge Procedure

Follow all steps in order — do not skip any.

1. **Review PR description** — rewrite with `gh pr edit` if empty/lacking. Include what changed, why, key changes.
2. **Search related issues** — `gh issue list`, reference with "Related: #N" (no auto-close keywords unless instructed)
3. **Check conflicts** — if `main` advanced, use `git merge-tree` to check; rebase/merge to resolve if needed
4. **Wait for CI** — `gh pr checks <number> --watch`. If CI fails, do NOT merge.
5. **Final review** — `gh pr diff <number>`, check for debug code, hardcoded paths, secrets, unused imports. Mandatory even if CI is green.
6. **Merge** — `gh pr merge <number> --merge` (**NEVER** use `--delete-branch` — worktree still uses the branch)
7. **Update local main** — `cd /Users/yhkwon/Documents/Projects/anytomd-rs && git pull`

---

## Toolchain

- MSRV is pinned by `rust-version` in `Cargo.toml` — stable only, no nightly features
- Do not bump `rust-version` in unrelated PRs — use a dedicated chore PR

### MSRV Policy — 6-Month Rolling Minimum

This project follows a **6-month rolling MSRV policy** (aligned with [tokio](https://crates.io/crates/tokio) and other major crates):

- The `rust-version` in `Cargo.toml` MUST target a Rust stable release that was published **at least 6 months ago**
- Rust stable releases ship every 6 weeks — consult [releases.rs](https://releases.rs/) for exact dates
- When a newer Rust version crosses the 6-month threshold, updating the MSRV is **allowed but not required** — only bump when a newer language feature or dependency demands it
- **Floor:** the MSRV can never go below the minimum required by `edition` in `Cargo.toml` (edition 2024 = Rust 1.85)

**Before any MSRV change:**
1. Verify no language features or APIs exclusive to versions above the target are used
2. Confirm all dependencies compile on the target version (`cargo check` with the target toolchain, or review dependency MSRV metadata)
3. Update CI matrix to include the new MSRV version
4. Update Dockerfile `RUST_VERSION` ARG to match

---

## Docker Development Environment

Optional Docker setup for reproducible Linux builds. Native `cargo` is the primary workflow; Docker is for cross-platform verification and CI simulation.

**Services:** `docker compose run --rm <service>`

| Service | Description |
|---------|-------------|
| `verify` | Full loop: fmt + clippy + test + release build |
| `test` / `test-lib` / `test-integration` | All / unit / integration tests |
| `lint` / `fmt` | clippy+fmt check / auto-format |
| `build` / `release` | Debug / release build |
| `shell` | Interactive bash |

**Key details:**
- Source is bind-mounted at `/app`; `cargo-chef` caches deps; named volumes persist `target/`, cargo registry/git
- Dockerfile `RUST_VERSION` ARG **MUST match** `rust-version` in `Cargo.toml` — update both in the same commit
- Cleanup: `docker compose down -v` / rebuild: `docker compose build --no-cache`

---

## Code Conventions

### Rust Style
- Follow standard Rust conventions (`rustfmt` defaults, `clippy` clean)
- Use `thiserror` for error types — see `ConvertError` in Technical Spec Section 7
- Prefer returning `Result<T, ConvertError>` over panicking
- Conversion should be **best-effort**: if a single element (e.g., one corrupted table) fails to parse, skip it and continue — do not fail the entire document
- Best-effort behavior must be observable: append structured warnings to `ConversionResult.warnings` instead of silently dropping parse failures

### Crate Structure
- `src/lib.rs` — public API (`convert_file`, `convert_bytes`)
- `src/converter/` — one module per format (`docx.rs`, `pptx.rs`, `xlsx.rs`, ...)
- Each converter implements the `Converter` trait (see Technical Spec Section 3.2)
- Public API must include conversion options (resource limits, strict mode) and warning output
- `src/markdown.rs` — shared Markdown generation utilities (table builder, heading formatter)
- `src/detection.rs` — file format detection by extension and magic bytes
- `src/error.rs` — `ConvertError` enum

### Dependencies
- MVP: `zip`, `quick-xml`, `calamine`, `csv`, `serde_json`, `thiserror` — all pure Rust (no C bindings)
- Minimize deps — do not add a crate for something achievable in <50 lines
- **Always verify latest stable version** on [crates.io](https://crates.io/) before adding/upgrading. If latest fails (MSRV, breaking changes), downgrade and add a comment in `Cargo.toml` explaining why.
- **Opportunistic upgrades:** when touching a module, check if its deps have newer stable versions and upgrade as part of the work

### Testing — TDD Required

**TDD is mandatory for all features and bug fixes:** write failing test → implement minimum code to pass → refactor. Bug fixes must always include a regression test.

**Test integrity:** NEVER delete/modify passing tests or use `#[ignore]` to work around failures — fix the code. Obsolete tests require documented justification in the commit message.

**Naming:** `test_<what>_<condition>_<expected>` or `test_<what>_<scenario>` (e.g., `test_table_parsing_empty_cells_preserved`)

**Unit tests** (`#[cfg(test)] mod tests` in each module):
- Every converter: heading extraction, table parsing, bold/italic, images, links, lists, etc.
- Edge cases: empty docs, malformed content, deeply nested structures, CJK/emoji Unicode
- Every public function and non-trivial private function must have at least one test

**Integration tests** (`tests/` with fixtures in `tests/fixtures/`):
- One test file per format minimum, testing end-to-end: file in → Markdown out
- **Golden tests:** expected output in `tests/fixtures/expected/`. Normalize whitespace before `assert_eq!`. Update golden files with documented reason when output intentionally changes.
- **Content coverage tests:** use `contains`/regex for key elements (more resilient to formatting changes)
- Include comparison against MarkItDown output for content parity (not exact match)

**Fixtures:** minimal, representative files per format. Binary formats (DOCX/PPTX/XLSX): build programmatically or create in LibreOffice/Google Docs. Document contents in test comments.

**Commands:** `cargo test` (all) / `cargo test --lib` (unit) / `cargo test --test '*'` (integration)

---

## Development Workflow — Build-Test-Verify Loop

**Run after every code change in `src/` or `tests/`** (optional for doc-only `*.md` changes):

- For every feature update, review `README.md` and update it in the same PR. Analyze what must be modified, added, or removed so README stays aligned with the current behavior.

```bash
cargo build && cargo test && cargo clippy -- -D warnings
```

After completing a full converter, also run `cargo fmt --check` and `cargo build --release`.

**Non-negotiable:** Do NOT proceed if any step fails — fix first, re-run, then continue. Never delete/ignore/weaken tests to pass the loop.

---

## Release Management

GitHub releases and crates.io versions **MUST stay in sync**. The single source of truth for the version is `Cargo.toml`.

### Release Procedure

Follow all steps in order — do not skip any.

1. **Verify unreleased changes exist** — `git log v<latest-tag>..HEAD --oneline`. If empty, there is nothing to release.
2. **Determine version bump** — follow [SemVer](https://semver.org/):
   - **patch** (`0.x.Y`): bug fixes, test-only changes, doc fixes
   - **minor** (`0.X.0`): new features, new converters, new public API, non-breaking changes
   - **major** (`X.0.0`): breaking API changes (defer until post-1.0)
3. **Bump version in `Cargo.toml`** — update `version = "..."` in `[package]`
4. **Create PR** — branch `chore/release-vX.Y.Z`, commit message `chore: release vX.Y.Z`
5. **Merge PR** — follow standard PR Merge Procedure above
6. **Publish to crates.io** — from `main` after merge: `cargo publish`
7. **Create GitHub release** — `gh release create vX.Y.Z --title "vX.Y.Z" --generate-notes --latest` (from `main` at the merge commit)
8. **Verify sync** — confirm `cargo search anytomd` matches the GitHub release tag and `Cargo.toml`

### Version Sync Rules

- **One version, three places**: `Cargo.toml` (source of truth) = crates.io = GitHub release tag
- **Never publish to crates.io without a matching GitHub release** and vice versa
- **Never manually edit crates.io metadata** — always go through `Cargo.toml` + `cargo publish`
- **Tag format**: always `vX.Y.Z` (e.g., `v0.6.0`), created automatically by `gh release create`
- **Cargo.lock**: committed to the repo for reproducible builds; updated automatically by version bump
- If a release is partially completed (e.g., crates.io published but GitHub release missing), fix immediately — do not leave versions out of sync
- **Yanking**: if a broken version is published to crates.io, yank it with `cargo yank --version X.Y.Z` and document in the next release notes

---

## CI — GitHub Actions

CI must pass on every push/PR. Matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`. Stable Rust matching `rust-version`.

**Required checks:** `cargo fmt --check` → `cargo clippy -- -D warnings` → `cargo test` → `cargo build --release`

**Async feature checks** (every push/PR):
`cargo clippy --features async -- -D warnings` → `cargo test --features async` → `cargo clippy --features async-gemini -- -D warnings` → `cargo test --features async-gemini`

**Gemini checks** (on `push` or `ci:gemini` labeled PRs only):
`cargo test --test test_gemini_live` (allowed-to-fail — depends on `GEMINI_API_KEY` secret)

**Rules:** Never merge code that breaks CI. Gemini live API failures do not block merging. New converters without tests = incomplete CI.
