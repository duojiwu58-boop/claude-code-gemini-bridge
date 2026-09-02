fn mcp_json_response(id: Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn mcp_protocol_error(id: Value, code: i64, message: impl Into<String>) -> Response {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    }))
    .into_response()
}

fn valid_mcp_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

fn image_generation_tool() -> Value {
    json!({
        "name": "generate_image",
        "title": "Generate image with Gemini",
        "description": "Generate a new high-quality image with Gemini 3.1 Flash Image. Use this whenever the user asks to draw, create, or generate an image. The tool saves the image in the bridge's generated-images directory and returns both a preview and the absolute file path.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "A complete, detailed description of the image to generate. Preserve the user's requested language and visible text exactly."
                },
                "aspect_ratio": {
                    "type": "string",
                    "enum": ["1:1", "1:4", "1:8", "2:3", "3:2", "3:4", "4:1", "4:3", "4:5", "5:4", "8:1", "9:16", "16:9", "21:9"],
                    "default": "1:1"
                },
                "image_size": {
                    "type": "string",
                    "enum": ["1K", "2K", "4K"],
                    "default": "2K"
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        }
    })
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_start_tool() -> Value {
    json!({
        "name": "computer_start",
        "title": "Start Gemini Computer Use",
        "description": "Start either an isolated loopback browser or the desktop window explicitly selected by the user in Claude Bridge Manager, then return its initial screenshot.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "environment": {"type": "string", "enum": ["browser", "desktop"], "default": "browser"},
                "local_url": {"type": "string", "description": "Initial http:// or https:// loopback URL; browser only."}
            },
            "additionalProperties": false
        },
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
    })
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_action_batch_tool() -> Value {
    json!({
        "name": "computer_action_batch",
        "title": "Execute a Gemini Computer Use action batch",
        "description": "Execute an authenticated batch of native Gemini UI calls strictly in order. Do not construct this input manually; it is emitted by the bridge from Gemini Computer Use output.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "protocol_version": {"type": "string", "const": COMPUTER_PROTOCOL_VERSION},
                "session_id": {"type": "string"},
                "batch_id": {"type": "string"},
                "sequence": {"type": "integer", "minimum": 1},
                "environment": {"type": "string", "enum": ["browser", "desktop"]},
                "viewport": {"type": "object"},
                "calls": {"type": "array", "minItems": 1, "items": {"type": "object"}}
            },
            "required": ["protocol_version", "session_id", "batch_id", "sequence", "environment", "viewport", "calls"],
            "additionalProperties": false
        },
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
    })
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_cancel_tool() -> Value {
    json!({
        "name": "computer_cancel",
        "title": "Stop Gemini Computer Use",
        "description": "Immediately stop the active isolated Computer Use session.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_loopback_url(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|error| format!("Invalid local_url: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || !url.host().is_some_and(|host| match host {
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    }) {
        return Err("Computer Use version 1 only permits http(s) loopback URLs (localhost, 127.0.0.1, or ::1)".to_string());
    }
    Ok(url.to_string())
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_coordinate(arguments: &Map<String, Value>, name: &str) -> Result<u64, String> {
    arguments.get(name).and_then(Value::as_u64).filter(|value| *value <= 999)
        .ok_or_else(|| format!("Computer action field '{name}' must be an integer from 0 through 999"))
}

#[allow(dead_code)]
#[cfg(any())]
fn validate_computer_action(call: &Value, environment: &str) -> Result<(), String> {
    let call_id = call.get("call_id").and_then(Value::as_str).filter(|value| !value.is_empty())
        .ok_or_else(|| "Computer action call_id is required".to_string())?;
    let name = call.get("name").and_then(Value::as_str).ok_or_else(|| format!("Computer action '{call_id}' has no name"))?;
    let arguments = call.get("arguments").and_then(Value::as_object)
        .ok_or_else(|| format!("Computer action '{call_id}' arguments must be an object"))?;
    if arguments.get("intent").and_then(Value::as_str).is_none_or(|value| value.trim().is_empty()) {
        return Err(format!("Computer action '{call_id}' is missing its Gemini intent"));
    }
    let coordinate_pair = |x: &str, y: &str| -> Result<(), String> {
        computer_coordinate(arguments, x)?;
        computer_coordinate(arguments, y)?;
        Ok(())
    };
    match name {
        "click" | "double_click" | "triple_click" | "middle_click" | "right_click"
        | "mouse_down" | "mouse_up" | "move" => coordinate_pair("x", "y")?,
        "type" => {
            let text = arguments.get("text").and_then(Value::as_str).ok_or_else(|| "Computer type action requires text".to_string())?;
            if text.encode_utf16().count() > 4_000 {
                return Err("Computer type text exceeds the 4000 UTF-16 unit safety limit".to_string());
            }
            if arguments.get("press_enter").is_some_and(|value| !value.is_boolean()) {
                return Err("Computer type press_enter must be boolean".to_string());
            }
        }
        "drag_and_drop" => {
            coordinate_pair("start_x", "start_y")?;
            coordinate_pair("end_x", "end_y")?;
        }
        "wait" => {
            if arguments.get("seconds").is_some_and(|value| value.as_u64().is_none_or(|seconds| seconds > 30)) {
                return Err("Computer wait seconds must be an integer from 0 through 30".to_string());
            }
        }
        "press_key" | "key_down" | "key_up" => {
            arguments.get("key").and_then(Value::as_str).filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Computer {name} requires key"))?;
        }
        "hotkey" => {
            arguments.get("keys").and_then(Value::as_array).filter(|keys| {
                !keys.is_empty() && keys.iter().all(|key| key.as_str().is_some_and(|value| !value.is_empty()))
            }).ok_or_else(|| "Computer hotkey requires a non-empty string array 'keys'".to_string())?;
        }
        "take_screenshot" | "go_back" | "go_forward" => {}
        "scroll" => {
            coordinate_pair("x", "y")?;
            if !matches!(arguments.get("direction").and_then(Value::as_str), Some("up" | "down" | "left" | "right")) {
                return Err("Computer scroll direction must be up, down, left, or right".to_string());
            }
            if arguments.get("magnitude_in_pixels").is_some_and(|value| value.as_u64().is_none_or(|pixels| pixels > 999)) {
                return Err("Computer scroll magnitude_in_pixels must be an integer from 0 through 999".to_string());
            }
        }
        "navigate" => {
            let target = arguments.get("url").and_then(Value::as_str).ok_or_else(|| "Computer navigate requires url".to_string())?;
            computer_loopback_url(target)?;
        }
        _ => return Err(format!("Unsupported Gemini Computer Use action '{name}'")),
    }
    if environment == "desktop" && matches!(name, "go_back" | "navigate" | "go_forward") {
        return Err(format!("Gemini desktop environment does not support '{name}'"));
    }
    Ok(())
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_batch_requires_confirmation(calls: &[Value], current_url: Option<&str>) -> Result<Option<String>, String> {
    let on_loopback = current_url.and_then(|value| url::Url::parse(value).ok()).is_some_and(|url| {
        url.host().is_some_and(|host| match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        })
    });
    let mut reasons = Vec::new();
    for call in calls {
        let name = call.get("name").and_then(Value::as_str).unwrap_or("unknown");
        let decision = call.pointer("/safety_decision/decision")
            .or_else(|| call.pointer("/arguments/safety_decision/decision"))
            .and_then(Value::as_str);
        if decision == Some("blocked") {
            return Err(format!("Gemini safety policy blocked Computer Use action '{name}'"));
        }
        if decision == Some("require_confirmation") {
            let explanation = call.pointer("/safety_decision/explanation")
                .or_else(|| call.pointer("/arguments/safety_decision/explanation"))
                .and_then(Value::as_str).unwrap_or("Gemini requires confirmation");
            reasons.push(format!("{name}: {explanation}"));
            continue;
        }
        let locally_low_risk = matches!(name, "take_screenshot" | "wait" | "move")
            || (on_loopback && matches!(name, "click" | "double_click" | "triple_click" | "scroll" | "go_back" | "go_forward"));
        if !locally_low_risk {
            reasons.push(format!("{name}: local policy requires a real user confirmation"));
        }
    }
    Ok((!reasons.is_empty()).then(|| reasons.join("\n")))
}

#[allow(dead_code)]
#[cfg(any())]
fn computer_result_content(mut result: Value) -> Result<Value, String> {
    let is_error = result.get("status").and_then(Value::as_str) != Some("success");
    let result_object = result.as_object_mut().ok_or_else(|| "Computer Host returned a non-object result".to_string())?;
    let mut screenshots = Vec::new();
    if let Some(results) = result_object.get_mut("results").and_then(Value::as_array_mut) {
        for item in results {
            if let Some(screenshot) = item.get_mut("screenshot").and_then(Value::as_object_mut) {
                if let Some(data) = screenshot.remove("data").and_then(|value| value.as_str().map(str::to_string)) {
                    screenshot.insert("content_index".to_string(), json!(screenshots.len() + 1));
                    screenshots.push(data);
                }
            }
        }
    }
    if let Some(screenshot) = result_object.get_mut("screenshot").and_then(Value::as_object_mut) {
        if let Some(data) = screenshot.remove("data").and_then(|value| value.as_str().map(str::to_string)) {
            screenshot.insert("content_index".to_string(), json!(screenshots.len() + 1));
            screenshots.push(data);
        }
    }
    let text = serde_json::to_string(&result).map_err(|error| format!("Cannot serialize Computer Host result: {error}"))?;
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend(screenshots.into_iter().map(|data| json!({"type": "image", "data": data, "mimeType": "image/png"})));
    Ok(json!({"content": content, "structuredContent": result, "isError": is_error}))
}

#[allow(dead_code)]
#[cfg(any())]
async fn mcp_computer_start(state: &AppState, arguments: Option<&Value>) -> Result<Value, String> {
    let arguments = arguments.and_then(Value::as_object).ok_or_else(|| "computer_start arguments must be an object".to_string())?;
    let environment = arguments.get("environment").and_then(Value::as_str).unwrap_or("browser");
    if !matches!(environment, "browser" | "desktop") {
        return Err("computer_start environment must be browser or desktop".to_string());
    }
    let (configured_url, selected_window) = {
        let inner = state.computer.inner.lock().await;
        (inner.default_local_url.clone(), inner.selected_window.clone())
    };
    let local_url = if environment == "browser" {
        Some(computer_loopback_url(arguments.get("local_url").and_then(Value::as_str)
            .or(configured_url.as_deref())
            .ok_or_else(|| "computer_start requires local_url or a URL configured in the model center".to_string())?
        )?)
    } else { None };
    let target_window = if environment == "desktop" {
        Some(selected_window.ok_or_else(|| "Select a desktop window in Claude Bridge Manager before computer_start".to_string())?)
    } else { None };
    let target_hwnd = target_window.as_ref().and_then(|window| window.get("hwnd")).and_then(Value::as_str).map(str::to_string);
    let session_id = format!("cus_{}", Uuid::new_v4().simple());
    {
        let mut inner = state.computer.inner.lock().await;
        if inner.session.is_some() {
            return Err("Only one Computer Use session may be active; cancel it before starting another".to_string());
        }
        inner.session = Some(ComputerSessionState {
            session_id: session_id.clone(), environment: environment.to_string(), sequence: 0,
            current_url: local_url.clone(),
            window_title: target_window.as_ref().and_then(|window| window.get("title")).and_then(Value::as_str).map(str::to_string),
            target_pid: target_window.as_ref().and_then(|window| window.get("pid")).and_then(Value::as_u64),
            target_hwnd: target_hwnd.clone(), last_intent: None,
            started_at: SystemTime::now(), last_activity: SystemTime::now(),
        });
    }
    let command_id = format!("start:{session_id}");
    let mut command = json!({
        "protocol_version": COMPUTER_PROTOCOL_VERSION, "command_id": command_id,
        "type": "start", "session_id": session_id, "environment": environment,
        "viewport": {"width": 1440, "height": 900, "device_scale_factor": 1}
    });
    if let Some(local_url) = local_url { command["local_url"] = json!(local_url); }
    if let Some(target_hwnd) = target_hwnd { command["target_hwnd"] = json!(target_hwnd); }
    match submit_computer_command(state, command_id, command).await {
        Ok(mut result) => {
            if result.get("status").and_then(Value::as_str) != Some("success") {
                let message = result.get("error").and_then(Value::as_str).unwrap_or("Computer Host failed to start Computer Use").to_string();
                let mut inner = state.computer.inner.lock().await;
                inner.session = None;
                return Err(message);
            }
            result["kind"] = json!("computer_start_result");
            result["protocol_version"] = json!(COMPUTER_PROTOCOL_VERSION);
            result["session_id"] = json!(session_id);
            result["sequence"] = json!(0);
            computer_result_content(result)
        }
        Err(message) => {
            let mut inner = state.computer.inner.lock().await;
            inner.session = None;
            Err(message)
        }
    }
}

#[allow(dead_code)]
#[cfg(any())]
async fn mcp_computer_action_batch(state: &AppState, arguments: Option<&Value>) -> Result<Value, String> {
    let arguments = arguments.and_then(Value::as_object).ok_or_else(|| "computer_action_batch arguments must be an object".to_string())?;
    if arguments.get("protocol_version").and_then(Value::as_str) != Some(COMPUTER_PROTOCOL_VERSION) {
        return Err("Unsupported Computer Use protocol_version".to_string());
    }
    let session_id = arguments.get("session_id").and_then(Value::as_str).filter(|value| !value.is_empty()).ok_or_else(|| "computer_action_batch requires session_id".to_string())?;
    let batch_id = arguments.get("batch_id").and_then(Value::as_str).filter(|value| !value.is_empty()).ok_or_else(|| "computer_action_batch requires batch_id".to_string())?;
    let sequence = arguments.get("sequence").and_then(Value::as_u64).ok_or_else(|| "computer_action_batch requires sequence".to_string())?;
    let environment = arguments.get("environment").and_then(Value::as_str).ok_or_else(|| "computer_action_batch requires environment".to_string())?;
    let calls = arguments.get("calls").and_then(Value::as_array).filter(|calls| !calls.is_empty()).ok_or_else(|| "computer_action_batch requires calls".to_string())?;
    for call in calls { validate_computer_action(call, environment)?; }
    let command_id = format!("batch:{session_id}:{batch_id}");
    let (current_sequence, current_url, cached) = {
        let inner = state.computer.inner.lock().await;
        let session = inner.session.as_ref().filter(|session| session.session_id == session_id)
            .ok_or_else(|| "Computer Use session is not active or session_id does not match".to_string())?;
        if session.environment != environment { return Err("Computer Use environment does not match the active session".to_string()); }
        (session.sequence, session.current_url.clone(), inner.completed.get(&command_id).cloned())
    };
    if let Some(mut result) = cached {
        result["kind"] = json!("computer_action_batch_result");
        result["protocol_version"] = json!(COMPUTER_PROTOCOL_VERSION);
        result["session_id"] = json!(session_id);
        result["batch_id"] = json!(batch_id);
        result["sequence"] = json!(sequence);
        return computer_result_content(result);
    }
    if sequence != current_sequence + 1 {
        return Err(format!("Computer Use sequence must be {}, received {sequence}", current_sequence + 1));
    }
    if sequence > 50 {
        return Err("Computer Use reached the 50-step safety limit".to_string());
    }
    {
        let mut inner = state.computer.inner.lock().await;
        if let Some(session) = inner.session.as_mut().filter(|session| session.session_id == session_id) {
            if session.started_at.elapsed().unwrap_or_default() > Duration::from_secs(900) {
                return Err("Computer Use session exceeded the 15-minute safety timeout".to_string());
            }
            session.last_intent = calls.last()
                .and_then(|call| call.get("intent").or_else(|| call.pointer("/arguments/intent")))
                .and_then(Value::as_str).map(str::to_string);
        }
    }
    let confirmation_reason = computer_batch_requires_confirmation(calls, current_url.as_deref())?;
    let mut approval_token = None;
    if let Some(reason) = confirmation_reason {
        approval_token = Some(await_computer_approval(state, session_id, batch_id, arguments.get("calls").unwrap(), reason).await?);
    }
    let mut command = Value::Object(arguments.clone());
    command["command_id"] = json!(command_id);
    command["type"] = json!("action_batch");
    command["approved_by_user"] = json!(approval_token.is_some());
    if let Some(token) = approval_token { command["approval_token"] = json!(token); }
    let mut result = submit_computer_command(state, command_id, command).await?;
    result["kind"] = json!("computer_action_batch_result");
    result["protocol_version"] = json!(COMPUTER_PROTOCOL_VERSION);
    result["session_id"] = json!(session_id);
    result["batch_id"] = json!(batch_id);
    result["sequence"] = json!(sequence);
    if let Some(results) = result.get_mut("results").and_then(Value::as_array_mut) {
        let approved = arguments.get("calls").and_then(Value::as_array).zip(Some(results)).map(|(calls, results)| {
            for (call, item) in calls.iter().zip(results.iter_mut()) {
                let required = call.pointer("/safety_decision/decision").or_else(|| call.pointer("/arguments/safety_decision/decision")).and_then(Value::as_str) == Some("require_confirmation");
                if required && item.get("status").and_then(Value::as_str) == Some("success") {
                    item["safety_acknowledgement"] = json!(true);
                }
            }
        });
        let _ = approved;
    }
    {
        let mut inner = state.computer.inner.lock().await;
        if let Some(session) = inner.session.as_mut().filter(|session| session.session_id == session_id) {
            session.sequence = sequence;
            session.last_activity = SystemTime::now();
        }
    }
    computer_result_content(result)
}

fn mcp_tool_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text.into()}],
        "isError": is_error
    })
}

