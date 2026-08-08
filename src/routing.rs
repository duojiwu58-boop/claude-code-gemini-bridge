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
            let diagnostics = match active_profile.openai_capabilities.chat_dialect {
                OpenAiChatDialect::DeepSeek => deepseek_anthropic_reasoning_diagnostics(&request),
                OpenAiChatDialect::Qwen => qwen_anthropic_reasoning_diagnostics(&request),
                _ => Vec::new(),
            };
            let provider_file = active_profile.file_name.clone();
            let response = forward_anthropic_profile(active_profile, &headers, request).await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::OpenAiChat => {
            let diagnostics = openai_request_diagnostics(
                &request,
                &active_profile.openai_capabilities,
                ProviderTransport::OpenAiChat,
            );
            let provider_file = active_profile.file_name.clone();
            let response =
                forward_openai_profile(active_profile, request, state.thought_signatures.clone())
                    .await;
            return attach_bridge_diagnostics(response, &provider_file, &diagnostics);
        }
        ProviderTransport::OpenAiResponses => {
            let diagnostics = openai_request_diagnostics(
                &request,
                &active_profile.openai_capabilities,
                ProviderTransport::OpenAiResponses,
            );
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
            let diagnostics = gemini_interaction_request_diagnostics(&request);
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
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                max_tokens = request
                    .get("max_tokens")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                reasoning_replay_messages = replay_messages,
                reasoning_replay_estimated_tokens = replay_tokens,
                "DeepSeek Anthropic reasoning policy"
            );
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
                max_tokens = request.get("max_tokens").and_then(|value| value.as_u64()).unwrap_or(0),
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
    let upstream_url = profile.upstream_url.clone();
    let upstream_request = profile.client.post(&upstream_url).json(&request);
    let upstream_request =
        apply_anthropic_forward_headers(upstream_request, &profile, client_headers);

    let upstream_started = Instant::now();
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
    // The response body owns the reqwest stream. If Claude Code disconnects,
    // Hyper drops this body and the upstream response stream with it, which
    // cancels further socket reads. The client-wide timeout also bounds
    // providers that ignore the closed connection and never finish a body.
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
    if profile.openai_capabilities.responses_session_cache {
        upstream_request = upstream_request.header("x-dashscope-session-cache", "enable");
    }
    upstream_request
}
