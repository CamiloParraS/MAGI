set shell := ["powershell.exe", "-Command"]

# List available recipes
default:
    @just --list

# Set up development environment and dependencies
setup:
    cd apps/desktop; pnpm install
    cargo xtask fetch-pdfium

# Start the development server
dev:
    cd apps/desktop; pnpm tauri dev

# Run all code formatting, linting, and type checks
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace
    cd apps/desktop; pnpm lint
    cd apps/desktop; pnpm typecheck
    cd apps/desktop; pnpm test

# Run Rust and frontend tests with a fake embedder (no model downloads required)
test:
    cargo test --workspace
    cd apps/desktop; pnpm test

# Regenerate ts-rs bindings and verify no unexpected git diff exists
bindings:
    cargo test --workspace --test generate_bindings
    git diff --exit-code

# Download required models into the dev data directory
models:
    cargo xtask fetch-models

# Run evaluations using the CLI against test fixtures
eval:
    cargo run -p magi-cli --release -- eval eval/queries.jsonl --corpus fixtures/corpus

# Build the production desktop application
build:
    cd apps/desktop; pnpm tauri build
