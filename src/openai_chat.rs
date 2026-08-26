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
    let mut cache = match thought_signatures.write() {
        Ok(cache) => cache,
        Err(poisoned) => {
            error!("Thought-signature cache write lock was poisoned; recovering cached state");
            poisoned.into_inner()
        }
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

fn recalled_thought_signature(
    thought_signatures: &ThoughtSignatureCache,
    call_id: &str,
) -> Option<String> {
    let cache = match thought_signatures.read() {
        Ok(cache) => cache,
        Err(poisoned) => {
            error!("Thought-signature cache read lock was poisoned; recovering cached state");
            poisoned.into_inner()
        }
    };
    cache.get(call_id).cloned()
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

#[cfg(test)]
fn translate_anthropic_request(
    request: &Value,
    default_model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    translate_anthropic_request_with_capabilities(
        request,
        default_model,
        thought_signatures,
        &OpenAiCapabilities::local_gemini(),
    )
}

fn translate_anthropic_request_with_capabilities(
    request: &Value,
    default_model: &str,
    thought_signatures: &ThoughtSignatureCache,
    capabilities: &OpenAiCapabilities,
) -> Result<Value, String> {
    let mut system_parts = Vec::new();
    let mut messages = Vec::new();
    let mut pending_tool_call_ids = HashSet::new();
    let mut replayed_reasoning = false;
    // Server-side tools (web_search_*) are dropped during tool translation,
    // so the DeepSeek reasoning replay decision must use the tools that
    // actually reach upstream. Basing it on the raw request would replay
    // every historical reasoning block even when upstream receives no tools
    // at all, needlessly inflating context.
    let translated_tools: Vec<Value> = request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| translate_anthropic_tool_with_capabilities(tool, capabilities))
                .collect()
        })
        .unwrap_or_default();
    let deepseek_request_has_tools =
        capabilities.chat_dialect == OpenAiChatDialect::DeepSeek && !translated_tools.is_empty();

    if let Some(system) = request.get("system") {
        let text = value_to_text(system);
        if !text.is_empty() {
            system_parts.push(text);
        }
    }

    let source_messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required array field 'messages'".to_string())?;
    let active_task_start = if capabilities.reasoning_replay_scope
        == ReasoningReplayScope::ActiveTask
    {
        source_messages.iter().rposition(|message| {
            message.get("role").and_then(Value::as_str) == Some("user")
                && is_genuine_anthropic_user_task(message.get("content").unwrap_or(&Value::Null))
        })
    } else {
        None
    };

    for (message_index, message) in source_messages.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "Each message must have a role".to_string())?;
        let content = message.get("content").unwrap_or(&Value::Null);

        match role {
            "assistant" => {
                let replay_reasoning = match capabilities.reasoning_replay_scope {
                    ReasoningReplayScope::None => false,
                    ReasoningReplayScope::All => true,
                    ReasoningReplayScope::ActiveTask => {
                        active_task_start.is_some_and(|start| message_index > start)
                    }
                };
                replayed_reasoning |= translate_anthropic_assistant_message(
                    content,
                    &mut messages,
                    thought_signatures,
                    &mut pending_tool_call_ids,
                    capabilities.chat_dialect,
                    replay_reasoning,
                    deepseek_request_has_tools,
                );
            }
            "user" => translate_anthropic_user_message(
                content,
                &mut messages,
                capabilities,
                &mut pending_tool_call_ids,
            ),
            "system" | "developer" => {
                let text = value_to_text(content);
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            _ => return Err(format!("Unsupported Anthropic message role '{role}'")),
        }
    }

    if !system_parts.is_empty() {
        messages.insert(
            0,
            json!({"role": "system", "content": system_parts.join("\n\n")}),
        );
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
    if stream_requested && capabilities.stream_options {
        body.insert("stream_options".to_string(), json!({"include_usage": true}));
    }

    if let Some(max_tokens) = request.get("max_tokens").and_then(value_as_u64) {
        match capabilities.max_tokens_field {
            MaxTokensField::MaxTokens => {
                body.insert("max_tokens".to_string(), json!(max_tokens));
            }
            MaxTokensField::MaxCompletionTokens => {
                body.insert("max_completion_tokens".to_string(), json!(max_tokens));
            }
            MaxTokensField::Omit => {}
        }
    }
    if capabilities.sampling_parameters {
        if let Some(temperature) = request.get("temperature").and_then(Value::as_f64) {
            body.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = request.get("top_p").and_then(Value::as_f64) {
            body.insert("top_p".to_string(), json!(top_p));
        }
    }
    if let Some(stop_sequences) = request.get("stop_sequences").and_then(Value::as_array) {
        if !stop_sequences.is_empty() {
            body.insert("stop".to_string(), Value::Array(stop_sequences.clone()));
        }
    }

    if !translated_tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(translated_tools));
    }

    let deepseek_policy = (capabilities.chat_dialect == OpenAiChatDialect::DeepSeek)
        .then(|| deepseek_reasoning_policy(request, capabilities));
    let thinking_enabled = deepseek_policy
        .map(|policy| policy.thinking_enabled)
        .unwrap_or_else(|| {
            request
                .pointer("/thinking/type")
                .and_then(Value::as_str)
                .map(|kind| kind != "disabled")
                .unwrap_or(false)
        });

    // DeepSeek V4 thinking mode accepts tool_choice auto/any/none but rejects
    // a named tool preference with HTTP 400 ("Thinking mode does not support
    // this tool_choice", verified live). Drop only the named form and let the
    // model use the supplied tools automatically.
    let suppress_tool_choice = capabilities.chat_dialect == OpenAiChatDialect::DeepSeek
        && thinking_enabled
        && request
            .get("tool_choice")
            .and_then(|choice| choice.get("type"))
            .and_then(Value::as_str)
            == Some("tool");
    if !suppress_tool_choice {
        if let Some(choice) = request.get("tool_choice") {
            if let Some(translated) = translate_anthropic_tool_choice(choice) {
                body.insert("tool_choice".to_string(), translated);
            }
        }
    }
    if let Some(choice) = request.get("tool_choice") {
        if capabilities.parallel_tool_calls
            && choice
                .get("disable_parallel_tool_use")
                .and_then(Value::as_bool)
                == Some(true)
        {
            body.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        }
    }

    match capabilities.chat_dialect {
        OpenAiChatDialect::DeepSeek => {
            let policy = deepseek_policy.expect("DeepSeek policy must be available");
            let thinking_type = if policy.thinking_enabled {
                "enabled"
            } else {
                "disabled"
            };
            body.insert("thinking".to_string(), json!({"type": thinking_type}));
            if capabilities.reasoning_effort && policy.thinking_enabled {
                if let Some(effort) = policy.effort {
                    body.insert("reasoning_effort".to_string(), json!(effort));
                }
            }
            // DeepSeek user_id isolates KVCache and per-user concurrency
            // quotas. Prefer a validated client value, then the profile
            // default, and drop values that violate [a-zA-Z0-9_-]{1,512}.
            if let Some(user_id) = request
                .pointer("/metadata/user_id")
                .and_then(Value::as_str)
                .and_then(validated_user_id)
                .or_else(|| capabilities.user_id.clone())
            {
                body.insert("user_id".to_string(), json!(user_id));
            }
            if let Some(format) = openai_chat_response_format(request, true)? {
                body.insert("response_format".to_string(), format);
            }
        }
        OpenAiChatDialect::Qwen => {
            let policy = qwen_reasoning_policy(request, capabilities);
            body.insert(
                "enable_thinking".to_string(),
                Value::Bool(policy.thinking_enabled),
            );
            if let Some(budget) = qwen_chat_thinking_budget(policy) {
                body.insert("thinking_budget".to_string(), json!(budget));
            }
            if policy.thinking_enabled && replayed_reasoning {
                body.insert("preserve_thinking".to_string(), Value::Bool(true));
            }
            if let Some(format) = openai_chat_response_format(request, false)? {
                body.insert("response_format".to_string(), format);
            }
        }
        OpenAiChatDialect::Kimi => {
            let effort = request
                .pointer("/output_config/effort")
                .and_then(Value::as_str)
                .map(kimi_reasoning_effort)
                .or_else(|| {
                    request
                        .pointer("/thinking/budget_tokens")
                        .and_then(Value::as_u64)
                        .map(kimi_reasoning_effort_from_budget)
                })
                .or_else(|| {
                    capabilities
                        .default_reasoning_effort
                        .as_deref()
                        .map(kimi_reasoning_effort)
                })
                .unwrap_or("max");
            if capabilities.reasoning_effort {
                body.insert("reasoning_effort".to_string(), json!(effort));
            }
            if let Some(format) = openai_chat_response_format(request, false)? {
                body.insert("response_format".to_string(), format);
            }
            body.insert(
                "prompt_cache_key".to_string(),
                json!(kimi_prompt_cache_key(request)),
            );
            if let Some(identifier) = kimi_safety_identifier(request) {
                body.insert("safety_identifier".to_string(), json!(identifier));
            }
        }
        OpenAiChatDialect::Generic => {}
    }

    if capabilities.reasoning_effort && capabilities.chat_dialect == OpenAiChatDialect::Generic {
        let thinking = request.get("thinking");
        let disabled = thinking
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            == Some("disabled");
        let effort = if disabled {
            capabilities
                .reasoning_effort_map
                .contains_key("none")
                .then_some("none")
        } else {
            request
                .pointer("/output_config/effort")
                .and_then(Value::as_str)
                .or_else(|| {
                    thinking
                        .and_then(|value| value.get("budget_tokens"))
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
                })
                .or_else(|| {
                    thinking
                        .and_then(|value| value.get("type"))
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind == "adaptive")
                        .then_some("high")
                })
                .or(capabilities.default_reasoning_effort.as_deref())
        };
        if let Some(effort) = effort {
            let effort = capabilities
                .reasoning_effort_map
                .get(effort)
                .map(String::as_str)
                .unwrap_or(effort);
            body.insert("reasoning_effort".to_string(), json!(effort));
        }
    }
    if capabilities.include_thoughts {
        let mut thinking_config = json!({"include_thoughts": true});
        if let Some(effort) = body.remove("reasoning_effort") {
            thinking_config["thinking_level"] = effort;
        }
        body.insert(
            "extra_body".to_string(),
            json!({"google": {"thinking_config": thinking_config}}),
        );
    }

    Ok(Value::Object(body))
}

