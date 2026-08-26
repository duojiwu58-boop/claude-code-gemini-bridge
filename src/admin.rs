fn provider_profile_json(profile: &ProviderProfile, active_file: &str) -> Value {
    let public_proxy_url = profile.proxy_url.as_deref().map(redact_url_credentials);
    json!({
        "file": profile.file_name,
        "name": profile.display_name,
        "source": profile.source.as_str(),
        "model": profile.model,
        "context_window": profile.context_window,
        "upstream_identity": profile.upstream_identity,
        "identity_override": profile.identity_override,
        "base_url": profile.base_url,
        "proxy": public_proxy_url,
        "local_gemini": profile.local_gemini,
        "transport": profile.transport.as_str(),
        "capabilities": openai_capabilities_json(&profile.openai_capabilities),
        "vision": {
            "mode": profile.vision.mode.as_str(),
            "profile": profile.vision.profile
        },
        "upstream_url": profile.upstream_url,
        "active": profile.file_name == active_file
    })
}

fn is_gemini_37_flash_profile(profile: &ProviderProfile) -> bool {
    profile.transport == ProviderTransport::GeminiInteractions
        && display_model_name(&profile.model).eq_ignore_ascii_case("gemini-3.7-flash")
}

fn effective_gemini_thinking_level(state: &AppState) -> Result<Option<String>, String> {
    let routing = state
        .routing
        .read()
        .map_err(|_| "Cannot read provider routing state".to_string())?;
    let Some(profile) = routing
        .profiles
        .iter()
        .find(|profile| profile.file_name == routing.active_file)
        .filter(|profile| is_gemini_37_flash_profile(profile))
    else {
        return Ok(None);
    };
    if let Some(level) = current_gemini_thinking_level(state)? {
        return Ok(Some(level));
    }
    Ok(profile
        .openai_capabilities
        .default_reasoning_effort
        .as_deref()
        .and_then(|level| normalize_gemini_thinking_level(level, "gemini-3.7-flash"))
        .or(Some("medium"))
        .map(str::to_owned))
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
    let (active_profile, profile_count, profile_source) = {
        let Ok(routing) = state.routing.read() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        };
        let active_profile = routing
            .profiles
            .iter()
            .find(|profile| profile.file_name == routing.active_file)
            .map(|profile| provider_profile_json(profile, &profile.file_name));
        (
            active_profile,
            routing.profiles.len(),
            routing.source.as_str(),
        )
    };
    let config_stamp =
        match provider_config_stamp_async(state.providers_dir.clone(), state.settings_dir.clone())
            .await
        {
            Ok(stamp) => stamp,
            Err(message) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": message})),
                )
                    .into_response();
            }
        };
    let gemini_thinking_level = match effective_gemini_thinking_level(&state) {
        Ok(level) => level,
        Err(message) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": message})),
            )
                .into_response();
        }
    };
    let proxy_enabled = transport.proxy_url.is_some();
    let public_proxy_url = transport.proxy_url.as_deref().map(redact_url_credentials);
    Json(json!({
        "status": "ok",
        "active_profile": active_profile,
        "profile_count": profile_count,
        "gemini_proxy": public_proxy_url,
        "gemini_proxy_mode": if proxy_enabled { "proxy" } else { "direct" },
        "listen_url": state.local_bridge_base_url,
        "providers_dir": state.providers_dir.to_string_lossy(),
        "profile_source": profile_source,
        "settings_dir": state.settings_dir.to_string_lossy(),
        "gemini_thinking_level": gemini_thinking_level,
        "config_stamp": config_stamp,
        "settings_stamp": config_stamp
    }))
    .into_response()
}

async fn admin_profiles(State(state): State<Arc<AppState>>) -> Response {
    let (profiles, profile_source) = {
        let Ok(routing) = state.routing.read() else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        };
        (
            routing
                .profiles
                .iter()
                .map(|profile| provider_profile_json(profile, &routing.active_file))
                .collect::<Vec<_>>(),
            routing.source.as_str(),
        )
    };
    let config_stamp =
        match provider_config_stamp_async(state.providers_dir.clone(), state.settings_dir.clone())
            .await
        {
            Ok(stamp) => stamp,
            Err(message) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": message})),
                )
                    .into_response();
            }
        };
    Json(json!({
        "profiles": profiles,
        "profile_source": profile_source,
        "config_stamp": config_stamp,
        "settings_stamp": config_stamp
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
    let _transition = state.admin_state_lock.lock().await;
    let selected = match state.routing.read() {
        Ok(routing) => routing
            .profiles
            .iter()
            .find(|profile| profile.file_name == file_name)
            .cloned(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        }
    };
    let Some(selected) = selected else {
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
    if let Err(err) = persist_bridge_state_async(
        state.bridge_state_path.clone(),
        selected.file_name.clone(),
        proxy_url,
    )
    .await
    {
        error!("Cannot persist active profile and proxy state: {err}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Provider switched but state was not persisted: {err}")})),
        )
            .into_response();
    }
    let Ok(mut routing) = state.routing.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update provider routing state"})),
        )
            .into_response();
    };
    routing.active_file = selected.file_name.clone();
    Json(json!({
        "status": "ok",
        "active_profile": provider_profile_json(&selected, &selected.file_name)
    }))
    .into_response()
}

