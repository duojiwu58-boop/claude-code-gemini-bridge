fn interaction_call_cache_key(profile_file: &str, call_id: &str) -> String {
    format!("{profile_file}\0{call_id}")
}

fn interaction_transcript_cache_key(
    profile_file: &str,
    system: Option<&Value>,
    messages: &[Value],
) -> String {
    let mut digest = Sha256::new();
    digest.update(profile_file.as_bytes());
    digest.update([0]);
    if let Some(system) = system {
        if let Ok(bytes) = serde_json::to_vec(system) {
            digest.update(bytes);
        }
    }
    digest.update([0]);
    if let Ok(bytes) = serde_json::to_vec(messages) {
        digest.update(bytes);
    }
    format!("{profile_file}:{:x}", digest.finalize())
}

fn evict_interaction_cache(cache: &mut InteractionContinuationCache) {
    if cache.calls.len() > INTERACTION_CONTINUATION_CAPACITY {
        let remove = INTERACTION_CONTINUATION_EVICTION_BATCH.min(cache.calls.len());
        cache.calls.drain(..remove);
    }
    if cache.transcripts.len() > INTERACTION_CONTINUATION_CAPACITY {
        let remove = INTERACTION_CONTINUATION_EVICTION_BATCH.min(cache.transcripts.len());
        cache.transcripts.drain(..remove);
    }
}

fn remember_interaction_calls(
    continuations: &InteractionContinuationState,
    profile_file: &str,
    interaction_id: &str,
    calls: &[(String, String)],
) {
    if interaction_id.is_empty() || calls.is_empty() {
        return;
    }
    let Ok(mut cache) = continuations.write() else {
        warn!("Cannot lock Gemini Interactions continuation cache for writing");
        return;
    };
    for (call_id, name) in calls {
        cache.calls.insert(
            interaction_call_cache_key(profile_file, call_id),
            InteractionCallContinuation {
                interaction_id: interaction_id.to_string(),
                name: name.clone(),
            },
        );
    }
    evict_interaction_cache(&mut cache);
}

fn remember_interaction_continuation(
    continuations: &InteractionContinuationState,
    profile_file: &str,
    request: &Value,
    interaction_id: &str,
    assistant_content: &[Value],
    calls: &[(String, String)],
) {
    if interaction_id.is_empty() {
        return;
    }
    let Some(source_messages) = request.get("messages").and_then(Value::as_array) else {
        return;
    };
    let mut messages = source_messages.clone();
    messages.push(json!({
        "role": "assistant",
        "content": assistant_content
    }));
    let transcript_key =
        interaction_transcript_cache_key(profile_file, request.get("system"), &messages);
    let Ok(mut cache) = continuations.write() else {
        warn!("Cannot lock Gemini Interactions continuation cache for writing");
        return;
    };
    cache
        .transcripts
        .insert(transcript_key, interaction_id.to_string());
    for (call_id, name) in calls {
        cache.calls.insert(
            interaction_call_cache_key(profile_file, call_id),
            InteractionCallContinuation {
                interaction_id: interaction_id.to_string(),
                name: name.clone(),
            },
        );
    }
    evict_interaction_cache(&mut cache);
}

fn interaction_content_from_anthropic(part: &Value) -> Option<Value> {
    match part.get("type").and_then(Value::as_str) {
        Some("text") => part
            .get("text")
            .and_then(Value::as_str)
            .map(|text| json!({"type": "text", "text": text})),
        Some(block_type @ ("image" | "document")) => {
            let source = part.get("source")?;
            let mut content = Map::new();
            content.insert("type".to_string(), json!(block_type));
            match source.get("type").and_then(Value::as_str) {
                Some("base64") => {
                    content.insert(
                        "data".to_string(),
                        source.get("data").cloned().unwrap_or(Value::Null),
                    );
                    content.insert(
                        "mime_type".to_string(),
                        source.get("media_type").cloned().unwrap_or(Value::Null),
                    );
                }
                Some("url") => {
                    content.insert(
                        "uri".to_string(),
                        source.get("url").cloned().unwrap_or(Value::Null),
                    );
                    if block_type == "document" {
                        content.insert(
                            "mime_type".to_string(),
                            source
                                .get("media_type")
                                .cloned()
                                .unwrap_or_else(|| json!("application/pdf")),
                        );
                    }
                }
                Some("text") if block_type == "document" => {
                    let text = source.get("data").and_then(Value::as_str)?;
                    content.insert(
                        "data".to_string(),
                        json!(BASE64_STANDARD.encode(text.as_bytes())),
                    );
                    content.insert("mime_type".to_string(), json!("text/plain"));
                }
                Some("content") if block_type == "document" => {
                    let text = value_to_text(source.get("content").unwrap_or(&Value::Null));
                    if text.is_empty() {
                        return None;
                    }
                    content.insert(
                        "data".to_string(),
                        json!(BASE64_STANDARD.encode(text.as_bytes())),
                    );
                    content.insert("mime_type".to_string(), json!("text/plain"));
                }
                _ => return None,
            }
            Some(Value::Object(content))
        }
        _ => None,
    }
}

fn interaction_tool_result_value(content: &Value, max_chars: Option<u64>) -> Value {
    if let Some(parts) = content.as_array() {
        let translated: Vec<Value> = parts
            .iter()
            .filter_map(interaction_content_from_anthropic)
            .collect();
        if !translated.is_empty() {
            let combined_text = translated
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let bounded_text = bound_tool_result_text(combined_text.clone(), max_chars);
            if bounded_text != combined_text {
                let mut bounded = vec![json!({"type": "text", "text": bounded_text})];
                bounded.extend(translated.into_iter().filter(|part| {
                    part.get("type").and_then(Value::as_str) != Some("text")
                }));
                return Value::Array(bounded);
            }
            return Value::Array(translated);
        }
    }
    let text = if let Some(text) = content.as_str() {
        text.to_string()
    } else {
        value_to_text(content)
    };
    Value::String(bound_tool_result_text(text, max_chars))
}

