fn interaction_call_cache_key(profile_file: &str, call_id: &str) -> String {
    format!("{profile_file}\0{call_id}")
}

fn interaction_continuation_state_path(bridge_state_path: &Path) -> PathBuf {
    bridge_state_path.with_file_name("interaction-continuations.json")
}

fn load_interaction_continuation_cache(state_path: &Path) -> InteractionContinuationCache {
    let mut cache = InteractionContinuationCache {
        persistence_path: Some(state_path.to_path_buf()),
        ..InteractionContinuationCache::default()
    };
    let contents = match fs::read_to_string(state_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return cache,
        Err(err) => {
            warn!(
                "Cannot read Gemini interaction continuation state '{}': {err}",
                state_path.display()
            );
            return cache;
        }
    };
    let value = match serde_json::from_str::<Value>(&contents) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                "Cannot parse Gemini interaction continuation state '{}': {err}",
                state_path.display()
            );
            return cache;
        }
    };
    if let Some(calls) = value.get("calls").and_then(Value::as_array) {
        for call in calls
            .iter()
            .rev()
            .take(INTERACTION_CONTINUATION_CAPACITY)
            .rev()
        {
            let Some(key) = call.get("key").and_then(Value::as_str) else {
                continue;
            };
            let Some(interaction_id) = call.get("interaction_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = call.get("name").and_then(Value::as_str) else {
                continue;
            };
            if key.is_empty() || interaction_id.is_empty() || name.is_empty() {
                continue;
            }
            cache.calls.insert(
                key.to_string(),
                InteractionCallContinuation {
                    interaction_id: interaction_id.to_string(),
                    name: name.to_string(),
                },
            );
        }
    }
    if let Some(transcripts) = value.get("transcripts").and_then(Value::as_array) {
        for transcript in transcripts
            .iter()
            .rev()
            .take(INTERACTION_CONTINUATION_CAPACITY)
            .rev()
        {
            let Some(key) = transcript.get("key").and_then(Value::as_str) else {
                continue;
            };
            let Some(interaction_id) = transcript.get("interaction_id").and_then(Value::as_str)
            else {
                continue;
            };
            if key.is_empty() || interaction_id.is_empty() {
                continue;
            }
            cache
                .transcripts
                .insert(key.to_string(), interaction_id.to_string());
        }
    }
    cache
}

fn persist_interaction_continuation_cache(cache: &InteractionContinuationCache) {
    let Some(state_path) = cache.persistence_path.as_deref() else {
        return;
    };
    let calls = cache
        .calls
        .iter()
        .map(|(key, continuation)| {
            json!({
                "key": key,
                "interaction_id": continuation.interaction_id,
                "name": continuation.name
            })
        })
        .collect::<Vec<_>>();
    let transcripts = cache
        .transcripts
        .iter()
        .map(|(key, interaction_id)| {
            json!({
                "key": key,
                "interaction_id": interaction_id
            })
        })
        .collect::<Vec<_>>();
    let contents = json!({
        "version": 1,
        "calls": calls,
        "transcripts": transcripts
    })
    .to_string();
    if let Err(err) = write_state_atomically(state_path, &contents) {
        warn!("{err}");
    }
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

fn read_interaction_continuation_cache(
    continuations: &InteractionContinuationState,
) -> std::sync::RwLockReadGuard<'_, InteractionContinuationCache> {
    match continuations.read() {
        Ok(cache) => cache,
        Err(poisoned) => {
            warn!("Gemini Interactions continuation cache read lock was poisoned; recovering cached state");
            poisoned.into_inner()
        }
    }
}

fn write_interaction_continuation_cache(
    continuations: &InteractionContinuationState,
) -> std::sync::RwLockWriteGuard<'_, InteractionContinuationCache> {
    match continuations.write() {
        Ok(cache) => cache,
        Err(poisoned) => {
            warn!("Gemini Interactions continuation cache write lock was poisoned; recovering cached state");
            poisoned.into_inner()
        }
    }
}

fn remember_interaction_calls_in_memory(
    continuations: &InteractionContinuationState,
    profile_file: &str,
    interaction_id: &str,
    calls: &[(String, String)],
) {
    if interaction_id.is_empty() || calls.is_empty() {
        return;
    }
    let mut cache = write_interaction_continuation_cache(continuations);
    for (call_id, name) in calls {
        let key = interaction_call_cache_key(profile_file, call_id);
        cache.calls.shift_remove(&key);
        cache.calls.insert(
            key,
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
    let snapshot = {
        let mut cache = write_interaction_continuation_cache(continuations);
        cache.transcripts.shift_remove(&transcript_key);
        cache
            .transcripts
            .insert(transcript_key, interaction_id.to_string());
        for (call_id, name) in calls {
            let key = interaction_call_cache_key(profile_file, call_id);
            cache.calls.shift_remove(&key);
            cache.calls.insert(
                key,
                InteractionCallContinuation {
                    interaction_id: interaction_id.to_string(),
                    name: name.clone(),
                },
            );
        }
        evict_interaction_cache(&mut cache);
        cache.clone()
    };
    persist_interaction_continuation_cache(&snapshot);
}

fn interaction_content_from_anthropic(part: &Value) -> Option<Value> {
    match part.get("type").and_then(Value::as_str) {
        Some("text") => part
            .get("text")
            .and_then(Value::as_str)
            .map(|text| json!({"type": "text", "text": text})),
        Some("search_result") => {
            let title = part
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Search result");
            let source = part
                .get("source")
                .or_else(|| part.get("url"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let body = value_to_text(part.get("content").unwrap_or(&Value::Null));
            let mut text = format!("Search result: {title}");
            if !source.is_empty() {
                text.push_str("\nSource: ");
                text.push_str(source);
            }
            if !body.is_empty() {
                text.push_str("\n\n");
                text.push_str(&body);
            }
            Some(json!({"type": "text", "text": text}))
        }
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

struct InteractionToolResultTranslation {
    result: Value,
    documents: Vec<Value>,
}

fn interaction_tool_result_value(
    content: &Value,
    max_chars: Option<u64>,
) -> InteractionToolResultTranslation {
    if let Some(parts) = content.as_array() {
        let translated: Vec<Value> = parts
            .iter()
            .filter_map(interaction_content_from_anthropic)
            .collect();
        let (documents, translated): (Vec<_>, Vec<_>) = translated
            .into_iter()
            .partition(|part| part.get("type").and_then(Value::as_str) == Some("document"));
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
                bounded.extend(
                    translated
                        .into_iter()
                        .filter(|part| part.get("type").and_then(Value::as_str) != Some("text")),
                );
                return InteractionToolResultTranslation {
                    result: Value::Array(bounded),
                    documents,
                };
            }
            return InteractionToolResultTranslation {
                result: Value::Array(translated),
                documents,
            };
        }
        if !documents.is_empty() {
            return InteractionToolResultTranslation {
                result: json!("Document attached in the following user input."),
                documents,
            };
        }
    }
    let text = if let Some(text) = content.as_str() {
        text.to_string()
    } else {
        value_to_text(content)
    };
    InteractionToolResultTranslation {
        result: Value::String(bound_tool_result_text(text, max_chars)),
        documents: Vec::new(),
    }
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
            if computer_tool_name_is(&name, "computer_action_batch") {
                if !user_content.is_empty() {
                    steps.push(json!({"type": "user_input", "content": std::mem::take(&mut user_content)}));
                }
                if let Some(expanded) = expand_computer_batch_tool_result(part.get("content").unwrap_or(&Value::Null)) {
                    steps.extend(expanded);
                    continue;
                }
            }
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
                let translated_result = interaction_tool_result_value(
                    part.get("content").unwrap_or(&Value::Null),
                    max_tool_result_chars,
                );
                match translated_result.result {
                    Value::Array(result_content) => user_content.extend(result_content),
                    Value::String(text) if !text.is_empty() => {
                        user_content.push(json!({"type": "text", "text": text}));
                    }
                    _ => {}
                }
                user_content.extend(translated_result.documents);
                continue;
            }
            if !user_content.is_empty() {
                steps.push(json!({
                    "type": "user_input",
                    "content": std::mem::take(&mut user_content)
                }));
            }
            let translated_result = interaction_tool_result_value(
                part.get("content").unwrap_or(&Value::Null),
                max_tool_result_chars,
            );
            steps.push(json!({
                "type": "function_result",
                "call_id": call_id,
                "name": name,
                "is_error": part.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                "result": translated_result.result
            }));
            if !translated_result.documents.is_empty() {
                steps.push(json!({
                    "type": "user_input",
                    "content": translated_result.documents
                }));
            }
        } else if let Some(translated) = interaction_content_from_anthropic(part) {
            user_content.push(translated);
        }
    }
    if !user_content.is_empty() {
        steps.push(json!({"type": "user_input", "content": user_content}));
    }
    steps
}

fn computer_tool_name_is(name: &str, short_name: &str) -> bool {
    name == short_name || name.ends_with(&format!("__{short_name}"))
}

fn completed_computer_cancel(request: &Value) -> bool {
    let Some(messages) = request.get("messages").and_then(Value::as_array) else {
        return false;
    };
    let completed_tool_uses = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| {
            part.get("type").and_then(Value::as_str) == Some("tool_result")
                && part.get("is_error").and_then(Value::as_bool) != Some(true)
        })
        .filter_map(|part| part.get("tool_use_id").and_then(Value::as_str))
        .collect::<std::collections::HashSet<_>>();

    let mut latest_lifecycle_was_cancel = None;
    for part in messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
    {
        if part.get("type").and_then(Value::as_str) != Some("tool_use")
            || !part
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| completed_tool_uses.contains(id))
        {
            continue;
        }
        let Some(name) = part.get("name").and_then(Value::as_str) else {
            continue;
        };
        if computer_tool_name_is(name, "computer_start") {
            latest_lifecycle_was_cancel = Some(false);
        } else if computer_tool_name_is(name, "computer_cancel") {
            latest_lifecycle_was_cancel = Some(true);
        }
    }
    latest_lifecycle_was_cancel.unwrap_or(false)
}

