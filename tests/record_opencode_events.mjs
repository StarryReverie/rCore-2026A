import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [root, id] = process.argv.slice(2);
const { default: definition } = await import(pathToFileURL(join(root, ".opencode/plugins/rcore-session-archive.js")));
const messages = ["user", "assistant"].map((role, i) => ({
  info: { id: `msg_${i}`, sessionID: id, role, parentID: "msg_0", finish: "stop",
    path: { cwd: root, root }, time: { created: 1789129800000 + i * 1000, completed: 1789129802500 } },
  parts: [{ id: `part_${i}`, messageID: `msg_${i}`, sessionID: id, type: "text",
    text: `${role === "user" ? "QUESTION" : "ANSWER"}_${id}` }],
}));
const errors = [];
const plugin = await definition.server({ directory: root, client: {
  session: {
    get: async () => ({ data: { id, directory: root, time: { created: 1789129800000 } } }),
    messages: async () => ({ data: messages }),
  },
  app: { log: async (entry) => errors.push(entry.body.message) },
} });
await plugin.event({ event: { type: "session.idle", properties: { sessionID: id } } });
if (errors.length) throw new Error(errors.join("\n"));
