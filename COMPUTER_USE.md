# Gemini Computer Use（Browser + Desktop）

这是 v0.8.0 的重大升级，目标是把 **Gemini 3.8 Flash 新提供的原生 Computer Use 能力**完整接入 Claude Code，而不是在桥接器里另造一个模型代理。Gemini 3.8 Flash 负责理解画面并决定动作；桥接器负责 Gemini Interactions 与 Claude 工具协议的对齐；Claude Code 负责工具调度；本机 Host 只负责执行。

`claude-computer-host.exe` 是由安装器注册、Claude Code 托管的本地 stdio MCP Server。Claude Code 在需要工具时自动拉起它，在会话结束或 stdin 关闭时由 Host 清理隔离浏览器、按键和鼠标状态后退出。

不需要打开模型中心，不需要点击“启用 Host”，不依赖 Codex 插件，也不会调用第二个模型。模型中心仍只负责模型路由和 Provider 配置。

## 架构

```text
Claude Code
  ├─ 模型请求 -> ClaudeCodeBridge 服务 -> Gemini
  │                                <- 原生 Computer Use 动作
  └─ 本地 stdio MCP -> claude-computer-host.exe（普通用户、PerMonitorV2）
                         Browser: isolated Edge/Chrome + DevTools
                         Desktop: user-selected HWND + WGC + SendInput
```

桥接服务负责 API 与工具协议对齐；Claude Code 负责 MCP 生命周期和工具调度；Host 只负责本机截图、输入、安全复核与按需用户交互。三者不通过旧的 Host 注册、心跳或命令轮询接口串联。

`computer_start` 接受 `browser` 或 `desktop` 并返回首张 PNG。Gemini 同一轮的动作合并成一个 `computer_action_batch`，Host 严格按原顺序执行，并在每个动作后返回截图。批次保留原始 `call_id`、参数、`intent` 与 `safety_decision`。

`computer_cancel` 是显式生命周期出口。活动 Computer Use 上下文中，如果用户要求取消，桥接器只向 Gemini 暴露这一项本地函数，避免 Gemini `computer_use` 与搜索等工具组合被上游拒绝；成功的取消结果会立即关闭本轮上下文，防止重复取消，同时允许同一 Claude Code 对话随后重新 `computer_start`。

## Claude Code 注册

Windows 安装器会在 `%USERPROFILE%\.claude.json` 中注册：

```json
{
  "mcpServers": {
    "gemini-computer": {
      "type": "stdio",
      "command": "C:\\Program Files\\ClaudeCodeBridge\\claude-computer-host.exe",
      "args": ["--stdio-mcp"]
    }
  }
}
```

安装或升级后新开一个 Claude Code 会话即可生效。Host 的 stdout 只承载逐行 JSON-RPC；诊断输出只写 stderr。

## Browser 使用

向 Claude Code 提供 `http://localhost/...`、`http://127.0.0.1/...` 或 `http://[::1]/...` 页面并要求操作。Claude Code 会自动启动 Host；无需预先运行任何 GUI。

Browser 固定观察尺寸为 1440×900，使用每会话独立 profile，并阻止初始 URL、导航和实际跳转离开 loopback allowlist。

## Desktop 使用

要求 Claude Code 操作桌面窗口并调用 `computer_start` 的 `desktop` 环境。Host 此时才显示顶置选窗对话框；真实用户逐个查看候选窗口，选择“是”绑定一个窗口，或取消并拒绝。模型无权提供或更换 `target_hwnd`。

Gemini 要求确认或本地策略判定为非低风险时，Host 才显示一次性安全确认对话框，其中包含目标、模型意图和确认原因。拒绝后本批不会执行任何动作。除此之外 Host 没有常驻 GUI。

Desktop 截图使用 Windows Graphics Capture 的 `CreateForWindow` 路径；输入使用 `SendInput`。Host 内嵌 `asInvoker`、`uiAccess=false`、`PerMonitorV2` manifest，坐标按虚拟桌面的物理像素映射，支持负坐标多显示器和 100%/125%/150% DPI。截图跟随所选窗口；同进程相关模态对话框成为前台窗口时，观察和坐标范围会切换到该对话框。

## 支持动作

Browser 支持 20 个动作：

`click`、`double_click`、`triple_click`、`middle_click`、`right_click`、`mouse_down`、`mouse_up`、`move`、`type`、`drag_and_drop`、`wait`、`press_key`、`key_down`、`key_up`、`hotkey`、`take_screenshot`、`scroll`、`go_back`、`navigate`、`go_forward`。

Desktop 支持其中 17 个动作，不支持 `go_back`、`navigate`、`go_forward`。`type` 单次最多 4000 个 UTF-16 单元并采用节流输入；长输入期间只要焦点离开所选窗口范围，剩余文本立即停止发送。

## Desktop 安全边界

- Session 固定绑定真实用户所选的顶层 HWND 与 PID；每次截图和输入前重新核对窗口身份。
- 仅允许所选 HWND、其子窗口，以及同 PID 且由目标拥有的相关对话框。
- 坐标被无关窗口遮挡时拒绝点击；键盘焦点无法可靠落到目标范围时拒绝按键。
- 提权窗口由 token elevation 检查阻止；Host 使用 `asInvoker` 且 `uiAccess=false`，不绕过 UIPI。
- 密码管理器、凭据与 Windows Security 等敏感窗口按进程路径、类名和标题阻止。
- 只允许名为 `Default` 的正常输入桌面。锁屏、用户切换、UAC/Winlogon 安全桌面、目标退出或 HWND 换主都会使后续动作失败并终止相应会话。
- Claude Code 关闭 stdio 时，Host 释放所有输入状态并结束隔离浏览器。

## 审批、幂等与恢复

- `safety_decision == blocked` 永不执行。
- Gemini 要求确认，或本地策略判定非低风险时，整个批次先暂停；确认前一个动作也不会执行。
- 安全确认只对当前进程、当前 session 和当前批次生效。
- 相同 `session_id + batch_id` 重试返回进程内缓存，不重复点击或输入；上限 50 步、15 分钟。
- 截图必须是有效 PNG，单张不超过 8 MiB，并返回宽高、SHA-256 与捕获源。
- Host 不监听端口，不读取桥接服务 token，也不连接模型 API。

## Provider 配置

```json
{
  "type": "computer_use",
  "environment": "desktop",
  "enable_prompt_injection_detection": true
}
```

`environment` 可为 `browser` 或 `desktop`。`enable_prompt_injection_detection: false`、未知环境或任何 `disabled_safety_policies` 都会拒绝加载。桥接器只会在 `computer_start` 的首张截图成功后启用本轮 Gemini 原生 Computer Use。

## Browser 本地验证页

仓库提供 `tests\fixtures\computer-use-localhost.html`：

```powershell
python -m http.server 8765 --bind 127.0.0.1 --directory .\tests\fixtures
```

然后使用 `http://127.0.0.1:8765/computer-use-localhost.html` 验证坐标、截图、审批和批次重试。

## 当前边界

- 原生动作规划依赖 Gemini 3.8 Flash Computer Use；其他模型只有在自身提供兼容动作协议且完成适配后才能使用这条链路。
- 仅 Windows x64；Desktop 需要 Windows 10 1903 或更高版本的 Windows Graphics Capture。
- 不处理验证码，不绕过登录、UAC、UIPI、锁屏或网站安全策略。
- 多用户/RDP 必须由对应用户的 Claude Code 启动自己的 stdio Host；服务不会跨 Session 注入。
- 二进制尚未签名，生产分发前应增加代码签名，并始终保持 Host 为普通用户完整性级别。
