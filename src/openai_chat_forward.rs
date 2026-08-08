async fn forward_openai_profile(
    profile: ProviderProfile,
    request: Value,
    thought_signatures: Arc<ThoughtSignatureCache>,
) -> Response {
    let upstream_model = display_model_name(&profile.model);
    let chat_request = match translate_anthropic_request_with_capabilities(
        &request,
        &upstream_model,
        &thought_signatures,
        &profile.openai_capabilities,
    ) {
        Ok(value) => value,
        Err(message) => {
            return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
        }
    };
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::DeepSeek {
        let policy = deepseek_reasoning_policy(&request, &profile.openai_capabilities);
        let (replay_messages, replay_tokens) = chat_replayed_reasoning_stats(&chat_request);
        let effective_effort = chat_request
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .unwrap_or("omitted");
        info!(
            provider = %profile.file_name,
            thinking_enabled = policy.thinking_enabled,
            reasoning_effort = effective_effort,
            policy_source = policy.source,
            reasoning_replay_messages = replay_messages,
            reasoning_replay_estimated_tokens = replay_tokens,
            "DeepSeek Chat reasoning policy"
        );
    } else if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        let policy = qwen_reasoning_policy(&request, &profile.openai_capabilities);
        let (replay_messages, replay_tokens) = chat_replayed_reasoning_stats(&chat_request);
        info!(
            provider = %profile.file_name,
            thinking_enabled = policy.thinking_enabled,
            reasoning_effort = policy.effort.unwrap_or("omitted"),
            thinking_budget_tokens = chat_request.get("thinking_budget").and_then(|value| value.as_u64()).unwrap_or(0),
            policy_source = policy.source,
            estimated_input_tokens = estimate_anthropic_input_tokens(&request),
            reasoning_replay_messages = replay_messages,
            reasoning_replay_estimated_tokens = replay_tokens,
            "Qwen Chat reasoning policy"
        );
    }
    let stream_requested = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let estimated_input_tokens =
        u64::try_from(estimate_anthropic_input_tokens(&request)).unwrap_or(u64::MAX);

    let Some(credential) = profile.auth_token.as_ref().or(profile.api_key.as_ref()) else {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "The active OpenAI-compatible profile has no API credential",
        );
    };
    let upstream_started = Instant::now();
    let upstream = match profile
        .client
        .post(&profile.upstream_url)
        .bearer_auth(credential)
        .json(&chat_request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!(
                "Provider '{}' OpenAI-compatible request to '{}' failed: {err}",
                profile.file_name, profile.upstream_url
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &err.to_string());
        }
    };
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        info!(
            provider = %profile.file_name,
            status = upstream.status().as_u16(),
            upstream_response_headers_ms = upstream_started.elapsed().as_millis(),
            "Qwen Chat upstream response"
        );
    }

    let status = upstream.status();
    if !status.is_success() {
        let response_text = match read_response_text_limited(upstream).await {
            Ok(text) => text,
            Err(err) => format!("Cannot read provider error response: {err}"),
        };
        let message = serde_json::from_str::<Value>(&response_text)
            .ok()
            .map(|value| safe_error_message(&value))
            .unwrap_or(response_text);
        error!(
            "Provider '{}' returned HTTP {status}: {message}",
            profile.file_name
        );
        let response_status =
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let (response_status, error_type) = openai_error_contract(response_status, &message);
        return anthropic_error(response_status, error_type, &message);
    }

    if stream_requested {
        return anthropic_upstream_stream_response(
            upstream,
            upstream_model,
            thought_signatures,
            estimated_input_tokens,
            profile.openai_capabilities,
        );
    }

    let upstream_body = match read_response_json_limited(upstream).await {
        Ok(value) => value,
        Err(err) => {
            error!(
                "Provider '{}' returned an invalid or oversized JSON response: {err}",
                profile.file_name
            );
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "OpenAI-compatible provider returned an invalid or oversized JSON response",
            );
        }
    };
    let message = match translate_anthropic_response_with_capabilities(
        &upstream_body,
        &upstream_model,
        &thought_signatures,
        &profile.openai_capabilities,
    ) {
        Ok(value) => value,
        Err(message) => {
            error!(
                "Cannot translate provider '{}' response: {message}",
                profile.file_name
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &message);
        }
    };
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        log_qwen_usage(&profile.file_name, "openai-chat", &message["usage"]);
    }
    Json(message).into_response()
}
