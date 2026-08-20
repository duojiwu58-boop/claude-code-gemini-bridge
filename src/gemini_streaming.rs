enum InteractionStreamingBlock {
    Thought {
        thinking: String,
        signature: Option<String>,
    },
    Text {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        arguments: String,
    },
}

struct ActiveInteractionStreamingBlock {
    anthropic_index: usize,
    block: InteractionStreamingBlock,
}

struct GeminiInteractionsStreamTranslator {
    message_id: String,
    model: String,
    profile_file: String,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
    interaction_id: Option<String>,
    status: Option<String>,
    usage: Value,
    input_tokens: u64,
    next_content_index: usize,
    active_blocks: IndexMap<usize, ActiveInteractionStreamingBlock>,
    assistant_content: Vec<Value>,
    calls: Vec<(String, String)>,
    server_tools: InteractionServerToolTrace,
    completed: bool,
    finished: bool,
}

impl GeminiInteractionsStreamTranslator {
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
            interaction_id: None,
            status: None,
            usage: json!({}),
            input_tokens: estimated_input_tokens,
            next_content_index: 0,
            active_blocks: IndexMap::new(),
            assistant_content: Vec::new(),
            calls: Vec::new(),
            server_tools: InteractionServerToolTrace::default(),
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
                        "input_tokens": self.input_tokens,
                        "output_tokens": 0
                    }
                }
            }),
        )?;
        Ok(events)
    }

    fn process_payload(&mut self, payload: &str) -> Result<Vec<Event>, String> {
        if payload.trim().is_empty() {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(payload)
            .map_err(|err| format!("Invalid JSON in Gemini Interactions SSE stream: {err}"))?;
        let event_type = event
            .get("event_type")
            .or_else(|| event.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event.get("error").is_some() || event_type == "error" {
            return Err(safe_error_message(&event));
        }
        if let Some(usage) = event.pointer("/metadata/total_usage") {
            self.capture_usage(usage);
        }
        match event_type {
            "interaction.created"
            | "interaction.in_progress"
            | "interaction.requires_action"
            | "interaction.status_update" => {
                if let Some(interaction) = event.get("interaction") {
                    self.capture_interaction(interaction, false);
                } else {
                    if let Some(id) = event.get("interaction_id").and_then(Value::as_str) {
                        self.interaction_id = Some(id.to_string());
                    }
                    if let Some(status) = event.get("status").and_then(Value::as_str) {
                        self.status = Some(status.to_string());
                    }
                }
                if event_type == "interaction.requires_action" {
                    self.completed = true;
                }
                Ok(Vec::new())
            }
            "interaction.completed" => {
                if let Some(interaction) = event.get("interaction") {
                    self.capture_interaction(interaction, true);
                } else {
                    self.capture_interaction(&event, true);
                }
                Ok(Vec::new())
            }
            "step.start" => self.start_step(&event),
            "step.delta" => self.delta_step(&event),
            "step.stop" => self.stop_step(&event),
            _ => Ok(Vec::new()),
        }
    }

    fn capture_interaction(&mut self, interaction: &Value, completed: bool) {
        if let Some(id) = interaction.get("id").and_then(Value::as_str) {
            self.interaction_id = Some(id.to_string());
        }
        if let Some(status) = interaction.get("status").and_then(Value::as_str) {
            self.status = Some(status.to_string());
        }
        if let Some(usage) = interaction.get("usage") {
            self.capture_usage(usage);
        }
        if let Some(steps) = interaction.get("steps").and_then(Value::as_array) {
            for step in steps {
                self.server_tools.capture(step);
            }
        }
        self.completed |= completed;
        if let Some(interaction_id) = self.interaction_id.as_deref() {
            remember_interaction_calls(
                &self.continuations,
                &self.profile_file,
                interaction_id,
                &self.calls,
            );
        }
    }

    fn capture_usage(&mut self, usage: &Value) {
        self.usage = usage.clone();
        self.input_tokens = usage_token(
            usage,
            &["total_input_tokens", "prompt_tokens", "input_tokens"],
        )
        .unwrap_or(self.input_tokens);
    }

    fn start_step(&mut self, event: &Value) -> Result<Vec<Event>, String> {
        let index = event
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "Gemini Interactions step.start has no valid index".to_string())?;
        let step = event.get("step").unwrap_or(&Value::Null);
        let Some(step_type) = step.get("type").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        let anthropic_index = self.next_content_index;
        let (block, content_block) = match step_type {
            "thought" => (
                InteractionStreamingBlock::Thought {
                    thinking: String::new(),
                    signature: None,
                },
                json!({"type": "thinking", "thinking": ""}),
            ),
            "model_output" => (
                InteractionStreamingBlock::Text {
                    text: String::new(),
                },
                json!({"type": "text", "text": ""}),
            ),
            "function_call" => {
                let id = step
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
                (
                    InteractionStreamingBlock::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    },
                    json!({"type": "tool_use", "id": id, "name": name, "input": {}}),
                )
            }
            _ => {
                if is_gemini_server_tool_step(step_type) {
                    self.server_tools.capture(step);
                    info!(
                        provider = %self.profile_file,
                        step_type,
                        "Gemini server-side tool step started"
                    );
                }
                return Ok(Vec::new());
            }
        };
        self.next_content_index += 1;
        self.active_blocks.insert(
            index,
            ActiveInteractionStreamingBlock {
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
        if let Some(active) = self.active_blocks.get_mut(&index) {
            match &mut active.block {
                InteractionStreamingBlock::Thought {
                    thinking,
                    signature,
                } => {
                    let initial_thinking = value_to_text(
                        step.get("summary")
                            .or_else(|| step.get("content"))
                            .unwrap_or(&Value::Null),
                    );
                    if !initial_thinking.is_empty() {
                        thinking.push_str(&initial_thinking);
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "thinking_delta", "thinking": initial_thinking}
                            }),
                        )?;
                    }
                    if let Some(value) = step.get("signature").and_then(Value::as_str) {
                        *signature = Some(value.to_string());
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "signature_delta", "signature": value}
                            }),
                        )?;
                    }
                }
                InteractionStreamingBlock::Text { text } => {
                    let initial_text =
                        value_to_text(step.get("content").unwrap_or(&Value::Null));
                    if !initial_text.is_empty() {
                        text.push_str(&initial_text);
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "text_delta", "text": initial_text}
                            }),
                        )?;
                    }
                }
                InteractionStreamingBlock::Tool { .. } => {}
            }
        }
        Ok(events)
    }

    fn delta_step(&mut self, event: &Value) -> Result<Vec<Event>, String> {
        let Some(index) = event
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return Ok(Vec::new());
        };
        let Some(active) = self.active_blocks.get_mut(&index) else {
            return Ok(Vec::new());
        };
        let delta = event.get("delta").unwrap_or(&Value::Null);
        let delta_type = delta
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut events = Vec::new();
        match &mut active.block {
            InteractionStreamingBlock::Thought {
                thinking,
                signature,
            } => match delta_type {
                "thought" | "thought_summary" => {
                    let text = delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            value_to_text(delta.get("content").unwrap_or(&Value::Null))
                        });
                    if !text.is_empty() {
                        thinking.push_str(&text);
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
                "thought_signature" => {
                    if let Some(value) = delta.get("signature").and_then(Value::as_str) {
                        *signature = Some(value.to_string());
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "signature_delta", "signature": value}
                            }),
                        )?;
                    }
                }
                _ => {}
            },
            InteractionStreamingBlock::Text { text } => {
                if delta_type == "text" || delta.get("text").is_some() {
                    if let Some(value) = delta.get("text").and_then(Value::as_str) {
                        if !value.is_empty() {
                            text.push_str(value);
                            push_anthropic_event(
                                &mut events,
                                "content_block_delta",
                                json!({
                                    "type": "content_block_delta",
                                    "index": active.anthropic_index,
                                    "delta": {"type": "text_delta", "text": value}
                                }),
                            )?;
                        }
                    }
                }
            }
            InteractionStreamingBlock::Tool { arguments, .. } => {
                if delta_type == "arguments_delta" || delta_type == "arguments" {
                    if let Some(value) = delta
                        .get("arguments")
                        .or_else(|| delta.get("partial_arguments"))
                        .and_then(Value::as_str)
                    {
                        arguments.push_str(value);
                        push_anthropic_event(
                            &mut events,
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": active.anthropic_index,
                                "delta": {"type": "input_json_delta", "partial_json": value}
                            }),
                        )?;
                    }
                }
            }
        }
        Ok(events)
    }

    fn stop_step(&mut self, event: &Value) -> Result<Vec<Event>, String> {
        if let Some(step) = event.get("step") {
            self.server_tools.capture(step);
        }
        let Some(index) = event
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return Ok(Vec::new());
        };
        let Some(active) = self.active_blocks.shift_remove(&index) else {
            return Ok(Vec::new());
        };
        match active.block {
            InteractionStreamingBlock::Thought {
                thinking,
                signature,
            } => {
                let mut block = json!({"type": "thinking", "thinking": thinking});
                if let Some(signature) = signature {
                    block["signature"] = json!(signature);
                }
                self.assistant_content.push(block);
            }
            InteractionStreamingBlock::Text { text } => {
                self.assistant_content
                    .push(json!({"type": "text", "text": text}));
            }
            InteractionStreamingBlock::Tool {
                id,
                name,
                arguments,
            } => {
                let input = parse_tool_arguments(if arguments.is_empty() {
                    "{}"
                } else {
                    &arguments
                })?;
                let call = (id.clone(), name.clone());
                self.calls.push(call.clone());
                if let Some(interaction_id) = self.interaction_id.as_deref() {
                    remember_interaction_calls(
                        &self.continuations,
                        &self.profile_file,
                        interaction_id,
                        std::slice::from_ref(&call),
                    );
                }
                self.assistant_content.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
        }
        let mut events = Vec::new();
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
            return Err(
                "Gemini Interactions stream ended before interaction.completed".to_string(),
            );
        }
        if !self.active_blocks.is_empty() {
            return Err("Gemini Interactions stream ended with an unfinished step".to_string());
        }
        let interaction_id = self.interaction_id.as_deref().ok_or_else(|| {
            "Gemini Interactions stream completed without an interaction id".to_string()
        })?;
        remember_interaction_continuation(
            &self.continuations,
            &self.profile_file,
            &self.request,
            interaction_id,
            &self.assistant_content,
            &self.calls,
        );
        let status = self.status.as_deref().unwrap_or("completed");
        let stop_reason = if !self.calls.is_empty() || status == "requires_action" {
            "tool_use"
        } else if status == "incomplete" {
            "max_tokens"
        } else {
            "end_turn"
        };
        self.finished = true;
        let mut events = Vec::new();
        let mut delta = json!({"stop_reason": stop_reason, "stop_sequence": Value::Null});
        if let Some(metadata) = self.server_tools.provider_metadata() {
            delta["provider_metadata"] = metadata;
        }
        let mut usage = gemini_interaction_usage_to_anthropic(&self.usage, self.input_tokens);
        if let Some(object) = usage.as_object_mut() {
            object.remove("input_tokens");
        }
        if let Some(server_tool_use) = self.server_tools.anthropic_usage() {
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

fn gemini_interactions_event_stream<S, B, E>(
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
    let translator = GeminiInteractionsStreamTranslator::new(
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
                match byte_stream.next().await {
                    Some(Ok(bytes)) => match decoder.push_bytes(bytes.as_ref()) {
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
                    Some(Err(err)) => {
                        pending.push_back(anthropic_stream_error_event(&format!(
                            "Gemini Interactions stream failed: {err}"
                        )));
                        ended = true;
                    }
                    None => {
                        let mut processing_failed = false;
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
                                            processing_failed = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(message) => {
                                pending.push_back(anthropic_stream_error_event(&message));
                                processing_failed = true;
                            }
                        }
                        if !processing_failed {
                            match translator.finish() {
                                Ok(events) => pending.extend(events),
                                Err(message) => {
                                    pending.push_back(anthropic_stream_error_event(&message))
                                }
                            }
                        }
                        ended = true;
                    }
                }
            }
        },
    )
}

fn gemini_interactions_stream_response(
    upstream: reqwest::Response,
    model: String,
    profile_file: String,
    request: Value,
    continuations: Arc<InteractionContinuationState>,
    estimated_input_tokens: u64,
) -> Response {
    let event_stream = gemini_interactions_event_stream(
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
