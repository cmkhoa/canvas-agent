#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required. Install Rust 1.88+ from https://rustup.rs" >&2
  exit 1
fi

if [[ ! -f .env ]]; then
  cp .env.example .env
  chmod 600 .env
  echo "Created .env from .env.example. Edit it before running the planner."
fi

cargo fetch --locked
cargo build --locked
echo
echo "Setup complete. Next:"
echo "  1. edit .env (LLM_API_KEY, STUDENT_TIMEZONE, CANVAS_MCP_COMMAND)"
echo "  2. cargo run --locked -- --once \"What should I prioritize this week?\""
