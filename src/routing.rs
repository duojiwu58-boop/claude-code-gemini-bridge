async fn responses(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    // Legacy compatibility only. Codex is intentionally kept on its native GPT
    // provider and is not maintained through this bridge. Do not expand this
    // route unless the maintenance policy at the top of this file changes.
    let api_key = state
        .fallback_api_key
        .clone()
        .filter(|value| !value.trim().is_empty());
    let Some(api_key) = api_key else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"code": "server_error", "message": "Legacy Responses requires GEMINI_API_KEY or GEMINI_BRIDGE_API_KEY_PROFILE"}})),
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
        .timeout(UPSTREAM_REQUEST_TIMEOUT)
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
    let upstream_body = match read_response_json_limited(upstream).await {
        Ok(value) => value,
        Err(err) => {
            error!("Gemini returned an invalid or oversized JSON response: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": "server_error", "message": "Gemini returned an invalid or oversized JSON response"}})),
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

fn apply_provider_request_overrides(
    profile: &ProviderProfile,
    request: &mut Value,
) -> Result<Vec<String>, String> {
    let Some(effort) = profile
        .openai_capabilities
        .reasoning_effort_override
        .as_deref()
    else {
        return Ok(Vec::new());
    };
    let request = request
        .as_object_mut()
        .ok_or_else(|| "Anthropic request body must be a JSON object".to_string())?;
    let previous = {
        let output_config = request
            .entry("output_config".to_string())
            .or_insert_with(|| json!({}));
        let output_config = output_config
            .as_object_mut()
            .ok_or_else(|| "Anthropic field 'output_config' must be an object".to_string())?;
        let previous = output_config
            .get("effort")
            .and_then(Value::as_str)
            .map(str::to_owned);
        output_config.insert("effort".to_string(), json!(effort));
        previous
    };

    let mut diagnostics = Vec::new();
    if previous.as_deref() != Some(effort) {
        diagnostics.push(match previous {
            Some(previous) => format!(
                "Provider profile overrode Anthropic output_config.effort from '{previous}' to '{effort}'"
            ),
            None => format!("Provider profile set Anthropic output_config.effort to '{effort}'"),
        });
    }

    let thinking = request
        .entry("thinking".to_string())
        .or_insert_with(|| json!({}));
    let thinking = thinking
        .as_object_mut()
        .ok_or_else(|| "Anthropic field 'thinking' must be an object".to_string())?;
    let thinking_type = thinking.get("type").and_then(Value::as_str);
    if effort == "none" {
        if thinking_type != Some("disabled") {
            thinking.insert("type".to_string(), json!("disabled"));
            thinking.remove("budget_tokens");
            diagnostics.push(
                "Provider profile reasoning_effort 'none' disabled Anthropic thinking".to_string(),
            );
        }
    } else if thinking_type == Some("disabled") {
        thinking.insert("type".to_string(), json!("adaptive"));
        thinking.remove("budget_tokens");
        diagnostics.push(format!(
            "Provider profile reasoning_effort '{effort}' overrode Anthropic thinking.type 'disabled' with 'adaptive'"
        ));
    }
    if thinking.is_empty() {
        request.remove("thinking");
    }
    Ok(diagnostics)
}

