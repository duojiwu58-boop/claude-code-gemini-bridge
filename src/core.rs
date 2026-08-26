#[derive(Clone)]
struct InteractionCallContinuation {
    interaction_id: String,
    name: String,
}

#[derive(Clone)]
struct InteractionContinuationCache {
    calls: IndexMap<String, InteractionCallContinuation>,
    transcripts: IndexMap<String, String>,
    persistence_path: Option<PathBuf>,
}

impl Default for InteractionContinuationCache {
    fn default() -> Self {
        Self {
            calls: IndexMap::new(),
            transcripts: IndexMap::new(),
            persistence_path: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderTransport {
    LocalGemini,
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
    GeminiInteractions,
}

impl ProviderTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalGemini => "gemini",
            Self::Anthropic => "anthropic",
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::GeminiInteractions => "gemini-interactions",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolSchemaMode {
    Sanitize,
    Preserve,
}

impl ToolSchemaMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sanitize => "sanitize",
            Self::Preserve => "preserve",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolResultMediaMode {
    SeparateUser,
    Inline,
}

impl ToolResultMediaMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::SeparateUser => "separate_user",
            Self::Inline => "inline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
    Omit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OpenAiChatDialect {
    #[default]
    Generic,
    DeepSeek,
    Qwen,
    Kimi,
}

impl OpenAiChatDialect {
    fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::DeepSeek => "deepseek",
            Self::Qwen => "qwen",
            Self::Kimi => "kimi",
        }
    }
}

impl MaxTokensField {
    fn as_str(self) -> &'static str {
        match self {
            Self::MaxTokens => "max_tokens",
            Self::MaxCompletionTokens => "max_completion_tokens",
            Self::Omit => "omit",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ReasoningReplayScope {
    #[default]
    None,
    All,
    ActiveTask,
}

impl ReasoningReplayScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
            Self::ActiveTask => "active_task",
        }
    }