fn is_genuine_anthropic_user_task(content: &Value) -> bool {
    if let Some(text) = content.as_str() {
        return !text.trim().is_empty();
    }

    let Some(parts) = content.as_array() else {
        return false;
    };
    if parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))
    {
        return false;
    }

    parts
        .iter()
        .any(|part| match part.get("type").and_then(Value::as_str) {
            Some("text") => part
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
            Some("image" | "document") => true,
            _ => false,
        })
}

fn translate_anthropic_assistant_message(
    content: &Value,
    messages: &mut Vec<Value>,
    thought_signatures: &ThoughtSignatureCache,
    pending_tool_call_ids: &mut HashSet<String>,
    chat_dialect: OpenAiChatDialect,
    reasoning_replay: bool,
    deepseek_request_has_tools: bool,
) -> bool {
    pending_tool_call_ids.clear();
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            messages.push(json!({"role": "assistant", "content": text}));
        }
        return false;
    }

    let Some(parts) = content.as_array() else {
        return false;
    };
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut assistant_signature = None;
    let mut reasoning_parts = Vec::new();

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
                pending_tool_call_ids.insert(call_id.to_string());
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
                if let Some(signature) = recalled_thought_signature(thought_signatures, call_id) {
                    translated["extra_content"] = json!({
                        "google": {"thought_signature": signature}
                    });
                }
                tool_calls.push(translated);
            }
            Some("thinking") => {
                if let Some(thinking) = part.get("thinking").and_then(Value::as_str) {
                    if !thinking.is_empty() {
                        reasoning_parts.push(thinking.to_string());
                    }
                }
                assistant_signature = part
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("redacted_thinking") => {
                assistant_signature = part.get("data").and_then(Value::as_str).map(str::to_owned);
            }
            _ => {}
        }
    }

    if !text_parts.is_empty() || !tool_calls.is_empty() {
        let mut translated = json!({
            "role": "assistant",
            "content": if text_parts.is_empty() {
                Value::Null
            } else {
                Value::String(text_parts.join("\n"))
            }
        });
        let has_tool_calls = !tool_calls.is_empty();
        if has_tool_calls {
            translated["tool_calls"] = Value::Array(tool_calls);
        } else if let Some(signature) = assistant_signature {
            translated["extra_content"] = json!({"google": {"thought_signature": signature}});
        }
        let replay_reasoning = !reasoning_parts.is_empty()
            && (reasoning_replay || chat_dialect != OpenAiChatDialect::Generic)
            && (chat_dialect != OpenAiChatDialect::DeepSeek
                || deepseek_request_has_tools
                || has_tool_calls);
        if replay_reasoning {
            translated["reasoning_content"] = Value::String(reasoning_parts.join("\n"));
        }
        messages.push(translated);
        return replay_reasoning;
    }

    false
}