async fn admin_reload_profiles(State(state): State<Arc<AppState>>) -> Response {
    let providers_dir = state.providers_dir.clone();
    let settings_dir = state.settings_dir.clone();
    let local_bridge_base_url = state.local_bridge_base_url.clone();
    let loaded_profiles = match tokio::task::spawn_blocking(move || {
        load_provider_profiles(&providers_dir, &settings_dir, &local_bridge_base_url)
    })
    .await
    {
        Ok(Ok(profiles)) => profiles,
        Ok(Err(message)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Cannot join profile loader: {err}")})),
            )
                .into_response();
        }
    };
    let _transition = state.admin_state_lock.lock().await;
    let active_file = match state.routing.read() {
        Ok(routing) => routing.active_file.clone(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        }
    };
    let selected = if loaded_profiles
        .profiles
        .iter()
        .any(|profile| profile.file_name == active_file)
    {
        active_file
    } else {
        loaded_profiles
            .profiles
            .iter()
            .find(|profile| profile.local_gemini)
            .or_else(|| loaded_profiles.profiles.first())
            .map(|profile| profile.file_name.clone())
            .unwrap_or_default()
    };
    let count = loaded_profiles.profiles.len();
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
        persist_bridge_state_async(state.bridge_state_path.clone(), selected.clone(), proxy_url)
            .await
    {
        error!("Cannot persist profiles and proxy state: {err}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Profiles were not reloaded because state persistence failed: {err}")})),
        )
            .into_response();
    }
    let Ok(mut routing) = state.routing.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update provider routing state"})),
        )
            .into_response();
    };
    routing.profiles = loaded_profiles.profiles;
    routing.source = loaded_profiles.source;
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

fn gemini_thinking_level_from_admin_request(request: &Value) -> Result<String, String> {
    match request.get("level").and_then(Value::as_str) {
        Some(level @ ("low" | "medium" | "high")) => Ok(level.to_string()),
        Some(_) => Err("Field 'level' must be low, medium, or high".to_string()),
        None => Err("Missing string field 'level'".to_string()),
    }
}

async fn admin_set_gemini_thinking_level(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Response {
    let level = match gemini_thinking_level_from_admin_request(&request) {
        Ok(level) => level,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
        }
    };
    let _transition = state.admin_state_lock.lock().await;
    let supported = match state.routing.read() {
        Ok(routing) => routing
            .profiles
            .iter()
            .find(|profile| profile.file_name == routing.active_file)
            .is_some_and(is_gemini_37_flash_profile),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot read provider routing state"})),
            )
                .into_response();
        }
    };
    if !supported {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Thinking level control requires an active gemini-3.7-flash Interactions profile"})),
        )
            .into_response();
    }
    if let Err(message) =
        persist_gemini_thinking_level_async(state.bridge_state_path.clone(), level.clone()).await
    {
        error!("{message}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": message})),
        )
            .into_response();
    }
    let Ok(mut current_level) = state.gemini_thinking_level.write() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Cannot update Gemini thinking-level state"})),
        )
            .into_response();
    };
    *current_level = Some(level.clone());
    info!(level = %level, "Gemini thinking level changed");
    Json(json!({
        "status": "ok",
        "gemini_thinking_level": level
    }))
    .into_response()
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
    let _transition = state.admin_state_lock.lock().await;
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
    if let Err(message) = persist_bridge_state_async(
        state.bridge_state_path.clone(),
        active_profile,
        proxy_url.clone(),
    )
    .await
    {
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
    let public_proxy_url = proxy_url.as_deref().map(redact_url_credentials);
    info!(
        "Gemini network route changed to {}",
        public_proxy_url.as_deref().unwrap_or("direct connection")
    );
    Json(json!({
        "status": "ok",
        "gemini_proxy": public_proxy_url,
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
    let body = read_response_json_limited(upstream)
        .await
        .unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": format!("Gemini returned HTTP {status}: {}", safe_error_message(&body))
            })),
        )
            .into_response();
    }
    let public_proxy_url = proxy_url.as_deref().map(redact_url_credentials);
    Json(json!({
        "status": "ok",
        "model": body.get("id").and_then(Value::as_str).unwrap_or(&state.model),
        "gemini_proxy": public_proxy_url,
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
        "gemini_thinking_level": effective_gemini_thinking_level(&state).ok().flatten(),
        "active_profile": active_profile.as_ref().map(|profile| provider_profile_json(profile, &profile.file_name))
    }))
}

async fn models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let active_profile = active_provider_profile(&state);
    Json(json!({
        "object": "list",
        "models": [],
        "data": [{
            "id": state.model,
            "object": "model",
            "created": 0,
            "owned_by": "claude-bridge",
            "upstream_model": active_profile.as_ref().map(|profile| profile.model.clone()),
            "context_window": active_profile.as_ref().and_then(|profile| profile.context_window)
        }]
    }))
}