fn is_legacy_interaction_tool_call_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("[Tool call:") && text.contains("]\nArguments:")
}

fn interaction_user_steps(
    content: &Value,
    tool_names: &HashMap<String, String>,
    text_tool_history: bool,
    max_tool_result_chars: Option<u64>,
) -> Vec<Value> {
    if let Some(text) = content.as_str() {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![json!({"type": "user_input", "content": [{"type": "text", "text": text}]})]
        };
    }
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    let mut user_content = Vec::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) == Some("tool_result") {
            let call_id = part
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("toolu_unknown");
            let name = tool_names
                .get(call_id)
                .cloned()
                .unwrap_or_else(|| "unknown_function".to_string());
            if text_tool_history {
                let status = if part.get("is_error").and_then(Value::as_bool) == Some(true) {
                    "error"
                } else {
                    "result"
                };
                user_content.push(json!({
                    "type": "text",
                    "text": format!(
                        "An earlier {name} operation produced this {status}. It is historical context:"
                    )
                }));
                match interaction_tool_result_value(
                    part.get("content").unwrap_or(&Value::Null),
                    max_tool_result_chars,
                ) {
                    Value::Array(result_content) => user_content.extend(result_content),
                    Value::String(text) if !text.is_empty() => {
                        user_content.push(json!({"type": "text", "text": text}));
                    }
                    _ => {}
                }
                continue;
            }
            if !user_content.is_empty() {
                steps.push(json!({
                    "type": "user_input",
                    "content": std::mem::take(&mut user_content)
                }));
            }
            steps.push(json!({
                "type": "function_result",
                "call_id": call_id,
                "name": name,
                "is_error": part.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                "result": interaction_tool_result_value(
                    part.get("content").unwrap_or(&Value::Null),
                    max_tool_result_chars
                )
            }));
        } else if let Some(translated) = interaction_content_from_anthropic(part) {
            user_content.push(translated);
        }
    }
    if !user_content.is_empty() {
        steps.push(json!({"type": "user_input", "content": user_content}));
    }
    steps
}

fn interaction_assistant_steps(
    content: &Value,
    tool_names: &mut HashMap<String, String>,
    text_tool_history: bool,
) -> Vec<Value> {
    if let Some(text) = content.as_str() {
        return if text.is_empty()
            || (text_tool_history && is_legacy_interaction_tool_call_text(text))
        {
            Vec::new()
        } else {
            vec![json!({"type": "model_output", "content": [{"type": "text", "text": text}]})]
        };
    }
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    let mut text_content = Vec::new();
    let flush_text = |steps: &mut Vec<Value>, text_content: &mut Vec<Value>| {
        if !text_content.is_empty() {
            steps.push(json!({
                "type": "model_output",
                "content": std::mem::take(text_content)
            }));
        }
    };
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty()
                        && !(text_tool_history && is_legacy_interaction_tool_call_text(text))
                    {
                        text_content.push(json!({"type": "text", "text": text}));
                    }
                }
            }
            Some("thinking") => {
                if text_tool_history {
                    continue;
                }
                flush_text(&mut steps, &mut text_content);
                let thinking = part
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut thought = json!({
                    "type": "thought",
                    "summary": if thinking.is_empty() {
                        Vec::<Value>::new()
                    } else {
                        vec![json!({"type": "text", "text": thinking})]
                    }
                });
                if let Some(signature) = part.get("signature").and_then(Value::as_str) {
                    thought["signature"] = json!(signature);
                }
                steps.push(thought);
            }
            Some("redacted_thinking") => {
                if text_tool_history {
                    continue;
                }
                flush_text(&mut steps, &mut text_content);
                steps.push(json!({
                    "type": "thought",
                    "signature": part.get("data").cloned().unwrap_or(Value::Null),
                    "summary": []
                }));
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
                tool_names.insert(call_id.to_string(), name.to_string());
                if text_tool_history {
                    continue;
                }
                flush_text(&mut steps, &mut text_content);
                steps.push(json!({
                    "type": "function_call",
                    "id": call_id,
                    "name": name,
                    "arguments": part.get("input").cloned().unwrap_or_else(|| json!({}))
                }));
            }
            _ => {}
        }
    }
    flush_text(&mut steps, &mut text_content);
    steps
}

fn interaction_messages_have_tool_history(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    matches!(
                        part.get("type").and_then(Value::as_str),
                        Some("tool_use" | "tool_result")
                    )
                })
            })
    })
}

fn interaction_tool_names_from_messages(messages: &[Value]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in messages {
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            if part.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(call_id) = part.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = part.get("name").and_then(Value::as_str) else {
                continue;
            };
            names.insert(call_id.to_string(), name.to_string());
        }
    }
    names
}

fn interaction_steps_from_messages(
    messages: &[Value],
    text_tool_history: bool,
    max_tool_result_chars: Option<u64>,
) -> Vec<Value> {
    let mut steps = Vec::new();
    let mut tool_names = HashMap::new();
    for message in messages {
        let content = message.get("content").unwrap_or(&Value::Null);
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                steps.extend(interaction_assistant_steps(
                    content,
                    &mut tool_names,
                    text_tool_history,
                ));
            }
            Some("user") => steps.extend(interaction_user_steps(
                content,
                &tool_names,
                text_tool_history,
                max_tool_result_chars,
            )),
            _ => {}
        }
    }
    steps
}

