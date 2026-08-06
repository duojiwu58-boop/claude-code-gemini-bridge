//! Maintenance scope: this bridge is maintained exclusively for Claude Code.
//! Codex uses its native GPT provider as the primary and permanent path, so it
//! is not a target for new bridge features. The legacy OpenAI Responses route
//! remains only for backward compatibility; future protocol, routing, GUI, and
//! reliability work should prioritize the Anthropic Messages API used by
//! Claude Code.

mod windows_service;

use std::{
    collections::VecDeque,
    convert::Infallible,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use indexmap::IndexMap;
use reqwest::{Client, Proxy};
use serde_json::{json, Map, Value};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use uuid::Uuid;

const THOUGHT_SIGNATURE_CAPACITY: usize = 4096;
const THOUGHT_SIGNATURE_EVICTION_BATCH: usize = 512;
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const BRIDGE_IDENTITY_MARKER: &str = "<bridge_runtime_identity>";
const MAX_UPSTREAM_IDENTITY_CHARS: usize = 200;
// Identity phrases Claude Code injects into system prompts. These are matched
// as phrases/patterns rather than whole declarations so that subagent persona
// variants ("You are a file search specialist for Claude Code, ...", "You are
// an agent for Claude Code, ...") and future rewordings are still neutralized.
const CLAUDE_OFFICIAL_CLI_PHRASE: &str = "Claude Code, Anthropic's official CLI for Claude";
const CLAUDE_CLI_SDK_SUFFIX: &str = ", running within the Claude Agent SDK";
const CLAUDE_AGENT_SDK_DECLARATION: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const CLAUDE_COORDINATOR_DECLARATION: &str =
    "You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers.";
const CLAUDE_POWERED_BY_PREFIX: &str = "You are powered by the model";
const CLAUDE_EXACT_MODEL_ID_PREFIX: &str = " The exact model ID is";
const CLAUDE_CO_AUTHOR_LINE: &str = "Co-Authored-By: Claude <noreply@anthropic.com>";

type ThoughtSignatureCache = RwLock<IndexMap<String, String>>;

#[derive(Clone)]
struct ProviderProfile {
    file_name: String,
    model: String,
    upstream_identity: Option<String>,
    identity_override: bool,
    base_url: String,
    auth_token: Option<String>,
    api_key: Option<String>,
    proxy_url: Option<String>,
    local_gemini: bool,
    client: Client,
}

struct ProviderRoutingState {
    profiles: Vec<ProviderProfile>,
    active_file: String,
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
    routing: Arc<RwLock<ProviderRoutingState>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    settings_dir: PathBuf,
    bridge_state_path: PathBuf,
    local_bridge_base_url: String,
}

fn main() {
    let service_mode = env::args_os().any(|arg| arg == "--windows-service");
    let result = if service_mode {
        windows_service::run_dispatcher()
    } else {
        run_console()
    };

    if let Err(err) = result {
        eprintln!("Bridge startup failed: {err}");
        std::process::exit(1);
    }
}

fn run_console() -> Result<(), String> {
    let _log_guard = init_logging(false)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Cannot create Tokio runtime: {err}"))?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    runtime.block_on(run_bridge(shutdown_tx, shutdown_rx, true, || Ok(())))
}

pub(crate) fn init_logging(service_mode: bool) -> Result<Option<WorkerGuard>, String> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "claude_bridge=info,tower_http=info".into());

    if service_mode {
        let log_dir = env::var("GEMINI_BRIDGE_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("logs")
            });
        fs::create_dir_all(&log_dir).map_err(|err| {
            format!(
                "Cannot create service log directory '{}': {err}",
                log_dir.display()
            )
        })?;
        let file_appender = tracing_appender::rolling::daily(&log_dir, "claude-code-bridge.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(non_blocking)
            .try_init()
            .map_err(|err| format!("Cannot initialize service logging: {err}"))?;
        Ok(Some(guard))
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .try_init()
            .map_err(|err| format!("Cannot initialize console logging: {err}"))?;
        Ok(None)
    }
}

pub(crate) async fn run_bridge<F>(
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    include_ctrl_c: bool,
    on_started: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let fallback_api_key = resolve_fallback_api_key()?;
    let listen = env::var("GEMINI_BRIDGE_LISTEN").unwrap_or_else(|_| "127.0.0.1:18787".to_string());
    let upstream_url = env::var("GEMINI_BRIDGE_UPSTREAM").unwrap_or_else(|_| {
        "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions".to_string()
    });
    let model = env::var("GEMINI_BRIDGE_MODEL").unwrap_or_else(|_| "gemini-3.6-flash".to_string());
    let settings_dir = env::var("CLAUDE_SETTINGS_DIR")
        .map(PathBuf::from)
        .or_else(|_| env::var("USERPROFILE").map(|profile| PathBuf::from(profile).join(".claude")))
        .map_err(|_| "CLAUDE_SETTINGS_DIR or USERPROFILE is required".to_string())?;
    let bridge_state_path = env::var("GEMINI_BRIDGE_STATE_FILE")
        .map(PathBuf::from)
        .or_else(|_| {
            env::current_dir()
                .map(|path| path.join("bridge-state.json"))
                .map_err(|_| env::VarError::NotPresent)
        })
        .map_err(|_| "Cannot resolve bridge state file path".to_string())?;
    let local_bridge_base_url = format!("http://{listen}");
    let profiles = load_provider_profiles(&settings_dir, &local_bridge_base_url)
        .map_err(|err| format!("Cannot load Claude provider profiles: {err}"))?;
    let active_profile = select_initial_profile(&profiles, &bridge_state_path);
    let proxy_url = load_persisted_gemini_proxy(&bridge_state_path).unwrap_or_else(|| {
        env::var("GEMINI_BRIDGE_PROXY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let gemini_client = build_gemini_client(proxy_url.as_deref(), None)?;

    let state = Arc::new(AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client: gemini_client,
            proxy_url,
        })),
        fallback_api_key,
        upstream_url,
        model,
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles,
            active_file: active_profile,
        })),
        shutdown_tx,
        settings_dir,
        bridge_state_path,
        local_bridge_base_url,
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        .route("/admin/status", get(admin_status))
        .route("/admin/profiles", get(admin_profiles))
        .route("/admin/active-profile", post(admin_set_active_profile))
        .route("/admin/reload-profiles", post(admin_reload_profiles))
        .route("/admin/gemini-proxy", post(admin_set_gemini_proxy))
        .route("/admin/gemini-proxy/test", post(admin_test_gemini_proxy))
        .route("/admin/shutdown", post(admin_shutdown))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let address: SocketAddr = listen
        .parse()
        .map_err(|err| format!("Invalid GEMINI_BRIDGE_LISTEN '{listen}': {err}"))?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|err| format!("Cannot listen on {address}: {err}"))?;

    if let Err(err) = record_listen_in_state(&state.bridge_state_path, &listen) {
        error!("{err}");
    }

    on_started()?;

    info!("Claude Code bridge listening on http://{address}");
    info!(
        "Upstream model: {}",
        env::var("GEMINI_BRIDGE_MODEL").unwrap_or_else(|_| "gemini-3.6-flash".into())
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx, include_ctrl_c))
        .await
        .map_err(|err| format!("Server failed: {err}"))
}

fn resolve_fallback_api_key() -> Result<Option<String>, String> {
    if let Some(api_key) = env::var("GEMINI_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(api_key));
    }

    let Some(profile_path) = env::var("GEMINI_BRIDGE_API_KEY_PROFILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&profile_path)
        .map_err(|err| format!("Cannot read API key profile '{profile_path}': {err}"))?;
    for line in contents.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != "experimental_bearer_token" {
            continue;
        }
        let token: String = serde_json::from_str(value.trim()).map_err(|err| {
            format!(
                "Invalid experimental_bearer_token in API key profile '{}': {err}",
                profile_path
            )
        })?;
        if token.trim().is_empty() {
            return Err(format!(
                "API key profile '{}' contains an empty token",
                profile_path
            ));
        }
        return Ok(Some(token));
    }
    Err(format!(
        "API key profile '{}' does not contain experimental_bearer_token",
        profile_path
    ))
}

async fn shutdown_signal(
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    include_ctrl_c: bool,
) {
    if include_ctrl_c {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            changed = shutdown_rx.changed() => {
                let _ = changed;
            }
        }
    } else {
        let _ = shutdown_rx.changed().await;
    }
}

fn is_provider_profile_file_name(file_name: &str) -> bool {
    !file_name.eq_ignore_ascii_case("settings.json")
        && file_name.starts_with("settings")
        && file_name.ends_with(".json")
}

fn load_provider_profiles(
    settings_dir: &Path,
    local_bridge_base_url: &str,
) -> Result<Vec<ProviderProfile>, String> {
    let entries = fs::read_dir(settings_dir)
        .map_err(|err| format!("Cannot read '{}': {err}", settings_dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_provider_profile_file_name(file_name) {
            continue;
        }
        paths.push(path);
    }
    paths.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });

    let mut profiles = Vec::new();
    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("Invalid profile file name '{}'", path.display()))?
            .to_string();
        let text = fs::read_to_string(&path)
            .map_err(|err| format!("Cannot read '{}': {err}", path.display()))?;
        let json_text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let settings: Value = serde_json::from_str(json_text)
            .map_err(|err| format!("Invalid JSON in '{}': {err}", path.display()))?;
        let env = settings
            .get("env")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("Profile '{file_name}' has no env object"))?;
        let get_env = |name: &str| {
            env.get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let base_url = get_env("ANTHROPIC_BASE_URL")
            .ok_or_else(|| format!("Profile '{file_name}' has no ANTHROPIC_BASE_URL"))?;
        let model = get_env("ANTHROPIC_MODEL")
            .or_else(|| get_env("ANTHROPIC_DEFAULT_SONNET_MODEL"))
            .ok_or_else(|| format!("Profile '{file_name}' has no model"))?;
        let upstream_identity = get_env("CLAUDE_BRIDGE_UPSTREAM_IDENTITY");
        let identity_override = get_env("CLAUDE_BRIDGE_IDENTITY_OVERRIDE")
            .map(|value| identity_override_enabled(&value))
            .unwrap_or(true);
        let auth_token = get_env("ANTHROPIC_AUTH_TOKEN");
        let api_key = get_env("ANTHROPIC_API_KEY");
        if auth_token.is_none() && api_key.is_none() {
            return Err(format!("Profile '{file_name}' has no API credential"));
        }
        let proxy_url = get_env("HTTPS_PROXY")
            .or_else(|| get_env("HTTP_PROXY"))
            .or_else(|| get_env("ALL_PROXY"));
        let mut client_builder = Client::builder();
        if let Some(proxy_url) = &proxy_url {
            client_builder = client_builder.proxy(
                Proxy::all(proxy_url)
                    .map_err(|err| format!("Invalid proxy in '{file_name}': {err}"))?,
            );
        } else {
            client_builder = client_builder.no_proxy();
        }
        let client = client_builder
            .build()
            .map_err(|err| format!("Cannot create HTTP client for '{file_name}': {err}"))?;
        let local_gemini =
            normalize_base_url(&base_url) == normalize_base_url(local_bridge_base_url);

        profiles.push(ProviderProfile {
            file_name,
            model,
            upstream_identity,
            identity_override,
            base_url,
            auth_token,
            api_key,
            proxy_url,
            local_gemini,
            client,
        });
    }

    Ok(profiles)
}

fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn identity_override_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn build_gemini_client(
    proxy_url: Option<&str>,
    timeout: Option<Duration>,
) -> Result<Client, String> {
    let mut builder = Client::builder().connect_timeout(Duration::from_secs(15));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder = match proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        Some(proxy_url) => builder.proxy(
            Proxy::all(proxy_url)
                .map_err(|err| format!("Invalid Gemini proxy '{proxy_url}': {err}"))?,
        ),
        None => builder.no_proxy(),
    };
    builder
        .build()
        .map_err(|err| format!("Cannot create Gemini HTTP client: {err}"))
}

fn settings_dir_stamp(settings_dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(settings_dir) else {
        return String::new();
    };
    let mut count = 0u64;
    let mut newest_secs = 0u64;
    let mut total_len = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_provider_profile_file_name(file_name) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        count += 1;
        total_len += metadata.len();
        if let Ok(modified) = metadata.modified() {
            if let Ok(secs) = modified.duration_since(UNIX_EPOCH) {
                newest_secs = newest_secs.max(secs.as_secs());
            }
        }
    }
    format!("{count}:{newest_secs}:{total_len}")
}