#[derive(Clone)]
struct KimiFormulaTool {
    formula: String,
    name: String,
    mcp_definition: Value,
}

fn kimi_formula_url(
    profile: &ProviderProfile,
    formula: &str,
    operation: &str,
) -> Result<String, String> {
    if !is_supported_kimi_formula(formula) {
        return Err(format!("Unsupported Kimi formula '{formula}'"));
    }
    let mut url = url::Url::parse(&profile.base_url)
        .map_err(|error| format!("Invalid Kimi base_url '{}': {error}", profile.base_url))?;
    url.set_path(&format!("/v1/formulas/{formula}/{operation}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn kimi_profile_credential(profile: &ProviderProfile) -> Result<&str, String> {
    profile
        .auth_token
        .as_deref()
        .or(profile.api_key.as_deref())
        .ok_or_else(|| format!("Kimi profile '{}' has no credential", profile.file_name))
}

async fn configured_kimi_formula_tools(state: &AppState) -> Result<Vec<KimiFormulaTool>, String> {
    let profile = active_provider_profile(state)
        .filter(is_kimi_profile)
        .ok_or_else(|| "The active provider is not Kimi".to_string())?;
    if profile.openai_capabilities.kimi_formula_tools.is_empty() {
        return Ok(Vec::new());
    }
    let credential = kimi_profile_credential(&profile)?;
    let mut configured = Vec::new();
    let mut names = HashSet::new();
    for formula in &profile.openai_capabilities.kimi_formula_tools {
        let url = kimi_formula_url(&profile, formula, "tools")?;
        let operation = async {
            let response = profile
                .client
                .get(url)
                .bearer_auth(credential)
                .send()
                .await
                .map_err(|error| format!("Cannot load Kimi formula '{formula}': {error}"))?;
            let status = response.status();
            let body = read_response_json_limited(response)
                .await
                .map_err(|error| format!("Cannot read Kimi formula '{formula}': {error}"))?;
            Ok::<_, String>((status, body))
        };
        let (status, body) = tokio::time::timeout(KIMI_FORMULA_TIMEOUT, operation)
            .await
            .map_err(|_| {
                format!(
                    "Loading Kimi formula '{formula}' timed out after {} seconds",
                    KIMI_FORMULA_TIMEOUT.as_secs()
                )
            })??;
        if !status.is_success() {
            return Err(format!(
                "Kimi formula '{formula}' returned HTTP {status}: {}",
                safe_error_message(&body)
            ));
        }
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("Kimi formula '{formula}' returned no tools array"))?;
        for tool in tools {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                continue;
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("Kimi formula '{formula}' returned an invalid function"))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("Kimi formula '{formula}' returned an unnamed function"))?;
            if name == "generate_image" || !names.insert(name.to_string()) {
                return Err(format!(
                    "Kimi formula tool name '{name}' conflicts with another MCP tool"
                ));
            }
            configured.push(KimiFormulaTool {
                formula: formula.clone(),
                name: name.to_string(),
                mcp_definition: json!({
                    "name": name,
                    "title": format!("Kimi Formula: {name}"),
                    "description": function.get("description").cloned().unwrap_or_else(|| json!(format!("Kimi official formula tool {name}"))),
                    "inputSchema": function.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    "annotations": {
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": true
                    }
                }),
            });
        }
    }
    Ok(configured)
}

