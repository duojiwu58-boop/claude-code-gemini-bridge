# Changelog / 更新日志

All notable changes to the **Claude Code ↔ Gemini Deep-Compatibility Bridge** project will be documented in this file.  
本项目的所有重要更新记录均将在此文档中记录。

---

## English

## [Unreleased]

- Added a terminal batch barrier for Gemini Interactions client function calls. From the first `function_call` through terminal `requires_action`, translated `tool_use` events are retained and then emitted in order under one assistant message with one `message_stop`, preventing Claude Code from discovering same-turn calls one at a time. Live Claude Code validation confirmed overlapping execution for two independent read-only `Grep` calls; side-effect-capable `Bash` calls remain subject to the client's serialization policy.
- Added a validated top-level Provider `reasoning_effort` override. A model profile can now replace Claude Code's process-wide/request effort across native Anthropic, OpenAI Chat, OpenAI Responses, Gemini Interactions, and local Gemini while preserving the existing default-only and capability-switch semantics. Profile overrides also outrank the Gemini GUI level; absent fields remain strict no-ops.
- Added first-class OpenRouter Anthropic handling for Claude Sonnet 5 and Opus 5: OpenRouter API keys use documented Bearer authentication; removed manual thinking is converted to adaptive thinking with summarized display when unspecified; incompatible non-default sampling fields are omitted with explicit bridge warnings. Opus 5 additionally caps `xhigh`/`max` effort at `high` when thinking is disabled, without changing Sonnet 5 or other models.
- Preserved Anthropic user-profile/OpenRouter attribution request headers plus upstream rate-limit and tracing response headers. `/v1/models` now identifies the active profile model and reports both Claude 5 models' official 1M context and 128k output limits instead of a stale bridge default. OpenRouter token counting remains explicitly estimated because its current Anthropic API exposes no native count endpoint. The locked Rust suite now contains 204 passing tests.
- Made profile-level reasoning overrides authoritative even when a client disables thinking, limited OpenRouter-only attribution headers to OpenRouter hosts, accepted explicit plain-text output formats on Chat fallbacks, and normalized Kimi Responses effort tiers consistently with Kimi Chat.
- Fixed Gemini/OpenAI streaming edge cases: terminal failed/cancelled Interactions no longer become successful `end_turn` responses, safety-block chunks retain usage, server-tool counters continue beyond the bounded diagnostic trace, continuation state is persisted once outside the global lock, and schema property names no longer trigger ignored-field warnings. Kimi Formula MCP operations now have a 60-second deadline.
- Hardened Windows installation by protecting local-token, API-key, and install-metadata ACLs before writing secret bytes; restoring existing service configuration or removing a newly created service after failure; and capturing the pre-UAC user's profile, shell folders, and SID for GUI installs.

## [v0.7.2] - 2026-08-27

### Local Security and Reliability Remediation

- Enforced loopback-only binding and authenticated Messages, Responses, token-count, MCP, and admin routes with an installer-generated 256-bit local token. Claude settings, MCP registration, Model Center, and shutdown scripts now receive the token automatically; upgrades reuse a valid installed token, and local authentication is no longer reused as an upstream provider credential.
- Source-development launch, stop, and test scripts now resolve the same local token automatically. When no installed, explicit, or existing development token is available, the launcher creates `target\local-auth-token` through an exclusive same-directory temporary file whose ACL is protected before any secret bytes are written, then atomically publishes it.
- Removed the ten-minute whole-response deadline from streaming requests while retaining connection, idle, and non-streaming deadlines. Added cumulative 8 MiB streamed tool-argument bounds and preserved inline Gemini and object-shaped Responses arguments instead of silently replacing them with `{}`.
- Completed Gemini Interactions stream fidelity for tool parameters that appear only on `step.stop`: the bridge now emits a full `input_json_delta` before `content_block_stop`, so Claude Code reconstructs the same input retained by continuation state. Object-shaped non-streaming thought summaries are decoded to plain text rather than serialized JSON wrappers.
- Made Vision Proxy fail closed on untranslatable media, bounded historical media work, and parallelized accepted jobs with ordered injection. Continuation recovery is now error-specific, cache eviction refreshes recency, and unknown Gemini stream events are observable without treating incomplete streams as successful.
- Redacted proxy userinfo from public management output and logs, allowlisted Responses built-in/server-tool types, and accepted digit-only string forms for primary token-limit fields plus OpenAI Chat streaming/Responses usage. Anthropic `web_search_*` declarations are no longer mistranslated as ordinary Responses functions; provider-native Responses tools remain explicitly opt-in.
- Bounded Gemini/Responses server-tool metadata to 32 diagnostic summaries and replaced any serialized argument/action/result/signature value above 4,096 characters with an explicit truncated preview, preventing unbounded search or execution output from entering client responses.
- Fixed Gemini PDF requests under profiles that enable native Code Execution: the bridge omits only `code_execution` for the affected request, reports the compatibility downgrade, and retains Search, URL Context, custom functions, and other compatible tools. Non-PDF Code Execution is unchanged.
- Documented and live-verified Gemini Interactions implicit caching as upstream-managed and probabilistic. Reported hits map exactly to Anthropic `usage.cache_read_input_tokens` and remain available in raw Google provider metadata; an eligible request that reports zero cached tokens is not by itself a bridge failure.
- Made Windows JSON settings writes atomic, stopped unhealthy newly installed services, recorded a protected pre-install Claude environment snapshot, and restored unchanged bridge-managed values during uninstall.
- Expanded the locked Rust regression suite to 187 passing tests, including PDF/tool compatibility, bounded server-tool traces, stop-only tool arguments, thought-summary normalization, poisoned continuation-cache recovery, Responses web-search filtering, and numeric-string usage.

## [v0.7.0] - 2026-08-26

### Gemini 3.7 Flash Six-Capability Completion

