fn gemini_count_token_parts(
    content: &Value,
    tool_names: &HashMap<String, String>,
    max_tool_result_chars: Option<u64>,
) -> Vec<Value> {
    let Some(parts) = content.as_array() else {
        return content
            .as_str()
            .filter(|text| !text.is_empty())
            .map(|text| vec![json!({"text": text})])
            .unwrap_or_default();
    };
    let mut translated = Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    translated.push(json!({"text": text}));
                }
            }
            Some("image" | "document") => {
                let Some(source) = part.get("source") else {
                    continue;
                };
                match source.get("type").and_then(Value::as_str) {
                    Some("base64") => translated.push(json!({
                        "inlineData": {
                            "mimeType": source.get("media_type").cloned().unwrap_or_else(|| json!("application/octet-stream")),
                            "data": source.get("data").cloned().unwrap_or(Value::Null)
                        }
                    })),
                    Some("url") => translated.push(json!({
                        "fileData": {
                            "mimeType": source.get("media_type").cloned().unwrap_or_else(|| {
                                if part.get("type").and_then(Value::as_str) == Some("document") {
                                    json!("application/pdf")
                                } else {
                                    json!("application/octet-stream")
                                }
                            }),
                            "fileUri": source.get("url").cloned().unwrap_or(Value::Null)
                        }
                    })),
                    Some("text") => {
                        if let Some(text) = source.get("data").and_then(Value::as_str) {
                            translated.push(json!({"text": text}));
                        }
                    }
                    Some("content") => {
                        let text = value_to_text(source.get("content").unwrap_or(&Value::Null));
                        if !text.is_empty() {
                            translated.push(json!({"text": text}));
                        }
                    }
                    _ => {}
                }
            }
            Some("tool_use") => {
                let Some(name) = part.get("name").and_then(Value::as_str) else {
                    continue;
                };
                translated.push(json!({
                    "functionCall": {
                        "name": name,
                        "args": part.get("input").cloned().unwrap_or_else(|| json!({}))
                    }
                }));
            }
            Some("tool_result") => {
                let call_id = part
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = tool_names
                    .get(call_id)
                    .map(String::as_str)
                    .unwrap_or("unknown_function");
                let translated_result = interaction_tool_result_value(
                    part.get("content").unwrap_or(&Value::Null),
                    max_tool_result_chars,
                );
                translated.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": {
                            "result": translated_result.result,
                            "is_error": part.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                        }
                    }
                }));
                for document in translated_result.documents {
                    if let Some(data) = document.get("data") {
                        translated.push(json!({
                            "inlineData": {
                                "mimeType": document
                                    .get("mime_type")
                                    .cloned()
                                    .unwrap_or_else(|| json!("application/pdf")),
                                "data": data
                            }
                        }));
                    } else if let Some(uri) = document.get("uri") {
                        translated.push(json!({
                            "fileData": {
                                "mimeType": document
                                    .get("mime_type")
                                    .cloned()
                                    .unwrap_or_else(|| json!("application/pdf")),
                                "fileUri": uri
                            }
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    translated
}

fn gemini_count_tokens_request(
    request: &Value,
    profile: &ProviderProfile,
) -> Result<Value, String> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'messages'".to_string())?;
    let tool_names = interaction_tool_names_from_messages(messages);
    let contents: Vec<Value> = messages
        .iter()
        .filter_map(|message| {
            let role = match message.get("role").and_then(Value::as_str) {
                Some("user") => "user",
                Some("assistant") => "model",
                _ => return None,
            };
            let parts = gemini_count_token_parts(
                message.get("content").unwrap_or(&Value::Null),
                &tool_names,
                profile.openai_capabilities.max_tool_result_chars,
            );
            (!parts.is_empty()).then(|| json!({"role": role, "parts": parts}))
        })
        .collect();
    if contents.is_empty() {
        return Err("Gemini token count request produced no supported contents".to_string());
    }

    let model = display_model_name(&profile.model);
    let model = model.strip_prefix("models/").unwrap_or(&model);
    let mut generate = Map::new();
    generate.insert("model".to_string(), json!(format!("models/{model}")));
    generate.insert("contents".to_string(), Value::Array(contents));
    let system = value_to_text(request.get("system").unwrap_or(&Value::Null));
    if !system.is_empty() {
        generate.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system}]}),
        );
    }
    let functions: Vec<Value> = translated_interaction_tools(request, &profile.openai_capabilities)?
        .into_iter()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("function"))
        .map(|tool| {
            let mut function = Map::new();
            for field in ["name", "description"] {
                if let Some(value) = tool.get(field) {
                    function.insert(field.to_string(), value.clone());
                }
            }
            if let Some(parameters) = tool.get("parameters") {
                function.insert("parameters".to_string(), parameters.clone());
            }
            Value::Object(function)
        })
        .collect();
    if !functions.is_empty() {
        generate.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": functions}]),
        );
    }
    if let Some(format) = interaction_response_format(request)? {
        generate.insert(
            "generationConfig".to_string(),
            json!({
                "responseMimeType": "application/json",
                "responseJsonSchema": format.get("schema").cloned().unwrap_or_else(|| json!({}))
            }),
        );
    }
    Ok(json!({"generateContentRequest": generate}))
}