const DEEPSEEK_MAX_EFFORT_BUDGET_TOKENS: u64 = 32_768;
const ANTHROPIC_THINKING_OUTPUT_HEADROOM_TOKENS: u64 = 8_192;

fn ensure_anthropic_thinking_output_headroom(
    request: &mut serde_json::Map<String, Value>,
    thinking_enabled: bool,
    provider: &str,
) -> Result<(), String> {
    if !thinking_enabled {
        return Ok(());
    }
    let Some(budget) = request
        .get("thinking")
        .and_then(|thinking| thinking.get("budget_tokens"))
        .and_then(value_as_u64)
    else {
        return Ok(());
    };
    let Some(max_tokens) = request.get("max_tokens").and_then(value_as_u64) else {
        return Ok(());
    };
    if max_tokens > budget {
        return Ok(());
    }
    // DeepSeek ignores budget_tokens upstream, but a client that sent
    // max_tokens <= budget_tokens still expects visible output beyond its
    // thinking budget; raising max_tokens preserves that headroom.
    let required_max_tokens = budget
        .checked_add(ANTHROPIC_THINKING_OUTPUT_HEADROOM_TOKENS)
        .ok_or_else(|| {
            format!(
                "{provider} thinking.budget_tokens is too large to keep max_tokens above the budget with output headroom"
            )
        })?;
    request.insert("max_tokens".to_string(), json!(required_max_tokens));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeepSeekReasoningPolicy {
    thinking_enabled: bool,
    effort: Option<&'static str>,
    source: &'static str,
}

fn deepseek_reasoning_policy(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> DeepSeekReasoningPolicy {
    if request.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
        return DeepSeekReasoningPolicy {
            thinking_enabled: false,
            effort: None,
            source: "thinking.type=disabled",
        };
    }

    let effort_policy = |effort: &str, source| match effort {
        "none" => DeepSeekReasoningPolicy {
            thinking_enabled: false,
            effort: None,
            source,
        },
        // DeepSeek V4 (flash and pro, verified live) accepts a "low"
        // reasoning tier. Keep thinking on for minimal/low requests so
        // simple turns still get the cheapest reasoning tier.
        "minimal" | "low" => DeepSeekReasoningPolicy {
            thinking_enabled: true,
            effort: Some("low"),
            source,
        },
        "xhigh" | "max" => DeepSeekReasoningPolicy {
            thinking_enabled: true,
            effort: Some("max"),
            source,
        },
        _ => DeepSeekReasoningPolicy {
            thinking_enabled: true,
            effort: Some("high"),
            source,
        },
    };

    if let Some(effort) = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
    {
        return effort_policy(effort, "output_config.effort");
    }
    if let Some(budget) = request
        .pointer("/thinking/budget_tokens")
        .and_then(Value::as_u64)
    {
        // DeepSeek's Anthropic endpoint ignores budget_tokens (verified
        // live: a 64-token budget still produced full reasoning), so the
        // bridge is the only place the budget has meaning. Translate it
        // into effort so Claude Code's thinking intensity is preserved.
        return DeepSeekReasoningPolicy {
            thinking_enabled: true,
            effort: Some(if budget >= DEEPSEEK_MAX_EFFORT_BUDGET_TOKENS {
                "max"
            } else {
                "high"
            }),
            source: "thinking.budget_tokens",
        };
    }
    if let Some(effort) = capabilities.default_reasoning_effort.as_deref() {
        return effort_policy(effort, "profile default_reasoning_effort");
    }
    DeepSeekReasoningPolicy {
        thinking_enabled: true,
        effort: Some("high"),
        source: "DeepSeek default",
    }
}

fn apply_deepseek_anthropic_reasoning_policy(
    request: &mut Value,
    capabilities: &OpenAiCapabilities,
) -> Result<DeepSeekReasoningPolicy, String> {
    let policy = deepseek_reasoning_policy(request, capabilities);
    let request = request
        .as_object_mut()
        .ok_or_else(|| "Anthropic request body must be a JSON object".to_string())?;

    let thinking = request
        .entry("thinking".to_string())
        .or_insert_with(|| json!({}));
    let Some(thinking) = thinking.as_object_mut() else {
        return Err("Anthropic field 'thinking' must be an object".to_string());
    };
    thinking.insert(
        "type".to_string(),
        json!(if policy.thinking_enabled {
            "enabled"
        } else {
            "disabled"
        }),
    );
    if !policy.thinking_enabled {
        thinking.remove("budget_tokens");
    }

    if capabilities.reasoning_effort && policy.thinking_enabled {
        let output_config = request
            .entry("output_config".to_string())
            .or_insert_with(|| json!({}));
        let Some(output_config) = output_config.as_object_mut() else {
            return Err("Anthropic field 'output_config' must be an object".to_string());
        };
        if let Some(effort) = policy.effort {
            output_config.insert("effort".to_string(), json!(effort));
        }
    } else if let Some(output_config) = request.get_mut("output_config") {
        let Some(output_config) = output_config.as_object_mut() else {
            return Err("Anthropic field 'output_config' must be an object".to_string());
        };
        output_config.remove("effort");
        if output_config.is_empty() {
            request.remove("output_config");
        }
    }

    // DeepSeek V4 thinking mode rejects a named tool preference with HTTP 400
    // ("Thinking mode does not support this tool_choice", verified live) but
    // accepts auto/any/none. Strip only the named form so the model can still
    // call the supplied tools automatically.
    if policy.thinking_enabled
        && request
            .get("tool_choice")
            .and_then(|choice| choice.get("type"))
            .and_then(Value::as_str)
            == Some("tool")
    {
        request.remove("tool_choice");
    }

    ensure_anthropic_thinking_output_headroom(request, policy.thinking_enabled, "DeepSeek")?;

    Ok(policy)
}

const QWEN_LOW_CHAT_BUDGET_TOKENS: u64 = 4_096;
const QWEN_MEDIUM_CHAT_BUDGET_TOKENS: u64 = 16_384;
const QWEN_LOW_EFFORT_BUDGET_THRESHOLD: u64 = 8_192;
// Claude Code's strongest thinking trigger uses a 31,999-token budget (it must
// stay below max_tokens), so the xhigh threshold sits at exactly that value;
// otherwise the strongest Claude turn could never reach Qwen's maximum effort.
const QWEN_XHIGH_EFFORT_BUDGET_THRESHOLD: u64 = 31_999;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QwenReasoningPolicy {
    thinking_enabled: bool,
    effort: Option<&'static str>,
    budget_tokens: Option<u64>,
    source: &'static str,
}

fn qwen_reasoning_policy(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> QwenReasoningPolicy {
    if request.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
        return QwenReasoningPolicy {
            thinking_enabled: false,
            effort: None,
            budget_tokens: None,
            source: "thinking.type=disabled",
        };
    }

    let budget_tokens = request
        .pointer("/thinking/budget_tokens")
        .and_then(Value::as_u64);
    let effort_policy = |effort: &str, source| match effort {
        "none" => QwenReasoningPolicy {
            thinking_enabled: false,
            effort: None,
            budget_tokens: None,
            source,
        },
        "minimal" | "low" => QwenReasoningPolicy {
            thinking_enabled: true,
            effort: Some("low"),
            budget_tokens,
            source,
        },
        "xhigh" | "max" => QwenReasoningPolicy {
            thinking_enabled: true,
            effort: Some("xhigh"),
            budget_tokens,
            source,
        },
        _ => QwenReasoningPolicy {
            thinking_enabled: true,
            effort: Some("medium"),
            budget_tokens,
            source,
        },
    };

    if let Some(effort) = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
    {
        return effort_policy(effort, "output_config.effort");
    }
    if let Some(budget) = budget_tokens {
        return QwenReasoningPolicy {
            thinking_enabled: true,
            effort: Some(if budget < QWEN_LOW_EFFORT_BUDGET_THRESHOLD {
                "low"
            } else if budget < QWEN_XHIGH_EFFORT_BUDGET_THRESHOLD {
                "medium"
            } else {
                "xhigh"
            }),
            budget_tokens: Some(budget),
            source: "thinking.budget_tokens",
        };
    }
    if let Some(effort) = capabilities.default_reasoning_effort.as_deref() {
        return effort_policy(effort, "profile default_reasoning_effort");
    }
    QwenReasoningPolicy {
        thinking_enabled: true,
        effort: Some("medium"),
        budget_tokens: None,
        source: "bridge default",
    }
}

fn qwen_chat_thinking_budget(policy: QwenReasoningPolicy) -> Option<u64> {
    match policy.effort {
        Some("low") => Some(
            policy
                .budget_tokens
                .unwrap_or(QWEN_LOW_CHAT_BUDGET_TOKENS)
                .min(QWEN_LOW_CHAT_BUDGET_TOKENS),
        ),
        Some("medium") => Some(
            policy
                .budget_tokens
                .unwrap_or(QWEN_MEDIUM_CHAT_BUDGET_TOKENS)
                .min(QWEN_MEDIUM_CHAT_BUDGET_TOKENS),
        ),
        Some("xhigh") => policy.budget_tokens,
        _ => None,
    }
}

fn apply_qwen_anthropic_reasoning_policy(
    request: &mut Value,
    capabilities: &OpenAiCapabilities,
) -> Result<QwenReasoningPolicy, String> {
    let policy = qwen_reasoning_policy(request, capabilities);
    let request = request
        .as_object_mut()
        .ok_or_else(|| "Anthropic request body must be a JSON object".to_string())?;

    let thinking = request
        .entry("thinking".to_string())
        .or_insert_with(|| json!({}));
    let Some(thinking) = thinking.as_object_mut() else {
        return Err("Anthropic field 'thinking' must be an object".to_string());
    };
    thinking.insert(
        "type".to_string(),
        json!(if policy.thinking_enabled {
            "enabled"
        } else {
            "disabled"
        }),
    );
    if !policy.thinking_enabled {
        thinking.remove("budget_tokens");
    }

    if capabilities.reasoning_effort && policy.thinking_enabled {
        let output_config = request
            .entry("output_config".to_string())
            .or_insert_with(|| json!({}));
        let Some(output_config) = output_config.as_object_mut() else {
            return Err("Anthropic field 'output_config' must be an object".to_string());
        };
        if let Some(effort) = policy.effort {
            output_config.insert("effort".to_string(), json!(effort));
        }
    } else if let Some(output_config) = request.get_mut("output_config") {
        let Some(output_config) = output_config.as_object_mut() else {
            return Err("Anthropic field 'output_config' must be an object".to_string());
        };
        output_config.remove("effort");
        if output_config.is_empty() {
            request.remove("output_config");
        }
    }

    ensure_anthropic_thinking_output_headroom(request, policy.thinking_enabled, "Qwen")?;

    Ok(policy)
}

fn qwen_responses_reasoning_effort(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> (&'static str, &'static str) {
    if request.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
        return ("none", "thinking.type=disabled");
    }
    if let Some(effort) = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
    {
        let effort = match effort {
            "none" => "none",
            "minimal" => "minimal",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" => "xhigh",
            "max" => "max",
            _ => "medium",
        };
        return (effort, "output_config.effort");
    }
    if let Some(budget) = request
        .pointer("/thinking/budget_tokens")
        .and_then(Value::as_u64)
    {
        let effort = if budget < 2_048 {
            "low"
        } else if budget < 8_192 {
            "medium"
        } else if budget < QWEN_XHIGH_EFFORT_BUDGET_THRESHOLD {
            "high"
        } else {
            "xhigh"
        };
        return (effort, "thinking.budget_tokens");
    }
    if let Some(effort) = capabilities.default_reasoning_effort.as_deref() {
        let effort = match effort {
            "none" => "none",
            "minimal" => "minimal",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" => "xhigh",
            "max" => "max",
            _ => "medium",
        };
        return (effort, "profile default_reasoning_effort");
    }
    ("medium", "bridge default")
}

fn qwen_prompt_contains_json_keyword(request: &Value) -> bool {
    let contains_json = |value: &Value| value_to_text(value).to_ascii_lowercase().contains("json");
    if request.get("system").is_some_and(contains_json) {
        return true;
    }
    request
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message.get("content").is_some_and(contains_json))
        })
}