- Fixed Claude Code PDF `Read` continuation: documents are removed from `function_result.result` (which only accepts text/image) and emitted as a following native `user_input`; native token counting mirrors the same legal split.
- Decoded structured `thought_summary.content` deltas to plain Anthropic thinking text instead of leaking serialized JSON wrappers.
- Preserved native Google Maps and File Search options, including map widget/location fields, File Search metadata filters, and configured store names.
- Added validated, opt-in Gemini native Remote MCP servers with HTTPS Streamable HTTP URLs, safe names, optional `allowed_tools`, and redacted authorization header values in management output.
- Restricted Gemini 3.7 Flash service-tier configuration to the supported `standard`, `flex`, and `priority` values; the stale `deferred` value is now rejected.
- Documented the recommended local-development baseline: native Interactions, stateful continuation, and the `standard` tier provide the complete core coding workflow without provisioning File Search, Remote MCP, Maps, Flex, or Priority. These cloud/production extensions remain explicitly opt-in.
- Defined the Gemini product boundary around Claude Code local development: the legacy `generateContent` transport, explicit `cachedContents`, File Search store provisioning/local sync, standalone Files/Batch management, Live/background Interaction management, and Computer Use execution are intentionally unsupported for now. Existing optional Interactions extensions remain default-off profile features.
- Added an accuracy-first Gemini Interactions source-navigation coach for Claude Code. Requests exposing `Read`, `Grep`, or `Glob` receive a final-position system policy for high-information search, complete logical-unit reads, large non-overlapping ranges, evidence-gap tracking, and accuracy-first completion. Fixed navigation-call cutoffs were removed so useful long investigations remain enabled; the exact three-identical-cycle safety breaker remains.

### Native Interactions Reliability and Fidelity

- Made the default stateful Claude Code path near-lossless for text coding: packaged profiles no longer truncate tool results, signature-only thoughts remain replayable, streamed Google tool deltas and annotations are retained, actual service tier is observable, and built-in tool objects preserve native Google options.
- Restored exact stored continuation when Claude Code appends runtime `system` context after a tool result or splits one tool turn across trailing messages; every current result is forwarded with the matched `previous_interaction_id`, and runtime context remains a system instruction.
- Added a hard repeated-tool circuit breaker: three consecutive completed cycles with identical canonical tool names, arguments, and results override every normal tool-choice setting with `none`, emit a diagnostic, and request the best final answer. Changed arguments or results reset the guard.
- Expanded the Rust regression suite from 121 to 167 passing tests with Gemini 3.7 REST SSE, function-tool pause, usage details, reasoning levels, hot GUI override, persistence, prefill, bridge-managed credentials, native token counting, optional server tools, restart-safe continuation, loop safety, and source-navigation coverage.

## [v0.6.0] - 2026-08-19

### Gemini 3.7 Flash and Current Interactions API

- Updated runtime, installer, provider, Claude settings, test scripts, documentation, and marketing defaults to the GA `gemini-3.7-flash` model. The native profile records its 1,048,576-token context window and uses the model's recommended `medium` default thinking level.
- Changed the Windows installer's generated Gemini profile to the native `gemini-interactions` transport. Its Google key remains in the service-managed credential file instead of being duplicated in provider JSON, while the native Messages and Token Count paths inherit the bridge's global Gemini proxy.
- Added dual-schema SSE compatibility for both the formal `event_type`/`thought_summary`/`arguments_delta` resource schema and the `type`/`thought`/`arguments` shapes in the Gemini 3.7 migration examples. Initial thought signatures, initial model-output content, and terminal `interaction.requires_action` tool streams are now preserved.
- Normalized all Claude reasoning settings to Gemini 3.7's supported `low`/`medium`/`high` levels. Disabled, `none`, and `minimal` requests use the model's minimum `low` level; `xhigh` and `max` clamp to `high`.
- Mapped current Interactions usage (`total_input_tokens`, `total_output_tokens`, `total_thought_tokens`, and `total_cached_tokens`) plus the migration aliases (`prompt_tokens` and `completion_tokens`) into Claude usage. Thought tokens are included in `output_tokens` and also exposed as `reasoning_tokens`.
- Reject assistant-prefill requests locally with a clear Anthropic error because Gemini 3.7 no longer accepts prefilled model turns. Deprecated sampling parameters remain suppressed, and `candidate_count` is diagnosed and ignored.
- Added a Model Center **Low / Medium / High** control for active Gemini 3.7 Flash Interactions profiles. The persisted override applies to the next request without restarting the service and intentionally takes precedence over request-level effort/budget and the profile default.
- Expanded the Rust regression suite from 121 to 130 passing tests with Gemini 3.7 REST SSE, function-tool pause, usage-detail, reasoning-level, hot GUI override, persistence, prefill, bridge-managed credential, and native token-count coverage.

## [v0.5.1] - 2026-08-08

### Qwen3.8-Max Capability Maximization

- Changed Qwen's budget-to-effort mapping so a 31,999-token `thinking.budget_tokens` — Claude Code's strongest ultrathink budget — now selects `xhigh` instead of stopping at `medium` on the Anthropic, Chat, and Responses transports. Smaller budgets stay in the low/medium tiers so routine tool turns remain cheap.
- When `max_tokens <= budget_tokens`, the Qwen and DeepSeek Anthropic routes now raise `max_tokens` to `budget_tokens + 8,192`, preventing strict endpoint validation failures and preserving visible-output headroom instead of squeezing the answer into a single token.
- The Anthropic transport now sends `x-dashscope-session-cache: enable` for official Qwen domains, matching the Responses path. The flag stays toggleable through `capabilities.responses_session_cache`; live effectiveness on the Anthropic endpoint is pending confirmation, and upstreams that do not support the header ignore it.
- Documented the `reasoning_effort: false` escape hatch in case a Bailian Anthropic endpoint rejects the injected `output_config.effort`, and the `auth_scheme: bearer` fallback for workspaces that return 401 against `x-api-key`.
- Set the bridge Claude Code settings template to `CLAUDE_CODE_EFFORT_LEVEL=max` for maximum-effort DeepSeek operation; this is a Claude Code process-wide environment setting rather than a Provider-profile field.
- Split the former 16,394-line `src/main.rs` into responsibility-focused Rust source slices and a separate test module. The crate root is now roughly 100 lines while preserving existing private item paths and behavior through a deliberately mechanical `include!` decomposition.
- 121 Rust tests pass, adding coverage for the ultrathink-to-xhigh mapping, both providers' output-headroom protection, and the Anthropic session-cache header; rustfmt and strict Clippy remain clean.

## [v0.5.0] - 2026-08-08

