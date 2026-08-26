# Provider 配置指南

桥接器使用自己的 Provider 配置文件，不再要求用户把上游模型伪装成
`ANTHROPIC_*` 环境变量。对于支持 OpenAI Chat Completions 的服务，通常只需
把官网示例中的 `base_url`、`api_key` 和 `model` 抄进一个 JSON 文件。

## 配置目录

默认目录：

```text
%USERPROFILE%\.claude\bridge-providers\
```

这是一个桥接器专用子目录：它与 Claude Code 配置放在一起，方便查找和备份，
但不会与 Claude Code 自己的 `settings.json` 或旧 `settings - *.json` 混在一起。
服务和 GUI 都读取这个目录。可通过服务环境变量
`CLAUDE_BRIDGE_PROVIDERS_DIR` 改为其他绝对路径。

目录中的每个 `.json` 文件代表一个可切换模型。文件名也是配置的稳定 ID，建议
使用 `deepseek.json`、`qwen-coder.json` 这类简短名称。修改文件后，在 GUI 中
点击“刷新”；不需要重启 Claude Code、VS Code 或桥接服务。

## 最小配置

```json
{
  "model": "官网给出的模型 ID",
  "base_url": "官网 OpenAI SDK 示例中的 base_url",
  "api_key": "你的 API Key"
}
```

`protocol` 默认是 `openai`。桥接器把 Claude Code 的 Anthropic Messages 请求
转换为 OpenAI Chat Completions，再把下游响应转换回来。回答内容仍由下游模型
生成，桥接器不会代答身份问题或改写模型输出。

官网 OpenAI 兼容示例与本配置的对应关系：

| 官网示例                                    | Provider JSON |
| ------------------------------------------- | ------------- |
| `OpenAI(api_key=...)`                       | `api_key`     |
| `OpenAI(base_url=...)`                      | `base_url`    |
| `client.chat.completions.create(model=...)` | `model`       |

`base_url` 按 OpenAI SDK 的“基地址”语义处理：桥接器只在其后补
`/chat/completions`。如果供应商给出的不是 SDK 基地址，或网关路径比较特殊，
请用 `endpoint` 填写完整的 Chat Completions 请求地址。

## OpenRouter Claude Sonnet 5 / Opus 5 原生 Anthropic 配置

OpenRouter 提供 Anthropic Messages 入口时，应直接使用 `protocol: anthropic`，避免先降为
Chat Completions 再重建工具、Thinking 与 SSE 生命周期：

```json
{
  "name": "OpenRouter Claude Sonnet 5",
  "model": "anthropic/claude-sonnet-5",
  "base_url": "https://openrouter.ai/api",
  "protocol": "anthropic",
  "api_key": "你的 OpenRouter API Key",
  "reasoning_effort": "high",
  "context_window": 1000000
}
```

Opus 5 使用同一配置，只需把 `name` 和 `model` 分别改为 `OpenRouter Claude Opus 5` 与
`anthropic/claude-opus-5`。

顶层 `reasoning_effort` 是可选的模型级强制值，可取 `none`、`minimal`、`low`、`medium`、
`high`、`xhigh` 或 `max`。例如上面的 `high` 会覆盖 Claude Code 进程级
`CLAUDE_CODE_EFFORT_LEVEL=max` 所产生的请求值，只影响切换到这个 profile 后的新请求；删除该字段
则继续采用 Claude Code 请求值。它不同于 `capabilities.default_reasoning_effort`：后者只在请求没有
指定 effort 时兜底。也不同于布尔值 `capabilities.reasoning_effort`：后者用于关闭上游 effort 字段；
两者不能同时配置为“顶层有值、能力关闭”。顶层值为 `none` 时会强制写入
`thinking:{"type":"disabled"}` 并移除 budget；顶层值非 `none` 时，即使客户端显式发送
`thinking:{"type":"disabled"}`，也会改为 adaptive thinking，使 profile 强制值保持最高优先级。

桥接器会按 OpenRouter 文档使用 Bearer 鉴权。`anthropic-version` 与 `anthropic-beta` 会继续发往
Anthropic 兼容上游；`anthropic-user-profile-id`、`X-OpenRouter-Metadata`、`HTTP-Referer` 与
`X-OpenRouter-Title` 只在目标 profile 的主机属于 OpenRouter 时转发，避免把归属元数据泄露给
其他 Anthropic 兼容服务。`retry-after`、Anthropic/OpenRouter 限流与追踪响应头也会返回给本地客户端。
`/v1/models` 使用活动 profile 的真实模型 ID；Sonnet 5 和 Opus 5 在
profile 未写 `context_window` 时仍会报告官方 1,000,000-token 上下文和 128,000-token 最大输出。

Sonnet 5 和 Opus 5 已移除 `thinking:{"type":"enabled","budget_tokens":...}`。桥接器仅在
OpenRouter 的这两个精确模型 ID 上将旧配置改为 `thinking:{"type":"adaptive"}`；已有
`display` 会保留，未提供时则设为 `summarized`，避免旧客户端丢失可见的 thinking 摘要。
`temperature != 1.0`、`top_p < 0.99` 和任意 `top_k` 也会被省略。Opus 5 还有一条独立约束：
显式 `thinking:{"type":"disabled"}` 且没有顶层 profile 强制值替换该设置时，`xhigh`/`max` effort
会降为 `high`；Sonnet 5 不应用这项限制。所有兼容修正都会通过
`x-claude-bridge-warning` 报告；已接受的默认值及其他模型保持原样。

实测可用的核心能力包括文本/流式输出、带签名的 adaptive thinking、严格与并行客户端工具、
工具结果续接、结构化输出、图片/PDF、显式提示缓存、Web Search/Web Fetch，以及 context
management/compaction 请求包络。当前 OpenRouter OpenAPI 没有 Anthropic
`/v1/messages/count_tokens`、Files 或 Message Batches 端点，因此本地 Token Count 会以
`x-claude-bridge-token-count: estimated` 明确标识估算来源，桥接器不会伪造 Files/Batch。
Anthropic Code Execution 服务端工具及“用户提供文档”的 citation metadata 也不应从邻近能力
推断为可用；这些仍取决于 OpenRouter 上游实现。

## DeepSeek / Qwen 推荐配置与 Responses

DeepSeek V4 Flash 与 Qwen3.8-Max 的推荐默认配置是厂商原生 Anthropic Messages transport：

