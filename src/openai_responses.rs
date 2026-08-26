fn responses_input_from_anthropic(
    messages: &[Value],
    custom_apply_patch: bool,
    known_tool_names: &HashMap<String, String>,
) -> Vec<Value> {
    let mut input = Vec::new();
    let mut tool_names = known_tool_names.clone();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content").unwrap_or(&Value::Null);
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                input.push(json!({"role": role, "content": text}));
            }
            continue;
        }
        let Some(parts) = content.as_array() else {
            continue;
        };
        let mut message_parts = Vec::new();
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        let kind = if role == "assistant" {
                            "output_text"
                        } else {
                            "input_text"
                        };
                        message_parts.push(json!({"type": kind, "text": text}));
                    }
                }
                Some("thinking") => {
                    flush_responses_message_parts(&mut input, role, &mut message_parts);
                    let thinking = part
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !thinking.is_empty() {
                        input.push(json!({
                            "type": "reasoning",
                            "content": [{"type": "reasoning_text", "text": thinking}]
                        }));
                    }
                }
                Some("tool_use") => {
                    flush_responses_message_parts(&mut input, role, &mut message_parts);
                    let call_id = part
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("toolu_unknown");
                    let name = part
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown_function");
                    tool_names.insert(call_id.to_string(), name.to_string());
                    let arguments = part
                        .get("input")
                        .map(Value::to_string)
                        .unwrap_or_else(|| "{}".to_string());
                    if custom_apply_patch && name == "apply_patch" {
                        let custom_input = part
                            .pointer("/input/patch")
                            .or_else(|| part.pointer("/input/input"))
                            .and_then(Value::as_str)
                            .unwrap_or(&arguments);
                        input.push(json!({
                            "type": "custom_tool_call",
                            "call_id": call_id,
                            "name": name,
                            "input": custom_input
                        }));
                    } else {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": name,
                            "arguments": arguments
                        }));
                    }
                }
                Some("tool_result") => {
                    flush_responses_message_parts(&mut input, role, &mut message_parts);
                    let call_id = part
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("toolu_unknown");
                    if !tool_names.contains_key(call_id) {
                        warn!(
                            tool_call_id = %call_id,
                            "Skipping orphan Anthropic tool result while translating to Responses"
                        );
                        continue;
                    }
                    let output = value_to_text(part.get("content").unwrap_or(&Value::Null));
                    let item_type = if custom_apply_patch
                        && tool_names.get(call_id).map(String::as_str) == Some("apply_patch")
                    {
                        "custom_tool_call_output"
                    } else {
                        "function_call_output"
                    };
                    input.push(json!({"type": item_type, "call_id": call_id, "output": output}));
                }
                _ => {}
            }
        }
        flush_responses_message_parts(&mut input, role, &mut message_parts);
    }
    input
}

fn flush_responses_message_parts(input: &mut Vec<Value>, role: &str, parts: &mut Vec<Value>) {
    if !parts.is_empty() {
        input.push(json!({
            "role": role,
            "content": Value::Array(std::mem::take(parts))
        }));
    }
}

fn responses_tools_from_anthropic(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> Vec<Value> {
    let mut tools = Vec::new();
    if let Some(source) = request.get("tools").and_then(Value::as_array) {
        for tool in source {
            if tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("web_search_"))
            {
                warn!("Skipping Anthropic server-side web search tool for Responses upstream");
                continue;
            }
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            if capabilities.responses_apply_patch_custom && name == "apply_patch" {
                let mut translated = json!({"type": "custom", "name": name});
                if let Some(description) = tool.get("description") {
                    translated["description"] = description.clone();
                }
                tools.push(translated);
                continue;
            }
            let mut schema = tool
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            if capabilities.tool_schema == ToolSchemaMode::Sanitize {
                sanitize_json_schema(&mut schema);
            }
            let mut translated = json!({
                "type": "function",
                "name": name,
                "parameters": schema
            });
            if let Some(description) = tool.get("description") {
                translated["description"] = description.clone();
            }
            tools.push(translated);
        }
    }
    tools.extend(
        capabilities
            .responses_builtin_tools
            .iter()
            .map(|kind| json!({"type": kind})),
    );
    tools
}