fn requested_computer_cancel_tool_name(request: &Value) -> Option<String> {
    if completed_computer_cancel(request) {
        return None;
    }
    let cancel_tools = request
        .get("tools")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .filter(|name| computer_tool_name_is(name, "computer_cancel"))
        .collect::<Vec<_>>();
    if cancel_tools.is_empty() {
        return None;
    }

    if let Some(selected) = request
        .pointer("/tool_choice/name")
        .and_then(Value::as_str)
        .filter(|name| computer_tool_name_is(name, "computer_cancel"))
    {
        return cancel_tools
            .contains(&selected)
            .then(|| selected.to_string());
    }

    let latest_user_text = request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| {
            let text = match message.get("content")? {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            (!text.trim().is_empty()).then_some(text)
        })
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    latest_user_text.contains("computer_cancel")
        .then(|| cancel_tools[0].to_string())
}

fn computer_result_envelope(content: &Value) -> Option<Value> {
    let parts = content.as_array()?;
    parts.iter().find_map(|part| {
        (part.get("type").and_then(Value::as_str) == Some("text"))
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(|value| value.get("protocol_version").and_then(Value::as_str) == Some(COMPUTER_PROTOCOL_VERSION))
    })
}

fn computer_result_images(content: &Value) -> Vec<Value> {
    content.as_array().into_iter().flatten().filter_map(|part| {
        if part.get("type").and_then(Value::as_str) != Some("image") { return None; }
        let source = part.get("source").unwrap_or(part);
        let data = source.get("data").and_then(Value::as_str)?;
        let mime_type = source.get("media_type").or_else(|| source.get("mimeType")).and_then(Value::as_str).unwrap_or("image/png");
        Some(json!({"type": "image", "data": data, "mime_type": mime_type}))
    }).collect()
}

fn expand_computer_batch_tool_result(content: &Value) -> Option<Vec<Value>> {
    let envelope = computer_result_envelope(content)?;
    if envelope.get("kind").and_then(Value::as_str) != Some("computer_action_batch_result") { return None; }
    let results = envelope.get("results").and_then(Value::as_array)?;
    let images = computer_result_images(content);
    Some(results.iter().map(|item| {
        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("computer_call_unknown");
        let name = item.get("name").and_then(Value::as_str).unwrap_or("unknown_computer_action");
        let is_error = item.get("status").and_then(Value::as_str) != Some("success");
        let mut result_item = item.clone();
        if result_item.get("safety_acknowledgement").and_then(Value::as_bool) != Some(true) {
            if let Some(object) = result_item.as_object_mut() { object.remove("safety_acknowledgement"); }
        }
        let mut result = vec![json!({"type": "text", "text": result_item.to_string()})];
        let image_index = item.pointer("/screenshot/content_index").and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index.saturating_sub(1)).ok());
        if let Some(image) = image_index.and_then(|index| images.get(index)).cloned().or_else(|| images.last().cloned()) {
            result.push(image);
        }
        json!({"type": "function_result", "call_id": call_id, "name": name, "is_error": is_error, "result": result})
    }).collect())
}

#[derive(Clone)]
struct ComputerConversationContext {
    session_id: String,
    sequence: u64,
    environment: String,
    viewport: Value,
    batch_tool_name: String,
}

fn computer_conversation_context(request: &Value) -> Option<ComputerConversationContext> {
    if completed_computer_cancel(request) {
        return None;
    }
    let batch_tool_name = request.get("tools").and_then(Value::as_array).and_then(|tools| {
        tools.iter().filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .find(|name| computer_tool_name_is(name, "computer_action_batch")).map(str::to_string)
    }).unwrap_or_else(|| "computer_action_batch".to_string());
    let messages = request.get("messages").and_then(Value::as_array)?;
    for message in messages.iter().rev() {
        let Some(parts) = message.get("content").and_then(Value::as_array) else { continue; };
        for part in parts.iter().rev() {
            if part.get("type").and_then(Value::as_str) != Some("tool_result") { continue; }
            let Some(envelope) = computer_result_envelope(part.get("content").unwrap_or(&Value::Null)) else { continue; };
            if !matches!(envelope.get("kind").and_then(Value::as_str), Some("computer_start_result" | "computer_action_batch_result")) { continue; }
            return Some(ComputerConversationContext {
                session_id: envelope.get("session_id").and_then(Value::as_str)?.to_string(),
                sequence: envelope.get("sequence").and_then(Value::as_u64).unwrap_or_default(),
                environment: envelope.get("environment").and_then(Value::as_str)
                    .or_else(|| envelope.pointer("/environment_state/environment").and_then(Value::as_str))
                    .unwrap_or("browser").to_string(),
                viewport: envelope.get("viewport").cloned()
                    .or_else(|| envelope.pointer("/environment_state/viewport").cloned())
                    .unwrap_or_else(|| json!({"width": 1440, "height": 900, "device_scale_factor": 1})),
                batch_tool_name,
            });
        }
    }
    None
}

fn is_gemini_computer_action(name: &str, environment: &str) -> bool {
    matches!(name, "click" | "double_click" | "triple_click" | "middle_click" | "right_click"
        | "mouse_down" | "mouse_up" | "move" | "type" | "drag_and_drop" | "wait"
        | "press_key" | "key_down" | "key_up" | "hotkey" | "take_screenshot" | "scroll")
        || (environment == "browser" && matches!(name, "go_back" | "navigate" | "go_forward"))
}