async fn execute_kimi_formula(
    state: &AppState,
    name: &str,
    arguments: Option<&Value>,
) -> Result<Value, String> {
    let profile = active_provider_profile(state)
        .filter(is_kimi_profile)
        .ok_or_else(|| "The active provider is not Kimi".to_string())?;
    let tools = configured_kimi_formula_tools(state).await?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| format!("Unknown or disabled Kimi formula tool '{name}'"))?;
    let credential = kimi_profile_credential(&profile)?;
    let url = kimi_formula_url(&profile, &tool.formula, "fibers")?;
    let arguments = serde_json::to_string(arguments.unwrap_or(&Value::Object(Map::new())))
        .map_err(|error| format!("Cannot serialize Kimi formula arguments: {error}"))?;
    let operation = async {
        let response = profile
            .client
            .post(url)
            .bearer_auth(credential)
            .json(&json!({"name": name, "arguments": arguments}))
            .send()
            .await
            .map_err(|error| format!("Kimi formula '{name}' failed: {error}"))?;
        let status = response.status();
        let body = read_response_json_limited(response)
            .await
            .map_err(|error| format!("Cannot read Kimi formula '{name}' response: {error}"))?;
        Ok::<_, String>((status, body))
    };
    let (status, body) = tokio::time::timeout(KIMI_FORMULA_TIMEOUT, operation)
        .await
        .map_err(|_| {
            format!(
                "Kimi formula '{name}' timed out after {} seconds",
                KIMI_FORMULA_TIMEOUT.as_secs()
            )
        })??;
    if !status.is_success() {
        return Err(format!(
            "Kimi formula '{name}' returned HTTP {status}: {}",
            safe_error_message(&body)
        ));
    }
    Ok(body)
}