fn responses_output_format(request: &Value) -> Result<Option<Value>, String> {
    let Some(format) = request
        .pointer("/output_config/format")
        .or_else(|| request.get("output_format"))
        .filter(|format| !format.is_null())
    else {
        return Ok(None);
    };
    match format.get("type").and_then(Value::as_str) {
        Some("text") => Ok(None),
        Some("json_object") => Ok(Some(json!({"type": "json_object"}))),
        Some("json_schema") => {
            let schema = format.get("schema").cloned().ok_or_else(|| {
                "Anthropic json_schema output format is missing 'schema'".to_string()
            })?;
            let mut translated = json!({
                "type": "json_schema",
                "name": format.get("name").and_then(Value::as_str).unwrap_or("response"),
                "schema": schema
            });
            if let Some(strict) = format.get("strict").and_then(Value::as_bool) {
                translated["strict"] = Value::Bool(strict);
            }
            Ok(Some(translated))
        }
        Some(other) => Err(format!("Unsupported Responses output format '{other}'")),
        None => Err("Anthropic output format must have a string 'type'".to_string()),
    }
}

fn translate_anthropic_to_responses(
    request: &Value,
    profile: &ProviderProfile,
    continuations: &InteractionContinuationState,
) -> Result<Value, String> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'messages'".to_string())?;
    if messages.is_empty() {
        return Err("Responses requires at least one message".to_string());
    }
    let continuation = profile
        .openai_capabilities
        .responses_stateful
        .then(|| {
            interaction_continuation_for_request(
                &profile.file_name,
                request,
                messages,
                continuations,
            )
        })
        .flatten();
    let (source_messages, names) = if let Some(continuation) = &continuation {
        (
            &messages[continuation.input_start..],
            continuation.tool_names.clone(),
        )
    } else {
        (&messages[..], HashMap::new())
    };
    let input = responses_input_from_anthropic(
        source_messages,
        profile.openai_capabilities.responses_apply_patch_custom,
        &names,
    );
    if input.is_empty() {
        return Err("Responses request produced no supported input items".to_string());
    }

    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        json!(display_model_name(&profile.model)),
    );
    body.insert("input".to_string(), Value::Array(input));
    body.insert(
        "stream".to_string(),
        json!(request
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
    );
    body.insert(
        "store".to_string(),
        Value::Bool(profile.openai_capabilities.responses_stateful),
    );
    if let Some(continuation) = continuation {
        info!(
            provider = %profile.file_name,
            continuation = continuation.kind,
            "Continuing stored Responses request"
        );
        body.insert(
            "previous_response_id".to_string(),
            json!(continuation.previous_id),
        );
    } else {
        let instructions = value_to_text(request.get("system").unwrap_or(&Value::Null));
        if !instructions.is_empty() {
            body.insert("instructions".to_string(), json!(instructions));
        }
    }
    if let Some(max_tokens) = request.get("max_tokens").and_then(value_as_u64) {
        body.insert("max_output_tokens".to_string(), json!(max_tokens));
    }
    if let Some(stop) = request.get("stop_sequences").and_then(Value::as_array) {
        if !stop.is_empty() {
            body.insert("stop".to_string(), Value::Array(stop.clone()));
        }
    }
    if profile.openai_capabilities.sampling_parameters {
        if let Some(temperature) = request.get("temperature").and_then(Value::as_f64) {
            body.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = request.get("top_p").and_then(Value::as_f64) {
            body.insert("top_p".to_string(), json!(top_p));
        }
    }
    match profile.openai_capabilities.chat_dialect {
        OpenAiChatDialect::Qwen => {
            let (effort, _) =
                qwen_responses_reasoning_effort(request, &profile.openai_capabilities);
            body.insert("reasoning".to_string(), json!({"effort": effort}));
        }
        OpenAiChatDialect::DeepSeek => {
            // DeepSeek Responses controls reasoning exclusively through
            // reasoning.effort (none/low/high/max). A disabled thinking
            // request must still send {"effort":"none"}: omitting the field
            // leaves upstream default reasoning switched on. The profile
            // default goes through the same policy as the request value so
            // configured tiers like "minimal"/"xhigh" never leak unmapped
            // values that upstream would reject with HTTP 400. Profiles that
            // set reasoning_effort: false opt out of the field entirely for
            // endpoints that reject it.
            if profile.openai_capabilities.reasoning_effort {
                let policy = deepseek_reasoning_policy(request, &profile.openai_capabilities);
                body.insert(
                    "reasoning".to_string(),
                    json!({"effort": policy.effort.unwrap_or("none")}),
                );
            }
            // DeepSeek user_id isolates KVCache and per-user concurrency
            // quotas. Prefer a validated client value, then the profile
            // default, and drop values that violate [a-zA-Z0-9_-]{1,512}.
            if let Some(user_id) = request
                .pointer("/metadata/user_id")
                .and_then(Value::as_str)
                .and_then(validated_user_id)
                .or_else(|| profile.openai_capabilities.user_id.clone())
            {
                body.insert("user_id".to_string(), json!(user_id));
            }
        }
        _ => {
            if request.pointer("/thinking/type").and_then(Value::as_str) != Some("disabled") {
                let effort = request
                    .pointer("/output_config/effort")
                    .and_then(Value::as_str)
                    .or(profile
                        .openai_capabilities
                        .default_reasoning_effort
                        .as_deref());
                if let Some(effort) = effort {
                    let effort = if profile.openai_capabilities.chat_dialect
                        == OpenAiChatDialect::Kimi
                    {
                        kimi_reasoning_effort(effort)
                    } else {
                        effort
                    };
                    body.insert("reasoning".to_string(), json!({"effort": effort}));
                }
            }
        }
    }
    let tools = responses_tools_from_anthropic(request, &profile.openai_capabilities);
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = request.get("tool_choice") {
        if let Some(choice) = translate_responses_tool_choice(choice) {
            body.insert("tool_choice".to_string(), choice);
        }
    }
    if let Some(format) = responses_output_format(request)? {
        body.insert("text".to_string(), json!({"format": format}));
    }
    Ok(Value::Object(body))
}

fn translate_responses_tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"type": "function", "name": name})),
        _ => None,
    }
}