fn read_state_object(state_path: &Path) -> Map<String, Value> {
    fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn load_persisted_gemini_proxy(state_path: &Path) -> Option<Option<String>> {
    let value = fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let proxy = value.as_object()?.get("gemini_proxy")?;
    match proxy {
        Value::Null => Some(None),
        Value::String(value) => Some((!value.trim().is_empty()).then(|| value.trim().to_string())),
        _ => None,
    }
}

fn persist_bridge_state(
    state_path: &Path,
    active_profile: &str,
    proxy_url: Option<&str>,
) -> Result<(), String> {
    // Preserve unrelated keys (for example the recorded listen address) that
    // other writers stored in the state file.
    let mut state_json = read_state_object(state_path);
    state_json.insert(
        "active_profile".to_string(),
        Value::String(active_profile.to_string()),
    );
    state_json.insert(
        "gemini_proxy".to_string(),
        proxy_url
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    fs::write(state_path, Value::Object(state_json).to_string()).map_err(|err| {
        format!(
            "Cannot persist bridge state to '{}': {err}",
            state_path.display()
        )
    })
}

fn record_listen_in_state(state_path: &Path, listen: &str) -> Result<(), String> {
    let mut state_json = read_state_object(state_path);
    state_json.insert("listen".to_string(), Value::String(listen.to_string()));
    fs::write(state_path, Value::Object(state_json).to_string()).map_err(|err| {
        format!(
            "Cannot record listen address in '{}': {err}",
            state_path.display()
        )
    })
}

fn current_gemini_transport(state: &AppState) -> Result<GeminiTransport, String> {
    state
        .gemini_transport
        .read()
        .map(|transport| transport.clone())
        .map_err(|_| "Cannot read Gemini proxy state".to_string())
}

fn select_initial_profile(profiles: &[ProviderProfile], state_path: &Path) -> String {
    let persisted = fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("active_profile")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    if let Some(file_name) = persisted {
        if profiles
            .iter()
            .any(|profile| profile.file_name == file_name)
        {
            return file_name;
        }
    }
    profiles
        .iter()
        .find(|profile| profile.local_gemini)
        .or_else(|| profiles.first())
        .map(|profile| profile.file_name.clone())
        .unwrap_or_default()
}

fn active_provider_profile(state: &AppState) -> Option<ProviderProfile> {
    let routing = state.routing.read().ok()?;
    routing
        .profiles
        .iter()
        .find(|profile| profile.file_name == routing.active_file)
        .cloned()
}

fn provider_profile_json(profile: &ProviderProfile, active_file: &str) -> Value {
    json!({
        "file": profile.file_name,
        "model": profile.model,
        "upstream_identity": profile.upstream_identity,
        "identity_override": profile.identity_override,
        "base_url": profile.base_url,
        "proxy": profile.proxy_url,
        "local_gemini": profile.local_gemini,
        "active": profile.file_name == active_file
    })
}

async fn admin_status(State(state): State<Arc<AppState>>) -> Response {
    let transport = match current_gemini_transport(&state) {
        Ok(transport) => transport,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": message})),
            )
                .into_response();
        }
    };
    let Ok(routing) = state.routing.read() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot read provider routing state"})),
        )
            .into_response();
    };
    let profile = routing
        .profiles
        .iter()
        .find(|profile| profile.file_name == routing.active_file);
    Json(json!({
        "status": "ok",
        "active_profile": profile.map(|profile| provider_profile_json(profile, &profile.file_name)),
        "profile_count": routing.profiles.len(),
        "gemini_proxy": transport.proxy_url,
        "gemini_proxy_mode": if transport.proxy_url.is_some() { "proxy" } else { "direct" },
        "listen_url": state.local_bridge_base_url,
        "settings_dir": state.settings_dir.to_string_lossy(),
        "settings_stamp": settings_dir_stamp(&state.settings_dir)
    }))
    .into_response()
}

async fn admin_profiles(State(state): State<Arc<AppState>>) -> Response {
    let Ok(routing) = state.routing.read() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot read provider routing state"})),
        )
            .into_response();
    };
    let profiles = routing
        .profiles
        .iter()
        .map(|profile| provider_profile_json(profile, &routing.active_file))
        .collect::<Vec<_>>();
    Json(json!({
        "profiles": profiles,
        "settings_stamp": settings_dir_stamp(&state.settings_dir)
    }))
    .into_response()
}

async fn admin_set_active_profile(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Response {
    let Some(file_name) = request.get("file").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Missing string field 'file'"})),
        )
            .into_response();
    };
    let Ok(mut routing) = state.routing.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update provider routing state"})),
        )
            .into_response();
    };
    let Some(selected) = routing
        .profiles
        .iter()
        .find(|profile| profile.file_name == file_name)
        .cloned()
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Unknown provider profile"})),
        )
            .into_response();
    };
    let proxy_url = match current_gemini_transport(&state) {
        Ok(transport) => transport.proxy_url,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": message})),
            )
                .into_response();
        }
    };
    if let Err(err) = persist_bridge_state(
        &state.bridge_state_path,
        &selected.file_name,
        proxy_url.as_deref(),
    ) {
        error!("Cannot persist active profile and proxy state: {err}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Provider switched but state was not persisted: {err}")})),
        )
            .into_response();
    }
    routing.active_file = selected.file_name.clone();
    drop(routing);
    Json(json!({
        "status": "ok",
        "active_profile": provider_profile_json(&selected, &selected.file_name)
    }))
    .into_response()
}

async fn admin_reload_profiles(State(state): State<Arc<AppState>>) -> Response {
    let profiles = match load_provider_profiles(&state.settings_dir, &state.local_bridge_base_url) {
        Ok(profiles) => profiles,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let Ok(mut routing) = state.routing.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update provider routing state"})),
        )
            .into_response();
    };
    let active_file = routing.active_file.clone();
    let selected = if profiles
        .iter()
        .any(|profile| profile.file_name == active_file)
    {
        active_file
    } else {
        select_initial_profile(&profiles, &state.bridge_state_path)
    };
    let count = profiles.len();
    let proxy_url = match current_gemini_transport(&state) {
        Ok(transport) => transport.proxy_url,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": message})),
            )
                .into_response();
        }
    };
    if let Err(err) =
        persist_bridge_state(&state.bridge_state_path, &selected, proxy_url.as_deref())
    {
        error!("Cannot persist profiles and proxy state: {err}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Profiles were not reloaded because state persistence failed: {err}")})),
        )
            .into_response();
    }
    routing.profiles = profiles;
    routing.active_file = selected;
    Json(json!({"status": "ok", "profile_count": count})).into_response()
}

fn proxy_from_admin_request(request: &Value) -> Result<Option<String>, String> {
    match request.get("proxy") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.to_string()))
            }
        }
        Some(_) => Err("Field 'proxy' must be a string or null".to_string()),
        None => Err("Missing field 'proxy'".to_string()),
    }
}

async fn admin_set_gemini_proxy(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Response {
    let proxy_url = match proxy_from_admin_request(&request) {
        Ok(proxy_url) => proxy_url,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let client = match build_gemini_client(proxy_url.as_deref(), None) {
        Ok(client) => client,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let active_profile = match state.routing.read() {
        Ok(routing) => routing.active_file.clone(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        }
    };
    if let Err(message) = persist_bridge_state(
        &state.bridge_state_path,
        &active_profile,
        proxy_url.as_deref(),
    ) {
        error!("{message}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": message})),
        )
            .into_response();
    }
    let Ok(mut transport) = state.gemini_transport.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update Gemini proxy state"})),
        )
            .into_response();
    };
    *transport = GeminiTransport {
        client,
        proxy_url: proxy_url.clone(),
    };
    drop(transport);
    info!(
        "Gemini network route changed to {}",
        proxy_url.as_deref().unwrap_or("direct connection")
    );
    Json(json!({
        "status": "ok",
        "gemini_proxy": proxy_url,
        "gemini_proxy_mode": if proxy_url.is_some() { "proxy" } else { "direct" }
    }))
    .into_response()
}

async fn admin_test_gemini_proxy(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Response {
    let proxy_url = match proxy_from_admin_request(&request) {
        Ok(proxy_url) => proxy_url,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let client = match build_gemini_client(proxy_url.as_deref(), Some(Duration::from_secs(25))) {
        Ok(client) => client,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let Some(api_key) = state
        .fallback_api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "Gemini API key is not configured"})),
        )
            .into_response();
    };
    let Some(models_base_url) = state.upstream_url.strip_suffix("/chat/completions") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Gemini upstream URL does not use the OpenAI chat/completions path"})),
        )
            .into_response();
    };
    let model_url = format!("{models_base_url}/models/{}", state.model);
    let upstream = match client.get(&model_url).bearer_auth(api_key).send().await {
        Ok(response) => response,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Gemini connection test failed: {err}")})),
            )
                .into_response();
        }
    };
    let status = upstream.status();
    let body = upstream.json::<Value>().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": format!("Gemini returned HTTP {status}: {}", safe_error_message(&body))
            })),
        )
            .into_response();
    }
    Json(json!({
        "status": "ok",
        "model": body.get("id").and_then(Value::as_str).unwrap_or(&state.model),
        "gemini_proxy": proxy_url,
        "gemini_proxy_mode": if proxy_url.is_some() { "proxy" } else { "direct" }
    }))
    .into_response()
}

async fn admin_shutdown(State(state): State<Arc<AppState>>) -> Response {
    if state.shutdown_tx.send(true).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Graceful shutdown channel is unavailable"})),
        )
            .into_response();
    }
    Json(json!({"status": "shutting_down"})).into_response()
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let active_profile = active_provider_profile(&state);
    let proxy_url = current_gemini_transport(&state)
        .ok()
        .and_then(|transport| transport.proxy_url);
    Json(json!({
        "status": "ok",
        "model": state.model,
        "upstream": state.upstream_url,
        "gemini_proxy": proxy_url,
        "active_profile": active_profile.as_ref().map(|profile| provider_profile_json(profile, &profile.file_name))
    }))
}

async fn models(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "object": "list",
        "models": [],
        "data": [{
            "id": state.model,
            "object": "model",
            "created": 0,
            "owned_by": "google"
        }]
    }))
}

async fn responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    // Legacy compatibility only. Codex is intentionally kept on its native GPT
    // provider and is not maintained through this bridge. Do not expand this
    // route unless the maintenance policy at the top of this file changes.
    let api_key = bearer_token(&headers)
        .or_else(|| state.fallback_api_key.clone())
        .filter(|value| !value.trim().is_empty());
    let Some(api_key) = api_key else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"code": "authentication_error", "message": "Missing Bearer API key"}})),
        )
            .into_response();
    };

    let chat_request = match translate_request(&request, &state.model, &state.thought_signatures) {
        Ok(value) => value,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"code": "invalid_prompt", "message": message}})),
            )
                .into_response();
        }
    };
    let transport = match current_gemini_transport(&state) {
        Ok(transport) => transport,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"code": "server_error", "message": message}})),
            )
                .into_response();
        }
    };

    let upstream = match transport
        .client
        .post(&state.upstream_url)
        .bearer_auth(api_key)
        .json(&chat_request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Gemini request failed: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": "server_error", "message": err.to_string()}})),
            )
                .into_response();
        }
    };

    let status = upstream.status();
    let upstream_body = match upstream.json::<Value>().await {
        Ok(value) => value,
        Err(err) => {
            error!("Gemini returned a non-JSON response: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": "server_error", "message": "Gemini returned a non-JSON response"}})),
            )
                .into_response();
        }
    };

    if !status.is_success() {
        error!(
            "Gemini returned HTTP {status}: {}",
            safe_error_message(&upstream_body)
        );
        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(upstream_body),
        )
            .into_response();
    }

    let events = match translate_response_events(
        &request,
        &upstream_body,
        &state.model,
        &state.thought_signatures,
    ) {
        Ok(events) => events,
        Err(message) => {
            error!("Cannot translate Gemini response: {message}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": "server_error", "message": message}})),
            )
                .into_response();
        }
    };

    let event_stream = stream::iter(events.into_iter().map(Ok::<Event, Infallible>));
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    // Claude Code only requires a non-empty local gateway token. When the
    // bridge was started with GEMINI_API_KEY, keep the real Google key inside
    // the bridge process instead of duplicating it in Claude settings.
    let api_key = state
        .fallback_api_key
        .clone()
        .or_else(|| bearer_token(&headers))
        .filter(|value| !value.trim().is_empty());
    let Some(api_key) = api_key else {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "Missing API key",
        );
    };

    let Some(active_profile) = active_provider_profile(&state) else {
        return anthropic_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "No active provider profile",
        );
    };
    if let Some(identity) = upstream_identity_label(&active_profile, &state.model) {
        if let Err(message) = append_bridge_identity(&mut request, &identity) {
            return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
        }
    }
    if !active_profile.local_gemini {
        return forward_anthropic_profile(active_profile, &headers, request).await;
    }

    let chat_request =
        match translate_anthropic_request(&request, &state.model, &state.thought_signatures) {
            Ok(value) => value,
            Err(message) => {
                return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        };
    let transport = match current_gemini_transport(&state) {
        Ok(transport) => transport,
        Err(message) => {
            return anthropic_error(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &message);
        }
    };

    let upstream = match transport
        .client
        .post(&state.upstream_url)
        .bearer_auth(api_key)
        .json(&chat_request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Gemini request failed: {err}");
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &err.to_string());
        }
    };

    let status = upstream.status();
    if !status.is_success() {
        let upstream_body = match upstream.json::<Value>().await {
            Ok(value) => value,
            Err(err) => {
                error!("Gemini returned a non-JSON error response: {err}");
                return anthropic_error(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    "Gemini returned a non-JSON error response",
                );
            }
        };
        let message = safe_error_message(&upstream_body);
        error!("Gemini returned HTTP {status}: {message}");
        return anthropic_error(
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            "api_error",
            &message,
        );
    }

    let stream_requested = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if stream_requested {
        let estimated_input_tokens =
            u64::try_from(estimate_anthropic_input_tokens(&request)).unwrap_or(u64::MAX);
        return anthropic_upstream_stream_response(
            upstream,
            state.model.clone(),
            state.thought_signatures.clone(),
            estimated_input_tokens,
        );
    }

    let upstream_body = match upstream.json::<Value>().await {
        Ok(value) => value,
        Err(err) => {
            error!("Gemini returned a non-JSON response: {err}");
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Gemini returned a non-JSON response",
            );
        }
    };
    let message =
        match translate_anthropic_response(&upstream_body, &state.model, &state.thought_signatures)
        {
            Ok(value) => value,
            Err(message) => {
                error!("Cannot translate Gemini response: {message}");
                return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &message);
            }
        };
    Json(message).into_response()
}

