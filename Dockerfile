# syntax=docker/dockerfile:1

# ============================================================
# anytomd-rs Development Container
# ============================================================
# Provides a reproducible Linux build/test environment.
# Usage: see docker-compose.yml or the "Docker Development" section in CLAUDE.md.

ARG RUST_VERSION=1.90.0

# ------------------------------------------------------------
# Stage 1: chef — install cargo-chef for dependency caching
# ------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ------------------------------------------------------------
# Stage 2: planner — generate a dependency recipe
# ------------------------------------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ------------------------------------------------------------
# Stage 3: builder — build dependencies (cached), then the project
# ------------------------------------------------------------
FROM chef AS builder

# Install clippy and rustfmt components
RUN rustup component add clippy rustfmt

# Build dependencies first (cached as long as recipe.json doesn't change)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --recipe-path recipe.json
RUN cargo chef cook --recipe-path recipe.json --release

# Copy the full source tree
COPY . .

# Default command: run the full verification loop
CMD ["sh", "-c", "cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo build --release"]