```json
{
  "name": "DeepSeek V4 Flash",
  "model": "deepseek-v4-flash",
  "context_window": 1048576,
  "protocol": "anthropic",
  "base_url": "https://api.deepseek.com/anthropic",
  "api_key": "<DEEPSEEK_API_KEY>",
  "vision": { "mode": "proxy" }
}
```

Pro 模型只需把 `model` 换成 `deepseek-v4-pro[1m]`，其余字段与 Flash 一致，可直接复制
[`examples/providers/deepseek-v4-pro.example.json`](examples/providers/deepseek-v4-pro.example.json)。

```json
{
  "name": "Qwen3.8 Max",
  "model": "qwen3.8-max",
  "protocol": "anthropic",
  "base_url": "https://{WORKSPACE_ID}.cn-beijing.maas.aliyuncs.com/apps/anthropic",
  "api_key": "<DASHSCOPE_API_KEY>"
}
```

需要 Responses 服务端工具或语义 SSE 时，将 `protocol` 设为 `openai-responses`，并使用官网
Responses SDK 的 `base_url`。DeepSeek Responses 是无状态接口，桥接器不会发送
`previous_response_id`，而会完整回放经过验证的历史；Qwen 默认只在精确 transcript 或 opaque
tool call ID 命中时发送 `previous_response_id`，同时启用 `x-dashscope-session-cache: enable`。

```json
{
  "name": "DeepSeek V4 Flash (Responses)",
  "model": "deepseek-v4-flash",
  "protocol": "openai-responses",
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "<DEEPSEEK_API_KEY>",
  "capabilities": {
    "responses_builtin_tools": ["web_search"],
    "responses_apply_patch_custom": true
  }
}
```

服务端工具默认不自动开启，避免意外产生搜索/执行费用；在 `responses_builtin_tools` 中填写
供应商和当前模型明确支持的工具类型后，桥接器才会把它们加入请求。DeepSeek/Qwen Chat
Completions 仍可作为 fallback：桥接器会从官方域名推断方言，也可用
`capabilities.chat_dialect` 显式指定 `deepseek` 或 `qwen`。

DeepSeek 推荐的 Anthropic 路径与 Chat fallback 共用四种实际运行态：`none` 关闭 thinking，
`minimal/low` 映射为官方 `low`（V4 GA 起两模型均支持，官方文档与实测确认），`medium/high`
映射为官方 `high`，`xhigh/max` 映射为官方 `max`。仅使用 `thinking.budget_tokens` 时，
32,768 以下保持 `high`，达到 32,768 才进入 `max`，避免 Claude Code 常见的 16K budget
让简单轮次长期运行在最高推理档——DeepSeek Anthropic 端点会忽略 `budget_tokens`（实测
64 token budget 仍产出完整思考），桥接器的 budget→effort 翻译是唯一让该字段生效的机制。
Anthropic 路径在 `max_tokens <= budget_tokens` 时还会把 `max_tokens` 提高到
`budget_tokens + 8,192`，为可见输出保留空间。

DeepSeek V4 thinking 模式接受 `tool_choice` 的 `auto/any/none`，但拒绝命名工具偏好
（HTTP 400「Thinking mode does not support this tool_choice」，实测），桥接器只剥离
`{"type":"tool","name":...}` 形式并记录 `x-claude-bridge-warning`，其余形式正常转发。

V4 GA 相关事实（2026-08 官方文档，Pro 与 Flash 的 effort 映射完全一致）：两模型均 1M
上下文、最大输出 384K、纯文本（视觉走 proxy）；并发额度 Pro 500 / Flash 2500；
价格按 peak/off-peak 时段计费（off-peak 半价）；`metadata.user_id` 可用于 KVCache
隔离与每用户限流；Claude Code 的 Web Search 由 DeepSeek 服务端原生执行。

三个传输路径都带 429 退避重试（最多 3 次，500ms/1s/2s 递增，尊重 `Retry-After`，
上限 10s），多人共用同一账户时单个 peer 的突发不会把并发上限消耗成可见失败。
Web Search 的 `web_search_20250305` 服务端工具在 Anthropic 路径上原样透传、由
DeepSeek 服务端执行（实测：模型发出 `server_tool_use`，API 返回
`web_search_tool_result`）；Chat fallback 无法服务端执行，桥接器会跳过该工具并
发出 `x-claude-bridge-warning`，历史里的搜索结果以标题/URL 文本形式保留（正文
`encrypted_content` 仅供客户端解密）。Responses 转换同样不会把 Claude 的
`web_search_*` 服务端工具冒充为客户端 function；需要上游 Responses 原生搜索时，
应在确认供应商支持后通过 `capabilities.responses_builtin_tools` 显式启用。

若希望 Claude Code 对所有模型默认请求最高 effort，应在 Claude Code 自己的
`%USERPROFILE%\.claude\settings.json` 中设置 `env.CLAUDE_CODE_EFFORT_LEVEL` 为 `max`；
安装器使用的 `claude-settings.bridge.json` 模板已采用该值。它是客户端进程级全局设置，
因此会影响同一 Claude Code 进程切换到的所有模型，修改后需要重启正在运行的会话。
若只希望某个 Provider 使用固定档位，应在对应 Provider JSON 顶层设置
`reasoning_effort`；profile 值优先于客户端全局值，并可在 GUI“刷新”后从下一次请求生效。

Chat fallback 的历史回放遵循 DeepSeek 工具契约：不携带 `tools` 且历史中没有工具调用时，普通
assistant Thinking 不进入后续 Chat 上下文；只要当前请求携带 `tools`，全部历史
`reasoning_content` 就必须完整逐字回传，不能截断、摘要或只保留最近一轮，否则上游会返回
400。Claude Code 通常持续携带工具，因此桥接器不会冒险裁剪这类会话。每次 DeepSeek 请求都会在
服务日志记录有效 thinking 状态、effort、策略来源、回放消息数及估算 Token，便于观察长工具会话的
上下文成本。