fn is_claude_identity(identity: &str) -> bool {
    let identity = identity.to_ascii_lowercase();
    identity.contains("claude") || identity.contains("anthropic")
}

async fn forward_anthropic_profile(
    profile: ProviderProfile,
    client_headers: &HeaderMap,
    mut request: Value,
) -> Response {
    request["model"] = json!(profile.model);
    let upstream_url = format!("{}/v1/messages", profile.base_url.trim_end_matches('/'));
    let upstream_request = profile.client.post(&upstream_url).json(&request);
    let upstream_request =
        apply_anthropic_forward_headers(upstream_request, &profile, client_headers);

    let upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(err) => {
            error!(
                "Provider '{}' request to '{}' failed: {err}",
                profile.file_name, upstream_url
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &err.to_string());
        }
    };
    let status = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| "application/json".to_string());
    let request_id = upstream
        .headers()
        .get("request-id")
        .or_else(|| upstream.headers().get("x-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = Body::from_stream(upstream.bytes_stream());
    let mut builder = Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("cache-control", "no-cache");
    if let Some(request_id) = request_id {
        builder = builder.header("request-id", request_id);
    }
    builder.body(body).unwrap_or_else(|err| {
        anthropic_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("Cannot build provider response: {err}"),
        )
    })
}

fn apply_anthropic_forward_headers(
    mut upstream_request: reqwest::RequestBuilder,
    profile: &ProviderProfile,
    client_headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    if let Some(token) = &profile.auth_token {
        upstream_request = upstream_request.bearer_auth(token);
    } else if let Some(api_key) = &profile.api_key {
        upstream_request = upstream_request.header("x-api-key", api_key);
    }
    let anthropic_version = client_headers
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(DEFAULT_ANTHROPIC_VERSION);
    upstream_request = upstream_request.header("anthropic-version", anthropic_version);
    if let Some(value) = client_headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
    {
        upstream_request = upstream_request.header("anthropic-beta", value);
    }
    upstream_request
}

#[derive(Default)]
struct SseDataDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseDataDecoder {
    fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut payloads = Vec::new();

        while let Some(newline_index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line_bytes: Vec<u8> = self.buffer.drain(..=newline_index).collect();
            line_bytes.pop();
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8(line_bytes)
                .map_err(|err| format!("Invalid UTF-8 in Gemini SSE stream: {err}"))?;
            self.process_line(&line, &mut payloads);
        }

        Ok(payloads)
    }

    fn finish(&mut self) -> Result<Vec<String>, String> {
        let mut payloads = Vec::new();
        if !self.buffer.is_empty() {
            let mut line_bytes = std::mem::take(&mut self.buffer);
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8(line_bytes)
                .map_err(|err| format!("Invalid UTF-8 at end of Gemini SSE stream: {err}"))?;
            self.process_line(&line, &mut payloads);
        }
        self.flush_data(&mut payloads);
        Ok(payloads)
    }

    fn process_line(&mut self, line: &str, payloads: &mut Vec<String>) {
        if line.is_empty() {
            self.flush_data(payloads);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data_lines
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }

    fn flush_data(&mut self, payloads: &mut Vec<String>) {
        if !self.data_lines.is_empty() {
            payloads.push(std::mem::take(&mut self.data_lines).join("\n"));
        }
    }
}

#[derive(Default)]
struct StreamingToolCall {
    id: String,
    name: String,
    arguments: String,
    thought_signature: Option<String>,
}

fn finish_reason_is(finish_reason: Option<&str>, expected: &[&str]) -> bool {
    finish_reason.is_some_and(|reason| {
        expected
            .iter()
            .any(|candidate| reason.eq_ignore_ascii_case(candidate))
    })
}

fn anthropic_stop_reason(finish_reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if finish_reason_is(finish_reason, &["length", "max_tokens"]) {
        "max_tokens"
    } else if finish_reason_is(finish_reason, &["content_filter", "safety", "blocked"]) {
        "refusal"
    } else if has_tool_calls {
        "tool_use"
    } else {
        "end_turn"
    }
}

fn parse_tool_arguments(arguments: &str) -> Result<Value, String> {
    let arguments = if arguments.is_empty() {
        "{}"
    } else {
        arguments
    };
    let input: Value = serde_json::from_str(arguments)
        .map_err(|err| format!("Upstream returned invalid tool arguments JSON: {err}"))?;
    if !input.is_object() {
        return Err("Upstream tool arguments must be a JSON object".to_string());
    }
    Ok(input)
}

struct AnthropicStreamTranslator {
    message_id: String,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    next_content_index: usize,
    thinking_block_index: Option<usize>,
    text_block_index: Option<usize>,
    tool_calls: IndexMap<String, StreamingToolCall>,
    next_anonymous_tool: usize,
    finish_reason: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    finished: bool,
}

impl AnthropicStreamTranslator {
    fn new(
        model: String,
        thought_signatures: Arc<ThoughtSignatureCache>,
        estimated_input_tokens: u64,
    ) -> Self {
        Self {
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            model,
            thought_signatures,
            next_content_index: 0,
            thinking_block_index: None,
            text_block_index: None,
            tool_calls: IndexMap::new(),
            next_anonymous_tool: 0,
            finish_reason: None,
            input_tokens: estimated_input_tokens,
            output_tokens: 0,
            finished: false,
        }
    }

    fn start_events(&self) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        push_anthropic_event(
            &mut events,
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {
                        "input_tokens": self.input_tokens,
                        "output_tokens": 0
                    }
                }
            }),
        )?;
        Ok(events)
    }

    fn process_payload(&mut self, payload: &str) -> Result<Vec<Event>, String> {
        let chunk: Value = serde_json::from_str(payload)
            .map_err(|err| format!("Invalid JSON in Gemini SSE stream: {err}"))?;
        if chunk.get("error").is_some() {
            return Err(safe_error_message(&chunk));
        }

        if let Some(usage) = chunk.get("usage") {
            self.input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(self.input_tokens);
            self.output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(self.output_tokens);
        }

        let Some(choice) = chunk.pointer("/choices/0") else {
            return Ok(Vec::new());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(Vec::new());
        };

        let mut events = Vec::new();
        let reasoning_text = delta
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .or_else(|| {
                delta
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
            });
        if let Some(reasoning_text) = reasoning_text {
            let index = if let Some(index) = self.thinking_block_index {
                index
            } else {
                let index = self.next_content_index;
                self.next_content_index += 1;
                self.thinking_block_index = Some(index);
                push_anthropic_event(
                    &mut events,
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "thinking", "thinking": ""}
                    }),
                )?;
                index
            };
            push_anthropic_event(
                &mut events,
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {"type": "thinking_delta", "thinking": reasoning_text}
                }),
            )?;
        }

        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                self.stop_thinking_block(&mut events)?;
                let index = if let Some(index) = self.text_block_index {
                    index
                } else {
                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    self.text_block_index = Some(index);
                    push_anthropic_event(
                        &mut events,
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": {"type": "text", "text": ""}
                        }),
                    )?;
                    index
                };
                push_anthropic_event(
                    &mut events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                )?;
            }
        }

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            if !tool_calls.is_empty() {
                self.stop_thinking_block(&mut events)?;
            }
            for tool_call in tool_calls {
                self.accumulate_tool_call(tool_call);
            }
        }
        Ok(events)
    }

    fn stop_thinking_block(&mut self, events: &mut Vec<Event>) -> Result<(), String> {
        if let Some(index) = self.thinking_block_index.take() {
            push_anthropic_event(
                events,
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )?;
        }
        Ok(())
    }

    fn accumulate_tool_call(&mut self, tool_call: &Value) {
        let index = tool_call.get("index").and_then(Value::as_u64);
        let incoming_id = tool_call.get("id").and_then(Value::as_str);
        let key = if let Some(index) = index {
            format!("index:{index}")
        } else if let Some(id) = incoming_id {
            format!("id:{id}")
        } else if self.tool_calls.len() == 1 {
            self.tool_calls
                .get_index(0)
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| self.next_tool_key())
        } else {
            self.next_tool_key()
        };

        let entry = self.tool_calls.entry(key).or_insert_with(|| {
            let id = incoming_id
                .map(str::to_owned)
                .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
            StreamingToolCall {
                id,
                ..StreamingToolCall::default()
            }
        });
        if let Some(id) = incoming_id {
            entry.id = id.to_string();
        }
        if let Some(name) = tool_call.pointer("/function/name").and_then(Value::as_str) {
            if entry.name.is_empty() {
                entry.name = name.to_string();
            } else if entry.name != name && !name.is_empty() {
                entry.name.push_str(name);
            }
        }
        if let Some(arguments) = tool_call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
        {
            entry.arguments.push_str(arguments);
        }
        if let Some(signature) = tool_call
            .pointer("/extra_content/google/thought_signature")
            .and_then(Value::as_str)
        {
            entry.thought_signature = Some(signature.to_string());
        }
    }

    fn next_tool_key(&mut self) -> String {
        let key = format!("anonymous:{}", self.next_anonymous_tool);
        self.next_anonymous_tool += 1;
        key
    }

    fn finish(&mut self) -> Result<Vec<Event>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let mut events = Vec::new();

        self.stop_thinking_block(&mut events)?;
        if let Some(index) = self.text_block_index.take() {
            push_anthropic_event(
                &mut events,
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )?;
        }

        let stop_reason =
            anthropic_stop_reason(self.finish_reason.as_deref(), !self.tool_calls.is_empty());
        let emit_tool_calls = stop_reason == "tool_use";
        if emit_tool_calls {
            for tool_call in self.tool_calls.values() {
                parse_tool_arguments(&tool_call.arguments)?;
            }

            for tool_call in self.tool_calls.values() {
                let index = self.next_content_index;
                self.next_content_index += 1;
                let name = if tool_call.name.is_empty() {
                    "unknown_function"
                } else {
                    &tool_call.name
                };
                let arguments = if tool_call.arguments.is_empty() {
                    "{}"
                } else {
                    &tool_call.arguments
                };
                if let Some(signature) = &tool_call.thought_signature {
                    remember_thought_signature(&self.thought_signatures, &tool_call.id, signature);
                }
                push_anthropic_event(
                    &mut events,
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {
                            "type": "tool_use",
                            "id": tool_call.id,
                            "name": name,
                            "input": {}
                        }
                    }),
                )?;
                push_anthropic_event(
                    &mut events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": arguments
                        }
                    }),
                )?;
                push_anthropic_event(
                    &mut events,
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                )?;
            }
        }

        push_anthropic_event(
            &mut events,
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": stop_reason,
                    "stop_sequence": Value::Null
                },
                "usage": {
                    "output_tokens": self.output_tokens
                }
            }),
        )?;
        push_anthropic_event(&mut events, "message_stop", json!({"type": "message_stop"}))?;
        Ok(events)
    }
}