fn openai_usage_to_anthropic(usage: &Value, estimated_input_tokens: u64) -> Value {
    let input_tokens = usage_token(
        usage,
        &["input_tokens", "prompt_tokens", "total_input_tokens"],
    )
    .unwrap_or(estimated_input_tokens);
    let output_tokens = usage_token(
        usage,
        &["output_tokens", "completion_tokens", "total_output_tokens"],
    )
    .unwrap_or(0);
    let cache_read = usage
        .pointer("/input_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(value_as_u64)
        .or_else(|| usage.get("prompt_cache_hit_tokens").and_then(value_as_u64))
        .or_else(|| usage.get("cached_tokens").and_then(value_as_u64));
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(value_as_u64);
    let reasoning_tokens = usage
        .pointer("/output_tokens_details/reasoning_tokens")
        .or_else(|| usage.pointer("/completion_tokens_details/reasoning_tokens"))
        .and_then(value_as_u64);
    let mut translated = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens
    });
    if let Some(value) = cache_read {
        translated["cache_read_input_tokens"] = json!(value);
    }
    if let Some(value) = cache_creation {
        translated["cache_creation_input_tokens"] = json!(value);
    }
    if let Some(value) = reasoning_tokens {
        translated["reasoning_tokens"] = json!(value);
    }
    translated
}

fn log_qwen_usage(provider: &str, transport: &str, usage: &Value) {
    let input_tokens = usage_token(
        usage,
        &["input_tokens", "prompt_tokens", "total_input_tokens"],
    )
    .unwrap_or(0);
    let output_tokens = usage_token(
        usage,
        &["output_tokens", "completion_tokens", "total_output_tokens"],
    )
    .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
        .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
        .or_else(|| usage.get("prompt_cache_hit_tokens"))
        .or_else(|| usage.get("cached_tokens"))
        .and_then(value_as_u64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(value_as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("reasoning_tokens")
        .or_else(|| usage.pointer("/output_tokens_details/reasoning_tokens"))
        .or_else(|| usage.pointer("/completion_tokens_details/reasoning_tokens"))
        .and_then(value_as_u64)
        .unwrap_or(0);
    info!(
        provider,
        transport,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        reasoning_tokens,
        "Qwen usage"
    );
}

fn is_responses_server_tool_item(item_type: &str) -> bool {
    matches!(
        item_type,
        "web_search_call"
            | "file_search_call"
            | "computer_call"
            | "code_interpreter_call"
            | "image_generation_call"
            | "mcp_call"
            | "mcp_list_tools"
    )
}

fn responses_server_tool_metadata(items: &[Value]) -> Option<Value> {
    let captured: Vec<Value> = items
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(is_responses_server_tool_item)
        })
        .map(interaction_server_tool_summary)
        .collect();
    (!captured.is_empty()).then(|| json!({"openai": {"responses_server_tools": captured}}))
}