Qwen 的 Anthropic 与 Chat 路径使用可实际区分的三档策略：`none` 关闭 thinking，
`minimal/low` 映射为 `low`，`medium/high` 映射为 `medium`，显式 `xhigh/max` 或达到
31,999 的 `thinking.budget_tokens` 进入 `xhigh`。仅提供 budget 时，8,192 以下为
`low`，31,999 以下为 `medium`，达到 31,999 即为 `xhigh`——31,999 是 Claude Code
最强思考触发（ultrathink）的预算上限，这样最强思考轮次能够真正到达 Qwen 的最高推理档，
而常规中小预算仍停留在低档以控制费用。Chat fallback 还会把 `low/medium` 的
`thinking_budget` 分别限制在 4,096/16,384，明确请求 `xhigh/max` 时保留原预算。
Anthropic 路径保留预算；若 `max_tokens <= budget_tokens`，桥接器会按官方约束将
`max_tokens` 提高到 `budget_tokens + 8,192`，为可见输出保留余量而不是挤压到 1 个 token。

Qwen Responses 当前原生支持七档 effort，因此显式的 `none/minimal/low/medium/high/xhigh/max`
会原样保留；仅有 budget 时按 `<2K / <8K / <31,999 / >=31,999` 映射为
`low/medium/high/xhigh`，没有任何控制信号时使用桥接器的 `medium` 默认值。三条 Qwen 路径都会记录
有效策略和上游响应头延迟；Chat 额外记录推理回放占用，Chat/Responses 普通响应还会记录输入、输出、
缓存读取、缓存创建和推理 Token。结构化输出的 prompt 未包含 `JSON` 关键字时，会返回明确的
`x-claude-bridge-warning`。

官方 Qwen 域名的 Anthropic 请求也会携带 `x-dashscope-session-cache: enable`，与
Responses 路径一致，可用 `capabilities.responses_session_cache: false` 关闭。该请求头在
Anthropic 端点上的实际缓存效果尚待线上验证；若上游不支持，请求头会被忽略，不影响功能。
`output_config.effort` 在 Anthropic 端点的接受情况同样依赖线上验证；如果目标端点拒绝该字段，
可在 `capabilities` 中设置 `reasoning_effort: false`，桥接器将只保留
`thinking.type/budget_tokens` 控制。

## Kimi K3 1M 推荐配置

Kimi K3 面向 Claude Code 的首选路径是 Moonshot 官方 Anthropic-compatible endpoint。全球站
配置如下；API Key 必须与创建它的平台和区域匹配，国内站 Key 应将域名保持为
`api.moonshot.cn`，不要与全球站 `.ai` Key 混用。

```json
{
  "name": "Kimi K3 1M",
  "model": "kimi-k3",
  "protocol": "anthropic",
  "base_url": "https://api.moonshot.ai/anthropic",
  "api_key": "<MOONSHOT_API_KEY>",
  "auth_scheme": "bearer",
  "context_window": 1048576,
  "identity": "Kimi K3"
}
```

`auth_scheme: bearer` 对应官方 `ANTHROPIC_AUTH_TOKEN` 鉴权。`context_window` 会出现在管理 API
和 `/v1/models` 元数据中；Claude Code 已运行进程的环境变量不能被热更新，如需让客户端按
完整 1M 窗口触发自动压缩，还需在启动 Claude Code 前设置
`CLAUDE_CODE_AUTO_COMPACT_WINDOW=1048576`。在不同上下文规格的模型之间热切换时，建议重启
Claude Code 会话后再进行超长任务。

官方 Anthropic endpoint 不可用时可回退到 `protocol: openai` 与
`https://api.moonshot.ai/v1`。桥接器会自动启用 Kimi Chat 方言：使用
`max_completion_tokens`，禁止不兼容的采样参数，回放 `reasoning_content`，映射
`low/high/max` effort、JSON Schema、稳定散列的 `prompt_cache_key`/`safety_identifier`，并把
顶层 `usage.cached_tokens` 还原为 Claude cache usage。`/v1/messages/count_tokens` 会调用 Kimi
原生 `/v1/tokenizers/estimate-token-count`，失败时才回退到本地估算。

Kimi Formula 官方工具通过本地 MCP 显式启用；空数组表示完全关闭，不会产生工具调用费用：

```json
{
  "capabilities": {
    "kimi_formula_tools": [
      "moonshot/web-search:latest",
      "moonshot/fetch:latest",
      "moonshot/code-runner:latest"
    ]
  }
}
```

启用后，桥接器从 Formula API 获取标准函数 schema，并只把配置中的工具暴露给 Claude Code；
实际调用由 Formula `fibers` endpoint 执行。工具发现和按需选择继续由 Claude Code/MCP 负责，
桥接器不会把全部可用 Formula 自动塞入每个模型请求。

## Gemini 原生 Interactions（推荐）

Gemini 3.7 Flash 建议使用已 GA 的 Google 原生 Interactions API，而不是 OpenAI 兼容层：

```json
{
  "name": "Google Gemini 3.7 Flash",
  "model": "gemini-3.7-flash",
  "context_window": 1048576,
  "base_url": "https://generativelanguage.googleapis.com/v1beta",
  "api_key": "<GEMINI_API_KEY>",
  "protocol": "gemini-interactions",
  "proxy": "http://127.0.0.1:8080",
  "capabilities": {
    "default_reasoning_effort": "medium",
    "include_thoughts": true,
    "sampling_parameters": false,
    "max_output_tokens": 65536,
    "gemini_store": true,
    "gemini_service_tier": "standard",
    "gemini_tool_choice_override": "validated",
    "gemini_builtin_tools": [],
    "gemini_file_search_store_names": [],
    "gemini_remote_mcp_servers": []
  }
}
```

对于本地项目开发，建议保持原生 `gemini-interactions`、`gemini_store: true` 和
`gemini_service_tier: "standard"`。文本/流式输出、Thinking、Claude Code 的本地
Read/Grep/Edit/构建测试工具及工具结果续接、图片/PDF、结构化输出、原生 Token Count 和
Usage 观测都不依赖 File Search、Remote MCP、Maps、Flex 或 Priority。Google Search、URL Context
和 Code Execution 可按任务需要启用；其余能力主要面向云端知识库、外部业务系统、地理信息或
特殊成本/延迟目标，普通本地仓库工作可保持空数组或不配置。

项目范围以 Gemini Interactions 和 Claude Code 本地开发为核心。传统 `generateContent` transport
及其显式 `cachedContents` 生命周期暂不支持；Interactions 自身的有状态续接和隐式缓存已经覆盖
当前工作流。桥接器也不负责创建 File Search store、把本地目录同步到云端、调用独立 Files/Batch
管理 API、管理 Live API/后台 Interaction，或执行 Computer Use 动作。已经实现的 Maps、指定 store
的 File Search 查询、Remote MCP、Flex 和 Priority 只作为默认关闭的 Interactions 可选配置保留，
不属于本地开发核心链路的默认启用项或验收前置条件。

