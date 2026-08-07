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

| 官网示例 | Provider JSON |
| --- | --- |
| `OpenAI(api_key=...)` | `api_key` |
| `OpenAI(base_url=...)` | `base_url` |
| `client.chat.completions.create(model=...)` | `model` |

`base_url` 按 OpenAI SDK 的“基地址”语义处理：桥接器只在其后补
`/chat/completions`。如果供应商给出的不是 SDK 基地址，或网关路径比较特殊，
请用 `endpoint` 填写完整的 Chat Completions 请求地址。

## Gemini 原生 Interactions（推荐）

Gemini 3.6 Flash 建议使用 Google 原生 Interactions API，而不是 OpenAI 兼容层：

```json
{
  "name": "Google Gemini 3.6 Flash",
  "model": "gemini-3.6-flash",
  "base_url": "https://generativelanguage.googleapis.com/v1beta",
  "api_key": "<GEMINI_API_KEY>",
  "protocol": "gemini-interactions",
  "proxy": "http://127.0.0.1:8080",
  "capabilities": {
    "default_reasoning_effort": "high",
    "include_thoughts": true,
    "sampling_parameters": false,
    "gemini_builtin_tools": ["google_search", "url_context", "code_execution", "google_maps"],
    "gemini_file_search_store_names": ["fileSearchStores/project-docs"]
  }
}
```

此 transport 固定发送 `store: true`。首轮成功后，普通多轮按完整消息前缀指纹、
工具轮次按 Google 的 opaque `call_id` 查找并发送 `previous_interaction_id`；只有
精确命中才发送本轮增量。历史被编辑、缓存淘汰或服务重启后，桥接器会自动回退为
完整 `steps`，不会把会话接到错误分支。Google 会保存交互状态；请仅在接受对应的
数据保留与隐私政策时使用此协议。

`gemini_builtin_tools` 可选值为 `google_search`、`url_context`、`code_execution` 和
`google_maps`。`gemini_file_search_store_names` 非空时还会增加一个 Google 原生
`file_search` 工具，并将这些 store 名称原样传给 Interactions API。这些工具由 Google 服务端执行；Claude Code/MCP 的本地工具仍
作为自定义函数并存。若不希望服务端自行搜索或执行代码，将此数组设为空即可。

Google 当前对“服务端内置工具 + 自定义函数”的部分模型组合仍可能返回要求
`include_server_side_tool_invocations` 的 400，但 Interactions 请求结构尚无对应可移植字段。
桥接器只在识别到该错误时重试一次，并只移除本次请求的服务端工具，保留 Claude Code
函数工具。带 `previous_interaction_id` 的请求若返回 501，也只重试一次并改用安全的完整历史恢复。
其他 4xx/5xx 不会被此机制掩盖。

Claude Code 请求在此 transport 上按下表映射：

| Anthropic 请求 | Gemini Interactions |
| --- | --- |
| `/v1/messages/count_tokens` | 调用 `models/{model}:countTokens`；响应头 `x-claude-bridge-token-count` 为 `google-native` 或 `estimated-fallback` |
| `output_config.format` | `response_format`（JSON Schema 会经过 Gemini 兼容清洗） |
| `output_config.effort: low/medium/high` | `thinking_level: low/medium/high` |
| `output_config.effort: xhigh/max` | 保守钳制为 `thinking_level: high` |
| document 的 URL/base64/text/content source | 原生 `document` content，尽量保留 MIME type |
| `service_tier: standard_only` | `service_tier: standard` |
| `service_tier: auto` | 不发送该字段，使用 Google 默认 `standard`，不会自动升级为付费 `priority` |

无法安全映射的 Anthropic 字段会写入服务日志，并以可重复的
`x-claude-bridge-warning` 响应头返回；因此可在调用端明确看到降级，而不是被静默忽略。
不支持或格式错误的 `output_config.format` 会返回 400。原生 token count 上游失败或超时会进行
有界的本地估算回退，计数来源由上述响应头明确标注。

普通响应和流式结束事件会通过
`provider_metadata.google.interaction_server_tools` 回传最多 32 个服务端工具步骤；单个参数或
结果值限制为 4096 个字符。Google Search 与 URL Context 的调用次数同时映射到 Anthropic
标准 `usage.server_tool_use`。由于 Google 的搜索/URL 结果不包含 Anthropic 引用协议要求的
加密内容或索引，桥接器不会伪造 `web_search_tool_result`、`web_fetch_tool_result` 或引用块。

