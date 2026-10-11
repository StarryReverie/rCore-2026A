# 在 pi 中实现等价的过程记录

本仓库的课程过程记录工具默认支持 Codex、Claude Code、Cursor、
VS Code Copilot 和 OpenCode。本文研究如何用 **pi** 扩展实现同等能力，
并给出一个经过验证的原型。

原型位置：

- `.pi/extensions/rcore-session-archive/index.ts`（pi 项目扩展，pi 自动加载）
- `.pi/extensions/rcore-session-archive/core.mjs`（pi 会话 → 课程归档的纯转换器）
- `tests/test_pi_extension.mjs`（功能测试）

## 1. 对照：课程工具需要产出什么

| 目录 | 内容 | 生成者 |
| --- | --- | --- |
| `.ai/agent-sessions/<agent>/*.jsonl` | 会话归档，`schema_version: 1` | 各 Agent 归档插件 |
| `.ai/events/*.jsonl` | `course-log-v1` 事件：`ai_prompt`、`ai_file_operation`、`ai_command_result` 等 | VS Code 扩展 + AI 显式适配器 |
| `.ai/submissions/*.jsonl` | 提交快照（从 events 去重导出） | git pre-commit hook（`export.py`） |

归档头与事件（来自 `plugins/rcore-session-archive/scripts/archive_session.py`）：

```json
{"type":"session","schema_version":1,"agent":"codex","session_id":"...","started_at":"...","mode":"messages"}
{"type":"message","role":"user","content":[...],"event_id":"<sha256>","turn":1,"timestamp":"..."}
{"type":"message","role":"assistant","content":[{"type":"text","text":"..."}],"...":...}
{"type":"tool_call","tool_type":"tool_use","name":"bash","call_id":"...","input":{...},"...":...}
{"type":"tool_result","call_id":"...","content":[...],"is_error":false,"...":...}
{"type":"reasoning","content":"...","...":...}
{"type":"intermediate","role":"assistant","content":[...],"...":...}
```

模式过滤（`archive_storage.py` / `filter_*_record`）：

- `messages`：用户消息 + 助手最终回答。
- `tool-calls`：再加工具调用。
- `full`：再加推理、过程文本、工具结果。

`.ai/events/` 的事件统一由 `.ai/course-tools/.course-monitor/ai-ingest.py`
校验、脱敏后写盘（限制 64 KiB、校验项目路径、去除密钥、不含代码正文）。

## 2. pi 提供的对应能力

| 需求 | pi 能力 |
| --- | --- |
| 加载本地逻辑 | 项目扩展 `.pi/extensions/`（需 project trust）或用户扩展 `~/.pi/agent/extensions/`；`pi --extension file.ts` 临时加载 |
| 监听会话/消息/工具 | `pi.on(...)` 生命周期事件 |
| 读取当前会话 | `ctx.sessionManager.getBranch()/getEntries()/getHeader()/getSessionId()` |
| 获取工作目录 | `ctx.cwd` |
| 运行外部程序 | 扩展进程内 `node:child_process`（也可用 `pi.exec()`） |
| 会话持久化 | 会话文件 `~/.pi/agent/sessions/--<path>--/<ts>_<id>.jsonl`（v3 树） |
| 进程标识 | 子进程继承 `AI_AGENT=pi`、`PI_CODING_AGENT=true`；bash 工具另见 `PI_SESSION_ID/FILE` |

关键事件（`dist/core/extensions/types.d.ts`）：

| 事件 | 载荷要点 |
| --- | --- |
| `session_start` | `reason`；可在此初始化路径/配置 |
| `before_agent_start` | `prompt`（原始用户输入） |
| `message_start` / `message_update` / `message_end` | `message: AgentMessage`（含 role/content） |
| `tool_call` | `toolName`、`toolCallId`、`input`（可含 `path`/`command`） |
| `tool_result` | `toolCallId`、`content`、`isError`、`details` |
| `tool_execution_start` / `tool_execution_end` | `toolName`、`args` / `result`、`isError` |
| `agent_settled` | 一次运行彻底结束（无重试/压缩/排队），重建归档的最佳时机 |
| `session_shutdown` | `reason`（quit/reload/new/resume/fork），收尾时机 |

pi 的 `AgentMessage` 形状（对照 `docs/message-types.md`，并由本机真实会话确认）：

- `user`：`content: string | (TextContent | ImageContent)[]`
- `assistant`：`content: (TextContent | ThinkingContent | ToolCall)[]`，外带 `stopReason`、`usage`、`model` 等；`stopReason === "toolUse"` 时该消息只是过程，`"stop"` 等为最终回答
- `toolResult`：`toolCallId`、`toolName`、`content[]`、`isError`

> 本机的当前会话文件已用于验证：`~/.pi/agent/sessions/--<cwd>--/<ts>_<id>.jsonl`
> 是 v3 树，含 `session` / `message` / `model_change` / `thinking_level_change` /
> `context_edit` 等条目。

## 3. 映射设计

触发点：

| 时机 | 动作 |
| --- | --- |
| `session_start` | 求项目根、读配置、初始化归档 |
| `agent_settled` | 从 `getBranch()` 重建整个归档（幂等，覆盖同文件） |
| `session_shutdown` | 最后重建一次 |
| `before_agent_start` | `ai_prompt` 事件 |
| `tool_call`（read/write/edit） | `ai_file_operation` 事件 |
| `tool_execution_start/end`（bash） | `ai_command_result` 事件 |

