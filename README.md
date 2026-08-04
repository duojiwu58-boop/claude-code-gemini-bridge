# Claude Code ↔ Gemini Deep-Compatibility Bridge

**A protocol-aware Rust bridge built specifically to make Gemini behave like a
first-class Claude Code backend—not merely an OpenAI-shaped chat endpoint—with
live model switching that does not require restarting VS Code or Claude Code.**

This project aims to be one of the most deeply adapted open-source bridges for
running Google Gemini from Claude Code. Its primary path translates the
Anthropic Messages API used by Claude Code into Google AI Studio's Gemini
OpenAI-compatible Chat Completions endpoint, then reconstructs Anthropic
semantics on the way back.

> **中文定位：** 本项目不是只做字段改名的通用 API 转发器，而是针对 Claude
> Code 的思考流、工具调用状态机、Gemini thought signature、多模态工具结果、
> JSON Schema 严格校验和安全拦截等边界行为进行深度适配；同时通过固定本地
> 端点和配套 GUI，在无需重启 VS Code 或 Claude Code 的情况下，随时热切换
> Gemini 及其他 Anthropic-compatible 模型。目标是让模型在 Claude Code 中
> 不仅“能回答”，而且能稳定、方便地完成真实的长链路编程代理任务。

It can also forward requests to other Anthropic-compatible providers and
retains a legacy OpenAI Responses API route for Codex, but Claude Code ↔ Gemini
compatibility is the main maintenance target.

![Claude Code model center showing Gemini and Anthropic-compatible profiles with zero-restart switching](gui/delphi11/ClaudeBridgeManager/assets/ClaudeBridgeManager-screenshot.png)

*The bundled model center keeps Claude Code on one stable local endpoint while
Gemini, DeepSeek, Kimi, Claude, Qwen, proxy settings, and other compatible
profiles can be switched live. The next request uses the selected route—no
VS Code reload or Claude Code restart required.*

## Why this is different from a generic bridge

A generic bridge often stops after mapping `messages`, `content`, and
`tool_calls`. That is enough for a chat demo, but coding agents exercise a much
larger protocol surface: interleaved thinking and text, streamed partial JSON,
parallel tools, multimodal tool results, strict schemas, truncated generations,
and provider-specific state that must survive a tool round trip.

This bridge handles those behaviors explicitly:

| Compatibility surface | Common generic-bridge behavior | This project |
| --- | --- | --- |
| Streaming | Buffers the upstream response or assumes each network chunk is one SSE event | Calls Gemini with `stream: true`, incrementally decodes SSE across UTF-8 and network boundaries, and forwards Anthropic events as they become valid |
| Extended thinking | Drops Gemini `reasoning_content` / `thinking` | Emits Anthropic `thinking` blocks and `thinking_delta` events, closes them before text or tools, and preserves thinking in non-streaming responses |
| Tool arguments | Forwards incomplete JSON fragments directly | Accumulates streamed fragments, preserves parallel-call order, validates the completed JSON object, then emits a valid Anthropic `tool_use` block |
| Gemini thought signatures | Loses `extra_content.google.thought_signature` after the first tool call | Caches signatures by tool-call ID and restores them on the next assistant/tool round trip, with bounded eviction |
| Tool-result ordering | Preserves Claude block order even when Gemini requires tool results immediately after assistant tool calls | Emits `role: tool` results before remaining user text/images while preserving tool-call identity |
| Multimodal tool results | Stringifies or drops image/document blocks | Converts base64 images and PDFs to Gemini-compatible `image_url` data URIs, including structured `tool_result` content |
| Tool schemas | Passes Anthropic JSON Schema through unchanged and receives Gemini HTTP 400 errors | Recursively removes unsupported `$schema`, `$id`, and `$comment`, traverses definitions/combinators/items, and infers `type: object` when `properties` is present |
| Safety filtering | Crashes on an empty `choices` array | Converts Gemini `promptFeedback.blockReason` into a valid Anthropic `refusal` message with a readable reason |
| Truncated tool calls | Executes malformed/empty arguments or reports the wrong stop reason | Maps cutoff responses to `max_tokens` and suppresses incomplete tool calls and uncached signatures |
| Token accounting | Uses a byte-only estimate that undercounts non-ASCII prompts | Uses a Unicode-aware conservative input estimate and updates usage from streamed Gemini usage data when available |
| Live model switching | Requires editing Claude settings and restarting/reloading the client whenever the provider changes | Keeps Claude Code connected to one stable local endpoint and switches Gemini or another Anthropic-compatible profile from the GUI without restarting VS Code, Claude Code, or the bridge service |
| Operations | Runs as an ad hoc console proxy | Includes a native Windows service, delayed auto-start, recovery policy, graceful shutdown, health checks, persistent routing state, packaging, and a Delphi model-switcher GUI |

