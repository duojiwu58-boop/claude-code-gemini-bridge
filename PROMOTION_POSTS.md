# 项目推广与社区发帖文案 (v0.1.3)

直接复制以下文案去各个社区/平台发帖推广：

---

## 1. V2EX (程序员 / AI 节点)

**标题：** 开源了一个专为 Claude Code 打造的多模型智能体运行时（Rust + Windows Service + Delphi GUI）

**正文：**

大家好！最近 Claude Code 越来越火，终端 Agent 体验确实非常丝滑。但由于各种协议细节（思维链流式、Tool Call 签名、多模态、Schema 校验等），直接把 Claude Code 接到 Gemini 上往往会遇到各种 400 报错、卡死或思维链丢失。

为了让 Gemini 3.7 Flash 的极速推理、大上下文和超强性能在 Claude Code 中 **100% 完全发挥**，我用 Rust 和 Delphi 写了这个深度协议适配的多模型智能体运行时并开源了：

👉 **GitHub 仓库：** https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime
📦 **Release v0.1.3 下载：** https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime/releases/tag/v0.1.3

### 💡 为什么它不同于普通 API 转发器？

普通转发网关只能做简单的字段改名，但在 Agent 真实编程场景下会遇到很多硬核问题。本项目做了深度协议适配：

1. 🧠 **Gemini 3 思考链 (Thinking Stream)**：完整将 Gemini `reasoning_content` / `thinking` 转译为 Anthropic 标准的 `thinking` Block 与 `thinking_delta` 流事件，并在文本/工具到时自动平滑闭合。
2. 🔑 **`thought_signature` 状态机**：自动提取并用 LRU 缓存维护 Gemini 3 的加密思考签名，并在多轮 Tool Roundtrip 中精准还原回传，解决 Tool Call 400 报错。
3. 🛠️ **MCP / JSON Schema 递归清洗**：自动递归清洗 `$schema` / `$id` / `$comment` 并自动补全 `type: object`，彻底解决 Claude Code 内置与 MCP 工具的 Gemini 校验失败问题。
4. 🖼️ **多模态 Tool Result & PDF**：完整支持截图、图片和 PDF 文档 (`application/pdf`) 传入 Tool Results。
5. 🛡️ **安全拦截与截断保护**：Gemini Safety 拦截自动优雅转为 Anthropic `refusal` 拒绝消息；`max_tokens` 截断时自动抑制坏 JSON 工具调用。
6. 🖥️ **Delphi 11 VCL 原生 GUI + Windows 服务**：极轻量原生 Windows GUI 界面，支持不重启 Claude Code 秒切 Gemini / DeepSeek / Kimi / Opus 等不同模型路由；支持一键安装为 Windows 系统服务。

欢迎大家下载体验或提 PR/Issue！如果觉得好用，求个 Star ⭐️ 支持一下，希望能帮助到广大 Gemini 和 Claude Code 粉丝！

---

## 2. Reddit (`r/ClaudeAI` / `r/GoogleGemini` / `r/LocalLLaMA`)

**Title:** [Open Source] Claude Code Multi-Model Agent Runtime with deep Gemini 3.7 Flash compatibility (Windows Service & Native GUI)

**Body:**

Hi everyone!

Claude Code's terminal agent interface is fantastic, but using Google Gemini 3.7 Flash as its backend through generic API proxies usually falls apart due to missing thinking streams, schema validation errors on MCP tools, or broken `thought_signature` state in multi-turn tool loops.

I built and open-sourced **Claude Code Multi-Model Agent Runtime** (`claude-code-multi-model-agent-runtime`), a protocol-aware Rust runtime designed to make Gemini behave like a native Claude Code backend:

🚀 **GitHub Repo:** https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime
📦 **Release v0.1.3:** https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime/releases/tag/v0.1.3

### Key Features:
- 🧠 **Streaming Thinking Blocks**: Maps Gemini `reasoning_content`/`thinking` directly into Anthropic SSE `thinking_delta` events with proper state-machine closing.
- 🔑 **Gemini 3 Thought Signature Retention**: Captures `extra_content.google.thought_signature` from tool calls and restores it across multi-turn tool roundtrips.
- 🛠️ **Recursive JSON Schema Sanitizer**: Automatically cleans `$schema`, `$id`, `$comment` and injects `type: object` to eliminate Gemini HTTP 400 errors on complex MCP tools.
- 📄 **Multimodal Tool Results & PDFs**: Native support for base64 images, screenshots, and PDF documents inside `tool_result` content.
- 🛡️ **Safety Guardrails & Truncation Protection**: Converts Gemini safety blocks into clean Anthropic `refusal` stop reasons; suppresses malformed/truncated JSON tools on `max_tokens`.
- 🖥️ **Native Windows GUI & Service**: Includes a lightweight Delphi VCL model-switcher GUI and a Windows Service wrapper for instant hot-switching without restarting Claude Code.

Pre-built standalone executables (`Setup.exe` & `ZIP`) are available under Releases. Feedback and Star ⭐️ appreciated!

---

## 3. X / Twitter

**推文内容：**

🚀 Open-sourced **Claude Code Multi-Model Agent Runtime** (`claude-code-multi-model-agent-runtime`) v0.1.3!

A protocol-aware Rust Agent Runtime that makes @GoogleAI Gemini 3.7 Flash run as a first-class backend inside @AnthropicAI Claude Code!

✨ Real-time Thinking Stream
🔑 Thought Signature Loop
🛠️ MCP JSON Schema Cleaning
📄 PDF & Vision Tool Results
🖥️ Delphi GUI & Windows Service

🔗 https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime

#ClaudeCode #Gemini #Rust #OpenSource #AI

---

## 4. 掘金 / 知乎 / 微信公众号 / 博客

**标题：** 让 Gemini 3.7 Flash 成为 Claude Code 的超级大脑：多模型智能体运行时开源！

**摘要：** 针对 Claude Code 的思考流、Tool Call 签名、多模态、Schema 递归清洗和安全拦截等边界行为进行深度适配，无需重启终端即可热切换模型。提供一键 Setup 安装包与免安装 ZIP。

*(正文可直接引用 README.md 中的中文说明与技术对比表格)*
