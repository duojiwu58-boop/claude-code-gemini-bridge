# Claude Code 多模型智能体运行时

[English](README.md) | **简体中文**

> 面向 Windows Claude Code 的深度语义网关。

![Claude Code 模型中心可在 Gemini、DeepSeek、Kimi、Qwen 等提供商之间免重启切换](gui/delphi11/ClaudeBridgeManager/assets/ClaudeBridgeManager-screenshot.png)

以 Claude Code 作为稳定的编码智能体入口，同时在后端选用 Gemini、DeepSeek、Qwen、Kimi、OpenRouter Claude 或其他兼容模型。桥接器保留真实 Claude Code 任务所依赖的推理、流式输出、工具、状态、用量和错误语义，而不是把所有提供商压缩成“HTTP 200 加一段文本”。

> **v0.8.0 重大升级：** 本运行时已针对 **Gemini 3.8 Flash 新提供的原生 Computer Use 能力**完成 Claude Code 深度适配。Gemini 决定动作，Claude Code 托管本地 stdio MCP 生命周期，Windows 用户态 Host 执行截图与输入；不需要打开模型中心，不调用第二个模型，也不需要手动启动 Host。

本项目不是一层简单的请求格式转换器，而是围绕三项职责构建：

- **语义适配：** 将 Claude Code 生命周期映射到各提供商所能安全承载的最丰富协议。
- **有状态 Agent 连续性：** 保留 Thinking、工具调用与结果、并行批次、结构化输出、缓存与用量信号，以及精确的会话分支。
- **显式多模型执行：** 让当前编码模型编排有边界的 MCP 工具，由专用模型或服务完成具体执行。

普通对话由当前选中的下游模型生成答案。使用 MCP 扩展时，则由该模型显式请求工具，再由声明好的执行器产出结果或文件。桥接器不会按提示词关键词偷偷路由，不会伪装当前模型并不具备的能力，也不会改写模型的最终回答。

## 当前真实能力

下面的状态刻意区分真实链路证据与仅在源码中实现的支持。

| 状态 | 含义 |
| --- | --- |
| **已实测** | 已通过真实 VS Code Claude Code 客户端、已安装的 Windows 服务和对应上游链路 |
| **按需启用** | 桥接器已实现，但需要显式配置、外部资源或不同计费层级 |
| **不在范围** | 当前 Claude Code 入口没有暴露，或项目有意不实现；上游模型支持并不等于本桥接器可用 |

| 能力 | 状态 | 当前行为 |
| --- | --- | --- |
| 文本、系统指令与 SSE | **已实测** | 普通与流式 Claude Code 对话均保留消息生命周期和终止错误 |
| Thinking 与推理控制 | **已实测** | 按提供商映射 effort/budget；Gemini Flash 3.7 及更高版本提供 Low/Medium/High，并保持签名思维连续性 |
| Claude Code 本地工具 | **已实测** | 函数声明、流式参数、工具结果、失败及后续推理均可完整往返 |
| 并行客户端工具 | **已实测** | Gemini 调用会保留到终态 `requires_action`；真实 Claude Code 验证过多个独立只读 `Grep` 重叠执行 |
| 结构化输出 | **已实测** | 转换 JSON Schema，仅在必要时净化；契约无法表达时明确拒绝 |
| 图片与 PDF 输入 | **已实测** | 原生媒体路径覆盖 Claude Code `Read` 工具结果；Vision Proxy 可服务已配置的纯文本目标 |
| Token 计数、用量与缓存 | **已实测** | 可用时调用原生计数端点；如实映射输入、输出、推理、缓存和服务端工具用量，不虚构命中 |
| Gemini 服务端工具 | **已实测 / 按需启用** | Google Search、URL Context、Code Execution 已实测；Maps 和已配置的 File Search 查询按需启用 |
| 图片生成 | **已实测** | `generate_image` MCP 默认委托 `gemini-3.1-flash-image`，返回预览和保存路径 |
| Remote MCP、Kimi Formula、Flex/Priority | **按需启用** | profile 与 Claude 请求级 MCP connector 均可映射为 Gemini Remote MCP，并保留校验、脱敏、allowlist 和响应块转换 |
| Claude Bash/Text Editor/Memory 声明 | **已实测** | schema-less 客户端声明会展开为 Gemini 可调用的带 schema 函数，仍由 Claude Code 执行；tool-search 声明改为立即暴露函数 |
| Claude Code 向 Gemini 输入音频/视频 | **不在范围** | Gemini 支持这些模态，但当前 Anthropic Messages 入口没有音频/视频块映射 |
| Gemini 3.8 Flash Computer Use（Windows） | **已实测** | Gemini 3.8 Flash 原生动作会转换给 Claude Code 自动托管的 `gemini-computer` stdio MCP；Browser 和 Desktop 均已完成真实链路验证 |
| Files/Batch/store 管理、Live/后台 API | **不在范围** | 保留清晰边界，不隐式启动额外服务或模型 |