fn estimated_reasoning_text_tokens(reasoning: &str) -> usize {
    let mut ascii_bytes = 0usize;
    let mut non_ascii_bytes = 0usize;
    for character in reasoning.chars() {
        if character.is_ascii() {
            ascii_bytes = ascii_bytes.saturating_add(character.len_utf8());
        } else {
            non_ascii_bytes = non_ascii_bytes.saturating_add(character.len_utf8());
        }
    }
    ascii_bytes.div_ceil(4) + non_ascii_bytes.div_ceil(2)
}

fn chat_replayed_reasoning_stats(chat_request: &Value) -> (usize, usize) {
    let mut message_count = 0usize;
    let mut estimated_tokens = 0usize;
    if let Some(messages) = chat_request.get("messages").and_then(Value::as_array) {
        for message in messages {
            let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) else {
                continue;
            };
            message_count = message_count.saturating_add(1);
            estimated_tokens =
                estimated_tokens.saturating_add(estimated_reasoning_text_tokens(reasoning));
        }
    }
    (message_count, estimated_tokens)
}

fn deepseek_anthropic_reasoning_stats(request: &Value) -> (usize, usize) {
    let mut message_count = 0usize;
    let mut estimated_tokens = 0usize;
    if let Some(messages) = request.get("messages").and_then(Value::as_array) {
        for message in messages {
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let mut message_has_reasoning = false;
            if let Some(parts) = message.get("content").and_then(Value::as_array) {
                for part in parts {
                    if part.get("type").and_then(Value::as_str) != Some("thinking") {
                        continue;
                    }
                    let Some(reasoning) = part.get("thinking").and_then(Value::as_str) else {
                        continue;
                    };
                    if reasoning.is_empty() {
                        continue;
                    }
                    message_has_reasoning = true;
                    estimated_tokens =
                        estimated_tokens.saturating_add(estimated_reasoning_text_tokens(reasoning));
                }
            }
            if message_has_reasoning {
                message_count = message_count.saturating_add(1);
            }
        }
    }
    (message_count, estimated_tokens)
}