Interactions 的隐式缓存由 Google 自动管理，不提供可强制插入或保证命中的 cache key。即使前缀
满足当前模型条件并保持完全稳定，单次请求仍可能不命中；较大的共同前缀放在请求开头并在较短时间内
重复发送，只能提高概率。上游 `usage.total_cached_tokens` 会映射为 Anthropic
`usage.cache_read_input_tokens`，原始 usage 同时保存在
`provider_metadata.google.interaction_usage`；两处为 0 表示本次上游未报告命中，不表示桥接器
丢失缓存字段。有状态续接会使用 `previous_interaction_id`，但仍按 Google 契约重新发送工具、system
instruction 和生成配置。需要确定性折扣的显式 `cachedContents` 不属于此 transport。

Gemini 3.7 Flash 的输入窗口为 1,048,576 tokens、最大输出为 65,536 tokens，默认 Thinking
档位为 `medium`。模型只接受 `low`、`medium`、`high`；桥接器会把 Claude 的
`none`/`minimal` 映射到 `low`，把 `xhigh`/`max` 映射到 `high`，并继续丢弃 3.x
不支持的 `temperature`、`top_p`、`top_k` 和 `candidate_count`。Interactions 流同时兼容
官方资源 schema 的 `event_type`/`thought_summary`/`arguments_delta`，以及 3.7 迁移示例中的
`type`/`thought`/`arguments`；对象型 thought summary 会提取其中的文本，不会泄漏 JSON 包装串。
若完整工具参数只出现在 `step.stop`，桥接器会在 `content_block_stop` 前补发一个完整
`input_json_delta`，使 Claude Code 能重建真实输入。cached、thought 和新旧 token usage 字段都会映射。

模型中心为活动的 Gemini 3.7 Flash Interactions profile 提供“低 / 中 / 高”即时选择。该值写入
`bridge-state.json`，切换后从下一次请求生效，无需重启；它的优先级高于 Claude 请求携带的
`output_config.effort`、`thinking.budget_tokens` 以及 profile 的 `default_reasoning_effort`。
若 profile 顶层设置了 `reasoning_effort`，该模型级强制值再覆盖 GUI 档位；未设置时保持现有 GUI
行为。完整优先级为：profile 顶层强制值 > Gemini GUI 值 > Claude 请求/全局值 > profile 默认值。

此 transport 默认发送 `store: true`；可用 `gemini_store: false` 关闭 Google 服务端存储，
同时禁用 `previous_interaction_id` 续接。首轮成功后，普通多轮按完整消息前缀指纹、
工具轮次按 Google 的 opaque `call_id` 查找并发送 `previous_interaction_id`；Claude Code
在同一轮结果前后插入运行时 system 消息时，会从整个尾部 user/system 区间收集 call ID 并完整发送所有本轮结果，
同时把 runtime system 内容合入 `system_instruction`，不会降级为 user input。
只有全部 call ID 精确命中同一 interaction 才发送本轮增量。桥接器会在续接轮次重新发送 system instruction、工具和生成配置，
并把 interaction ID、调用名和消息指纹写入本地原子 sidecar；不会落盘 system/user/tool 内容。
因此服务重启后仍可续接。历史被编辑、状态过期、缓存淘汰或 Google 返回 404/410/501 时，
桥接器会自动回退为完整 `steps`，不会把会话接到错误分支。Google 会保存交互状态；请仅在
接受对应的数据保留与隐私政策时使用此协议。

`gemini_builtin_tools` 的每一项可以是兼容旧配置的工具名字符串，也可以是保留 Google 原生
选项的对象；支持 `google_search`、`url_context`、`code_execution`、`google_maps` 和
`file_search`。例如可通过 `{"type":"google_search","search_types":["web_search"]}`
限制搜索类型；Maps 对象可保留 `enable_widget`、`latitude`、`longitude`，File Search 对象可保留
`metadata_filter`，这些字段不会在桥接层被展平。`gemini_file_search_store_names` 非空时还会增加或补全 Google 原生
`file_search` 工具，并将这些 store 名称原样传给 Interactions API。这些工具由 Google 服务端执行；Claude Code/MCP 的本地工具仍
作为自定义函数并存。若请求含 PDF，Google 不接受 `application/pdf` 与原生 Code Execution 同时出现，
因此桥接器只对该请求省略 `code_execution`，并通过服务日志及
`x-claude-bridge-warning` 响应头报告降级；
Google Search、URL Context、自定义函数及其他兼容工具继续保留，非 PDF 请求不受影响。
若不希望服务端自行搜索或执行代码，将此数组设为空即可。

`gemini_remote_mcp_servers` 可向 Interactions 请求加入 Google 原生 Remote MCP 工具。例如：

```json
"gemini_remote_mcp_servers": [{
  "name": "project_tools",
  "url": "https://mcp.example.com/v1",
  "headers": {"Authorization": "Bearer <REMOTE_MCP_TOKEN>"},
  "allowed_tools": ["lookup_issue"]
}]
```

桥接器只接受 HTTPS Streamable HTTP 地址；`name` 必须只含 ASCII 字母、数字和下划线，
`headers` 的名称和值必须是合法 HTTP header，`allowed_tools` 必须是由非空字符串组成的数组。管理/状态输出会把
所有 header 值替换为 `<redacted>`，但 profile 文件本身仍含凭据，应按密钥文件保护。Google 原生 Remote MCP
由 Google 连接远端 server，不会注册成本地 Claude Code MCP；SSE transport 不受支持。

Google 当前对“服务端内置工具 + 自定义函数”的部分模型组合仍可能返回要求
`include_server_side_tool_invocations` 的 400，但 Interactions 请求结构尚无对应可移植字段。
桥接器只在识别到该错误时重试一次，并只移除本次请求的服务端工具，保留 Claude Code
函数工具。带 `previous_interaction_id` 的请求若返回 404、410 或 501，也只重试一次并改用安全的完整历史恢复。
其他 4xx/5xx 不会被此机制掩盖。