### DeepSeek V4 Flash and Qwen3.8-Max Deep Adaptation

- Switched the recommended DeepSeek/Qwen profiles to their native Anthropic Messages endpoints and current `deepseek-v4-flash` / `qwen3.8-max` model IDs.
- Added provider-aware Chat fallbacks: DeepSeek reasoning replay, thinking type, high/max effort, and incompatible tool-choice suppression; Qwen thinking enablement/budget preservation and structured-output mapping.
- Refined DeepSeek reasoning control across Anthropic and Chat routes into disabled/high/max operating modes: low-effort Claude turns disable thinking and 16K budgets remain high until the 32K max threshold. Chat omits ordinary reasoning only in tool-free requests and fully replays it whenever `tools` are present, as required by the API; both routes log the effective policy and estimated replay-token cost.
- Reworked Qwen reasoning control so Anthropic/Chat use meaningful disabled/low/medium/xhigh modes instead of silently promoting routine Claude `high` turns to Qwen `xhigh`. A 31,999-token budget now selects medium; Chat caps low/medium budgets at 4K/16K while explicit xhigh/max remains uncapped, and Anthropic automatically keeps `max_tokens` above the thinking budget.
- Kept all seven native Qwen Responses effort levels, with a medium bridge default and a separate budget-to-effort mapping. Qwen routes now report effective policy and upstream response-header latency; Chat reports replay size, and unary Chat/Responses report cache and reasoning usage. Missing `JSON` prompt keywords produce an explicit compatibility warning.
- Added the first-class `openai-responses` transport with semantic unary/SSE translation, function and server-side tools, DeepSeek custom `apply_patch`, DeepSeek stateless history replay, and branch-safe Qwen `previous_response_id` continuation with session caching.
- Mapped cache-read, cache-creation, and reasoning token details for Chat and Responses usage. Provider compatibility downgrades are logged and returned through repeatable `x-claude-bridge-warning` headers.
- Added official-contract fixtures for both providers, including the complete thinking → tool call → tool result → continued reasoning state path. The Rust suite now contains 120 tests.

### Kimi K3 1M Deep Adaptation

- Switched the recommended Kimi profile to the Anthropic-compatible endpoint with the verified working model ID `kimi-k3`, explicit Bearer authentication, and 1,048,576-token context metadata. Provider profiles now support `auth_scheme` and `context_window`.
- Added the Kimi Chat fallback dialect with complete `reasoning_content` replay across tool turns, K3 `low/high/max` effort mapping, `max_completion_tokens`, fixed-sampling suppression, JSON Schema output, stable hashed `prompt_cache_key`/`safety_identifier`, and direct `usage.cached_tokens` accounting.
- Routed Claude Code token counting through Kimi's native `/v1/tokenizers/estimate-token-count` endpoint with a bounded local-estimate fallback.
- Added opt-in Kimi Formula tools through the bridge MCP server. Only explicitly listed official Formula URIs are exposed and executed; the default empty list has no tool cost or behavior change.
- Added official-contract tests for Anthropic Bearer profiles, the reasoning → tool → result replay path, stable cache identity, native token estimation, and Formula discovery/execution.

### Gemini Interactions Compatibility and Native Tools

- Replaced local estimation on `/v1/messages/count_tokens` with Google's native `models.countTokens` for `gemini-interactions` profiles. The request includes the translated prompt, system instruction, custom function declarations, structured-output schema, and supported media; bounded upstream failures fall back to the local estimate and expose the count source through `x-claude-bridge-token-count`.
- Mapped Anthropic `output_config.format` to native Interactions `response_format`, `output_config.effort` to `thinking_level` with conservative `xhigh`/`max` → `high` clamping, document URL/text/content sources to native document input, and `service_tier: standard_only` to Gemini `standard` while leaving `auto` at Google's standard default.
- Added explicit compatibility diagnostics for Anthropic fields that Gemini Interactions cannot represent. Diagnostics are logged and returned through repeatable `x-claude-bridge-warning` headers instead of being silently discarded; malformed or unsupported structured-output formats return a request error.
- Added bounded server-tool event return through `provider_metadata.google.interaction_server_tools` for unary and streaming responses, plus standard Anthropic usage counters for Google Search and URL Context calls. The bridge does not fabricate Anthropic citation/result blocks when Google does not provide the required encrypted citation fields.
- Added one-shot compatibility fallbacks for two upstream gaps: mixed Google server tools plus Claude Code function tools retry with function tools only after the known 400 rejection, and unsupported `previous_interaction_id` requests retry with safe full-history recovery after HTTP 501.
- Added native `google_maps` configuration and File Search through `gemini_file_search_store_names`. Documented the current reachable modality and safety boundaries for audio/video, Computer Use, and remote MCP Server tools.
- Expanded regression coverage to 98 Rust tests, including native token-count endpoint verification, request-semantic mappings, explicit diagnostics, both compatibility fallback paths, and server-tool metadata/usage translation.

### Native Stateful Gemini Interactions

- Added the independent `gemini-interactions` transport for Google's native `/v1beta/interactions` API, using `x-goog-api-key` authentication and `store: true`. The existing OpenAI Chat-compatible Gemini route remains available as a fallback.
- Added bounded, branch-safe conversation continuation. Exact transcript matches reuse `previous_interaction_id`; Claude/MCP tool-result turns recover the interaction through the opaque tool call ID.
- Fixed tool-continuation races with fast clients. Call-ID mappings are now recorded as soon as a streamed function step closes, before the overall interaction stream completes. If neither the call-ID nor exact-transcript mapping survives, recovery uses neutral historical observations plus an explicit real-tool instruction instead of replaying rejected function steps or imitable `[Tool call: ...]` text.
- Implemented the current step-based unary and streaming schemas, including thought summaries and signatures, model output, custom function calls/results, streamed function arguments, images and documents, and Anthropic Messages/SSE translation.
- Added high-thinking defaults, deprecated-sampling suppression, and optional Google server-side tools through `gemini_builtin_tools`: `google_search`, `url_context`, and `code_execution`.
- Updated the Gemini provider examples and documentation, including the privacy implication that Google stores request and response state when this transport is enabled.

### Interactions Verification

