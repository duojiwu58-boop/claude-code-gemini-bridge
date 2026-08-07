# Changelog / 更新日志

All notable changes to the **Claude Code ↔ Gemini Deep-Compatibility Bridge** project will be documented in this file.  
本项目的所有重要更新记录均将在此文档中记录。

---

## English

## [v0.3.2] - 2026-08-07

### OpenAI Compatibility and Tool Reliability
- Invalid or truncated arguments in one parallel tool call no longer discard valid text or other tool calls from the same response. Invalid calls are skipped individually in streaming and non-streaming paths.
- Non-object JSON request bodies now return a controlled `400` response instead of panicking in Anthropic-transport profiles.
- Streaming requests now honor each Provider profile's configured OpenAI capabilities, keeping reasoning fields, thinking-tag extraction, and `stream_options` behavior consistent with non-streaming requests.
- Orphan Anthropic tool results and Responses API function outputs are filtered before forwarding, preventing upstream `400` errors.
- Sequential anonymous streamed tool calls are kept separate, and URL-referenced Anthropic images are preserved as OpenAI `image_url` content.

### Streaming, State, and Provider Resilience
- Malformed optional legacy `settings*.json` profiles are skipped with warnings instead of preventing bridge startup or profile reload.
- Added bounded upstream request timeouts and ensured abandoned response streams release their upstream request when the client disconnects.
- Moved profile file reads and hashing off async request handlers, removed duplicate configuration-stamp work, and kept blocking persistence outside routing write locks.
- Bridge state updates are serialized and persisted through same-directory temporary files with atomic replacement, preventing partial files and lost concurrent updates.
- OpenAI-compatible SSE decoding now limits individual lines and events to 8 MiB and replaces malformed UTF-8 instead of immediately terminating the stream.
- Streamed Gemini safety blocks are surfaced as Anthropic refusal events; poisoned thought-signature locks recover with an error log instead of becoming permanent silent failures.

### Windows Service and Packaging
- The Windows service now runs under the isolated `NT SERVICE\ClaudeCodeBridge` virtual account with explicit least-required filesystem access instead of LocalSystem.
- Raised the installer minimum to Windows 10 so the declared platform matches the PowerShell and networking commands used by the installer.
- Release packaging writes the bilingual usage guide with a UTF-8 BOM for reliable display in Windows Notepad.
- Bumped the release and installer version to `0.3.2` and published matching Setup, portable ZIP, and SHA-256 manifests.

### Verification
- 66 Rust unit tests passed, including malformed parallel tools, orphan results, anonymous streamed calls, bounded SSE frames, streamed safety blocks, URL images, and poisoned-lock recovery.
- Rust formatting, release compilation, PowerShell parser checks, static-CRT packaging, portable ZIP inspection, Inno Setup compilation, and local Windows service health checks completed successfully.

## [v0.3.1] - 2026-08-07

### Fixes
- Normalize missing, empty, and whitespace-only OpenAI tool-call IDs to unique bridge-generated IDs in streaming and non-streaming responses, preserving parallel tool-result correlation in Claude Code.

### Verification
- Confirmed the Qwen/Claude Code tool loop is resolved in a live local-service test.
- 58 Rust unit tests, strict Clippy checks, the Delphi 11 GUI build, static-CRT package build, portable ZIP, Inno Setup installer, and SHA-256 manifests completed successfully.

## [v0.3.0] - 2026-08-07

### Near-Lossless OpenAI Semantic Core
- Rebuilt the Claude Messages → OpenAI Chat Completions path around a provider-neutral semantic core. Provider behavior is now selected by response fields and explicit capabilities rather than model-name checks.
- Added optional per-Provider capability overrides for `stream_options`, parallel-tool controls, `reasoning_effort`, reasoning response fields, `<think>` extraction, multimodal tool-result placement, tool-schema policy, and output-token field selection. Ordinary Providers still need only `model`, `base_url`, and `api_key`.
- Exposed the effective capability policy through the local management API so the GUI and diagnostics can show the behavior actually in force.