fn responses_server_tool_usage(items: &[Value]) -> Option<Value> {
    let mut web_search_requests = 0_u64;
    let mut web_fetch_requests = 0_u64;
    for item_type in items
        .iter()
        .filter_map(|item| item.get("type").and_then(Value::as_str))
    {
        if item_type.starts_with("web_search") {
            web_search_requests = web_search_requests.saturating_add(1);
        } else if item_type.starts_with("web_fetch") || item_type.starts_with("url_context") {
            web_fetch_requests = web_fetch_requests.saturating_add(1);
        }
    }
    (web_search_requests > 0 || web_fetch_requests > 0).then(|| {
        json!({
            "web_search_requests": web_search_requests,
            "web_fetch_requests": web_fetch_requests
        })
    })
}

fn translate_openai_responses_response(
    upstream: &Value,
    model: &str,
    estimated_input_tokens: u64,
) -> Result<InteractionResponseTranslation, String> {
    let response_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            format!(
                "Responses payload has no id: {}",
                safe_error_message(upstream)
            )
        })?
        .to_string();
    let status = upstream
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    if matches!(status, "failed" | "cancelled") {
        return Err(safe_error_message(upstream));
    }
    let output = upstream
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| "Responses payload has no output array".to_string())?;
    let mut content = Vec::new();
    let mut calls = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                let reasoning = item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .chain(
                        item.get("summary")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten(),
                    )
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !reasoning.is_empty() {
                    content.push(json!({"type": "thinking", "thinking": reasoning}));
                }
            }
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for part in parts {
                        match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        content.push(json!({"type": "text", "text": text}));
                                    }
                                }
                            }
                            Some("refusal") => {
                                if let Some(text) = part
                                    .get("refusal")
                                    .or_else(|| part.get("text"))
                                    .and_then(Value::as_str)
                                {
                                    content.push(json!({"type": "text", "text": text}));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("function_call") => {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_function")
                    .to_string();
                let parsed = match item.get("arguments") {
                    Some(Value::String(arguments)) => parse_tool_arguments(arguments)?,
                    Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
                    Some(Value::Null) | None => json!({}),
                    Some(_) => {
                        return Err(
                            "Responses function_call arguments must be a JSON string or object"
                                .to_string(),
                        )
                    }
                };
                calls.push((id.clone(), name.clone()));
                content.push(json!({"type": "tool_use", "id": id, "name": name, "input": parsed}));
            }
            Some("custom_tool_call") => {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("apply_patch")
                    .to_string();
                let custom_input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                calls.push((id.clone(), name.clone()));
                content.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": {"patch": custom_input}
                }));
            }
            Some(item_type) if is_responses_server_tool_item(item_type) => {
                info!(item_type, "Responses server-side tool item completed");
            }
            _ => {}
        }
    }
    let stop_reason = if !calls.is_empty() {
        "tool_use"
    } else if status == "incomplete" {
        "max_tokens"
    } else {
        "end_turn"
    };
    let mut message = json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": openai_usage_to_anthropic(
            upstream.get("usage").unwrap_or(&Value::Null),
            estimated_input_tokens
        )
    });
    if let Some(metadata) = responses_server_tool_metadata(output) {
        message["provider_metadata"] = metadata;
    }
    if let Some(server_tool_use) = responses_server_tool_usage(output) {
        message["usage"]["server_tool_use"] = server_tool_use;
    }
    Ok(InteractionResponseTranslation {
        message,
        interaction_id: response_id,
        assistant_content: content,
        calls,
    })
}

