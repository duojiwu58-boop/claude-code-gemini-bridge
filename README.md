# Claude Code Multi-Model Bridge

**English** | [简体中文](README.zh-CN.md)

> Connect Claude Code to Gemini, DeepSeek, Qwen, Kimi, and other AI models while preserving as much of each model's native capability as its API exposes.

![Claude Code Model Center switching between Gemini, DeepSeek, Kimi, Qwen, and other providers without a restart](gui/delphi11/ClaudeBridgeManager/assets/ClaudeBridgeManager-screenshot.png)

Claude Code is more than a chat client. Its agent loop depends on thinking lifecycles, streamed events, parallel tool calls, cross-turn tool state, structured output, usage accounting, context-limit signals, and retry semantics. A basic API proxy may return text successfully while losing the behaviors required by a real agent task.

This project therefore aims beyond “make the request work”:

- Preserve Claude Code's agent, MCP, tool-use, and task-orchestration experience.
- Prefer the provider API that retains the most native capability instead of flattening every model to the lowest common denominator.
- Map Claude Code request semantics into the reasoning, tool, cache, and output controls the selected model actually supports.
- Reconstruct downstream responses as the Anthropic lifecycle Claude Code needs to keep running.
- Diagnose fields that cannot be mapped faithfully instead of silently dropping them or inventing capabilities the upstream does not have.

The selected downstream model always generates the final answer. The bridge handles transport, semantic translation, and state continuity; it does not answer by keyword or rewrite the model's final output.

## The architecture at a glance

```text
Claude Code
Anthropic Messages · agent · MCP · tools · thinking · media
                              │
                              ▼
             Claude Code Multi-Model Bridge
 semantic decode · capability mapping · state · streaming · diagnostics
             ┌────────────────┼────────────────┐
             ▼                ▼                ▼
   Anthropic Messages   Gemini Interactions   OpenAI Responses
      direct route        native stateful       semantic events /
                             protocol             server tools
             └────────────────┬────────────────┘
                              ▼
                    OpenAI Chat Completions
                    broad compatibility path
                              │
                              ▼
          Gemini · DeepSeek · Qwen · Kimi · other models
```

## What “maximizing model capability” means

| Capability surface | Bridge policy |
| --- | --- |
| Reasoning and thinking | Map effort, thinking budgets, and provider reasoning fields; reconstruct the Anthropic thinking-block lifecycle while streaming |
| Tool use | Accumulate streamed arguments, validate and conservatively repair JSON, preserve parallel order, and replay state across “think → call tool → receive result → continue thinking” turns |
| Native state | Use `previous_interaction_id` or `previous_response_id` only after an exact conversation-branch match; safely replay full history when uncertain |
| Structured output | Translate JSON Schema and either preserve or sanitize it for the provider; fail explicitly when the requested contract cannot be satisfied |
| Streaming | Incrementally decode real SSE across network-frame and UTF-8 boundaries, then emit events in the order Claude Code expects |
| Usage and caching | Map input, output, reasoning, cache read/write, and server-tool usage; prefer native token-count endpoints when available |
| Multimodality | Pass images and PDFs to natively capable transports; optionally give text-only models bounded visual evidence through Vision Proxy |
| Error semantics | Preserve authentication, rate-limit, overload, refusal, context-limit, truncation, and abnormal-stream termination semantics so Claude Code can retry or compact correctly |
| Capability downgrades | Log the downgrade and return repeatable `x-claude-bridge-warning` diagnostics instead of presenting a lossy fallback as full support |

“Near-lossless” does not mean byte-for-byte forwarding. It means preserving every intent, event, and state transition that can be represented safely across two different protocols. The bridge does not pretend that a model or API has a capability it does not expose.

## Deeply adapted models