fn anthropic_upstream_stream_response(
    upstream: reqwest::Response,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    estimated_input_tokens: u64,
) -> Response {
    let byte_stream = Box::pin(upstream.bytes_stream());
    let translator =
        AnthropicStreamTranslator::new(model, thought_signatures, estimated_input_tokens);
    let initial_events = match translator.start_events() {
        Ok(events) => VecDeque::from(events),
        Err(message) => VecDeque::from([anthropic_stream_error_event(&message)]),
    };
    let decoder = SseDataDecoder::default();

    let event_stream = stream::unfold(
        (byte_stream, decoder, translator, initial_events, false),
        |(mut byte_stream, mut decoder, mut translator, mut pending, mut ended)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((
                        Ok::<Event, Infallible>(event),
                        (byte_stream, decoder, translator, pending, ended),
                    ));
                }
                if ended {
                    return None;
                }

                match byte_stream.next().await {
                    Some(Ok(bytes)) => match decoder.push_bytes(&bytes) {
                        Ok(payloads) => {
                            for payload in payloads {
                                if payload.trim() == "[DONE]" {
                                    match translator.finish() {
                                        Ok(events) => pending.extend(events),
                                        Err(message) => pending
                                            .push_back(anthropic_stream_error_event(&message)),
                                    }
                                    ended = true;
                                    break;
                                }
                                match translator.process_payload(&payload) {
                                    Ok(events) => pending.extend(events),
                                    Err(message) => {
                                        pending.push_back(anthropic_stream_error_event(&message));
                                        ended = true;
                                        break;
                                    }
                                }
                            }
                        }
                        Err(message) => {
                            pending.push_back(anthropic_stream_error_event(&message));
                            ended = true;
                        }
                    },
                    Some(Err(err)) => {
                        pending.push_back(anthropic_stream_error_event(&format!(
                            "Gemini stream failed: {err}"
                        )));
                        ended = true;
                    }
                    None => {
                        match decoder.finish() {
                            Ok(payloads) => {
                                for payload in payloads {
                                    if payload.trim() != "[DONE]" {
                                        match translator.process_payload(&payload) {
                                            Ok(events) => pending.extend(events),
                                            Err(message) => pending
                                                .push_back(anthropic_stream_error_event(&message)),
                                        }
                                    }
                                }
                            }
                            Err(message) => {
                                pending.push_back(anthropic_stream_error_event(&message))
                            }
                        }
                        match translator.finish() {
                            Ok(events) => pending.extend(events),
                            Err(message) => {
                                pending.push_back(anthropic_stream_error_event(&message))
                            }
                        }
                        ended = true;
                    }
                }
            }
        },
    );

    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

fn anthropic_stream_error_event(message: &str) -> Event {
    Event::default().event("error").data(
        json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": message
            }
        })
        .to_string(),
    )
}

async fn anthropic_count_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    let has_api_key = bearer_token(&headers)
        .or_else(|| state.fallback_api_key.clone())
        .is_some_and(|value| !value.trim().is_empty());
    if !has_api_key {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "Missing API key",
        );
    }

    if let Some(profile) = active_provider_profile(&state) {
        if let Some(identity) = upstream_identity_label(&profile, &state.model) {
            if let Err(message) = append_bridge_identity(&mut request, &identity) {
                return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        }
    }

    let input_tokens = estimate_anthropic_input_tokens(&request);
    Json(json!({"input_tokens": input_tokens})).into_response()
}

fn estimate_anthropic_input_tokens(request: &Value) -> usize {
    // Claude Code uses this value for proactive context management. The
    // OpenAI-compatible Gemini endpoint has no tokenizer route, so estimate
    // ASCII at roughly four bytes per token and non-ASCII UTF-8 at two bytes
    // per token. Walk the existing Value tree instead of allocating another
    // complete serialized copy of a potentially very large conversation.
    let (ascii_bytes, non_ascii_bytes) = count_serialized_json_bytes(request);
    (ascii_bytes.div_ceil(4) + non_ascii_bytes.div_ceil(2)).max(1)
}

fn count_serialized_json_bytes(value: &Value) -> (usize, usize) {
    fn add_json_string(value: &str, ascii_bytes: &mut usize, non_ascii_bytes: &mut usize) {
        *ascii_bytes += 2;
        for character in value.chars() {
            if character.is_ascii() {
                *ascii_bytes += match character {
                    '"' | '\\' | '\u{0008}' | '\t' | '\n' | '\u{000C}' | '\r' => 2,
                    '\u{0000}'..='\u{001F}' => 6,
                    _ => 1,
                };
            } else {
                *non_ascii_bytes += character.len_utf8();
            }
        }
    }

    fn visit(value: &Value, ascii_bytes: &mut usize, non_ascii_bytes: &mut usize) {
        match value {
            Value::Null => *ascii_bytes += 4,
            Value::Bool(true) => *ascii_bytes += 4,
            Value::Bool(false) => *ascii_bytes += 5,
            Value::Number(number) => *ascii_bytes += number.to_string().len(),
            Value::String(text) => add_json_string(text, ascii_bytes, non_ascii_bytes),
            Value::Array(items) => {
                *ascii_bytes += 2 + items.len().saturating_sub(1);
                for item in items {
                    visit(item, ascii_bytes, non_ascii_bytes);
                }
            }
            Value::Object(object) => {
                *ascii_bytes += 2 + object.len().saturating_sub(1) + object.len();
                for (key, item) in object {
                    add_json_string(key, ascii_bytes, non_ascii_bytes);
                    visit(item, ascii_bytes, non_ascii_bytes);
                }
            }
        }
    }

    let mut ascii_bytes = 0;
    let mut non_ascii_bytes = 0;
    visit(value, &mut ascii_bytes, &mut non_ascii_bytes);
    (ascii_bytes, non_ascii_bytes)
}

fn anthropic_error(status: StatusCode, error_type: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "type": "error",
            "error": {
                "type": error_type,
                "message": message
            }
        })),
    )
        .into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        if let Some(token) = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
        {
            return Some(token.to_owned());
        }
    }

    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn remember_thought_signature(
    thought_signatures: &ThoughtSignatureCache,
    call_id: &str,
    signature: &str,
) {
    let Ok(mut cache) = thought_signatures.write() else {
        return;
    };

    // Refresh an existing entry to the newest position. At capacity, evict a
    // bounded batch of the oldest entries instead of invalidating every active
    // conversation by clearing the whole cache.
    let existed = cache.shift_remove(call_id).is_some();
    if !existed && cache.len() >= THOUGHT_SIGNATURE_CAPACITY {
        let eviction_count = THOUGHT_SIGNATURE_EVICTION_BATCH.min(cache.len());
        for _ in 0..eviction_count {
            cache.shift_remove_index(0);
        }
    }
    cache.insert(call_id.to_string(), signature.to_string());
}

