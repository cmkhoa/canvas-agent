# Canvas Academic Planner

A small, auditable terminal agent that turns the read-only
[canvas-mcp](https://github.com/cmkhoa/canvas-mcp) server into a student planning
assistant.

```text
User → canvas-planner → canvas-mcp (:stdio JSON-RPC) → Canvas API
                 ↓
      deterministic policy, validation, citations,
      explicit session state, JSONL audit, offline evals
```

The model interprets intent, chooses one of a few application tools, and writes a short
explanation. **All dates, statuses, scores, ranks, citations, refusals, and state transitions
are computed by code.** Canvas free text is treated as untrusted data.

This is a separate application. It does not reimplement Canvas HTTP, and it does not
duplicate canvas-mcp's 46 read-only tools. It calls four of them through an allowlist.

> Local-first reference implementation. No deployment and no Canvas writes.

## Status

| Gate | Local result |
|---|---|
| `cargo fmt --check` | passing |
| `cargo clippy --all-targets -- -D warnings` | passing |
| Unit + mocked integration tests | 68 passing |
| Offline evaluation suite | 12/12 scenarios passing |
| `cargo deny check` (advisories, licenses, sources) | passing |
| GitHub Actions CI | configured; remote run pending the first push |

[![CI](https://github.com/cmkhoa/canvas-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/cmkhoa/canvas-agent/actions/workflows/ci.yml)

## Quick start

Requirements: Rust 1.88 (pinned by `rust-toolchain.toml`), a working `canvas-mcp`
installation signed in to your own account, and an OpenAI-compatible tool-calling model.

```bash
git clone <this-repo> && cd canvas-agent
./scripts/setup.sh          # creates .env, fetches locked deps, builds
$EDITOR .env                # LLM_API_KEY, STUDENT_TIMEZONE, CANVAS_MCP_COMMAND
cargo run --locked -- --once "What should I prioritize this week?"
```

Interactive mode:

```bash
cargo run --locked
```

With a deterministic clock for reproducible transcripts:

```bash
cargo run --locked -- --now 2026-11-12T09:25:00-06:00 \
  --once "What do I need to finish before Friday?"
```

Offline evaluation suite (no Canvas account, credentials, or model key):

```bash
cargo run --quiet --locked --bin canvas-eval -- --scenarios evals/scenarios.json
cargo run --quiet --locked --bin canvas-eval -- --scenarios evals/scenarios.json --verbose
```

Discover the CLI:

```bash
cargo run --locked -- --help
cargo run --locked -- eval --scenarios evals/scenarios.json
```

## Configuration

Copy `.env.example` to `.env`. Required variables:

| Variable | Purpose |
|---|---|
| `LLM_API_KEY` | Model provider key. Never logged, committed, or included in errors. |
| `LLM_BASE_URL` | OpenAI-compatible base URL, default `https://api.openai.com/v1`. |
| `LLM_MODEL` | Tool-calling chat model, default `gpt-4.1-mini`. |
| `STUDENT_TIMEZONE` | IANA zone used to read and display Canvas dates. |
| `CANVAS_MCP_COMMAND` | Command that starts canvas-mcp (default `canvas-mcp`). |
| `CANVAS_MCP_ARGS` | Optional whitespace-separated args, e.g. `--env-file /path/to/canvas.env`. |
| `CANVAS_AGENT_NOW` | Optional deterministic clock: RFC 3339 with offset or `YYYY-MM-DD`. |
| `CANVAS_AGENT_AUDIT_LOG` | Append-only JSONL audit path (default `logs/audit.jsonl`). |
| `CANVAS_AGENT_POLICY` | Policy file (default `config/policy.toml`). |
| `CANVAS_AGENT_HORIZON_DAYS` | Default planning horizon, default 7. |
| `CANVAS_AGENT_MAX_STEPS` | Model turns per request, default 6, max 12. |
| `CANVAS_AGENT_MAX_TOOL_CALLS` | Tool calls per request, default 12, max 30. |

Canvas credentials belong to canvas-mcp. Configure its own
`~/.config/canvas-mcp/.env`, or export `CANVAS_API_TOKEN` before launching the planner;
the MCP child process inherits that environment. Never put credentials in a tracked file.

**Timezone contract:** canvas-mcp formats deadline text in its `CANVAS_TIMEZONE`. Set
`CANVAS_TIMEZONE` and `STUDENT_TIMEZONE` to the same IANA zone. If a tool result's timezone
abbreviation does not match `STUDENT_TIMEZONE`, the parser fails closed rather than guessing
an offset.

The model API receives only the curated application-tool results, not Canvas credentials.
`LLM_BASE_URL` must be HTTPS except for `localhost`/`127.0.0.1` during local testing.

## Commands

| Command | What it does |
|---|---|
| `./scripts/setup.sh` | Create `.env` if absent, fetch locked dependencies, build. |
| `cargo fmt --all -- --check` | Formatting gate. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint/static gate. |
| `cargo check --all-targets --locked` | Type/static check. |
| `cargo test --all-targets --locked` | Unit + mocked integration tests. |
| `cargo run --quiet --bin canvas-eval -- --scenarios evals/scenarios.json` | Offline evals. |
| `cargo deny check` | Advisories, licenses, sources. |
| `./scripts/check.sh` | All of the above in one command. |

## File tree

```text
canvas-agent/
├── Cargo.toml / Cargo.lock / rust-toolchain.toml / rustfmt.toml
├── .env.example / .gitignore / deny.toml
├── config/policy.toml              # every priority threshold
├── src/
│   ├── bin/canvas-planner.rs       # terminal CLI/REPL
│   ├── bin/canvas-eval.rs          # offline evaluation runner
│   ├── agent.rs                    # explicit tool loop + validation + composition
│   ├── audit.rs                    # append-only JSONL, hashes, 0600 permissions
│   ├── clock.rs                    # Clock trait, FixedClock, timezone-aware parsing
│   ├── config.rs                   # env/CLI config, secret redaction
│   ├── error.rs / model.rs / session.rs / policy.rs
│   ├── mcp/
│   │   ├── mod.rs                  # rmcp stdio client + 4-tool allowlist
│   │   └── parse.rs                # strict parsers for canvas-mcp text output
│   ├── planner.rs                  # fetch/merge/score/rank/render/citations
│   ├── tools.rs                    # app tools exposed to the model
│   ├── validation.rs               # citation/claim/canvas-required checks
│   ├── untrusted.rs                # injection detection (never execution)
│   └── eval/mod.rs                 # scripted-model + mocked-MCP scenario runner
├── evals/
│   ├── scenarios.json              # 12 structured scenarios
│   └── fixtures/                   # normal, missing, error, injection
├── tests/
│   ├── agent_flow.rs               # 9 mocked end-to-end tests
│   └── evals.rs                    # CI gate for the eval suite
├── scripts/setup.sh / scripts/check.sh
└── .github/workflows/ci.yml / security.yml / dependabot.yml
```

## Why Rust, and why stdio

canvas-mcp is a Rust `rmcp` server, so the clean integration is the official `rmcp` client over
stdio: one toolchain, no protocol reimplementation, typed JSON-RPC results, and the same MCP
version the server already targets. The alternative (a Python client with a subprocess bridge)
would work but would duplicate protocol handling and split the build/test story. The application
still keeps its own policy, validation, audit, and rendering layers; Rust is an integration
choice, not a replacement for the agent design.

The supported MCP transport is a **command** (`CANVAS_MCP_COMMAND`), because canvas-mcp is a
stdio server. A URL/HTTP transport is not enabled; it is listed in the future path with
per-user credential isolation.

## Architecture

```text
┌────────────────────────────── canvas-planner ──────────────────────────────┐
│ CLI / REPL                                                                  │
│   └── Agent loop (raw OpenAI-compatible HTTP, no framework)                 │
│         ├── model: intent, tool choice, short narrative, follow-up         │
│         ├── tools: plan_priorities, assess_deadline_risk,                  │
│         │          summarize_workload, get_course_list,                    │
│         │          get_assignment_detail, refuse_request                   │
│         ├── Planner (code): MCP fetch → parse → merge → score → cite       │
│         ├── Validation (code): citations, unsupported claims, grounding    │
│         ├── Session (explicit): timezone, horizon, bounded history, memory │
│         └── Audit (append-only JSONL)                                      │
└──────────────────────────────────┬─────────────────────────────────────────┘
                                   │ stdio JSON-RPC (rmcp)
                                   ▼
                         canvas-mcp (existing Rust server)
                                   │ HTTPS, read-only token
                                   ▼
                              Canvas API
```

Only these existing canvas-mcp tools can be reached:
`list_courses`, `get_my_submission_status`, `get_my_upcoming_assignments`,
`get_assignment_details`. Unknown tools, write tools, and extra arguments are rejected in
code before any MCP call.

### Model-owned vs code-owned

| Model owns | Code owns |
|---|---|
| Interpreting the request ("before Friday" → `weekday: friday`) | Resolving that weekday/date in the student's zone |
| Choosing one of the allowlisted tools | The tool allowlist, argument schema, and MCP budget |
| A short, cited explanation and one follow-up question | Parsing every date, merging statuses/points, computing scores and ranks |
| Deciding an out-of-scope/write request needs refusal | The refusal text, refusal audit event, and the fact that no write path exists |
| | Citations, source rendering, deterministic tables, validation, session state |

If the model cites a source that does not exist, omits citations when Canvas data was used,
makes an unsupported grade/write claim, or answers a Canvas-planning question without
calling a tool, code runs **one** repair attempt. If the repair still fails, the answer
degrades to the deterministic table + sources rather than shipping an ungrounded narrative.

## Priority policy

All thresholds are in [`config/policy.toml`](config/policy.toml), validated at startup.

```text
score = deadline proximity (0–120)
      + point value (0–30: min(points, 100) / 100 * 30)
      + submission status (0–40 missing, 15 not submitted, 5 unknown)
```

- Overdue work is `critical`; Canvas-flagged missing work is `critical`.
- Unsubmitted work due within 3 days is `high`; within the horizon is `medium`.
- Work with no due date is `unknown`, listed separately, and **never** given a guessed date.
- Work with no point value contributes zero value points and says so.
- Completed work is excluded. External-tool work is listed as "completion cannot be verified."
- Tie-break: earlier due date, more points, course, assignment ID.

This is **deadline risk**, not a grade prediction. The planner does not claim to know a
student's actual grade risk. Grade prediction is out of scope.

## Citations

Every Canvas fact in the model narrative must use `[S#]`. Code assigns source IDs after
ranking, in deterministic order, and appends the full source block. A source includes the
course name, course ID when Canvas returns it, assignment title, assignment ID, and due date
or an explicit "due date not provided by Canvas." If `list_courses` fails, citations say the
course ID was not returned.

## Session state

The only state is:

- student timezone (startup flag/env),
- default planning horizon (`/horizon N`),
- turn count,
- bounded conversation history (10 messages / 16k characters),
- a compact memory of the last plan's assignment IDs/titles for follow-ups,
- last sources for `/sources`.

Identity, enrollment, permissions, and timezone are never inferred from user text. They come
from Canvas/canvas-mcp or explicit CLI/env configuration.

## Failure and refusal handling

| Situation | Behavior |
|---|---|
| MCP cannot start | Startup error with the command and a configuration hint. |
| Submission status fails | Planning/risk fails closed; answer says Canvas could not be read. |
| Point values fail | Plan proceeds; point value is marked unknown and contributes zero. |
| Missing due date | Listed separately as "no due date"; never ranked by invented urgency. |
| Unparseable date / timezone mismatch | Line is skipped with a caveat; a whole malformed result fails the tool. |
| Ambiguous horizon (`days` + `weekday`) | Code returns `invalid_horizon`; model explains. |
| Prompt injection in Canvas text | Flagged, audited, shown as a security note; instructions never followed. |
| Write/out-of-scope request | `refuse_request` returns code-owned refusal text; no write path exists. |
| Unknown tool call | Rejected before MCP; error is audited and reported. |
| Model unreachable | If Canvas evidence exists, deterministic evidence is shown; otherwise a clear failure. |
| Citation/claim validation failure | One repair attempt, then deterministic fallback. |

## Audit log

Append-only JSONL, created with `0600` permissions on Unix. Events:

- `request` — input SHA-256/length, timezone, horizon, effective clock, policy version.
- `tool_call` — tool, validated args (truncated), success/error code, source IDs, injection flag.
- `priority_calculation` — policy version, clock, horizon, ranks, score components, risk, source.
- `validation` — issues, cited IDs, whether a repair ran and succeeded.
- `refusal` — reason code.
- `suspicious_content` — scanner labels only.
- `final_response` — SHA-256, length, source IDs, degradation/refusal status.
- `session_state` — explicit horizon changes.

It never writes the model key, Canvas token/session cookie, authorization headers, full Canvas
content, or the raw user request. Use a private `CANVAS_AGENT_AUDIT_LOG` path.

## Evaluations

`evals/scenarios.json` uses `ScriptedLlm` (deterministic tool calls/narratives) plus fixture
MCP text. It asserts structured outcomes, not prose quality:

| Scenario | Asserts |
|---|---|
| `prioritize-week` | Ranked order, score/citation consistency, undated handling |
| `risk-missing` | Canvas-missing = critical; "not a grade prediction" |
| `workload-by-course` | Deterministic per-course counts/points and sources |
| `finish-before-friday` | Code resolves weekday in the student's zone |
| `missing-deadline` | No invented date; undated work separated |
| `tool-error` | Failure surfaced, no fabricated plan |
| `prompt-injection` | Flagged/audited, not followed, security note |
| `out-of-scope-write` | Code-owned refusal, no sources/mutation |
| `unknown-tool` | Rejected before MCP |
| `citation-repair` | One repair, valid citation after |
| `ambiguous-horizon` | Conflicting args rejected deterministically |
| `assignment-detail` | One assignment's due date/points/status with a citation |

The runner prints `PASS`/`FAIL`, exits non-zero on failure, and runs with no Canvas account or
model key. `--verbose` prints full deterministic answers.

## Example transcripts

Clock: **Thursday 2026-11-12, 9:25 AM Central**. Fixtures are in `evals/fixtures/`.

**What should I prioritize this week?**

```text
The missing problem set is the top priority [S1]; the sorting lab is next [S2].
Would you like a day-by-day plan?

### Deterministic priority plan
Policy 1 · as of Thu Nov 12, 2026, 9:25 AM CST · horizon the next 7 days
(through Thu Nov 19, 2026, 9:25 AM CST)

| # | Course | Assignment | Due | Points | Status | Score | Risk | Source |
|---|---|---|---|---|---|---|---|---|
| 1 | BIO 200 | Problem Set 4 (id 9001) | Thu Nov 12, 2026, 11:59 PM CST | 50 | Canvas flags missing | 145 | critical | [S1] |
| 2 | CS 101 | Lab 5: Sorting (id 9002) | Fri Nov 13, 2026, 11:59 PM CST | 100 | not submitted | 120 | high | [S2] |
| 3 | CS 101 | Quiz 3 (id 9006) | Mon Nov 16, 2026, 11:59 PM CST | 20 | not submitted | 76 | medium | [S3] |

**No due date in Canvas (not ranked by urgency):**
- CS 101 · Reading reflection (id 9003) · points unknown · not submitted [S4]

Sources (Canvas via canvas-mcp):
[S1] BIO 200 (course 202) — "Problem Set 4" (assignment 9001), due Thu Nov 12, 2026, 11:59 PM CST
[S2] CS 101 (course 101) — "Lab 5: Sorting" (assignment 9002), due Fri Nov 13, 2026, 11:59 PM CST
[S3] CS 101 (course 101) — "Quiz 3" (assignment 9006), due Mon Nov 16, 2026, 11:59 PM CST
[S4] CS 101 (course 101) — "Reading reflection" (assignment 9003), due date not provided by Canvas
```

**Submit assignment 9001 for me.**

```text
I can't make changes in Canvas: this planner is read-only. It cannot submit, edit, delete,
message, enroll, or change a grade. I can still help you plan the work and show what Canvas
says is due.

Would you like me to show what's due next so you can do it in Canvas yourself?
```

**What should I prioritize this week?** (Canvas gives one item no due date)

```text
Canvas gives no due date for the reading reflection [S1], so I can't rank it by urgency.
Would you like to check Canvas for a deadline?

### Deterministic priority plan
No open, dated assignments were found in this horizon.

**No due date in Canvas (not ranked by urgency):**
- CS 101 · Reading reflection (id 9003) · points unknown · not submitted [S1]

Sources (Canvas via canvas-mcp):
[S1] CS 101 (course 101) — "Reading reflection" (assignment 9003), due date not provided by Canvas
```

## Trade-offs and limitations

- **Text contract.** canvas-mcp returns human-readable text, not structured JSON. Parsing is
  strict and fixture-tested. A future opt-in structured result in canvas-mcp would remove this
  boundary without changing the policy layer.
- **MCP clock.** `get_my_upcoming_assignments` uses canvas-mcp's real `Utc::now()` for its
  query window. The injected clock controls parsing, filtering, scoring, and rendering. Fully
  reproducible date-sensitive demos use the mocked MCP in evals.
- **Partial data.** Overdue items outside the upcoming window can lack point values; they are
  shown as unknown and contribute zero value points.
- **Course list independence.** Course/detail questions fetch only `list_courses`; planning
  questions require submission status and fail closed if it is unavailable.
- **One model.** The client is OpenAI-compatible; provider-specific tool-call quirks are not
  covered. No retries for schema-invalid tool arguments beyond the model's next turn.
- **Evals are contract evals.** They do not score live model prose/tool-choice quality. A live
  smoke test needs a real key and Canvas account.
- **No writes.** Canvas mutations are out of scope. Any future write interface must require
  preview, explicit confirmation, an idempotency key, and an audit event.
- **No deployment.** Local stdio only; no multi-user server, hosted transport, or web UI.

## Future path

1. Add an opt-in HTTP MCP transport for a hosted canvas-mcp with per-user credential isolation.
2. Add an opt-in structured tool result in canvas-mcp, then delete text parsing.
3. Add live-model eval runs (separate from CI's deterministic contract evals).
4. Add a read-only deployment profile (container, health checks, no secrets in image,
   per-user sessions) before any hosted use.
5. Scope write actions behind an interface only: preview → confirmation → idempotency key →
   audit event. Keep mutations disabled by default.

## License

MIT OR Apache-2.0. See [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
