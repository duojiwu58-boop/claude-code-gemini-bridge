# Claude Code Multi-Model Bridge

**English** | [简体中文](README.zh-CN.md)

> A deeply adapted semantic gateway and multi-model Agent Runtime for Claude Code on Windows.

![Claude Code Model Center switching between Gemini, DeepSeek, Kimi, Qwen, and other providers without a restart](gui/delphi11/ClaudeBridgeManager/assets/ClaudeBridgeManager-screenshot.png)

Use Claude Code as one stable coding-agent entry point while choosing Gemini, DeepSeek, Qwen, Kimi, OpenRouter Claude, or another compatible model behind it. The bridge preserves the reasoning, streaming, tools, state, usage, and error semantics that a real Claude Code task depends on instead of reducing every provider to “HTTP 200 plus text.”

This project is not a thin request-schema converter. It is built around three responsibilities:

- **Semantic adaptation:** translate the Claude Code lifecycle into the richest safe protocol exposed by each provider.
- **Stateful agent continuity:** preserve thinking, tool calls/results, parallel batches, structured output, cache/usage signals, and exact conversation branches.
- **Explicit multi-model execution:** let the selected coding model orchestrate bounded MCP tools backed by specialized models or services.

For ordinary turns, the selected downstream model generates the answer. For an MCP extension, that model explicitly requests a tool and the declared executor produces the artifact or result. The bridge does not route prompts by keyword, impersonate capabilities the active model lacks, or rewrite the model's final answer.

## What works today

The labels below deliberately separate live evidence from source-level support.

| Status | Meaning |
| --- | --- |
| **Verified** | Passed through a real VS Code Claude Code client, the installed Windows service, and the named upstream path |
| **Opt-in** | Implemented by the bridge, but requires explicit configuration, an external resource, or a different billing tier |
| **Out of scope** | Not exposed by the current Claude Code entry or intentionally not implemented; upstream model support alone does not make it available |

| Capability | Status | Current behavior |
| --- | --- | --- |
| Text, system instructions, and SSE | **Verified** | Normal and streamed Claude Code turns preserve message lifecycle and terminal errors |
| Thinking and reasoning controls | **Verified** | Provider-aware effort/budget mapping; Gemini 3.7 exposes Low/Medium/High with signed thought continuity |
| Claude Code local tools | **Verified** | Function declarations, streamed arguments, tool results, failures, and continued reasoning survive round trips |
| Parallel client tools | **Verified** | Gemini calls are held until terminal `requires_action`; live Claude Code validation showed overlapping independent read-only `Grep` calls |
| Structured output | **Verified** | JSON Schema is translated, sanitized only when required, and rejected explicitly when the contract cannot be represented |
| Images and PDFs | **Verified** | Native media paths include Claude Code `Read` tool results; Vision Proxy covers configured text-only targets |
| Token count, usage, and caching | **Verified** | Native count endpoints when available; input/output/reasoning/cache/server-tool usage is mapped without inventing hits |
| Gemini server tools | **Verified / Opt-in** | Google Search, URL Context, and Code Execution are live-tested; Maps and configured File Search queries are opt-in |
| Image generation | **Verified** | `generate_image` MCP delegates to `gemini-3.1-flash-image` by default and returns a preview plus a saved path |
| Remote MCP, Kimi Formula, Flex/Priority | **Opt-in** | Implemented with validation and redaction; disabled unless explicitly configured |
| Gemini audio/video input from Claude Code | **Out of scope** | Gemini supports the modalities, but the current Anthropic Messages entry has no audio/video block mapping |
| Computer Use, Files/Batch/store management, Live/background APIs | **Out of scope** | Require client executors or platform-management surfaces that this local coding bridge does not expose |

“Callable” is not the same as “deeply adapted.” A generic compatible provider receives only the semantics its real API and explicit profile can support.

## Architecture

```text
                              Claude Code
              Anthropic Messages · MCP · local tools · media
                                  │
                                  ▼
                   Claude Code Multi-Model Bridge
          semantic decode · capability mapping · state · SSE
                diagnostics · auth · usage · model switching
                     ┌────────────┴────────────┐
                     │                         │
                     ▼                         ▼
              Model transports          Tool execution
       Anthropic Messages                    ├─ Claude Code local tools
       Gemini Interactions                   ├─ Google server tools
       OpenAI Responses                      ├─ Gemini Remote MCP
       OpenAI Chat fallback                  └─ bridge MCP extensions
                     │                               │
                     ▼                               ▼
       Gemini · DeepSeek · Qwen · Kimi      specialized executors
          OpenRouter Claude · others        Gemini Image · Kimi Formula
```

