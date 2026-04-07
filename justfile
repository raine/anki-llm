# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run all checks
check: _fix _verify

# Phase 1: auto-fix (parallel)
[parallel]
_fix: format clippy-fix

# Phase 2: verify (parallel)
[parallel]
_verify: clippy test

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

# Format Rust files
format:
    @cargo fmt --all

# Run clippy and fail on any warnings
clippy:
    @cargo clippy --quiet --color always -- -D clippy::all && echo "clippy: ok"

# Auto-fix clippy warnings
clippy-fix:
    @cargo clippy --quiet --color always --fix --allow-dirty -- -W clippy::all

# Build the project
build:
    cargo build --all

# Run tests
test:
    #!/usr/bin/env bash
    set -euo pipefail
    output=$(cargo test --quiet --color always 2>&1) || { echo "$output"; exit 1; }
    passed=$(grep -oE '[0-9]+ passed' <<< "$output" | awk '{s+=$1} END{print s}')
    echo "test: ok. ${passed} passed"

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

# Run the application
run *ARGS:
    cargo run -- "$@"

# Release a new patch version
release-patch:
    @just _release patch

# Release a new minor version
release-minor:
    @just _release minor

# Release a new major version
release-major:
    @just _release major

# Internal release helper
_release bump:
    @cargo-release {{bump}}