fn computer_batch_tool_block(interaction_id: &str, request: &Value, native_calls: &[Value]) -> Option<(Value, (String, String))> {
    let context = computer_conversation_context(request)?;
    if native_calls.is_empty() { return None; }
    let calls = native_calls.iter().map(|step| {
        let arguments = step.get("arguments").cloned().unwrap_or_else(|| json!({}));
        json!({
            "call_id": step.get("id").and_then(Value::as_str).unwrap_or("computer_call_unknown"),
            "name": step.get("name").and_then(Value::as_str).unwrap_or("unknown_computer_action"),
            "arguments": arguments,
            "intent": step.pointer("/arguments/intent").cloned().unwrap_or(Value::Null),
            "safety_decision": step.pointer("/arguments/safety_decision").cloned().unwrap_or(Value::Null)
        })
    }).collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(interaction_id.as_bytes());
    digest.update(serde_json::to_vec(&calls).unwrap_or_default());
    let hash = format!("{:x}", digest.finalize());
    let batch_id = format!("cub_{}", &hash[..24]);
    let tool_use_id = format!("toolu_computer_{}", &hash[..24]);
    let name = context.batch_tool_name;
    Some((json!({
        "type": "tool_use", "id": tool_use_id, "name": name,
        "input": {
            "protocol_version": COMPUTER_PROTOCOL_VERSION,
            "session_id": context.session_id,
            "batch_id": batch_id,
            "sequence": context.sequence + 1,
            "environment": context.environment,
            "viewport": context.viewport,
            "calls": calls
        }
    }), (tool_use_id, name)))
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
                if computer_tool_name_is(name, "computer_action_batch") {
                    if let Some(calls) = part.pointer("/input/calls").and_then(Value::as_array) {
                        flush_text(&mut steps, &mut text_content);
                        steps.extend(calls.iter().filter_map(|call| {
                            let native_call_id = call.get("call_id").and_then(Value::as_str)?;
                            let native_name = call.get("name").and_then(Value::as_str)?;
                            Some(json!({
                                "type": "function_call",
                                "id": native_call_id,
                                "name": native_name,
                                "arguments": call.get("arguments").cloned().unwrap_or_else(|| json!({}))
                            }))
                        }));
                        continue;
                    }
                }
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

fn interaction_message_shapes(messages: &[Value]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let content = match message.get("content") {
                Some(Value::String(_)) => "text".to_string(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .map(|part| {
                        part.get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    })
                    .collect::<Vec<_>>()
                    .join("+"),
                _ => "unsupported".to_string(),
            };
            format!("{index}:{role}:{content}")
        })
        .collect::<Vec<_>>()
        .join(",")
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

fn interaction_value_contains_pdf_document(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(interaction_value_contains_pdf_document),
        Value::Object(object) => {
            let is_pdf_document = object.get("type").and_then(Value::as_str) == Some("document")
                && object.get("source").and_then(Value::as_object).is_some_and(|source| {
                    source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .is_some_and(|mime| mime.eq_ignore_ascii_case("application/pdf"))
                        || (source.get("type").and_then(Value::as_str) == Some("url")
                            && source.get("media_type").is_none_or(Value::is_null))
                });
            is_pdf_document
                || object
                    .values()
                    .any(interaction_value_contains_pdf_document)
        }
        _ => false,
    }
}

fn interaction_request_contains_pdf(request: &Value) -> bool {
    request
        .get("messages")
        .is_some_and(interaction_value_contains_pdf_document)
}

fn gemini_interaction_pdf_tool_diagnostic(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> Option<String> {
    (interaction_request_contains_pdf(request)
        && (capabilities.gemini_builtin_tools.iter().any(|tool| {
            tool.get("type").and_then(Value::as_str) == Some("code_execution")
        }) || request
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|tool| {
                    anthropic_server_tool_gemini_type(tool) == Some("code_execution")
                })
            })))
    .then(|| {
        "Omitted Gemini Code Execution because Gemini rejects code_execution with application/pdf input"
            .to_string()
    })
}

fn anthropic_server_tool_gemini_type(tool: &Value) -> Option<&'static str> {
    let tool_type = tool.get("type").and_then(Value::as_str)?;
    if tool_type.starts_with("web_search_") {
        Some("google_search")
    } else if tool_type.starts_with("web_fetch_") {
        Some("url_context")
    } else if tool_type.starts_with("code_execution_") {
        Some("code_execution")
    } else {
        None
    }
}

fn anthropic_client_tool_as_function(tool: &Value) -> Result<Option<Value>, String> {
    let Some(tool_type) = tool.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    let (expected_name, description, parameters) = match tool_type {
        "bash_20250124" => (
            "bash",
            "Run a command in the client's persistent Bash session, or restart that session.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Bash command to execute."},
                    "restart": {"type": "boolean", "description": "Restart the persistent Bash session."}
                }
            }),
        ),
        value if value.starts_with("text_editor_") => (
            "str_replace_based_edit_tool",
            "Use the client-side text editor to view, create, replace, or insert text in files.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "enum": ["view", "str_replace", "create", "insert"]},
                    "path": {"type": "string"},
                    "view_range": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
                    "old_str": {"type": "string"},
                    "new_str": {"type": "string"},
                    "file_text": {"type": "string"},
                    "insert_line": {"type": "integer"},
                    "insert_text": {"type": "string"}
                },
                "required": ["command", "path"]
            }),
        ),
        "memory_20250818" => (
            "memory",
            "Operate on client-managed persistent memory. All paths must remain under /memories.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "enum": ["view", "create", "str_replace", "insert", "delete", "rename"]},
                    "path": {"type": "string", "description": "Path under /memories."},
                    "view_range": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
                    "file_text": {"type": "string"},
                    "old_str": {"type": "string"},
                    "new_str": {"type": "string"},
                    "insert_line": {"type": "integer"},
                    "insert_text": {"type": "string"},
                    "old_path": {"type": "string", "description": "Source path under /memories."},
                    "new_path": {"type": "string", "description": "Destination path under /memories."}
                },
                "required": ["command"]
            }),
        ),
        _ => return Ok(None),
    };
    let actual_name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
    if actual_name != expected_name {
        return Err(format!(
            "Anthropic client tool '{tool_type}' must use name '{expected_name}'"
        ));
    }
    let mut description = description.to_string();
    if let Some(max_characters) = tool.get("max_characters").and_then(Value::as_u64) {
        description.push_str(&format!(
            " The client limits view output to {max_characters} characters."
        ));
    }
    Ok(Some(json!({
        "type": "function",
        "name": expected_name,
        "description": description,
        "parameters": parameters
    })))
}

fn push_unique_interaction_tool(translated: &mut Vec<Value>, tool: Value) {
    let Some(tool_type) = tool.get("type").and_then(Value::as_str) else {
        translated.push(tool);
        return;
    };
    let duplicate_index = if tool_type == "function" {
        let name = tool.get("name").and_then(Value::as_str);
        translated.iter().position(|existing| {
            existing.get("type").and_then(Value::as_str) == Some("function")
                && existing.get("name").and_then(Value::as_str) == name
        })
    } else {
        translated
            .iter()
            .position(|existing| existing.get("type").and_then(Value::as_str) == Some(tool_type))
    };
    if let Some(index) = duplicate_index {
        let existing_fields = translated[index].as_object().map_or(0, Map::len);
        let new_fields = tool.as_object().map_or(0, Map::len);
        if tool_type != "function" && new_fields > existing_fields {
            translated[index] = tool;
        }
    } else {
        translated.push(tool);
    }
}

fn gemini_mcp_server_name(name: &str) -> Option<String> {
    let mut translated = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            translated.push(ch);
        } else if ch == '-' {
            translated.push('_');
        } else {
            return None;
        }
    }
    (!translated.is_empty()).then_some(translated)
}