fn sanitize_identity_label(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let sanitized = normalized
        .chars()
        .take(MAX_UPSTREAM_IDENTITY_CHARS)
        .collect::<String>();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn upstream_identity_label(profile: &ProviderProfile, local_model: &str) -> Option<String> {
    if !profile.identity_override {
        return None;
    }

    let identity = profile.upstream_identity.clone().unwrap_or_else(|| {
        if profile.local_gemini {
            format!("Google Gemini ({local_model})")
        } else {
            display_model_name(&profile.model)
        }
    });
    sanitize_identity_label(&identity)
}

/// Model IDs can carry routing suffixes such as "deepseek-v4-pro[1m]". Those
/// are transport details, not part of a presentable identity.
fn display_model_name(model: &str) -> String {
    if let Some(bracket_start) = model.find('[') {
        if model.ends_with(']') {
            let trimmed = model[..bracket_start].trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    model.to_string()
}

fn bridge_identity_note(identity: &str) -> String {
    format!(
        "{BRIDGE_IDENTITY_MARKER}\nRouting metadata supplied by the bridge: this request is served by the upstream model \"{identity}\". Your actual model and first-person assistant identity is \"{identity}\". This is factual runtime information, not a request to role-play or impersonate another model. Claude Code is only the client and agent environment, not your identity. If asked who you are, what model you are, or who developed you, answer truthfully based on what you actually are.\n桥接器路由信息：本次请求由上游模型“{identity}”提供服务，你的真实模型与第一人称身份是“{identity}”。这是运行时事实，不是角色扮演。Claude Code 只是客户端与智能体环境，不是你的身份。当被问及你是谁、你是什么模型或由谁开发时，请根据你的真实身份如实回答。\n</bridge_runtime_identity>"
    )
}

fn replace_all_occurrences(text: &mut String, from: &str, to: &str) -> bool {
    let mut changed = false;
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(from) {
        let start = cursor + offset;
        text.replace_range(start..start + from.len(), to);
        cursor = start + to.len();
        changed = true;
    }
    changed
}

/// Find the end of the sentence starting at `start` (index just past the
/// terminating period), bounded to a single line and a sane length so a stray
/// match cannot swallow a whole system prompt.
fn sentence_end_after(text: &str, start: usize) -> Option<usize> {
    const MAX_SENTENCE_CHARS: usize = 300;
    for (offset, character) in text[start..].char_indices() {
        if offset >= MAX_SENTENCE_CHARS || character == '\n' {
            return None;
        }
        if character == '.' {
            return Some(start + offset + 1);
        }
    }
    None
}

/// The persona declaration that replaces Claude Code's client persona. Merely
/// saying "you operate inside the Claude Code client" is not enough: upstream
/// models without an identity anchor can infer a Claude identity from the
/// "Claude Code" context alone. The replacement must anchor the true identity
/// and ask for truthful identity answers.
fn persona_declaration(identity: &str) -> String {
    format!(
        "You are \"{identity}\", the upstream model serving this route. You operate inside the Claude Code client. Answer questions about your identity truthfully based on what you actually are."
    )
}

/// Catch reworded persona declarations Claude Code updates may introduce, e.g.
/// "You are Claude Code, a coding assistant built by Anthropic." Only
/// sentence-initial occurrences are rewritten; the replacement itself cannot
/// match the prefixes again, so the scan terminates.
fn neutralize_claude_persona_sentences(text: &mut String, identity: &str) -> bool {
    const PERSONA_PREFIXES: [&str; 2] = ["You are Claude Code", "You are a Claude agent"];
    let replacement = persona_declaration(identity);

    let mut changed = false;
    let mut cursor = 0;
    while cursor < text.len() {
        let Some((start, prefix)) = PERSONA_PREFIXES
            .iter()
            .filter_map(|prefix| {
                let offset = text[cursor..].find(prefix)?;
                Some((cursor + offset, *prefix))
            })
            .min_by_key(|(start, _)| *start)
        else {
            break;
        };
        let at_sentence_start = start == 0 || text[..start].ends_with('\n');
        if !at_sentence_start {
            cursor = start + prefix.len();
            continue;
        }
        let Some(end) = sentence_end_after(text, start) else {
            cursor = start + prefix.len();
            continue;
        };
        text.replace_range(start..end, &replacement);
        cursor = start + replacement.len();
        changed = true;
    }
    changed
}

/// The "# Environment" section says "You are powered by the model named X. The
/// exact model ID is Y." with the client's configured model name, which is the
/// wrong upstream for bridged routes. Rewrite the whole line; it is always a
/// standalone line in Claude Code's system prompt.
fn neutralize_powered_by_line(text: &mut String, identity: &str) -> bool {
    let replacement = format!("You are powered by {identity}.");
    let exact_model_prefix = CLAUDE_EXACT_MODEL_ID_PREFIX.trim_start();
    let mut changed = false;
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(CLAUDE_POWERED_BY_PREFIX) {
        let start = cursor + offset;
        let line_start = text[..start]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let line_end = text[start..]
            .find('\n')
            .map(|index| start + index)
            .unwrap_or(text.len());
        if !text[line_start..line_end]
            .trim_start()
            .starts_with(CLAUDE_POWERED_BY_PREFIX)
        {
            cursor = start + CLAUDE_POWERED_BY_PREFIX.len();
            continue;
        }
        // Claude Code sometimes splits the exact-model-ID sentence onto its own
        // line; drop that line too.
        let mut replace_end = line_end;
        if line_end < text.len() {
            let next_line_start = line_end + 1;
            let next_line_end = text[next_line_start..]
                .find('\n')
                .map(|index| next_line_start + index)
                .unwrap_or(text.len());
            if text[next_line_start..next_line_end]
                .trim_start()
                .starts_with(exact_model_prefix)
            {
                replace_end = next_line_end;
            }
        }
        text.replace_range(line_start..replace_end, &replacement);
        cursor = line_start + replacement.len();
        changed = true;
    }
    changed
}

fn neutralize_claude_identity_declaration(text: &mut String, identity: &str) -> bool {
    if is_claude_identity(identity) {
        return false;
    }

    let persona = persona_declaration(identity);
    // Same wording without the leading "You are" so declarations that carry a
    // role prefix ("You are a file search specialist for ...", "You are an
    // agent for ...") keep the role and only swap the product identity.
    let persona_tail = format!(
        "\"{identity}\", the upstream model serving this route, operating inside the Claude Code client"
    );

    let mut changed = false;

    // 1. Whole-sentence persona declarations.
    changed |= replace_all_occurrences(text, CLAUDE_AGENT_SDK_DECLARATION, &persona);
    changed |= replace_all_occurrences(text, CLAUDE_COORDINATOR_DECLARATION, &persona);

    // 2. The "Claude Code, Anthropic's official CLI for Claude" phrase with an
    //    optional SDK suffix. This covers the main persona and every subagent
    //    persona variant built on the same phrase.
    let sdk_phrase = format!("{CLAUDE_OFFICIAL_CLI_PHRASE}{CLAUDE_CLI_SDK_SUFFIX}");
    changed |= replace_all_occurrences(text, &sdk_phrase, &persona_tail);
    changed |= replace_all_occurrences(text, CLAUDE_OFFICIAL_CLI_PHRASE, &persona_tail);

    // 3. Fallback for future rewordings of the persona sentences.
    changed |= neutralize_claude_persona_sentences(text, identity);

    // 4. The model line Claude Code injects always names the configured model,
    //    not the bridge's actual upstream.
    changed |= neutralize_powered_by_line(text, identity);

    // 5. Git attribution would otherwise stamp "Co-Authored-By: Claude" into
    //    every commit made through a bridged model.
    let co_author = format!("Co-Authored-By: {identity} <noreply@anthropic.com>");
    changed |= replace_all_occurrences(text, CLAUDE_CO_AUTHOR_LINE, &co_author);

    changed
}

fn neutralize_system_identity(system: &mut Value, identity: &str) -> bool {
    match system {
        Value::String(text) => neutralize_claude_identity_declaration(text, identity),
        Value::Array(blocks) => {
            let mut changed = false;
            for block in blocks {
                if let Some(Value::String(text)) = block.get_mut("text") {
                    changed |= neutralize_claude_identity_declaration(text, identity);
                }
            }
            changed
        }
        _ => false,
    }
}

fn append_bridge_identity(request: &mut Value, identity: &str) -> Result<bool, String> {
    let request = request
        .as_object_mut()
        .ok_or_else(|| "Anthropic request body must be a JSON object".to_string())?;
    let system = request.entry("system").or_insert(Value::Null);
    if value_to_text(system).contains(BRIDGE_IDENTITY_MARKER) {
        return Ok(false);
    }

    neutralize_system_identity(system, identity);
    let note = bridge_identity_note(identity);
    match system {
        Value::Null => *system = Value::String(note),
        Value::String(text) => {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&note);
        }
        Value::Array(blocks) => blocks.push(json!({"type": "text", "text": note})),
        _ => {
            return Err(
                "Anthropic field 'system' must be a string, an array of content blocks, or null"
                    .to_string(),
            )
        }
    }
    Ok(true)
}

fn translate_anthropic_request(
    request: &Value,
    default_model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    let mut messages = Vec::new();
    let mut runtime_identity_reminder = None;

    if let Some(system) = request.get("system") {
        let text = value_to_text(system);
        if !text.is_empty() {
            runtime_identity_reminder = text
                .rfind(BRIDGE_IDENTITY_MARKER)
                .map(|start| text[start..].to_string());
            messages.push(json!({"role": "system", "content": text}));
        }
    }

    let source_messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'messages'".to_string())?;

    for message in source_messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "Each message must have a role".to_string())?;
        let content = message.get("content").unwrap_or(&Value::Null);

        match role {
            "assistant" => {
                translate_anthropic_assistant_message(content, &mut messages, thought_signatures);
            }
            "user" => translate_anthropic_user_message(content, &mut messages),
            "system" | "developer" => {
                let text = value_to_text(content);
                if !text.is_empty() {
                    messages.push(json!({"role": "system", "content": text}));
                }
            }
            _ => return Err(format!("Unsupported Anthropic message role '{role}'")),
        }
    }

    if let Some(reminder) = runtime_identity_reminder {
        messages.push(json!({"role": "system", "content": reminder}));
    }

    let mut body = Map::new();
    // Claude Code normally sends a Claude model alias. This bridge is bound to
    // one Gemini model, so never forward the client-side alias upstream.
    body.insert("model".to_string(), json!(default_model));
    body.insert("messages".to_string(), Value::Array(messages));
    let stream_requested = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    body.insert("stream".to_string(), Value::Bool(stream_requested));
    if stream_requested {
        body.insert("stream_options".to_string(), json!({"include_usage": true}));
    }

    if let Some(max_tokens) = request.get("max_tokens").and_then(Value::as_u64) {
        body.insert("max_tokens".to_string(), json!(max_tokens));
    }
    if let Some(temperature) = request.get("temperature").and_then(Value::as_f64) {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.get("top_p").and_then(Value::as_f64) {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(stop_sequences) = request.get("stop_sequences").and_then(Value::as_array) {
        if !stop_sequences.is_empty() {
            body.insert("stop".to_string(), Value::Array(stop_sequences.clone()));
        }
    }

    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        let translated_tools: Vec<Value> =
            tools.iter().filter_map(translate_anthropic_tool).collect();
        if !translated_tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(translated_tools));
        }
    }

    if let Some(choice) = request.get("tool_choice") {
        if let Some(translated) = translate_anthropic_tool_choice(choice) {
            body.insert("tool_choice".to_string(), translated);
        }
        if choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            body.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        }
    }

    if let Some(thinking) = request.get("thinking") {
        let effort = thinking
            .get("budget_tokens")
            .and_then(Value::as_u64)
            .map(|budget| {
                if budget >= 8_192 {
                    "high"
                } else if budget >= 2_048 {
                    "medium"
                } else {
                    "low"
                }
            })
            .or_else(|| {
                (thinking.get("type").and_then(Value::as_str) == Some("adaptive")).then_some("high")
            });
        if let Some(effort) = effort {
            body.insert("reasoning_effort".to_string(), json!(effort));
        }
    }

    Ok(Value::Object(body))
}

fn translate_anthropic_assistant_message(
    content: &Value,
    messages: &mut Vec<Value>,
    thought_signatures: &ThoughtSignatureCache,
) {
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            messages.push(json!({"role": "assistant", "content": text}));
        }
        return;
    }

    let Some(parts) = content.as_array() else {
        return;
    };
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            Some("tool_use") => {
                let call_id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown");
                let name = part
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_function");
                let arguments = part
                    .get("input")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".to_string());
                let mut translated = json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                });
                if let Some(signature) = thought_signatures
                    .read()
                    .ok()
                    .and_then(|cache| cache.get(call_id).cloned())
                {
                    translated["extra_content"] = json!({
                        "google": {"thought_signature": signature}
                    });
                }
                tool_calls.push(translated);
            }
            Some("thinking") | Some("redacted_thinking") => {}
            _ => {}
        }
    }

    if !text_parts.is_empty() || !tool_calls.is_empty() {
        messages.push(json!({
            "role": "assistant",
            "content": if text_parts.is_empty() {
                Value::Null
            } else {
                Value::String(text_parts.join("\n"))
            },
            "tool_calls": tool_calls
        }));
    }
}

fn translate_anthropic_user_message(content: &Value, messages: &mut Vec<Value>) {
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            messages.push(json!({"role": "user", "content": text}));
        }
        return;
    }

    let Some(parts) = content.as_array() else {
        return;
    };
    let mut tool_results = Vec::new();
    let mut user_parts = Vec::new();

    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                let tool_use_id = part
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown");
                let result_content = translate_anthropic_tool_result_content(
                    part.get("content").unwrap_or(&Value::Null),
                    part.get("is_error").and_then(Value::as_bool) == Some(true),
                );
                tool_results.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": result_content
                }));
            }
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    user_parts.push(json!({"type": "text", "text": text}));
                }
            }
            Some("image") => {
                if let Some(media_part) = translate_anthropic_base64_media(part) {
                    user_parts.push(media_part);
                }
            }
            Some("document") => {
                if let Some(media_part) = translate_anthropic_base64_media(part) {
                    user_parts.push(media_part);
                }
            }
            _ => {}
        }
    }

    // Gemini requires every assistant tool_call to be followed immediately by
    // its role=tool result. Claude may mix tool_result with text or images in
    // one user message, so emit every tool result before the remaining user
    // content regardless of their original content-block order.
    messages.extend(tool_results);
    flush_anthropic_user_parts(messages, &mut user_parts);
}

fn translate_anthropic_tool_result_content(content: &Value, is_error: bool) -> Value {
    if let Some(parts) = content.as_array() {
        let mut translated_parts = Vec::new();
        let mut has_media = false;

        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            translated_parts.push(json!({"type": "text", "text": text}));
                        }
                    }
                }
                Some("image") | Some("document") => {
                    if let Some(media_part) = translate_anthropic_base64_media(part) {
                        has_media = true;
                        translated_parts.push(media_part);
                    }
                }
                _ => {}
            }
        }

        if has_media {
            if is_error {
                if let Some(text_part) = translated_parts
                    .iter_mut()
                    .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                {
                    let text = text_part
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    text_part["text"] = json!(format!("Tool error: {text}"));
                } else {
                    translated_parts.insert(0, json!({"type": "text", "text": "Tool error:"}));
                }
            }
            return Value::Array(translated_parts);
        }
    }

    let mut result_text = value_to_text(content);
    if is_error {
        result_text = format!("Tool error: {result_text}");
    }
    Value::String(result_text)
}

fn translate_anthropic_base64_media(part: &Value) -> Option<Value> {
    let block_type = part.get("type").and_then(Value::as_str)?;
    let source = part.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }

    let media_type = source.get("media_type").and_then(Value::as_str)?;
    if block_type == "document" && media_type != "application/pdf" {
        return None;
    }
    if block_type != "image" && block_type != "document" {
        return None;
    }
    let data = source.get("data").and_then(Value::as_str)?;
    if data.is_empty() {
        return None;
    }

    Some(json!({
        "type": "image_url",
        "image_url": {
            "url": format!("data:{media_type};base64,{data}")
        }
    }))
}

fn flush_anthropic_user_parts(messages: &mut Vec<Value>, parts: &mut Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": Value::Array(std::mem::take(parts))
    }));
}

fn translate_anthropic_tool(tool: &Value) -> Option<Value> {
    let name = tool.get("name")?.as_str()?;
    let mut function = Map::new();
    function.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    let mut schema = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    sanitize_json_schema(&mut schema);
    function.insert("parameters".to_string(), schema);
    Some(json!({"type": "function", "function": function}))
}

fn sanitize_json_schema(schema: &mut Value) {
    let Value::Object(object) = schema else {
        return;
    };

    object.remove("$schema");
    object.remove("$id");
    object.remove("$comment");
    if object.contains_key("properties") && !object.contains_key("type") {
        object.insert("type".to_string(), json!("object"));
    }

    for key in ["properties", "$defs", "definitions"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                sanitize_json_schema(child);
            }
        }
    }
    if let Some(items) = object.get_mut("items") {
        match items {
            Value::Array(items) => {
                for item in items {
                    sanitize_json_schema(item);
                }
            }
            item => sanitize_json_schema(item),
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                sanitize_json_schema(child);
            }
        }
    }
}

fn translate_anthropic_tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"type": "function", "function": {"name": name}})),
        _ => None,
    }
}