fn translated_interaction_tools(request: &Value, capabilities: &OpenAiCapabilities) -> Vec<Value> {
    let mut translated = Vec::new();
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mut schema = tool
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            if capabilities.tool_schema == ToolSchemaMode::Sanitize {
                sanitize_json_schema(&mut schema);
            }
            let mut translated_tool = json!({
                "type": "function",
                "name": name,
                "parameters": schema
            });
            if let Some(description) = tool.get("description") {
                translated_tool["description"] = description.clone();
            }
            translated.push(translated_tool);
        }
    }
    translated.extend(
        capabilities
            .gemini_builtin_tools
            .iter()
            .map(|tool| json!({"type": tool})),
    );
    if !capabilities.gemini_file_search_store_names.is_empty() {
        translated.push(json!({
            "type": "file_search",
            "file_search_store_names": capabilities.gemini_file_search_store_names
        }));
    }
    translated
}

fn interaction_tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("any")),
        "none" => Some(json!("none")),
        "tool" => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"allowed_tools": {"mode": "any", "tools": [name]}})),
        _ => None,
    }
}

fn interaction_thinking_level(
    request: &Value,
    capabilities: &OpenAiCapabilities,
    model: &str,
) -> Option<String> {
    if let Some(level) = capabilities
        .gemini_thinking_level_override
        .as_deref()
        .and_then(|level| normalize_gemini_thinking_level(level, model))
    {
        return Some(level.to_string());
    }
    if request.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
        return Some(if model.starts_with("gemini-3") {
            "low"
        } else {
            "none"
        }
        .to_string());
    }
    request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
        .and_then(|effort| normalize_gemini_thinking_level(effort, model))
        .map(str::to_owned)
        .or_else(|| {
            request
                .get("thinking")
                .and_then(|thinking| {
                    thinking
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
                            (thinking.get("type").and_then(Value::as_str) == Some("adaptive"))
                                .then_some("high")
                        })
                })
                .map(str::to_owned)
        })
        .or_else(|| {
            capabilities
                .default_reasoning_effort
                .as_deref()
                .and_then(|effort| normalize_gemini_thinking_level(effort, model))
                .map(str::to_owned)
        })
}

fn normalize_gemini_thinking_level<'a>(effort: &'a str, model: &str) -> Option<&'a str> {
    match effort {
        "none" | "minimal" if model.starts_with("gemini-3") => Some("low"),
        "none" | "minimal" | "low" => Some(effort),
        "medium" => Some("medium"),
        "high" | "xhigh" | "max" => Some("high"),
        _ => None,
    }
}

fn interaction_response_format(request: &Value) -> Result<Option<Value>, String> {
    let Some(format) = request
        .pointer("/output_config/format")
        .or_else(|| request.get("output_format"))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let format_type = format
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "Anthropic output format must have a string 'type'".to_string())?;
    if format_type == "text" {
        return Ok(None);
    }
    if format_type != "json_schema" {
        return Err(format!(
            "Unsupported Anthropic output format '{format_type}' for Gemini Interactions"
        ));
    }
    let mut schema = format
        .get("schema")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| {
            "Anthropic json_schema output format requires an object 'schema'".to_string()
        })?;
    sanitize_json_schema(&mut schema);
    Ok(Some(json!({
        "type": "text",
        "mime_type": "application/json",
        "schema": schema
    })))
}

fn interaction_service_tier(request: &Value) -> Option<&'static str> {
    match request.get("service_tier").and_then(Value::as_str) {
        Some("standard_only") => Some("standard"),
        _ => None,
    }
}

fn json_contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Array(values) => values.iter().any(|value| json_contains_key(value, key)),
        Value::Object(object) => {
            object.contains_key(key) || object.values().any(|value| json_contains_key(value, key))
        }
        _ => false,
    }
}

fn gemini_interaction_request_diagnostics(request: &Value) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut add = |message: &str| {
        if !diagnostics.iter().any(|existing| existing == message) {
            diagnostics.push(message.to_string());
        }
    };

    for field in [
        "metadata",
        "container",
        "context_management",
        "inference_geo",
        "mcp_servers",
    ] {
        if request.get(field).is_some_and(|value| !value.is_null()) {
            add(&format!(
                "Ignored Anthropic field '{field}': Gemini Interactions has no equivalent on this model transport"
            ));
        }
    }
    let ignored_sampling: Vec<&str> = ["temperature", "top_p", "top_k"]
        .into_iter()
        .filter(|field| request.get(*field).is_some())
        .collect();
    if !ignored_sampling.is_empty() {
        add(&format!(
            "Ignored Anthropic sampling fields for this Gemini thinking profile: {}",
            ignored_sampling.join(", ")
        ));
    }
    if request.get("candidate_count").is_some() {
        add("Ignored candidate_count: Gemini 3.x supports a single response candidate");
    }
    if let Some(effort) = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
    {
        match effort {
            "none" | "minimal" => add(&format!(
                "Mapped Anthropic output_config.effort '{effort}' to Gemini 3.7 minimum thinking level 'low'"
            )),
            "low" | "medium" | "high" | "xhigh" | "max" => {}
            _ => add(&format!(
                "Ignored unsupported Anthropic output_config.effort '{effort}'"
            )),
        }
    }
    if let Some(tier) = request.get("service_tier").and_then(Value::as_str) {
        match tier {
            "standard_only" => {}
            "auto" => add(
                "Anthropic service_tier 'auto' is left unset; Gemini uses its default standard tier",
            ),
            _ => add(&format!(
                "Ignored unsupported Anthropic service_tier '{tier}'"
            )),
        }
    }
    if request
        .pointer("/tool_choice/disable_parallel_tool_use")
        .and_then(Value::as_bool)
        == Some(true)
    {
        add("Ignored Anthropic disable_parallel_tool_use: Gemini Interactions has no equivalent control");
    }
    for field in [
        "cache_control",
        "citations",
        "defer_loading",
        "input_examples",
        "allowed_callers",
        "eager_input_streaming",
    ] {
        if json_contains_key(request, field) {
            add(&format!(
                "Ignored Anthropic extension '{field}' while translating to Gemini Interactions"
            ));
        }
    }
    diagnostics
}