fn kimi_reasoning_effort(effort: &str) -> &'static str {
    match effort {
        "none" | "minimal" | "low" => "low",
        "medium" | "high" => "high",
        "xhigh" | "max" => "max",
        _ => "max",
    }
}

fn kimi_reasoning_effort_from_budget(budget: u64) -> &'static str {
    if budget >= 32_768 {
        "max"
    } else if budget >= 8_192 {
        "high"
    } else {
        "low"
    }
}

fn kimi_prompt_cache_key(request: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(b"claude-bridge-kimi-session-v1\0");
    if let Some(container) = request.get("container").filter(|value| !value.is_null()) {
        if let Ok(bytes) = serde_json::to_vec(container) {
            digest.update(b"container\0");
            digest.update(bytes);
            return format!("claude-bridge-{:x}", digest.finalize());
        }
    }
    digest.update(b"system\0");
    if let Some(system) = request.get("system") {
        if let Ok(bytes) = serde_json::to_vec(system) {
            digest.update(bytes);
        }
    }
    digest.update(b"\0first-message\0");
    if let Some(first_message) = request
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
    {
        if let Ok(bytes) = serde_json::to_vec(first_message) {
            digest.update(bytes);
        }
    }
    format!("claude-bridge-{:x}", digest.finalize())
}