fn kimi_formula_result_text(body: &Value) -> String {
    body.pointer("/context/output")
        .or_else(|| body.pointer("/context/encrypted_output"))
        .or_else(|| body.get("output"))
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_else(|| body.to_string())
}

async fn mcp(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    if !valid_mcp_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return mcp_protocol_error(Value::Null, -32600, "Invalid JSON-RPC request");
    }

    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    if id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = id.unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested_version = request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let protocol_version = match requested_version {
                "2025-11-25" | "2025-06-18" | "2025-03-26" => requested_version,
                _ => MCP_PROTOCOL_VERSION,
            };
            mcp_json_response(
                id,
                json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {
                        "name": "claude-code-gemini-image",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Use generate_image when the user asks to create or draw an image. It returns a rendered image and a saved local file path."
                }),
            )
        }
        "ping" => mcp_json_response(id, json!({})),
        "tools/list" => {
            let mut tools = vec![image_generation_tool()];
            match configured_kimi_formula_tools(&state).await {
                Ok(formula_tools) => {
                    tools.extend(formula_tools.into_iter().map(|tool| tool.mcp_definition))
                }
                Err(message) if message == "The active provider is not Kimi" => {}
                Err(message) => warn!(
                    error = message,
                    "Cannot expose configured Kimi formula tools"
                ),
            }
            mcp_json_response(id, json!({"tools": tools}))
        }
        "tools/call" => {
            let name = request.pointer("/params/name").and_then(Value::as_str);
            if name == Some("generate_image") {
                return match generate_image(&state, request.pointer("/params/arguments")).await {
                    Ok(image) => mcp_json_response(
                        id,
                        json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!(
                                        "Image generated with {} and saved to: {}",
                                        state.image_model,
                                        image.path.display()
                                    )
                                },
                                {
                                    "type": "image",
                                    "data": image.base64_data,
                                    "mimeType": image.mime_type
                                }
                            ],
                            "structuredContent": {
                                "path": image.path,
                                "mime_type": image.mime_type,
                                "model": state.image_model
                            },
                            "isError": false
                        }),
                    ),
                    Err(message) => mcp_json_response(id, mcp_tool_result(message, true)),
                };
            }
            let Some(name) = name else {
                return mcp_protocol_error(id, -32602, "Tool name is required");
            };
            match execute_kimi_formula(&state, name, request.pointer("/params/arguments")).await {
                Ok(body) => mcp_json_response(
                    id,
                    json!({
                        "content": [{"type": "text", "text": kimi_formula_result_text(&body)}],
                        "structuredContent": body,
                        "isError": false
                    }),
                ),
                Err(message) => mcp_json_response(id, mcp_tool_result(message, true)),
            }
        }
        _ => mcp_protocol_error(id, -32601, "Method not found"),
    }
}

