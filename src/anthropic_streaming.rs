#[derive(Default)]
struct SseDataDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
    data_bytes: usize,
}

impl SseDataDecoder {
    fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        let mut payloads = Vec::new();
        let mut cursor = 0;
        while let Some(relative_newline) = bytes[cursor..].iter().position(|byte| *byte == b'\n') {
            let newline = cursor + relative_newline;
            self.extend_buffer(&bytes[cursor..newline])?;
            let mut line_bytes = std::mem::take(&mut self.buffer);
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8_lossy(&line_bytes);
            self.process_line(&line, &mut payloads)?;
            cursor = newline + 1;
        }
        self.extend_buffer(&bytes[cursor..])?;

        Ok(payloads)
    }

    fn finish(&mut self) -> Result<Vec<String>, String> {
        let mut payloads = Vec::new();
        if !self.buffer.is_empty() {
            let mut line_bytes = std::mem::take(&mut self.buffer);
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8_lossy(&line_bytes);
            self.process_line(&line, &mut payloads)?;
        }
        self.flush_data(&mut payloads);
        Ok(payloads)
    }

    fn extend_buffer(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.buffer.len().saturating_add(bytes.len()) > MAX_UPSTREAM_SSE_BUFFER_BYTES {
            return Err(format!(
                "OpenAI-compatible SSE line exceeds {} bytes",
                MAX_UPSTREAM_SSE_BUFFER_BYTES
            ));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn process_line(&mut self, line: &str, payloads: &mut Vec<String>) -> Result<(), String> {
        if line.is_empty() {
            self.flush_data(payloads);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            let separator_bytes = usize::from(!self.data_lines.is_empty());
            if self
                .data_bytes
                .saturating_add(separator_bytes)
                .saturating_add(data.len())
                > MAX_UPSTREAM_SSE_BUFFER_BYTES
            {
                return Err(format!(
                    "OpenAI-compatible SSE event exceeds {} bytes",
                    MAX_UPSTREAM_SSE_BUFFER_BYTES
                ));
            }
            self.data_bytes += separator_bytes + data.len();
            self.data_lines.push(data.to_string());
        }
        Ok(())
    }

    fn flush_data(&mut self, payloads: &mut Vec<String>) {
        if !self.data_lines.is_empty() {
            payloads.push(std::mem::take(&mut self.data_lines).join("\n"));
            self.data_bytes = 0;
        }
    }
}

#[derive(Default)]
struct StreamingToolCall {
    id: String,
    name: String,
    arguments: String,
    thought_signature: Option<String>,
}

fn finish_reason_is(finish_reason: Option<&str>, expected: &[&str]) -> bool {
    finish_reason.is_some_and(|reason| {
        expected
            .iter()
            .any(|candidate| reason.eq_ignore_ascii_case(candidate))
    })
}

fn anthropic_stop_reason(finish_reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if finish_reason_is(finish_reason, &["length", "max_tokens"]) {
        "max_tokens"
    } else if finish_reason_is(finish_reason, &["content_filter", "safety", "blocked"]) {
        "refusal"
    } else if has_tool_calls {
        "tool_use"
    } else {
        "end_turn"
    }
}

fn stream_eof_is_complete(
    saw_done: bool,
    finish_reason: Option<&str>,
    usage_only_tail_seen: bool,
) -> bool {
    saw_done || finish_reason.is_some() || usage_only_tail_seen
}

fn parse_tool_arguments(arguments: &str) -> Result<Value, String> {
    parse_tool_arguments_with_json(arguments).map(|(input, _)| input)
}

fn parse_tool_arguments_with_json(arguments: &str) -> Result<(Value, String), String> {
    let arguments = if arguments.is_empty() {
        "{}"
    } else {
        arguments
    };
    let (input, normalized): (Value, String) = match serde_json::from_str(arguments) {
        Ok(input) => (input, arguments.to_string()),
        Err(original_error) => {
            let repaired = repair_tool_arguments_json(arguments).ok_or_else(|| {
                format!("Upstream returned invalid tool arguments JSON: {original_error}")
            })?;
            let input: Value = serde_json::from_str(&repaired).map_err(|_| {
                format!("Upstream returned invalid tool arguments JSON: {original_error}")
            })?;
            let normalized = input.to_string();
            (input, normalized)
        }
    };
    if !input.is_object() {
        return Err("Upstream tool arguments must be a JSON object".to_string());
    }
    Ok((input, normalized))
}

fn repair_tool_arguments_json(arguments: &str) -> Option<String> {
    let mut repaired = String::with_capacity(arguments.len() + 8);
    let mut expected_closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for character in arguments.chars() {
        if in_string {
            if escaped {
                repaired.push(character);
                escaped = false;
                continue;
            }
            match character {
                '\\' => {
                    repaired.push(character);
                    escaped = true;
                }
                '"' => {
                    repaired.push(character);
                    in_string = false;
                }
                '\n' => repaired.push_str("\\n"),
                '\r' => repaired.push_str("\\r"),
                '\t' => repaired.push_str("\\t"),
                character if character.is_control() => {
                    use std::fmt::Write as _;
                    write!(&mut repaired, "\\u{:04x}", character as u32).ok()?;
                }
                _ => repaired.push(character),
            }
            continue;
        }

        match character {
            '"' => {
                repaired.push(character);
                in_string = true;
            }
            '{' => {
                repaired.push(character);
                expected_closers.push('}');
            }
            '[' => {
                repaired.push(character);
                expected_closers.push(']');
            }
            '}' | ']' => {
                if expected_closers.pop() != Some(character) {
                    return None;
                }
                while repaired.ends_with(char::is_whitespace) {
                    repaired.pop();
                }
                if repaired.ends_with(',') {
                    repaired.pop();
                }
                repaired.push(character);
            }
            _ => repaired.push(character),
        }
    }

    // Never invent the end of a string: that can materially change a tool
    // argument. Closing structurally balanced containers is deterministic.
    if in_string || escaped {
        return None;
    }
    while let Some(closer) = expected_closers.pop() {
        while repaired.ends_with(char::is_whitespace) {
            repaired.pop();
        }
        if repaired.ends_with(',') {
            repaired.pop();
        }
        repaired.push(closer);
    }
    (repaired != arguments).then_some(repaired)
}

fn text_from_fields(value: &Value, fields: &[String]) -> Option<String> {
    fields.iter().find_map(|field| {
        let value = value.get(field)?;
        if !value.is_string() && !value.is_array() {
            return None;
        }
        let text = value_to_text(value);
        (!text.is_empty()).then_some(text)
    })
}

fn split_tagged_thinking(text: &str) -> Option<(&str, &str)> {
    let trimmed = text.trim_start();
    for (open_tag, close_tag) in [("<think>", "</think>"), ("<thought>", "</thought>")] {
        let Some(prefix) = trimmed.get(..open_tag.len()) else {
            continue;
        };
        if !prefix.eq_ignore_ascii_case(open_tag) {
            continue;
        }
        let body = &trimmed[open_tag.len()..];
        let lowered = body.to_ascii_lowercase();
        let end = lowered.find(close_tag)?;
        return Some((&body[..end], &body[end + close_tag.len()..]));
    }
    None
}

fn tool_arguments_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(arguments)) if !arguments.is_empty() => arguments.clone(),
        Some(Value::Null) | None | Some(Value::String(_)) => "{}".to_string(),
        Some(arguments) => arguments.to_string(),
    }
}

