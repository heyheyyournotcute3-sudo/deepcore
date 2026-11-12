#!/usr/bin/env bash
# Deepcore quality gate: formatting, lints and tests for the whole workspace.
# Run this before closing every milestone.
set -euo pipefail

cd "$(dirname "$0")/.."

export PATH="${HOME}/.cargo/bin:${PATH}"

echo "== cargo fmt --check =="
cargo fmt --all -- --check

echo "== cargo clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== cargo test =="
cargo test --workspace

echo "All checks passed."
