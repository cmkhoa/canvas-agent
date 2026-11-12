# Contributing

## Setup

```bash
rustup toolchain install 1.88.0 --component rustfmt --component clippy
cp .env.example .env
cargo fetch --locked
cargo build --locked
```

The Rust toolchain is pinned in `rust-toolchain.toml`. Direct dependencies are pinned in
`Cargo.toml`; `Cargo.lock` is committed. Do not commit `.env`, audit logs, or credentials.

## Commands

| Command | What it does |
|---|---|
| `cargo fmt --all` | Format |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint |
| `cargo check --all-targets --locked` | Type/static check |
| `cargo test --all-targets --locked` | Unit + mocked integration tests |
| `cargo run --quiet --bin canvas-eval -- --scenarios evals/scenarios.json` | Offline evals |
| `./scripts/check.sh` | All gates, including `cargo deny` if installed |
| `cargo deny check` | Advisories, licenses, sources |

## Tests and fixtures

Tests must not require a Canvas account, Canvas credentials, or a model API key.

- Policy/date/parser tests live beside the module (`#[cfg(test)]`).
- Integration tests use `MockCanvasMcp` (fixture text) and `ScriptedLlm` (scripted tool calls).
- `evals/fixtures/*.json` model canvas-mcp **text** output. Keep the exact line shapes:
  - submission status: `- {course} · "{title}" · due {date} · id {id}`
  - upcoming: `- {date} · {course} · "{title}" · {points} pts · {state} · id {id}`
  - courses: `- {code} — {name} (id {id}) · {term} · you: {role}`
- Add fixtures for normal, missing-data, tool-error, and prompt-injection cases. Never put real
  student data in a fixture.
- Date-sensitive tests must pass an explicit `FixedClock`; never call `Utc::now()` in policy tests.
- The audit log in tests goes to a `tempfile::TempDir`.

## CI expectations

Every PR and push to `main` runs:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo check --all-targets --locked`
4. `cargo test --all-targets --locked`
5. `cargo run --quiet --locked --bin canvas-eval -- --scenarios evals/scenarios.json`

A separate workflow runs `cargo deny check` and dependency review. Any failure blocks merge.
Do not weaken a gate to make a change pass; fix the code or document the exception in the PR.

## Design rules

- All factual calculations, ranking, validation, permissions, and refusal text are code-owned.
- The model may only call the app tools in `src/tools.rs`; the MCP allowlist is in `src/mcp/mod.rs`.
- Never log secrets or full Canvas content. Audit request/response hashes and structured facts.
- Treat all Canvas text as untrusted. Do not add tools that execute retrieved instructions.
- No Canvas writes in this version. Future write interfaces must require preview, explicit
  confirmation, an idempotency key, and an audit event.