- 89 Rust tests passed, covering continuation isolation, tool-result linking, safe missing-continuation recovery, request conversion, response conversion, and streaming translation.
- Live Windows-service verification confirmed transcript and tool-call continuations, a Claude Code custom-tool loop, all three Google server-side tools, successful recovery of the formerly failing tool-history probe, an immediate-result race probe, service health, and deployed-binary integrity.

### Gemini Image Generation

- Added a loopback Streamable HTTP MCP endpoint exposing `generate_image`, backed by the high-quality `gemini-3.1-flash-image` model with high thinking and 1K/2K/4K output options.
- Generated images are returned as MCP image previews and saved under `Pictures\ClaudeCodeBridge` in the user's Windows known Pictures folder, including redirected/OneDrive locations. The service never accepts arbitrary output paths from the model.
- The Windows installer automatically registers the user-scoped `gemini-image` MCP server and removes only that registration during uninstall. Existing Claude MCP configuration is backed up and preserved.
- Added bounded prompt, response, decoded-image, and total-time limits, local-Origin validation, safe MIME handling, and fallback to credentials/proxy settings from an official Google Gemini profile.

### Verification

- 82 Rust tests passed, including MCP Origin checks and an end-to-end mock image-generation tool call.
- Live Windows-service verification generated valid 1K and 2K JPEG images, returned MCP previews, and confirmed writes by `NT SERVICE\ClaudeCodeBridge` to the current user's Pictures directory.

### Release Verification

- Final refreshed `v0.5.0` verification passed all 98 Rust tests, `rustfmt`, strict Clippy, the static-CRT release build, portable ZIP, Inno Setup installer, and SHA-256 manifest verification.
- Delphi 11 Release GUI compiled as `0.5.0.0` with 0 warnings and 0 errors.
- The static-CRT bridge, portable ZIP, Inno Setup installer, and SHA-256 manifest were rebuilt and verified successfully.

## [v0.4.0] - 2026-08-07

### Generic Vision Proxy

- Added an opt-in, Provider-neutral Vision Proxy for text-only targets. Any native target profile can set `"vision":{"mode":"proxy"}` without model-name-specific routing.
- Gemini is the default vision Provider: the bridge prefers a native local Gemini profile and then an official Google Gemini profile. `vision.profile` can explicitly select another native-vision profile using the Gemini, OpenAI Chat, or Anthropic transport.
- Vision preprocessing now runs once at the Anthropic message layer before target routing, so streaming and non-streaming requests, OpenAI-compatible targets, Anthropic targets, user images, PDFs, and media inside tool results share one implementation.
- Original media is removed before forwarding to a text-only target and replaced with bounded visual evidence marked as untrusted data. Profiles with missing references, self-reference, or proxy chains are rejected during loading.

### OCR Fidelity, Caching, and Failure Semantics

- Changed the vision prompt from concise summarization to task-aware, lossless extraction. Text-heavy images and translation/explanation requests require complete verbatim OCR in reading order, preserving paragraphs, code, numbers, punctuation, and source language without ellipses.
- Raised the vision output budget from 1,200 to 4,096 tokens. Vision context is capped at 12,000 Unicode characters and the injected observation at 16,000 characters.
- Added a 90-second per-analysis timeout and a 128-entry SHA-256 in-memory cache for successful base64 media observations. URL media is never cached because its content can change; the cache is cleared on service restart.
- Vision failures are returned explicitly instead of silently asking the text model to guess. Rate-limit and overload semantics are preserved; automatic tiling, paging, resizing, bulk OCR, and runtime vision-Provider failover are not yet implemented.

### Documentation and Verification

- Expanded the bilingual README and Provider guide with configuration examples, routing and privacy boundaries, prompt-injection cautions, hard limits, latency/cost behavior, and recommended use cases.
- Updated the DeepSeek example profile to enable Vision Proxy.
- 71 Rust tests passed, including profile validation, media removal, nested tool-result media, request construction, observation parsing, SHA-256 cache reuse, and a mock vision Provider integration test.
- Live Windows-service verification paired Gemini vision with DeepSeek V4 Flash. A text-dense Chinese screenshot was transcribed through its bottom checkpoint and translated completely; the first request took about 26.7 seconds and the cached request about 7.0 seconds.
- Delphi 11 Release GUI compiled with version `0.4.0.0`, 0 warnings, and 0 errors. Strict Clippy, the static-CRT bridge build, portable ZIP inspection, Inno Setup compilation, and SHA-256 manifest verification completed successfully.

## [v0.3.3] - 2026-08-07

### Stable GUI Cleanup

- Removed the Gemini-specific HTTP proxy editor, Windows proxy detection, connection test, and apply controls from the model center. Proxy routing is now configured only in each Provider profile.
- The model list's proxy column now consistently displays the selected profile's own `proxy` value, including Gemini profiles, instead of being overwritten by bridge-global Gemini state.
- Expanded the model list into the space previously occupied by the dedicated proxy panel and updated GUI/README wording to match the profile-owned configuration model.
- Updated the GUI file and product version to `0.3.3.0`.

### Verification

- 66 Rust unit tests passed.
- Delphi 11 Release GUI compiled with 0 warnings and 0 errors, and the rebuilt GUI passed a local startup/responsiveness smoke test.
- Static-CRT bridge build, portable ZIP, Inno Setup installer, and SHA-256 manifests completed successfully.

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

## [未发布]

