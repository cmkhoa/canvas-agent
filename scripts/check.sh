#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> cargo test"
cargo test --all-targets --locked

echo "==> deterministic evaluation suite"
cargo run --quiet --locked --bin canvas-eval -- --scenarios evals/scenarios.json

if command -v cargo-deny >/dev/null 2>&1; then
  echo "==> cargo deny"
  cargo deny check
else
  echo "==> cargo deny skipped (install with: cargo install cargo-deny --locked)"
fi
