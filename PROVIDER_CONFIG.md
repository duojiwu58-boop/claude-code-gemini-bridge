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
  "enabled": true
}
```

字段说明：

- `model`：必填，原样发送给上游。
- `base_url`：必填，复制供应商 OpenAI SDK 示例的基地址。
- `api_key`：与 `api_key_env` 二选一。管理 API 和 GUI 从不返回密钥。
- `api_key_env`：从服务进程环境变量读取密钥，例如 `DEEPSEEK_API_KEY`。
- `name`：可选，默认使用 `model`。
- `protocol`：可选，默认 `openai`；原生 Anthropic Messages 服务可填
  `anthropic`。安装器生成的本地 Gemini 深度转换路由使用保留值 `gemini`。
- `endpoint`：可选，完整请求地址；设置后不会根据 `base_url`推导。
- `identity`：可选，告诉下游模型它在此路由中的真实身份；默认使用模型 ID。
- `identity_override`：可选，默认 `true`。设为 `false` 可关闭身份提示适配。
- `proxy`：可选，仅用于这个 Provider；省略表示直连。
- `enabled`：可选，默认 `true`。设为 `false` 后保留文件但不显示该配置。

同时兼容 JavaScript 风格的 `baseURL`、`apiKey` 和 `apiKeyEnv` 字段名。

## 常见供应商示例

仓库的 [`examples/providers`](examples/providers) 目录包含可直接复制的模板：

- `qwen.example.json`：阿里云百炼 / DashScope
- `deepseek.example.json`：DeepSeek
- `kimi.example.json`：Kimi / Moonshot
- `gemini.example.json`：Google Gemini 的 OpenAI 兼容接口
- `custom-openai.example.json`：其他 OpenAI 兼容网关

使用方法：复制所需模板到 `%USERPROFILE%\.claude\bridge-providers\`，去掉
文件名中的 `.example`，然后填写真实 Key 和官网当前模型 ID。

供应商可能调整模型 ID、地区域名或版本路径，应以各自官网当前的 OpenAI 兼容
示例为准：

- [阿里云百炼 OpenAI 兼容说明](https://help.aliyun.com/en/model-studio/more-tools)
- [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion)
- [Kimi API 文档](https://platform.kimi.com/docs/api/overview)
- [Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)

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
