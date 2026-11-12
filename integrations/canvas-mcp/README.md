# Optional structured contract for canvas-mcp

This directory contains an additive patch for
[canvas-mcp](https://github.com/cmkhoa/canvas-mcp) that exposes one new read-only tool,
`get_planning_snapshot`, for agent consumption.

## Why

The agent can parse canvas-mcp's human-readable tool text, but that couples it to the server's
presentation format. A wording or date-format change could silently drop an assignment. The
snapshot tool returns a schema-versioned JSON object with explicit IDs, RFC 3339 instants,
numeric points, and boolean submission flags. Existing text tools are unchanged, so other MCP
clients keep working.

The agent detects the tool with `tools/list` at connect time:

- present → structured snapshot path;
- absent → existing text-tool fallback.

Both paths are tested. The fallback remains for compatibility with released canvas-mcp.

## Contract v1

```json
{
  "schema_version": "1",
  "generated_at": "2026-11-12T15:25:00Z",
  "courses": [
    {
      "id": 101,
      "code": "CS 101",
      "name": "Intro to Computer Science",
      "term": "Fall 2026",
      "role": "student"
    }
  ],
  "assignments": [
    {
      "course_id": 101,
      "assignment_id": 9002,
      "title": "Lab 5: Sorting",
      "due_at": "2026-11-14T05:59:00Z",
      "points_possible": 100.0,
      "submission_types": ["online_upload"],
      "kind": "assignment",
      "submission": {
        "workflow_state": "unsubmitted",
        "submitted": false,
        "missing": false,
        "late": false,
        "excused": false,
        "graded": false
      }
    }
  ],
  "warnings": []
}
```

Rules:

- `due_at` is RFC 3339 UTC or `null`. The agent never infers a missing date.
- `warnings` carries truncated lists and per-course read failures. The agent surfaces them and
  fails closed where completeness matters.
- `title` and course names remain untrusted Canvas text.
- The schema version is checked by the agent. A breaking change must bump it.
- The tool is read-only and reuses the existing Canvas client/assignment path. It does not
  introduce a second Canvas API client.

## Apply

```bash
git clone https://github.com/cmkhoa/canvas-mcp
cd canvas-mcp
git apply /path/to/canvas-agent/integrations/canvas-mcp/get-planning-snapshot.patch
cargo test --test planning --test schemas
cargo build --release
```

`get-planning-snapshot.patch` was verified against canvas-mcp `0.1.1`:

- applies cleanly to a fresh clone;
- `tests/planning.rs` asserts structured content, IDs, due instant, points, and submission flags;
- the existing canvas-mcp test suite, including the schema-portability test, passes with the
  tool count updated from 46 to 47.

Upstream proposal: [cmkhoa/canvas-mcp#3](https://github.com/cmkhoa/canvas-mcp/pull/3). The
canvas-mcp `fmt, clippy, test` and workflow-audit checks pass on the proposal.

The patch changes no existing tool signature. It adds:

- `src/tools/planning.rs` (new tool and typed structs);
- `src/tools/mod.rs` (router registration);
- `src/tools/assignments.rs` (one helper visibility change);
- `tests/planning.rs` (new integration test);
- `tests/schemas.rs` and `README.md` (tool count).