pi 消息 → 归档事件：

| pi | 条件 | 归档事件 | 模式 |
| --- | --- | --- | --- |
| `user` | — | `message` / `role:"user"` | 全部 |
| `assistant` 的 text 块 | `stopReason != "toolUse"` | `message` / `role:"assistant"`（合并为一个 `content` 数组） | 全部 |
| `assistant` 的 text 块 | `stopReason == "toolUse"` | `intermediate` | `full` |
| `assistant` 的 thinking 块 | — | `reasoning` | `full` |
| `assistant` 的 toolCall 块 | — | `tool_call`（`tool_type:"tool_use"`，`name`、`call_id`、`input`） | `tool-calls` / `full` |
| `toolResult` | — | `tool_result`（`call_id`、`content`、`is_error`） | `full` |
| `system` / `model_change` / `context_edit` 等 | — | 不产出（`role:"system"` 属上下文） | — |

其它约定，全部对齐参考实现：

- **turn 编号**：沿用参考算法——遇到用户消息且上一轮已回复则 `turn += 1`。
- **event_id**：`sha256([pi 条目 id, 事件类型, 块下标])`，64 位十六进制，幂等去重。
- **content 脱敏**：复用 `sanitize_content` 的语义——数组递归、`data:` 内联二进制替换为 `[binary attachment omitted]`、图片转 `{type:"attachment",kind,...}`、丢弃 `signature`/`encrypted_content`（本实现对 pi 的 `thinkingSignature`/`thoughtSignature` 同样丢弃）。
- **文件名**：`.ai/agent-sessions/pi/<UTC 年月日_时分秒>_<session-id>.jsonl`。
- **写入**：先写临时文件再 `rename`，原子替换，保证幂等。

## 4. 与参考实现的差异

| 方面 | 参考实现 | pi 实现 |
| --- | --- | --- |
| agent 名 | `codex` 等 | `pi` |
| 数据来源 | 读取各 Agent 的 transcript 文件 | `ctx.sessionManager.getBranch()`（内存中的活动分支） |
| 分支/压缩 | 直接读源 transcript | 只归档**活动分支**；`context_edit`/compaction 后的可见文本仍可从条目重建，但被压缩掉的历史不在分支上 |
| 去重索引 | `.ai/agent-sessions/<agent>/.state/`（SQLite） | 不生成（最终 JSONL 相同；`.state/` 是实现细节） |
| 配置 | `.codex/session-archive.json` 等 | `.pi/session-archive.json`（或环境变量） |
| 文件操作事件 | VS Code 扩展独立采集 | pi 工具事件（`tool_call`）采集 |
| 终端事件 | VS Code Shell Integration | bash 工具 `tool_execution_*` |
| 触发 | Stop / SessionEnd hook | `agent_settled` / `session_shutdown` |

## 5. 原型与验证

文件：

```text
.pi/extensions/rcore-session-archive/
├── index.ts     # 扩展入口（触发、路径、配置、事件适配）
├── core.mjs     # 纯转换器
└── README.md
.pi/session-archive.example.json
tests/test_pi_extension.mjs
```

启用：

```sh
cp .pi/session-archive.example.json .pi/session-archive.json
# { "enabled": true, "mode": "full" }
```

验证结果（在本仓库、pi 真实运行时下）：

1. **转换器跑真实会话**（`core.mjs` 读本机 pi 会话文件）：
   - `messages` → 17 条、`tool-calls` → 147 条、`full` → 428 条；
   - 事件类型分布 `message/reasoning/tool_call/tool_result/intermediate`；
   - 所有 `event_id` 为唯一 sha256；头部字段与参考一致。
2. **扩展端到端**（`tests/test_pi_extension.mjs`，用假 pi 运行时 + 临时项目驱动）：
   - 事件序列 `message, reasoning, tool_call, tool_result, message`；
   - 头部 `agent=pi`、`schema_version=1`、`mode=full`；turn 编号与 event_id 校验全部通过。
3. 扩展经 pi 自带 esbuild 打包 + `node --check` 通过。

## 6. 限制与后续

- **只覆盖 pi 自身的操作**：VS Code 手工编辑、仓库外的终端命令不在记录范围。
- **`.ai/events/` 依赖课程工具安装**：需要 `python3 course.py install` 生成的
  `.ai/course-tools/.course-monitor/ai-ingest.py`；缺失时静默跳过（归档仍然工作）。
  运行 pi 时需保证 `python3` 在 `PATH`（在本仓库即 `nix develop`）。
- **不生成 `.state/` SQLite 索引**：参考实现的该索引用于增量排序；pi 直接按
  分支顺序重建，最终 `.jsonl` 一致。
- **活动分支语义**：`getBranch()` 只含当前分支；若使用 `/tree` 切换分支，
  归档会随活动分支变化。参考实现按源 transcript 归档，行为不同。
- **project trust**：项目扩展首次加载需要信任。
- **可选增强**：在 `session_shutdown` 末尾调用 `export.py --stage` 直接生成
  `.ai/submissions/` 快照；以及把 `agent_end` 的 `messages` 与 `getBranch()`
  做交叉校验。