fn usage_token(usage: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| usage.get(*field).and_then(Value::as_u64))
}

fn safety_refusal_text(block_reason: &str) -> String {
    format!(
        "Gemini Safety Intercept: Request was blocked by safety guardrails (Reason: {block_reason})."
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaggedContentState {
    Detecting,
    Thinking(&'static str),
    Text,
}

struct AnthropicStreamTranslator {
    message_id: String,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    capabilities: OpenAiCapabilities,
    next_content_index: usize,
    thinking_block_index: Option<usize>,
    text_block_index: Option<usize>,
    tagged_content_state: TaggedContentState,
    tagged_content_buffer: String,
    assistant_thought_signature: Option<String>,
    tool_calls: IndexMap<String, StreamingToolCall>,
    next_anonymous_tool: usize,
    finish_reason: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    usage_only_tail_seen: bool,
    refusal_seen: bool,
    safety_block_seen: bool,
    finished: bool,
}

impl AnthropicStreamTranslator {
    #[cfg(test)]
    fn new(
        model: String,
        thought_signatures: Arc<ThoughtSignatureCache>,
        estimated_input_tokens: u64,
    ) -> Self {
        Self::with_capabilities(
            model,
            thought_signatures,
            estimated_input_tokens,
            OpenAiCapabilities::default(),
        )
    }

    fn with_capabilities(
        model: String,
        thought_signatures: Arc<ThoughtSignatureCache>,
        estimated_input_tokens: u64,
        capabilities: OpenAiCapabilities,
    ) -> Self {
        let tagged_content_state = if capabilities.thinking_tags {
            TaggedContentState::Detecting
        } else {
            TaggedContentState::Text
        };
        Self {
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            model,
            thought_signatures,
            capabilities,
            next_content_index: 0,
            thinking_block_index: None,
            text_block_index: None,
            tagged_content_state,
            tagged_content_buffer: String::new(),
            assistant_thought_signature: None,
            tool_calls: IndexMap::new(),
            next_anonymous_tool: 0,
            finish_reason: None,
            input_tokens: estimated_input_tokens,
            output_tokens: 0,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_tokens: None,
            usage_only_tail_seen: false,
            refusal_seen: false,
            safety_block_seen: false,
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
        let chunk: Value = serde_json::from_str(payload)
            .map_err(|err| format!("Invalid JSON in OpenAI-compatible SSE stream: {err}"))?;
        if chunk.get("error").is_some() {
            return Err(safe_error_message(&chunk));
        }

        if let Some(block_reason) = chunk
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
        {
            self.refusal_seen = true;
            if self.safety_block_seen {
                return Ok(Vec::new());
            }
            self.safety_block_seen = true;
            let mut events = Vec::new();
            self.emit_text_delta(&mut events, &safety_refusal_text(block_reason))?;
            return Ok(events);
        }

        let usage_seen = chunk.get("usage").is_some();
        if let Some(usage) = chunk.get("usage") {
            self.input_tokens =
                usage_token(usage, &["prompt_tokens", "input_tokens"]).unwrap_or(self.input_tokens);
            self.output_tokens = usage_token(usage, &["completion_tokens", "output_tokens"])
                .unwrap_or(self.output_tokens);
            self.cache_read_input_tokens = usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
                .and_then(Value::as_u64)
                .or_else(|| usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64))
                .or_else(|| usage.get("cached_tokens").and_then(Value::as_u64))
                .or(self.cache_read_input_tokens);
            self.cache_creation_input_tokens = usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .or(self.cache_creation_input_tokens);
            self.reasoning_tokens = usage
                .pointer("/completion_tokens_details/reasoning_tokens")
                .or_else(|| usage.pointer("/output_tokens_details/reasoning_tokens"))
                .and_then(Value::as_u64)
                .or(self.reasoning_tokens);
        }

        let Some(choice) = chunk.pointer("/choices/0") else {
            self.usage_only_tail_seen = usage_seen;
            return Ok(Vec::new());
        };
        self.usage_only_tail_seen = false;
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(Vec::new());
        };

        let mut events = Vec::new();
        if let Some(signature) = delta
            .pointer("/extra_content/google/thought_signature")
            .and_then(Value::as_str)
        {
            self.assistant_thought_signature = Some(signature.to_string());
        }
        let reasoning_text = text_from_fields(delta, &self.capabilities.reasoning_fields);
        if let Some(reasoning_text) = reasoning_text {
            self.emit_thinking_delta(&mut events, &reasoning_text)?;
        }

        for field in ["content", "refusal"] {
            let text = value_to_text(delta.get(field).unwrap_or(&Value::Null));
            if text.is_empty() {
                continue;
            }
            if field == "refusal" {
                self.refusal_seen = true;
                self.emit_text_delta(&mut events, &text)?;
            } else {
                self.process_content_delta(&mut events, &text)?;
            }
        }

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            if !tool_calls.is_empty() {
                self.stop_thinking_block(&mut events)?;
            }
            for tool_call in tool_calls {
                self.accumulate_tool_call(tool_call)?;
            }
        }
        if let Some(function_call) = delta.get("function_call") {
            self.stop_thinking_block(&mut events)?;
            self.accumulate_tool_call(&json!({
                "index": 0,
                "function": function_call
            }))?;
        }
        Ok(events)
    }

    fn emit_thinking_delta(
        &mut self,
        events: &mut Vec<Event>,
        thinking: &str,
    ) -> Result<(), String> {
        if thinking.is_empty() {
            return Ok(());
        }
        self.stop_text_block(events)?;
        let index = if let Some(index) = self.thinking_block_index {
            index
        } else {
            let index = self.next_content_index;
            self.next_content_index += 1;
            self.thinking_block_index = Some(index);
            push_anthropic_event(
                events,
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "thinking", "thinking": ""}
                }),
            )?;
            index
        };
        push_anthropic_event(
            events,
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "thinking_delta", "thinking": thinking}
            }),
        )
    }

    fn process_content_delta(&mut self, events: &mut Vec<Event>, text: &str) -> Result<(), String> {
        match self.tagged_content_state {
            TaggedContentState::Text => self.emit_text_delta(events, text),
            TaggedContentState::Thinking(close_tag) => {
                self.process_tagged_thinking(events, text, close_tag)
            }
            TaggedContentState::Detecting => {
                self.tagged_content_buffer.push_str(text);
                let trimmed = self.tagged_content_buffer.trim_start();
                let lowered = trimmed.to_ascii_lowercase();
                let tags = [("<think>", "</think>"), ("<thought>", "</thought>")];
                if tags
                    .iter()
                    .any(|(open_tag, _)| open_tag.starts_with(&lowered))
                    && self.tagged_content_buffer.len() <= 64
                {
                    return Ok(());
                }
                if let Some((open_tag, close_tag)) = tags
                    .into_iter()
                    .find(|(open_tag, _)| lowered.starts_with(open_tag))
                {
                    let leading_bytes = self.tagged_content_buffer.len() - trimmed.len();
                    let rest =
                        self.tagged_content_buffer[leading_bytes + open_tag.len()..].to_string();
                    self.tagged_content_buffer.clear();
                    self.tagged_content_state = TaggedContentState::Thinking(close_tag);
                    return self.process_tagged_thinking(events, &rest, close_tag);
                }

                self.tagged_content_state = TaggedContentState::Text;
                let buffered = std::mem::take(&mut self.tagged_content_buffer);
                self.emit_text_delta(events, &buffered)
            }
        }
    }

    fn process_tagged_thinking(
        &mut self,
        events: &mut Vec<Event>,
        text: &str,
        close_tag: &'static str,
    ) -> Result<(), String> {
        self.tagged_content_buffer.push_str(text);
        let lowered = self.tagged_content_buffer.to_ascii_lowercase();
        if let Some(end) = lowered.find(close_tag) {
            let thinking = self.tagged_content_buffer[..end].to_string();
            let remaining = self.tagged_content_buffer[end + close_tag.len()..].to_string();
            self.tagged_content_buffer.clear();
            self.emit_thinking_delta(events, &thinking)?;
            self.tagged_content_state = TaggedContentState::Text;
            if !remaining.is_empty() {
                self.emit_text_delta(events, &remaining)?;
            }
            return Ok(());
        }

        let retained = (1..close_tag.len())
            .rev()
            .find(|length| lowered.ends_with(&close_tag[..*length]))
            .unwrap_or(0);
        let emit_length = self.tagged_content_buffer.len() - retained;
        if emit_length > 0 {
            let thinking = self.tagged_content_buffer[..emit_length].to_string();
            self.tagged_content_buffer.drain(..emit_length);
            self.emit_thinking_delta(events, &thinking)?;
        }
        Ok(())
    }

    fn flush_tagged_content(&mut self, events: &mut Vec<Event>) -> Result<(), String> {
        if self.tagged_content_buffer.is_empty() {
            return Ok(());
        }
        let buffered = std::mem::take(&mut self.tagged_content_buffer);
        match self.tagged_content_state {
            TaggedContentState::Thinking(_) => self.emit_thinking_delta(events, &buffered),
            TaggedContentState::Detecting | TaggedContentState::Text => {
                self.tagged_content_state = TaggedContentState::Text;
                self.emit_text_delta(events, &buffered)
            }
        }
    }

    fn stop_thinking_block(&mut self, events: &mut Vec<Event>) -> Result<(), String> {
        if let Some(index) = self.thinking_block_index.take() {
            if let Some(signature) = self.assistant_thought_signature.take() {
                push_anthropic_event(
                    events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "signature_delta", "signature": signature}
                    }),
                )?;
            }
            push_anthropic_event(
                events,
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )?;
        }
        Ok(())
    }

    fn stop_text_block(&mut self, events: &mut Vec<Event>) -> Result<(), String> {
        if let Some(index) = self.text_block_index.take() {
            push_anthropic_event(
                events,
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )?;
        }
        Ok(())
    }

    fn emit_text_delta(&mut self, events: &mut Vec<Event>, text: &str) -> Result<(), String> {
        self.stop_thinking_block(events)?;
        let index = if let Some(index) = self.text_block_index {
            index
        } else {
            let index = self.next_content_index;
            self.next_content_index += 1;
            self.text_block_index = Some(index);
            push_anthropic_event(
                events,
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "text", "text": ""}
                }),
            )?;
            index
        };
        push_anthropic_event(
            events,
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "text_delta", "text": text}
            }),
        )
    }

    fn accumulate_tool_call(&mut self, tool_call: &Value) -> Result<(), String> {
        let index = tool_call.get("index").and_then(Value::as_u64);
        let incoming_id = tool_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());
        let incoming_name = tool_call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty());
        let key = if let Some(index) = index {
            format!("index:{index}")
        } else if let Some(id) = incoming_id {
            format!("id:{id}")
        } else if let Some(incoming_name) = incoming_name {
            let unnamed_existing = self.tool_calls.len() == 1
                && self
                    .tool_calls
                    .get_index(0)
                    .is_some_and(|(_, call)| call.name.is_empty());
            if unnamed_existing {
                self.tool_calls
                    .get_index(0)
                    .map(|(key, _)| key.clone())
                    .unwrap_or_else(|| self.next_tool_key())
            } else if let Some((key, _)) = self.tool_calls.last().filter(|(_, call)| {
                call.name == incoming_name
                    && serde_json::from_str::<Value>(&call.arguments)
                        .map_or(true, |value| !value.is_object())
            }) {
                key.clone()
            } else {
                self.next_tool_key()
            }
        } else if !self.tool_calls.is_empty() {
            self.tool_calls
                .get_index(self.tool_calls.len() - 1)
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| self.next_tool_key())
        } else {
            self.next_tool_key()
        };

        let entry = self.tool_calls.entry(key).or_insert_with(|| {
            let id = incoming_id
                .map(str::to_owned)
                .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
            StreamingToolCall {
                id,
                ..StreamingToolCall::default()
            }
        });
        if let Some(id) = incoming_id {
            entry.id = id.to_string();
        }
        if let Some(name) = incoming_name {
            if entry.name.is_empty() {
                entry.name = name.to_string();
            } else if entry.name != name && !name.is_empty() {
                entry.name.push_str(name);
            }
        }
        if let Some(arguments) = tool_call.pointer("/function/arguments") {
            match arguments {
                Value::String(arguments) => {
                    append_streamed_tool_arguments(&mut entry.arguments, arguments)?
                }
                Value::Null => {}
                arguments if entry.arguments.is_empty() => {
                    append_streamed_tool_arguments(&mut entry.arguments, &arguments.to_string())?;
                }
                _ => {}
            }
        }
        if let Some(signature) = tool_call
            .pointer("/extra_content/google/thought_signature")
            .and_then(Value::as_str)
        {
            entry.thought_signature = Some(signature.to_string());
        }
        Ok(())
    }

    fn next_tool_key(&mut self) -> String {
        let key = format!("anonymous:{}", self.next_anonymous_tool);
        self.next_anonymous_tool += 1;
        key
    }

    fn finish(&mut self) -> Result<Vec<Event>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let mut events = Vec::new();

        self.flush_tagged_content(&mut events)?;
        self.stop_thinking_block(&mut events)?;
        self.stop_text_block(&mut events)?;

        let tool_calls_allowed = !self.refusal_seen
            && anthropic_stop_reason(self.finish_reason.as_deref(), !self.tool_calls.is_empty())
                == "tool_use";
        let valid_tool_calls = if tool_calls_allowed {
            self.tool_calls
                .values()
                .filter_map(|tool_call| {
                    match parse_tool_arguments_with_json(&tool_call.arguments) {
                        Ok((_, normalized)) => Some((tool_call, normalized)),
                        Err(message) => {
                            warn!(
                                tool_call_id = %tool_call.id,
                                error = %message,
                                "Skipping invalid streamed tool call"
                            );
                            None
                        }
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let stop_reason = if self.refusal_seen {
            "refusal"
        } else {
            anthropic_stop_reason(self.finish_reason.as_deref(), !valid_tool_calls.is_empty())
        };
        if stop_reason == "tool_use" {
            for (tool_call, arguments) in &valid_tool_calls {
                let index = self.next_content_index;
                self.next_content_index += 1;
                let name = if tool_call.name.is_empty() {
                    "unknown_function"
                } else {
                    &tool_call.name
                };
                if let Some(signature) = &tool_call.thought_signature {
                    remember_thought_signature(&self.thought_signatures, &tool_call.id, signature);
                }
                push_anthropic_event(
                    &mut events,
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {
                            "type": "tool_use",
                            "id": tool_call.id,
                            "name": name,
                            "input": {}
                        }
                    }),
                )?;
                push_anthropic_event(
                    &mut events,
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": arguments
                        }
                    }),
                )?;
                push_anthropic_event(
                    &mut events,
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                )?;
            }
        }

        let mut usage = json!({
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens
        });
        if let Some(value) = self.cache_read_input_tokens {
            usage["cache_read_input_tokens"] = json!(value);
        }
        if let Some(value) = self.cache_creation_input_tokens {
            usage["cache_creation_input_tokens"] = json!(value);
        }
        if let Some(value) = self.reasoning_tokens {
            usage["reasoning_tokens"] = json!(value);
        }
        push_anthropic_event(
            &mut events,
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": stop_reason,
                    "stop_sequence": Value::Null
                },
                "usage": usage
            }),
        )?;
        push_anthropic_event(&mut events, "message_stop", json!({"type": "message_stop"}))?;
        Ok(events)
    }
}