### Reasoning, Responses, and Streaming
- Preserved reasoning from configurable `reasoning_content`-style fields in both streaming and non-streaming responses without model-name gating.
- Added optional `<think>...</think>` extraction into Anthropic Thinking blocks, including opening and closing tags split across arbitrary SSE chunks. Extraction is limited to the start of answer content and can be disabled when literal tags must be preserved.
- Added field-driven support for array-shaped text, standard `refusal`, legacy `function_call`, object-valued tool arguments, and both `prompt_tokens/completion_tokens` and `input_tokens/output_tokens` usage conventions.
- Treats a stream ending without either `[DONE]` or `finish_reason` as an abnormal termination instead of falsely emitting `end_turn`.

### Tools, Multimodal Results, and Schemas
- Added conservative repair for common tool-argument JSON defects: raw control characters, trailing commas, and missing `}`/`]`. Unterminated strings are never guessed or executed.
- Fixed streaming tool calls to emit the repaired, valid JSON in Anthropic `input_json_delta` events instead of validating the repair and then forwarding the malformed source fragment.
- Standard OpenAI-compatible Providers now keep `role: tool.content` as a string and move attached images/PDFs into a following `role: user` message. The Gemini-specific route retains inline multimodal tool results.
- Kept recursive JSON Schema sanitization enabled by default, including definitions, combinators, array items, and object-valued `additionalProperties`; full schemas can still be preserved explicitly.

### Claude Code Reliability Contract
- Maps HTTP `429` to `rate_limit_error`, `400/413` to `invalid_request_error`, `529` to `overloaded_error`, and authentication/permission/not-found statuses to their Anthropic equivalents.
- Detects context-limit messages incorrectly wrapped by an upstream proxy as 5xx and normalizes them to `400 invalid_request_error`, restoring Claude Code retry and Auto-compact behavior.
- Preserves existing Gemini thought-signature round trips and safety/refusal handling while applying the generic OpenAI core to Qwen/DashScope, DeepSeek, Kimi/Moonshot, Ollama, vLLM, LM Studio, SiliconFlow, and other compatible endpoints.
- The native Codex Responses/GPT route remains deliberately unchanged; v0.3.0 improves Claude Code → OpenAI Chat Completions bridging only.

### Documentation, Packaging, and Verification
- Expanded the README and Provider guide with the two-layer architecture, capability matrix, proxy guidance, strict-endpoint fallbacks, compatibility boundaries, and a ready-to-copy capability override example.
- 56 Rust unit tests passed, including cross-chunk thinking tags, strict multimodal tool results, repaired streamed arguments, abnormal EOF, error contracts, provider variants, and Gemini regression coverage.
- Rust tests, strict Clippy checks, static-CRT release build, Delphi 11 GUI build, portable ZIP, Inno Setup installer, and SHA-256 manifests completed successfully.

## [v0.2.0] - 2026-08-07