`gemini_tool_choice_override` 可固定为 `auto`、`any`、`none` 或 Gemini 特有的 `validated`；
`validated` 允许模型自行决定是否调用工具，同时要求实际调用必须符合已声明 schema。
无论此字段配置为何值，若消息历史末尾出现三个连续、已完成且规范化工具名、参数、结果完全相同的
工具轮次，桥接器都会在最终生成配置中强制写入 `tool_choice: none`，追加直接作答指令并产生明确诊断。
调用参数或结果发生变化、出现新的用户任务或模型已经输出最终正文时，计数会重置。
当 Claude Code 提供 `Read`、`Grep` 或 `Glob` 时，Gemini Interactions 会在最终
`system_instruction` 位置追加准确性优先的源码导航规则：最多连续执行两次返回内容和行号的高信息量搜索，
一旦定位文件或符号就立即按完整逻辑单元读取；需要连续大范围上下文时，优先一次读取约 800–1,200 行而不是机械分页；每次结果后维护
“已证实 / 仍缺失”的证据清单，避免重复区间和重叠搜索，并在所有重要结论有源码依据前继续调查。
该规则不限制调用次数，也不会仅因读取较多而强制最终答案；无法覆盖的区域必须明确说明而不能猜测。
`gemini_service_tier` 可固定为 Gemini 3.7 Flash 支持的 `standard`、`priority` 或 `flex`，优先级高于 Claude
请求的 service tier。`max_output_tokens` 会作为 Interactions 的 profile 上限，对 Claude 请求值
取较小者；Gemini 3.7 Flash 要开放完整输出能力时设为 `65536`，并在启动 Claude Code 前同步
设置 `CLAUDE_CODE_MAX_OUTPUT_TOKENS=65536`。响应体或 `x-gemini-service-tier` 响应头给出的
实际档位会写入 `provider_metadata.google.service_tier`，因此可以观察 Google 是否发生降级。
默认不设置 `max_tool_result_chars`，工具结果不会被桥接器裁剪；只有为受限路由显式配置该字段时才会裁剪。

Gemini Computer Use 不是纯服务端工具：Google API 返回动作后，调用方必须在受控浏览器中执行、
回传截图并处理安全确认。本桥接器没有这个客户端执行器，因此不会把 Computer Use 伪装成已支持；
Google Search、URL Context、Code Execution、Maps 和 File Search 仍按上面的原生接口使用。

Claude Code 请求在此 transport 上按下表映射：

| Anthropic 请求                             | Gemini Interactions                                                                                                |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| `/v1/messages/count_tokens`                | 调用 `models/{model}:countTokens`；响应头 `x-claude-bridge-token-count` 为 `google-native` 或 `estimated-fallback` |
| `output_config.format`                     | `response_format`（JSON Schema 会经过 Gemini 兼容清洗）                                                            |
| `output_config.effort: low/medium/high`    | `thinking_level: low/medium/high`                                                                                  |
| `output_config.effort: xhigh/max`          | 保守钳制为 `thinking_level: high`                                                                                  |
| document 的 URL/base64/text/content source | 原生 `document` content，尽量保留 MIME type；工具结果中的 PDF 会移到紧随其后的合法 `user_input`               |
| 含 PDF 且 profile 启用 `code_execution`    | 仅对本次请求省略原生 Code Execution 并报告兼容性降级；其他兼容工具保留                                            |
| `service_tier: standard_only`              | `service_tier: standard`                                                                                           |
| `service_tier: auto`                       | 不发送该字段，使用 Google 默认 `standard`，不会自动升级为付费 `priority`                                           |

无法安全映射的 Anthropic 字段会写入服务日志，并以可重复的
`x-claude-bridge-warning` 响应头返回；因此可在调用端明确看到降级，而不是被静默忽略。
不支持或格式错误的 `output_config.format` 会返回 400。原生 token count 上游失败或超时会进行
有界的本地估算回退，计数来源由上述响应头明确标注。

普通响应和流式结束事件会通过
`provider_metadata.google.interaction_server_tools` 回传有界的服务端工具诊断摘要。最多保留 32 个
不同步骤；标识/状态字段会保留，`arguments`、`action`、`result`、`results`、`signature` 等值序列化后
超过 4,096 字符时改为带 `truncated: true` 的预览，避免搜索结果或执行输出无界进入客户端响应。
Google 文本 annotations 则原样保存在
`provider_metadata.google.interaction_annotations`。Google Search 与 URL Context 的调用次数同时映射到 Anthropic
标准 `usage.server_tool_use`。由于 Google 的搜索/URL 结果不包含 Anthropic 引用协议要求的
加密内容或索引，桥接器不会伪造 `web_search_tool_result`、`web_fetch_tool_result` 或引用块。

当前可从 Claude Code 输入到达的原生模态为文本、图片和 PDF 文档；Claude Code `Read` 返回的 PDF
不会放入只允许文本/图片的 `function_result.result`，而是作为后一条 `user_input` 发送。Google API 虽定义音频、
视频和 Computer Use，但 Claude Code 当前输入没有音频/视频内容块，Computer Use 又需要动作执行与安全确认，
因此这些能力仍未提供。Remote MCP 则可通过上面的显式、受校验配置使用。

## 完整字段

```json
{
  "name": "在 GUI 中显示的名称",
  "model": "provider-model-id",
  "base_url": "https://provider.example/v1",
  "api_key": "sk-...",
  "auth_scheme": "bearer",
  "context_window": 1048576,
  "reasoning_effort": "high",
  "protocol": "openai",
  "endpoint": "https://provider.example/v1/chat/completions",
  "identity": "模型对外说明的真实身份",
  "identity_override": true,
  "proxy": "http://127.0.0.1:8080",
  "enabled": true,
  "vision": {
    "mode": "native"
  },
  "capabilities": {
    "stream_options": true,
    "parallel_tool_calls": true,
    "reasoning_effort": true,
    "default_reasoning_effort": "high",
    "reasoning_fields": ["reasoning_content", "thinking"],
    "thinking_tags": true,
    "include_thoughts": false,
    "sampling_parameters": true,
    "tool_result_media": "separate_user",
    "tool_schema": "sanitize",
    "max_tokens_field": "max_tokens",
    "chat_dialect": "generic",
    "responses_stateful": false,
    "responses_session_cache": false,
    "responses_builtin_tools": [],
    "responses_apply_patch_custom": false,
    "kimi_formula_tools": [],
    "gemini_builtin_tools": [],
    "gemini_file_search_store_names": [],
    "gemini_remote_mcp_servers": []
  }
}
```