The bridge chooses the least-lossy configured transport, not a universal lowest common denominator:

| `protocol` | Preferred use | Semantic trade-off |
| --- | --- | --- |
| `anthropic` | Provider exposes an Anthropic Messages endpoint | Minimal translation; normally preferred for DeepSeek, Qwen, Kimi, and OpenRouter Claude |
| `gemini-interactions` | Google Gemini native Interactions API | Preserves Google state, steps, thinking signatures, server tools, and native continuation |
| `openai-responses` | Provider officially exposes Responses | Preserves Responses items/events, server tools, and validated stateful continuation |
| `openai` | OpenAI Chat Completions-compatible service | Broadest compatibility; more semantics may need provider-specific reconstruction or explicit downgrade |

## Five-minute Windows setup

### 1. Install

Download from [GitHub Releases](https://github.com/duojiwu58-boop/claude-code-gemini-bridge/releases/latest):

- `ClaudeCodeBridge-<version>-Setup.exe`: recommended; installs the service, Model Center, Start Menu entries, `gemini-image` MCP registration, and uninstaller.
- `ClaudeCodeBridge-<version>-windows-x64.zip`: extract the complete archive, then run `Install.cmd`.

The installer creates the automatic Windows service `ClaudeCodeBridge` and configures Claude Code to use:

```text
http://127.0.0.1:18787
```

Restart any running Claude Code session after installation or upgrade so it receives the new environment and local authentication settings.

### 2. Add or select a model

Provider profiles live in:

```text
%USERPROFILE%\.claude\bridge-providers\
```

A generic OpenAI-compatible profile can start with:

```json
{
  "model": "the provider's actual model ID",
  "base_url": "the base_url from the provider SDK example",
  "api_key": "your API key"
}
```

For deeply adapted models, start from a verified template:

- [Gemini native Interactions](examples/providers/gemini.example.json)
- [DeepSeek V4 Flash](examples/providers/deepseek.example.json)
- [DeepSeek V4 Pro](examples/providers/deepseek-v4-pro.example.json)
- [Qwen3.8 Max](examples/providers/qwen.example.json)
- [Kimi K3](examples/providers/kimi.example.json)
- [Generic OpenAI-compatible provider](examples/providers/custom-openai.example.json)
- [Capability overrides](examples/providers/capability-overrides.example.json)

Open **Claude Code Model Center**, select **Reload profiles**, and choose a model. The next request uses it without restarting VS Code, Claude Code, or the service. Active Gemini 3.7 Interactions profiles also expose hot **Low / Medium / High** Thinking controls.

### 3. Verify

```powershell
Invoke-RestMethod -Uri 'http://127.0.0.1:18787/health'
```

The response reports the active profile, actual model, transport, and upstream URL without exposing provider API keys.

## Deeply adapted models

| Model | Recommended path | Adapted surface |
| --- | --- | --- |
| **Gemini 3.7 Flash** | `gemini-interactions` | Current step-based SSE, Low/Medium/High Thinking, terminal-batched parallel client calls, exact stored continuation, 1M context, images/PDF, structured output, native token count, detailed usage/cache/tier, Google tools, optional native Remote MCP |
| **DeepSeek V4 Flash / Pro** | `anthropic` when available | Minimal Claude contract translation, provider-aware disabled/high/max reasoning, tool-turn reasoning replay, output-headroom protection; Responses/Chat fallbacks remain available |
| **Qwen3.8 Max** | `anthropic` or validated `openai-responses` | Meaningful effort tiers, bounded normal reasoning, exact Responses continuation, DashScope session cache, structured output, usage and latency diagnostics |
| **Kimi K3** | `anthropic` | Bearer auth, verified model ID, 1M context metadata, native token estimate and cache usage; Chat reasoning replay and opt-in Kimi Formula tools |
| **Claude Sonnet 5 / Opus 5 via OpenRouter** | `anthropic` | Messages/SSE pass-through, adaptive signed thinking, strict/parallel tools, structured output, prompt caching, image/PDF, web tools, OpenRouter auth and limit metadata |
| **Other compatible models** | Provider-dependent | Anthropic pass-through or the generic Responses/Chat semantic core; actual depth is declared by `capabilities` and proven behavior |

Model IDs, regional endpoints, quotas, pricing, and provider behavior can change. Use the [Provider configuration guide](PROVIDER_CONFIG.md) and [changelog](CHANGELOG.md) before changing a verified template.

## Gemini 3.7 Flash as a local coding baseline

The native `gemini-interactions` path on the `standard` tier is sufficient for ordinary repository work: read, search, edit, build, test, debug, and review. The verified path includes text/SSE, system instructions, signed Thinking, local tools and exact tool-result continuation, structured JSON, images/PDF, native token counting, stateful conversations, implicit cache reporting, and server-tool usage.

Gemini 3.7 Flash supports a 1,048,576-token input window and up to 65,536 output tokens. Set both the profile's `max_output_tokens` and Claude Code's `CLAUDE_CODE_MAX_OUTPUT_TOKENS` to `65536` to expose the full output ceiling. Implicit cache hits are controlled by Google and are not guaranteed; the bridge reports a hit only when the upstream reports one.

### Parallel tool calls

Gemini Interactions can emit multiple client function calls in one turn. The bridge retains translated `tool_use` events from the first client call until terminal `requires_action`, then emits the complete ordered batch under one assistant message with one `message_stop`. This prevents Claude Code from discovering a same-turn batch one call at a time.

The batch makes concurrency possible; Claude Code still decides what is safe to overlap. Independent read-only tools such as `Read`, `Grep`, and `Glob` may run concurrently, while side-effect-capable `Bash` calls may remain serialized. Live acceptance requires every intended `tool_dispatch_start` to occur before the first corresponding `tool_dispatch_end`; matching assistant message IDs alone are not proof of overlap.

### Image generation is explicit multi-model execution

Gemini 3.7 Flash does not natively emit images on this route. The installed path is:

```text
Gemini 3.7 Flash (reason and choose tool)
        │
        ▼
Claude Code calls generate_image over authenticated loopback MCP
        │
        ▼
bridge calls gemini-3.1-flash-image (generate the image)
        │
        ▼
MCP preview + MIME type + actual executor model + saved file path
```

The tool supports the documented aspect ratios and 1K/2K/4K output, uses high Thinking, and saves only under the current user's Windows known Pictures folder in `ClaudeCodeBridge`. The model cannot choose an arbitrary output directory. `GEMINI_BRIDGE_IMAGE_MODEL` can replace the fixed executor model; there is no automatic image-model selection or fallback.

### Optional Gemini extensions

Normal local coding does **not** require Google Maps, File Search stores, Remote MCP, Flex, or Priority. Enable them only when the task needs hosted RAG, external systems, geospatial context, or a different service tier. Google Search, URL Context, Code Execution, Maps, configured File Search queries, and Remote MCP can add data flows, resource requirements, or charges.

The current local-development scope intentionally excludes the legacy `generateContent` transport, explicit `cachedContents`, File Search store provisioning/local-directory sync, standalone Files/Batch management, Live API sessions, background Interaction management, and a Computer Use executor. Audio/video inputs are supported by the Gemini model but not mapped by the current Claude Code Messages entry.

## Cross-model extensions

### Vision Proxy

A text-only target can delegate an image or PDF to a configured vision provider, then continue reasoning with bounded extracted evidence:

```json
{
  "vision": {
    "mode": "proxy"
  }
}
```

The original media goes to the vision provider and the extracted observation goes to the target model, so one user request may cause two model calls, two charges, and two provider data flows. Vision Proxy is designed for screenshots, terminals, GUIs, webpages, and single-page OCR—not bulk scanned-document OCR or pixel-precise localization. See [Vision Proxy configuration](PROVIDER_CONFIG.md#通用-vision-proxy).

### MCP and server tools

- `gemini-image`: installer-managed local MCP image generation with preview and safe file persistence.
- Kimi Formula: only explicitly allowlisted official Formula URIs are exposed; default is empty.
- Gemini server tools: `google_search`, `url_context`, `code_execution`, `google_maps`, and configured File Search queries.
- Gemini native Remote MCP: validated HTTPS Streamable HTTP servers with redacted authorization values and optional tool allowlists.

### Per-model reasoning policy

A top-level profile value such as `"reasoning_effort": "high"` can override Claude Code's process/request effort for that model. Accepted values are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`; each transport maps or clamps them to what its upstream actually supports. See the [complete capability reference](PROVIDER_CONFIG.md#近乎无损兼容与能力覆盖).

## Security, data flow, and operations

The Windows distribution is designed as a local service rather than an unauthenticated LAN proxy:

- Runtime binding is loopback-only.
- The installer creates a random 256-bit local token and applies restrictive ACLs before secret bytes are published.
- Messages, Responses, token count, MCP, and every `/admin/*` route require Bearer authentication; `/health` and `/v1/models` remain local diagnostics.
- The local token is never reused as an upstream provider credential.
- Provider keys can be held in the protected service credential file, a profile, or a service-visible `api_key_env`; never commit or share a real key.
- Image MCP validates local Origin, bounds prompt/response/decoded sizes and total time, validates MIME, and never accepts a model-selected output path.
- Stateful upstream APIs may retain conversation state. Evaluate each provider's retention and privacy policy before enabling them.
- Vision Proxy, image generation, server tools, Remote MCP, and premium service tiers may create extra calls, charges, and external data flows.

Upgrades back up Claude settings and snapshot the existing service configuration. A failed installation restores the prior service state or removes only a service created by that failed attempt. The GUI preserves the pre-UAC user's profile, shell folders, and SID so per-user files are not silently redirected to an administrator account.

| Item | Default |
| --- | --- |
| Local endpoint | `http://127.0.0.1:18787` |
| Windows service | `ClaudeCodeBridge` |
| Production binaries | `C:\Program Files\ClaudeCodeBridge` |
| State, logs, protected credentials | `C:\ProgramData\ClaudeCodeBridge` |
| Claude Code settings | `%USERPROFILE%\.claude\settings.json` |
| Provider profiles | `%USERPROFILE%\.claude\bridge-providers\*.json` |
| Generated images | Windows known Pictures folder, `ClaudeCodeBridge` subdirectory |
| Health | `GET /health` |
| Authenticated status | `GET /admin/status` |

Native providers use independent HTTP clients and do not automatically inherit the Windows system proxy. Configure `proxy` in the relevant profile. Streaming requests keep a connection and idle deadline without a ten-minute whole-stream limit; non-streaming requests remain bounded. Exact limits and management calls are documented in [Provider configuration](PROVIDER_CONFIG.md).

## What “deeply adapted” means here

Returning text is not sufficient. A model enters the priority support tier only after reviewing and testing:

1. Official transport, authentication, model ID, regional endpoints, context, and output limits.
2. Thinking/effort/budget mapping and streamed reasoning output.
3. The complete think → tool call → tool result → continued thinking lifecycle.
4. Parallel calls, structured arguments, truncation, and multimodal tool results.
5. Input/output/reasoning/cache/server-tool usage accounting and token counting.
6. Exact stateful continuation, edited branches, cache eviction, restart recovery, and safe fallback.
7. Authentication, rate limits, overload, context limits, refusal, cancellation, and abnormal stream termination.
8. Official-contract request/response/stream fixtures, regressions, and real-client acceptance where the client boundary matters.

The current locked Rust suite contains 204 passing tests. Critical paths are also validated through the actual Windows service and VS Code Claude Code client; mock success alone is not treated as end-to-end compatibility.

## Build from source

The bridge service is Rust; the Windows Model Center is Delphi VCL. Delphi is not required to build the service:

```powershell
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked --release --target x86_64-pc-windows-msvc
```

Use `scripts\start-bridge.ps1` and `scripts\stop-bridge.ps1` for development. Use a released installer for production installation or upgrade instead of registering a development-tree binary as the service.

## Documentation

- [Provider configuration guide](PROVIDER_CONFIG.md)
- [Provider templates](examples/providers)
- [Rust source layout](src/README.md)
- [Changelog](CHANGELOG.md)
- [Windows package guide](packaging/windows-x64/使用说明.txt)

## License

Licensed under [GNU GPL v3.0](LICENSE). You may use, modify, and redistribute the project under the GPL-3.0 source-availability terms.
