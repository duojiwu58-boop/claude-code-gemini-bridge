# Claude Code Multi-Model Bridge

Local adapter and model router for Claude Code. It can translate Anthropic
Messages requests to Google AI Studio Gemini, or forward them to other
Anthropic-compatible providers.

It accepts the OpenAI Responses API shape used by Codex and converts requests
to Gemini's OpenAI-compatible Chat Completions endpoint. It also accepts the
Anthropic Messages API shape used by Claude Code, including streamed text and
tool-use events.

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
