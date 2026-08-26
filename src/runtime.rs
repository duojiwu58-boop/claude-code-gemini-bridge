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
    let local_auth_token = resolve_local_auth_token()?;
    let listen = env::var("GEMINI_BRIDGE_LISTEN").unwrap_or_else(|_| "127.0.0.1:18787".to_string());
    let address = validate_loopback_listen(&listen)?;
    let upstream_url = env::var("GEMINI_BRIDGE_UPSTREAM").unwrap_or_else(|_| {
        "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions".to_string()
    });
    let model =
        env::var("GEMINI_BRIDGE_MODEL").unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.to_string());
    let settings_dir = env::var("CLAUDE_SETTINGS_DIR")
        .map(PathBuf::from)
        .or_else(|_| env::var("USERPROFILE").map(|profile| PathBuf::from(profile).join(".claude")))
        .map_err(|_| "CLAUDE_SETTINGS_DIR or USERPROFILE is required".to_string())?;
    let providers_dir = env::var("CLAUDE_BRIDGE_PROVIDERS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| settings_dir.join("bridge-providers"));
    fs::create_dir_all(&providers_dir).map_err(|err| {
        format!(
            "Cannot create provider configuration directory '{}': {err}",
            providers_dir.display()
        )
    })?;
    let bridge_state_path = env::var("GEMINI_BRIDGE_STATE_FILE")
        .map(PathBuf::from)
        .or_else(|_| {
            env::current_dir()
                .map(|path| path.join("bridge-state.json"))
                .map_err(|_| env::VarError::NotPresent)
        })
        .map_err(|_| "Cannot resolve bridge state file path".to_string())?;
    let image_output_dir = env::var("GEMINI_BRIDGE_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            settings_dir
                .parent()
                .unwrap_or(&settings_dir)
                .join("Pictures")
                .join("ClaudeCodeBridge")
        });
    fs::create_dir_all(&image_output_dir).map_err(|err| {
        format!(
            "Cannot create generated image directory '{}': {err}",
            image_output_dir.display()
        )
    })?;
    let image_model =
        env::var("GEMINI_BRIDGE_IMAGE_MODEL").unwrap_or_else(|_| DEFAULT_IMAGE_MODEL.to_string());
    let image_upstream_url = env::var("GEMINI_BRIDGE_IMAGE_UPSTREAM")
        .unwrap_or_else(|_| DEFAULT_IMAGE_UPSTREAM.to_string());
    let local_bridge_base_url = format!("http://{listen}");
    let loaded_profiles =
        load_provider_profiles(&providers_dir, &settings_dir, &local_bridge_base_url)
            .map_err(|err| format!("Cannot load provider profiles: {err}"))?;
    let active_profile = select_initial_profile(&loaded_profiles.profiles, &bridge_state_path);
    let proxy_url = load_persisted_gemini_proxy(&bridge_state_path).unwrap_or_else(|| {
        env::var("GEMINI_BRIDGE_PROXY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let gemini_client = build_gemini_client(proxy_url.as_deref(), None)?;
    let gemini_thinking_level = load_persisted_gemini_thinking_level(&bridge_state_path);
    let interaction_continuations = load_interaction_continuation_cache(
        &interaction_continuation_state_path(&bridge_state_path),
    );

    let state = Arc::new(AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client: gemini_client,
            proxy_url,
        })),
        gemini_thinking_level: Arc::new(RwLock::new(gemini_thinking_level)),
        fallback_api_key,
        local_auth_token,
        upstream_url,
        model,
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(interaction_continuations)),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: loaded_profiles.profiles,
            active_file: active_profile,
            source: loaded_profiles.source,
        })),
        shutdown_tx,
        settings_dir,
        providers_dir,
        bridge_state_path,
        image_output_dir,
        image_model,
        image_upstream_url,
        local_bridge_base_url,
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    });

    let protected_routes = Router::new()
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        .route("/mcp", post(mcp))
        .route("/admin/status", get(admin_status))
        .route("/admin/profiles", get(admin_profiles))
        .route("/admin/active-profile", post(admin_set_active_profile))
        .route("/admin/reload-profiles", post(admin_reload_profiles))
        .route("/admin/gemini-proxy", post(admin_set_gemini_proxy))
        .route("/admin/gemini-proxy/test", post(admin_test_gemini_proxy))
        .route(
            "/admin/gemini-thinking-level",
            post(admin_set_gemini_thinking_level),
        )
        .route("/admin/shutdown", post(admin_shutdown))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_local_auth,
        ));

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .merge(protected_routes)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

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
        env::var("GEMINI_BRIDGE_MODEL").unwrap_or_else(|_| DEFAULT_GEMINI_MODEL.into())
    );

    let (graceful_tx, graceful_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = graceful_rx.await;
        }),
    );
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.map_err(|err| format!("Server failed: {err}")),
        _ = shutdown_signal(shutdown_rx, include_ctrl_c) => {
            let _ = graceful_tx.send(());
            finish_server_with_timeout(server.as_mut(), GRACEFUL_SHUTDOWN_TIMEOUT).await
        }
    }
}