fn kimi_safety_identifier(request: &Value) -> Option<String> {
    let user_id = request
        .pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mut digest = Sha256::new();
    digest.update(b"claude-bridge-kimi-user-v1\0");
    digest.update(user_id.as_bytes());
    Some(format!("user_{:x}", digest.finalize()))
}

fn openai_chat_response_format(
    request: &Value,
    json_schema_as_json_object: bool,
) -> Result<Option<Value>, String> {
    let Some(format) = request
        .pointer("/output_config/format")
        .or_else(|| request.get("output_format"))
        .filter(|format| !format.is_null())
    else {
        return Ok(None);
    };
    let format_type = format.get("type").and_then(Value::as_str).ok_or_else(|| {
        "Anthropic structured output format must have a string 'type'".to_string()
    })?;

    match format_type {
        "json_object" => Ok(Some(json!({"type": "json_object"}))),
        "json_schema" if json_schema_as_json_object => Ok(Some(json!({"type": "json_object"}))),
        "json_schema" => {
            let schema = format.get("schema").cloned().ok_or_else(|| {
                "Anthropic json_schema output format is missing 'schema'".to_string()
            })?;
            let name = format
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("response");
            let mut json_schema = json!({"name": name, "schema": schema});
            if let Some(strict) = format.get("strict").and_then(Value::as_bool) {
                json_schema["strict"] = Value::Bool(strict);
            }
            Ok(Some(
                json!({"type": "json_schema", "json_schema": json_schema}),
            ))
        }
        unsupported => Err(format!(
            "Unsupported Anthropic structured output format '{unsupported}'"
        )),
    }
}

