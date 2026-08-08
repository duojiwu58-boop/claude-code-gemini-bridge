# Claude Code Multi-Model Bridge

[English](README.md) | **简体中文**

> 让 Claude Code 调用 Gemini、DeepSeek、Qwen、Kimi 及其他 AI 大模型，并尽可能发挥每个模型的原生能力。

![Claude Code 模型中心：无需重启即可切换 Gemini、DeepSeek、Kimi、Qwen 与其他 Provider](gui/delphi11/ClaudeBridgeManager/assets/ClaudeBridgeManager-screenshot.png)

Claude Code 不只是一个聊天客户端。它依赖 Thinking 生命周期、流式事件、并行工具调用、跨轮工具状态、结构化输出、Usage、上下文超限与重试语义。普通 API 转发器即使能返回文本，也可能在真实 Agent 任务中丢失这些能力。

本项目的目标因此不是“让请求成功”，而是：

- 保留 Claude Code 的 Agent、MCP、工具调用和任务编排体验；
- 优先使用供应商最接近原生能力的 API，而不是把所有模型压到最低公分母；
- 将 Claude Code 的请求语义映射为模型真正支持的推理、工具、缓存和输出控制；
- 把下游响应重新组装为 Claude Code 能继续执行的 Anthropic 生命周期；
- 对无法等价映射的字段明确诊断，不静默丢弃，也不虚构上游没有的能力。

最终答案始终由所选下游模型生成。桥接器负责传输、语义转换和状态衔接，不按关键词代答，也不改写模型的最终输出。

## 一张图理解

```text
Claude Code
Anthropic Messages · Agent · MCP · tools · thinking · media
                              │
                              ▼
             Claude Code Multi-Model Bridge
       语义解码 · 能力映射 · 状态续接 · 流式重建 · 诊断
             ┌────────────────┼────────────────┐
             ▼                ▼                ▼
   Anthropic Messages   Gemini Interactions   OpenAI Responses
       直接转发           原生有状态协议         语义事件/服务端工具
             └────────────────┬────────────────┘
                              ▼
                    OpenAI Chat Completions
                         通用兼容回退
                              │
                              ▼
          Gemini · DeepSeek · Qwen · Kimi · 其他模型
```

## “发挥模型能力”具体指什么

| 能力面 | 桥接器的处理原则 |
| --- | --- |
| 推理与 Thinking | 映射 effort、thinking budget 和供应商推理字段；流式恢复 Anthropic Thinking 块生命周期 |
| 工具调用 | 聚合流式参数、校验并保守修复 JSON、维持并行顺序、跨“思考 → 工具 → 结果 → 继续思考”轮次回放状态 |
| 原生状态 | 只在精确命中会话分支时使用 `previous_interaction_id` 或 `previous_response_id`；不确定时安全重放完整历史 |
| 结构化输出 | 转换 JSON Schema，并按供应商约束选择保留或清洗；无法满足契约时明确报错 |
| Streaming | 增量解析真实 SSE，跨网络帧与 UTF-8 边界恢复 Claude Code 需要的事件顺序 |
| Usage 与缓存 | 映射输入、输出、推理、缓存读写和服务端工具计数；原生 Token Count 可用时优先调用 |
| 多模态 | 原生支持时传递图片和 PDF；纯文本模型可选择 Vision Proxy 获取有界视觉证据 |
| 错误语义 | 保留认证、限流、过载、拒绝、上下文超限、截断和异常断流语义，帮助 Claude Code 正确重试或压缩 |
| 能力降级 | 记录日志，并通过 `x-claude-bridge-warning` 返回可重复诊断；不把有损回退伪装成完整支持 |

这里的“近乎无损”不是字节原样转发，而是在两套不同协议之间保留所有能够安全表达的意图、事件和状态。模型或 API 本身没有的能力，桥接器不会假装存在。

## 重点模型适配