fn translated_anthropic_mcp_servers(request: &Value) -> Result<Vec<Value>, String> {
    let Some(servers) = request.get("mcp_servers") else {
        return Ok(Vec::new());
    };
    let servers = servers
        .as_array()
        .ok_or_else(|| "Anthropic mcp_servers must be an array".to_string())?;
    if servers.len() > 20 {
        return Err("Anthropic mcp_servers accepts at most 20 entries".to_string());
    }
    let toolsets = request
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut translated = Vec::with_capacity(servers.len());
    let mut translated_names = HashSet::new();
    for server in servers {
        let server = server
            .as_object()
            .ok_or_else(|| "Anthropic mcp_servers entries must be objects".to_string())?;
        if server.get("type").and_then(Value::as_str) != Some("url") {
            return Err("Anthropic MCP server type must be 'url'".to_string());
        }
        let original_name = server
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Anthropic MCP server requires a non-empty name".to_string())?;
        let translated_name = gemini_mcp_server_name(original_name).ok_or_else(|| {
            format!(
                "Anthropic MCP server '{original_name}' cannot be mapped to a Gemini snake_case name"
            )
        })?;
        if !translated_names.insert(translated_name.clone()) {
            return Err(format!(
                "Anthropic MCP server names collide after Gemini name normalization: '{translated_name}'"
            ));
        }
        let server_url = server
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("Anthropic MCP server '{original_name}' requires a URL"))?;
        let parsed_url = url::Url::parse(server_url).map_err(|_| {
            format!("Anthropic MCP server '{original_name}' has an invalid URL")
        })?;
        if parsed_url.scheme() != "https"
            || parsed_url.host_str().is_none()
            || !parsed_url.username().is_empty()
            || parsed_url.password().is_some()
        {
            return Err(format!(
                "Anthropic MCP server '{original_name}' must use HTTPS without embedded credentials"
            ));
        }
        let matching_toolsets = toolsets
            .iter()
            .filter(|tool| {
                tool.get("type").and_then(Value::as_str) == Some("mcp_toolset")
                    && tool.get("mcp_server_name").and_then(Value::as_str) == Some(original_name)
            })
            .collect::<Vec<_>>();
        if matching_toolsets.len() != 1 {
            return Err(format!(
                "Anthropic MCP server '{original_name}' must be referenced by exactly one mcp_toolset"
            ));
        }
        let toolset = matching_toolsets[0];
        let default_enabled = toolset
            .pointer("/default_config/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let configs = toolset
            .get("configs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if default_enabled
            && configs
                .values()
                .any(|config| config.get("enabled").and_then(Value::as_bool) == Some(false))
        {
            return Err(format!(
                "Anthropic MCP server '{original_name}' uses a denylist that Gemini Remote MCP cannot enforce; use an allowlist with default_config.enabled=false"
            ));
        }

        let mut mcp_server = Map::new();
        mcp_server.insert("type".to_string(), json!("mcp_server"));
        mcp_server.insert("name".to_string(), json!(translated_name));
        mcp_server.insert("url".to_string(), json!(server_url));
        if let Some(token) = server
            .get("authorization_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let header_value = format!("Bearer {token}");
            header_value
                .parse::<reqwest::header::HeaderValue>()
                .map_err(|_| {
                    format!(
                        "Anthropic MCP server '{original_name}' has an invalid authorization token"
                    )
                })?;
            mcp_server.insert(
                "headers".to_string(),
                json!({"Authorization": header_value}),
            );
        }
        if !default_enabled {
            let allowed_tools = configs
                .iter()
                .filter(|(_, config)| {
                    config.get("enabled").and_then(Value::as_bool) == Some(true)
                })
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            mcp_server.insert("allowed_tools".to_string(), json!(allowed_tools));
        }
        translated.push(Value::Object(mcp_server));
    }

    for toolset in toolsets
        .iter()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("mcp_toolset"))
    {
        let Some(name) = toolset.get("mcp_server_name").and_then(Value::as_str) else {
            return Err("Anthropic mcp_toolset requires mcp_server_name".to_string());
        };
        let references = servers.iter().filter(|server| {
            server.get("name").and_then(Value::as_str) == Some(name)
        });
        if references.count() != 1 {
            return Err(format!(
                "Anthropic mcp_toolset '{name}' must reference exactly one mcp_servers entry"
            ));
        }
    }
    Ok(translated)
}

fn translated_interaction_tools(
    request: &Value,
    capabilities: &OpenAiCapabilities,
) -> Result<Vec<Value>, String> {
    let mut translated = Vec::new();
    let computer_context = computer_conversation_context(request);
    let requested_cancel_tool = computer_context
        .as_ref()
        .and_then(|_| requested_computer_cancel_tool_name(request));
    let omit_code_execution =
        interaction_request_contains_pdf(request) || computer_context.is_some();
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let tool_name = tool.get("name").and_then(Value::as_str);
            if requested_cancel_tool.is_some()
                && !tool_name.is_some_and(|name| computer_tool_name_is(name, "computer_cancel"))
            {
                continue;
            }
            if requested_cancel_tool.is_none()
                && tool_name.is_some_and(|name| {
                    computer_tool_name_is(name, "computer_action_batch")
                        || (computer_context.is_some()
                            && (computer_tool_name_is(name, "computer_start")
                                || computer_tool_name_is(name, "computer_cancel")))
                })
            {
                continue;
            }
            if let Some(gemini_type) = anthropic_server_tool_gemini_type(tool) {
                if !(omit_code_execution && gemini_type == "code_execution") {
                    push_unique_interaction_tool(
                        &mut translated,
                        json!({"type": gemini_type}),
                    );
                }
                continue;
            }
            if let Some(translated_tool) = anthropic_client_tool_as_function(tool)? {
                push_unique_interaction_tool(&mut translated, translated_tool);
                continue;
            }
            if tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|tool_type| {
                    tool_type.starts_with("tool_search_tool_")
                        || tool_type.starts_with("advisor_")
                        || tool_type.starts_with("computer_")
                        || tool_type == "mcp_toolset"
                })
            {
                continue;
            }
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
            push_unique_interaction_tool(&mut translated, translated_tool);
        }
    }
    if requested_cancel_tool.is_some() {
        return Ok(translated);
    }
    for tool in capabilities.gemini_builtin_tools.iter().filter(|tool| {
        !(omit_code_execution
            && tool.get("type").and_then(Value::as_str) == Some("code_execution"))
            && tool.get("type").and_then(Value::as_str) != Some("computer_use")
    }) {
        push_unique_interaction_tool(&mut translated, tool.clone());
    }
    if let Some(context) = &computer_context {
        push_unique_interaction_tool(&mut translated, json!({
            "type": "computer_use",
            "environment": context.environment,
            "enable_prompt_injection_detection": true
        }));
    }
    if !capabilities.gemini_file_search_store_names.is_empty() {
        if let Some(file_search) = translated
            .iter_mut()
            .find(|tool| tool.get("type").and_then(Value::as_str) == Some("file_search"))
        {
            if file_search.get("file_search_store_names").is_none() {
                file_search["file_search_store_names"] =
                    json!(capabilities.gemini_file_search_store_names);
            }
        } else {
            translated.push(json!({
                "type": "file_search",
                "file_search_store_names": capabilities.gemini_file_search_store_names
            }));
        }
    }
    translated.extend(capabilities.gemini_remote_mcp_servers.iter().cloned());
    for request_server in translated_anthropic_mcp_servers(request)? {
        let duplicate = translated.iter().any(|existing| {
            existing.get("type").and_then(Value::as_str) == Some("mcp_server")
                && existing.get("name").and_then(Value::as_str)
                    == request_server.get("name").and_then(Value::as_str)
        });
        if !duplicate {
            translated.push(request_server);
        }
    }
    Ok(translated)
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
        return Some(
            if model.starts_with("gemini-3") || is_gemini_37_or_newer_flash_model(model) {
                "low"
            } else {
                "none"
            }
            .to_string(),
        );
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
        "none" | "minimal"
            if model.starts_with("gemini-3") || is_gemini_37_or_newer_flash_model(model) =>
        {
            Some("low")
        }
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