fn attach_bridge_diagnostics(
    mut response: Response,
    provider_file: &str,
    diagnostics: &[String],
) -> Response {
    for diagnostic in diagnostics {
        warn!(
            provider = provider_file,
            diagnostic, "Provider request compatibility downgrade"
        );
        if let Ok(value) = HeaderValue::from_str(diagnostic) {
            response.headers_mut().append(BRIDGE_WARNING_HEADER, value);
        }
    }
    response
}

fn deepseek_effort_mapping_diagnostic(request: &Value) -> Option<String> {
    let effort = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)?;
    match effort {
        "none" => Some(
            "Mapped Anthropic output_config.effort 'none' to DeepSeek thinking.type=disabled because DeepSeek has no zero-effort reasoning tier"
                .to_string(),
        ),
        "minimal" | "low" => Some(format!(
            "Mapped Anthropic output_config.effort '{effort}' to DeepSeek reasoning_effort 'low'"
        )),
        "medium" => Some(
            "Mapped Anthropic output_config.effort 'medium' to DeepSeek reasoning_effort 'high'"
                .to_string(),
        ),
        "xhigh" => Some(
            "Mapped Anthropic output_config.effort 'xhigh' to DeepSeek reasoning_effort 'max'"
                .to_string(),
        ),
        _ => None,
    }
}

fn anthropic_max_tokens_headroom_diagnostic(
    request: &Value,
    provider: &str,
    thinking_enabled: bool,
) -> Option<String> {
    if !thinking_enabled {
        return None;
    }
    let max_tokens = request.get("max_tokens").and_then(Value::as_u64)?;
    let budget = request
        .pointer("/thinking/budget_tokens")
        .and_then(Value::as_u64)?;
    if max_tokens > budget {
        return None;
    }
    Some(format!(
        "Raised {provider} Anthropic max_tokens from {max_tokens} to {} because the client's thinking budget exceeds max_tokens; the raise preserves visible-output headroom",
        budget.saturating_add(ANTHROPIC_THINKING_OUTPUT_HEADROOM_TOKENS)
    ))
}

fn deepseek_anthropic_reasoning_diagnostics(request: &Value) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = deepseek_effort_mapping_diagnostic(request) {
        diagnostics.push(diagnostic);
    }
    let policy = deepseek_reasoning_policy(request, &OpenAiCapabilities::default());
    if policy.thinking_enabled
        && request
            .get("tool_choice")
            .and_then(|choice| choice.get("type"))
            .and_then(Value::as_str)
            == Some("tool")
    {
        diagnostics.push(
            "Suppressed Anthropic tool_choice because DeepSeek thinking mode rejects a named tool preference; the model may still call the supplied tools automatically"
                .to_string(),
        );
    }
    if let Some(diagnostic) =
        anthropic_max_tokens_headroom_diagnostic(request, "DeepSeek", policy.thinking_enabled)
    {
        diagnostics.push(diagnostic);
    }
    diagnostics
}

fn qwen_effort_mapping_diagnostic(request: &Value) -> Option<String> {
    let effort = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)?;
    let policy = qwen_reasoning_policy(request, &OpenAiCapabilities::default());
    match (effort, policy.thinking_enabled, policy.effort) {
        ("none", false, _) => Some(
            "Mapped Anthropic output_config.effort 'none' to Qwen thinking.type=disabled"
                .to_string(),
        ),
        ("minimal", true, Some("low")) => Some(
            "Mapped Anthropic output_config.effort 'minimal' to Qwen reasoning effort 'low'"
                .to_string(),
        ),
        ("high", true, Some("medium")) => Some(
            "Mapped Anthropic output_config.effort 'high' to Qwen reasoning effort 'medium'; use xhigh/max only for maximum-intensity reasoning"
                .to_string(),
        ),
        ("max", true, Some("xhigh")) => Some(
            "Mapped Anthropic output_config.effort 'max' to Qwen reasoning effort 'xhigh'"
                .to_string(),
        ),
        _ => None,
    }
}

fn qwen_anthropic_reasoning_diagnostics(request: &Value) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if request.pointer("/thinking/type").and_then(Value::as_str) == Some("adaptive") {
        diagnostics.push(
            "Normalized Anthropic thinking.type 'adaptive' to Qwen thinking.type 'enabled'"
                .to_string(),
        );
    }
    if let Some(diagnostic) = qwen_effort_mapping_diagnostic(request) {
        diagnostics.push(diagnostic);
    }
    if request.pointer("/output_config/effort").is_none() {
        if let Some(budget) = request
            .pointer("/thinking/budget_tokens")
            .and_then(Value::as_u64)
        {
            let policy = qwen_reasoning_policy(request, &OpenAiCapabilities::default());
            diagnostics.push(format!(
                "Mapped Anthropic thinking budget {budget} to Qwen reasoning effort '{}'",
                policy.effort.unwrap_or("omitted")
            ));
        }
    }
    let policy = qwen_reasoning_policy(request, &OpenAiCapabilities::default());
    if let Some(diagnostic) =
        anthropic_max_tokens_headroom_diagnostic(request, "Qwen", policy.thinking_enabled)
    {
        diagnostics.push(diagnostic);
    }
    if request.pointer("/output_config/format").is_some()
        && !qwen_prompt_contains_json_keyword(request)
    {
        diagnostics.push(
            "Qwen structured output requires the system or messages content to contain the keyword 'JSON'; the upstream may reject this request"
                .to_string(),
        );
    }
    diagnostics
}