fn gemini_count_tokens_url(profile: &ProviderProfile) -> Result<String, String> {
    let model = display_model_name(&profile.model);
    let model = model.strip_prefix("models/").unwrap_or(&model);
    if model.is_empty()
        || !model.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!(
            "Unsupported Gemini model id for token counting: {model}"
        ));
    }
    Ok(format!(
        "{}/models/{model}:countTokens",
        profile.base_url.trim_end_matches('/')
    ))
}

fn is_kimi_profile(profile: &ProviderProfile) -> bool {
    profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Kimi
        || inferred_openai_chat_dialect(&profile.base_url) == OpenAiChatDialect::Kimi
}

fn kimi_count_tokens_url(profile: &ProviderProfile) -> Result<String, String> {
    let mut url = url::Url::parse(&profile.base_url)
        .map_err(|error| format!("Invalid Kimi base_url '{}': {error}", profile.base_url))?;
    url.set_path("/v1/tokenizers/estimate-token-count");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn kimi_count_tokens_request(
    request: &Value,
    profile: &ProviderProfile,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    let capabilities = if profile.openai_capabilities.chat_dialect == OpenAiChatDialect::Kimi {
        profile.openai_capabilities.clone()
    } else {
        OpenAiCapabilities::for_openai_base_url(&profile.base_url)
    };
    let mut translated = translate_anthropic_request_with_capabilities(
        request,
        &display_model_name(&profile.model),
        thought_signatures,
        &capabilities,
    )?;
    let object = translated
        .as_object_mut()
        .ok_or_else(|| "Kimi token count request must be a JSON object".to_string())?;
    object.retain(|field, _| matches!(field.as_str(), "model" | "messages" | "tools"));
    Ok(translated)
}

fn anthropic_token_count_response(input_tokens: usize, source: &'static str) -> Response {
    let mut response = Json(json!({"input_tokens": input_tokens})).into_response();
    response.headers_mut().insert(
        "x-claude-bridge-token-count",
        HeaderValue::from_static(source),
    );
    response
}

async fn anthropic_count_tokens(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    let active_profile = active_provider_profile(&state);
    if let Some(profile) = active_profile.as_ref() {
        if let Some(identity) = upstream_identity_label(profile, &state.model) {
            if let Err(message) = append_bridge_identity(&mut request, &identity) {
                return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        }
    }

    let input_tokens = estimate_anthropic_input_tokens(&request);
    if let Some(profile) = active_profile
        .as_ref()
        .filter(|profile| is_kimi_profile(profile))
    {
        let diagnostics = openai_request_diagnostics(
            &request,
            &OpenAiCapabilities::for_openai_base_url(&profile.base_url),
            ProviderTransport::OpenAiChat,
        );
        let native_result = async {
            let credential = profile
                .auth_token
                .as_ref()
                .or(profile.api_key.as_ref())
                .ok_or_else(|| "Kimi profile has no credential".to_string())?;
            let url = kimi_count_tokens_url(profile)?;
            let body =
                kimi_count_tokens_request(&request, profile, state.thought_signatures.as_ref())?;
            let response = profile
                .client
                .post(url)
                .bearer_auth(credential)
                .json(&body)
                .send()
                .await
                .map_err(|error| format!("Kimi estimate-token-count request failed: {error}"))?;
            let status = response.status();
            let body = read_response_json_limited(response)
                .await
                .map_err(|error| format!("Cannot read Kimi token count response: {error}"))?;
            if !status.is_success() {
                return Err(format!(
                    "Kimi estimate-token-count returned HTTP {status}: {}",
                    safe_error_message(&body)
                ));
            }
            body.pointer("/data/total_tokens")
                .or_else(|| body.get("total_tokens"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    "Kimi token count response has no valid data.total_tokens".to_string()
                })
        };
        let response = match tokio::time::timeout(KIMI_COUNT_TOKENS_TIMEOUT, native_result).await {
            Ok(Ok(native_tokens)) => anthropic_token_count_response(native_tokens, "kimi-native"),
            Ok(Err(message)) => {
                warn!(
                    provider = %profile.file_name,
                    error = message,
                    "Falling back to estimated Kimi input token count"
                );
                anthropic_token_count_response(input_tokens, "estimated-fallback")
            }
            Err(_) => {
                warn!(
                    provider = %profile.file_name,
                    "Kimi estimate-token-count timed out; falling back to estimated input token count"
                );
                anthropic_token_count_response(input_tokens, "estimated-fallback")
            }
        };
        return attach_bridge_diagnostics(response, &profile.file_name, &diagnostics);
    }
    let Some(mut profile) =
        active_profile.filter(|profile| profile.transport == ProviderTransport::GeminiInteractions)
    else {
        return anthropic_token_count_response(input_tokens, "estimated");
    };
    let diagnostics = gemini_interaction_request_diagnostics(&request);
    let credential_result = apply_bridge_managed_gemini_credentials(&state, &mut profile);
    let native_result = async {
        credential_result?;
        let api_key = profile
            .api_key
            .as_ref()
            .ok_or_else(|| "Gemini Interactions profile has no Google credential".to_string())?;
        let url = gemini_count_tokens_url(&profile)?;
        let body = gemini_count_tokens_request(&request, &profile)?;
        let response = profile
            .client
            .post(url)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("Gemini countTokens request failed: {error}"))?;
        let status = response.status();
        let body = read_response_json_limited(response)
            .await
            .map_err(|error| format!("Cannot read Gemini countTokens response: {error}"))?;
        if !status.is_success() {
            return Err(format!(
                "Gemini countTokens returned HTTP {status}: {}",
                safe_error_message(&body)
            ));
        }
        body.get("totalTokens")
            .or_else(|| body.get("total_tokens"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Gemini countTokens response has no valid totalTokens".to_string())
    };
    let response = match tokio::time::timeout(GEMINI_COUNT_TOKENS_TIMEOUT, native_result).await {
        Ok(Ok(native_tokens)) => anthropic_token_count_response(native_tokens, "google-native"),
        Ok(Err(message)) => {
            warn!(
                provider = %profile.file_name,
                error = message,
                "Falling back to estimated Anthropic input token count"
            );
            anthropic_token_count_response(input_tokens, "estimated-fallback")
        }
        Err(_) => {
            warn!(
                provider = %profile.file_name,
                "Gemini countTokens timed out; falling back to estimated Anthropic input token count"
            );
            anthropic_token_count_response(input_tokens, "estimated-fallback")
        }
    };
    attach_bridge_diagnostics(response, &profile.file_name, &diagnostics)
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

fn openai_error_contract(status: StatusCode, message: &str) -> (StatusCode, &'static str) {
    let status_error = match status.as_u16() {
        401 => Some("authentication_error"),
        403 => Some("permission_error"),
        404 => Some("not_found_error"),
        429 => Some("rate_limit_error"),
        529 => Some("overloaded_error"),
        _ => None,
    };
    if let Some(error_type) = status_error {
        return (status, error_type);
    }

    let lower = message.to_ascii_lowercase();
    let context_limit = [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "max context",
        "prompt is too long",
        "prompt too long",
        "too many tokens",
        "token limit",
        "input is too long",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if context_limit {
        let status = if status.is_client_error() {
            status
        } else {
            StatusCode::BAD_REQUEST
        };
        return (status, "invalid_request_error");
    }

    let error_type = match status.as_u16() {
        400..=499 => "invalid_request_error",
        _ => "api_error",
    };
    (status, error_type)
}