    fn enabled(self) -> bool {
        self != Self::None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenAiCapabilities {
    chat_dialect: OpenAiChatDialect,
    stream_options: bool,
    parallel_tool_calls: bool,
    reasoning_effort: bool,
    default_reasoning_effort: Option<String>,
    reasoning_effort_override: Option<String>,
    reasoning_effort_map: HashMap<String, String>,
    reasoning_replay_scope: ReasoningReplayScope,
    gemini_thinking_level_override: Option<String>,
    reasoning_fields: Vec<String>,
    thinking_tags: bool,
    include_thoughts: bool,
    sampling_parameters: bool,
    tool_result_media: ToolResultMediaMode,
    tool_schema: ToolSchemaMode,
    max_tokens_field: MaxTokensField,
    max_output_tokens: Option<u64>,
    max_tool_result_chars: Option<u64>,
    responses_stateful: bool,
    responses_session_cache: bool,
    responses_builtin_tools: Vec<String>,
    responses_apply_patch_custom: bool,
    kimi_formula_tools: Vec<String>,
    gemini_builtin_tools: Vec<Value>,
    gemini_file_search_store_names: Vec<String>,
    gemini_remote_mcp_servers: Vec<Value>,
    gemini_store: bool,
    gemini_service_tier: Option<String>,
    gemini_tool_choice_override: Option<String>,
    user_id: Option<String>,
}

impl Default for OpenAiCapabilities {
    fn default() -> Self {
        Self {
            chat_dialect: OpenAiChatDialect::Generic,
            stream_options: true,
            parallel_tool_calls: true,
            reasoning_effort: true,
            default_reasoning_effort: None,
            reasoning_effort_override: None,
            reasoning_effort_map: HashMap::new(),
            reasoning_replay_scope: ReasoningReplayScope::None,
            gemini_thinking_level_override: None,
            reasoning_fields: vec!["reasoning_content".to_string(), "thinking".to_string()],
            thinking_tags: true,
            include_thoughts: false,
            sampling_parameters: true,
            // OpenAI-compatible tool messages are most portable when their
            // content remains a string. Media is moved to a following user
            // message unless a provider explicitly accepts inline media.
            tool_result_media: ToolResultMediaMode::SeparateUser,
            // Retain the existing broadly compatible behavior. Providers that
            // accept the complete Anthropic schema can opt into preservation.
            tool_schema: ToolSchemaMode::Sanitize,
            max_tokens_field: MaxTokensField::MaxTokens,
            max_output_tokens: None,
            max_tool_result_chars: None,
            responses_stateful: false,
            responses_session_cache: false,
            responses_builtin_tools: Vec::new(),
            responses_apply_patch_custom: false,
            kimi_formula_tools: Vec::new(),
            gemini_builtin_tools: Vec::new(),
            gemini_file_search_store_names: Vec::new(),
            gemini_remote_mcp_servers: Vec::new(),
            gemini_store: true,
            gemini_service_tier: None,
            gemini_tool_choice_override: None,
            user_id: None,
        }
    }
}

impl OpenAiCapabilities {
    fn local_gemini() -> Self {
        Self {
            tool_result_media: ToolResultMediaMode::Inline,
            ..Self::default()
        }
    }

    fn gemini_interactions() -> Self {
        Self {
            default_reasoning_effort: Some("medium".to_string()),
            include_thoughts: true,
            sampling_parameters: false,
            tool_result_media: ToolResultMediaMode::Inline,
            ..Self::default()
        }
    }

    fn for_openai_base_url(base_url: &str) -> Self {
        let mut capabilities = Self {
            chat_dialect: inferred_openai_chat_dialect(base_url),
            ..Self::default()
        };
        if capabilities.chat_dialect == OpenAiChatDialect::Kimi {
            capabilities.default_reasoning_effort = Some("max".to_string());
            capabilities.sampling_parameters = false;
            capabilities.tool_schema = ToolSchemaMode::Preserve;
            capabilities.max_tokens_field = MaxTokensField::MaxCompletionTokens;
        }
        capabilities
    }

    fn for_responses_base_url(base_url: &str) -> Self {
        let mut capabilities = Self::for_openai_base_url(base_url);
        match capabilities.chat_dialect {
            OpenAiChatDialect::DeepSeek => {
                capabilities.responses_apply_patch_custom = true;
            }
            OpenAiChatDialect::Qwen => {
                capabilities.responses_stateful = true;
                capabilities.responses_session_cache = true;
            }
            OpenAiChatDialect::Generic | OpenAiChatDialect::Kimi => {}
        }
        capabilities
    }

    fn for_anthropic_base_url(base_url: &str) -> Self {
        let mut capabilities = Self::for_openai_base_url(base_url);
        // Official Qwen Anthropic endpoints receive the same DashScope session
        // cache header as the Responses transport so multi-turn Claude Code
        // sessions can reuse cached context.
        if capabilities.chat_dialect == OpenAiChatDialect::Qwen {
            capabilities.responses_session_cache = true;
        }
        capabilities
    }
}

fn inferred_openai_chat_dialect(base_url: &str) -> OpenAiChatDialect {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    match host.as_deref() {
        Some("api.deepseek.com") => OpenAiChatDialect::DeepSeek,
        Some("api.moonshot.ai" | "api.moonshot.cn") => OpenAiChatDialect::Kimi,
        Some(host)
            if host == "dashscope.aliyuncs.com"
                || host == "dashscope-intl.aliyuncs.com"
                || host.ends_with(".maas.aliyuncs.com") =>
        {
            OpenAiChatDialect::Qwen
        }
        _ => OpenAiChatDialect::Generic,
    }
}

fn is_supported_kimi_formula(formula: &str) -> bool {
    matches!(
        formula,
        "moonshot/convert:latest"
            | "moonshot/web-search:latest"
            | "moonshot/rethink:latest"
            | "moonshot/random-choice:latest"
            | "moonshot/mew:latest"
            | "moonshot/memory:latest"
            | "moonshot/excel:latest"
            | "moonshot/date:latest"
            | "moonshot/base64:latest"
            | "moonshot/fetch:latest"
            | "moonshot/quickjs:latest"
            | "moonshot/code-runner:latest"
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum VisionMode {
    #[default]
    Native,
    Proxy,
}

impl VisionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Proxy => "proxy",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VisionConfig {
    mode: VisionMode,
    profile: Option<String>,
}

#[derive(Clone)]
struct ProviderProfile {
    file_name: String,
    display_name: String,
    source: ProviderProfileSource,
    model: String,
    context_window: Option<u64>,
    upstream_identity: Option<String>,
    identity_override: bool,
    base_url: String,
    auth_token: Option<String>,
    api_key: Option<String>,
    proxy_url: Option<String>,
    local_gemini: bool,
    transport: ProviderTransport,
    openai_capabilities: OpenAiCapabilities,
    vision: VisionConfig,
    upstream_url: String,
    client: Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderProfileSource {
    Native,
    Legacy,
    Mixed,
}

impl ProviderProfileSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Legacy => "legacy",
            Self::Mixed => "native+legacy",
        }
    }
}

struct LoadedProviderProfiles {
    profiles: Vec<ProviderProfile>,
    source: ProviderProfileSource,
}

struct ProviderRoutingState {
    profiles: Vec<ProviderProfile>,
    active_file: String,
    source: ProviderProfileSource,
}

#[derive(Clone)]
struct GeminiTransport {
    client: Client,
    proxy_url: Option<String>,
}

#[derive(Clone)]
struct AppState {
    gemini_transport: Arc<RwLock<GeminiTransport>>,
    gemini_thinking_level: Arc<RwLock<Option<String>>>,
    fallback_api_key: Option<String>,
    local_auth_token: String,
    upstream_url: String,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    interaction_continuations: Arc<InteractionContinuationState>,
    vision_cache: Arc<VisionObservationCache>,
    routing: Arc<RwLock<ProviderRoutingState>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    settings_dir: PathBuf,
    providers_dir: PathBuf,
    bridge_state_path: PathBuf,
    image_output_dir: PathBuf,
    image_model: String,
    image_upstream_url: String,
    local_bridge_base_url: String,
    admin_state_lock: Arc<tokio::sync::Mutex<()>>,
}

const RATE_LIMIT_RETRY_ATTEMPTS: u32 = 3;
const RATE_LIMIT_RETRY_BACKOFF_MILLIS: [u64; 3] = [500, 1_000, 2_000];
const RATE_LIMIT_RETRY_AFTER_CAP_SECS: u64 = 10;
const MAX_USER_ID_CHARS: usize = 512;

/// DeepSeek enforces per-model concurrency (Pro 500, Flash 2500) with HTTP
/// 429. A shared bridge pools several peers onto one API account, so a burst
/// from one peer can exhaust the limit for everyone; retrying the 429 with
/// backoff keeps that burst from surfacing as failures. Honors Retry-After
/// when the upstream sends it.
fn rate_limit_retry_delay(
    attempt: u32,
    status_code: u16,
    retry_after_secs: Option<u64>,
) -> Option<Duration> {
    if status_code != StatusCode::TOO_MANY_REQUESTS.as_u16() || attempt >= RATE_LIMIT_RETRY_ATTEMPTS
    {
        return None;
    }
    if let Some(secs) = retry_after_secs {
        return Some(Duration::from_secs(
            secs.min(RATE_LIMIT_RETRY_AFTER_CAP_SECS),
        ));
    }
    Some(Duration::from_millis(
        RATE_LIMIT_RETRY_BACKOFF_MILLIS[attempt as usize],
    ))
}

async fn send_with_rate_limit_retry(
    build: impl Fn() -> reqwest::RequestBuilder,
    provider: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut attempt = 0;
    loop {
        let response = build().send().await?;
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let Some(delay) =
            rate_limit_retry_delay(attempt, response.status().as_u16(), retry_after_secs)
        else {
            return Ok(response);
        };
        warn!(
            provider,
            attempt = attempt + 1,
            retry_after_ms = delay.as_millis(),
            "Upstream rate limited (429); retrying with backoff"
        );
        drop(response);
        tokio::time::sleep(delay).await;
        attempt += 1;
    }
}

/// DeepSeek user_id must match [a-zA-Z0-9_-]{1,512}; it isolates KVCache and
/// per-user concurrency quotas. The API currently accepts arbitrary values,
/// but the bridge validates so client-supplied values stay portable if
/// upstream ever enforces the documented shape.
fn validated_user_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_USER_ID_CHARS
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    Some(trimmed.to_string())
}