fn openai_request_diagnostics(
    request: &Value,
    capabilities: &OpenAiCapabilities,
    transport: ProviderTransport,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut add = |message: String| {
        if !diagnostics.iter().any(|existing| existing == &message) {
            diagnostics.push(message);
        }
    };
    for field in [
        "metadata",
        "container",
        "context_management",
        "inference_geo",
        "mcp_servers",
    ] {
        if request.get(field).is_some_and(|value| !value.is_null()) {
            add(format!(
                "Ignored Anthropic field '{field}': this provider transport has no equivalent mapping"
            ));
        }
    }
    if json_contains_key(request, "cache_control") {
        add("Ignored Anthropic cache_control blocks: provider caching is reported when available but cache placement is not controllable through this transport".to_string());
    }
    if json_contains_key(request, "citations") {
        add("Ignored Anthropic citations controls: this transport cannot preserve the Anthropic citation contract".to_string());
    }
    for field in [
        "defer_loading",
        "input_examples",
        "allowed_callers",
        "eager_input_streaming",
    ] {
        if json_contains_key(request, field) {
            add(format!(
                "Ignored Anthropic extension '{field}' while translating to {}",
                transport.as_str()
            ));
        }
    }
    if request.get("top_k").is_some() {
        add("Ignored Anthropic sampling field 'top_k': this provider transport has no equivalent mapping"
            .to_string());
    }
    if let Some(tier) = request.get("service_tier").and_then(Value::as_str) {
        add(format!(
            "Ignored Anthropic service_tier '{tier}': no safe provider-neutral tier mapping is configured"
        ));
    }
    if !capabilities.sampling_parameters {
        for field in ["temperature", "top_p"] {
            if request.get(field).is_some() {
                add(format!(
                    "Ignored Anthropic sampling field '{field}' for this reasoning profile"
                ));
            }
        }
    }
    if capabilities.max_tokens_field == MaxTokensField::Omit && request.get("max_tokens").is_some()
    {
        add(
            "Ignored Anthropic max_tokens because this profile omits provider token limits"
                .to_string(),
        );
    }
    if transport == ProviderTransport::OpenAiChat {
        if request.get("tools").and_then(Value::as_array).is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.starts_with("web_search_"))
            })
        }) {
            add(
                "Skipped Claude Code's server-side web_search tool: this OpenAI Chat transport cannot execute it natively; the provider's Anthropic transport performs server-side search"
                    .to_string(),
            );
        }
        if capabilities.chat_dialect == OpenAiChatDialect::DeepSeek {
            let policy = deepseek_reasoning_policy(request, capabilities);
            let named_tool_choice = request
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(Value::as_str)
                == Some("tool");
            if policy.thinking_enabled && named_tool_choice {
                add(
                    "Suppressed Anthropic tool_choice because DeepSeek thinking mode rejects a named tool preference; the model may still call the supplied tools automatically"
                        .to_string(),
                );
            }
            if let Some(diagnostic) = deepseek_effort_mapping_diagnostic(request) {
                add(diagnostic);
            }
        }
        if capabilities.chat_dialect == OpenAiChatDialect::Qwen {
            if let Some(diagnostic) = qwen_effort_mapping_diagnostic(request) {
                add(diagnostic);
            }
            let policy = qwen_reasoning_policy(request, capabilities);
            let chat_budget = qwen_chat_thinking_budget(policy);
            if policy.budget_tokens.is_some() && chat_budget != policy.budget_tokens {
                add(format!(
                    "Capped Qwen Chat thinking_budget from {} to {} for effective '{}' reasoning effort",
                    policy.budget_tokens.unwrap_or(0),
                    chat_budget.unwrap_or(0),
                    policy.effort.unwrap_or("disabled")
                ));
            }
            if request.pointer("/output_config/format").is_some()
                && !qwen_prompt_contains_json_keyword(request)
            {
                add("Qwen structured output requires the system or messages content to contain the keyword 'JSON'; the upstream may reject this request"
                    .to_string());
            }
        }
        if request
            .pointer("/output_config/format/type")
            .and_then(Value::as_str)
            == Some("json_schema")
            && capabilities.chat_dialect == OpenAiChatDialect::DeepSeek
        {
            add("Downgraded Anthropic json_schema output to DeepSeek Chat JSON object mode; schema enforcement is unavailable"
                .to_string());
        }
        if capabilities.chat_dialect == OpenAiChatDialect::Kimi {
            if request.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
                add(
                    "Kimi K3 always reasons; Anthropic thinking.type=disabled was ignored"
                        .to_string(),
                );
            }
            if let Some(effort) = request
                .pointer("/output_config/effort")
                .and_then(Value::as_str)
                .filter(|effort| !matches!(*effort, "low" | "high" | "max"))
            {
                add(format!(
                    "Mapped Anthropic output_config.effort '{effort}' to Kimi K3 reasoning_effort '{}'",
                    kimi_reasoning_effort(effort)
                ));
            }
        }
        if request.get("output_config").is_some()
            && capabilities.chat_dialect == OpenAiChatDialect::Generic
        {
            add(
                "Ignored Anthropic output_config fields not supported by the generic Chat profile"
                    .to_string(),
            );
        }
    } else if transport == ProviderTransport::OpenAiResponses {
        if request
            .pointer("/tool_choice/disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            add("Ignored Anthropic disable_parallel_tool_use: this Responses profile has no equivalent control"
                .to_string());
        }
        if request.pointer("/thinking/budget_tokens").is_some()
            && request.pointer("/output_config/effort").is_none()
        {
            if capabilities.chat_dialect == OpenAiChatDialect::Qwen {
                let (effort, _) = qwen_responses_reasoning_effort(request, capabilities);
                add(format!(
                    "Mapped Anthropic thinking budget to Qwen Responses reasoning.effort '{effort}'"
                ));
            } else {
                add("Anthropic thinking budget has no exact Responses equivalent; configure output_config.effort for deterministic reasoning control"
                    .to_string());
            }
        }
        if !capabilities.responses_stateful
            && request
                .get("messages")
                .and_then(Value::as_array)
                .is_some_and(|messages| messages.len() > 1)
        {
            add("Responses profile is stateless; full validated conversation history is replayed instead of previous_response_id"
                .to_string());
        }
    }
    diagnostics
}