| Model | Recommended transport | Key adaptations | Optional paths |
| --- | --- | --- | --- |
| **Gemini 3.7 Flash** | `gemini-interactions` | Current `step.*` SSE variants, exact stateful continuation, thinking/signatures, 1M context, images and PDFs (including Claude Code `Read` tool results), native token count, structured output, detailed usage, Google server-side tools, and opt-in native Remote MCP | OpenAI Chat fallback |
| **DeepSeek V4 Flash** | `anthropic` | Minimal translation of the Claude Code contract; Anthropic and Chat routes provide disabled/high/max reasoning modes and keep 16K budgets at high; Chat omits ordinary reasoning only in tool-free requests and fully replays it whenever `tools` are present, as required by the API | Stateless `openai-responses`, OpenAI Chat, Vision Proxy |
| **Qwen3.8 Max** | `anthropic` | Three effective Anthropic/Chat reasoning modes (`low/medium/xhigh`) with bounded normal-turn budgets instead of permanent maximum reasoning; exact Responses continuation, seven-level native effort, session cache, usage and latency observability | Stateful `openai-responses`, OpenAI Chat |
| **Kimi K3** | `anthropic` | Bearer authentication, verified `kimi-k3` model ID, 1M context metadata, native token estimate, and cache usage; Chat fallback maps Kimi effort, reasoning replay, and structured output | OpenAI Chat and explicitly enabled Kimi Formula MCP tools |
| **Claude Sonnet 5 / Opus 5 through OpenRouter** | `anthropic` | Native Messages/SSE pass-through, signed adaptive thinking, strict/parallel tools, prompt caching, structured output, image/PDF input, web search/fetch, OpenRouter Bearer auth, and 1M/128k model metadata | OpenRouter extensions remain upstream-dependent |
| **Other compatible models** | Provider-dependent | Anthropic pass-through or the generic OpenAI Chat/Responses semantic core, with differences declared through `capabilities` | Depth depends on the upstream API |

“Callable” is not the same as “deeply adapted.” Priority models are reviewed against their provider API contracts and covered by request/response fixtures for reasoning, tools, streaming, usage, and continuation. Generic providers receive only the capabilities proven by their actual fields and explicit configuration.

Providers may change model IDs, regional domains, and API behavior. Repository templates record configurations verified by this project; check the [Provider configuration guide](PROVIDER_CONFIG.md) and [changelog](CHANGELOG.md) before upgrading.

For `anthropic/claude-sonnet-5` and `anthropic/claude-opus-5` on OpenRouter, the bridge keeps successful Anthropic Messages responses as byte streams and only repairs documented request incompatibilities. Removed manual thinking becomes adaptive thinking with summarized display when the legacy request did not choose a display mode; unsupported non-default sampling fields are omitted. Opus 5 alone also caps `xhigh`/`max` effort at `high` when thinking is explicitly disabled and no profile override replaces that client setting. Every compatibility edit is reported through `x-claude-bridge-warning`. Generic Anthropic version/beta headers are preserved for Anthropic-compatible upstreams; user-profile and OpenRouter attribution headers are forwarded only to an OpenRouter host. Upstream rate-limit/tracing headers are returned to the client. OpenRouter does not currently expose Anthropic Token Count, Files, or Message Batches endpoints, so token counting remains explicitly marked as `estimated`, and those missing APIs are not emulated.

To override Claude Code's process-wide effort for one model, set top-level
`"reasoning_effort": "high"` in that `bridge-providers\*.json` file (the other accepted values are
`none`, `minimal`, `low`, `medium`, `xhigh`, and `max`). The profile override wins over the incoming
Claude request and the Gemini GUI level; omitting it preserves existing behavior.
`none` authoritatively disables thinking and removes its budget. Any non-`none` override also replaces a client's
`thinking.disabled` with adaptive thinking, so the configured effort cannot be silently neutralized.
`capabilities.default_reasoning_effort` remains a fallback only, while the boolean
`capabilities.reasoning_effort` still controls whether effort is sent at all.

### Gemini 3.7 Flash for local project development

The native `gemini-interactions` route on the default `standard` tier is sufficient for ordinary local repository work. Its verified core path covers text and SSE streaming, Thinking/signatures, Claude Code local tools and exact tool-result continuation, images and PDFs, structured output, native token counting, usage/cache/tier observability, and restart-safe stateful conversations. Google Search, URL Context, and Code Execution can be enabled when a task benefits from current web evidence or a server-side sandbox.