| 模型 | 推荐 transport | 已适配的关键能力 | 可选路径 |
| --- | --- | --- | --- |
| **Gemini 3.6 Flash** | `gemini-interactions` | 原生 `step.*` 流、有状态精确续接、Thinking/签名、图片与 PDF、原生 Token Count、结构化输出、Google 服务端工具 | OpenAI Chat fallback |
| **DeepSeek V4 Flash** | `anthropic` | 最少转换保留 Claude Code 协议；Anthropic 与 Chat 路径提供 disabled/high/max 三态推理并让 16K budget 保持 high；Chat 仅在请求完全不携带工具时省略普通推理，携带 `tools` 时按 API 契约完整回放全部推理 | 无状态 `openai-responses`、OpenAI Chat、Vision Proxy |
| **Qwen3.8 Max** | `anthropic` | Anthropic/Chat 提供真正可区分的 `low/medium/xhigh` 三档并限制普通轮次预算，避免常驻最大推理；Responses 支持七档原生 effort、精确续接、会话缓存及 Usage/延迟观测 | 有状态 `openai-responses`、OpenAI Chat |
| **Kimi K3** | `anthropic` | Bearer 鉴权、`kimi-k3`、1M 上下文元数据、原生 Token Estimate、缓存 Usage；Chat 路径支持 Kimi effort、推理回放和结构化输出 | OpenAI Chat、显式启用的 Kimi Formula MCP 工具 |
| **其他兼容模型** | 视供应商而定 | Anthropic 直通或通用 OpenAI Chat/Responses 语义核心；可通过 `capabilities` 声明差异 | 能力完整度取决于上游 API |

“可调用”不等于“深度适配”。重点模型会按供应商 API 契约审核，并用请求/响应 fixture 覆盖推理、工具、流式、Usage 和续接行为；其他兼容 Provider 则以它真实提供的字段和显式能力配置为准。

供应商可能调整模型 ID、区域域名和 API 行为。仓库模板保存本项目实际验证过的配置；更新前请同时查看 [Provider 配置指南](PROVIDER_CONFIG.md) 与 [CHANGELOG](CHANGELOG.md)。

### Qwen3.8 Max 推理注意事项