#[derive(Clone)]
struct ImageProvider {
    client: Client,
    api_key: String,
}

struct GeneratedImage {
    path: PathBuf,
    mime_type: String,
    base64_data: String,
}

fn official_google_profile(profile: &ProviderProfile) -> bool {
    url::Url::parse(&profile.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .as_deref()
        == Some("generativelanguage.googleapis.com")
}

fn image_provider(state: &AppState) -> Result<ImageProvider, String> {
    if let Some(api_key) = state
        .fallback_api_key
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        let transport = current_gemini_transport(state)?;
        return Ok(ImageProvider {
            client: transport.client,
            api_key: api_key.clone(),
        });
    }

    let routing = state
        .routing
        .read()
        .map_err(|_| "Cannot read provider routing state for image generation".to_string())?;
    routing
        .profiles
        .iter()
        .find_map(|profile| {
            if !official_google_profile(profile) {
                return None;
            }
            let api_key = profile.auth_token.as_ref().or(profile.api_key.as_ref())?;
            Some(ImageProvider {
                client: profile.client.clone(),
                api_key: api_key.clone(),
            })
        })
        .ok_or_else(|| "Gemini API key is not configured for image generation".to_string())
}

fn validate_image_option<'a>(
    arguments: &'a Map<String, Value>,
    name: &str,
    default: &'static str,
    allowed: &[&str],
) -> Result<&'a str, String> {
    let value = arguments
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or(default);
    allowed
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| format!("Unsupported {name} '{value}'"))
}