fn interaction_service_tier(request: &Value, capabilities: &OpenAiCapabilities) -> Option<String> {
    let configured = capabilities
        .gemini_service_tier
        .as_deref()
        .filter(|tier| *tier != "auto");
    configured.map(str::to_owned).or_else(|| {
        if request.get("speed").and_then(Value::as_str) == Some("fast") {
            return Some("priority".to_string());
        }
        match request.get("service_tier").and_then(Value::as_str) {
            Some("standard_only") => Some("standard".to_string()),
            _ => None,
        }
    })
}

fn anthropic_request_uses_extension(request: &Value, key: &str) -> bool {
    fn content_uses_extension(content: &Value, key: &str) -> bool {
        match content {
            Value::Array(blocks) => blocks
                .iter()
                .any(|block| content_uses_extension(block, key)),
            Value::Object(block) => {
                block.contains_key(key)
                    || block
                        .get("content")
                        .is_some_and(|content| content_uses_extension(content, key))
            }
            _ => false,
        }
    }

    request.get(key).is_some()
        || request
            .get("system")
            .is_some_and(|system| content_uses_extension(system, key))
        || request
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().any(|tool| tool.get(key).is_some()))
        || request
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                messages.iter().any(|message| {
                    message
                        .get("content")
                        .is_some_and(|content| content_uses_extension(content, key))
                })
            })
}

#[derive(Clone)]
struct PendingInteractionToolCall {
    id: String,
    name: String,
    input: Value,
}

struct RepeatedInteractionToolLoop {
    repeats: usize,
    names: Vec<String>,
}

fn interaction_request_has_source_navigation_tools(request: &Value) -> bool {
    request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .any(|name| {
            name.eq_ignore_ascii_case("Read")
                || name.eq_ignore_ascii_case("Grep")
                || name.eq_ignore_ascii_case("Glob")
        })
}