字段说明：

- `model`：必填，原样发送给上游。
- `base_url`：必填，复制供应商 OpenAI SDK 示例的基地址。
- `api_key`：与 `api_key_env` 二选一。管理 API 和 GUI 从不返回密钥。
- `api_key_env`：从服务进程环境变量读取密钥，例如 `DEEPSEEK_API_KEY`。
- `name`：可选，默认使用 `model`。
- `protocol`：可选，默认 `openai`；原生 Anthropic Messages 服务可填
  `anthropic`；OpenAI Responses 使用 `openai-responses`；Google 原生有状态接口使用
  `gemini-interactions`。旧版安装器生成的本地 Gemini 兼容路由使用保留值 `gemini`。
- `bridge_managed_credentials`：仅适用于 `gemini-interactions`。设为 `true` 时，profile
  不必重复保存 Google Key；桥接器使用服务的 `GEMINI_BRIDGE_API_KEY_PROFILE` 凭据，并在
  profile 未单独设置 `proxy` 时沿用桥接器全局 Gemini 代理。为防止凭据外泄，此模式只允许
  Google 官方 `https://generativelanguage.googleapis.com` 主机。Windows 安装器默认使用此模式。
- `endpoint`：可选，完整请求地址；设置后不会根据 `base_url`推导。
- `identity`：可选，告诉下游模型它在此路由中的真实身份；默认使用模型 ID。
- `identity_override`：可选，默认 `true`。设为 `false` 可关闭身份提示适配。
- `auth_scheme`：可选，`bearer` 或 `x-api-key`。OpenAI transport 默认 `bearer`，普通 Anthropic
  transport 默认 `x-api-key`；OpenRouter Anthropic profile 会自动使用官方 Bearer 形式，Kimi
  官方 Anthropic endpoint 应显式使用 `bearer`。百炼工作区
  Anthropic endpoint 若对 `x-api-key` 返回 401，可显式改为 `bearer` 重试。
- `context_window`：可选正整数，记录上游上下文窗口并通过管理 API 与 `/v1/models` 暴露。
- `reasoning_effort`：可选的 profile 级强制推理档位，可取 `none`、`minimal`、`low`、`medium`、
  `high`、`xhigh` 或 `max`。它覆盖 Claude 请求和 Gemini GUI 的档位，并复用各 transport 的既有
  映射；`none` 会关闭 thinking 并移除 budget，非 `none` 值会把客户端的 disabled thinking 改为
  adaptive thinking；省略时严格保持原行为。该字段只控制发给上游的推理强度，不能覆盖 Claude Code
  客户端的上下文窗口、自动压缩阈值或代理编排。管理 API 会在 profile 顶层返回有效值。
- `proxy`：可选，仅用于这个 Provider；省略表示直连。
- `enabled`：可选，默认 `true`。设为 `false` 后保留文件但不显示该配置。
- `vision`：可选，默认 `{"mode":"native"}`，即图片仍由当前 Provider 原生处理。
  对无视觉能力的模型可设为 `{"mode":"proxy"}`；此时默认由本地 Gemini
  profile 提取视觉信息。也可用 `profile` 指定另一个视觉 Provider 配置文件。
- `capabilities`：可选。仅在供应商的 OpenAI 兼容实现与默认行为不同时填写；
  省略时使用下文列出的兼容默认值。

同时兼容 JavaScript 风格的 `baseURL`、`apiKey` 和 `apiKeyEnv` 字段名。

## 通用 Vision Proxy

对于 DeepSeek V4 Flash 等纯文本模型，可在目标 Provider 中开启视觉代理：

```json
{
  "name": "DeepSeek V4 Flash",
  "model": "deepseek-v4-flash",
  "protocol": "anthropic",
  "base_url": "https://api.deepseek.com/anthropic",
  "api_key": "<DEEPSEEK_API_KEY>",
  "vision": {
    "mode": "proxy"
  }
}
```

省略 `vision.profile` 时，桥接器优先选择第一个使用 `protocol: "gemini"` 的本地
Gemini profile；若没有，则选择 `base_url` 指向 Google 官方
`generativelanguage.googleapis.com` 的原生 Gemini profile。也可以显式指定任意一个
原生支持视觉的 Provider 文件，例如：

```json
"vision": {
  "mode": "proxy",
  "profile": "gemini-openai.json"
}
```

显式视觉 Provider 可以使用 `gemini`、`openai`、`openai-responses` 或 `anthropic` transport，但它
自己的 `vision.mode` 必须是 `native`。桥接器拒绝自引用和多级代理链，并在刷新
profile 时检查引用是否存在。

收到图片或 PDF 后，桥接器先让视觉 Provider 生成与当前任务相关的事实性观察；
对于文字密集图片以及翻译、总结、解释文字等请求，会要求按阅读顺序完整逐字 OCR、
保留原语言和排版结构，禁止用省略号代替内容。随后桥接器
移除发往目标文本模型的原始媒体块，并把观察作为标记过的“不可信视觉证据”加入
原用户消息。普通纯文本请求不会调用视觉 Provider。流式请求也复用同一预处理器，
所以视觉分析完成前不会产生首个 SSE 事件；分析超时为 90 秒，失败会明确返回错误，
不会静默让文本模型猜图。相同视觉 Provider、用户上下文和 base64 媒体内容的成功
结果会在进程内缓存，最多 128 项，服务重启即清空；远程 URL 内容可能变化，因此
不缓存。

隐私边界：启用后，原始图片会发送给配置的视觉 Provider，视觉观察会发送给当前
目标 Provider。若这两个服务属于不同厂商，请按两边的数据政策评估后再启用。

## 近乎无损兼容与能力覆盖

桥接器的默认原则是：标准字段走通用 OpenAI 语义核心；扩展字段根据响应内容
自动识别，而不是根据模型名称写死判断。例如，只要上游实际返回
`reasoning_content` 或 `thinking`，桥接器就会生成 Claude Code 的 Thinking
生命周期；`extra_content.google.thought_signature` 和
`promptFeedback.blockReason` 也分别触发 Gemini 的签名回传与安全拦截转换。

对于接口较完整的 Provider，仍然只需要最小的三个配置字段。只有供应商拒绝某个
可选参数、使用不同字段名，或者支持更完整的 Schema 时，才增加
`capabilities`：