## Deep compatibility details

### Thinking is translated as a real Anthropic content-block lifecycle

Gemini thinking is not exposed as ordinary assistant text. The streaming
translator recognizes both `delta.reasoning_content` and `delta.thinking` and
produces the event sequence Claude Code expects:

```text
Gemini reasoning token
  -> content_block_start(type=thinking)
  -> content_block_delta(type=thinking_delta)
  -> content_block_stop
  -> text or tool-use content begins
```

The thinking block is closed when normal text or a tool call arrives, and also
during stream finalization. Non-streaming `reasoning_content` is prepended as
an Anthropic `thinking` content block rather than silently discarded. This is
what allows Claude Code to render its thinking state instead of appearing
frozen while Gemini reasons.

### Tool use is treated as a stateful protocol, not a JSON rename

- Streamed tool calls are keyed and accumulated in insertion order, including
  parallel tool calls whose IDs, names, and argument fragments arrive in
  different chunks.
- Completed arguments must parse as a JSON object before Claude Code sees the
  tool request.
- Gemini 3 thought signatures are captured from
  `extra_content.google.thought_signature`, stored in a bounded cache, and
  restored when Claude Code returns the corresponding tool result.
- A `length` / `max_tokens` finish suppresses unfinished tool calls so Claude
  Code never executes truncated arguments. Signatures from those invalid calls
  are not cached.
- Claude user messages that mix `tool_result`, text, and images are reordered
  only where Gemini's sequencing contract requires it: tool results immediately
  follow the assistant calls, while the remaining user content stays structured.

### Tool results remain multimodal

Claude Code tools can return screenshots, images, or PDF documents. Instead of
flattening these blocks into lossy text, the bridge emits structured OpenAI
content parts for Gemini:

- Anthropic base64 image → `image_url` with `data:<media-type>;base64,...`
- Anthropic base64 PDF document → `image_url` with
  `data:application/pdf;base64,...`
- Mixed text and media → one structured `role: tool` content array
- `is_error: true` → preserved as an explicit tool-error text part without
  discarding accompanying media

This matters for screenshot-driven debugging, browser automation, document
inspection, and other Claude Code workflows where the tool output is not just
plain text.

### Anthropic tool schemas are sanitized for Gemini's stricter parser

Claude Code and MCP tools frequently send modern JSON Schema metadata that
Gemini function declarations reject. Before forwarding a tool, the bridge
recursively sanitizes `properties`, `items`, `oneOf`, `anyOf`, `allOf`, `$defs`,
and `definitions`; removes `$schema`, `$id`, and `$comment`; and adds
`type: object` when an object has `properties` but omits its type.

This avoids a class of HTTP 400 failures that only appears with real-world MCP
tool catalogs and is commonly missed by bridges tested with one simple
function schema.

### Provider failures degrade into valid Claude Code responses

Gemini safety interception can return `promptFeedback.blockReason` with no
`choices[0].message`. The bridge turns that provider-specific response into a
well-formed Anthropic assistant message with `stop_reason: refusal` instead of
returning an internal bridge error. Content-filter finish reasons are normalized
the same way, while length cutoffs become `max_tokens`.

The SSE decoder also tolerates CRLF/LF framing, multiple data lines, partial
network frames, and UTF-8 code points split across byte chunks. This prevents
transport fragmentation from becoming visible as protocol corruption.

### Models can be hot-switched without restarting VS Code or Claude Code

Claude Code stays pointed at the bridge's stable local endpoint:
`http://127.0.0.1:18787`. The bundled Delphi GUI discovers usable
`%USERPROFILE%\.claude\settings - *.json` provider profiles and changes the
bridge's active upstream route through its local management API. Subsequent
Claude Code requests use the newly selected provider immediately.

This makes it practical to move between:

- Google Gemini through the bridge's deep protocol translator
- Other Anthropic-compatible providers already represented by Claude settings
- Different Gemini configurations, including runtime proxy/direct-mode changes

