/**
 * rCore course recording for pi.
 *
 * Implements, inside pi, the equivalent of the rCore course tool's
 * `rcore-session-archive` (session JSONL) plus the AI adapter events.
 *
 * Outputs:
 *   .ai/agent-sessions/pi/<UTC-ts>_<session-id>.jsonl   conversation archive
 *   .ai/events/<date>.<uuid>.jsonl                      ai_prompt / ai_file_operation /
 *                                                       ai_command_result (via ai-ingest.py)
 *
 * `.ai/submissions/` is produced by the existing git pre-commit hook and needs
 * no pi-specific code.
 *
 * Enable it with `.pi/session-archive.json`:
 *   { "enabled": true, "mode": "full" }   // messages | tool-calls | full
 * or with PI_RCORE_ARCHIVE=1 / PI_RCORE_ARCHIVE_MODE=full.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { ARCHIVE_DIR, archiveToJsonl, finalizeRecords } from "./core.mjs";

type Mode = "messages" | "tool-calls" | "full";
const MODES: Mode[] = ["messages", "tool-calls", "full"];
const CONFIG_RELATIVE = join(".pi", "session-archive.json");
const INGEST_RELATIVE = join(".ai", "course-tools", ".course-monitor", "ai-ingest.py");

interface State {
	root: string | null;
	enabled: boolean;
	mode: Mode;
}

function findProjectRoot(cwd: string): string | null {
	let dir = cwd;
	for (;;) {
		if (existsSync(join(dir, ".git")) || existsSync(join(dir, ".pi"))) return dir;
		const parent = dirname(dir);
		if (parent === dir) return null;
		dir = parent;
	}
}

function readConfig(root: string): { enabled: boolean; mode: Mode } {
	const envFlag = process.env.PI_RCORE_ARCHIVE;
	const envMode = process.env.PI_RCORE_ARCHIVE_MODE as Mode | undefined;
	let enabled = envFlag === "1" || envFlag === "true";
	let mode: Mode = MODES.includes(envMode as Mode) ? (envMode as Mode) : "messages";
	try {
		const config = JSON.parse(readFileSync(join(root, CONFIG_RELATIVE), "utf8"));
		if (typeof config.enabled === "boolean") enabled = config.enabled;
		if (MODES.includes(config.mode)) mode = config.mode;
	} catch {
		// Missing or invalid config: fall back to environment / defaults.
	}
	return { enabled, mode };
}

function refresh(ctx: ExtensionContext, state: State): void {
	state.root = findProjectRoot(ctx.cwd);
	if (!state.root) {
		state.enabled = false;
		return;
	}
	const config = readConfig(state.root);
	state.enabled = config.enabled;
	state.mode = config.mode;
}

function utcStamp(iso: string): string {
	const date = new Date(iso);
	const pad = (value: number) => String(value).padStart(2, "0");
	return (
		`${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}` +
		`_${pad(date.getUTCHours())}-${pad(date.getUTCMinutes())}-${pad(date.getUTCSeconds())}`
	);
}

function safeId(value: string): string {
	const cleaned = value.replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^[._]+|[._]+$/g, "").slice(0, 160);
	return cleaned || "session";
}

function flush(ctx: ExtensionContext, state: State): void {
	if (!state.root || !state.enabled) return;
	const branch = ctx.sessionManager.getBranch();
	if (branch.length === 0) return;
	const header = ctx.sessionManager.getHeader();
	const sessionId = ctx.sessionManager.getSessionId();
	const startedAt = header?.timestamp ?? branch[0]?.timestamp ?? new Date().toISOString();
	const records = finalizeRecords(branch as never[], { sessionId, mode: state.mode, startedAt });

	const directory = join(state.root, ARCHIVE_DIR, "pi");
	mkdirSync(directory, { recursive: true, mode: 0o700 });
	const target = join(directory, `${utcStamp(startedAt)}_${safeId(sessionId)}.jsonl`);
	const temporary = join(directory, `.${safeId(sessionId)}.${process.pid}.tmp`);
	writeFileSync(temporary, archiveToJsonl(records), "utf8");
	renameSync(temporary, target);
}

/** Best-effort hand-off to the installed course AI adapter; ignored when absent. */
function recordEvent(root: string | null, event: Record<string, unknown>): void {
	if (!root) return;
	const script = join(root, INGEST_RELATIVE);
	if (!existsSync(script)) return;
	try {
		const child = spawn("python3", [script], { stdio: ["pipe", "ignore", "ignore"], detached: true });
		child.on("error", () => {});
		child.stdin.end(JSON.stringify(event));
		child.unref();
	} catch {
		// Recording never blocks the agent.
	}
}

export default function (pi: ExtensionAPI) {
	const state: State = { root: null, enabled: false, mode: "messages" };
	const pendingBash = new Map<string, { command: string; cwd: string; started: number }>();

	pi.on("session_start", async (_event, ctx) => {
		refresh(ctx, state);
		flush(ctx, state);
	});

	// Rebuild the archive when a run settles and when the session tears down,
	// mirroring the reference plugin's Stop / SessionEnd hooks.
	pi.on("agent_settled", async (_event, ctx) => flush(ctx, state));
	pi.on("session_shutdown", async (_event, ctx) => flush(ctx, state));

	pi.on("before_agent_start", async (event, _ctx) => {
		recordEvent(state.root, { type: "ai_prompt", prompt: event.prompt, tool: "pi" });
	});

	pi.on("tool_call", async (event, _ctx) => {
		const input = event.input as Record<string, unknown>;
		const path = typeof input?.path === "string" ? input.path : undefined;
		if (!path) return;
		if (event.toolName === "read") {
			recordEvent(state.root, { type: "ai_file_operation", tool: "pi", operation: "read", file: path, confidence: "high" });
		} else if (event.toolName === "write") {
			recordEvent(state.root, { type: "ai_file_operation", tool: "pi", operation: "write", file: path, confidence: "high" });
		} else if (event.toolName === "edit") {
			recordEvent(state.root, { type: "ai_file_operation", tool: "pi", operation: "edit", file: path, confidence: "high" });
		}
	});

	pi.on("tool_execution_start", async (event, _ctx) => {
		if (event.toolName !== "bash") return;
		pendingBash.set(event.toolCallId, {
			command: String((event.args as Record<string, unknown>)?.command ?? ""),
			cwd: state.root ?? "",
			started: Date.now(),
		});
	});

	pi.on("tool_execution_end", async (event, _ctx) => {
		const pending = pendingBash.get(event.toolCallId);
		if (!pending) return;
		pendingBash.delete(event.toolCallId);
		const structured = (event.result as Record<string, unknown> | undefined)?.structuredContent as
			| Record<string, unknown>
			| undefined;
		const exitCode = typeof structured?.exit_code === "number" ? structured.exit_code : undefined;
		recordEvent(state.root, {
			type: "ai_command_result",
			tool: "pi",
			command: pending.command,
			cwd: pending.cwd,
			exit_code: exitCode,
			duration_ms: Date.now() - pending.started,
		});
	});
}
