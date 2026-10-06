/**
 * pi -> rCore course session-archive converter (core).
 *
 * Pure, dependency-free ESM so it can be imported both by the pi extension and
 * by a standalone test. It reproduces the JSONL schema produced by the
 * rCore course tool's `plugins/rcore-session-archive` (schema_version 1).
 *
 * Input : pi session entries (the objects stored in
 *         ~/.pi/agent/sessions/<dir>/<ts>_<id>.jsonl, or the array returned by
 *         ctx.sessionManager.getBranch()).
 * Output: an array of records: one session header + one record per event.
 */

import { createHash } from "node:crypto";

export const ARCHIVE_AGENT = "pi";
export const ARCHIVE_DIR = ".ai/agent-sessions";
export const MODES = new Set(["messages", "tool-calls", "full"]);

const DROP_KEYS = new Set(["encrypted_content", "signature", "thinkingSignature", "thoughtSignature"]);
const ATTACHMENT_TYPES = new Set(["image", "input_image", "image_url", "document", "input_audio", "audio"]);

/** Mirror of the course tool's `sanitize_content`: keep text, drop inline binary. */
export function sanitizeContent(value) {
	if (Array.isArray(value)) return value.map(sanitizeContent);
	if (typeof value === "string" && value.startsWith("data:")) return "[binary attachment omitted]";
	if (value === null || typeof value !== "object") return value;
	const kind = value.type;
	if (typeof kind === "string" && ATTACHMENT_TYPES.has(kind)) {
		const result = { type: "attachment", kind };
		for (const key of ["title", "filename", "name", "path", "url", "image_url"]) {
			let location = value[key];
			if (location && typeof location === "object") location = location.url;
			if (typeof location === "string" && !location.startsWith("data:")) result[key] = location;
		}
		if (value.mimeType) result.mimeType = value.mimeType;
		const source = value.source;
		if (source && typeof source === "object") {
			if (typeof source.url === "string" && !source.url.startsWith("data:")) result.url = source.url;
			if (kind === "document" && source.type === "text") result.text = source.data ?? "";
		}
		return result;
	}
	const out = {};
	for (const [key, item] of Object.entries(value)) {
		if (DROP_KEYS.has(key)) continue;
		out[key] = sanitizeContent(item);
	}
	return out;
}

function sha256(parts) {
	return createHash("sha256").update(JSON.stringify(parts)).digest("hex");
}

function textBlocks(content) {
	if (typeof content === "string") return content ? [{ type: "text", text: content }] : [];
	if (!Array.isArray(content)) return [];
	return content.filter((b) => b && typeof b === "object" && b.type === "text");
}

/**
 * Convert one pi session entry into zero or more raw archive events
 * (without event_id/turn). Mode filtering happens here.
 */
export function convertEntry(entry, mode) {
	if (!entry || entry.type !== "message") return [];
	const message = entry.message;
	if (!message || typeof message !== "object") return [];
	const role = message.role;
	const timestamp = typeof entry.timestamp === "string" ? entry.timestamp : undefined;
	const events = [];
	const push = (fields) => {
		const event = { ...fields };
		if (timestamp) event.timestamp = timestamp;
		events.push(event);
	};

	if (role === "user") {
		push({ type: "message", role: "user", content: sanitizeContent(message.content) });
		return events;
	}

	if (role === "assistant") {
		const content = Array.isArray(message.content) ? message.content : [];
		const isToolUse = message.stopReason === "toolUse";
		const finalText = isToolUse ? [] : textBlocks(content);
		if (finalText.length > 0) {
			push({ type: "message", role: "assistant", content: sanitizeContent(finalText) });
		}
		content.forEach((block, index) => {
			if (!block || typeof block !== "object") return;
			if (block.type === "text") {
				if (isToolUse && mode === "full") {
					push({ type: "intermediate", role: "assistant", content: sanitizeContent([block]), _index: index });
				}
			} else if (block.type === "thinking") {
				if (mode === "full") {
					push({ type: "reasoning", content: sanitizeContent(block.thinking ?? ""), _index: index });
				}
			} else if (block.type === "toolCall") {
				if (mode === "tool-calls" || mode === "full") {
					push({
						type: "tool_call",
						tool_type: "tool_use",
						name: block.name,
						call_id: block.id,
						input: sanitizeContent(block.arguments),
						_index: index,
					});
				}
			}
		});
		return events;
	}

	if (role === "toolResult") {
		if (mode === "full") {
			push({
				type: "tool_result",
				call_id: message.toolCallId,
				content: sanitizeContent(message.content),
				is_error: Boolean(message.isError),
			});
		}
		return events;
	}

	return events;
}

function slug(value) {
	return String(value).replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^[._]+|[._]+$/g, "").slice(0, 160);
}

/** Apply turn numbering, stable event_id and drop internal fields. */
export function finalizeRecords(entries, { sessionId, mode, startedAt }) {
	if (!MODES.has(mode)) mode = "messages";
	const header = {
		type: "session",
		schema_version: 1,
		agent: ARCHIVE_AGENT,
		session_id: slug(sessionId),
		started_at: startedAt,
		mode,
	};
	const records = [header];
	let turn = 0;
	let agentHasResponded = false;
	entries.forEach((entry) => {
		for (const event of convertEntry(entry, mode)) {
			const isUser = event.type === "message" && event.role === "user";
			if (turn === 0 || (isUser && agentHasResponded)) {
				turn += 1;
				agentHasResponded = false;
			}
			const index = event._index ?? 0;
			const raw = { ...event };
			delete raw._index;
			records.push({ ...raw, event_id: sha256([entry.id, raw.type, index]), turn });
			if (!isUser) agentHasResponded = true;
		}
	});
	return records;
}

/** Reconstruct the active branch (root -> leaf) from a session file's entries. */
export function activeBranch(entries) {
	const byId = new Map();
	for (const entry of entries) if (entry && entry.id) byId.set(entry.id, entry);
	if (entries.length === 0) return [];
	let leaf = entries[entries.length - 1];
	const parentIds = new Set(entries.map((e) => e && e.parentId).filter(Boolean));
	for (let i = entries.length - 1; i >= 0; i--) {
		if (!parentIds.has(entries[i].id)) {
			leaf = entries[i];
			break;
		}
	}
	const chain = [];
	let cursor = leaf;
	const guard = new Set();
	while (cursor && !guard.has(cursor.id)) {
		guard.add(cursor.id);
		chain.push(cursor);
		cursor = cursor.parentId ? byId.get(cursor.parentId) : undefined;
	}
	return chain.reverse();
}

/** Parse a pi session JSONL file into its entries. */
export function parseSessionJsonl(text) {
	const entries = [];
	for (const line of text.split("\n")) {
		if (!line.trim()) continue;
		entries.push(JSON.parse(line));
	}
	return entries;
}

export function archiveToJsonl(records) {
	return records.map((r) => JSON.stringify(r)).join("\n") + "\n";
}
