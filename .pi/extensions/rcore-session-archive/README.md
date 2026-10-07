# pi course recording (`rcore-session-archive` for pi)

A pi extension that records the current session in the same shape as the rCore
course tool's `rcore-session-archive`, so pi can replace the Codex / Claude Code
/ Cursor / Copilot / OpenCode adapters.

## What it writes

| Path | Content |
| --- | --- |
| `.ai/agent-sessions/pi/<UTC-timestamp>_<session-id>.jsonl` | conversation archive, `schema_version: 1` |
| `.ai/events/<date>.<uuid>.jsonl` | `ai_prompt`, `ai_file_operation`, `ai_command_result` (via the installed course AI adapter) |

`.ai/submissions/` is produced by the existing git pre-commit hook and needs no
pi-specific code.

## Enable it

The extension is auto-loaded from this directory by pi as a **project
extension** (project trust required). It is opt-in through configuration:

```sh
cp .pi/session-archive.example.json .pi/session-archive.json
```

```json
{ "enabled": true, "mode": "full" }
```

`mode` is `messages` (user + assistant answers), `tool-calls` (adds tool calls)
or `full` (adds reasoning, intermediate text and tool results). Environment
overrides: `PI_RCORE_ARCHIVE=1`, `PI_RCORE_ARCHIVE_MODE=full`.

The `.ai/events/` records additionally require the course tool to be installed
(`python3 course.py install`), which creates
`.ai/course-tools/.course-monitor/ai-ingest.py`. When it is absent, event
recording is skipped and the archive still works.

## Files

| File | Purpose |
| --- | --- |
| `index.ts` | extension entry: lifecycle triggers, path/config handling, event adapter |
| `core.mjs` | pure pi-session-to-archive converter (also used by the tests) |

## Test

```sh
node tests/test_pi_extension.mjs
```

See [`docs/pi-session-archive.md`](../../../docs/pi-session-archive.md) for the
mapping design and limitations.