fn interaction_continuation_for_request(
    profile_file: &str,
    request: &Value,
    messages: &[Value],
    continuations: &InteractionContinuationState,
) -> Option<(String, HashMap<String, String>, &'static str)> {
    let last = messages.last()?;
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let mut result_ids = Vec::new();
    if let Some(parts) = last.get("content").and_then(Value::as_array) {
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("tool_result") {
                if let Some(call_id) = part.get("tool_use_id").and_then(Value::as_str) {
                    result_ids.push(call_id.to_string());
                }
            }
        }
    }
    let cache = continuations.read().ok()?;
    if !result_ids.is_empty() {
        let mut interaction_id = None;
        let mut names = HashMap::new();
        let mut complete = true;
        for call_id in &result_ids {
            let Some(continuation) = cache
                .calls
                .get(&interaction_call_cache_key(profile_file, call_id))
            else {
                complete = false;
                break;
            };
            if interaction_id
                .as_ref()
                .is_some_and(|existing| existing != &continuation.interaction_id)
            {
                complete = false;
                break;
            }
            interaction_id = Some(continuation.interaction_id.clone());
            names.insert(call_id.clone(), continuation.name.clone());
        }
        if complete {
            if let Some(id) = interaction_id {
                return Some((id, names, "tool_call_id"));
            }
        }
    }
    if messages.len() <= 1 {
        return None;
    }
    let previous_messages = &messages[..messages.len() - 1];
    let key =
        interaction_transcript_cache_key(profile_file, request.get("system"), previous_messages);
    let names = interaction_tool_names_from_messages(previous_messages);
    cache
        .transcripts
        .get(&key)
        .cloned()
        .map(|id| (id, names, "transcript"))
}

fn translate_gemini_interactions_request(
    request: &Value,
    profile: &ProviderProfile,
    continuations: &InteractionContinuationState,
) -> Result<Value, String> {
    translate_gemini_interactions_request_with_continuation(request, profile, continuations, true)
}

fn translate_gemini_interactions_request_with_continuation(
    request: &Value,
    profile: &ProviderProfile,
    continuations: &InteractionContinuationState,
    allow_continuation: bool,
) -> Result<Value, String> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'messages'".to_string())?;
    if messages.is_empty() {
        return Err("Gemini Interactions requires at least one message".to_string());
    }
    if display_model_name(&profile.model).starts_with("gemini-3")
        && messages
        .last()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("assistant")
    {
        return Err(
            "Gemini 3.x does not support an assistant prefill; the final message must be user input or a tool result"
                .to_string(),
        );
    }
    let continuation = allow_continuation
        .then(|| {
            interaction_continuation_for_request(
                &profile.file_name,
                request,
                messages,
                continuations,
            )
        })
        .flatten();
    let text_tool_history =
        continuation.is_none() && interaction_messages_have_tool_history(messages);
    let input = if let Some((_, tool_names, _)) = &continuation {
        interaction_user_steps(
            messages
                .last()
                .and_then(|message| message.get("content"))
                .unwrap_or(&Value::Null),
            tool_names,
            false,
            profile.openai_capabilities.max_tool_result_chars,
        )
    } else {
        interaction_steps_from_messages(
            messages,
            text_tool_history,
            profile.openai_capabilities.max_tool_result_chars,
        )
    };
    if input.is_empty() {
        return Err("Gemini Interactions request produced no supported input steps".to_string());
    }

    let mut generation_config = Map::new();
    if let Some(max_tokens) = request.get("max_tokens").and_then(Value::as_u64) {
        generation_config.insert("max_output_tokens".to_string(), json!(max_tokens));
    }
    if let Some(stop_sequences) = request.get("stop_sequences").and_then(Value::as_array) {
        if !stop_sequences.is_empty() {
            generation_config.insert(
                "stop_sequences".to_string(),
                Value::Array(stop_sequences.clone()),
            );
        }
    }
    if let Some(level) = interaction_thinking_level(
        request,
        &profile.openai_capabilities,
        &display_model_name(&profile.model),
    ) {
        generation_config.insert("thinking_level".to_string(), json!(level));
    }
    generation_config.insert(
        "thinking_summaries".to_string(),
        json!(if profile.openai_capabilities.include_thoughts {
            "auto"
        } else {
            "none"
        }),
    );
    if let Some(choice) = request.get("tool_choice").and_then(interaction_tool_choice) {
        generation_config.insert("tool_choice".to_string(), choice);
    }

    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        json!(display_model_name(&profile.model)),
    );
    body.insert("input".to_string(), Value::Array(input));
    body.insert("store".to_string(), Value::Bool(true));
    body.insert(
        "stream".to_string(),
        Value::Bool(
            request
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    body.insert(
        "generation_config".to_string(),
        Value::Object(generation_config),
    );
    if let Some(response_format) = interaction_response_format(request)? {
        body.insert("response_format".to_string(), response_format);
    }
    if let Some(service_tier) = interaction_service_tier(request) {
        body.insert("service_tier".to_string(), json!(service_tier));
    }
    let tools = translated_interaction_tools(request, &profile.openai_capabilities);
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some((previous_id, _, continuation_kind)) = continuation {
        info!(
            provider = %profile.file_name,
            continuation = continuation_kind,
            "Continuing stored Gemini interaction"
        );
        body.insert("previous_interaction_id".to_string(), json!(previous_id));
    } else {
        let mut system = value_to_text(request.get("system").unwrap_or(&Value::Null));
        if text_tool_history {
            if !system.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(INTERACTION_TOOL_HISTORY_RECOVERY_INSTRUCTION);
        }
        if !system.is_empty() {
            body.insert("system_instruction".to_string(), json!(system));
        }
    }
    Ok(Value::Object(body))
}

fn interaction_request_has_mixed_tools(request: &Value) -> bool {
    let Some(tools) = request.get("tools").and_then(Value::as_array) else {
        return false;
    };
    let has_function = tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some("function"));
    let has_server_tool = tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) != Some("function"));
    has_function && has_server_tool
}

