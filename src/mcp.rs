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