fn anthropic_upstream_event_stream<S, B, E>(
    byte_stream: S,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    estimated_input_tokens: u64,
    capabilities: OpenAiCapabilities,
) -> impl Stream<Item = Result<Event, Infallible>>
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let byte_stream = Box::pin(byte_stream);
    let translator = AnthropicStreamTranslator::with_capabilities(
        model,
        thought_signatures,
        estimated_input_tokens,
        capabilities,
    );
    let initial_events = match translator.start_events() {
        Ok(events) => VecDeque::from(events),
        Err(message) => VecDeque::from([anthropic_stream_error_event(&message)]),
    };
    let decoder = SseDataDecoder::default();

    stream::unfold(
        (byte_stream, decoder, translator, initial_events, false),
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
                            "OpenAI-compatible stream failed: {err}"
                        )));
                        ended = true;
                    }
                    Ok(None) => {
                        let mut saw_done = false;
                        let mut processing_failed = false;
                        match decoder.finish() {
                            Ok(payloads) => {
                                for payload in payloads {
                                    if payload.trim() == "[DONE]" {
                                        saw_done = true;
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
                            if stream_eof_is_complete(
                                saw_done,
                                translator.finish_reason.as_deref(),
                                translator.usage_only_tail_seen,
                            ) {
                                match translator.finish() {
                                    Ok(events) => pending.extend(events),
                                    Err(message) => {
                                        pending.push_back(anthropic_stream_error_event(&message))
                                    }
                                }
                            } else {
                                pending.push_back(anthropic_stream_error_event(
                                    "OpenAI-compatible stream ended before [DONE] or finish_reason",
                                ));
                            }
                        }
                        ended = true;
                    }
                    Err(_) => {
                        pending.push_back(anthropic_stream_error_event(
                            "OpenAI-compatible stream was idle for too long",
                        ));
                        ended = true;
                    }
                }
            }
        },
    )
}

fn anthropic_upstream_stream_response(
    upstream: reqwest::Response,
    model: String,
    thought_signatures: Arc<ThoughtSignatureCache>,
    estimated_input_tokens: u64,
    capabilities: OpenAiCapabilities,
) -> Response {
    let event_stream = anthropic_upstream_event_stream(
        upstream.bytes_stream(),
        model,
        thought_signatures,
        estimated_input_tokens,
        capabilities,
    );
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

fn anthropic_stream_error_event(message: &str) -> Event {
    Event::default().event("error").data(
        json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": message
            }
        })
        .to_string(),
    )
}