You do **not** need to provision File Search stores, Remote MCP servers, Google Maps, Flex, or Priority to read, search, edit, build, test, debug, or review a local project. Those are opt-in cloud or production extensions: File Search is for Google-hosted RAG corpora, Remote MCP connects external business systems, Maps serves geospatial work, Flex trades latency/reliability for lower cost, and Priority pays a premium for lower latency. Leaving them disabled keeps local development simpler and avoids unnecessary external resources, data flows, and billing modes.

The project's supported Gemini scope is deliberately Interactions-first and local-development-first. It does not implement the legacy `generateContent` transport or its explicit `cachedContents` lifecycle; Interactions' native stateful continuation and implicit caching are used instead. File Search store creation, local-directory cloud synchronization, the standalone Files/Batch management APIs, Live API sessions, background Interaction management, and a Computer Use executor are also currently out of scope. Existing optional Interactions extensions such as Maps, configured File Search queries, Remote MCP, Flex, and Priority remain available only through explicit profile configuration; they are default-off extensions, not part of the locally validated baseline.

Gemini implicit caching is managed by Google and a qualifying repeated prefix is **not guaranteed** to hit on every request. The bridge preserves stateful `previous_interaction_id` continuation and stable interaction-scoped configuration, maps a reported hit to Anthropic `usage.cache_read_input_tokens`, and retains Google's raw count in `provider_metadata.google.interaction_usage.total_cached_tokens`. A zero value means that Google reported no hit for that request; by itself it does not indicate a bridge failure. Larger stable prefixes sent close together can improve hit probability, while deterministic explicit-cache creation remains outside the Interactions transport.

Gemini Interactions terminal `failed` and `cancelled` states are returned as errors rather than successful `end_turn` messages. Google Search and URL Context usage counts cover the complete interaction even when the bounded diagnostic trace keeps only its first 32 distinct server-tool steps. Continuation mappings become available to fast tool clients immediately and are persisted at the terminal boundary without holding the global continuation lock during disk I/O.

### Qwen3.8 Max reasoning notes