The switch does **not** rewrite the active Claude Code configuration, reload the
VS Code window, restart Claude Code, or restart the Windows bridge service.
Provider credentials remain in their original local profile files and are never
returned by the management API; only the selected profile filename and Gemini
proxy mode are persisted. The result is a fixed Claude Code connection with a
hot-swappable backend—one of the project's most convenient differences from a
single-provider bridge.

## Compatibility scope and honest limits

- The Gemini OpenAI-compatible endpoint does not provide an Anthropic-compatible
  tokenizer endpoint, so `/v1/messages/count_tokens` is a conservative estimate,
  not an exact Gemini token count.
- The legacy Codex Responses route is intentionally buffered. True upstream
  streaming is implemented for the primary Claude Code Messages path.
- Gemini and Claude Code can evolve independently. The regression suite covers
  the protocol edge cases above, but compatibility is maintained continuously
  rather than claimed as a timeless blanket guarantee.
- This project focuses on protocol fidelity and agent reliability; it does not
  attempt to make Gemini produce the same model behavior as Claude.

## Environment

- `GEMINI_API_KEY` (optional fallback)
- `GEMINI_BRIDGE_LISTEN` (default `127.0.0.1:18787`)
- `GEMINI_BRIDGE_MODEL` (default `gemini-3.6-flash`)
- `GEMINI_BRIDGE_PROXY` (optional, for example `http://127.0.0.1:8080`)
- `GEMINI_BRIDGE_UPSTREAM` (optional)
- `GEMINI_BRIDGE_API_KEY_PROFILE` (optional Codex TOML file containing
  `experimental_bearer_token`; used by Windows service mode without copying
  the secret to the registry)
- `GEMINI_BRIDGE_STATE_FILE` (optional model-switcher state file)
- `GEMINI_BRIDGE_LOG_DIR` (Windows service log directory)
- `CLAUDE_SETTINGS_DIR` (Claude provider profile directory)

## Run

```powershell
$env:GEMINI_BRIDGE_PROXY = 'http://127.0.0.1:8080'
cargo run --release
```

The Codex provider base URL is `http://127.0.0.1:18787/v1`.

Codex sends the API key as a Bearer token and the bridge forwards it to
Google. `GEMINI_API_KEY` is only needed for clients that do not send an
Authorization header.

The bridge never logs the API key or request content.

## Windows service

The recommended daily-use deployment is the native `ClaudeCodeBridge` Windows
service. From an elevated PowerShell window:

```powershell
.\scripts\install-service.ps1
```

The installer builds the release executable, deploys a stable service copy
under `service\`, configures delayed automatic startup and restart-on-failure,
then verifies `http://127.0.0.1:18787/health`. The Google API key remains in
the existing Codex profile; only that file's path is stored in the service
environment. Daily logs are written under `service\logs`.

`GEMINI_BRIDGE_PROXY` is only the service-startup default for Gemini. The
model switcher can change the Gemini proxy at runtime without restarting the
service. The selected proxy, including explicit direct mode, is persisted in
`bridge-state.json` and takes precedence on later starts.

The existing GUI and scripts remain compatible:

```powershell
.\scripts\start-bridge.ps1
.\scripts\stop-bridge.ps1
```

When the service is installed these commands control it; otherwise they retain
the legacy hidden-process behavior. To remove the service while preserving
logs and state:

```powershell
.\scripts\uninstall-service.ps1
```

Use `-RemoveServiceFiles` only when the deployed executable and service logs
should also be deleted.

## Redistributable Windows package

Build the friend-ready x64 package with:

```powershell
gui\delphi11\ClaudeBridgeManager\build-gui.cmd
.\scripts\build-release-package.ps1
```

The build produces a recommended Inno Setup installer and a portable ZIP under
`dist\`. The installer registers the Windows service, adds normal Start Menu
and uninstall entries, and makes Gemini configuration optional. Users who
route only to existing Anthropic-compatible `settings - *.json` profiles can
skip the Gemini checkbox and do not need to enter a Google API key.

When Gemini is selected, the installer prefills the proxy from the current
Windows system proxy when one is enabled. The value remains editable, clearing
it selects a direct connection, and upgrade installs preserve the effective
proxy.

Both packages contain a statically linked service executable, the Delphi model
switcher, configuration tools, Chinese instructions, and no embedded API key
or machine-specific configuration. The target computer does not need Rust,
Delphi, or the Visual C++ runtime.

## Codex profile

```toml
model = "gemini-3.6-flash"
model_provider = "gemini_bridge"
model_reasoning_effort = "high"

