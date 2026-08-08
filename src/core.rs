#[derive(Clone)]
struct InteractionCallContinuation {
    interaction_id: String,
    name: String,
}

#[derive(Default)]
struct InteractionContinuationCache {
    calls: IndexMap<String, InteractionCallContinuation>,
    transcripts: IndexMap<String, String>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenAiCapabilities {
    chat_dialect: OpenAiChatDialect,
    stream_options: bool,
    parallel_tool_calls: bool,
    reasoning_effort: bool,
    default_reasoning_effort: Option<String>,
    reasoning_fields: Vec<String>,
    thinking_tags: bool,
    include_thoughts: bool,
    sampling_parameters: bool,
    tool_result_media: ToolResultMediaMode,
    tool_schema: ToolSchemaMode,
    max_tokens_field: MaxTokensField,
    responses_stateful: bool,
    responses_session_cache: bool,
    responses_builtin_tools: Vec<String>,
    responses_apply_patch_custom: bool,
    kimi_formula_tools: Vec<String>,
    gemini_builtin_tools: Vec<String>,
    gemini_file_search_store_names: Vec<String>,
}

impl Default for OpenAiCapabilities {
    fn default() -> Self {
        Self {
            chat_dialect: OpenAiChatDialect::Generic,
            stream_options: true,
            parallel_tool_calls: true,
            reasoning_effort: true,
            default_reasoning_effort: None,
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
            responses_stateful: false,
            responses_session_cache: false,
            responses_builtin_tools: Vec::new(),
            responses_apply_patch_custom: false,
            kimi_formula_tools: Vec::new(),
            gemini_builtin_tools: Vec::new(),
            gemini_file_search_store_names: Vec::new(),
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
            default_reasoning_effort: Some("high".to_string()),
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
    fallback_api_key: Option<String>,
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