fn translate_anthropic_user_message(
    content: &Value,
    messages: &mut Vec<Value>,
    capabilities: &OpenAiCapabilities,
    pending_tool_call_ids: &mut HashSet<String>,
) {
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            messages.push(json!({"role": "user", "content": text}));
        }
        pending_tool_call_ids.clear();
        return;
    }

    let Some(parts) = content.as_array() else {
        pending_tool_call_ids.clear();
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
                if !pending_tool_call_ids.remove(tool_use_id) {
                    warn!(
                        tool_call_id = %tool_use_id,
                        "Skipping orphan Anthropic tool result"
                    );
                    continue;
                }
                let (result_content, media_parts) = translate_anthropic_tool_result_content(
                    part.get("content").unwrap_or(&Value::Null),
                    part.get("is_error").and_then(Value::as_bool) == Some(true),
                    capabilities.tool_result_media,
                    capabilities.max_tool_result_chars,
                );
                tool_results.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": result_content
                }));
                user_parts.extend(media_parts);
            }
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    user_parts.push(json!({"type": "text", "text": text}));
                }
            }
            Some("image") => {
                if let Some(media_part) = translate_anthropic_media(part) {
                    user_parts.push(media_part);
                }
            }
            Some("document") => {
                if let Some(media_part) = translate_anthropic_media(part) {
                    user_parts.push(media_part);
                }
            }
            Some("web_search_tool_result") => {
                // Server-side search results keep cross-transport history
                // continuous: titles and URLs survive even though the body
                // arrives encrypted for the client, not the model.
                if let Some(text) = textify_web_search_tool_result(part) {
                    user_parts.push(json!({"type": "text", "text": text}));
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
    pending_tool_call_ids.clear();
}

fn textify_web_search_tool_result(part: &Value) -> Option<String> {
    let results = part.get("content").and_then(Value::as_array)?;
    let mut lines = Vec::new();
    for result in results {
        let title = result.get("title").and_then(Value::as_str).unwrap_or("");
        let url = result.get("url").and_then(Value::as_str).unwrap_or("");
        if title.is_empty() && url.is_empty() {
            continue;
        }
        lines.push(if url.is_empty() {
            format!("- {title}")
        } else {
            format!("- {title} ({url})")
        });
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Web search performed earlier in this conversation returned these results:\n{}",
        lines.join("\n")
    ))
}

fn translate_anthropic_tool_result_content(
    content: &Value,
    is_error: bool,
    media_mode: ToolResultMediaMode,
    max_chars: Option<u64>,
) -> (Value, Vec<Value>) {
    if let Some(parts) = content.as_array() {
        let mut translated_parts = Vec::new();
        let mut result_text = Vec::new();
        let mut media_parts = Vec::new();
        let mut has_media = false;

        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            translated_parts.push(json!({"type": "text", "text": text}));
                            result_text.push(text.to_string());
                        }
                    }
                }
                Some(block_type @ ("image" | "document")) => {
                    if let Some(media_part) = translate_anthropic_media(part) {
                        has_media = true;
                        translated_parts.push(media_part.clone());
                        result_text.push(
                            if block_type == "image" {
                                "[Image result attached]"
                            } else {
                                "[Document result attached]"
                            }
                            .to_string(),
                        );
                        media_parts.push(media_part);
                    }
                }
                _ => {}
            }
        }

        if has_media && media_mode == ToolResultMediaMode::Inline {
            let mut combined_text = result_text.join("\n");
            if is_error {
                combined_text = format!("Tool error: {combined_text}");
            }
            let bounded_text = bound_tool_result_text(combined_text.clone(), max_chars);
            if bounded_text != combined_text {
                let mut bounded_parts = vec![json!({"type": "text", "text": bounded_text})];
                bounded_parts.extend(
                    translated_parts
                        .into_iter()
                        .filter(|part| part.get("type").and_then(Value::as_str) != Some("text")),
                );
                return (Value::Array(bounded_parts), Vec::new());
            }
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
            return (Value::Array(translated_parts), Vec::new());
        }
        if has_media {
            let mut text = result_text.join("\n");
            if is_error {
                text = format!("Tool error: {text}");
            }
            return (
                Value::String(bound_tool_result_text(text, max_chars)),
                media_parts,
            );
        }
    }

    let mut result_text = value_to_text(content);
    if is_error {
        result_text = format!("Tool error: {result_text}");
    }
    (
        Value::String(bound_tool_result_text(result_text, max_chars)),
        Vec::new(),
    )
}

fn bound_tool_result_text(text: String, max_chars: Option<u64>) -> String {
    let Some(max_chars) = max_chars.and_then(|value| usize::try_from(value).ok()) else {
        return text;
    };
    let original_chars = text.chars().count();
    if original_chars <= max_chars {
        return text;
    }

    let original_lines = text.lines().count();
    let marker = format!(
        "\n\n[Bridge truncated oversized tool result: original {original_chars} chars across {original_lines} lines. Showing the beginning and end only. The complete result remains in Claude Code's local transcript. Re-run the tool with offset/limit or targeted search to inspect omitted evidence; do not treat omitted text as reviewed.]\n\n"
    );
    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return marker.chars().take(max_chars).collect();
    }

    let excerpt_chars = max_chars - marker_chars;
    let head_chars = excerpt_chars / 2;
    let tail_chars = excerpt_chars - head_chars;
    let head: String = text.chars().take(head_chars).collect();
    let mut tail: Vec<char> = text.chars().rev().take(tail_chars).collect();
    tail.reverse();
    format!("{head}{marker}{}", tail.into_iter().collect::<String>())
}