fn generated_image_extension(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn extract_generated_image(body: &Value) -> Result<(String, String), String> {
    fn image_data(value: &Value) -> Option<&Value> {
        value
            .get("inlineData")
            .or_else(|| value.get("inline_data"))
            .or_else(|| {
                (value.get("type").and_then(Value::as_str) == Some("image")).then_some(value)
            })
    }

    let mut candidates = Vec::new();
    if let Some(output_image) = body.get("output_image") {
        candidates.push(output_image);
    }
    if let Some(parts) = body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        candidates.extend(parts.iter());
    }
    if let Some(steps) = body.get("steps").and_then(Value::as_array) {
        for step in steps {
            if let Some(content) = step.get("content").and_then(Value::as_array) {
                candidates.extend(content.iter());
            }
        }
    }

    for candidate in candidates {
        let Some(inline) = image_data(candidate) else {
            continue;
        };
        let mime_type = inline
            .get("mimeType")
            .or_else(|| inline.get("mime_type"))
            .and_then(Value::as_str)
            .ok_or_else(|| "Gemini image response has no MIME type".to_string())?;
        if generated_image_extension(mime_type).is_none() {
            return Err(format!(
                "Gemini returned unsupported image type '{mime_type}'"
            ));
        }
        let data = inline
            .get("data")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Gemini image response has no image data".to_string())?;
        return Ok((mime_type.to_string(), data.to_string()));
    }
    Err(format!(
        "Gemini returned no generated image: {}",
        safe_error_message(body)
    ))
}