“能调用”不等于“深度适配”。通用兼容提供商只会收到其真实 API 和显式 profile 能承载的语义。

## 架构

```text
                              Claude Code
              Anthropic Messages · MCP · 本地工具 · 媒体
                                  │
                                  ▼
                   Claude Code 多模型智能体运行时
          语义解码 · 能力映射 · 状态 · SSE · 诊断 · 鉴权
                         用量 · 热切换模型
                     ┌────────────┴────────────┐
                     │                         │
                     ▼                         ▼
                    模型传输                  工具执行
       Anthropic Messages                    ├─ Claude Code 本地工具
       Gemini Interactions                   ├─ Google 服务端工具
       OpenAI Responses                      ├─ Gemini Remote MCP
       OpenAI Chat 回退                      ├─ 桥接器 MCP 扩展
                                             └─ Gemini Computer Host
                     │                               │
                     ▼                               ▼
       Gemini · DeepSeek · Qwen · Kimi          专用执行器
          OpenRouter Claude · 其他       Gemini Image · Kimi Formula · Windows Host
```

桥接器选择已配置且语义损失最小的传输，而不是把所有模型压到统一的最低公分母：

| `protocol` | 推荐用途 | 语义取舍 |
| --- | --- | --- |
| `anthropic` | 提供商暴露 Anthropic Messages 端点 | 转换最少；通常优先用于 DeepSeek、Qwen、Kimi 和 OpenRouter Claude |
| `gemini-interactions` | Google Gemini 原生 Interactions API | 保留 Google 状态、step、Thinking signature、服务端工具和原生续接 |
| `openai-responses` | 提供商正式暴露 Responses | 保留 Responses item/event、服务端工具和经校验的有状态续接 |
| `openai` | OpenAI Chat Completions 兼容服务 | 兼容面最广；更多语义可能需要按提供商重建或明确降级 |

## 五分钟完成 Windows 配置

### 1. 安装