- 为 Gemini Interactions 客户端函数调用新增终态批量屏障。从第一个 `function_call` 到终态 `requires_action`，转换后的 `tool_use` 事件会先保留，再按顺序在同一个 assistant message 下统一发出，并且只产生一个 `message_stop`，避免 Claude Code 逐个发现同轮调用。本机 Claude Code 实测确认两个独立只读 `Grep` 调用的执行区间发生重叠；可能产生副作用的 `Bash` 调用仍遵循客户端的串行策略。
- 新增经过严格校验的 Provider 顶层 `reasoning_effort` 强制值。单个模型 profile 现在可以在原生 Anthropic、OpenAI Chat、OpenAI Responses、Gemini Interactions 与本地 Gemini 路径中覆盖 Claude Code 的进程级/请求 effort，同时保留既有“仅兜底默认值”和“能力开关”语义。profile 强制值也高于 Gemini GUI 档位；未配置时严格保持原行为。
- 新增 OpenRouter Claude Sonnet 5 与 Opus 5 的原生 Anthropic 适配：OpenRouter API Key 按官方方式使用 Bearer 鉴权；已移除的手动 thinking 会转换为 adaptive thinking，未指定显示模式时保留摘要显示；不兼容的非默认采样字段会在给出明确桥接警告后省略。仅 Opus 5 在关闭 thinking 时会把 `xhigh`/`max` effort 限制为 `high`，Sonnet 5 与其他模型保持不变。
- 保留 Anthropic 用户 profile/OpenRouter 归属请求头及上游限流、追踪响应头。`/v1/models` 现在使用活动 profile 的真实模型 ID，并报告两个 Claude 5 模型官方 1M 上下文与 128k 输出上限，不再显示过期的桥接默认模型。由于 OpenRouter 当前 Anthropic API 没有原生计数端点，Token Count 仍明确标记为估算。锁定 Rust 回归现为 204 项且全部通过。
- Provider 级 reasoning 强制值现在即使遇到客户端关闭 thinking 也具有最高优先级；OpenRouter 专属归属请求头只发往 OpenRouter；Chat fallback 接受显式纯文本输出格式；Kimi Responses 与 Kimi Chat 使用一致的 effort 档位归一化。
- 修复 Gemini/OpenAI 流式边界：Interactions 的 failed/cancelled 终态不再伪装成成功 `end_turn`，安全拦截分片保留 usage，服务端工具计数不受诊断 trace 上限影响，续接状态在释放全局锁后仅于终态落盘，工具 schema 属性名也不再触发“字段被忽略”误报。Kimi Formula MCP 操作新增 60 秒时限。
- 加固 Windows 安装：本地令牌、API Key 与安装元数据会在写入密钥字节前先设置受限 ACL；失败时恢复原服务配置或删除本轮新建服务；GUI 安装通过原始用户令牌捕获 UAC 前用户的 profile、Shell 目录与 SID。

## [v0.7.2] - 2026-08-27

### 本地安全与可靠性修复

- 强制只允许 loopback 监听，并用安装器生成的 256-bit 随机本地令牌保护 Messages、Responses、Token Count、MCP 与全部管理路由。Claude settings、MCP 注册、模型中心和停止脚本会自动取得令牌；升级会复用已有有效令牌，本地鉴权令牌也不再被误作上游 Provider 凭据。
- 源码开发的启动、停止和测试脚本现在会自动解析同一本地令牌。没有安装令牌、显式令牌或已有开发令牌时，启动脚本会创建同目录独占临时文件，在写入任何密钥字节前先保护 ACL，再原子发布为 `target\local-auth-token`。
- 流式请求不再受 10 分钟整流总时限截断，同时保留连接、空闲和非流式超时。流式工具参数新增累计 8 MiB 上限；Gemini 内联参数与 Responses 对象参数不再被静默替换为 `{}`。
- 补全 Gemini Interactions 中仅在 `step.stop` 出现的工具参数：桥接器会在 `content_block_stop` 前发送完整 `input_json_delta`，使 Claude Code 重建的输入与续接状态一致。非流式对象型 thought summary 会解码为纯文本，不再输出 JSON 包装串。
- Vision Proxy 对无法转换的媒体改为明确失败，限制历史媒体任务数量，并在有界并发分析后按消息顺序注入。续接恢复改为错误特异判断，缓存淘汰会刷新最近使用顺序，未知 Gemini 流事件可观测但不会掩盖不完整流。
- 管理输出与日志会脱敏代理 URL 的用户信息，Responses 内建/服务端工具类型改为 allowlist，并兼容主要 token 限制字段及 OpenAI Chat 流式/Responses Usage 的纯数字字符串形式。Anthropic `web_search_*` 声明不再被错误转换成普通 Responses function；供应商原生 Responses 工具仍需显式启用。
- Gemini/Responses 服务端工具 metadata 最多保留 32 条诊断摘要；参数、动作、结果或签名值序列化后超过 4,096 字符时会替换为明确的截断预览，避免搜索或代码执行输出无界进入客户端响应。
- 修复启用原生 Code Execution 的 profile 处理 Gemini PDF 请求时失败的问题：只对受影响请求省略 `code_execution` 并报告兼容性降级，Search、URL Context、自定义函数及其他兼容工具继续保留；非 PDF 请求不受影响。
- 文档化并在线验证 Gemini Interactions 隐式缓存由上游管理且命中具有概率性。命中值会精确映射到 Anthropic `usage.cache_read_input_tokens` 并保留在 Google 原始 provider metadata；满足条件但 cached tokens 为 0 不能单独说明桥接器故障。
- Windows JSON 配置改为原子写入；安装后健康检查失败会停止异常服务；安装器保存受保护的 Claude 环境变量安装前快照，卸载时只恢复仍保持桥接器安装值的项目。
- 锁定的 Rust 回归测试扩展到 187 个且全部通过，新增覆盖 PDF/工具兼容、服务端工具摘要边界、stop-only 工具参数、thought summary 规范化、中毒续接缓存恢复、Responses web-search 过滤及数字字符串 Usage。

## [v0.7.0] - 2026-08-26

### Gemini 3.7 Flash 六项能力补全

- 修复 Claude Code PDF `Read` 续接：PDF 不再放入只允许 text/image 的 `function_result.result`，而是作为随后一条原生 `user_input` 发送；原生 Token Count 采用相同合法结构。
- 将结构化 `thought_summary.content` delta 解码为纯 Anthropic Thinking 文本，不再泄漏 JSON 包装字符串。
- 保留 Google Maps 与 File Search 原生选项，包括地图 widget/位置字段、File Search metadata filter 和 store 名称。
- 新增显式启用、经过校验的 Gemini 原生 Remote MCP：只接受 HTTPS Streamable HTTP、安全名称和可选 `allowed_tools`，管理输出会脱敏鉴权 header 值。
- Gemini 3.7 Flash 服务档位严格限制为受支持的 `standard`、`flex`、`priority`，陈旧的 `deferred` 配置会被拒绝。
- 明确记录推荐的本地开发基线：原生 Interactions、有状态续接和 `standard` 档位已覆盖完整核心编码流程，无需预先创建 File Search、Remote MCP、Maps、Flex 或 Priority；这些云端/生产扩展继续保持显式按需启用。
- 明确 Gemini 产品边界以 Claude Code 本地开发为核心：传统 `generateContent` transport、显式 `cachedContents`、File Search store 创建/本地同步、独立 Files/Batch 管理、Live/后台 Interaction 管理及 Computer Use 执行目前均不支持；已实现的 Interactions 扩展继续作为默认关闭的 profile 可选项保留。
- 新增准确性优先的 Gemini Interactions 源码导航教练：提供 `Read`、`Grep` 或 `Glob` 的请求会在系统指令末尾获得高信息量搜索、完整逻辑单元读取、大范围非重叠阅读、证据缺口跟踪和准确性优先完成规则。固定导航次数截断已移除，长时间但持续获得证据的调查不会被提前终止；完全相同三轮的安全熔断继续保留。