### Native OpenAI Provider Configuration
- Added bridge-owned Provider profiles under `%USERPROFILE%\.claude\bridge-providers\`, independent of legacy per-provider `ANTHROPIC_*` fields.
- Made OpenAI Chat Completions the default native Provider protocol. A typical provider now needs only the official SDK values `model`, `base_url`, and `api_key`.
- Added optional `endpoint`, `api_key_env`, `identity`, `identity_override`, `proxy`, `enabled`, and native Anthropic protocol support, plus JavaScript-style `baseURL` / `apiKey` aliases.
- Added native OpenAI routing for Gemini, DeepSeek, Kimi/Moonshot, Qwen/DashScope, OpenRouter, and other compatible services while keeping the downstream model responsible for the answer.
- Kept legacy `settings - *.json` profiles available during gradual migration, with equivalent native profiles taking precedence.

### GUI, Installer, and Documentation
- Updated the model center to display Provider names, watch the native configuration directory, and hot-reload its contents without restarting Claude Code, VS Code, or the bridge service.
- Updated the Windows installer and service scripts to create and register the native Provider directory and to generate Gemini as a bridge-owned profile.
- Added Provider templates, a detailed configuration guide, installer shortcuts to the configuration directory and guide, and package integration for all new documentation.
- Expanded the README with the architectural significance, concrete benefits, capability boundaries, and a three-minute Provider setup tutorial.

### Verification
- 41 Rust unit tests passed, including native Provider parsing, official OpenAI SDK base URL semantics, endpoint overrides, disabled profiles, migration, and local Gemini routing.
- Rust release build and Delphi 11 GUI build completed successfully; GUI compiled with 0 warnings and 0 errors.

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

## [v0.3.2] - 2026-08-07

### OpenAI 兼容与工具可靠性
- 并行工具调用中即使有一个参数 JSON 非法或被截断，也不会再丢弃同一响应中的有效文本和其他工具调用；流式与非流式路径都会只跳过损坏的调用。
- Anthropic transport profile 收到非对象 JSON 请求体时会返回受控的 `400`，不再触发 panic。
- 流式请求现在遵循各 Provider profile 配置的 OpenAI capabilities，使 reasoning 字段、Thinking 标签提取和 `stream_options` 与非流式行为保持一致。
- 转发前会过滤无匹配调用的 Anthropic tool result 和 Responses API function output，避免上游返回 `400`。
- 连续到达的匿名流式工具调用不再互相污染；Anthropic URL 引用图片会保留为 OpenAI `image_url` 内容。

### 流式、状态与 Provider 韧性
- 可选的旧版 `settings*.json` profile 即使格式损坏也只会被跳过并告警，不再阻止服务启动或 profile 热重载。
- 增加有界的上游请求超时；客户端断开并丢弃响应流后，上游请求也会随之释放。
- Profile 文件读取与哈希已移出异步请求处理线程，消除重复配置戳计算，并避免在持有 routing 写锁时执行阻塞式状态持久化。
- Bridge state 更新改为串行处理，并通过同目录临时文件原子替换，避免并发更新丢失和状态文件半写入。
- OpenAI-compatible SSE 的单行和单事件上限为 8 MiB；畸形 UTF-8 会替换为恢复字符，不再立即中断整个流。
- 流式 Gemini 安全拦截会输出 Anthropic refusal 事件；thought-signature 锁中毒后会恢复并记录错误，不再永久静默失效。

### Windows 服务与安装包
- Windows 服务由 LocalSystem 改为隔离的 `NT SERVICE\ClaudeCodeBridge` 虚拟账户，并仅授予所需文件系统权限。
- 安装器最低系统版本提高到 Windows 10，使声明的支持范围与实际使用的 PowerShell、网络命令一致。
- 发布流程会为双语使用说明写入 UTF-8 BOM，确保 Windows 记事本稳定显示中文。
- 版本及安装器升级为 `0.3.2`，并发布对应的 Setup、便携 ZIP 和 SHA-256 校验清单。

### 验证
- 66 项 Rust 单元测试全部通过，覆盖畸形并行工具、孤立结果、匿名流式调用、SSE 上限、流式安全拦截、URL 图片和锁中毒恢复。
- Rust 格式检查、Release 编译、PowerShell 语法检查、静态 CRT 打包、便携 ZIP 检查、Inno Setup 编译及本机 Windows 服务健康检查均成功完成。

## [v0.3.1] - 2026-08-07

### 修复
- 流式与非流式 OpenAI 响应中的工具调用 ID 如果缺失、为空或仅含空白，桥接器会生成各自唯一的 ID，保证 Claude Code 能正确关联并行工具结果。

### 验证
- 已通过本机服务实测，确认千问在 Claude Code 中反复调用工具的问题已解决。
- 58 项 Rust 单元测试、严格 Clippy 检查、Delphi 11 GUI、静态 CRT 发布构建、便携 ZIP、Inno Setup 安装程序及 SHA-256 清单均成功完成。

## [v0.3.0] - 2026-08-07

### 近乎无损的 OpenAI 语义核心
- 将 Claude Messages → OpenAI Chat Completions 路径重构为供应商中立的语义核心；供应商行为由响应字段和显式能力配置决定，不再依赖模型名称判断。
- 新增逐 Provider 可选能力覆盖，可控制 `stream_options`、并行工具参数、`reasoning_effort`、推理响应字段、`<think>` 提取、多模态工具结果位置、工具 Schema 策略和输出 Token 字段。普通 Provider 仍只需 `model`、`base_url`、`api_key`。
- 本地管理 API 会返回实际生效的能力策略，GUI 和诊断工具因此能够展示真实运行行为。

### 深度思考、响应与流式转换
- 流式和非流式响应均可从配置的 `reasoning_content` 类字段保留推理内容，全程不依赖模型名称。
- 支持将 `<think>...</think>` 提取为 Anthropic Thinking 块，开闭标签即使横跨任意 SSE Chunk 也能正确识别；仅在答案正文开头启用识别，也可以关闭以保留字面标签。
- 字段驱动兼容数组型文本、标准 `refusal`、旧式 `function_call`、对象型工具参数，以及两套 OpenAI Usage Token 命名。
- 流在既没有 `[DONE]`、也没有 `finish_reason` 时会被识别为异常中断，不再伪装成正常 `end_turn`。

### 工具、多模态结果与 Schema
- 对工具参数 JSON 增加保守修复：可处理原始控制字符、尾逗号和缺失的 `}`/`]`，但绝不猜测补全未闭合字符串或冒险执行工具。
- 修复流式工具调用已通过 JSON 修复校验、却仍把原始畸形参数写入 Anthropic `input_json_delta` 的问题。
- 普通 OpenAI 兼容 Provider 的 `role: tool.content` 始终保持字符串，图片/PDF 会移到随后的 `role: user` 消息；Gemini 专有线路继续保留工具结果内联多模态。
- JSON Schema 默认继续递归清洗定义、组合器、数组项和对象型 `additionalProperties`；确认上游完整支持时仍可显式选择保留原 Schema。

### Claude Code 稳定性契约
- 将 HTTP `429` 映射为 `rate_limit_error`，`400/413` 映射为 `invalid_request_error`，`529` 映射为 `overloaded_error`，并保留认证、权限和资源不存在等 Anthropic 错误语义。
- 能识别被上游代理错误包装成 5xx 的上下文超限信息，并规范化为 `400 invalid_request_error`，恢复 Claude Code 的退避重试与 Auto-compact 行为。
- 在通用 OpenAI 核心覆盖 Qwen/百炼、DeepSeek、Kimi/Moonshot、Ollama、vLLM、LM Studio、SiliconFlow 等兼容端点的同时，继续保留 Gemini thought signature 往返与安全拒绝深度适配。
- Codex 原生 Responses/GPT 路径保持不变；v0.3.0 只增强 Claude Code → OpenAI Chat Completions 桥接。

### 文档、打包与验证
- README 和 Provider 指南新增双层架构、能力矩阵、代理说明、严格端点降级策略、兼容边界及可直接复制的能力覆盖示例。
- 56 项 Rust 单元测试全部通过，覆盖跨 Chunk Thinking 标签、严格多模态工具结果、修复后的流式参数、异常 EOF、错误契约、供应商变体和 Gemini 回归。
- Rust 测试、严格 Clippy、静态 CRT Release、Delphi 11 GUI、便携 ZIP、Inno Setup 安装程序及 SHA-256 清单均构建验证成功。

## [v0.2.0] - 2026-08-07

### 原生 OpenAI Provider 配置
- 新增桥接器自有的 `%USERPROFILE%\.claude\bridge-providers\` Provider 配置目录，上游模型不再依赖旧的 `ANTHROPIC_*` 字段。
- 原生 Provider 默认使用 OpenAI Chat Completions；通常只需照抄供应商官网 SDK 示例中的 `model`、`base_url`、`api_key`。
- 支持 `endpoint`、`api_key_env`、`identity`、`identity_override`、`proxy`、`enabled`、原生 Anthropic 协议，以及 `baseURL` / `apiKey` 等 JavaScript 风格别名。
- Gemini、DeepSeek、Kimi/Moonshot、Qwen/百炼、OpenRouter 及其他兼容服务均可走原生 OpenAI 路由；回答仍由真实下游模型生成。
- 保留旧 `settings - *.json` 作为渐进迁移兼容项，同等原生配置优先。

### GUI、安装器与文档
- 模型中心可显示 Provider 名称、监控原生配置目录并热刷新，无需重启 Claude Code、VS Code 或桥接服务。
- Windows 安装器和服务脚本会创建并注册原生 Provider 目录，Gemini 也改为桥接器自有配置。
- 新增 Provider 模板、完整配置指南、配置目录/指南快捷方式，并将相关文档和示例纳入安装包与便携包。
- README 新增本次架构升级的意义、实际收益、能力边界和三分钟配置教程。

### 验证
- 41 项 Rust 单元测试全部通过，覆盖原生 Provider 解析、官网 OpenAI SDK 基地址语义、完整端点覆盖、禁用配置、渐进迁移和本地 Gemini 路由。
- Rust release 与 Delphi 11 GUI 均构建成功，GUI 为 0 警告、0 错误。

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