- 仅提供 budget 时，Anthropic 与 Chat 路径在 8,192 以下使用 `low`、31,999 以下使用 `medium`、达到 31,999 后进入 `xhigh`；Responses 保留更细的 `<2K / <8K / <31,999 / >=31,999` → `low / medium / high / xhigh` 映射。这样 Claude Code 的 31,999-token ultrathink 上限能够进入 Qwen 最高档，常规轮次仍可控制费用。
- Chat 会把实际 `low` 和 `medium` thinking budget 分别限制在 4,096 和 16,384；Anthropic 保留请求预算，并在 `max_tokens <= budget_tokens` 时把 `max_tokens` 提高到 `budget_tokens + 8,192`，为可见输出保留空间。
- 官方 DashScope/百炼 Qwen 域名在 Responses 与 Anthropic transport 上都会自动收到 `x-dashscope-session-cache: enable`。Responses 缓存已经验证；Anthropic 请求头的实际缓存效果和端点对注入 `output_config.effort` 的接受情况仍待线上验证。可分别设置 `capabilities.responses_session_cache: false` 或 `capabilities.reasoning_effort: false` 关闭。
- Anthropic profile 默认使用 `x-api-key`；若百炼工作区返回 HTTP 401，请设置 `auth_scheme: bearer`。完整示例和诊断说明见 [Qwen Provider 配置](PROVIDER_CONFIG.md#deepseek--qwen-推荐配置与-responses)。

## 四条一级传输路径

| `protocol` | 适用场景 | 取舍 |
| --- | --- | --- |
| `anthropic` | 供应商提供 Anthropic Messages endpoint | 转换最少，通常是 DeepSeek、Qwen、Kimi 面向 Claude Code 的首选 |
| `gemini-interactions` | Google Gemini 原生 Interactions API | 保留 Google 原生状态、事件和服务端工具；使用 `store: true`，需接受服务端状态保留 |
| `openai-responses` | 供应商正式提供 Responses API | 支持 Responses item/event、服务端工具和经过验证的有状态续接 |
| `openai` | OpenAI Chat Completions 兼容端点 | 覆盖面最广；桥接器按 DeepSeek、Qwen、Kimi、Gemini 或通用方言恢复扩展能力 |

推荐顺序不是固定的“某协议最好”，而是选择对当前供应商损失最少的路径：官方 Anthropic endpoint、供应商原生协议或 Responses 可用时优先；Chat Completions 作为广泛兼容的可靠回退。

## 快速开始

### 1. 安装 Windows 服务

大多数用户无需安装 Rust、Delphi 或 Inno Setup。直接从 [GitHub Releases](https://github.com/duojiwu58-boop/claude-code-gemini-bridge/releases/latest) 下载：

- `ClaudeCodeBridge-<version>-Setup.exe`：推荐；安装 Windows 服务、模型中心、开始菜单入口和卸载程序；
- `ClaudeCodeBridge-<version>-windows-x64.zip`：完整解压后运行 `Install.cmd`。

安装器默认监听：

```text
http://127.0.0.1:18787
```

Windows 服务名为 `ClaudeCodeBridge`。安装器会备份并更新 Claude Code 的 `settings.json`，让 Claude Code 始终连接这个稳定的本地地址。首次安装后请重新启动正在运行的 Claude Code 会话。

### 2. 添加 Provider

Provider 默认目录：

```text
%USERPROFILE%\.claude\bridge-providers\
```

每个 `.json` 文件代表一个可热切换的模型。对于普通 OpenAI-compatible 服务，最小配置只有三个字段：

```json
{
  "model": "供应商实际模型 ID",
  "base_url": "供应商 SDK 示例中的 base_url",
  "api_key": "你的 API Key"
}
```

重点模型请直接复制经过适配的模板，而不是从最小配置猜参数：

- [Gemini 原生 Interactions](examples/providers/gemini.example.json)
- [DeepSeek V4 Flash](examples/providers/deepseek.example.json)
- [Qwen3.8 Max](examples/providers/qwen.example.json)
- [Kimi K3](examples/providers/kimi.example.json)
- [通用 OpenAI-compatible](examples/providers/custom-openai.example.json)
- [能力覆盖示例](examples/providers/capability-overrides.example.json)

保存后打开“Claude Code 模型中心”，点击“刷新配置”并选择模型。下一个请求立即使用新路由，无需重启 VS Code、Claude Code 或桥接服务。

完整字段、区域域名、鉴权方式、代理、Responses、Vision Proxy 和旧配置迁移见 [Provider 配置指南](PROVIDER_CONFIG.md)。

### 3. 验证运行状态

```powershell
Invoke-RestMethod -Uri 'http://127.0.0.1:18787/health'
```

健康响应会显示当前 profile、实际模型、transport 和上游地址，但不会返回 Provider API Key。

## 跨模型增强

### Vision Proxy

没有原生视觉能力的文本模型可以把图片或 PDF 先交给指定视觉 Provider 分析，再让当前目标模型继续推理和调用工具：

```json
{
  "vision": {
    "mode": "proxy"
  }
}
```

原始媒体会发送给视觉 Provider，提取后的有界视觉证据会发送给目标 Provider，因此可能产生两次模型调用与两份费用，也需要同时评估双方的数据政策。Vision Proxy 适合代码截图、终端、GUI、网页和单页 OCR，不是批量扫描件 OCR 或像素级定位引擎。详细限制见 [Vision Proxy 配置](PROVIDER_CONFIG.md#通用-vision-proxy)。

### MCP 扩展

- `gemini-image`：Windows 安装器可为 Claude Code 注册 `generate_image`，使用 Gemini 图像模型生成预览并保存到系统“图片”目录下的 `ClaudeCodeBridge` 文件夹；
- Kimi Formula：只暴露 Provider 中 `kimi_formula_tools` 明确列出的官方 Formula，默认关闭；
- Google 服务端工具：`google_search`、`url_context`、`code_execution`、`google_maps` 和 File Search 均需在 Gemini profile 中显式启用。

服务端搜索、执行和生图可能产生额外费用。桥接器不会默认开启这些能力。

## 配置与运维

| 项目 | 默认位置或值 |
| --- | --- |
| 本地端点 | `http://127.0.0.1:18787` |
| Windows 服务 | `ClaudeCodeBridge` |
| 正式程序目录 | `C:\Program Files\ClaudeCodeBridge` |
| 状态与日志 | `C:\ProgramData\ClaudeCodeBridge` |
| Claude Code 配置 | `%USERPROFILE%\.claude\settings.json` |
| Provider 配置 | `%USERPROFILE%\.claude\bridge-providers\*.json` |
| 健康检查 | `GET /health` |
| 模型列表 | `GET /admin/profiles` |
| 刷新 Provider | `POST /admin/reload-profiles` |

每个原生 Provider 使用独立 HTTP 客户端，不会自动继承 Windows 系统代理或 Gemini 的旧代理配置。需要代理时，在对应 profile 中设置：

```json
{
  "proxy": "http://127.0.0.1:8080"
}
```

API Key 可以直接写入 Provider 文件，也可以通过 `api_key_env` 引用服务进程可见的环境变量。不要提交、截图或分发包含真实密钥的文件。

Kimi K3 的 `context_window: 1048576` 会进入管理 API 和 `/v1/models` 元数据；若要让 Claude Code 客户端按完整 1M 窗口自动压缩，还需在启动 Claude Code 前设置 `CLAUDE_CODE_AUTO_COMPACT_WINDOW=1048576`。不同上下文规格之间切换后进行超长任务时，建议重启 Claude Code 会话。

## 设计边界

- 桥接器只能保留上游 API 暴露的能力，不能为普通聊天端点创造可靠工具调用、视觉或推理状态；
- Chat Completions fallback 通常比官方 Anthropic、Interactions 或 Responses 路径更容易发生语义降级；
- 有状态 API 会把部分会话状态保存在供应商服务端；选择前应评估隐私和数据保留政策；
- Provider 的模型 ID、上下文、配额、价格和可用区可能变化，应以实际调用结果为准；
- 通用兼容不代表模型一定适合 Coding Agent；可靠体验仍依赖足够的代码能力、上下文、流式输出和工具支持；
- 对无法安全映射的字段，项目倾向于明确警告或失败，而不是静默返回看似成功但违反契约的结果。

## 新模型的深度适配标准

本项目以后不以“返回了一段文本”作为适配完成。一个模型进入重点支持范围，至少应审核并测试：

1. 官方推荐 transport、鉴权、模型 ID、区域域名与上下文规格；
2. Thinking/effort/budget 的请求映射及流式推理输出；
3. “思考 → 工具调用 → 工具结果 → 继续思考”的完整回放；
4. 并行工具、结构化参数、截断调用与多模态工具结果；
5. 缓存、推理 Token、服务端工具和总 Usage；
6. 有状态续接的精确命中、历史编辑、缓存淘汰和安全回退；
7. 限流、过载、上下文超限、拒绝与异常流结束；
8. 官方请求/响应 fixture、流式 fixture 与回归测试。

这套标准也是“最大化发挥模型能力”的可验证定义。

## 从源码构建

项目主体使用 Rust，Windows 模型中心使用 Delphi VCL。仅构建桥接服务时不需要 Delphi：

```powershell
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked --release --target x86_64-pc-windows-msvc
```

开发环境可使用仓库中的 `scripts\start-bridge.ps1` 与 `scripts\stop-bridge.ps1`；正式 Windows 安装和升级应使用发布包中的安装程序，不要把开发目录当作生产安装目录。

## 文档

- [Provider 配置指南](PROVIDER_CONFIG.md)
- [Provider 模板](examples/providers)
- [Rust 源码结构](src/README.md)
- [变更记录](CHANGELOG.md)
- [Windows 发布包使用说明](packaging/windows-x64/使用说明.txt)

## 许可证

本项目采用 [GNU GPL v3.0](LICENSE) 开源。您可以使用、修改和再分发，但衍生作品也必须按 GPL-3.0 提供源代码。