fn translate_anthropic_response(
    upstream: &Value,
    model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    if let Some(block_reason) = upstream
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        let choices_are_empty = upstream
            .get("choices")
            .and_then(Value::as_array)
            .map(Vec::is_empty)
            .unwrap_or(true);
        if choices_are_empty {
            let usage = upstream.get("usage").cloned().unwrap_or_else(|| json!({}));
            let input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            return Ok(json!({
                "id": format!("msg_{}", Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Gemini Safety Intercept: Request was blocked by safety guardrails (Reason: {block_reason})."
                    )
                }],
                "stop_reason": "refusal",
                "stop_sequence": Value::Null,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens
                }
            }));
        }
    }

    let upstream_message = upstream.pointer("/choices/0/message").ok_or_else(|| {
        format!(
            "Missing choices[0].message: {}",
            safe_error_message(upstream)
        )
    })?;
    let finish_reason = upstream
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str);
    let allow_tool_calls = anthropic_stop_reason(finish_reason, true) == "tool_use";
    let mut content = Vec::new();
    let reasoning_text = upstream_message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            upstream_message
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        });
    if let Some(reasoning_text) = reasoning_text {
        content.push(json!({"type": "thinking", "thinking": reasoning_text}));
    }
    let text = value_to_text(upstream_message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }

    if allow_tool_calls {
        let tool_calls = upstream_message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for tool_call in tool_calls {
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_function");
            let arguments = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = parse_tool_arguments(arguments)?;

            if let Some(signature) = tool_call
                .pointer("/extra_content/google/thought_signature")
                .and_then(Value::as_str)
            {
                remember_thought_signature(thought_signatures, &call_id, signature);
            }

            content.push(json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input
            }));
        }
    }

    let has_tools = content
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
    let stop_reason = anthropic_stop_reason(finish_reason, has_tools);
    let usage = upstream.get("usage").cloned().unwrap_or_else(|| json!({}));
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Ok(json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens
        }
    }))
}

fn push_anthropic_event(
    events: &mut Vec<Event>,
    event_type: &str,
    body: Value,
) -> Result<(), String> {
    let event = Event::default()
        .event(event_type)
        .json_data(body)
        .map_err(|err| err.to_string())?;
    events.push(event);
    Ok(())
}

fn translate_request(
    request: &Value,
    default_model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    // This translates the legacy Codex Responses shape. It is deliberately
    // buffered/non-streaming because Codex is not a supported bridge target;
    // Claude Code streaming is implemented in translate_anthropic_request.
    let mut messages = Vec::new();

    if let Some(instructions) = request.get("instructions").and_then(Value::as_str) {
        if !instructions.is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }

    match request.get("input") {
        Some(Value::String(text)) => messages.push(json!({"role": "user", "content": text})),
        Some(Value::Array(items)) => {
            translate_input_items(items, &mut messages, thought_signatures)
        }
        Some(_) => return Err("'input' must be a string or an array".to_string()),
        None => return Err("Missing required field 'input'".to_string()),
    }

    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        json!(request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(default_model)),
    );
    body.insert("messages".to_string(), Value::Array(messages));
    body.insert("stream".to_string(), Value::Bool(false));

    if let Some(max_tokens) = request.get("max_output_tokens").and_then(Value::as_u64) {
        body.insert("max_tokens".to_string(), json!(max_tokens));
    }
    if let Some(parallel) = request.get("parallel_tool_calls").and_then(Value::as_bool) {
        body.insert("parallel_tool_calls".to_string(), json!(parallel));
    }
    if let Some(effort) = request
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
    {
        if matches!(effort, "low" | "medium" | "high") {
            body.insert("reasoning_effort".to_string(), json!(effort));
        }
    }

    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        let translated_tools: Vec<Value> = tools.iter().filter_map(translate_tool).collect();
        if !translated_tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(translated_tools));
        }
    }

    if let Some(tool_choice) = request.get("tool_choice") {
        if let Some(translated) = translate_tool_choice(tool_choice) {
            body.insert("tool_choice".to_string(), translated);
        }
    }

    Ok(Value::Object(body))
}

fn translate_input_items(
    items: &[Value],
    messages: &mut Vec<Value>,
    thought_signatures: &ThoughtSignatureCache,
) {
    let mut pending_tool_calls = Vec::new();

    let flush_tool_calls = |messages: &mut Vec<Value>, pending: &mut Vec<Value>| {
        if !pending.is_empty() {
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": std::mem::take(pending)
            }));
        }
    };

    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown");
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_function");
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let mut translated_call = json!({
                    "id": call_id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                });
                if let Some(signature) = thought_signatures
                    .read()
                    .ok()
                    .and_then(|cache| cache.get(call_id).cloned())
                {
                    translated_call["extra_content"] = json!({
                        "google": {"thought_signature": signature}
                    });
                }
                pending_tool_calls.push(translated_call);
            }
            Some("function_call_output") => {
                flush_tool_calls(messages, &mut pending_tool_calls);
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown");
                let output = value_to_text(item.get("output").unwrap_or(&Value::Null));
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output
                }));
            }
            Some("reasoning") => {}
            _ => {
                flush_tool_calls(messages, &mut pending_tool_calls);
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .map(|role| if role == "developer" { "system" } else { role })
                    .unwrap_or("user");
                let content = value_to_text(item.get("content").unwrap_or(&Value::Null));
                if !content.is_empty() {
                    messages.push(json!({"role": role, "content": content}));
                }
            }
        }
    }

    flush_tool_calls(messages, &mut pending_tool_calls);
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn translate_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }

    let name = tool.get("name")?.as_str()?;
    let mut function = Map::new();
    function.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    function.insert(
        "parameters".to_string(),
        tool.get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    );

    Some(json!({"type": "function", "function": function}))
}

fn translate_tool_choice(choice: &Value) -> Option<Value> {
    if let Some(text) = choice.as_str() {
        return match text {
            "auto" | "none" | "required" => Some(json!(text)),
            _ => None,
        };
    }

    let name = choice.get("name").and_then(Value::as_str)?;
    Some(json!({"type": "function", "function": {"name": name}}))
}

fn translate_response_events(
    request: &Value,
    upstream: &Value,
    default_model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Vec<Event>, String> {
    let message = upstream.pointer("/choices/0/message").ok_or_else(|| {
        format!(
            "Missing choices[0].message: {}",
            safe_error_message(upstream)
        )
    })?;
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created_at = unix_timestamp();
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(default_model);
    let mut output = Vec::new();
    let mut event_values = Vec::new();
    let mut sequence = 0_u64;

    let created_response = response_object(
        &response_id,
        created_at,
        model,
        "in_progress",
        Vec::new(),
        request,
        upstream,
    );
    push_event(
        &mut event_values,
        &mut sequence,
        "response.created",
        json!({"response": created_response}),
    );

    let content = value_to_text(message.get("content").unwrap_or(&Value::Null));
    if !content.is_empty() {
        let output_index = output.len();
        let item_id = format!("msg_{}", Uuid::new_v4().simple());
        let in_progress_item = json!({
            "id": item_id,
            "type": "message",
            "status": "in_progress",
            "role": "assistant",
            "content": []
        });
        push_event(
            &mut event_values,
            &mut sequence,
            "response.output_item.added",
            json!({"output_index": output_index, "item": in_progress_item}),
        );
        push_event(
            &mut event_values,
            &mut sequence,
            "response.content_part.added",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []}
            }),
        );
        push_event(
            &mut event_values,
            &mut sequence,
            "response.output_text.delta",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "delta": content,
                "logprobs": []
            }),
        );
        push_event(
            &mut event_values,
            &mut sequence,
            "response.output_text.done",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": content,
                "logprobs": []
            }),
        );
        let part = json!({"type": "output_text", "text": content, "annotations": []});
        push_event(
            &mut event_values,
            &mut sequence,
            "response.content_part.done",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": part
            }),
        );
        let completed_item = json!({
            "id": item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [part]
        });
        push_event(
            &mut event_values,
            &mut sequence,
            "response.output_item.done",
            json!({"output_index": output_index, "item": completed_item}),
        );
        output.push(completed_item);
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let output_index = output.len();
            let item_id = format!("fc_{}", Uuid::new_v4().simple());
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_function");
            let arguments = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            if let Some(signature) = tool_call
                .pointer("/extra_content/google/thought_signature")
                .and_then(Value::as_str)
            {
                remember_thought_signature(thought_signatures, &call_id, signature);
            }

            let in_progress_item = json!({
                "id": item_id,
                "type": "function_call",
                "status": "in_progress",
                "call_id": call_id,
                "name": name,
                "arguments": ""
            });
            push_event(
                &mut event_values,
                &mut sequence,
                "response.output_item.added",
                json!({"output_index": output_index, "item": in_progress_item}),
            );
            push_event(
                &mut event_values,
                &mut sequence,
                "response.function_call_arguments.delta",
                json!({
                    "item_id": item_id,
                    "output_index": output_index,
                    "delta": arguments
                }),
            );
            push_event(
                &mut event_values,
                &mut sequence,
                "response.function_call_arguments.done",
                json!({
                    "item_id": item_id,
                    "output_index": output_index,
                    "arguments": arguments
                }),
            );
            let completed_item = json!({
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments
            });
            push_event(
                &mut event_values,
                &mut sequence,
                "response.output_item.done",
                json!({"output_index": output_index, "item": completed_item}),
            );
            output.push(completed_item);
        }
    }

    let completed_response = response_object(
        &response_id,
        created_at,
        model,
        "completed",
        output,
        request,
        upstream,
    );
    push_event(
        &mut event_values,
        &mut sequence,
        "response.completed",
        json!({"response": completed_response}),
    );

    event_values
        .into_iter()
        .map(|(event_type, value)| {
            Event::default()
                .event(event_type)
                .json_data(value)
                .map_err(|err| err.to_string())
        })
        .collect()
}