| 能力字段                         | 默认值                                                           | 用途                                                                                                                                         |
| -------------------------------- | ---------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `stream_options`                 | `true`                                                           | 流式请求发送 `stream_options.include_usage`；不支持该参数的端点设为 `false`                                                                  |
| `parallel_tool_calls`            | `true`                                                           | 允许发送 `parallel_tool_calls` 控制；端点不认识该字段时设为 `false`                                                                          |
| `reasoning_effort`               | `true`                                                           | 将 Claude Thinking 预算映射为 `reasoning_effort`；端点拒绝该参数时设为 `false`                                                               |
| `default_reasoning_effort`       | 未设置                                                           | Claude 请求没有 Thinking 配置时使用的默认推理等级；可选 `none`、`minimal`、`low`、`medium`、`high`、`xhigh` 或 `max`                         |
| `reasoning_effort_map`           | `{}`                                                             | 按 Provider 重映射推理等级；键和值使用上述七种等级。例如本地 Qwen 可把 `high/max` 映射为 `xhigh`、把 `none/minimal` 映射为 `low`             |
| `reasoning_replay`               | `false`                                                          | 在通用 OpenAI Chat assistant 历史中回放 `reasoning_content`；仅当目标模型的 chat template 明确支持该字段时启用                              |
| `reasoning_replay_scope`         | 未设置                                                           | 可选 `none`、`all` 或 `active_task`；`active_task` 只回放最新真实用户任务之后的思考，工具结果和最终正文不受影响；未设置时兼容旧 `reasoning_replay` 布尔值 |
| `reasoning_fields`               | `["reasoning_content", "thinking"]`                              | 响应中按顺序识别的推理文本字段；可以填写一个字符串或字符串数组，空数组表示不提取推理文本                                                     |
| `thinking_tags`                  | `true`                                                           | 将正文开头的 `<think>...</think>` 提取为 Thinking 块，并支持标签跨流式 Chunk；若模型需要原样输出该标签则设为 `false`                         |
| `include_thoughts`               | `false`                                                          | 为 Google OpenAI 兼容端点请求思考摘要；启用时桥接器会把推理等级写入同一个 `thinking_config`，避免与 `reasoning_effort` 同时发送导致 HTTP 400 |
| `sampling_parameters`            | `true`                                                           | 转发 Claude 的 `temperature` 和 `top_p`；Gemini 3.7 Flash 不支持这些参数，应设为 `false`                                                     |
| `tool_result_media`              | `separate_user`                                                  | 保持 `role: tool` 的 `content` 为字符串，并将图片/PDF 移至后一条 `user` 消息；仅对明确支持工具消息内联媒体的端点使用 `inline`                |
| `tool_schema`                    | `sanitize`                                                       | `sanitize` 清理常见不兼容元数据；确认端点支持完整 JSON Schema 时使用 `preserve`                                                              |
| `max_tokens_field`               | `max_tokens`                                                     | 可选值为 `max_tokens`、`max_completion_tokens` 或 `omit`                                                                                     |
| `max_output_tokens`              | 未设置                                                           | 可选正整数；限制单次 OpenAI Chat 或 Gemini Interactions 响应的最大 token 数；Interactions 与 Claude 请求值取较小者                           |
| `max_tool_result_chars`          | 未设置                                                           | 可选整数且不得小于 1024；未设置时桥接器不裁剪工具结果，显式设置后才会确定性保留超长工具文本的头尾并加入重读提示，图片/PDF 与 Claude Code 本地完整记录不受影响 |
| `chat_dialect`                   | 按官方域名推断，否则 `generic`                                   | Chat fallback 可选 `generic`、`deepseek`、`qwen` 或 `kimi`；控制官方专属 thinking、推理回放与结构化输出参数                                  |
| `responses_stateful`             | Qwen 官方域名为 `true`，其他为 `false`                           | 仅精确命中历史分支后发送 `previous_response_id`                                                                                              |
| `responses_session_cache`        | Qwen 官方域名为 `true`（含 Anthropic transport），其他为 `false` | 发送 `x-dashscope-session-cache: enable`；Responses 路径已验证，Anthropic 路径效果尚待线上确认，不支持时会被上游忽略                         |
| `responses_builtin_tools`        | `[]`                                                             | 为 Responses 请求显式增加供应商支持的服务端工具类型；空数组不会产生额外费用                                                                  |
| `responses_apply_patch_custom`   | DeepSeek 官方域名为 `true`，其他为 `false`                       | 将名为 `apply_patch` 的函数工具映射为 Responses custom tool，并保持 raw patch 输入/输出轮次                                                  |
| `user_id`                        | 无                                                               | DeepSeek 业务侧用户标识（`[a-zA-Z0-9_-]{1,512}`）；客户端 `metadata.user_id` 有效时优先，否则注入该默认值，用于 KVCache 隔离与每用户并发配额      |
| `kimi_formula_tools`             | `[]`                                                             | 显式启用的 Kimi 官方 Formula URI；通过本地 MCP 获取 schema 并执行，默认不启用、不产生额外费用                                                |
| `gemini_builtin_tools`           | `[]`                                                             | 仅 `gemini-interactions` 使用；接受工具名或含原生选项的对象，支持 `google_search`、`url_context`、`code_execution`、`google_maps`、`file_search` |
| `gemini_file_search_store_names` | `[]`                                                             | 仅 `gemini-interactions` 使用；非空时启用 Google 原生 File Search，并传入指定 `fileSearchStores/...` 资源名                                  |
| `gemini_remote_mcp_servers`      | `[]`                                                             | 仅 `gemini-interactions` 使用；显式配置 HTTPS Streamable HTTP MCP server、可选 headers 与 `allowed_tools`；管理输出会脱敏 header 值          |
| `gemini_store`                   | `true`                                                           | 仅 `gemini-interactions` 使用；关闭时不在 Google 端存储交互，也不使用 `previous_interaction_id`                                             |
| `gemini_service_tier`            | 未设置                                                           | 可选 `standard`、`priority`、`flex`；设置后覆盖 Claude 请求映射                                                                             |
| `gemini_tool_choice_override`    | 未设置                                                           | 可选 `auto`、`any`、`none`、`validated`；设置后覆盖 Claude 的 tool choice，`validated` 保留 Gemini 原生 schema 约束                          |