fn remove_interaction_server_tools(request: &mut Value) -> bool {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let original_len = tools.len();
    tools.retain(|tool| tool.get("type").and_then(Value::as_str) == Some("function"));
    original_len != tools.len()
}

fn is_mixed_interaction_tools_error(status: u16, message: &str) -> bool {
    if status != 400 {
        return false;
    }
    let message = message.to_ascii_lowercase();
    message.contains("include_server_side_tool_invocations")
        || (message.contains("server-side tool") && message.contains("function"))
        || (message.contains("built-in tool") && message.contains("function"))
}

fn is_interaction_continuation_not_implemented(status: u16, request: &Value) -> bool {
    status == 501 && request.get("previous_interaction_id").is_some()
}

struct InteractionResponseTranslation {
    message: Value,
    interaction_id: String,
    assistant_content: Vec<Value>,
    calls: Vec<(String, String)>,
}

fn is_gemini_server_tool_step(step_type: &str) -> bool {
    matches!(
        step_type,
        "google_search_call"
            | "google_search_result"
            | "url_context_call"
            | "url_context_result"
            | "code_execution_call"
            | "code_execution_result"
            | "google_maps_call"
            | "google_maps_result"
            | "file_search_call"
            | "file_search_result"
            | "mcp_server_tool_call"
            | "mcp_server_tool_result"
    )
}

#[derive(Default)]
struct InteractionServerToolTrace {
    steps: Vec<Value>,
    web_search_requests: u64,
    web_fetch_requests: u64,
}

impl InteractionServerToolTrace {
    fn capture(&mut self, step: &Value) {
        let Some(step_type) = step.get("type").and_then(Value::as_str) else {
            return;
        };
        if !is_gemini_server_tool_step(step_type) {
            return;
        }
        let key = interaction_server_tool_trace_key(step);
        let summary = interaction_server_tool_summary(step);
        if let Some(index) = key.as_ref().and_then(|key| {
            self.steps.iter().position(|existing| {
                interaction_server_tool_trace_key(existing).as_ref() == Some(key)
            })
        }) {
            self.steps[index] = summary;
            return;
        }
        if self.steps.len() >= INTERACTION_SERVER_TOOL_TRACE_CAPACITY {
            return;
        }
        if step_type == "google_search_call" {
            self.web_search_requests = self.web_search_requests.saturating_add(1);
        } else if step_type == "url_context_call" {
            self.web_fetch_requests = self.web_fetch_requests.saturating_add(1);
        }
        self.steps.push(summary);
    }

    fn provider_metadata(&self) -> Option<Value> {
        (!self.steps.is_empty()).then(|| {
            json!({
                "google": {
                    "interaction_server_tools": self.steps
                }
            })
        })
    }

    fn anthropic_usage(&self) -> Option<Value> {
        (self.web_search_requests > 0 || self.web_fetch_requests > 0).then(|| {
            json!({
                "web_search_requests": self.web_search_requests,
                "web_fetch_requests": self.web_fetch_requests
            })
        })
    }
}