async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    let Some(active_profile) = active_provider_profile(&state) else {
        return anthropic_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "No active provider profile",
        );
    };
    let request_override_diagnostics =
        match apply_provider_request_overrides(&active_profile, &mut request) {
            Ok(diagnostics) => diagnostics,
            Err(message) => {
                return anthropic_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    &message,
                );
            }
        };
    if let Some(identity) = upstream_identity_label(&active_profile, &state.model) {
        if let Err(message) = append_bridge_identity(&mut request, &identity) {
            return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
        }
    }
    if let Err(err) = apply_vision_proxy(&state, &active_profile, &mut request).await {
        error!(
            provider = %active_profile.file_name,
            error = %err.message,
            "Vision proxy failed"
        );
        return anthropic_error(err.status, err.error_type, &err.message);
    }
    let local_capabilities = active_profile.openai_capabilities.clone();
    match active_profile.transport {
        ProviderTransport::Anthropic => {
            let mut diagnostics = request_override_diagnostics;
            diagnostics.extend(match active_profile.openai_capabilities.chat_dialect {
                OpenAiChatDialect::DeepSeek => deepseek_anthropic_reasoning_diagnostics(&request),
                OpenAiChatDialect::Qwen => qwen_anthropic_reasoning_diagnostics(&request),
                _ => Vec::new(),
            });
            diagnostics.extend(normalize_openrouter_claude5_request(
                &active_profile,
                &mut request,
            ));
            let provider_file = active_profile.file_name.clone();
            let response = forward_anthropic_profile(active_profile, &headers, request).await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::OpenAiChat => {
            let mut diagnostics = request_override_diagnostics;
            diagnostics.extend(openai_request_diagnostics(
                &request,
                &active_profile.openai_capabilities,
                ProviderTransport::OpenAiChat,
            ));
            let provider_file = active_profile.file_name.clone();
            let response =
                forward_openai_profile(active_profile, request, state.thought_signatures.clone())
                    .await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::OpenAiResponses => {
            let mut diagnostics = request_override_diagnostics;
            diagnostics.extend(openai_request_diagnostics(
                &request,
                &active_profile.openai_capabilities,
                ProviderTransport::OpenAiResponses,
            ));
            let provider_file = active_profile.file_name.clone();
            let response = forward_openai_responses_profile(
                active_profile,
                request,
                state.interaction_continuations.clone(),
            )
            .await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::GeminiInteractions => {
            let mut active_profile = active_profile;
            if active_profile
                .openai_capabilities
                .reasoning_effort_override
                .is_none()
            {
                match current_gemini_thinking_level(&state) {
                    Ok(level) => {
                        active_profile
                            .openai_capabilities
                            .gemini_thinking_level_override = level;
                    }
                    Err(message) => {
                        return anthropic_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "api_error",
                            &message,
                        );
                    }
                }
            }
            if let Err(message) =
                apply_bridge_managed_gemini_credentials(&state, &mut active_profile)
            {
                return anthropic_error(StatusCode::UNAUTHORIZED, "authentication_error", &message);
            }
            let mut diagnostics = request_override_diagnostics;
            diagnostics.extend(gemini_interaction_request_diagnostics(&request));
            if let Some(diagnostic) = gemini_interaction_pdf_tool_diagnostic(
                &request,
                &active_profile.openai_capabilities,
            ) {
                diagnostics.push(diagnostic);
            }
            let provider_file = active_profile.file_name.clone();
            let response = forward_gemini_interactions_profile(
                active_profile,
                request,
                state.interaction_continuations.clone(),
            )
            .await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::LocalGemini => {}
    }

    let Some(api_key) = state
        .fallback_api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return anthropic_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "LocalGemini requires GEMINI_API_KEY or GEMINI_BRIDGE_API_KEY_PROFILE",
        );
    };

    let stream_requested = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let chat_request = match translate_anthropic_request_with_capabilities(
        &request,
        &state.model,
        &state.thought_signatures,
        &local_capabilities,
    ) {
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

    let upstream_request = transport
        .client
        .post(&state.upstream_url)
        .bearer_auth(api_key)
        .json(&chat_request);
    let upstream = match apply_upstream_total_timeout(upstream_request, stream_requested)
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
        let upstream_body = match read_response_json_limited(upstream).await {
            Ok(value) => value,
            Err(err) => {
                error!("Gemini returned an invalid or oversized JSON error response: {err}");
                return anthropic_error(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    "Gemini returned an invalid or oversized JSON error response",
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

    if stream_requested {
        let estimated_input_tokens =
            u64::try_from(estimate_anthropic_input_tokens(&request)).unwrap_or(u64::MAX);
        return anthropic_upstream_stream_response(
            upstream,
            state.model.clone(),
            state.thought_signatures.clone(),
            estimated_input_tokens,
            local_capabilities.clone(),
        );
    }

    let upstream_body = match read_response_json_limited(upstream).await {
        Ok(value) => value,
        Err(err) => {
            error!("Gemini returned an invalid or oversized JSON response: {err}");
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Gemini returned an invalid or oversized JSON response",
            );
        }
    };
    let message = match translate_anthropic_response_with_capabilities(
        &upstream_body,
        &state.model,
        &state.thought_signatures,
        &local_capabilities,
    ) {
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

fn is_openrouter_profile(profile: &ProviderProfile) -> bool {
    url::Url::parse(&profile.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "openrouter.ai" || host.ends_with(".openrouter.ai"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Claude5Model {
    Sonnet,
    Opus,
}

fn claude_5_model(model: &str) -> Option<Claude5Model> {
    let model = display_model_name(model);
    match model.rsplit('/').next().unwrap_or(&model).to_ascii_lowercase().as_str() {
        "claude-sonnet-5" => Some(Claude5Model::Sonnet),
        "claude-opus-5" => Some(Claude5Model::Opus),
        _ => None,
    }
}

fn normalize_openrouter_claude5_request(
    profile: &ProviderProfile,
    request: &mut Value,
) -> Vec<String> {
    if profile.transport != ProviderTransport::Anthropic || !is_openrouter_profile(profile) {
        return Vec::new();
    }
    let Some(model) = claude_5_model(&profile.model) else {
        return Vec::new();
    };
    let Some(object) = request.as_object_mut() else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    if let Some(thinking) = object.get_mut("thinking").and_then(Value::as_object_mut) {
        if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
            thinking.insert("type".to_string(), json!("adaptive"));
            thinking.remove("budget_tokens");
            thinking
                .entry("display".to_string())
                .or_insert_with(|| json!("summarized"));
            diagnostics.push(
                "Converted removed Claude 5 manual thinking to adaptive thinking; budget_tokens is not supported"
                    .to_string(),
            );
        }
    }
    if object
        .get("temperature")
        .and_then(Value::as_f64)
        .is_some_and(|value| value != 1.0)
    {
        object.remove("temperature");
        diagnostics.push(
            "Removed non-default temperature because Claude 5 models accept only temperature=1.0"
                .to_string(),
        );
    }
    if object
        .get("top_p")
        .and_then(Value::as_f64)
        .is_some_and(|value| value < 0.99)
    {
        object.remove("top_p");
        diagnostics.push(
            "Removed top_p below 0.99 because Claude 5 models reject non-default top_p"
                .to_string(),
        );
    }
    if object.remove("top_k").is_some() {
        diagnostics.push(
            "Removed top_k because Claude 5 models do not accept that parameter".to_string(),
        );
    }
    let thinking_disabled = object
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        == Some("disabled");
    let conflicting_effort = (model == Claude5Model::Opus && thinking_disabled)
        .then(|| {
            object
                .get("output_config")
                .and_then(Value::as_object)
                .and_then(|output_config| output_config.get("effort"))
                .and_then(Value::as_str)
                .filter(|effort| matches!(*effort, "xhigh" | "max"))
                .map(str::to_owned)
        })
        .flatten();
    if let Some(effort) = conflicting_effort {
        if let Some(output_config) = object
            .get_mut("output_config")
            .and_then(Value::as_object_mut)
        {
            output_config.insert("effort".to_string(), json!("high"));
            diagnostics.push(format!(
                "Reduced Claude Opus 5 effort from '{effort}' to 'high' because thinking is disabled"
            ));
        }
    }
    diagnostics
}

async fn forward_anthropic_profile(
    profile: ProviderProfile,
    client_headers: &HeaderMap,
    mut request: Value,
) -> Response {
    match profile.openai_capabilities.chat_dialect {
        OpenAiChatDialect::DeepSeek => {
            let policy = match apply_deepseek_anthropic_reasoning_policy(
                &mut request,
                &profile.openai_capabilities,
            ) {
                Ok(policy) => policy,
                Err(message) => {
                    return anthropic_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        &message,
                    )
                }
            };
            let (replay_messages, replay_tokens) = deepseek_anthropic_reasoning_stats(&request);
            info!(
                provider = %profile.file_name,
                thinking_enabled = policy.thinking_enabled,
                reasoning_effort = policy.effort.unwrap_or("omitted"),
                policy_source = policy.source,
                thinking_budget_tokens = request
                    .pointer("/thinking/budget_tokens")
                    .and_then(value_as_u64)
                    .unwrap_or(0),
                max_tokens = request
                    .get("max_tokens")
                    .and_then(value_as_u64)
                    .unwrap_or(0),
                reasoning_replay_messages = replay_messages,
                reasoning_replay_estimated_tokens = replay_tokens,
                "DeepSeek Anthropic reasoning policy"
            );
            // DeepSeek user_id isolates KVCache and per-user concurrency
            // quotas. Forward a validated client value or the profile
            // default, and drop values that violate the documented
            // [a-zA-Z0-9_-]{1,512} shape.
            if let Some(request_object) = request.as_object_mut() {
                let metadata = request_object
                    .entry("metadata".to_string())
                    .or_insert_with(|| json!({}));
                if let Some(metadata) = metadata.as_object_mut() {
                    let user_id = metadata
                        .get("user_id")
                        .and_then(Value::as_str)
                        .and_then(validated_user_id)
                        .or_else(|| profile.openai_capabilities.user_id.clone());
                    match user_id {
                        Some(user_id) => {
                            metadata.insert("user_id".to_string(), json!(user_id));
                        }
                        None => {
                            metadata.remove("user_id");
                        }
                    }
                }
            }
        }
        OpenAiChatDialect::Qwen => {
            let policy = match apply_qwen_anthropic_reasoning_policy(
                &mut request,
                &profile.openai_capabilities,
            ) {
                Ok(policy) => policy,
                Err(message) => {
                    return anthropic_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        &message,
                    )
                }
            };
            info!(
                provider = %profile.file_name,
                thinking_enabled = policy.thinking_enabled,
                reasoning_effort = policy.effort.unwrap_or("omitted"),
                thinking_budget_tokens = policy.budget_tokens.unwrap_or(0),
                max_tokens = request.get("max_tokens").and_then(value_as_u64).unwrap_or(0),
                policy_source = policy.source,
                estimated_input_tokens = estimate_anthropic_input_tokens(&request),
                message_count = request.get("messages").and_then(|value| value.as_array()).map(Vec::len).unwrap_or(0),
                "Qwen Anthropic reasoning policy"
            );
        }
        _ => {}
    }
    let Some(request_object) = request.as_object_mut() else {
        return anthropic_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Anthropic request body must be a JSON object",
        );
    };
    request_object.insert("model".to_string(), Value::String(profile.model.clone()));
    let stream_requested = request_object
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let upstream_url = profile.upstream_url.clone();
    let upstream_build = || {
        let upstream_request = profile.client.post(&upstream_url).json(&request);
        let upstream_request =
            apply_anthropic_forward_headers(upstream_request, &profile, client_headers);
        apply_upstream_total_timeout(upstream_request, stream_requested)
    };

    let upstream_started = Instant::now();
    let upstream = match send_with_rate_limit_retry(upstream_build, &profile.file_name).await {
        Ok(response) => response,
        Err(err) => {
            error!(
                "Provider '{}' request to '{}' failed: {err}",
                profile.file_name, upstream_url
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &err.to_string());
        }
    };
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        info!(
            provider = %profile.file_name,
            status = upstream.status().as_u16(),
            upstream_response_headers_ms = upstream_started.elapsed().as_millis(),
            "Qwen Anthropic upstream response"
        );
    }
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
    let forwarded_headers = upstream
        .headers()
        .iter()
        .filter(|(name, _)| should_forward_anthropic_response_header(name.as_str()))
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    // The response body owns the reqwest stream. If Claude Code disconnects,
    // Hyper drops this body and the upstream response stream with it, which
    // cancels further socket reads. The stream wrapper bounds a connected
    // upstream that stops producing response bytes without imposing a total
    // deadline on long responses that continue making progress.
    let body = Body::from_stream(anthropic_passthrough_stream(
        upstream.bytes_stream(),
        UPSTREAM_STREAM_IDLE_TIMEOUT,
    ));
    let mut builder = Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("cache-control", "no-cache");
    if let Some(request_id) = request_id {
        builder = builder.header("request-id", request_id);
    }
    for (name, value) in forwarded_headers {
        builder = builder.header(name, value);
    }
    builder.body(body).unwrap_or_else(|err| {
        anthropic_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("Cannot build provider response: {err}"),
        )
    })
}

fn anthropic_passthrough_stream<S, B, E>(
    byte_stream: S,
    idle_timeout: Duration,
) -> impl Stream<Item = Result<B, std::io::Error>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    stream::unfold(
        Some(Box::pin(byte_stream)),
        move |byte_stream| async move {
            let mut byte_stream = byte_stream?;
            match tokio::time::timeout(idle_timeout, byte_stream.next()).await {
                Ok(Some(Ok(bytes))) => Some((Ok(bytes), Some(byte_stream))),
                Ok(Some(Err(err))) => Some((
                    Err(std::io::Error::other(format!(
                        "Anthropic upstream stream failed: {err}"
                    ))),
                    None,
                )),
                Ok(None) => None,
                Err(_) => Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Anthropic upstream stream was idle for too long",
                    )),
                    None,
                )),
            }
        },
    )
}

fn apply_anthropic_forward_headers(
    mut upstream_request: reqwest::RequestBuilder,
    profile: &ProviderProfile,
    client_headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    if let Some(token) = &profile.auth_token {
        upstream_request = upstream_request.bearer_auth(token);
    } else if let Some(api_key) = &profile.api_key {
        upstream_request = if is_openrouter_profile(profile) {
            upstream_request.bearer_auth(api_key)
        } else {
            upstream_request.header("x-api-key", api_key)
        };
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
    if is_openrouter_profile(profile) {
        for name in [
            "anthropic-user-profile-id",
            "x-openrouter-metadata",
            "http-referer",
            "x-openrouter-title",
        ] {
            if let Some(value) = client_headers.get(name).and_then(|value| value.to_str().ok()) {
                upstream_request = upstream_request.header(name, value);
            }
        }
    }
    if profile.openai_capabilities.responses_session_cache {
        upstream_request = upstream_request.header("x-dashscope-session-cache", "enable");
    }
    upstream_request
}

fn should_forward_anthropic_response_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("retry-after")
        || name.starts_with("anthropic-ratelimit-")
        || name.starts_with("x-ratelimit-")
        || name.starts_with("x-openrouter-")
}