async fn finish_server_with_timeout<F, E>(
    server: std::pin::Pin<&mut F>,
    timeout: Duration,
) -> Result<(), String>
where
    F: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout, server).await {
        Ok(result) => result.map_err(|err| format!("Server failed: {err}")),
        Err(_) => Err(format!(
            "Server graceful shutdown timed out after {} seconds",
            timeout.as_secs()
        )),
    }
}

fn validate_loopback_listen(listen: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = listen
        .parse()
        .map_err(|err| format!("Invalid GEMINI_BRIDGE_LISTEN '{listen}': {err}"))?;
    if !address.ip().is_loopback() {
        return Err(format!(
            "GEMINI_BRIDGE_LISTEN must use a loopback address; refusing non-local address '{address}'"
        ));
    }
    Ok(address)
}

fn resolve_local_auth_token() -> Result<String, String> {
    if let Some(token) = env::var("GEMINI_BRIDGE_LOCAL_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return validate_local_auth_token(token, "GEMINI_BRIDGE_LOCAL_TOKEN");
    }

    let token_path = env::var("GEMINI_BRIDGE_LOCAL_TOKEN_FILE").map_err(|_| {
        "GEMINI_BRIDGE_LOCAL_TOKEN or GEMINI_BRIDGE_LOCAL_TOKEN_FILE is required".to_string()
    })?;
    let token = fs::read_to_string(&token_path)
        .map_err(|err| format!("Cannot read local auth token file '{token_path}': {err}"))?;
    validate_local_auth_token(token.trim().to_string(), &token_path)
}

fn validate_local_auth_token(token: String, source: &str) -> Result<String, String> {
    if token.len() < 32 {
        return Err(format!(
            "Local auth token from '{source}' must contain at least 32 characters"
        ));
    }
    Ok(token)
}

fn constant_time_token_eq(actual: &str, expected: &str) -> bool {
    let actual = actual.as_bytes();
    let expected = expected.as_bytes();
    let mut different = actual.len() ^ expected.len();
    let compared_len = actual.len().max(expected.len());
    for index in 0..compared_len {
        let actual_byte = actual.get(index).copied().unwrap_or_default();
        let expected_byte = expected.get(index).copied().unwrap_or_default();
        different |= usize::from(actual_byte ^ expected_byte);
    }
    different == 0
}

fn local_request_authorized(headers: &HeaderMap, expected_token: &str) -> bool {
    bearer_token(headers)
        .as_deref()
        .is_some_and(|actual| constant_time_token_eq(actual, expected_token))
}

async fn require_local_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if local_request_authorized(request.headers(), &state.local_auth_token) {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "type": "authentication_error",
                "message": "Missing or invalid local bridge token"
            }
        })),
    )
        .into_response()
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
    if *shutdown_rx.borrow() {
        return;
    }
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
