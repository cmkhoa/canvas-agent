# Canvas Academic Planner

A small terminal agent that turns the read-only [canvas-mcp](https://github.com/cmkhoa/canvas-mcp)
tools into a planning assistant for a student:

- "What should I prioritize this week?"
- "What assignments am I most at risk of missing?"
- "Summarize my upcoming workload by course."
- "What do I need to finish before Friday?"

It is a **separate application**. It does not reimplement Canvas API calls and it does not
duplicate canvas-mcp's 46 tools. It launches the existing MCP server over stdio and calls a
curated allowlist of its read-only tools.

> Status: local-first reference implementation. No deployment, no Canvas writes.

## Quick start

Requirements: Rust 1.88 (pinned by `rust-toolchain.toml`), a working `canvas-mcp`
installation signed in to your own account, and an OpenAI-compatible chat model.

```bash
git clone <this-repo> && cd canvas-agent
./scripts/setup.sh          # creates .env, fetches locked deps, builds
$EDITOR .env                # LLM_API_KEY, STUDENT_TIMEZONE, CANVAS_MCP_COMMAND
cargo run --locked -- --once "What should I prioritize this week?"
```

Interactive terminal session:

```bash
cargo run --locked
```

Reproducible clock (affects all policy and rendering, never Canvas's own server-side window):

```bash
cargo run --locked -- --now 2026-11-12T09:25:00-06:00 \
  --once "What do I need to finish before Friday?"
```

Offline deterministic evaluation suite (no Canvas account or model key):

```bash
cargo run --quiet --locked --bin canvas-eval -- --scenarios evals/scenarios.json
```

## Environment

Copy `.env.example` to `.env`. Required variables:

| Variable | Purpose |
|---|---|
| `LLM_API_KEY` | Model provider key. Never logged or committed. |
| `LLM_BASE_URL` | OpenAI-compatible base URL, default `https://api.openai.com/v1`. |
| `LLM_MODEL` | Tool-calling chat model, default `gpt-4.1-mini`. |
| `STUDENT_TIMEZONE` | IANA zone used to read and display Canvas dates (e.g. `America/Chicago`). |
| `CANVAS_MCP_COMMAND` | Command that starts canvas-mcp (default `canvas-mcp`). |
| `CANVAS_MCP_ARGS` | Optional whitespace-separated args, e.g. `--env-file /path/to/canvas-mcp.env`. |
| `CANVAS_AGENT_NOW` | Optional deterministic clock: RFC 3339 with offset or `YYYY-MM-DD`. |
| `CANVAS_AGENT_AUDIT_LOG` | Append-only JSONL audit path (default `logs/audit.jsonl`). |
| `CANVAS_AGENT_POLICY` | Policy file (default `config/policy.toml`). |

Canvas credentials belong to canvas-mcp. Configure its own `~/.config/canvas-mcp/.env`, or
export `CANVAS_API_TOKEN` before launching the planner; the MCP child process inherits it.
Do not put real credentials in a tracked file.

**Timezone contract:** canvas-mcp formats deadline text in its `CANVAS_TIMEZONE`. Set
`CANVAS_TIMEZONE` and `STUDENT_TIMEZONE` to the same IANA zone. If the abbreviation in a tool
result does not match `STUDENT_TIMEZONE`, the parser fails closed instead of guessing an offset.

## Architecture

```text
User
  │
  ▼
canvas-planner (Rust)
  ├── agent loop (raw OpenAI-compatible HTTP; no agent framework)
  ├── app tools: plan_priorities, assess_deadline_risk, summarize_workload,
  │              get_course_list, get_assignment_detail, refuse_request
  ├── deterministic engine: date parser, merge, policy, ranking, citations
  ├── validation, session state, append-only JSONL audit
  ▼
canvas-mcp (existing Rust MCP server, stdio JSON-RPC)
  ▼
Canvas API (read-only)
```

The model never sees raw Canvas HTTP. It sees curated app-tool results and Canvas free text is
passed through as untrusted data (canvas-mcp already fences instructor/classmate text).

### Model-owned vs code-owned

| Model decides | Code decides |
|---|---|
| Intent interpretation ("before Friday" → weekday tool argument) | Resolving that weekday/date in the student's zone |
| Which allowlisted planning tool to call | Which MCP tools are allowed and what arguments are valid |
| Short student-facing explanation and follow-up question | Every due date, point value, status merge, score, rank, risk tier |
| Whether a request is out of scope | Refusal text, audit event, and tool permissions |
| | Citations, sources, validation, deterministic appendix, session state |

The final answer is the model's short narrative **plus** code-rendered factual tables and a
source list. The model cannot change the tables; it can only cite their `[S#]` sources.

## Priority policy

All thresholds are in [`config/policy.toml`](config/policy.toml), validated at startup.

```text
score = deadline proximity (0–120)
      + point value (0–30: min(points, 100)/100 * 30)
      + Canvas submission status (0–40 missing, 15 not submitted, 5 unknown)
```

- Overdue work scores higher than work due today, which scores higher than work due next week.
- Canvas-flagged **missing** work and past-due unsubmitted work are `critical`.
- Unsubmitted work due within 3 days is `high`; within the horizon is `medium`.
- Work with no due date is `unknown` and is listed separately. It is never assigned a guessed date.
- Work with no point value contributes zero value points and says so.
- Tie-break: earlier due date, then more points, then course, then assignment ID.

This is **deadline risk**, not a grade prediction. The planner does not claim to know a student's
actual grade risk unless Canvas data supports the claim; it does not compute unweighted grade
outcomes.

## Safety and privacy

- **Read-only.** Only 4 canvas-mcp read-only tools are ever called: `list_courses`,
  `get_my_submission_status`, `get_my_upcoming_assignments`, `get_assignment_details`.
  Write tools do not exist in this app. Canvas mutations are out of scope.
- **No identity inference.** Identity, enrollment, and permissions come from Canvas/canvas-mcp,
  never from chat text. Session state is only timezone, horizon, turn count, and a compact
  plan memory.
- **Untrusted Canvas text.** Assignment titles/descriptions/announcements are data. Injection-like
  text is detected, flagged, audited, and never followed. It cannot alter the tool allowlist.
- **Fail closed.** Missing dates, unparseable dates, timezone mismatches, malformed tool output,
  and MCP failures produce explicit caveats or refusals — not invented facts.
- **Audit.** Append-only JSONL records requests (hashed), tool calls, refusals, priority
  calculations, validation, and final responses (hashed). It never logs API keys, cookies,
  authorization headers, or full Canvas content. Set `CANVAS_AGENT_AUDIT_LOG` to a private path.
- **No deployment.** Local stdio only; the model provider receives only curated tool results.

## Developer workflow

See [CONTRIBUTING.md](CONTRIBUTING.md). The single quality gate is:

```bash
./scripts/check.sh
```

It runs `cargo fmt --check`, `cargo clippy -D warnings`, unit + mocked integration tests, the
offline evaluation suite, and `cargo deny` when installed. CI runs the same gates on every PR and
on pushes to `main`; a separate dependency-security workflow runs `cargo deny` and PR dependency
review.

## Evaluations

`evals/scenarios.json` contains deterministic scenarios with scripted model turns and fixtures
for normal, missing-data, tool-error, and prompt-injection cases. The runner asserts structured
outcomes (priority order, source citations, refusal reason, audit events, suspicious-content
flag), prints pass/fail, and exits non-zero on failure. It needs no real Canvas account or model
key, so CI can run it.

## Trade-offs and limitations

- canvas-mcp returns **text**, not structured JSON. The parser is deliberately strict and tested
  against the server's documented line formats. A future compatible improvement would be an
  opt-in structured tool result in canvas-mcp; this app does not modify the server for that.
- `get_my_upcoming_assignments` uses canvas-mcp's real `Utc::now()` for its window. The injected
  clock controls all policy, filtering, and rendering. For fully reproducible date-sensitive
  demos, the eval harness mocks MCP outputs.
- Point values for overdue items outside the upcoming window may not be returned; they are shown
  as unknown and contribute zero value points.
- Course IDs come from `list_courses`; if that call fails, answers still work but citations say
  the course ID was not returned.
- Live model quality (wording, tool choice) is not judged by the offline evals. They test the
  deterministic contract; a live smoke test requires a model key.
- No write actions, no web UI, no multi-user server, no deployment. See "Future path".

## Future path (not implemented)

1. Add an opt-in `--transport http` client for a hosted canvas-mcp, with OAuth/token isolation.
2. Add structured output to canvas-mcp behind a capability flag, then remove text parsing.
3. Add a read-only deployment profile (container, no secrets in image), health checks, and
   per-user session isolation before any hosted use.
4. Scope write actions behind an interface only: preview → explicit confirmation → idempotency
   key → audit event. Mutations stay disabled in this version.

## License

MIT OR Apache-2.0.
