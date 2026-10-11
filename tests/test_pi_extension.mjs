/**
 * Functional harness for the pi course-recording extension.
 *
 * Drives the extension factory with a minimal fake of the pi runtime and a
 * temporary project, then asserts the archive JSONL that gets written. Run:
 *
 *   node tests/test_pi_extension.mjs
 *
 * Node 22.6+/23.6+ strips the extension's TypeScript types automatically.
 */

import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import factory from "../.pi/extensions/rcore-session-archive/index.ts";

const root = mkdtempSync(join(tmpdir(), "pi-archive-test-"));
mkdirSync(join(root, ".git"), { recursive: true });
mkdirSync(join(root, ".pi"), { recursive: true });
writeFileSync(join(root, ".pi", "session-archive.json"), JSON.stringify({ enabled: true, mode: "full" }));

const sessionId = "11111111-2222-3333-4444-555555555555";
const entries = [
	{ type: "session", id: sessionId, timestamp: "2026-01-02T03:04:05.000Z", cwd: root },
	{
		type: "message",
		id: "u1",
		parentId: null,
		timestamp: "2026-01-02T03:04:06.000Z",
		message: { role: "user", content: [{ type: "text", text: "hello" }], timestamp: 1 },
	},
	{
		type: "message",
		id: "a1",
		parentId: "u1",
		timestamp: "2026-01-02T03:04:07.000Z",
		message: {
			role: "assistant",
			content: [
				{ type: "thinking", thinking: "thinking..." },
				{ type: "toolCall", id: "call_1", name: "read", arguments: { path: "os/src/main.rs" } },
			],
			stopReason: "toolUse",
			timestamp: 2,
		},
	},
	{
		type: "message",
		id: "t1",
		parentId: "a1",
		timestamp: "2026-01-02T03:04:08.000Z",
		message: { role: "toolResult", toolCallId: "call_1", toolName: "read", content: [{ type: "text", text: "file body" }] },
	},
	{
		type: "message",
		id: "a2",
		parentId: "t1",
		timestamp: "2026-01-02T03:04:09.000Z",
		message: { role: "assistant", content: [{ type: "text", text: "final answer" }], stopReason: "stop", timestamp: 3 },
	},
];

const handlers = new Map();
const fakePi = { on: (name, handler) => (handlers.set(name, handler), () => {}) };
factory(fakePi);

const fakeCtx = {
	cwd: root,
	sessionManager: {
		getBranch: () => entries.slice(1),
		getHeader: () => ({ type: "session", id: sessionId, timestamp: entries[0].timestamp, cwd: root }),
		getSessionId: () => sessionId,
	},
};

const event = {};
await handlers.get("session_start")(event, fakeCtx);
await handlers.get("agent_settled")(event, fakeCtx);

const archivePath = join(root, ".ai", "agent-sessions", "pi", `2026-01-02_03-04-05_${sessionId}.jsonl`);
const lines = readFileSync(archivePath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
rmSync(root, { recursive: true, force: true });

const header = lines[0];
const types = lines.slice(1).map((record) => record.type);
const expected = ["message", "reasoning", "tool_call", "tool_result", "message"];
const checks = [
	["header agent", header.agent === "pi"],
	["header schema", header.schema_version === 1 && header.mode === "full"],
	["header session id", header.session_id === sessionId],
	["event sequence", JSON.stringify(types) === JSON.stringify(expected)],
	["turns", lines.slice(1).every((record) => record.turn === 1)],
	["event ids", lines.slice(1).every((record) => /^[0-9a-f]{64}$/.test(record.event_id))],
];

console.log("wrote:", archivePath);
console.log("types:", types.join(", "));
let failed = 0;
for (const [name, ok] of checks) {
	console.log(`${ok ? "PASS" : "FAIL"}  ${name}`);
	if (!ok) failed++;
}
process.exit(failed === 0 ? 0 : 1);