### 原生 Interactions 可靠性与保真度

- 默认有状态 Claude Code 文本编程路径现已接近无损：随包 profile 不再裁剪工具结果，仅有签名的 Thinking 仍可回放，流式 Google 工具 delta 与 annotations 得到保留，实际 service tier 可观测，内置工具对象也会保留 Google 原生选项。
- 修复 Claude Code 在工具结果后追加运行时 `system` 上下文或拆分尾部消息时无法命中原生状态的问题；现在会完整转发本轮结果、使用匹配的 `previous_interaction_id`，并保持运行时上下文的 system 优先级。
- 新增硬性重复工具熔断：若连续三个已完成轮次的规范化工具名、参数和结果完全相同，最终工具策略会无条件改为 `none`，同时给出诊断并要求模型直接作答；参数或结果变化会重置计数。
- Rust 回归测试由 121 增加到 167 个且全部通过，新增覆盖 3.7 REST SSE、函数工具暂停、Usage 细分、推理档位、GUI 热切换与持久化、prefill、桥接器托管凭据、原生 Token Count、可选服务端工具、跨重启续接、循环安全及源码导航。

## [v0.6.0] - 2026-08-19

### Gemini 3.7 Flash 与最新 Interactions API

- 将运行时、安装器、Provider、Claude settings、测试脚本、文档和推广内容的默认模型升级为 GA 的 `gemini-3.7-flash`。原生配置记录 1,048,576-token 上下文窗口，并采用模型推荐的 `medium` 默认 Thinking 档位。
- Windows 安装器生成的 Gemini profile 已切换到原生 `gemini-interactions` transport。Google Key 继续只保存在服务托管的凭据文件中，不会重复写入 Provider JSON；原生 Messages 与 Token Count 路径会继承桥接器全局 Gemini 代理。
- 同时兼容正式资源 schema 的 `event_type`/`thought_summary`/`arguments_delta`，以及 Gemini 3.7 迁移示例中的 `type`/`thought`/`arguments`。现在会保留 step 起始签名、起始模型文本，并把 `interaction.requires_action` 正确视为工具流终态。
- 将 Claude 的所有推理设置规范化到 Gemini 3.7 支持的 `low`/`medium`/`high`：disabled、`none`、`minimal` 使用最低的 `low`，`xhigh`、`max` 收敛到 `high`。
- 映射最新 Interactions Usage（`total_input_tokens`、`total_output_tokens`、`total_thought_tokens`、`total_cached_tokens`）及迁移示例别名（`prompt_tokens`、`completion_tokens`）。Thinking tokens 会计入 `output_tokens`，并单独暴露为 `reasoning_tokens`。
- Gemini 3.7 不再接受 assistant prefill，因此桥接器会在本地返回清晰的 Anthropic 请求错误；废弃采样参数继续被抑制，`candidate_count` 会产生诊断并被忽略。
- 模型中心新增 Gemini 3.7 Flash Interactions 专用的“低 / 中 / 高”Thinking 控件；选择会持久化并从下一次请求生效，无需重启服务，且明确优先于请求级 effort/budget 和 profile 默认值。
- Rust 回归测试由 121 增加到 130 个且全部通过，新增覆盖 3.7 REST SSE、函数工具暂停、Usage 细分、推理档位、GUI 热切换与持久化、prefill、桥接器托管凭据及原生 Token Count。

## [v0.5.1] - 2026-08-08

### Qwen3.8-Max 能力最大化

- 调整 Qwen 的 budget→effort 映射：31,999 token 的 `thinking.budget_tokens`（Claude Code ultrathink 最强思考触发的预算上限）现在进入 `xhigh`，不再被压在 `medium`；Anthropic、Chat、Responses 三条路径一致。中小预算继续停留在 low/medium 档，控制常规工具轮次的费用。
- `max_tokens <= budget_tokens` 时，Qwen 与 DeepSeek 的 Anthropic 路径都会把 `max_tokens` 提高到 `budget_tokens + 8,192`，既避免严格端点校验失败，也为可见输出保留余量，防止答案被挤压成 1 个 token。
- Anthropic transport 现在对官方 Qwen 域名发送 `x-dashscope-session-cache: enable`，与 Responses 路径一致，可用 `capabilities.responses_session_cache` 关闭；该请求头在 Anthropic 端点的实际缓存效果尚待线上确认，不支持的上游会忽略该头。
- 文档补充：百炼 Anthropic 端点若拒绝注入的 `output_config.effort`，可用 `reasoning_effort: false` 回退；工作区端点对 `x-api-key` 返回 401 时可改用 `auth_scheme: bearer`。
- 桥接器的 Claude Code 配置模板改为 `CLAUDE_CODE_EFFORT_LEVEL=max`，用于 DeepSeek 最高 effort；该值属于 Claude Code 进程级环境设置，不是 Provider profile 字段。
- 将原先 16,394 行的 `src/main.rs` 按职责拆为多个 Rust 源码切片，并把测试独立成文件；crate root 现在约 100 行，通过机械式 `include!` 拆分保持原有私有路径和运行行为不变。
- 121 个 Rust 测试全部通过，新增 ultrathink→xhigh、两家供应商输出余量保护与 Anthropic 会话缓存头的覆盖；rustfmt 与严格 Clippy 保持零告警。

## [v0.5.0] - 2026-08-08

### DeepSeek V4 Flash 与 Qwen3.8-Max 深度适配