fn write_generated_image(
    output_dir: &Path,
    mime_type: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    fs::create_dir_all(output_dir).map_err(|err| {
        format!(
            "Cannot create generated image directory '{}': {err}",
            output_dir.display()
        )
    })?;
    let extension = generated_image_extension(mime_type)
        .ok_or_else(|| format!("Unsupported generated image type '{mime_type}'"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let unique = Uuid::new_v4().simple().to_string();
    let file_name = format!("gemini-image-{timestamp}-{}.{}", &unique[..8], extension);
    let path = output_dir.join(file_name);
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|err| format!("Cannot create generated image '{}': {err}", path.display()))?;
    if let Err(err) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!(
            "Cannot write generated image '{}': {err}",
            path.display()
        ));
    }
    Ok(path)
}

async fn generate_image(
    state: &AppState,
    arguments: Option<&Value>,
) -> Result<GeneratedImage, String> {
    let arguments = arguments
        .and_then(Value::as_object)
        .ok_or_else(|| "Image tool arguments must be a JSON object".to_string())?;
    let prompt = arguments
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Image prompt must be a non-empty string".to_string())?;
    if prompt.chars().count() > MAX_IMAGE_PROMPT_CHARS {
        return Err(format!(
            "Image prompt exceeds {MAX_IMAGE_PROMPT_CHARS} characters"
        ));
    }
    let aspect_ratio = validate_image_option(
        arguments,
        "aspect_ratio",
        "1:1",
        &[
            "1:1", "1:4", "1:8", "2:3", "3:2", "3:4", "4:1", "4:3", "4:5", "5:4", "8:1", "9:16",
            "16:9", "21:9",
        ],
    )?;
    let image_size = validate_image_option(arguments, "image_size", "2K", &["1K", "2K", "4K"])?;
    let provider = image_provider(state)?;
    let request_body = json!({
        "model": state.image_model,
        "input": prompt,
        "response_format": {
            "type": "image",
            "mime_type": "image/jpeg",
            "aspect_ratio": aspect_ratio,
            "image_size": image_size
        },
        "generation_config": {"thinking_level": "high"}
    });
    let operation = async {
        let response = provider
            .client
            .post(&state.image_upstream_url)
            .header("x-goog-api-key", &provider.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|err| format!("Gemini image request failed: {err}"))?;
        let status = response.status();
        let bytes = read_response_bytes_limited(response, MAX_IMAGE_RESPONSE_BYTES).await?;
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|err| format!("Gemini returned invalid image JSON: {err}"))?;
        if !status.is_success() {
            return Err(format!(
                "Gemini image request returned HTTP {}: {}",
                status.as_u16(),
                safe_error_message(&body)
            ));
        }
        Ok(body)
    };
    let body = tokio::time::timeout(Duration::from_secs(180), operation)
        .await
        .map_err(|_| "Gemini image generation timed out after 180 seconds".to_string())??;
    let (mime_type, base64_data) = extract_generated_image(&body)?;
    let bytes = BASE64_STANDARD
        .decode(&base64_data)
        .map_err(|err| format!("Gemini returned invalid base64 image data: {err}"))?;
    if bytes.len() > MAX_GENERATED_IMAGE_BYTES {
        return Err(format!(
            "Generated image exceeds {MAX_GENERATED_IMAGE_BYTES} bytes"
        ));
    }
    let output_dir = state.image_output_dir.clone();
    let write_mime_type = mime_type.clone();
    let path = tokio::task::spawn_blocking(move || {
        write_generated_image(&output_dir, &write_mime_type, &bytes)
    })
    .await
    .map_err(|err| format!("Cannot join generated image writer: {err}"))??;
    Ok(GeneratedImage {
        path,
        mime_type,
        base64_data,
    })
}
