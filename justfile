# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run all read-only project checks
check:
    @checkle run all

# Run check and fail if there are uncommitted changes (for CI)
check-ci: check
    #!/usr/bin/env bash
    set -euo pipefail
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "Error: check caused uncommitted changes"
        echo "Run 'just check' locally and commit the results"
        git diff --stat
        exit 1
    fi

# Install local tools used by quality gates
install-quality-tools:
    cargo install checkle --locked

# Format Rust files
format:
    @cargo fmt --all

# Check Rust formatting without changing files
format-check:
    @checkle run format-check

# Run clippy and fail on any warnings
clippy:
    @checkle run clippy

# Auto-fix clippy warnings
clippy-fix:
    @cargo clippy --fix --allow-dirty --target-dir target/clippy --all-targets -- -D warnings -W clippy::all

# Build the project
build:
    cargo build --all

# Run tests
test:
    @checkle run test

# Run integration tests against a disposable Anki Docker container
test-integration *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    docker build -q -t anki-test ./docker
    docker run --rm -d -p 8765:8765 --name anki-test anki-test
    cleanup() { docker stop anki-test > /dev/null 2>&1 || true; }
    trap cleanup EXIT
    echo "Waiting for AnkiConnect..."
    for i in $(seq 1 30); do
        curl -s http://127.0.0.1:8765 -X POST -d '{"action":"version","version":6}' > /dev/null 2>&1 && break
        sleep 1
    done
    cargo test --test anki_integration --features integration -- --test-threads=1 "$@"

# Install release binary globally
install:
    cargo install --offline --path . --locked

# Install debug binary globally via symlink
install-dev:
    cargo build && ln -sf $(pwd)/target/debug/anki-llm ~/.cargo/bin/anki-llm

# Run the docs development server
docs:
    #!/usr/bin/env bash
    set -euo pipefail
    bun install --cwd docs --frozen-lockfile
    preferred_port=4321
    max_port=$((preferred_port + 50))
    for ((port = preferred_port; port <= max_port; port++)); do
        if ! nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
            exec bun run --cwd docs dev --port "$port"
        fi
    done
    echo "Error: could not find an available docs port between ${preferred_port} and ${max_port}" >&2
    exit 1

# Build the documentation site
docs-build:
    bun install --cwd docs --frozen-lockfile
    bun run --cwd docs build

# Run the application
run *ARGS:
    cargo run -- "$@"

# Internal release helper
_release bump:
    @cargo-release {{bump}}

# Release a new patch version
release *ARGS:
    @just _release patch {{ARGS}}