async fn forward_openai_responses_profile(
    profile: ProviderProfile,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
) -> Response {
    let responses_request =
        match translate_anthropic_to_responses(&request, &profile, &continuations) {
            Ok(value) => value,
            Err(message) => {
                return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message)
            }
        };
    let stream_requested = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let estimated_input_tokens =
        u64::try_from(estimate_anthropic_input_tokens(&request)).unwrap_or(u64::MAX);
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        let effective_effort = responses_request
            .pointer("/reasoning/effort")
            .and_then(Value::as_str)
            .unwrap_or("omitted");
        let (_, policy_source) =
            qwen_responses_reasoning_effort(&request, &profile.openai_capabilities);
        info!(
            provider = %profile.file_name,
            reasoning_effort = effective_effort,
            policy_source,
            estimated_input_tokens,
            stateful = profile.openai_capabilities.responses_stateful,
            session_cache = profile.openai_capabilities.responses_session_cache,
            continued = responses_request.get("previous_response_id").is_some(),
            "Qwen Responses reasoning policy"
        );
    }
    let Some(credential) = profile.auth_token.as_ref().or(profile.api_key.as_ref()) else {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "The active Responses profile has no API credential",
        );
    };
    let upstream_started = Instant::now();
    let upstream_build = || {
        let mut upstream_request = profile
            .client
            .post(&profile.upstream_url)
            .bearer_auth(credential)
            .json(&responses_request);
        if profile.openai_capabilities.responses_session_cache {
            upstream_request = upstream_request.header("x-dashscope-session-cache", "enable");
        }
        apply_upstream_total_timeout(upstream_request, stream_requested)
    };
    let upstream = match send_with_rate_limit_retry(upstream_build, &profile.file_name).await {
        Ok(response) => response,
        Err(err) => {
            error!(
                "Provider '{}' Responses request to '{}' failed: {err}",
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
            "Qwen Responses upstream response"
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
        let response_status =
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let (response_status, error_type) = openai_error_contract(response_status, &message);
        return anthropic_error(response_status, error_type, &message);
    }
    let model = display_model_name(&profile.model);
    if stream_requested {
        return openai_responses_stream_response(
            upstream,
            model,
            profile.file_name,
            request,
            continuations,
            estimated_input_tokens,
        );
    }
    let upstream_body = match read_response_json_limited(upstream).await {
        Ok(value) => value,
        Err(err) => {
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                &format!("Responses provider returned invalid JSON: {err}"),
            )
        }
    };
    let translated =
        match translate_openai_responses_response(&upstream_body, &model, estimated_input_tokens) {
            Ok(value) => value,
            Err(message) => return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &message),
        };
    if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Qwen {
        log_qwen_usage(
            &profile.file_name,
            "openai-responses",
            &translated.message["usage"],
        );
    }
    if profile.openai_capabilities.responses_stateful {
        remember_interaction_continuation(
            &continuations,
            &profile.file_name,
            &request,
            &translated.interaction_id,
            &translated.assistant_content,
            &translated.calls,
        );
    }
    Json(translated.message).into_response()
}

enum ResponsesStreamingBlock {
    Thinking(String),
    Text(String),
    Tool {
        id: String,
        name: String,
        arguments: String,
        custom: bool,
    },
}

struct ActiveResponsesStreamingBlock {
    anthropic_index: usize,
    block: ResponsesStreamingBlock,
}

struct OpenAiResponsesStreamTranslator {
    message_id: String,
    model: String,
    profile_file: String,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
    response_id: Option<String>,
    status: Option<String>,
    usage: Value,
    estimated_input_tokens: u64,
    next_content_index: usize,
    active_blocks: IndexMap<usize, ActiveResponsesStreamingBlock>,
    assistant_content: Vec<Value>,
    calls: Vec<(String, String)>,
    server_tool_items: Vec<Value>,
    completed: bool,
    finished: bool,
}

