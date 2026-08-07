# Changelog / 更新日志

All notable changes to the **Claude Code ↔ Gemini Deep-Compatibility Bridge** project will be documented in this file.  
本项目的所有重要更新记录均将在此文档中记录。

---

## English

## [v0.1.4] - 2026-08-07

### Highlights & Fixes
- Added provider-aware routing through native OpenAI Chat Completions endpoints for Qwen/DashScope, DeepSeek, and Kimi/Moonshot while keeping Claude and unknown providers on Anthropic pass-through.
- Removed bridge-generated identity answers: identity questions now reach the downstream model verbatim, and response text is never rewritten by the bridge.
- Added explicit per-profile `CLAUDE_BRIDGE_TRANSPORT` and `CLAUDE_BRIDGE_UPSTREAM_URL` overrides.
- Fixed GUI background-task lifetime handling, refresh-button recovery, and concurrent model/proxy state updates.
- Added deterministic profile-change detection and serialized bridge-state persistence.
- Added GPL-3.0 licensing metadata and bundled the license in the portable package and installer.

### Verification
- 37 Rust unit tests passed.
- Delphi 11 GUI compiled with 0 warnings and 0 errors.
- Real Qwen routing returned its downstream identity through the OpenAI-compatible endpoint.
- Installer and portable ZIP verified with SHA-256 checksums.

## [v0.1.3] - 2026-08-04

### Highlights & Features
- **Extended Thinking / Reasoning Stream Translation**: Fully supports Gemini 3.6 Flash thinking streams (`reasoning_content` / `thinking`). Translates them into Anthropic SSE `thinking` blocks and `thinking_delta` events, automatically closing them before text or tool calls arrive.
- **Multimodal Tool Results & PDF Documents**: Supports base64 images and PDF documents (`application/pdf`) inside user messages and structured `tool_result` content, preserving mixed text/media arrays.
- **Recursive JSON Schema Sanitizer**: Automatically cleans `$schema`, `$id`, and `$comment` from Anthropic/MCP tool input schemas, infers missing `type: object`, and recursively traverses `$defs`, `definitions`, `items`, `oneOf`, `anyOf`, and `allOf` to prevent Gemini HTTP 400 schema validation errors.
- **Gemini Safety Interceptions & Refusals**: Gracefully converts Gemini `promptFeedback.blockReason` (such as SAFETY blocks) into Anthropic `stop_reason: "refusal"` messages with human-readable feedback instead of returning raw error responses.
- **Gemini Thought Signature Round Trips**: Captures `extra_content.google.thought_signature` from Gemini 3 tool calls and restores it on subsequent tool-result turns using a bounded LRU cache (`IndexMap`).
- **Tool Call Truncation Protection**: Suppresses incomplete/malformed tool calls if Gemini output gets truncated due to `max_tokens`.
- **Delphi 11 VCL GUI Model Center**: High-performance Native Windows GUI for hot-switching between Gemini, DeepSeek, Kimi, Claude, Qwen, and custom profiles without restarting Claude Code or VS Code.
- **Native Windows Service Integration**: Supports running as a background Windows service (`ClaudeCodeBridge`) with auto-start, failure recovery, health endpoints, and proxy management.

### Verification
- 24 Rust unit tests passed (`cargo test`).
- Delphi 11 GUI compiled with 0 warnings and 0 errors.
- Package verified with SHA-256 checksums.

---

## 中文

## [v0.1.4] - 2026-08-07

### 核心改进与修复
- 为 Qwen/百炼、DeepSeek、Kimi/Moonshot 增加原生 OpenAI Chat Completions 路由；Claude 和未知供应商继续使用 Anthropic 直通。
- 删除桥接器生成身份回答的逻辑：身份问题原样发送给下游模型，桥接器不再改写回答正文。
- 新增逐配置的 `CLAUDE_BRIDGE_TRANSPORT` 与 `CLAUDE_BRIDGE_UPSTREAM_URL` 显式覆盖选项。
- 修复 GUI 后台任务生命周期、刷新按钮恢复，以及模型切换与代理设置的并发状态覆盖问题。
- 增加可靠的配置变更检测，并串行化桥接状态持久化。
- 增加 GPL-3.0 许可证元数据，并将许可证纳入便携包和安装程序。

### 验证
- 37 项 Rust 单元测试全部通过。
- Delphi 11 GUI 编译 0 警告、0 错误。
- 真实 Qwen 路由通过 OpenAI-compatible 端点返回其下游身份。
- 安装程序和便携 ZIP 均通过 SHA-256 校验。

## [v0.1.3] - 2026-08-04

### 核心特性与改进
- **深度思维链流式转译（Thinking Stream）**：完整支持 Gemini 3.6 Flash 的思维链输出 (`reasoning_content` / `thinking`)，转译为 Anthropic SSE 的 `thinking` 块和 `thinking_delta` 流事件，并在文本或工具调用到达时平滑闭合。
- **多模态工具结果与 PDF 支持**：支持在用户消息及结构化 `tool_result` 中传入 Base64 图片与 PDF 文档 (`application/pdf`)，完整保留混合文本与媒体数组。
- **JSON Schema 递归清洗器（Sanitizer）**：自动递归清理 Anthropic/MCP 工具 Schema 中的 `$schema`、`$id`、`$comment` 关键字，补全缺失的 `type: object`，并深度遍历 `$defs`、`definitions`、`items`、`oneOf`、`anyOf`、`allOf`，彻底解决 Gemini HTTP 400 校验报错。
- **Gemini 安全拦截转拒绝（Safety Refusal）**：将 Gemini 的安全拦截策略（`promptFeedback.blockReason`）优雅转译为 Anthropic 标准的 `stop_reason: "refusal"` 拒绝消息与友好提示，避免返回原始报错。
- **Gemini 思维链签名状态机（Thought Signature）**：自动提取并用环形 LRU 缓存（`IndexMap`）维护 Gemini 3 工具调用的 `extra_content.google.thought_signature` 加密签名，在多轮工具交互中精准还原回传。
- **工具调用截断保护（Truncation Guard）**：当 Gemini 响应触发 `max_tokens` 导致工具参数截断时，自动抑制不完整/损坏的工具调用 JSON，保障本地运行安全。
- **Delphi 11 VCL 模型中心 GUI**：极轻量 Windows 原生控制台，支持不重启 Claude Code 或 VS Code 实时热切换 Gemini、DeepSeek、Kimi、Claude、Qwen 等模型路由。
- **Windows 原生服务集成**：支持安装为 Windows 系统守护服务 (`ClaudeCodeBridge`)，提供开机自启、故障恢复、健康检查与网络代理管理。

### 验证情况
- 24 个 Rust 单元测试 100% 通过 (`cargo test`)。
- Delphi 11 GUI 编译 0 警告、0 错误。
- 安装包与压缩包通过 SHA-256 哈希校验。