fn canonical_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json_value).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json_value(&object[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn repeated_interaction_tool_loop(messages: &[Value]) -> Option<RepeatedInteractionToolLoop> {
    let mut pending_calls = Vec::<PendingInteractionToolCall>::new();
    let mut pending_results = HashMap::<String, (Value, bool)>::new();
    let mut completed_cycles = Vec::<(Vec<u8>, Vec<String>)>::new();

    for message in messages {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                let parts = message.get("content").and_then(Value::as_array);
                let calls = parts
                    .into_iter()
                    .flatten()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .filter_map(|part| {
                        Some(PendingInteractionToolCall {
                            id: part.get("id")?.as_str()?.to_string(),
                            name: part.get("name")?.as_str()?.to_string(),
                            input: part.get("input").cloned().unwrap_or_else(|| json!({})),
                        })
                    })
                    .collect::<Vec<_>>();
                if calls.is_empty() {
                    let has_final_text = parts.into_iter().flatten().any(|part| {
                        part.get("type").and_then(Value::as_str) == Some("text")
                            && part
                                .get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.is_empty())
                    });
                    if has_final_text {
                        pending_calls.clear();
                        pending_results.clear();
                        completed_cycles.clear();
                    }
                } else {
                    if !pending_calls.is_empty() {
                        completed_cycles.clear();
                    }
                    pending_calls = calls;
                    pending_results.clear();
                }
            }
            Some("user") => {
                let results = message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))
                    .filter_map(|part| {
                        Some((
                            part.get("tool_use_id")?.as_str()?.to_string(),
                            part.get("content").cloned().unwrap_or(Value::Null),
                            part.get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        ))
                    })
                    .collect::<Vec<_>>();
                if results.is_empty() {
                    if !pending_calls.is_empty() || !completed_cycles.is_empty() {
                        pending_calls.clear();
                        pending_results.clear();
                        completed_cycles.clear();
                    }
                    continue;
                }
                if pending_calls.is_empty() {
                    completed_cycles.clear();
                    continue;
                }
                for (call_id, result, is_error) in results {
                    if pending_calls.iter().any(|call| call.id == call_id) {
                        pending_results.insert(call_id, (result, is_error));
                    }
                }
                if pending_calls
                    .iter()
                    .all(|call| pending_results.contains_key(&call.id))
                {
                    let cycle = pending_calls
                        .iter()
                        .map(|call| {
                            let (result, is_error) = &pending_results[&call.id];
                            json!({
                                "name": call.name,
                                "input": canonical_json_value(&call.input),
                                "result": canonical_json_value(result),
                                "is_error": is_error
                            })
                        })
                        .collect::<Vec<_>>();
                    let fingerprint = serde_json::to_vec(&cycle).ok()?;
                    let names = pending_calls.iter().map(|call| call.name.clone()).collect();
                    completed_cycles.push((fingerprint, names));
                    pending_calls.clear();
                    pending_results.clear();
                }
            }
            Some("system") => {}
            _ => {
                pending_calls.clear();
                pending_results.clear();
                completed_cycles.clear();
            }
        }
    }

    let (latest, names) = completed_cycles.last()?;
    let repeats = completed_cycles
        .iter()
        .rev()
        .take_while(|(fingerprint, _)| fingerprint == latest)
        .count();
    (repeats >= 3).then(|| RepeatedInteractionToolLoop {
        repeats,
        names: names.clone(),
    })
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
    let unsupported_parameters: Vec<&str> = [
        "candidate_count",
        "frequency_penalty",
        "presence_penalty",
    ]
    .into_iter()
    .filter(|field| request.get(*field).is_some())
    .collect();
    if !unsupported_parameters.is_empty() {
        add(&format!(
            "Dropped unsupported Gemini parameters: {}",
            unsupported_parameters.join(", ")
        ));
    }
    if let Some(effort) = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
    {
        match effort {
            "none" | "minimal" => add(&format!(
                "Mapped Anthropic output_config.effort '{effort}' to the minimum supported Gemini thinking level 'low'"
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
            "auto" if request.get("speed").and_then(Value::as_str) == Some("fast") => {}
            "auto" => add("Anthropic service_tier 'auto' is left unset; Gemini uses its default standard tier"),
            _ => add(&format!(
                "Ignored unsupported Anthropic service_tier '{tier}'"
            )),
        }
    }
    if let Some(speed) = request.get("speed").and_then(Value::as_str) {
        match speed {
            "fast" => add("Mapped Anthropic fast mode to Gemini Priority inference when the provider profile does not force another service tier"),
            "standard" => {}
            _ => add(&format!("Ignored unsupported Anthropic speed '{speed}'")),
        }
    }
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(tool_type) = tool.get("type").and_then(Value::as_str) else {
                continue;
            };
            if let Some(gemini_type) = anthropic_server_tool_gemini_type(tool) {
                add(&format!(
                    "Mapped Anthropic server tool '{tool_type}' to Gemini native '{gemini_type}'"
                ));
                for field in [
                    "allowed_domains",
                    "blocked_domains",
                    "max_uses",
                    "max_content_tokens",
                    "use_cache",
                    "user_location",
                    "response_inclusion",
                ] {
                    if tool.get(field).is_some() {
                        add(&format!(
                            "Gemini native '{gemini_type}' does not expose Anthropic server-tool control '{field}'; the control was not forwarded"
                        ));
                    }
                }
            } else if matches!(tool_type, "bash_20250124" | "memory_20250818")
                || tool_type.starts_with("text_editor_")
            {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    add(&format!(
                        "Expanded Anthropic schema-less client tool '{tool_type}' into Gemini function '{name}' for client-side execution"
                    ));
                }
            } else if matches!(
                tool_type,
                "computer_toolset_20260801" | "browser_toolset_20260801"
            ) || tool_type.starts_with("computer_")
            {
                add(&format!(
                    "Cannot safely translate Anthropic schema-less toolset '{tool_type}' to Gemini: use Claude Code Chrome/Computer Use or an MCP browser tool so the client executes actions and returns screenshots"
                ));
            } else if tool_type.starts_with("tool_search_tool_") {
                add(&format!(
                    "Replaced Anthropic server tool search '{tool_type}' by eagerly exposing the request's function tools to Gemini"
                ));
            } else if tool_type.starts_with("advisor_") {
                add(&format!(
                    "Skipped Anthropic advisor tool '{tool_type}': Gemini has no separate advisor service and performs the reasoning in the active model"
                ));
            } else if tool_type == "mcp_toolset" {
                if let Some(name) = tool.get("mcp_server_name").and_then(Value::as_str) {
                    add(&format!(
                        "Mapped Anthropic request MCP toolset '{name}' to Gemini native Remote MCP"
                    ));
                }
                let uses_deferred_loading = tool
                    .pointer("/default_config/defer_loading")
                    .is_some_and(|value| !value.is_null())
                    || tool
                        .get("configs")
                        .and_then(Value::as_object)
                        .is_some_and(|configs| {
                            configs.values().any(|config| {
                                config
                                    .get("defer_loading")
                                    .is_some_and(|value| !value.is_null())
                            })
                        });
                if uses_deferred_loading {
                    add("Gemini Remote MCP discovers tools server-side and does not expose Anthropic defer_loading controls; the controls were not forwarded");
                }
            }
        }
    }
    if let Some(servers) = request.get("mcp_servers").and_then(Value::as_array) {
        for server in servers {
            let Some(name) = server.get("name").and_then(Value::as_str) else {
                continue;
            };
            if let Some(mapped) = gemini_mcp_server_name(name).filter(|mapped| mapped != name) {
                add(&format!(
                    "Normalized Anthropic MCP server name '{name}' to Gemini-compatible '{mapped}'"
                ));
            }
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
        if anthropic_request_uses_extension(request, field) {
            add(&format!(
                "Ignored Anthropic extension '{field}' while translating to Gemini Interactions"
            ));
        }
    }
    if let Some(repeated) = request
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| repeated_interaction_tool_loop(messages))
    {
        add(&format!(
            "Stopped a repeated Gemini tool loop after {} identical completed cycles ({}) and forced a final no-tools turn",
            repeated.repeats,
            repeated.names.join(", ")
        ));
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
    let max_tokens = request.get("max_tokens").and_then(value_as_u64)?;
    let budget = request
        .pointer("/thinking/budget_tokens")
        .and_then(value_as_u64)?;
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
    if anthropic_request_uses_extension(request, "cache_control") {
        add("Ignored Anthropic cache_control blocks: provider caching is reported when available but cache placement is not controllable through this transport".to_string());
    }
    if anthropic_request_uses_extension(request, "citations") {
        add("Ignored Anthropic citations controls: this transport cannot preserve the Anthropic citation contract".to_string());
    }
    for field in [
        "defer_loading",
        "input_examples",
        "allowed_callers",
        "eager_input_streaming",
    ] {
        if anthropic_request_uses_extension(request, field) {
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
        if request
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("web_search_"))
                })
            })
        {
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

struct InteractionRequestContinuation {
    previous_id: String,
    tool_names: HashMap<String, String>,
    input_start: usize,
    kind: &'static str,
}

fn trailing_interaction_input_start(messages: &[Value]) -> Option<usize> {
    let mut start = messages.len();
    let mut has_user = false;
    while start > 0 {
        match messages[start - 1].get("role").and_then(Value::as_str) {
            Some("user") => {
                has_user = true;
                start -= 1;
            }
            Some("system") => start -= 1,
            _ => break,
        }
    }
    has_user.then_some(start)
}

fn interaction_continuation_for_request(
    profile_file: &str,
    request: &Value,
    messages: &[Value],
    continuations: &InteractionContinuationState,
) -> Option<InteractionRequestContinuation> {
    let input_start = trailing_interaction_input_start(messages)?;
    let mut result_ids = Vec::new();
    for message in &messages[input_start..] {
        if let Some(parts) = message.get("content").and_then(Value::as_array) {
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if let Some(call_id) = part.get("tool_use_id").and_then(Value::as_str) {
                        result_ids.push(call_id.to_string());
                    }
                }
            }
        }
    }
    if !result_ids.is_empty() {
        let matched = {
            let cache = read_interaction_continuation_cache(continuations);
            let mut interaction_id = None;
            let mut names = HashMap::new();
            let mut matched_keys = Vec::new();
            let mut complete = true;
            for call_id in &result_ids {
                let key = interaction_call_cache_key(profile_file, call_id);
                let Some(continuation) = cache.calls.get(&key) else {
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
                matched_keys.push(key);
            }
            complete
                .then_some(interaction_id)
                .flatten()
                .map(|id| (id, names, matched_keys))
        };
        if let Some((id, names, matched_keys)) = matched {
            {
                let mut cache = write_interaction_continuation_cache(continuations);
                for key in matched_keys {
                    if let Some(continuation) = cache.calls.shift_remove(&key) {
                        cache.calls.insert(key, continuation);
                    }
                }
            }
            return Some(InteractionRequestContinuation {
                previous_id: id,
                tool_names: names,
                input_start,
                kind: "tool_call_id",
            });
        }
    }
    if input_start == 0 {
        return None;
    }
    let previous_messages = &messages[..input_start];
    let key =
        interaction_transcript_cache_key(profile_file, request.get("system"), previous_messages);
    let names = interaction_tool_names_from_messages(previous_messages);
    let transcript_id = {
        let cache = read_interaction_continuation_cache(continuations);
        cache.transcripts.get(&key).cloned()
    };
    if let Some(id) = transcript_id {
        let mut cache = write_interaction_continuation_cache(continuations);
        if let Some(current_id) = cache.transcripts.shift_remove(&key) {
            cache.transcripts.insert(key, current_id);
        }
        return Some(InteractionRequestContinuation {
            previous_id: id,
            tool_names: names,
            input_start,
            kind: "transcript",
        });
    }
    if !result_ids.is_empty() {
        let cached_results = {
            let cache = read_interaction_continuation_cache(continuations);
            result_ids
                .iter()
                .filter(|call_id| {
                    cache
                        .calls
                        .contains_key(&interaction_call_cache_key(profile_file, call_id))
                })
                .count()
        };
        warn!(
            provider = profile_file,
            result_count = result_ids.len(),
            cached_results,
            result_ids = %result_ids.join(","),
            "Could not select a stored interaction for trailing tool results"
        );
    }
    None
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
    let model = display_model_name(&profile.model);
    if (model.to_ascii_lowercase().starts_with("gemini-3")
        || is_gemini_37_or_newer_flash_model(&model))
        && messages
            .last()
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
    {
        return Err(
            format!(
                "{} does not support an assistant prefill; the final message must be user input or a tool result",
                model
            ),
        );
    }
    let store_interaction = profile.openai_capabilities.gemini_store;
    let continuation = (allow_continuation && store_interaction)
        .then(|| {
            interaction_continuation_for_request(
                &profile.file_name,
                request,
                messages,
                continuations,
            )
        })
        .flatten();
    if allow_continuation
        && store_interaction
        && continuation.is_none()
        && interaction_messages_have_tool_history(messages)
    {
        warn!(
            provider = %profile.file_name,
            shapes = %interaction_message_shapes(messages),
            "Stored Gemini continuation was not selected for a request with tool history"
        );
    }
    let text_tool_history =
        continuation.is_none() && interaction_messages_have_tool_history(messages);
    let input = if let Some(continuation) = &continuation {
        messages[continuation.input_start..]
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .flat_map(|message| {
                interaction_user_steps(
                    message.get("content").unwrap_or(&Value::Null),
                    &continuation.tool_names,
                    false,
                    profile.openai_capabilities.max_tool_result_chars,
                )
            })
            .collect()
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
    let repeated_tool_loop = repeated_interaction_tool_loop(messages);
    if let Some(max_tokens) = request
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map(|requested| {
            profile
                .openai_capabilities
                .max_output_tokens
                .map_or(requested, |limit| requested.min(limit))
        })
    {
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
    let requested_cancel_tool = computer_conversation_context(request)
        .as_ref()
        .and_then(|_| requested_computer_cancel_tool_name(request));
    let tools = translated_interaction_tools(request, &profile.openai_capabilities)?;
    if !tools.is_empty() {
        let tool_choice = requested_cancel_tool
            .as_ref()
            .map(|name| json!({"allowed_tools": {"mode": "any", "tools": [name]}}))
            .or_else(|| {
                profile
                    .openai_capabilities
                    .gemini_tool_choice_override
                    .as_ref()
                    .map(|choice| json!(choice))
            })
            .or_else(|| request.get("tool_choice").and_then(interaction_tool_choice));
        if let Some(choice) = tool_choice {
            generation_config.insert("tool_choice".to_string(), choice);
        }
    }
    if let Some(repeated) = &repeated_tool_loop {
        generation_config.insert("tool_choice".to_string(), json!("none"));
        warn!(
            provider = %profile.file_name,
            repeats = repeated.repeats,
            tools = %repeated.names.join(","),
            "Stopped repeated Gemini tool loop and forced a final no-tools turn"
        );
    }

    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        json!(display_model_name(&profile.model)),
    );
    body.insert("input".to_string(), Value::Array(input));
    body.insert("store".to_string(), Value::Bool(store_interaction));
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
    if let Some(service_tier) = interaction_service_tier(request, &profile.openai_capabilities) {
        body.insert("service_tier".to_string(), json!(service_tier));
    }
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(continuation) = continuation {
        info!(
            provider = %profile.file_name,
            continuation = continuation.kind,
            "Continuing stored Gemini interaction"
        );
        body.insert(
            "previous_interaction_id".to_string(),
            json!(continuation.previous_id),
        );
    }
    let mut system = value_to_text(request.get("system").unwrap_or(&Value::Null));
    for message in messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
    {
        let message_text = value_to_text(message.get("content").unwrap_or(&Value::Null));
        if message_text.is_empty() {
            continue;
        }
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(&message_text);
    }
    if text_tool_history {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(INTERACTION_TOOL_HISTORY_RECOVERY_INSTRUCTION);
    }
    if interaction_request_has_source_navigation_tools(request) {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(INTERACTION_SOURCE_NAVIGATION_COACH_INSTRUCTION);
    }
    if repeated_tool_loop.is_some() {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(INTERACTION_REPEATED_TOOL_LOOP_INSTRUCTION);
    }
    if !system.is_empty() {
        body.insert("system_instruction".to_string(), json!(system));
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
    tools.retain(|tool| matches!(tool.get("type").and_then(Value::as_str), Some("function" | "computer_use")));
    original_len != tools.len()
}

fn interaction_request_has_computer_use(request: &Value) -> bool {
    request.get("tools").and_then(Value::as_array).is_some_and(|tools| {
        tools.iter().any(|tool| tool.get("type").and_then(Value::as_str) == Some("computer_use"))
    })
}

fn remove_interaction_tools_incompatible_with_computer_use(request: &mut Value) -> bool {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let original_len = tools.len();
    tools.retain(|tool| {
        tool.get("type").and_then(Value::as_str) == Some("computer_use")
    });
    original_len != tools.len() && !tools.is_empty()
}

fn is_mixed_interaction_tools_error(status: u16, message: &str) -> bool {
    if status != 400 {
        return false;
    }
    let message = message.to_ascii_lowercase();
    message.contains("include_server_side_tool_invocations")
        || (message.contains("server-side tool") && message.contains("function"))
        || (message.contains("built-in tool") && message.contains("function"))
        || (message.contains("computer_use") && message.contains("cannot be combined"))
}

fn is_interaction_continuation_unavailable(status: u16, message: &str, request: &Value) -> bool {
    if request.get("previous_interaction_id").is_none() {
        return false;
    }
    if matches!(status, 404 | 410 | 501) {
        return true;
    }
    if status != 400 {
        return false;
    }
    let message = message.to_ascii_lowercase();
    let names_previous_interaction =
        message.contains("previous_interaction_id") || message.contains("previous interaction");
    let describes_unavailable = [
        "expired",
        "not found",
        "invalid",
        "unavailable",
        "no longer exists",
    ]
    .iter()
    .any(|reason| message.contains(reason));
    names_previous_interaction && describes_unavailable
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
        if step_type == "google_search_call" {
            self.web_search_requests = self.web_search_requests.saturating_add(1);
        } else if step_type == "url_context_call" {
            self.web_fetch_requests = self.web_fetch_requests.saturating_add(1);
        }
        if self.steps.len() >= INTERACTION_SERVER_TOOL_TRACE_CAPACITY {
            return;
        }
        self.steps.push(summary);
    }

    fn provider_metadata(
        &self,
        interaction_usage: &Value,
        interaction_annotations: &[Value],
        service_tier: Option<&str>,
    ) -> Option<Value> {
        let mut google = Map::new();
        if !self.steps.is_empty() {
            google.insert(
                "interaction_server_tools".to_string(),
                Value::Array(self.steps.clone()),
            );
        }
        if interaction_usage
            .as_object()
            .is_some_and(|usage| !usage.is_empty())
        {
            google.insert("interaction_usage".to_string(), interaction_usage.clone());
        }
        if !interaction_annotations.is_empty() {
            google.insert(
                "interaction_annotations".to_string(),
                Value::Array(interaction_annotations.to_vec()),
            );
        }
        if let Some(service_tier) = service_tier.filter(|value| !value.is_empty()) {
            google.insert("service_tier".to_string(), json!(service_tier));
        }
        (!google.is_empty()).then(|| json!({"google": google}))
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
    for field in ["arguments", "action", "result", "results", "signature"] {
        if let Some(value) = step.get(field) {
            summary.insert(field.to_string(), bounded_interaction_trace_value(value));
        }
    }
    Value::Object(summary)
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

fn gemini_mcp_step_to_anthropic(step: &Value) -> Option<Value> {
    match step.get("type").and_then(Value::as_str)? {
        "mcp_server_tool_call" => {
            let id = step.get("id").and_then(Value::as_str)?;
            let name = step.get("name").and_then(Value::as_str)?;
            let server_name = step.get("server_name").and_then(Value::as_str)?;
            Some(json!({
                "type": "mcp_tool_use",
                "id": id,
                "name": name,
                "server_name": server_name,
                "input": step.get("arguments").cloned().unwrap_or_else(|| json!({}))
            }))
        }
        "mcp_server_tool_result" => {
            let call_id = step.get("call_id").and_then(Value::as_str)?;
            let result = step.get("result").unwrap_or(&Value::Null);
            let content = match result {
                Value::Array(parts) => parts
                    .iter()
                    .map(|part| {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            json!({
                                "type": "text",
                                "text": part.get("text").and_then(Value::as_str).unwrap_or_default()
                            })
                        } else {
                            json!({"type": "text", "text": part.to_string()})
                        }
                    })
                    .collect::<Vec<_>>(),
                Value::String(text) => vec![json!({"type": "text", "text": text})],
                Value::Null => Vec::new(),
                value => vec![json!({"type": "text", "text": value.to_string()})],
            };
            Some(json!({
                "type": "mcp_tool_result",
                "tool_use_id": call_id,
                "is_error": step.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                "content": content
            }))
        }
        _ => None,
    }
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
        &[
            "total_thought_tokens",
            "total_reasoning_tokens",
            "reasoning_tokens",
        ],
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
    if let Some(tokens) = usage_token(usage, &["total_tool_use_tokens", "tool_use_tokens"]) {
        translated["tool_use_tokens"] = json!(tokens);
    }
    if let Some(tokens) = usage_token(usage, &["total_tokens"]) {
        translated["total_tokens"] = json!(tokens);
    }
    translated
}

fn attach_anthropic_inference_usage(usage: &mut Value, service_tier: Option<&str>) {
    let Some(service_tier) = service_tier.filter(|value| !value.is_empty()) else {
        return;
    };
    match service_tier {
        "priority" => {
            usage["service_tier"] = json!("priority");
            usage["speed"] = json!("fast");
        }
        "standard" => {
            usage["service_tier"] = json!("standard");
            usage["speed"] = json!("standard");
        }
        "flex" => {
            // Claude has no Flex usage enum. Preserve the exact provider tier in
            // provider_metadata and report only its standard output speed here.
            usage["speed"] = json!("standard");
        }
        _ => {}
    }
}

fn interaction_stop_reason(status: &str, has_calls: bool) -> &'static str {
    if has_calls || status == "requires_action" {
        "tool_use"
    } else if matches!(status, "incomplete" | "budget_exceeded") {
        "max_tokens"
    } else {
        "end_turn"
    }
}

#[cfg(test)]
fn translate_gemini_interactions_response(
    upstream: &Value,
    model: &str,
) -> Result<InteractionResponseTranslation, String> {
    translate_gemini_interactions_response_with_service_tier(upstream, model, None)
}

#[cfg(test)]
fn translate_gemini_interactions_response_with_service_tier(
    upstream: &Value,
    model: &str,
    actual_service_tier: Option<&str>,
) -> Result<InteractionResponseTranslation, String> {
    translate_gemini_interactions_response_for_request(upstream, model, actual_service_tier, None)
}

fn translate_gemini_interactions_response_for_request(
    upstream: &Value,
    model: &str,
    actual_service_tier: Option<&str>,
    request: Option<&Value>,
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
    if matches!(status, "queued" | "in_progress") {
        return Err(format!(
            "Gemini Interactions returned non-terminal status '{status}' to a synchronous request"
        ));
    }
    let steps = upstream
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| "Gemini Interactions response has no steps array".to_string())?;
    let mut content = Vec::new();
    let mut calls = Vec::new();
    let computer_context = request.and_then(computer_conversation_context);
    let mut native_computer_calls = Vec::new();
    let mut interaction_annotations = Vec::new();
    let mut server_tools = InteractionServerToolTrace::default();
    for (step_index, step) in steps.iter().enumerate() {
        match step.get("type").and_then(Value::as_str) {
            Some("thought") => {
                let thinking = value_to_text(step.get("summary").unwrap_or(&Value::Null));
                let signature = step
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                if thinking.is_empty() && signature.is_none() {
                    continue;
                }
                let mut block = json!({"type": "thinking", "thinking": thinking});
                if let Some(signature) = signature {
                    block["signature"] = json!(signature);
                }
                content.push(block);
            }
            Some("model_output") => {
                if let Some(parts) = step.get("content").and_then(Value::as_array) {
                    for (part_index, part) in parts.iter().enumerate() {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            let mut content_block_index = Value::Null;
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    content_block_index = json!(content.len());
                                    content.push(json!({"type": "text", "text": text}));
                                }
                            }
                            if let Some(annotations) = part
                                .get("annotations")
                                .and_then(Value::as_array)
                                .filter(|annotations| !annotations.is_empty())
                            {
                                interaction_annotations.push(json!({
                                    "step_index": step_index,
                                    "part_index": part_index,
                                    "content_block_index": content_block_index,
                                    "annotations": annotations
                                }));
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
                if computer_context.as_ref().is_some_and(|context| is_gemini_computer_action(&name, &context.environment)) {
                    let mut native_call = step.clone();
                    native_call["id"] = json!(call_id);
                    native_computer_calls.push(native_call);
                    continue;
                }
                let input = step.get("arguments").cloned().unwrap_or_else(|| json!({}));
                calls.push((call_id.clone(), name.clone()));
                content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                }));
            }
            Some(step_type @ ("mcp_server_tool_call" | "mcp_server_tool_result")) => {
                server_tools.capture(step);
                if let Some(block) = gemini_mcp_step_to_anthropic(step) {
                    content.push(block);
                }
                info!(step_type, "Gemini MCP server-side tool step completed");
            }
            Some(step_type) if is_gemini_server_tool_step(step_type) => {
                server_tools.capture(step);
                info!(step_type, "Gemini server-side tool step completed");
            }
            _ => {}
        }
    }
    if let Some(request) = request {
        if let Some((block, call)) = computer_batch_tool_block(&interaction_id, request, &native_computer_calls) {
            content.push(block);
            calls.push(call);
        }
    }
    let interaction_usage = upstream.get("usage").unwrap_or(&Value::Null);
    let resolved_service_tier =
        actual_service_tier.or_else(|| upstream.get("service_tier").and_then(Value::as_str));
    let mut usage = gemini_interaction_usage_to_anthropic(interaction_usage, 0);
    attach_anthropic_inference_usage(&mut usage, resolved_service_tier);
    let stop_reason = interaction_stop_reason(status, !calls.is_empty());
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
    if let Some(metadata) = server_tools.provider_metadata(
        interaction_usage,
        &interaction_annotations,
        resolved_service_tier,
    ) {
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
        let upstream_request = profile
            .client
            .post(&profile.upstream_url)
            .header("x-goog-api-key", api_key)
            .json(&interaction_request);
        let upstream_request =
            apply_upstream_total_timeout(upstream_request, stream_requested);
        let upstream_request = if !stream_requested
            && interaction_request.get("service_tier").and_then(Value::as_str) == Some("flex")
        {
            upstream_request.timeout(GEMINI_FLEX_REQUEST_TIMEOUT)
        } else {
            upstream_request
        };
        let response = match upstream_request.send().await {
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

        if !mixed_tools_fallback && is_mixed_interaction_tools_error(status.as_u16(), &message) {
            if interaction_request_has_computer_use(&interaction_request)
                && remove_interaction_tools_incompatible_with_computer_use(
                    &mut interaction_request,
                )
            {
                mixed_tools_fallback = true;
                warn!(
                    provider = %profile.file_name,
                    upstream_error = %message,
                    "Gemini rejected tools combined with computer_use; retrying with computer_use preserved as the sole tool"
                );
                continue;
            }
            if interaction_request_has_mixed_tools(&interaction_request)
                && remove_interaction_server_tools(&mut interaction_request)
            {
                mixed_tools_fallback = true;
                warn!(
                    provider = %profile.file_name,
                    "Gemini rejected mixed function and server-side tools; retrying this request with Claude Code function tools only"
                );
                continue;
            }
        }

        if !continuation_fallback
            && is_interaction_continuation_unavailable(
                status.as_u16(),
                &message,
                &interaction_request,
            )
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
                if interaction_request_has_computer_use(&interaction_request) {
                    remove_interaction_tools_incompatible_with_computer_use(
                        &mut interaction_request,
                    );
                } else {
                    remove_interaction_server_tools(&mut interaction_request);
                }
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
        let flex_requested =
            interaction_request.get("service_tier").and_then(Value::as_str) == Some("flex");
        return gemini_interactions_stream_response(
            upstream,
            model,
            profile.file_name,
            request,
            continuations,
            GeminiInteractionsStreamOptions {
                estimated_input_tokens,
                store_interactions: profile.openai_capabilities.gemini_store,
                flex_requested,
            },
        );
    }
    let actual_service_tier = upstream
        .headers()
        .get("x-gemini-service-tier")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
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
    let translated = match translate_gemini_interactions_response_for_request(
        &upstream_body,
        &model,
        actual_service_tier.as_deref(),
        Some(&request),
    ) {
        Ok(value) => value,
        Err(message) => {
            error!(
                "Cannot translate provider '{}' Gemini Interactions response: {message}",
                profile.file_name
            );
            return anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &message);
        }
    };
    if profile.openai_capabilities.gemini_store {
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