从 [GitHub Releases](https://github.com/duojiwu58-boop/claude-code-multi-model-agent-runtime/releases/latest) 下载：

- `ClaudeCodeBridge-<version>-Setup.exe`：推荐；安装服务、模型中心、开始菜单入口、`gemini-image` HTTP MCP、`gemini-computer` stdio MCP 注册和卸载器。
- `ClaudeCodeBridge-<version>-windows-x64.zip`：完整解压后运行 `Install.cmd`。

安装器会创建自动启动的 Windows 服务 `ClaudeCodeBridge`，并将 Claude Code 指向：

```text
http://127.0.0.1:18787
```

安装或升级后，请重启正在运行的 Claude Code 会话，使其获取新的环境和本地鉴权设置。

### 2. 添加或选择模型

提供商 profile 位于：

```text
%USERPROFILE%\.claude\bridge-providers\
```

通用 OpenAI 兼容 profile 可以从下面的最小配置开始：

```json
{
  "model": "提供商实际模型 ID",
  "base_url": "提供商 SDK 示例中的 base_url",
  "api_key": "你的 API Key"
}
```

对于深度适配模型，请从已验证模板开始：

- [Gemini 原生 Interactions](examples/providers/gemini.example.json)
- [DeepSeek V4 Flash](examples/providers/deepseek.example.json)
- [DeepSeek V4 Pro](examples/providers/deepseek-v4-pro.example.json)
- [Qwen3.8 Max](examples/providers/qwen.example.json)
- [Kimi K3](examples/providers/kimi.example.json)
- [通用 OpenAI 兼容提供商](examples/providers/custom-openai.example.json)
- [能力覆盖配置](examples/providers/capability-overrides.example.json)

打开 **Claude Code 模型中心**，选择 **重新加载配置**，再选择模型。下一次请求立即使用新模型，无需重启 VS Code、Claude Code 或服务。当前 Gemini 3.7 及更高版本 Flash Interactions profile 还提供可热切换的 **Low / Medium / High** Thinking 控件。

### 3. 验证

```powershell
Invoke-RestMethod -Uri 'http://127.0.0.1:18787/health'
```

响应会显示当前 profile、真实模型、传输协议和上游 URL，但不会暴露提供商 API Key。

## 深度适配模型

| 模型 | 推荐路径 | 已适配能力 |
| --- | --- | --- |
| **Gemini 3.8 Flash** | `gemini-interactions` | 当前 step 型 SSE、Low/Medium/High Thinking、终态批量并行客户端调用、精确 stored continuation、1M 上下文、图片/PDF、结构化输出、原生 Token 计数、详细用量/缓存/服务层、Google 工具、可选原生 Remote MCP |
| **DeepSeek V4 Flash / Pro** | 可用时使用 `anthropic` | 最小化 Claude 契约转换、按提供商处理 disabled/high/max 推理、工具轮推理回放、输出余量保护；保留 Responses/Chat 回退 |
| **Qwen3.8 Max** | `anthropic` 或经验证的 `openai-responses` | 有意义的 effort 档位、有界普通推理、精确 Responses 续接、DashScope 会话缓存、结构化输出、用量与延迟诊断 |
| **Kimi K3** | `anthropic` | Bearer 鉴权、已验证模型 ID、1M 上下文元数据、原生 Token 估算和缓存用量；Chat 推理回放及按需 Kimi Formula 工具 |
| **通过 OpenRouter 使用 Claude Sonnet 5 / Opus 5** | `anthropic` | Messages/SSE 透传、自适应签名 Thinking、严格/并行工具、结构化输出、Prompt Caching、图片/PDF、Web 工具、OpenRouter 鉴权和限额元数据 |
| **其他兼容模型** | 取决于提供商 | Anthropic 透传或通用 Responses/Chat 语义核心；实际深度由 `capabilities` 和实测行为共同声明 |

模型 ID、区域端点、配额、价格和提供商行为都可能变化。修改已验证模板前，请查看[提供商配置指南](PROVIDER_CONFIG.md)和[更新日志](CHANGELOG.md)。

## Gemini 3.8 Flash：本地开发基线

`standard` 服务层上的原生 `gemini-interactions` 路径足以完成常规仓库开发：读取、搜索、编辑、构建、测试、调试和审查。已验证链路包括文本/SSE、系统指令、签名 Thinking、本地工具及精确工具结果续接、结构化 JSON、图片/PDF、原生 Token 计数、有状态会话、隐式缓存报告和服务端工具用量。

Claude 最新版本化服务端工具会按请求自动映射：`web_search_*` → `google_search`、
`web_fetch_*` → `url_context`、`code_execution_*` → `code_execution`，并与 profile 工具去重。
Anthropic 独有的域名、缓存和调用次数控制若无 Gemini 等价字段，会产生明确诊断。profile 服务层设为
`auto` 时，Claude `speed: "fast"` 映射为 Gemini Priority，实际 tier/speed 写回 usage；Flex 的非流式总等待
和流式空闲等待均扩展到 20 分钟。

Gemini 3.8 Flash 支持 1,048,576 Token 输入窗口和最多 65,536 输出 Token。Thinking 只接受 `low`、`medium`、`high`，Google 默认使用 `medium`。要暴露完整输出上限，请同时把 profile 的 `max_output_tokens` 和 Claude Code 的 `CLAUDE_CODE_MAX_OUTPUT_TOKENS` 设为 `65536`。隐式缓存命中由 Google 控制，不能保证发生；只有上游报告命中时，桥接器才会记录。

### 并行工具调用

Gemini Interactions 可以在一轮内产生多个客户端函数调用。桥接器从第一个客户端调用开始保留已转换的 `tool_use` 事件，直到终态 `requires_action`，随后在同一条 assistant message 下，以一个 `message_stop` 发出完整有序批次。这样 Claude Code 不会逐个发现本应属于同一轮的调用。

这个批次让并发成为可能，但哪些调用可以重叠仍由 Claude Code 决定。独立只读的 `Read`、`Grep`、`Glob` 可能并行，而有副作用风险的 `Bash` 可能继续串行。真实验收要求所有预期的 `tool_dispatch_start` 都发生在第一个对应 `tool_dispatch_end` 之前；仅仅看到相同的 assistant message ID 不能证明调用发生了重叠。

### 图片生成是显式的多模型执行

Gemini 3.8 Flash 在这条路由上不会原生输出图片。当前安装链路是：

```text
Gemini 3.8 Flash（推理并选择工具）
        │
        ▼
Claude Code 通过带鉴权的回环 MCP 调用 generate_image
        │
        ▼
桥接器调用 gemini-3.1-flash-image（生成图片）
        │
        ▼
MCP 返回预览 + MIME 类型 + 实际执行模型 + 保存路径
```

该工具支持文档规定的宽高比和 1K/2K/4K 输出，使用 High Thinking，并且只保存到当前用户 Windows 已知“图片”文件夹下的 `ClaudeCodeBridge` 子目录。模型不能选择任意输出目录。`GEMINI_BRIDGE_IMAGE_MODEL` 可以替换固定执行模型；目前没有自动图片模型选择或回退。

### Gemini 可选扩展

普通本地开发**不需要** Google Maps、File Search store、Remote MCP、Flex 或 Priority。只有任务确实需要托管 RAG、外部系统、地理上下文或不同服务层时才启用。Google Search、URL Context、Code Execution、Maps、已配置的 File Search 查询和 Remote MCP 都可能增加数据流、资源要求或费用。

Claude 请求级 `mcp_servers` 及其匹配的 `mcp_toolset` 会转换为 Gemini Remote MCP：Bearer token 转为脱敏 Authorization header，带连字符的 server 名会规范化为 Gemini 接受的下划线，allowlist 会保留；Gemini 无法执行的 denylist 会明确报错。Gemini 返回的 MCP 调用与结果在普通和流式响应中都会还原成 Claude `mcp_tool_use`/`mcp_tool_result` 块。此路径仅支持 Streamable HTTP，不支持 Anthropic 的 SSE MCP transport。

当前本地开发范围有意排除旧版 `generateContent` 传输、显式 `cachedContents`、File Search store 创建/本地目录同步、独立 Files/Batch 管理、Live API 会话和后台 Interaction 管理。Windows x64 已提供 Gemini Computer Use：安装器把独立用户态 Host 注册为 Claude Code 的本地 stdio MCP Server，由 Claude Code 自动拉起、调度和停止；Windows 服务只做 API 与工具协议对齐，模型中心不在执行链路中。Browser 支持隔离的 1440×900 浏览器、全部 20 个浏览器动作和仅 localhost 导航；Desktop 在 `computer_start` 时按需弹出真实用户选窗，支持 Windows Graphics Capture、SendInput 的 17 个动作、PerMonitorV2/多屏坐标、HWND+PID 范围约束以及提权/敏感窗口阻止。两种环境均提供逐动作 PNG、顺序幂等批次和 Host 一次性安全确认。详见 [Gemini Computer Use](COMPUTER_USE.md)。Gemini 模型支持音频/视频输入，但当前 Claude Code Messages 入口尚未映射。

## 跨模型扩展

### Vision Proxy

纯文本目标模型可以把图片或 PDF 委托给已配置的视觉提供商，再根据有边界的提取证据继续推理：

```json
{
  "vision": {
    "mode": "proxy"
  }
}
```

原始媒体会发送给视觉提供商，提取结果再发送给目标模型，因此一次用户请求可能产生两次模型调用、两笔费用和两条提供商数据流。Vision Proxy 适合截图、终端、GUI、网页和单页 OCR，不适合批量扫描文档 OCR 或像素级精确定位。参见 [Vision Proxy 配置](PROVIDER_CONFIG.md#通用-vision-proxy)。

### MCP 与服务端工具

- `gemini-image`：由安装器管理的本地 MCP 图片生成，提供预览和安全文件落盘。
- `gemini-computer`：由安装器管理的本地 stdio MCP Computer Use 执行器，由 Claude Code 自动拉起和停止。
- Kimi Formula：只暴露显式 allowlist 中的官方 Formula URI；默认列表为空。
- Gemini 服务端工具：`google_search`、`url_context`、`code_execution`、`google_maps` 和已配置的 File Search 查询。
- Gemini 原生 Remote MCP：经校验的 HTTPS Streamable HTTP 服务，Authorization 值会脱敏，并可限制工具 allowlist。
- Claude 请求级 MCP connector：转换成 Gemini 原生 Remote MCP，普通与流式 MCP 结果会还原为 Claude 协议块。

### 按模型设置推理策略

profile 顶层的 `"reasoning_effort": "high"` 等值可以覆盖 Claude Code 进程级或请求级 effort。可接受值为 `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`；各传输层会根据上游实际支持进行映射或截断。参见[完整能力字段说明](PROVIDER_CONFIG.md#近乎无损兼容与能力覆盖)。

## 安全、数据流与运维

Windows 发行版按本地服务设计，而不是一个无鉴权的局域网代理：

- 运行时只绑定回环地址。
- 安装器生成随机 256-bit 本地 Token，并在写入秘密字节前设置严格 ACL。
- Messages、Responses、Token 计数、MCP 和所有 `/admin/*` 路由都要求 Bearer 鉴权；`/health` 和 `/v1/models` 仅保留为本地诊断。
- 本地 Token 永远不会被复用为上游提供商凭据。
- 提供商 Key 可以存放在受保护的服务凭据文件、profile 或服务可见的 `api_key_env` 中；不要提交或分享真实 Key。
- 图片 MCP 校验本地 Origin，限制 Prompt、响应、解码体积和总时长，校验 MIME，且永不接受模型指定的输出路径。Computer Host 不监听端口、不读取桥接令牌，也不调用第二个模型。
- 有状态上游 API 可能保留会话。启用前请评估各提供商的数据保留和隐私政策。
- Vision Proxy、图片生成、服务端工具、Remote MCP 和高级服务层可能产生额外调用、费用和外部数据流。

升级时会备份 Claude 设置并快照现有服务配置。安装失败会恢复旧服务状态，或只移除本次失败过程新建的服务。GUI 会保留 UAC 提权前用户的 profile、Shell 文件夹和 SID，避免用户文件被悄悄重定向到管理员账户。

| 项目 | 默认值 |
| --- | --- |
| 本地端点 | `http://127.0.0.1:18787` |
| Windows 服务 | `ClaudeCodeBridge` |
| 生产二进制 | `C:\Program Files\ClaudeCodeBridge` |
| 状态、日志、受保护凭据 | `C:\ProgramData\ClaudeCodeBridge` |
| Claude Code 设置 | `%USERPROFILE%\.claude\settings.json` |
| 提供商 profile | `%USERPROFILE%\.claude\bridge-providers\*.json` |
| 生成图片 | Windows 已知“图片”文件夹下的 `ClaudeCodeBridge` 子目录 |
| 健康检查 | `GET /health` |
| 带鉴权状态 | `GET /admin/status` |

原生提供商使用彼此独立的 HTTP 客户端，不会自动继承 Windows 系统代理。请在对应 profile 中配置 `proxy`。流式请求使用连接超时和空闲超时，不再受十分钟整流时限影响；非流式请求仍有总时长边界。精确限制和管理调用见[提供商配置](PROVIDER_CONFIG.md)。

## 这里所说的“深度适配”

只返回文本远远不够。一个模型只有经过以下审查和测试，才会进入优先支持层：

1. 官方传输协议、鉴权、模型 ID、区域端点、上下文和输出上限。
2. Thinking/effort/budget 映射和流式推理输出。
3. 完整的思考 → 工具调用 → 工具结果 → 继续思考生命周期。
4. 并行调用、结构化参数、截断和多模态工具结果。
5. 输入/输出/推理/缓存/服务端工具用量与 Token 计数。
6. 精确有状态续接、编辑分支、缓存淘汰、重启恢复和安全回退。
7. 鉴权、限流、过载、上下文上限、拒绝、取消和异常流终止。
8. 官方契约请求/响应/流式 fixture、回归测试，以及客户端边界相关的真实客户端验收。

当前锁定的 Rust 测试套件包含 225 项桥接器测试和 3 项 Computer Host 测试，全部通过。关键路径还会经过真实 Windows 服务和 VS Code Claude Code 客户端验证；仅仅 mock 成功不视为端到端兼容。

## 从源码构建

桥接服务使用 Rust，Windows 模型中心使用 Delphi VCL。构建服务不需要 Delphi：

```powershell
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked --release --target x86_64-pc-windows-msvc
```

开发时使用 `scripts\start-bridge.ps1` 和 `scripts\stop-bridge.ps1`。生产安装或升级请使用正式发行的安装器，不要把开发目录中的二进制注册为服务。

## 文档

- [提供商配置指南](PROVIDER_CONFIG.md)
- [提供商模板](examples/providers)
- [Rust 源码结构](src/README.md)
- [更新日志](CHANGELOG.md)
- [Windows 打包说明](packaging/windows-x64/使用说明.txt)

## 许可证

本项目采用 [GNU GPL v3.0](LICENSE)。你可以依据 GPL-3.0 的源码开放条款使用、修改和再发布本项目。
