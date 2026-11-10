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

# Rebuild and relaunch the desktop app on every save (needs `cargo install cargo-watch --locked`).
# Only the changed crate recompiles, so a UI edit is ~10s; keep rust-analyzer on its own
# target dir (`rust-analyzer.cargo.targetDir: true`) so it never invalidates this build.
watch:
    cargo watch -w apps/desktop/src -w crates/magi-core/src -x "run -p magi-desktop"

# Run the desktop app with onboarding reset to its first step (folders and features stay)
onboarding:
    cargo run -p magi-desktop -- --onboarding

# Photograph every screen (light/dark, en/es) into <target>/shots/ and compare with the last run;
# open <target>/shots/index.html. `just shots --only ask,settings --theme dark --lang es` retakes some
shots *args:
    cargo run -p magi-desktop --features shots --bin magi-shots -- {{args}}

# Run formatting, lints and all tests, then the Ask Magi Worker's
check: && server-check
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

# Test and type-check the Ask Magi Worker (run `npm ci` in server/ once first)
[working-directory: 'server']
server-check:
    npm test
    npm run typecheck

# Run all tests with a fake embedder (no model downloads required)
test:
    cargo test --workspace

# Download required models into the dev data directory
models:
    cargo run -p xtask -- fetch-models

# Run evaluations using the CLI against test fixtures
eval:
    cargo run -p magi-cli --release -- eval eval/queries.jsonl --corpus fixtures/corpus

# The same queries over local copies of the corpus with random file names
# (fixtures/random_names/, not in git): search when names don't help.
# `just eval-random realistic_names`: the same files under camera-style names
eval-random names="random_names":
    cargo run -p magi-cli --release -- eval eval/{{names}}.jsonl --corpus fixtures/{{names}}

# Run every eval on the current code and save the reports as target/eval/<run>/
eval-all run:
    cargo build --release -p magi-cli
    cargo run -p xtask --release -- eval-all {{run}}

# What moved between two eval-all runs, and every regression (eval/README.md)
eval-diff before after:
    cargo run -p xtask --release -- eval-diff {{before}} {{after}}

# Run the 100k-synthetic-chunk NFR-2/NFR-3 search-latency benchmark
bench:
    cargo run -p xtask --release -- bench-corpus

# Regenerate THIRD_PARTY_LICENSES.md (needs: cargo install --locked cargo-about --features cli)
licenses:
    cargo run -p xtask -- licenses

# Build the desktop app in release mode (installers: SPEC.md §9 Q9)
build:
    cargo build -p magi-desktop --release

# Installer for this OS into target/packages (ADR-0012; needs `cargo install cargo-packager --locked`)
package formats="default":
    cargo run -p xtask -- stage-libs
    cargo packager --release --formats {{formats}}