当前可从 Claude Code 输入到达的原生模态为文本、图片和 PDF 文档。Google API 虽定义音频、
视频、Computer Use 和 MCP Server 等能力，但 Claude Code 当前输入没有音频/视频内容块，
而 Computer Use/MCP Server 还需要动作执行、安全确认及凭据脱敏，因此本版本不提供不完整开关。

## 完整字段

```json
{
  "name": "在 GUI 中显示的名称",
  "model": "provider-model-id",
  "base_url": "https://provider.example/v1",
  "api_key": "sk-...",
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
    "gemini_builtin_tools": [],
    "gemini_file_search_store_names": []
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
  `anthropic`；Google 原生有状态接口使用 `gemini-interactions`。安装器生成的
  本地 Gemini 深度转换路由使用保留值 `gemini`。
- `endpoint`：可选，完整请求地址；设置后不会根据 `base_url`推导。
- `identity`：可选，告诉下游模型它在此路由中的真实身份；默认使用模型 ID。
- `identity_override`：可选，默认 `true`。设为 `false` 可关闭身份提示适配。
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
  "base_url": "https://api.deepseek.com",
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

显式视觉 Provider 可以使用 `gemini`、`openai` 或 `anthropic` transport，但它
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

| 能力字段 | 默认值 | 用途 |
| --- | --- | --- |
| `stream_options` | `true` | 流式请求发送 `stream_options.include_usage`；不支持该参数的端点设为 `false` |
| `parallel_tool_calls` | `true` | 允许发送 `parallel_tool_calls` 控制；端点不认识该字段时设为 `false` |
| `reasoning_effort` | `true` | 将 Claude Thinking 预算映射为 `reasoning_effort`；端点拒绝该参数时设为 `false` |
| `default_reasoning_effort` | 未设置 | Claude 请求没有 Thinking 配置时使用的默认推理等级；可选 `minimal`、`low`、`medium` 或 `high` |
| `reasoning_fields` | `["reasoning_content", "thinking"]` | 响应中按顺序识别的推理文本字段；可以填写一个字符串或字符串数组，空数组表示不提取推理文本 |
| `thinking_tags` | `true` | 将正文开头的 `<think>...</think>` 提取为 Thinking 块，并支持标签跨流式 Chunk；若模型需要原样输出该标签则设为 `false` |
| `include_thoughts` | `false` | 为 Google OpenAI 兼容端点请求思考摘要；启用时桥接器会把推理等级写入同一个 `thinking_config`，避免与 `reasoning_effort` 同时发送导致 HTTP 400 |
| `sampling_parameters` | `true` | 转发 Claude 的 `temperature` 和 `top_p`；Gemini 3.6 Flash 已废弃这些参数，应设为 `false` |
| `tool_result_media` | `separate_user` | 保持 `role: tool` 的 `content` 为字符串，并将图片/PDF 移至后一条 `user` 消息；仅对明确支持工具消息内联媒体的端点使用 `inline` |
| `tool_schema` | `sanitize` | `sanitize` 清理常见不兼容元数据；确认端点支持完整 JSON Schema 时使用 `preserve` |
| `max_tokens_field` | `max_tokens` | 可选值为 `max_tokens`、`max_completion_tokens` 或 `omit` |
| `gemini_builtin_tools` | `[]` | 仅 `gemini-interactions` 使用；可启用 `google_search`、`url_context`、`code_execution`、`google_maps` 服务端工具 |
| `gemini_file_search_store_names` | `[]` | 仅 `gemini-interactions` 使用；非空时启用 Google 原生 File Search，并传入指定 `fileSearchStores/...` 资源名 |

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
命名。工具调用参数在标准 JSON 解析失败后只进行保守修复：未转义控制字符、闭合
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
- `deepseek.example.json`：DeepSeek
- `kimi.example.json`：Kimi / Moonshot
- `gemini.example.json`：Google Gemini 的原生有状态 Interactions 接口
- `custom-openai.example.json`：其他 OpenAI 兼容网关
- `capability-overrides.example.json`：仅供需要覆盖可选参数或字段的兼容端点使用

使用方法：复制所需模板到 `%USERPROFILE%\.claude\bridge-providers\`，去掉
文件名中的 `.example`，然后填写真实 Key 和官网当前模型 ID。

供应商可能调整模型 ID、地区域名或版本路径，应以各自官网当前的 OpenAI 兼容
示例为准：

- [阿里云百炼 OpenAI 兼容说明](https://help.aliyun.com/en/model-studio/more-tools)
- [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion)
- [Kimi API 文档](https://platform.kimi.com/docs/api/overview)
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