fn push_event(
    events: &mut Vec<(&'static str, Value)>,
    sequence: &mut u64,
    event_type: &'static str,
    mut body: Value,
) {
    if let Some(object) = body.as_object_mut() {
        object.insert("type".to_string(), json!(event_type));
        object.insert("sequence_number".to_string(), json!(*sequence));
    }
    *sequence += 1;
    events.push((event_type, body));
}

fn response_object(
    id: &str,
    created_at: u64,
    model: &str,
    status: &str,
    output: Vec<Value>,
    request: &Value,
    upstream: &Value,
) -> Value {
    let usage = upstream.get("usage").cloned().unwrap_or_else(|| json!({}));
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    json!({
        "id": id,
        "object": "response",
        "created_at": created_at,
        "status": status,
        "background": false,
        "error": Value::Null,
        "incomplete_details": Value::Null,
        "instructions": request.get("instructions").cloned().unwrap_or(Value::Null),
        "max_output_tokens": request.get("max_output_tokens").cloned().unwrap_or(Value::Null),
        "model": model,
        "output": output,
        "parallel_tool_calls": request.get("parallel_tool_calls").cloned().unwrap_or(json!(true)),
        "previous_response_id": Value::Null,
        "reasoning": request.get("reasoning").cloned().unwrap_or(Value::Null),
        "store": false,
        "temperature": Value::Null,
        "text": request.get("text").cloned().unwrap_or_else(|| json!({"format": {"type": "text"}})),
        "tool_choice": request.get("tool_choice").cloned().unwrap_or(json!("auto")),
        "tools": request.get("tools").cloned().unwrap_or_else(|| json!([])),
        "top_p": Value::Null,
        "truncation": request.get("truncation").cloned().unwrap_or(json!("disabled")),
        "usage": {
            "input_tokens": input_tokens,
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": output_tokens,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": input_tokens + output_tokens
        }
    })
}

fn safe_error_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value
                .pointer("/promptFeedback/blockReason")
                .and_then(Value::as_str)
                .map(|reason| {
                    format!(
                        "Gemini Safety Intercept: Request was blocked by safety guardrails (Reason: {reason})."
                    )
                })
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Unknown upstream response".to_string())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_empty_provider_profile_directory() {
        let settings_dir =
            env::temp_dir().join(format!("claude-bridge-empty-profiles-{}", Uuid::new_v4()));
        fs::create_dir_all(&settings_dir).unwrap();
        let result = load_provider_profiles(&settings_dir, "http://127.0.0.1:18787");
        fs::remove_dir(&settings_dir).unwrap();

        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn persists_and_loads_gemini_proxy_modes() {
        let state_path =
            env::temp_dir().join(format!("claude-bridge-proxy-state-{}.json", Uuid::new_v4()));

        persist_bridge_state(&state_path, "settings - test.json", None).unwrap();
        assert_eq!(load_persisted_gemini_proxy(&state_path), Some(None));

        persist_bridge_state(
            &state_path,
            "settings - test.json",
            Some("http://127.0.0.1:8080"),
        )
        .unwrap();
        assert_eq!(
            load_persisted_gemini_proxy(&state_path),
            Some(Some("http://127.0.0.1:8080".to_string()))
        );

        fs::remove_file(state_path).unwrap();
    }

    #[test]
    fn translates_responses_tools_and_outputs() {
        let request = json!({
            "model": "gemini-3.6-flash",
            "instructions": "Be concise.",
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "List files"}]},
                {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"}
            ],
            "tools": [{
                "type": "function",
                "name": "shell",
                "description": "Run a command",
                "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}
            }],
            "reasoning": {"effort": "high"}
        });

        let signatures = RwLock::new(IndexMap::new());
        let translated = translate_request(&request, "fallback", &signatures).unwrap();
        assert_eq!(translated["model"], "gemini-3.6-flash");
        assert_eq!(translated["messages"][0]["role"], "system");
        assert_eq!(translated["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(translated["messages"][3]["role"], "tool");
        assert_eq!(translated["tools"][0]["function"]["name"], "shell");
        assert_eq!(translated["reasoning_effort"], "high");
    }

    #[test]
    fn produces_completed_response_event() {
        let request = json!({"model": "gemini-3.6-flash", "input": "hello"});
        let upstream = json!({
            "choices": [{"message": {"role": "assistant", "content": "OK"}}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1}
        });

        let signatures = RwLock::new(IndexMap::new());
        let events =
            translate_response_events(&request, &upstream, "gemini-3.6-flash", &signatures)
                .unwrap();
        assert!(!events.is_empty());
    }

    #[test]
    fn preserves_gemini_thought_signature_for_tool_roundtrip() {
        let signatures = RwLock::new(IndexMap::new());
        let first_request = json!({"model": "gemini-3.6-flash", "input": "run tool"});
        let upstream = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
                        "extra_content": {
                            "google": {"thought_signature": "encrypted-signature"}
                        }
                    }]
                }
            }]
        });

        translate_response_events(&first_request, &upstream, "gemini-3.6-flash", &signatures)
            .unwrap();

        let second_request = json!({
            "model": "gemini-3.6-flash",
            "input": [
                {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"}
            ]
        });
        let translated =
            translate_request(&second_request, "gemini-3.6-flash", &signatures).unwrap();
        assert_eq!(
            translated["messages"][0]["tool_calls"][0]["extra_content"]["google"]
                ["thought_signature"],
            "encrypted-signature"
        );
    }

    #[test]
    fn translates_anthropic_messages_and_tools() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "system": [{"type": "text", "text": "You are a coding agent."}],
            "max_tokens": 4096,
            "stream": true,
            "messages": [
                {"role": "system", "content": "Runtime system context."},
                {"role": "user", "content": [{"type": "text", "text": "List files"}]},
                {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "shell",
                    "input": {"cmd": "dir"}
                }]},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "a.txt"
                }]}
            ],
            "tools": [{
                "name": "shell",
                "description": "Run a command",
                "input_schema": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"]
                }
            }],
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "adaptive"}
        });

        let signatures = RwLock::new(IndexMap::new());
        let translated =
            translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
        assert_eq!(translated["model"], "gemini-3.6-flash");
        assert_eq!(translated["stream"], true);
        assert_eq!(translated["stream_options"]["include_usage"], true);
        assert_eq!(translated["messages"][0]["role"], "system");
        assert_eq!(translated["messages"][1]["role"], "system");
        assert_eq!(
            translated["messages"][3]["tool_calls"][0]["function"]["name"],
            "shell"
        );
        assert_eq!(translated["messages"][4]["role"], "tool");
        assert_eq!(
            translated["tools"][0]["function"]["parameters"]["type"],
            "object"
        );
        assert_eq!(translated["tool_choice"], "auto");
        assert_eq!(translated["reasoning_effort"], "high");
    }

    #[test]
    fn sanitizes_anthropic_tool_json_schema_recursively() {
        let tool = json!({
            "name": "inspect",
            "input_schema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$id": "root",
                "$comment": "unsupported",
                "properties": {
                    "target": {
                        "$comment": "nested",
                        "properties": {
                            "path": {"type": "string", "$id": "path"}
                        }
                    },
                    "options": {
                        "oneOf": [
                            {"type": "string", "$schema": "nested"},
                            {"items": {"type": "integer", "$comment": "item"}}
                        ]
                    }
                },
                "$defs": {
                    "metadata": {"type": "string", "$comment": "definition"}
                }
            }
        });

        let translated = translate_anthropic_tool(&tool).unwrap();
        let schema = &translated["function"]["parameters"];

        assert_eq!(schema["type"], "object");
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("$id").is_none());
        assert!(schema.get("$comment").is_none());
        assert_eq!(schema["properties"]["target"]["type"], "object");
        assert!(schema["properties"]["target"].get("$comment").is_none());
        assert!(schema["properties"]["target"]["properties"]["path"]
            .get("$id")
            .is_none());
        assert!(schema["properties"]["options"]["oneOf"][0]
            .get("$schema")
            .is_none());
        assert!(schema["properties"]["options"]["oneOf"][1]["items"]
            .get("$comment")
            .is_none());
        assert!(schema["$defs"]["metadata"].get("$comment").is_none());
    }

    #[test]
    fn translates_multimodal_tool_results_and_pdf_user_documents() {
        let request = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_media",
                        "content": [
                            {"type": "text", "text": "Screenshot captured"},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "aW1hZ2U="
                                }
                            }
                        ]
                    },
                    {
                        "type": "document",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": "cGRm"
                        }
                    }
                ]
            }]
        });

        let signatures = RwLock::new(IndexMap::new());
        let translated =
            translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
        let messages = translated["messages"].as_array().unwrap();

        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["content"][0]["text"], "Screenshot captured");
        assert_eq!(
            messages[0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,aW1hZ2U="
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(
            messages[1]["content"][0]["image_url"]["url"],
            "data:application/pdf;base64,cGRm"
        );
    }

    #[test]
    fn produces_anthropic_tool_use_and_stream_events() {
        let upstream = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "I will inspect it.",
                    "tool_calls": [{
                        "id": "toolu_1",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"cmd\":\"dir\"}"
                        },
                        "extra_content": {
                            "google": {"thought_signature": "signature-1"}
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 4}
        });

        let signatures = RwLock::new(IndexMap::new());
        let message =
            translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();
        assert_eq!(message["stop_reason"], "tool_use");
        assert_eq!(message["content"][1]["type"], "tool_use");
        assert_eq!(message["content"][1]["input"]["cmd"], "dir");
        assert_eq!(
            signatures
                .read()
                .unwrap()
                .get("toolu_1")
                .map(String::as_str),
            Some("signature-1")
        );
    }

    #[test]
    fn emits_mixed_anthropic_tool_results_before_user_content() {
        let request = json!({
            "model": "gemini-3.6-flash",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "first_tool",
                            "input": {}
                        },
                        {
                            "type": "tool_use",
                            "id": "toolu_2",
                            "name": "second_tool",
                            "input": {}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Text before results"},
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": "first result"
                        },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "AA=="
                            }
                        },
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_2",
                            "content": "second result"
                        },
                        {"type": "text", "text": "Text after results"}
                    ]
                }
            ]
        });

        let signatures = RwLock::new(IndexMap::new());
        let translated =
            translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
        let messages = translated["messages"].as_array().unwrap();

        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "toolu_1");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "toolu_2");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[3]["content"][0]["text"], "Text before results");
        assert_eq!(messages[3]["content"][1]["type"], "image_url");
        assert_eq!(messages[3]["content"][2]["text"], "Text after results");
    }

    #[test]
    fn evicts_only_oldest_thought_signatures_at_capacity() {
        let signatures = RwLock::new(IndexMap::new());
        for index in 0..THOUGHT_SIGNATURE_CAPACITY {
            remember_thought_signature(
                &signatures,
                &format!("call_{index}"),
                &format!("signature_{index}"),
            );
        }

        remember_thought_signature(&signatures, "call_new", "signature_new");
        let cache = signatures.read().unwrap();
        assert_eq!(
            cache.len(),
            THOUGHT_SIGNATURE_CAPACITY - THOUGHT_SIGNATURE_EVICTION_BATCH + 1
        );
        assert!(!cache.contains_key("call_0"));
        assert!(!cache.contains_key("call_511"));
        assert!(cache.contains_key("call_512"));
        assert_eq!(
            cache.get("call_new").map(String::as_str),
            Some("signature_new")
        );
    }

    #[test]
    fn token_estimate_gives_non_ascii_text_more_headroom() {
        let request = json!({
            "messages": [{
                "role": "user",
                "content": "汉".repeat(100)
            }]
        });
        let serialized = request.to_string();
        let old_byte_quarter_estimate = serialized.len().div_ceil(4);
        let estimate = estimate_anthropic_input_tokens(&request);

        assert!(estimate > old_byte_quarter_estimate);
        assert!(estimate >= 150);
    }

    #[test]
    fn counts_serialized_json_bytes_without_allocating_a_full_copy() {
        let request = json!({
            "escaped": "quote=\" slash=\\ newline=\n",
            "unicode": "中文🙂",
            "values": [true, false, null, 123, -4.5]
        });
        let serialized = request.to_string();
        let expected_ascii = serialized.bytes().filter(u8::is_ascii).count();
        let expected_non_ascii = serialized.len() - expected_ascii;

        assert_eq!(
            count_serialized_json_bytes(&request),
            (expected_ascii, expected_non_ascii)
        );
    }

    #[test]
    fn decodes_sse_across_utf8_and_network_chunk_boundaries() {
        let frame =
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\r\n\r\ndata: [DONE]\n\n";
        let mut decoder = SseDataDecoder::default();
        let mut payloads = Vec::new();
        for byte in frame.as_bytes().chunks(1) {
            payloads.extend(decoder.push_bytes(byte).unwrap());
        }
        payloads.extend(decoder.finish().unwrap());

        assert_eq!(payloads.len(), 2);
        assert!(payloads[0].contains("你好"));
        assert_eq!(payloads[1], "[DONE]");
    }

    #[test]
    fn emits_text_delta_before_upstream_stream_finishes() {
        let signatures = Arc::new(RwLock::new(IndexMap::new()));
        let mut translator =
            AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures, 42);

        assert_eq!(translator.start_events().unwrap().len(), 1);
        assert_eq!(translator.input_tokens, 42);
        let first_delta = translator
            .process_payload(
                r#"{"choices":[{"delta":{"role":"assistant","content":"Hello "},"finish_reason":null}]}"#,
            )
            .unwrap();
        assert_eq!(first_delta.len(), 2);
        assert!(!translator.finished);

        let second_delta = translator
            .process_payload(
                r#"{"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            )
            .unwrap();
        assert_eq!(second_delta.len(), 1);
        assert_eq!(translator.input_tokens, 10);
        assert_eq!(translator.output_tokens, 2);
        assert_eq!(translator.finish().unwrap().len(), 3);
    }

    #[test]
    fn streams_thinking_blocks_and_closes_them_before_text() {
        let signatures = Arc::new(RwLock::new(IndexMap::new()));
        let mut translator =
            AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures, 0);

        let thinking_events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"reasoning_content":"Inspecting context"},"finish_reason":null}]}"#,
            )
            .unwrap();
        let thinking_debug = format!("{thinking_events:?}");
        assert_eq!(thinking_events.len(), 2);
        assert!(thinking_debug.contains("thinking"));
        assert!(thinking_debug.contains("thinking_delta"));
        assert!(thinking_debug.contains("Inspecting context"));
        assert_eq!(translator.thinking_block_index, Some(0));

        let text_events = translator
            .process_payload(r#"{"choices":[{"delta":{"content":"Done"},"finish_reason":"stop"}]}"#)
            .unwrap();
        let text_debug = format!("{text_events:?}");
        assert_eq!(text_events.len(), 3);
        assert!(text_debug.contains("content_block_stop"));
        assert!(text_debug.contains("text_delta"));
        assert_eq!(translator.thinking_block_index, None);
        assert_eq!(translator.text_block_index, Some(1));
    }

    #[test]
    fn prepends_non_streaming_thinking_content() {
        let upstream = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "I should inspect the repository.",
                    "content": "I found the issue."
                },
                "finish_reason": "stop"
            }]
        });
        let signatures = RwLock::new(IndexMap::new());

        let message =
            translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

        assert_eq!(message["content"][0]["type"], "thinking");
        assert_eq!(
            message["content"][0]["thinking"],
            "I should inspect the repository."
        );
        assert_eq!(message["content"][1]["type"], "text");
    }

    #[test]
    fn turns_prompt_feedback_into_anthropic_refusal() {
        let upstream = json!({
            "promptFeedback": {"blockReason": "SAFETY"},
            "choices": []
        });
        let signatures = RwLock::new(IndexMap::new());

        let message =
            translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

        assert_eq!(message["stop_reason"], "refusal");
        assert_eq!(
            message["content"][0]["text"],
            "Gemini Safety Intercept: Request was blocked by safety guardrails (Reason: SAFETY)."
        );
        assert!(safe_error_message(&upstream).contains("Reason: SAFETY"));
    }

    #[test]
    fn aggregates_streamed_tool_call_and_preserves_signature() {
        let signatures = Arc::new(RwLock::new(IndexMap::new()));
        let mut translator =
            AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures.clone(), 0);

        let events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"id":"toolu_stream_1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":"},"extra_content":{"google":{"thought_signature":"stream-signature"}}}]},"finish_reason":null}]}"#,
            )
            .unwrap();
        assert!(events.is_empty());
        translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"\"pwd\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

        let finish_events = translator.finish().unwrap();
        assert_eq!(finish_events.len(), 5);
        assert_eq!(
            signatures
                .read()
                .unwrap()
                .get("toolu_stream_1")
                .map(String::as_str),
            Some("stream-signature")
        );
    }

    #[test]
    fn max_tokens_suppresses_truncated_non_streaming_tool_call() {
        let upstream = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "toolu_truncated",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"cmd\":\"unterminated"
                        },
                        "extra_content": {
                            "google": {"thought_signature": "must-not-be-cached"}
                        }
                    }]
                },
                "finish_reason": "length"
            }]
        });
        let signatures = RwLock::new(IndexMap::new());

        let message =
            translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

        assert_eq!(message["stop_reason"], "max_tokens");
        assert_eq!(message["content"], json!([]));
        assert!(!signatures.read().unwrap().contains_key("toolu_truncated"));
    }

    #[test]
    fn max_tokens_suppresses_truncated_streaming_tool_call() {
        let signatures = Arc::new(RwLock::new(IndexMap::new()));
        let mut translator =
            AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures.clone(), 12);
        translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"id":"toolu_truncated","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"unterminated"},"extra_content":{"google":{"thought_signature":"must-not-be-cached"}}}]},"finish_reason":"length"}]}"#,
            )
            .unwrap();

        let events = translator.finish().unwrap();

        assert_eq!(events.len(), 2);
        assert!(!signatures.read().unwrap().contains_key("toolu_truncated"));
    }

    #[test]
    fn rejects_invalid_completed_tool_arguments() {
        let upstream = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "toolu_invalid",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"cmd\":\"unterminated"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let signatures = RwLock::new(IndexMap::new());

        let error =
            translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap_err();

        assert!(error.contains("invalid tool arguments JSON"));
    }

    #[test]
    fn maps_content_filter_to_refusal() {
        assert_eq!(
            anthropic_stop_reason(Some("content_filter"), false),
            "refusal"
        );
        assert_eq!(anthropic_stop_reason(Some("SAFETY"), true), "refusal");
        assert_eq!(anthropic_stop_reason(Some("length"), true), "max_tokens");
    }

    #[test]
    fn forwarding_prefers_bearer_and_supplies_default_version() {
        let client = Client::builder().build().unwrap();
        let profile = ProviderProfile {
            file_name: "test.json".to_string(),
            model: "test-model".to_string(),
            upstream_identity: None,
            identity_override: true,
            base_url: "https://example.invalid".to_string(),
            auth_token: Some("bearer-secret".to_string()),
            api_key: Some("api-key-secret".to_string()),
            proxy_url: None,
            local_gemini: false,
            client: client.clone(),
        };
        let request = apply_anthropic_forward_headers(
            client.post("https://example.invalid/v1/messages"),
            &profile,
            &HeaderMap::new(),
        )
        .build()
        .unwrap();

        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer bearer-secret")
        );
        assert!(!request.headers().contains_key("x-api-key"));
        assert_eq!(
            request
                .headers()
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some(DEFAULT_ANTHROPIC_VERSION)
        );
        assert!(!request.headers().contains_key("anthropic-beta"));
    }

    #[test]
    fn neutralizes_claude_identity_and_appends_runtime_identity() {
        let original = "You are Claude Code, Anthropic's official CLI for Claude.";
        let mut request = json!({
            "system": format!("<system-reminder>Runtime context.</system-reminder>\n{original}"),
            "messages": [{"role": "user", "content": "Who are you?"}]
        });

        assert!(append_bridge_identity(&mut request, "Google Gemini (gemini-3.6-flash)").unwrap());
        assert!(!append_bridge_identity(&mut request, "Google Gemini (gemini-3.6-flash)").unwrap());

        let system = request["system"].as_str().unwrap();
        assert!(system.contains(
            "You are \"Google Gemini (gemini-3.6-flash)\", the upstream model serving this route, operating inside the Claude Code client."
        ));
        assert!(!system.contains(original));
        assert_eq!(system.matches(BRIDGE_IDENTITY_MARKER).count(), 1);
        assert!(system.contains(
            "actual model and first-person assistant identity is \"Google Gemini (gemini-3.6-flash)\""
        ));
        assert!(system.contains("answer truthfully based on what you actually are"));
        assert!(system.contains("请根据你的真实身份如实回答"));

        let signatures = RwLock::new(IndexMap::new());
        let translated =
            translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
        let translated_system = translated["messages"][0]["content"].as_str().unwrap();
        assert!(translated_system.contains(
            "You are \"Google Gemini (gemini-3.6-flash)\", the upstream model serving this route, operating inside the Claude Code client."
        ));
        assert!(translated_system.contains(BRIDGE_IDENTITY_MARKER));
        let translated_messages = translated["messages"].as_array().unwrap();
        let reminder = translated_messages.last().unwrap();
        assert_eq!(reminder["role"], "system");
        assert!(reminder["content"]
            .as_str()
            .unwrap()
            .starts_with(BRIDGE_IDENTITY_MARKER));
    }

    #[test]
    fn preserves_system_blocks_and_cache_control_when_appending_identity() {
        let mut request = json!({
            "system": [{
                "type": "text",
                "text": "You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.\nCached Claude Code instructions.",
                "cache_control": {"type": "ephemeral"}
            }, {
                "type": "text",
                "text": "You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers."
            }],
            "messages": []
        });

        append_bridge_identity(&mut request, "DeepSeek V3").unwrap();
        let blocks = request["system"].as_array().unwrap();

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
        assert!(blocks[0]["text"].as_str().unwrap().starts_with(
            "You are \"DeepSeek V3\", the upstream model serving this route, operating inside the Claude Code client."
        ));
        assert!(blocks[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("Cached Claude Code instructions."));
        assert!(blocks[1]["text"].as_str().unwrap().starts_with(
            "You are \"DeepSeek V3\", the upstream model serving this route. You operate inside the Claude Code client."
        ));
        assert!(blocks[2]["text"]
            .as_str()
            .unwrap()
            .contains("actual model and first-person assistant identity is \"DeepSeek V3\""));
    }

    #[test]
    fn keeps_claude_identity_and_unrelated_claude_code_references_unchanged() {
        assert!(is_claude_identity("Anthropic Claude Sonnet 4"));
        assert!(!is_claude_identity("qwen3.8-max"));

        let claude_declaration =
            "You are Claude Code, Anthropic's official CLI for Claude.".to_string();
        let mut actual_claude = claude_declaration.clone();
        assert!(!neutralize_claude_identity_declaration(
            &mut actual_claude,
            "Anthropic Claude Sonnet 4"
        ));
        assert_eq!(actual_claude, claude_declaration);

        let mut unrelated = "Use Claude Code tools exactly as documented.".to_string();
        assert!(!neutralize_claude_identity_declaration(
            &mut unrelated,
            "Google Gemini"
        ));
        assert_eq!(unrelated, "Use Claude Code tools exactly as documented.");
    }

    #[test]
    fn neutralizes_subagent_personas_environment_and_attribution() {
        let mut text = concat!(
            "You are a file search specialist for Claude Code, Anthropic's official CLI for Claude.\n",
            "You are an agent for Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.\n",
            "# Environment\n",
            "You are powered by the model named Claude Sonnet 4.5. The exact model ID is claude-sonnet-4-5-20250929.\n",
            "End git commit messages with: Co-Authored-By: Claude <noreply@anthropic.com>"
        )
        .to_string();

        assert!(neutralize_claude_identity_declaration(
            &mut text,
            "qwen3.8-max"
        ));
        assert!(text.contains(
            "You are a file search specialist for \"qwen3.8-max\", the upstream model serving this route, operating inside the Claude Code client."
        ));
        assert!(text.contains(
            "You are an agent for \"qwen3.8-max\", the upstream model serving this route, operating inside the Claude Code client."
        ));
        assert!(text.contains("You are powered by qwen3.8-max."));
        assert!(!text.contains("claude-sonnet-4-5"));
        assert!(text.contains("Co-Authored-By: qwen3.8-max <noreply@anthropic.com>"));
        assert!(!text.contains("Anthropic's official CLI"));
        assert!(text.contains("# Environment"));
    }

    #[test]
    fn neutralizes_reworded_persona_sentences() {
        let mut text = concat!(
            "You are Claude Code, a helpful coding assistant built by Anthropic.\n",
            "You are a Claude agent that helps with programming tasks.\n",
            "Claude Code is available as a CLI in the terminal.\n",
            "Other instructions stay."
        )
        .to_string();

        assert!(neutralize_claude_identity_declaration(
            &mut text,
            "DeepSeek V3"
        ));
        assert_eq!(
            text.matches(
                "You are \"DeepSeek V3\", the upstream model serving this route. You operate inside the Claude Code client. Answer questions about your identity truthfully based on what you actually are."
            )
            .count(),
            2
        );
        // Factual capability sentences must survive the rewrite.
        assert!(text.contains(
            "Claude Code is available as a CLI in the terminal.\nOther instructions stay."
        ));
        assert!(!text.contains("You are Claude Code"));
        assert!(!text.contains("You are a Claude agent"));
    }

    #[test]
    fn rewrites_powered_by_line_with_model_names_containing_periods() {
        let mut text = concat!(
            "# Environment\n",
            "You are powered by the model named Claude Sonnet 4.5. The exact model ID is claude-sonnet-4-5-20250929.\n",
            "Today's date is 2026-08-06."
        )
        .to_string();

        assert!(neutralize_powered_by_line(&mut text, "kimi-k3"));
        assert!(text.contains("You are powered by kimi-k3."));
        assert!(!text.contains("Claude Sonnet 4.5"));
        assert!(!text.contains("exact model ID"));
        assert!(text.contains("Today's date is 2026-08-06."));
    }

    #[test]
    fn strips_bracket_suffix_from_model_identity_fallback() {
        let client = Client::builder().build().unwrap();
        let profile = ProviderProfile {
            file_name: "settings - ds4.json".to_string(),
            model: "deepseek-v4-pro[1m]".to_string(),
            upstream_identity: None,
            identity_override: true,
            base_url: "https://example.invalid".to_string(),
            auth_token: None,
            api_key: Some("secret".to_string()),
            proxy_url: None,
            local_gemini: false,
            client,
        };

        assert_eq!(
            upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn passes_identity_questions_through_to_the_upstream_model() {
        // The bridge no longer answers identity questions itself. Every
        // phrasing must reach the upstream model verbatim; only the system
        // prompt is adapted.
        let questions = [
            "who are you?",
            "What model are you?",
            "are you Claude?",
            "你是谁？",
            "你是？",
            "您是？",
            "你是什么模型？",
            "妳系边个？",
        ];
        for question in questions {
            let mut request = json!({
                "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                "messages": [{"role": "user", "content": question}]
            });
            let original_messages = request["messages"].clone();

            append_bridge_identity(&mut request, "qwen3.8-max").unwrap();

            assert_eq!(
                request["messages"], original_messages,
                "user message must pass through unchanged: {question}"
            );
            let system = request["system"].as_str().unwrap();
            assert!(!system.contains("Anthropic's official CLI"));
            assert!(system.contains("\"qwen3.8-max\""));
        }
    }

    #[test]
    fn creates_system_prompt_when_identity_is_the_only_system_instruction() {
        let mut request = json!({"messages": []});

        append_bridge_identity(&mut request, "Kimi K2").unwrap();

        assert!(request["system"]
            .as_str()
            .unwrap()
            .contains("actual model and first-person assistant identity is \"Kimi K2\""));
    }

    #[test]
    fn selects_profile_identity_and_honors_disable_switch() {
        let client = Client::builder().build().unwrap();
        let mut profile = ProviderProfile {
            file_name: "settings - deepseek.json".to_string(),
            model: "deepseek-chat".to_string(),
            upstream_identity: Some("  DeepSeek\nV3  ".to_string()),
            identity_override: true,
            base_url: "https://example.invalid".to_string(),
            auth_token: Some("secret".to_string()),
            api_key: None,
            proxy_url: None,
            local_gemini: false,
            client,
        };

        assert_eq!(
            upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
            Some("DeepSeek V3")
        );
        profile.upstream_identity = None;
        profile.local_gemini = true;
        assert_eq!(
            upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
            Some("Google Gemini (gemini-3.6-flash)")
        );
        profile.identity_override = false;
        assert_eq!(upstream_identity_label(&profile, "gemini-3.6-flash"), None);
    }
}