impl OpenAiResponsesStreamTranslator {
    fn new(
        model: String,
        profile_file: String,
        request: Value,
        continuations: Arc<InteractionContinuationState>,
        estimated_input_tokens: u64,
    ) -> Self {
        Self {
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            model,
            profile_file,
            request,
            continuations,
            response_id: None,
            status: None,
            usage: json!({}),
            estimated_input_tokens,
            next_content_index: 0,
            active_blocks: IndexMap::new(),
            assistant_content: Vec::new(),
            calls: Vec::new(),
            server_tool_items: Vec::new(),
            completed: false,
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
                        "input_tokens": self.estimated_input_tokens,
                        "output_tokens": 0
                    }
                }
            }),
        )?;
        Ok(events)
    }

    fn capture_response(&mut self, response: &Value) {
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.response_id = Some(id.to_string());
        }
        if let Some(status) = response.get("status").and_then(Value::as_str) {
            self.status = Some(status.to_string());
        }
        if let Some(usage) = response.get("usage") {
            self.usage = usage.clone();
        }
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                if item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(is_responses_server_tool_item)
                {
                    self.capture_server_tool(item);
                }
            }
        }
    }

    fn capture_server_tool(&mut self, item: &Value) {
        let key = interaction_server_tool_trace_key(item);
        let summary = interaction_server_tool_summary(item);
        if let Some(index) = key.as_ref().and_then(|key| {
            self.server_tool_items.iter().position(|existing| {
                interaction_server_tool_trace_key(existing).as_ref() == Some(key)
            })
        }) {
            self.server_tool_items[index] = summary;
        } else if self.server_tool_items.len() < INTERACTION_SERVER_TOOL_TRACE_CAPACITY {
            self.server_tool_items.push(summary);
        }
    }

    fn process_payload(&mut self, payload: &str) -> Result<Vec<Event>, String> {
        if payload.trim().is_empty() {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(payload)
            .map_err(|err| format!("Invalid JSON in Responses SSE stream: {err}"))?;
        if event.get("error").is_some()
            && event.get("type").and_then(Value::as_str) != Some("error")
        {
            return Err(safe_error_message(&event));
        }
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" | "response.in_progress" | "response.queued" => {
                if let Some(response) = event.get("response") {
                    self.capture_response(response);
                }
                Ok(Vec::new())
            }
            "response.completed" | "response.incomplete" => {
                if let Some(response) = event.get("response") {
                    self.capture_response(response);
                }
                self.completed = true;
                Ok(Vec::new())
            }
            "response.failed" | "error" => Err(safe_error_message(&event)),
            "response.output_item.added" => self.start_item(&event),
            "response.output_item.done" => self.stop_item(&event),
            "response.reasoning_text.delta"
            | "response.reasoning_content.delta"
            | "response.reasoning.delta"
            | "response.reasoning_summary_text.delta" => self.delta_text(&event, true),
            "response.output_text.delta" | "response.refusal.delta" => {
                self.delta_text(&event, false)
            }
            "response.function_call_arguments.delta" => self.delta_arguments(&event, false),
            "response.custom_tool_call_input.delta" => self.delta_arguments(&event, true),
            kind if kind.ends_with("_call.in_progress")
                || kind.ends_with("_call.searching")
                || kind.ends_with("_call.completed") =>
            {
                if let Some(item) = event.get("item") {
                    self.capture_server_tool(item);
                }
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn event_output_index(event: &Value) -> Option<usize> {
        event
            .get("output_index")
            .or_else(|| event.get("index"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
    }

    fn start_item(&mut self, event: &Value) -> Result<Vec<Event>, String> {
        let Some(output_index) = Self::event_output_index(event) else {
            return Ok(Vec::new());
        };
        let item = event.get("item").unwrap_or(&Value::Null);
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        if is_responses_server_tool_item(item_type) {
            self.capture_server_tool(item);
            return Ok(Vec::new());
        }
        let anthropic_index = self.next_content_index;
        let (block, content_block) = match item_type {
            "reasoning" => (
                ResponsesStreamingBlock::Thinking(String::new()),
                json!({"type": "thinking", "thinking": ""}),
            ),
            "message" => (
                ResponsesStreamingBlock::Text(String::new()),
                json!({"type": "text", "text": ""}),
            ),
            "function_call" | "custom_tool_call" => {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_unknown")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(if item_type == "custom_tool_call" {
                        "apply_patch"
                    } else {
                        "unknown_function"
                    })
                    .to_string();
                (
                    ResponsesStreamingBlock::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                        custom: item_type == "custom_tool_call",
                    },
                    json!({"type": "tool_use", "id": id, "name": name, "input": {}}),
                )
            }
            _ => return Ok(Vec::new()),
        };
        self.next_content_index += 1;
        self.active_blocks.insert(
            output_index,
            ActiveResponsesStreamingBlock {
                anthropic_index,
                block,
            },
        );
        let mut events = Vec::new();
        push_anthropic_event(
            &mut events,
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": anthropic_index,
                "content_block": content_block
            }),
        )?;
        Ok(events)
    }

    fn delta_text(&mut self, event: &Value, reasoning: bool) -> Result<Vec<Event>, String> {
        let Some(output_index) = Self::event_output_index(event) else {
            return Ok(Vec::new());
        };
        let Some(active) = self.active_blocks.get_mut(&output_index) else {
            return Ok(Vec::new());
        };
        let delta = event
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        match (&mut active.block, reasoning) {
            (ResponsesStreamingBlock::Thinking(text), true) => {
                text.push_str(delta);
                push_anthropic_event(
                    &mut events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": active.anthropic_index,
                        "delta": {"type": "thinking_delta", "thinking": delta}
                    }),
                )?;
            }
            (ResponsesStreamingBlock::Text(text), false) => {
                text.push_str(delta);
                push_anthropic_event(
                    &mut events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": active.anthropic_index,
                        "delta": {"type": "text_delta", "text": delta}
                    }),
                )?;
            }
            _ => {}
        }
        Ok(events)
    }

    fn delta_arguments(&mut self, event: &Value, custom: bool) -> Result<Vec<Event>, String> {
        let Some(output_index) = Self::event_output_index(event) else {
            return Ok(Vec::new());
        };
        let Some(active) = self.active_blocks.get_mut(&output_index) else {
            return Ok(Vec::new());
        };
        let delta = event
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ResponsesStreamingBlock::Tool {
            arguments,
            custom: is_custom,
            ..
        } = &mut active.block
        else {
            return Ok(Vec::new());
        };
        if custom != *is_custom || delta.is_empty() {
            return Ok(Vec::new());
        }
        append_streamed_tool_arguments(arguments, delta)?;
        // A custom tool carries raw text rather than JSON. Buffer it until the
        // item is complete so the Anthropic input_json_delta is one valid JSON
        // object instead of a concatenation of independently escaped chunks.
        if custom {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        push_anthropic_event(
            &mut events,
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": active.anthropic_index,
                "delta": {"type": "input_json_delta", "partial_json": delta}
            }),
        )?;
        Ok(events)
    }

    fn stop_item(&mut self, event: &Value) -> Result<Vec<Event>, String> {
        let Some(output_index) = Self::event_output_index(event) else {
            return Ok(Vec::new());
        };
        let item = event.get("item").unwrap_or(&Value::Null);
        if item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(is_responses_server_tool_item)
        {
            self.capture_server_tool(item);
            return Ok(Vec::new());
        }
        let Some(mut active) = self.active_blocks.shift_remove(&output_index) else {
            return Ok(Vec::new());
        };
        let mut events = Vec::new();
        match &mut active.block {
            ResponsesStreamingBlock::Thinking(text) => {
                if text.is_empty() {
                    *text = value_to_text(
                        item.get("content")
                            .or_else(|| item.get("summary"))
                            .unwrap_or(&Value::Null),
                    );
                    if !text.is_empty() {
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "thinking_delta", "thinking": text}
                            }),
                        )?;
                    }
                }
                self.assistant_content
                    .push(json!({"type": "thinking", "thinking": text}));
            }
            ResponsesStreamingBlock::Text(text) => {
                if text.is_empty() {
                    *text = value_to_text(item.get("content").unwrap_or(&Value::Null));
                    if !text.is_empty() {
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "text_delta", "text": text}
                            }),
                        )?;
                    }
                }
                self.assistant_content
                    .push(json!({"type": "text", "text": text}));
            }
            ResponsesStreamingBlock::Tool {
                id,
                name,
                arguments,
                custom,
            } => {
                if arguments.is_empty() {
                    *arguments = item
                        .get(if *custom { "input" } else { "arguments" })
                        .and_then(Value::as_str)
                        .unwrap_or(if *custom { "" } else { "{}" })
                        .to_string();
                }
                let input = if *custom {
                    json!({"patch": arguments})
                } else {
                    parse_tool_arguments(arguments)?
                };
                if *custom {
                    push_anthropic_event(
                        &mut events,
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": active.anthropic_index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())
                            }
                        }),
                    )?;
                }
                self.calls.push((id.clone(), name.clone()));
                self.assistant_content.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
        }
        push_anthropic_event(
            &mut events,
            "content_block_stop",
            json!({"type": "content_block_stop", "index": active.anthropic_index}),
        )?;
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<Event>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !self.completed {
            return Err("Responses stream ended before a terminal response event".to_string());
        }
        if !self.active_blocks.is_empty() {
            return Err("Responses stream ended with an unfinished output item".to_string());
        }
        let response_id = self
            .response_id
            .as_deref()
            .ok_or_else(|| "Responses stream completed without a response id".to_string())?;
        remember_interaction_continuation(
            &self.continuations,
            &self.profile_file,
            &self.request,
            response_id,
            &self.assistant_content,
            &self.calls,
        );
        let status = self.status.as_deref().unwrap_or("completed");
        let stop_reason = if !self.calls.is_empty() {
            "tool_use"
        } else if status == "incomplete" {
            "max_tokens"
        } else {
            "end_turn"
        };
        let mut delta = json!({"stop_reason": stop_reason, "stop_sequence": Value::Null});
        if let Some(metadata) = responses_server_tool_metadata(&self.server_tool_items) {
            delta["provider_metadata"] = metadata;
        }
        self.finished = true;
        let mut events = Vec::new();
        let mut usage = openai_usage_to_anthropic(&self.usage, self.estimated_input_tokens);
        if let Some(server_tool_use) = responses_server_tool_usage(&self.server_tool_items) {
            usage["server_tool_use"] = server_tool_use;
        }
        push_anthropic_event(
            &mut events,
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": delta,
                "usage": usage
            }),
        )?;
        push_anthropic_event(&mut events, "message_stop", json!({"type": "message_stop"}))?;
        Ok(events)
    }
}

