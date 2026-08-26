# Rust source layout

The bridge is a single binary crate, but its implementation is split by responsibility:

| File | Responsibility |
| --- | --- |
| `main.rs` | Crate imports, shared constants, source-slice assembly, and the test-module entry |
| `core.rs` | Shared provider, capability, routing-state, and application-state types |
| `runtime.rs` | Console/service entry points, loopback/authentication boundary, Axum router construction, and shutdown handling |
| `provider.rs` | Provider discovery, profile parsing, endpoint inference, clients, and persisted state |
| `mcp.rs` | Authenticated MCP protocol handling, Kimi Formula tools, and image generation |
| `vision.rs` | Generic Vision Proxy collection, request construction, caching, and injection |
| `admin.rs` | Health, model listing, authenticated profile switching/reload, redacted proxy administration, and shutdown APIs |
| `routing.rs` | Authenticated Anthropic/Responses handlers, route selection, and native Anthropic forwarding |
| `gemini_interactions.rs` | Gemini Interactions request/response translation and continuation state |
| `openai_chat_forward.rs` | OpenAI Chat upstream forwarding and response handling |
| `openai_responses.rs` | OpenAI Responses request/response translation and streaming |
| `gemini_streaming.rs` | Gemini Interactions streaming state machine |
| `anthropic_streaming.rs` | SSE decoding and OpenAI Chat-to-Anthropic streaming translation |
| `token_count.rs` | Native/fallback token counting and provider error normalization |
| `openai_chat.rs` | Anthropic-to-OpenAI Chat translation, provider reasoning policies, tools, and identity handling |
| `tests.rs` | Binary-crate regression tests |
| `windows_service.rs` | Windows Service Control Manager integration |

The production slices currently use `include!` at crate root. This intentionally keeps the first decomposition behavior-neutral: private item paths, cross-feature access, and the existing root-level tests remain unchanged. Convert slices to namespaced Rust modules only when their shared dependencies can be made explicit without a broad `pub(crate)` visibility expansion.

Because `rustfmt` does not reliably discover arbitrary `include!` targets, format and check every Rust source file from PowerShell with:

```powershell
$sourceFiles = Get-ChildItem -LiteralPath 'src' -Filter '*.rs' -File | Select-Object -ExpandProperty FullName
rustfmt --edition 2021 $sourceFiles
rustfmt --edition 2021 --check $sourceFiles
```

Behavioral verification remains:

```powershell
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked --release --target x86_64-pc-windows-msvc
```
