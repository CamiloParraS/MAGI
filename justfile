# Recipes stay shell-agnostic: one command per line, no `cd` chaining, and
# per-recipe `[working-directory]` instead, so the same justfile runs under
# PowerShell on Windows and `sh` on macOS/Linux (SPEC.md §5.2).
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# List available recipes
default:
    @just --list

# Set up development environment and dependencies
setup: install-frontend
    cargo run -p xtask -- fetch-pdfium
    cargo run -p xtask -- fetch-onnxruntime

[working-directory: 'apps/desktop']
install-frontend:
    pnpm install

# Start the development server
[working-directory: 'apps/desktop']
dev:
    pnpm tauri dev

# Run all code formatting, linting, and type checks
check: check-rust check-frontend

check-rust:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

[working-directory: 'apps/desktop']
check-frontend:
    pnpm lint
    pnpm typecheck
    pnpm test

# Run Rust and frontend tests with a fake embedder (no model downloads required)
test: test-rust test-frontend

test-rust:
    cargo test --workspace

[working-directory: 'apps/desktop']
test-frontend:
    pnpm test

# Regenerate ts-rs bindings (they are written by `cargo test`) and fail if they changed
bindings:
    cargo test -p magi-core export_bindings
    git diff --exit-code -- apps/desktop/src/bindings

# Download required models into the dev data directory
models:
    cargo run -p xtask -- fetch-models

# Run evaluations using the CLI against test fixtures
eval:
    cargo run -p magi-cli --release -- eval eval/queries.jsonl --corpus fixtures/corpus

# Run the 100k-synthetic-chunk NFR-2/NFR-3 search-latency benchmark
bench:
    cargo run -p xtask --release -- bench-corpus

# Build the production desktop application
[working-directory: 'apps/desktop']
build:
    pnpm tauri build