fn interaction_server_tool_trace_key(step: &Value) -> Option<String> {
    let step_type = step.get("type").and_then(Value::as_str)?;
    let id = step
        .get("id")
        .or_else(|| step.get("call_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(format!("{step_type}\0{id}"))
}

fn bounded_interaction_trace_value(value: &Value) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    if serialized.chars().count() <= INTERACTION_SERVER_TOOL_TRACE_VALUE_CHARS {
        return value.clone();
    }
    let preview = serialized
        .chars()
        .take(INTERACTION_SERVER_TOOL_TRACE_VALUE_CHARS)
        .collect::<String>();
    json!({"truncated": true, "preview": preview})
}

fn interaction_server_tool_summary(step: &Value) -> Value {
    let mut summary = Map::new();
    for field in [
        "type",
        "id",
        "call_id",
        "name",
        "server_name",
        "status",
        "is_error",
        "search_type",
    ] {
        if let Some(value) = step.get(field) {
            summary.insert(field.to_string(), value.clone());
        }
    }
    for field in ["arguments", "action", "result", "results"] {
        if let Some(value) = step.get(field) {
            summary.insert(field.to_string(), bounded_interaction_trace_value(value));
        }
    }
    Value::Object(summary)
}

fn gemini_interaction_usage_to_anthropic(usage: &Value, fallback_input_tokens: u64) -> Value {
    let input_tokens = usage_token(
        usage,
        &["total_input_tokens", "prompt_tokens", "input_tokens"],
    )
    .unwrap_or(fallback_input_tokens);
    let visible_output_tokens = usage_token(
        usage,
        &["total_output_tokens", "completion_tokens", "output_tokens"],
    )
    .unwrap_or(0);
    let thought_tokens = usage_token(
        usage,
        &["total_thought_tokens", "total_reasoning_tokens", "reasoning_tokens"],
    );
    let mut translated = json!({
        "input_tokens": input_tokens,
        "output_tokens": visible_output_tokens.saturating_add(thought_tokens.unwrap_or(0))
    });
    if let Some(tokens) = thought_tokens {
        translated["reasoning_tokens"] = json!(tokens);
    }
    if let Some(tokens) = usage_token(usage, &["total_cached_tokens", "cached_tokens"]) {
        translated["cache_read_input_tokens"] = json!(tokens);
    }
    translated
}

fn translate_gemini_interactions_response(
    upstream: &Value,
    model: &str,
) -> Result<InteractionResponseTranslation, String> {
    let interaction_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            format!(
                "Gemini Interactions response has no id: {}",
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
    let steps = upstream
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| "Gemini Interactions response has no steps array".to_string())?;
    let mut content = Vec::new();
    let mut calls = Vec::new();
    let mut server_tools = InteractionServerToolTrace::default();
    for step in steps {
        match step.get("type").and_then(Value::as_str) {
            Some("thought") => {
                let thinking = value_to_text(step.get("summary").unwrap_or(&Value::Null));
                if thinking.is_empty() {
                    continue;
                }
                let mut block = json!({"type": "thinking", "thinking": thinking});
                if let Some(signature) = step.get("signature").and_then(Value::as_str) {
                    block["signature"] = json!(signature);
                }
                content.push(block);
            }
            Some("model_output") => {
                if let Some(parts) = step.get("content").and_then(Value::as_array) {
                    for part in parts {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    content.push(json!({"type": "text", "text": text}));
                                }
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = step
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
                let name = step
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_function")
                    .to_string();
                let input = step.get("arguments").cloned().unwrap_or_else(|| json!({}));
                calls.push((call_id.clone(), name.clone()));
                content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                }));
            }
            Some(step_type) if is_gemini_server_tool_step(step_type) => {
                server_tools.capture(step);
                info!(step_type, "Gemini server-side tool step completed");
            }
            _ => {}
        }
    }
    let usage = gemini_interaction_usage_to_anthropic(
        upstream.get("usage").unwrap_or(&Value::Null),
        0,
    );
    let stop_reason = if !calls.is_empty() || status == "requires_action" {
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
        "usage": usage
    });
    if let Some(usage) = server_tools.anthropic_usage() {
        message["usage"]["server_tool_use"] = usage;
    }
    if let Some(metadata) = server_tools.provider_metadata() {
        message["provider_metadata"] = metadata;
    }
    Ok(InteractionResponseTranslation {
        message,
        interaction_id,
        assistant_content: content,
        calls,
    })
}

async fn forward_gemini_interactions_profile(
    profile: ProviderProfile,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
) -> Response {
    let mut interaction_request =
        match translate_gemini_interactions_request(&request, &profile, &continuations) {
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
    let Some(api_key) = profile.api_key.as_ref() else {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "The active Gemini Interactions profile has no API key",
        );
    };
    let mut mixed_tools_fallback = false;
    let mut continuation_fallback = false;
    let upstream = loop {
        let response = match profile
            .client
            .post(&profile.upstream_url)
            .header("x-goog-api-key", api_key)
            .json(&interaction_request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                error!(
                    "Provider '{}' Gemini Interactions request to '{}' failed: {err}",
                    profile.file_name, profile.upstream_url
                );
                return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &err.to_string());
            }
        };
        let status = response.status();
        if status.is_success() {
            break response;
        }
        let response_text = match read_response_text_limited(response).await {
            Ok(text) => text,
            Err(err) => format!("Cannot read provider error response: {err}"),
        };
        let message = serde_json::from_str::<Value>(&response_text)
            .ok()
            .map(|value| safe_error_message(&value))
            .unwrap_or(response_text);

        if !mixed_tools_fallback
            && interaction_request_has_mixed_tools(&interaction_request)
            && is_mixed_interaction_tools_error(status.as_u16(), &message)
            && remove_interaction_server_tools(&mut interaction_request)
        {
            mixed_tools_fallback = true;
            warn!(
                provider = %profile.file_name,
                "Gemini rejected mixed function and server-side tools; retrying this request with Claude Code function tools only"
            );
            continue;
        }

        if !continuation_fallback
            && is_interaction_continuation_not_implemented(status.as_u16(), &interaction_request)
        {
            interaction_request = match translate_gemini_interactions_request_with_continuation(
                &request,
                &profile,
                &continuations,
                false,
            ) {
                Ok(value) => value,
                Err(translation_error) => {
                    return anthropic_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        &translation_error,
                    )
                }
            };
            if mixed_tools_fallback {
                remove_interaction_server_tools(&mut interaction_request);
            }
            continuation_fallback = true;
            warn!(
                provider = %profile.file_name,
                "Gemini did not implement this stored continuation; retrying with safe full-history recovery"
            );
            continue;
        }

        error!(
            "Provider '{}' Gemini Interactions returned HTTP {status}: {message}",
            profile.file_name
        );
        let response_status =
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let (response_status, error_type) = openai_error_contract(response_status, &message);
        return anthropic_error(response_status, error_type, &message);
    };
    let model = display_model_name(&profile.model);
    if stream_requested {
        return gemini_interactions_stream_response(
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
                &format!("Gemini Interactions returned invalid JSON: {err}"),
            )
        }
    };
    let translated = match translate_gemini_interactions_response(&upstream_body, &model) {
        Ok(value) => value,
        Err(message) => {
            error!(
                "Cannot translate provider '{}' Gemini Interactions response: {message}",
                profile.file_name
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &message);
        }
    };
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &request,
        &translated.interaction_id,
        &translated.assistant_content,
        &translated.calls,
    );
    Json(translated.message).into_response()
}