本地 vLLM 提供的 Qwen OpenAI Chat 端点应继续使用 `chat_dialect: "generic"`，避免发送仅
DashScope/百炼兼容层支持的 `enable_thinking`、`thinking_budget` 等字段。若模型只接受
`low/medium/xhigh`，可同时设置 `reasoning_effort_map`；确认其 chat template 会读取 assistant
消息的 `reasoning_content` 后，再开启 `reasoning_replay`。

例如，一个拒绝 `stream_options` 和 `reasoning_effort`、要求
`max_completion_tokens`，但支持完整 JSON Schema 的端点可以这样配置：

```json
{
  "model": "provider-model-id",
  "base_url": "https://provider.example/v1",
  "api_key": "sk-...",
  "capabilities": {
    "stream_options": false,
    "reasoning_effort": false,
    "thinking_tags": true,
    "tool_result_media": "separate_user",
    "tool_schema": "preserve",
    "max_tokens_field": "max_completion_tokens"
  }
}
```

能力覆盖会随 Provider 热刷新，并通过本地管理 API 返回给 GUI。字段类型或枚举值
错误时，刷新会明确报错，不会静默回退。桥接器还兼容对象型工具参数、旧式
`function_call`、标准 `refusal`、数组型文本内容，以及
`prompt_tokens/completion_tokens` 和 `input_tokens/output_tokens` 两套 Usage
命名。OpenAI Chat 流式和 Responses 路径的输入、输出、缓存读取、缓存创建和 reasoning Usage
计数兼容 JSON 数字与纯数字字符串；
事件索引等结构字段仍要求真正的 JSON 数字。工具调用参数在标准 JSON 解析失败后只进行保守修复：未转义控制字符、闭合
容器前的尾逗号和缺失的 `}`/`]` 可以修复，未闭合字符串不会被猜测补全。

上游错误会映射回 Claude Code 所依赖的 Anthropic 契约：`429` 为
`rate_limit_error`，`400/413` 为 `invalid_request_error`，`529` 为
`overloaded_error`；上下文超限若被代理错误包装成 5xx，也会规范化为
`400 invalid_request_error`。没有 `[DONE]` 且没有 `finish_reason` 的流式 EOF
会作为异常流结束，不会伪装为正常 `end_turn`。

“近乎无损”指桥接器不会静默丢弃已支持的语义；它不能补出上游接口本身没有的
能力。如果一个标称 OpenAI 兼容的模型不支持工具调用、流式参数或多模态输入，
应通过能力覆盖关闭不兼容请求字段，或者选择能力更完整的模型。

## 常见供应商示例

仓库的 [`examples/providers`](examples/providers) 目录包含可直接复制的模板：

- `qwen.example.json`：阿里云百炼 / DashScope
- `deepseek.example.json`：DeepSeek V4 Flash
- `deepseek-v4-pro.example.json`：DeepSeek V4 Pro（`deepseek-v4-pro[1m]`，1M 上下文）
- `kimi.example.json`：Kimi / Moonshot
- `gemini.example.json`：Google Gemini 的原生有状态 Interactions 接口
- `custom-openai.example.json`：其他 OpenAI 兼容网关
- `capability-overrides.example.json`：仅供需要覆盖可选参数或字段的兼容端点使用

使用方法：复制所需模板到 `%USERPROFILE%\.claude\bridge-providers\`，去掉
文件名中的 `.example`，然后填写真实 Key 和官网当前模型 ID。

供应商可能调整模型 ID、地区域名或版本路径，应以各自官网当前的 OpenAI 兼容
示例为准：

- [阿里云百炼 Anthropic Messages](https://help.aliyun.com/en/model-studio/anthropic-api-messages)
- [阿里云百炼 Qwen Responses](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-responses)
- [DeepSeek Claude Code / Anthropic 配置](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)
- [DeepSeek Chat Thinking](https://api-docs.deepseek.com/guides/thinking_mode)
- [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/)
- [Kimi API 文档](https://platform.kimi.ai/docs/api/overview)
- [Kimi Claude Code 配置](https://platform.kimi.ai/docs/guide/claude-code-kimi)
- [Gemini Interactions API](https://ai.google.dev/api/interactions-api-v1)
- [Gemini streaming interactions](https://ai.google.dev/gemini-api/docs/streaming)

## 密钥安全

最方便的方式是直接填写 `api_key`。请不要提交、截图或转发包含真实密钥的配置
文件。若使用 `api_key_env`，必须让 Windows 服务进程也能读取该变量；修改服务
环境后需要重启服务。普通终端临时设置的变量不会自动进入已运行的 Windows
服务。

## 旧配置兼容

桥接器仍会兼容读取旧的 `%USERPROFILE%\.claude\settings - *.json` 配置，保证
升级或逐个迁移时原有模型不会突然消失。原生配置排在前面；若原生与旧配置的
`model` 和 `base_url` 相同，只保留原生项。建议迁移完成后把旧 Provider 文件
移出 `.claude` 目录，避免长期维护两种格式。

注意：Claude Code 自己的 `%USERPROFILE%\.claude\settings.json` 仍需通过它所
支持的 `ANTHROPIC_BASE_URL=http://127.0.0.1:18787` 连接本地桥接器。这是 Claude
Code 客户端的入口设置，不是上游 Provider 配置，两者职责不同。

新版 Windows 安装器会生成独立的 256-bit 本地桥接令牌，并自动把它写入 Claude Code
入口使用的 `ANTHROPIC_AUTH_TOKEN`、HTTP MCP header、模型中心和停止脚本。该令牌只验证
本机调用者，不会作为 Gemini 或其他上游 Provider 的 API Key 转发。升级时会复用
`C:\ProgramData\ClaudeCodeBridge\local-auth-token` 中已有的有效令牌；升级后应重启正在运行的
Claude Code 会话。源码开发使用 `scripts\start-bridge.ps1` 时，脚本会复用显式令牌、安装令牌或
已有开发令牌；都不存在时，会先在空临时文件上收紧 ACL、写入随机令牌，再原子发布为
`target\local-auth-token`。随附的停止和测试脚本会解析同一令牌。只有直接启动可执行文件时，才必须
设置至少 32 字符的 `GEMINI_BRIDGE_LOCAL_TOKEN`，或让 `GEMINI_BRIDGE_LOCAL_TOKEN_FILE`
指向受保护的令牌文件。
除本机诊断用的 `/health` 和 `/v1/models` 外，Messages、Responses、Token Count、MCP 与
全部 `/admin/*` 路由都需要 `Authorization: Bearer <local-token>`。