fn openai_responses_event_stream<S, B, E>(
    byte_stream: S,
    model: String,
    profile_file: String,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
    estimated_input_tokens: u64,
) -> impl Stream<Item = Result<Event, Infallible>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let byte_stream = Box::pin(byte_stream);
    let translator = OpenAiResponsesStreamTranslator::new(
        model,
        profile_file,
        request,
        continuations,
        estimated_input_tokens,
    );
    let initial_events = match translator.start_events() {
        Ok(events) => VecDeque::from(events),
        Err(message) => VecDeque::from([anthropic_stream_error_event(&message)]),
    };
    stream::unfold(
        (
            byte_stream,
            SseDataDecoder::default(),
            translator,
            initial_events,
            false,
        ),
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
                match tokio::time::timeout(UPSTREAM_STREAM_IDLE_TIMEOUT, byte_stream.next()).await {
                    Ok(Some(Ok(bytes))) => match decoder.push_bytes(bytes.as_ref()) {
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
                    Ok(Some(Err(err))) => {
                        pending.push_back(anthropic_stream_error_event(&format!(
                            "Responses stream failed: {err}"
                        )));
                        ended = true;
                    }
                    Ok(None) => {
                        let mut failed = false;
                        match decoder.finish() {
                            Ok(payloads) => {
                                for payload in payloads {
                                    if payload.trim() == "[DONE]" {
                                        continue;
                                    }
                                    match translator.process_payload(&payload) {
                                        Ok(events) => pending.extend(events),
                                        Err(message) => {
                                            pending
                                                .push_back(anthropic_stream_error_event(&message));
                                            failed = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(message) => {
                                pending.push_back(anthropic_stream_error_event(&message));
                                failed = true;
                            }
                        }
                        if !failed {
                            match translator.finish() {
                                Ok(events) => pending.extend(events),
                                Err(message) => {
                                    pending.push_back(anthropic_stream_error_event(&message))
                                }
                            }
                        }
                        ended = true;
                    }
                    Err(_) => {
                        pending.push_back(anthropic_stream_error_event(
                            "Responses stream was idle for too long",
                        ));
                        ended = true;
                    }
                }
            }
        },
    )
}

fn openai_responses_stream_response(
    upstream: reqwest::Response,
    model: String,
    profile_file: String,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
    estimated_input_tokens: u64,
) -> Response {
    let event_stream = openai_responses_event_stream(
        upstream.bytes_stream(),
        model,
        profile_file,
        request,
        continuations,
        estimated_input_tokens,
    );
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