- DeepSeek/Qwen 推荐配置切换为原生 Anthropic Messages endpoint 与当前模型 ID `deepseek-v4-flash` / `qwen3.8-max`。
- 新增供应商感知的 Chat fallback：DeepSeek 推理回放、thinking type、high/max effort 与不兼容 tool-choice 抑制；Qwen thinking 开关/预算保留及结构化输出映射。
- DeepSeek 的 Anthropic 与 Chat 路径统一为 disabled/high/max 三种实际运行态：Claude 低 effort 轮次关闭 thinking，16K budget 在 32K max 阈值前保持 high；Chat 仅在请求完全不携带工具时省略普通推理，携带 `tools` 时按契约完整回放全部推理。两条路径每次请求都会报告有效策略与估算回放 Token 成本。
- Qwen 的 Anthropic/Chat 路径改为真正可区分的 disabled/low/medium/xhigh 运行态，不再把 Claude 普通 `high` 轮次静默抬成 Qwen `xhigh`。31,999 budget 现在选择 medium；Chat 将 low/medium 预算限制为 4K/16K，显式 xhigh/max 不限流；Anthropic 自动保证 `max_tokens` 大于 thinking budget。
- Qwen Responses 保留原生七档 effort，并采用独立的 budget 映射和 bridge medium 默认值。Qwen 各路径会上报有效策略与上游响应头延迟，Chat 上报推理回放占用，普通 Chat/Responses 上报缓存与推理 Usage；结构化输出 prompt 缺少 `JSON` 关键字时返回明确诊断。
- 新增一级 `openai-responses` transport，支持语义化普通/流式事件、函数与服务端工具、DeepSeek custom `apply_patch`、DeepSeek 无状态历史回放，以及 Qwen 分支安全的 `previous_response_id` 续接与会话缓存。
- Chat 与 Responses Usage 完整映射缓存读取、缓存创建和推理 Token；兼容性降级会写入日志及可重复的 `x-claude-bridge-warning` 响应头。
- 新增两家官方契约 fixture，覆盖“思考 → 工具调用 → 工具结果 → 继续思考”的完整状态路径。
- Rust 回归测试现为 120 项。

### Kimi K3 1M 深度适配

- Kimi 推荐配置切换为 Anthropic-compatible endpoint、经实际验证可用的模型 ID `kimi-k3`、显式 Bearer 鉴权与 1,048,576 Token 上下文元数据；Provider 新增 `auth_scheme` 与 `context_window`。
- 新增 Kimi Chat fallback 方言：工具轮次完整回放 `reasoning_content`，映射 K3 `low/high/max` effort、`max_completion_tokens`、固定采样约束、JSON Schema、稳定散列的 `prompt_cache_key`/`safety_identifier`，并读取顶层 `usage.cached_tokens`。
- Claude Code Token 计数改为调用 Kimi 原生 `/v1/tokenizers/estimate-token-count`，受限失败时回退本地估算。
- 通过桥接器 MCP 新增显式启用的 Kimi Formula 官方工具；只暴露配置列出的 URI，默认空数组不会产生工具费用或行为变化。
- 新增 Anthropic Bearer profile、思考→工具→结果回放、稳定缓存标识、原生 Token Estimate 与 Formula 发现/执行测试。

### Gemini Interactions 兼容性与原生工具

- `gemini-interactions` profile 的 `/v1/messages/count_tokens` 已由本地估算改为调用 Google 原生 `models.countTokens`，请求包含转换后的提示、系统指令、自定义函数声明、结构化输出 schema 和受支持媒体；受限的上游失败会回退到本地估算，并通过 `x-claude-bridge-token-count` 标明计数来源。
- 新增 Anthropic 请求语义映射：`output_config.format` → 原生 Interactions `response_format`，`output_config.effort` → `thinking_level`（`xhigh`/`max` 保守钳制为 `high`），文档 URL/文本/content → 原生文档输入，`service_tier: standard_only` → Gemini `standard`；`auto` 保持 Google 默认标准层级。
- 对 Gemini Interactions 无法表达的 Anthropic 字段新增明确兼容诊断：同时写入日志与可重复的 `x-claude-bridge-warning` 响应头，不再静默丢弃；格式错误或不支持的结构化输出会直接返回请求错误。
- 普通响应和流式响应新增有界的 `provider_metadata.google.interaction_server_tools` 服务端工具事件回传，并为 Google Search 与 URL Context 调用补充 Anthropic 标准 usage 计数。Google 未提供所需加密引用字段时，桥接器不会伪造 Anthropic 引用或工具结果块。
- 针对两个上游缺口增加单次兼容降级：混合 Google 服务端工具与 Claude Code 函数工具遇到已知 400 时仅用函数工具重试；`previous_interaction_id` 遇到 HTTP 501 时使用安全完整历史恢复重试。
- 新增原生 `google_maps` 配置，以及通过 `gemini_file_search_store_names` 配置 File Search；同时明确音频/视频、Computer Use 与远程 MCP Server 工具当前可达性和安全边界。
- Rust 回归测试扩充至 98 项，包括 Google 原生 token count 端点验证、请求语义映射、明确诊断、两条兼容降级路径及服务端工具元数据/usage 转换。

### Gemini 原生有状态 Interactions

- 新增独立的 `gemini-interactions` transport，直接调用 Google 原生 `/v1beta/interactions` API，使用 `x-goog-api-key` 鉴权并固定启用 `store: true`；原有 OpenAI Chat 兼容路径继续作为回退方案保留。
- 新增有界且分支安全的会话续接：对完全匹配的对话记录复用 `previous_interaction_id`；Claude/MCP 工具结果回合通过不透明的 tool call ID 找回对应 interaction。
- 修复快速客户端触发的工具续接竞态：流式函数 step 一结束便立即登记 call-ID 映射，不再等待整个 interaction 流完成。若 call-ID 与完全匹配的对话映射都不存在，恢复路径会使用中性的历史观察和“必须调用真实工具”指令，避免回放 Google 拒绝的函数 steps，也避免生成可被模仿的 `[Tool call: ...]` 文本。
- 实现当前基于 step 的普通响应与流式协议，包括思考摘要与签名、模型输出、自定义函数调用/结果、流式函数参数、图片和文档，并完整转换为 Anthropic Messages/SSE。
- 新增高思考默认值、废弃采样参数抑制，以及通过 `gemini_builtin_tools` 可选启用 Google 服务端 `google_search`、`url_context`、`code_execution` 工具。
- 更新 Gemini Provider 示例和文档，并明确说明启用该 transport 后，Google 会保存请求与响应状态。

