# Recipes stay shell-agnostic: one command per line, no `cd` chaining, and
# per-recipe `[working-directory]` instead, so the same justfile runs under
# PowerShell on Windows and `sh` on macOS/Linux (SPEC.md §5.2).
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# List available recipes
default:
    @just --list

# Set up development environment and dependencies
setup:
    cargo run -p xtask -- fetch-pdfium
    cargo run -p xtask -- fetch-onnxruntime

# Run the desktop app (the GPUI crate lands in M6 Plan 3; ADR-0011)
dev:
    cargo run -p magi-desktop

# Run formatting, lints and all tests
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

# Run all tests with a fake embedder (no model downloads required)
test:
    cargo test --workspace

# Download required models into the dev data directory
models:
    cargo run -p xtask -- fetch-models

# Run evaluations using the CLI against test fixtures
eval:
    cargo run -p magi-cli --release -- eval eval/queries.jsonl --corpus fixtures/corpus

# Run the 100k-synthetic-chunk NFR-2/NFR-3 search-latency benchmark
bench:
    cargo run -p xtask --release -- bench-corpus

# Build the desktop app in release mode (installers: SPEC.md §9 Q9)
build:
    cargo build -p magi-desktop --release