- Budget-only Anthropic and Chat requests use `low` below 8,192 tokens, `medium` below 31,999, and `xhigh` at 31,999 or above. Responses keeps its finer mapping: `<2K / <8K / <31,999 / >=31,999` becomes `low / medium / high / xhigh`. This lets Claude Code's 31,999-token ultrathink ceiling reach Qwen's highest tier while keeping ordinary turns cheaper.
- Chat caps effective `low` and `medium` thinking budgets at 4,096 and 16,384 tokens. Anthropic preserves the requested thinking budget; when `max_tokens <= budget_tokens`, the bridge raises `max_tokens` to `budget_tokens + 8,192` so visible output still has room.
- Official DashScope and Bailian Qwen domains automatically receive `x-dashscope-session-cache: enable` on Responses and Anthropic transports. Responses caching is verified; the Anthropic header's live cache effect and the endpoint's acceptance of injected `output_config.effort` still require live validation. Set `capabilities.responses_session_cache` or `capabilities.reasoning_effort` to `false` to disable either behavior.
- Anthropic profiles use `x-api-key` by default. If a Bailian workspace returns HTTP 401, set `auth_scheme` to `bearer`. See the [Qwen provider notes](PROVIDER_CONFIG.md#deepseek--qwen-推荐配置与-responses) for complete examples and diagnostics.

## Four first-class transport paths

| `protocol` | Use it when | Trade-off |
| --- | --- | --- |
| `anthropic` | The provider exposes an Anthropic Messages endpoint | Minimal translation and usually the preferred Claude Code route for DeepSeek, Qwen, and Kimi |
| `gemini-interactions` | Calling Google's native Gemini Interactions API | Preserves Google-native state, events, and server tools; storage is configurable and enabled by default |
| `openai-responses` | The provider officially exposes a Responses API | Supports semantic Responses items/events, server tools, and validated stateful continuation |
| `openai` | Calling an OpenAI Chat Completions-compatible endpoint | Broadest coverage; provider dialects restore DeepSeek, Qwen, Kimi, Gemini, or generic extensions where available |

There is no universally best protocol. Choose the least lossy route for the current provider: prefer an official Anthropic endpoint, provider-native protocol, or Responses API when it preserves more semantics; use Chat Completions as the broad compatibility fallback.

## Quick start

### 1. Install the Windows service

Most users do not need Rust, Delphi, or Inno Setup. Download a package from [GitHub Releases](https://github.com/duojiwu58-boop/claude-code-gemini-bridge/releases/latest):

- `ClaudeCodeBridge-<version>-Setup.exe`: recommended; installs the Windows service, Model Center, Start Menu entries, and uninstaller.
- `ClaudeCodeBridge-<version>-windows-x64.zip`: extract the complete archive, then run `Install.cmd`.

The installer listens on:

```text
http://127.0.0.1:18787
```

The Windows service is named `ClaudeCodeBridge`. The installer backs up and updates Claude Code's `settings.json` so Claude Code always connects to this stable local endpoint. Restart any running Claude Code session after installation or upgrade so the process receives the current settings; a session started before the authentication upgrade can otherwise receive `401 Unauthorized`.

During an upgrade, the installer snapshots the existing service account, command line, start mode, description, security descriptor, environment, recovery settings, and running state. If a later installation step fails, it restores that service configuration (or removes a service created by the failed attempt) before reporting the error. GUI installs capture the pre-UAC user's profile, Pictures, Desktop, and SID so per-user Claude files and shortcuts are not redirected into a different administrator account. This original-user recovery is unavailable only when Setup itself was launched from an already elevated process.

The installer also generates a random 256-bit local bridge token, stores it in the access-controlled `C:\ProgramData\ClaudeCodeBridge\local-auth-token` file, and configures Claude Code, the Model Center, shutdown scripts, and the `gemini-image` MCP entry to send it automatically. A valid existing token is reused on upgrade. New local-token, Gemini-key, and install-metadata files receive their restrictive ACL before secret bytes are written, then are atomically published. The runtime refuses every non-loopback listen address. `/health` and `/v1/models` remain public local diagnostics; Messages, Responses, token count, MCP, and every `/admin/*` route require the configured Bearer token. This credential authenticates callers to the local bridge only and is never reused as a Gemini or provider API key.

For source development, `scripts\start-bridge.ps1` reuses an explicit token, an installed `C:\ProgramData\ClaudeCodeBridge\local-auth-token`, or an existing development token; if none exists, it atomically creates an ACL-protected `target\local-auth-token`. The bundled stop and test scripts resolve the same token automatically. Only direct launches of the executable must provide a token of at least 32 characters through `GEMINI_BRIDGE_LOCAL_TOKEN` or point `GEMINI_BRIDGE_LOCAL_TOKEN_FILE` at a protected file. Use `-LocalTokenFile` with the start script when a different development-token path is required.

When a Gemini key is supplied, the installer creates a native `gemini-interactions` profile and keeps the real Google credential in the protected service credential file rather than duplicating it in provider JSON.

### 2. Add a provider

The default provider directory is:

```text
%USERPROFILE%\.claude\bridge-providers\
```

Each `.json` file represents one hot-switchable model. A generic OpenAI-compatible service needs only three fields:

```json
{
  "model": "the provider's actual model ID",
  "base_url": "the base_url from the provider SDK example",
  "api_key": "your API key"
}
```

For a priority model, copy its adapted template instead of guessing from the minimal configuration:

- [Gemini native Interactions](examples/providers/gemini.example.json)
- [DeepSeek V4 Flash](examples/providers/deepseek.example.json)
- [DeepSeek V4 Pro](examples/providers/deepseek-v4-pro.example.json)
- [Qwen3.8 Max](examples/providers/qwen.example.json)
- [Kimi K3](examples/providers/kimi.example.json)
- [Generic OpenAI-compatible provider](examples/providers/custom-openai.example.json)
- [Capability overrides](examples/providers/capability-overrides.example.json)

Save the file, open **Claude Code Model Center**, select **Reload profiles**, and choose the model. The next request uses the new route without restarting VS Code, Claude Code, or the bridge service.

When the active profile is Gemini 3.7 Flash over `gemini-interactions`, Model Center also exposes **Low / Medium / High** Thinking controls. A selection is persisted and takes effect on the next request immediately—no service or Claude Code restart is required.

See the [Provider configuration guide](PROVIDER_CONFIG.md) for the complete field reference, regional endpoints, authentication, proxies, Responses, Vision Proxy, and legacy migration.

### 3. Verify the service

```powershell
Invoke-RestMethod -Uri 'http://127.0.0.1:18787/health'
```

The health response reports the active profile, actual model, transport, and upstream URL without returning provider API keys.

## Cross-model enhancements

### Vision Proxy

A text-only model can send an image or PDF to a designated vision provider first, then continue reasoning and tool use with the current target model:

```json
{
  "vision": {
    "mode": "proxy"
  }
}
```

The original media is sent to the vision provider, and bounded extracted evidence is sent to the target provider. This can incur two model calls and two sets of charges, and both providers' data policies must be considered. Vision Proxy is intended for code screenshots, terminals, GUIs, web pages, and single-page OCR; it is not a bulk scanned-document OCR or pixel-precise localization engine. See [Vision Proxy configuration](PROVIDER_CONFIG.md#通用-vision-proxy) for detailed limits.

### MCP extensions

- `gemini-image`: the Windows installer can register a `generate_image` tool for Claude Code, returning a preview and saving the generated file under the `ClaudeCodeBridge` folder in the Windows known Pictures directory.
- Kimi Formula: only official Formula tools explicitly listed in `kimi_formula_tools` are exposed; the feature is off by default.
- Google server-side tools: `google_search`, `url_context`, `code_execution`, `google_maps`, and File Search must be explicitly enabled in the Gemini profile. Maps/File Search objects retain their native options. Because Gemini does not accept `application/pdf` together with native Code Execution, the bridge omits only `code_execution` for a request containing PDF input and reports the downgrade; Search, URL Context, custom functions, and other compatible tools remain available, while non-PDF requests keep Code Execution.
- Gemini native Remote MCP: configure validated HTTPS Streamable HTTP servers through `gemini_remote_mcp_servers`; optional authorization header values are redacted from management output.

Server-side search, execution, Maps, configured File Search queries, Remote MCP, and priority/flex service tiers may incur extra charges or require provisioned resources. The bridge does not enable them by default and does not provision or synchronize their external resources.

## Configuration and operations

| Item | Default location or value |
| --- | --- |
| Local endpoint | `http://127.0.0.1:18787` |
| Windows service | `ClaudeCodeBridge` |
| Production binaries | `C:\Program Files\ClaudeCodeBridge` |
| State and logs | `C:\ProgramData\ClaudeCodeBridge` |
| Claude Code settings | `%USERPROFILE%\.claude\settings.json` |
| Provider profiles | `%USERPROFILE%\.claude\bridge-providers\*.json` |
| Health check | `GET /health` |
| Provider list | Authenticated `GET /admin/profiles` |
| Reload providers | Authenticated `POST /admin/reload-profiles` |

Manual management calls can reuse the installer-managed token without printing it:

```powershell
$bridgeToken = [System.IO.File]::ReadAllText('C:\ProgramData\ClaudeCodeBridge\local-auth-token').Trim()
$headers = @{ Authorization = "Bearer $bridgeToken" }
Invoke-RestMethod -Uri 'http://127.0.0.1:18787/admin/status' -Headers $headers
```

Streaming requests retain the 15-second connection timeout but have no ten-minute whole-response deadline; a stream that produces no bytes for two minutes is terminated explicitly. Non-streaming requests keep the ten-minute deadline. Streamed tool arguments are cumulatively limited to 8 MiB. Vision Proxy fails the request if any media block cannot be represented, accepts at most 16 historical media messages, and analyzes up to four of them concurrently before restoring message order.

Each native provider owns an independent HTTP client and does not automatically inherit the Windows system proxy or a legacy Gemini proxy setting. Configure a proxy in the relevant profile when needed:

```json
{
  "proxy": "http://127.0.0.1:8080"
}
```

An API key can be stored directly in a provider profile or referenced through `api_key_env`, provided the variable is visible to the Windows service process. Never commit, screenshot, or distribute a configuration file containing a real credential.

Kimi K3's `context_window: 1048576` is exposed through the management API and `/v1/models` metadata. To make the Claude Code client auto-compact against the full 1M window, set `CLAUDE_CODE_AUTO_COMPACT_WINDOW=1048576` before starting Claude Code. Restart the Claude Code session before a very long task after switching between models with different context sizes.

Gemini 3.7 Flash supports a 1,048,576-token input window and up to 65,536 output tokens. Set the profile's `max_output_tokens` and Claude Code's `CLAUDE_CODE_MAX_OUTPUT_TOKENS` to `65536` to expose the full output limit. Native Interactions continuation survives bridge restarts through an opaque local sidecar that contains no prompt or tool-result text; split trailing Claude Code tool results and runtime `system` context messages are scanned together before selecting `previous_interaction_id`, while runtime context remains in `system_instruction`. The default profile does not truncate tool results. Thinking signatures, assembled server-tool deltas, Google annotations, and the actual service tier are preserved for stateful text-coding workflows; built-in tools may use native option objects. When Claude Code exposes `Read`, `Grep`, or `Glob`, the bridge appends an accuracy-first source-navigation coach: perform no more than two consecutive content-and-line discovery searches before reading the best located source, read complete logical units in sufficiently large non-overlapping ranges, track proven and missing evidence after each result, and continue until every material claim is supported. It never forces an incomplete answer merely because many useful reads were needed. The separate safety breaker still forces `tool_choice: none` after three consecutive completed cycles with identical canonical tool names, arguments, and results. Computer Use remains intentionally disabled because it requires a client-side browser executor, screenshot loop, and safety confirmations that this bridge does not provide.

## Design boundaries

- The bridge can preserve only capabilities exposed by the upstream API; it cannot manufacture reliable tool use, vision, or reasoning state for a basic chat endpoint.
- A Chat Completions fallback is generally more likely to lose semantics than an official Anthropic, Interactions, or Responses route.
- Stateful APIs retain part of the conversation on the provider's servers; evaluate the provider's privacy and retention policy before enabling them.
- Provider model IDs, context limits, quotas, prices, and regions can change; actual API results are authoritative.
- Generic API compatibility does not guarantee that a model is suitable for a coding agent. Reliable operation still depends on model quality, context, streaming, and tool support.
- When a field cannot be mapped safely, this project prefers an explicit warning or failure over a response that appears successful while violating the caller's contract.

## Acceptance criteria for deep model adaptation

This project does not consider a model adapted merely because it returned text. A model entering the priority support tier should be reviewed and tested for:

1. The recommended official transport, authentication, model ID, regional endpoints, and context specification.
2. Request mapping for thinking, effort, and budget, plus streamed reasoning output.
3. The complete “think → call tool → receive result → continue thinking” replay cycle.
4. Parallel tools, structured arguments, truncated calls, and multimodal tool results.
5. Cache, reasoning-token, server-tool, and total usage accounting.
6. Exact stateful continuation, edited history, cache eviction, and safe fallback.
7. Rate limits, overload, context limits, refusal, and abnormal stream termination.
8. Official request/response fixtures, streaming fixtures, and regression tests.

These criteria are the project's verifiable definition of “maximizing model capability.”

## Build from source

The bridge runtime is written in Rust, while the Windows Model Center uses Delphi VCL. Delphi is not required to build only the bridge service:

```powershell
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked --release --target x86_64-pc-windows-msvc
```

For development, use `scripts\start-bridge.ps1` and `scripts\stop-bridge.ps1`. For a production Windows installation or upgrade, use the released installer rather than registering a binary from the development tree as the production service.

## Documentation

- [Provider configuration guide](PROVIDER_CONFIG.md)
- [Provider templates](examples/providers)
- [Rust source layout](src/README.md)
- [Changelog](CHANGELOG.md)
- [Windows package guide (English and Chinese)](packaging/windows-x64/使用说明.txt)

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE). You may use, modify, and redistribute it, provided derivative works are also released under GPL-3.0 with source available.