[model_providers.gemini_bridge]
name = "Local Gemini Bridge"
base_url = "http://127.0.0.1:18787/v1"
wire_api = "responses"
supports_websockets = false
experimental_bearer_token = "YOUR_GOOGLE_AI_STUDIO_KEY"

[features]
enable_request_compression = false
```

Start the bridge before starting or reloading Codex. The current implementation
supports Responses text output, reasoning-effort mapping, Codex function tools,
parallel tool calls, and Gemini 3 thought-signature round trips.

The legacy Codex Responses route buffers the Gemini upstream response before
emitting Responses events; it does not provide true upstream streaming. The
bridge is maintained primarily for Claude Code, while Codex is expected to use
its native GPT provider.

## Claude Code

The Claude Code base URL is `http://127.0.0.1:18787` (without `/v1`). The
bridge exposes:

- `POST /v1/messages`
- `POST /v1/messages/count_tokens`

`scripts\start-bridge.ps1` loads the Google key from the Codex Gemini profile
into the bridge process. Claude Code therefore uses the harmless local token
`local-gemini-bridge`; the Google key does not need to be copied into Claude
settings.

Copy `claude-settings.example.json` to `%USERPROFILE%\.claude\settings.json`,
or merge its `env` object into the active settings. A ready-to-swap personal
copy is also stored as:

```text
%USERPROFILE%\.claude\settings - gemini3.6 bridge.json
```

Keep the active `settings.json` pointed at the local bridge. The bridge maps
Claude text, images, client tools, tool results, tool choice, token limits, and
streaming Messages events to Gemini. When Claude requests streaming, Gemini is
also called with `stream: true`; text deltas are converted and forwarded as
they arrive. Streamed tool-call arguments are accumulated until complete before
the bridge emits the Anthropic `tool_use` block, preserving valid JSON and
parallel-tool ordering.

Token counting is an estimate because the Gemini OpenAI-compatible endpoint
does not expose a tokenizer route.

Run `scripts\test-claude-streaming.ps1` to measure first-text latency and verify
that a response contains multiple incremental text deltas.

For Anthropic-compatible upstream profiles, the bridge forwards exactly one
credential header: `Authorization: Bearer` takes precedence over `x-api-key`.
It also supplies `anthropic-version: 2023-06-01` when the client omits the
required version header. OAuth subscription tokens are not inferred from
`ANTHROPIC_AUTH_TOKEN`; OAuth-specific beta headers must be configured
explicitly if that authentication mode is added later.

Adaptive Claude thinking requests are mapped to Gemini `reasoning_effort:
high`. If a Gemini response is cut off while constructing tool arguments, the
bridge reports `max_tokens` and suppresses the incomplete tool call instead of
letting Claude Code execute malformed or empty arguments. Safety-filtered
responses are reported as `refusal`.

## Delphi model switcher

`ClaudeBridgeManager.exe` is the Delphi 11 VCL companion GUI. It reads the
usable `%USERPROFILE%\.claude\settings - *.json` files and switches the
bridge's active upstream route immediately:

The manager uses the native Windows common-controls theme, per-monitor DPI,
Windows 11 rounded-window chrome, responsive command layout, and separate
status, model, proxy, and activity surfaces.

- Claude Code remains connected to `http://127.0.0.1:18787`.
- No VS Code reload or Claude Code restart is required.
- The original profile files are not overwritten.
- Provider tokens stay in the local Claude configuration files and are never
  returned by the management API.
- Only the selected profile filename is persisted in `bridge-state.json`.

Double-click a model row, or select it and click **切换到选中模型**. The same
window can start and stop the bridge. When the Windows service is installed,
its scripts use the service lifecycle and the bridge's graceful shutdown
endpoint instead of a hidden console process.

The Delphi source is under:

```text
gui\delphi11\ClaudeBridgeManager
```

Build it with:

```powershell
gui\delphi11\ClaudeBridgeManager\build-gui.cmd
```

The local-only management endpoints used by the GUI are:

- `GET /admin/status`
- `GET /admin/profiles`
- `POST /admin/active-profile`
- `POST /admin/reload-profiles`
- `POST /admin/gemini-proxy`
- `POST /admin/gemini-proxy/test`
- `POST /admin/shutdown`