### Interactions 验证

- 89 项 Rust 测试全部通过，覆盖续接隔离、工具结果关联、续接丢失时的安全恢复、请求转换、响应转换与流式转换。
- Windows 服务实测确认普通对话与工具调用续接、Claude Code 自定义工具闭环、三种 Google 服务端工具、原失败工具历史探针恢复成功、即时工具结果竞态探针、服务健康状态以及部署二进制一致性。

### Gemini 图片生成

- 新增仅监听本机的 Streamable HTTP MCP 入口，向 Claude Code 提供 `generate_image` 工具；默认使用高质量 `gemini-3.1-flash-image`、高思考，并支持 1K/2K/4K 输出。
- 生成结果既作为 MCP 图片预览返回，也保存到用户 Windows“图片”已知目录下的 `ClaudeCodeBridge` 文件夹，兼容重定向和 OneDrive；服务不接受模型指定任意输出路径。
- Windows 安装器会自动注册用户级 `gemini-image` MCP 服务；卸载时仅移除这一项。修改前会备份并保留用户原有 Claude MCP 配置。
- 增加提示长度、上游响应、解码后图片和总时长上限，并校验本机 Origin、图片 MIME；没有桥接器主密钥时，可复用 Google 官方 Gemini profile 的凭据和代理。

### 验证

- 82 项 Rust 测试全部通过，包括 MCP Origin 校验和端到端 mock 生图工具调用。
- Windows 服务真实生成 1K、2K JPEG 图片并正确返回 MCP 预览，确认 `NT SERVICE\ClaudeCodeBridge` 可以写入当前用户的“图片”目录。

### 发布验证

- 刷新后的 `v0.5.0` 最终验证通过全部 98 项 Rust 测试、`rustfmt`、严格 Clippy、静态 CRT Release 构建、便携 ZIP、Inno Setup 安装程序及 SHA-256 清单校验。
- Delphi 11 Release GUI 以 `0.5.0.0` 编译通过，0 警告、0 错误。
- 静态 CRT bridge、便携 ZIP、Inno Setup 安装程序及 SHA-256 清单均已重新构建并验证成功。

## [v0.4.0] - 2026-08-07

### 通用 Vision Proxy

- 新增显式启用、与目标模型名称无关的通用 Vision Proxy。任何原生目标 profile 都可以设置 `"vision":{"mode":"proxy"}`，无需为 DeepSeek 等模型写死路由分支。
- Gemini 作为默认视觉 Provider：桥接器优先选择本地原生 Gemini profile，其次选择 Google 官方 Gemini profile；也可通过 `vision.profile` 显式指定使用 Gemini、OpenAI Chat 或 Anthropic transport 的原生视觉 profile。
- 视觉预处理统一位于 Anthropic 消息层并发生在目标路由之前，因此流式与非流式请求、OpenAI-compatible 与 Anthropic 目标、普通用户图片、PDF 及工具结果内媒体共用同一实现。
- 发往纯文本目标前会移除原始媒体，替换为有界且标记为“不可信数据”的视觉证据。缺失引用、自引用和多级代理链会在 profile 加载时被拒绝。

### OCR 保真、缓存与故障语义

- 将视觉提示从“简洁摘要”改为任务敏感的近乎无损提取。对于文字密集图片以及翻译、总结、解释文字等请求，要求按阅读顺序完整逐字 OCR，保留段落、代码、数字、标点和原语言，禁止用省略号替代可见内容。
- 视觉输出预算由 1,200 提高到 4,096 tokens；视觉上下文最多 12,000 个 Unicode 字符，注入目标模型的观察最多 16,000 个字符。
- 增加单次视觉分析 90 秒超时，以及基于 SHA-256 指纹、最多 128 项的 base64 成功结果内存缓存。URL 媒体因内容可能变化而不缓存，服务重启后缓存清空。
- 视觉分析失败会明确返回错误，不会静默让纯文本模型猜图；保留限流和过载语义。当前尚未实现自动切图、分页、缩放、批量 OCR 和视觉 Provider 运行时故障转移。

### 文档与验证

- 扩充 README 和 Provider 配置指南的中英文说明，加入配置示例、路由与隐私边界、图片提示注入风险、硬限制、延迟/成本行为及推荐使用范围。
- DeepSeek 示例 profile 已默认启用 Vision Proxy。
- 71 项 Rust 测试全部通过，覆盖 profile 校验、媒体移除、工具结果嵌套媒体、请求构造、观察解析、SHA-256 缓存复用和本地 mock 视觉 Provider 集成测试。
- Windows 服务实测完成 Gemini 视觉与 DeepSeek V4 Flash 接力：文字密集中文长图成功 OCR 到底部校验码并完整翻译；首次请求约 26.7 秒，缓存后约 7.0 秒。
- Delphi 11 Release GUI 以 `0.4.0.0` 版本编译通过，0 警告、0 错误；严格 Clippy、静态 CRT bridge、便携 ZIP 内容检查、Inno Setup 编译及 SHA-256 清单验证均成功完成。

## [v0.3.3] - 2026-08-07

### 稳定版 GUI 清理

- 从模型中心移除 Gemini 专用 HTTP 代理编辑框、Windows 代理检测、连接测试和保存控件；代理路由现在只在各 Provider profile 中配置。
- 模型列表的“代理”列统一显示对应 profile 自己的 `proxy`，Gemini profile 也不再被桥接器全局 Gemini 状态覆盖。
- 模型列表扩展到原专用代理面板占用的空间，并同步更新 GUI 与 README 文案，使其符合 profile 自主管理配置的模式。
- GUI 文件版本和产品版本更新为 `0.3.3.0`。

### 验证

- 66 项 Rust 单元测试全部通过。
- Delphi 11 Release GUI 以 0 警告、0 错误完成编译，重新生成的 GUI 已通过本机启动与响应性冒烟测试。
- 静态 CRT Bridge 构建、便携 ZIP、Inno Setup 安装程序和 SHA-256 清单均成功完成。

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