fn translate_anthropic_media(part: &Value) -> Option<Value> {
    let block_type = part.get("type").and_then(Value::as_str)?;
    let source = part.get("source")?;
    if block_type != "image" && block_type != "document" {
        return None;
    }

    if source.get("type").and_then(Value::as_str) == Some("url") {
        if block_type != "image" {
            return None;
        }
        let url = source
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| !url.trim().is_empty())?;
        return Some(json!({
            "type": "image_url",
            "image_url": {"url": url}
        }));
    }
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }

    let media_type = source.get("media_type").and_then(Value::as_str)?;
    if block_type == "document" && media_type != "application/pdf" {
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

#[cfg(test)]
fn translate_anthropic_tool(tool: &Value) -> Option<Value> {
    translate_anthropic_tool_with_capabilities(tool, &OpenAiCapabilities::default())
}

fn translate_anthropic_tool_with_capabilities(
    tool: &Value,
    capabilities: &OpenAiCapabilities,
) -> Option<Value> {
    let name = tool.get("name")?.as_str()?;
    // Claude Code server tools (web_search_*) are executed server-side by
    // providers that implement them natively; no OpenAI Chat endpoint treats
    // them as client-side function tools, so never downgrade one to an empty
    // function schema.
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.starts_with("web_search_"))
    {
        warn!(
            tool = name,
            "Skipping Anthropic server tool in OpenAI Chat translation"
        );
        return None;
    }
    let mut function = Map::new();
    function.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    let mut schema = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    if capabilities.tool_schema == ToolSchemaMode::Sanitize {
        sanitize_json_schema(&mut schema);
    }
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
    if let Some(additional_properties) = object.get_mut("additionalProperties") {
        if additional_properties.is_object() {
            sanitize_json_schema(additional_properties);
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

#[cfg(test)]
fn translate_anthropic_response(
    upstream: &Value,
    model: &str,
    thought_signatures: &ThoughtSignatureCache,
) -> Result<Value, String> {
    translate_anthropic_response_with_capabilities(
        upstream,
        model,
        thought_signatures,
        &OpenAiCapabilities::default(),
    )
}

fn translate_anthropic_response_with_capabilities(
    upstream: &Value,
    model: &str,
    thought_signatures: &ThoughtSignatureCache,
    capabilities: &OpenAiCapabilities,
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
            return Ok(json!({
                "id": format!("msg_{}", Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{
                    "type": "text",
                    "text": safety_refusal_text(block_reason)
                }],
                "stop_reason": "refusal",
                "stop_sequence": Value::Null,
                "usage": openai_usage_to_anthropic(&usage, 0)
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
    let mut tool_calls = upstream_message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if tool_calls.is_empty() {
        if let Some(function_call) = upstream_message
            .get("function_call")
            .filter(|function_call| !function_call.is_null())
        {
            tool_calls.push(json!({"type": "function", "function": function_call}));
        }
    }
    let allow_tool_calls =
        anthropic_stop_reason(finish_reason, !tool_calls.is_empty()) == "tool_use";
    let mut content = Vec::new();
    let mut assistant_signature = upstream_message
        .pointer("/extra_content/google/thought_signature")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let reasoning_text = text_from_fields(upstream_message, &capabilities.reasoning_fields);
    if let Some(reasoning_text) = reasoning_text {
        let mut block = json!({"type": "thinking", "thinking": reasoning_text});
        if let Some(signature) = assistant_signature.take() {
            block["signature"] = json!(signature);
        }
        content.push(block);
    }
    let text = value_to_text(upstream_message.get("content").unwrap_or(&Value::Null));
    if capabilities.thinking_tags {
        if let Some((thinking, answer)) = split_tagged_thinking(&text) {
            if !thinking.is_empty() {
                let mut block = json!({"type": "thinking", "thinking": thinking});
                if let Some(signature) = assistant_signature.take() {
                    block["signature"] = json!(signature);
                }
                content.push(block);
            }
            if !answer.is_empty() {
                content.push(json!({"type": "text", "text": answer}));
            }
        } else if !text.is_empty() {
            content.push(json!({"type": "text", "text": text}));
        }
    } else if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    let refusal = value_to_text(upstream_message.get("refusal").unwrap_or(&Value::Null));
    if !refusal.is_empty() {
        content.push(json!({"type": "text", "text": refusal}));
    }

    if allow_tool_calls {
        for tool_call in &tool_calls {
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_function");
            let arguments = tool_arguments_text(tool_call.pointer("/function/arguments"));
            let input = match parse_tool_arguments(&arguments) {
                Ok(input) => input,
                Err(message) => {
                    warn!(
                        tool_call_id = %call_id,
                        error = %message,
                        "Skipping invalid non-streaming tool call"
                    );
                    continue;
                }
            };

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
    let stop_reason = if refusal.is_empty() {
        anthropic_stop_reason(finish_reason, has_tools)
    } else {
        "refusal"
    };
    let usage = upstream.get("usage").cloned().unwrap_or_else(|| json!({}));

    Ok(json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": openai_usage_to_anthropic(&usage, 0)
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

    if let Some(max_tokens) = request.get("max_output_tokens").and_then(value_as_u64) {
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
    let mut pending_tool_call_ids = HashSet::new();

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
                pending_tool_call_ids.insert(call_id.to_string());
                let mut translated_call = json!({
                    "id": call_id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                });
                if let Some(signature) = recalled_thought_signature(thought_signatures, call_id) {
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
                if !pending_tool_call_ids.remove(call_id) {
                    warn!(
                        tool_call_id = %call_id,
                        "Skipping orphan Responses tool output"
                    );
                    continue;
                }
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
                pending_tool_call_ids.clear();
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
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            value
                .pointer("/detail/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            value
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
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
