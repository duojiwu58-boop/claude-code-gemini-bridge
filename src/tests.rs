use super::*;

async fn collect_translated_sse_from_mock(mock: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let upstream = Client::builder()
        .build()
        .unwrap()
        .get(format!("http://{address}/stream"))
        .send()
        .await
        .unwrap();
    let response = anthropic_upstream_stream_response(
        upstream,
        "integration-model".to_string(),
        Arc::new(RwLock::new(IndexMap::new())),
        7,
        OpenAiCapabilities::default(),
    );
    let body = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_RESPONSE_BYTES)
        .await
        .unwrap();
    server.abort();
    String::from_utf8(body.to_vec()).unwrap()
}

async fn collect_translated_sse(upstream_body: String) -> String {
    let mock = Router::new().route(
        "/stream",
        get(move || {
            let upstream_body = upstream_body.clone();
            async move {
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(upstream_body))
                    .unwrap()
            }
        }),
    );
    collect_translated_sse_from_mock(mock).await
}

fn anthropic_sse_event_values(body: &str) -> Vec<Value> {
    let mut decoder = SseDataDecoder::default();
    let mut payloads = decoder.push_bytes(body.as_bytes()).unwrap();
    payloads.extend(decoder.finish().unwrap());
    payloads
        .into_iter()
        .map(|payload| serde_json::from_str(&payload).unwrap())
        .collect()
}

fn test_provider_profile(client: Client, upstream_url: String) -> ProviderProfile {
    ProviderProfile {
        file_name: "test-provider.json".to_string(),
        display_name: "Test Provider".to_string(),
        source: ProviderProfileSource::Native,
        model: "test-model".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: upstream_url.clone(),
        auth_token: Some("secret".to_string()),
        api_key: None,
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::OpenAiChat,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url,
        client,
    }
}

#[test]
fn accepts_an_empty_provider_profile_directory() {
    let settings_dir =
        env::temp_dir().join(format!("claude-bridge-empty-profiles-{}", Uuid::new_v4()));
    fs::create_dir_all(&settings_dir).unwrap();
    let result = load_provider_profiles(&settings_dir, &settings_dir, "http://127.0.0.1:18787");
    fs::remove_dir(&settings_dir).unwrap();

    let loaded = result.unwrap();
    assert!(loaded.profiles.is_empty());
    assert_eq!(loaded.source, ProviderProfileSource::Legacy);
}

#[test]
fn native_provider_profiles_use_openai_sdk_base_urls_and_keep_legacy_during_migration() {
    let root = env::temp_dir().join(format!("claude-bridge-native-profiles-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("deepseek.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "DeepSeek",
            "model": "deepseek-chat",
            "base_url": "https://api.deepseek.com",
            "api_key": "secret"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        settings_dir.join("settings - legacy.json"),
        serde_json::to_vec_pretty(&json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://legacy.example",
                "ANTHROPIC_MODEL": "legacy-model",
                "ANTHROPIC_API_KEY": "legacy-secret"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(settings_dir.join("settings.local.json"), b"{invalid").unwrap();
    fs::write(settings_dir.join("settings - draft.json"), b"{invalid").unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(loaded.source, ProviderProfileSource::Mixed);
    assert_eq!(loaded.profiles.len(), 2);
    let profile = &loaded.profiles[0];
    assert_eq!(profile.display_name, "DeepSeek");
    assert_eq!(profile.transport, ProviderTransport::OpenAiChat);
    assert_eq!(
        profile.upstream_url,
        "https://api.deepseek.com/chat/completions"
    );
    assert_eq!(profile.auth_token.as_deref(), Some("secret"));
    assert!(profile.api_key.is_none());
}

#[test]
fn native_kimi_anthropic_profile_uses_bearer_auth_and_exposes_one_million_context() {
    let root = env::temp_dir().join(format!("claude-bridge-kimi-profile-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("kimi-k3.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "Kimi K3",
            "model": "kimi-k3",
            "protocol": "anthropic",
            "base_url": "https://api.moonshot.ai/anthropic",
            "api_key": "secret",
            "auth_scheme": "bearer",
            "context_window": 1_048_576,
            "capabilities": {
                "kimi_formula_tools": ["moonshot/web-search:latest"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();

    let profile = &loaded.profiles[0];
    assert_eq!(profile.transport, ProviderTransport::Anthropic);
    assert_eq!(
        profile.upstream_url,
        "https://api.moonshot.ai/anthropic/v1/messages"
    );
    assert_eq!(profile.auth_token.as_deref(), Some("secret"));
    assert!(profile.api_key.is_none());
    assert_eq!(profile.context_window, Some(1_048_576));
    assert!(is_kimi_profile(profile));
    assert_eq!(
        profile.openai_capabilities.kimi_formula_tools,
        vec!["moonshot/web-search:latest"]
    );
}

#[test]
fn native_openai_base_url_matches_official_sdk_semantics() {
    let cases = [
        (
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
        (
            "https://api.moonshot.cn/v1/",
            "https://api.moonshot.cn/v1/chat/completions",
        ),
        (
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        ),
    ];

    for (base_url, expected) in cases {
        assert_eq!(openai_compatible_chat_endpoint(base_url), expected);
    }
    assert_eq!(
        openai_responses_endpoint("https://api.deepseek.com/v1"),
        "https://api.deepseek.com/v1/responses"
    );
    assert_eq!(
        openai_responses_endpoint("https://workspace.cn-beijing.maas.aliyuncs.com/v1/responses"),
        "https://workspace.cn-beijing.maas.aliyuncs.com/v1/responses"
    );
}

#[test]
fn native_provider_endpoint_override_and_disabled_profiles_work() {
    let root = env::temp_dir().join(format!("claude-bridge-native-options-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("custom.json"),
        serde_json::to_vec_pretty(&json!({
            "model": "custom-model",
            "baseURL": "https://gateway.example/api",
            "endpoint": "https://gateway.example/special/chat",
            "apiKey": "secret"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        providers_dir.join("disabled.json"),
        serde_json::to_vec_pretty(&json!({
            "model": "disabled-model",
            "base_url": "https://disabled.example/v1",
            "api_key": "secret",
            "enabled": false
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(loaded.source, ProviderProfileSource::Native);
    assert_eq!(loaded.profiles.len(), 1);
    assert_eq!(
        loaded.profiles[0].upstream_url,
        "https://gateway.example/special/chat"
    );
}

#[test]
fn vision_proxy_defaults_to_native_local_gemini_profile() {
    let root = env::temp_dir().join(format!("claude-bridge-vision-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("deepseek.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "DeepSeek",
            "model": "deepseek-v4-flash",
            "base_url": "https://api.deepseek.com",
            "api_key": "secret",
            "vision": {"mode": "proxy"}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        providers_dir.join("gemini.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "Gemini Vision",
            "model": "gemini-3.6-flash",
            "base_url": "http://127.0.0.1:18787",
            "protocol": "gemini"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        providers_dir.join("gemini-openai.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "Gemini OpenAI Vision",
            "model": "gemini-3.6-flash",
            "base_url": "https://generativelanguage.googleapis.com/v1beta/openai",
            "api_key": "secret"
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();
    let target = loaded
        .profiles
        .iter()
        .find(|profile| profile.file_name == "deepseek.json")
        .unwrap();
    let source = resolve_vision_provider(&loaded.profiles, target)
        .unwrap()
        .unwrap();

    assert_eq!(target.vision.mode, VisionMode::Proxy);
    assert_eq!(source.file_name, "gemini.json");
    assert_eq!(source.transport, ProviderTransport::LocalGemini);
    assert_eq!(
        provider_profile_json(target, &target.file_name)["vision"]["mode"],
        "proxy"
    );

    let profiles_without_local = loaded
        .profiles
        .iter()
        .filter(|profile| !profile.local_gemini)
        .cloned()
        .collect::<Vec<_>>();
    let target = profiles_without_local
        .iter()
        .find(|profile| profile.file_name == "deepseek.json")
        .unwrap();
    let source = resolve_vision_provider(&profiles_without_local, target)
        .unwrap()
        .unwrap();
    assert_eq!(source.file_name, "gemini-openai.json");
    assert_eq!(source.transport, ProviderTransport::OpenAiChat);
}

#[test]
fn vision_profile_validation_rejects_missing_sources_and_proxy_chains() {
    let missing = json!({"vision": {"mode": "proxy", "profile": "missing.json"}});
    let parsed = parse_vision_config(missing.as_object().unwrap(), "target.json").unwrap();
    assert_eq!(parsed.profile.as_deref(), Some("missing.json"));

    let invalid_native = json!({"vision": {"mode": "native", "profile": "gemini.json"}});
    assert!(
        parse_vision_config(invalid_native.as_object().unwrap(), "target.json")
            .unwrap_err()
            .contains("vision.profile")
    );
    let invalid_mode = json!({"vision": {"mode": "magic"}});
    assert!(
        parse_vision_config(invalid_mode.as_object().unwrap(), "target.json")
            .unwrap_err()
            .contains("unsupported")
    );
    let invalid_mode_type = json!({"vision": {"mode": true}});
    assert!(
        parse_vision_config(invalid_mode_type.as_object().unwrap(), "target.json")
            .unwrap_err()
            .contains("non-empty string")
    );

    let client = Client::builder().build().unwrap();
    let make_profile =
        |file_name: &str, local_gemini: bool, vision: VisionConfig| ProviderProfile {
            file_name: file_name.to_string(),
            display_name: file_name.to_string(),
            source: ProviderProfileSource::Native,
            model: file_name.to_string(),
            context_window: None,
            upstream_identity: None,
            identity_override: true,
            base_url: "https://example.invalid".to_string(),
            auth_token: Some("secret".to_string()),
            api_key: None,
            proxy_url: None,
            local_gemini,
            transport: if local_gemini {
                ProviderTransport::LocalGemini
            } else {
                ProviderTransport::OpenAiChat
            },
            openai_capabilities: OpenAiCapabilities::default(),
            vision,
            upstream_url: "https://example.invalid/chat/completions".to_string(),
            client: client.clone(),
        };
    let missing_target = make_profile(
        "target.json",
        false,
        VisionConfig {
            mode: VisionMode::Proxy,
            profile: Some("missing.json".to_string()),
        },
    );
    let missing_error =
        match resolve_vision_provider(std::slice::from_ref(&missing_target), &missing_target) {
            Err(message) => message,
            Ok(_) => panic!("missing vision profile must be rejected"),
        };
    assert!(missing_error.contains("missing"));

    let chain_source = make_profile(
        "source.json",
        true,
        VisionConfig {
            mode: VisionMode::Proxy,
            profile: Some("target.json".to_string()),
        },
    );
    let chain_target = make_profile(
        "target.json",
        false,
        VisionConfig {
            mode: VisionMode::Proxy,
            profile: Some("source.json".to_string()),
        },
    );
    let chain_error =
        match resolve_vision_provider(&[chain_target.clone(), chain_source], &chain_target) {
            Err(message) => message,
            Ok(_) => panic!("vision proxy chains must be rejected"),
        };
    assert!(chain_error.contains("proxy chains"));
}

#[test]
fn vision_proxy_removes_media_and_injects_untrusted_observation() {
    let client = Client::builder().build().unwrap();
    let source = ProviderProfile {
        file_name: "gemini.json".to_string(),
        display_name: "Gemini Vision".to_string(),
        source: ProviderProfileSource::Native,
        model: "gemini-3.6-flash".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: "http://127.0.0.1:18787".to_string(),
        auth_token: None,
        api_key: None,
        proxy_url: None,
        local_gemini: true,
        transport: ProviderTransport::LocalGemini,
        openai_capabilities: OpenAiCapabilities::local_gemini(),
        vision: VisionConfig::default(),
        upstream_url: "http://127.0.0.1:18787".to_string(),
        client,
    };
    let mut request = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "What error is visible?"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}},
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [
                    {"type": "text", "text": "Screenshot from the test runner"},
                    {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "cGRm"}}
                ]}
            ]
        }]
    });
    let jobs = collect_vision_jobs(&request);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].media.len(), 2);
    assert!(vision_job_is_cacheable(&jobs[0]));
    assert!(jobs[0].context.contains("What error is visible?"));
    assert_eq!(
        openai_vision_request(&source, &jobs[0])["messages"][1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part["type"] == "image_url")
            .count(),
        2
    );
    let mut completion_tokens_source = source.clone();
    completion_tokens_source
        .openai_capabilities
        .max_tokens_field = MaxTokensField::MaxCompletionTokens;
    let vision_request = openai_vision_request(&completion_tokens_source, &jobs[0]);
    assert!(vision_request.get("max_tokens").is_none());
    assert_eq!(
        vision_request["max_completion_tokens"],
        VISION_MAX_OUTPUT_TOKENS
    );
    assert!(vision_request["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("transcribe every legible character verbatim"));
    assert!(vision_request["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("complete verbatim OCR without omissions"));

    inject_vision_observation(&mut request, 0, &source, "A red compiler error is visible.")
        .unwrap();
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert!(!content.iter().any(|part| matches!(
        part.get("type").and_then(Value::as_str),
        Some("image" | "document")
    )));
    assert!(content.last().unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("untrusted visual evidence"));
    assert!(request.pointer("/messages/0/content/1/content/0").is_some());
    assert!(request.pointer("/messages/0/content/1/content/1").is_none());

    let url_request = json!({
        "messages": [{
            "role": "user",
            "content": [{"type": "image", "source": {"type": "url", "url": "https://example.invalid/changeable.png"}}]
        }]
    });
    assert!(!vision_job_is_cacheable(
        &collect_vision_jobs(&url_request)[0]
    ));
}

#[test]
fn parses_openai_and_anthropic_vision_observations() {
    assert_eq!(
        parse_vision_observation(
            ProviderTransport::OpenAiChat,
            &json!({"choices": [{"message": {"content": "visible text"}}]})
        ),
        "visible text"
    );
    assert_eq!(
        parse_vision_observation(
            ProviderTransport::Anthropic,
            &json!({"content": [{"type": "text", "text": "two buttons"}]})
        ),
        "two buttons"
    );
}

#[tokio::test]
async fn vision_proxy_calls_provider_once_and_reuses_base64_cache() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = Arc::new(AtomicUsize::new(0));
    let mock_calls = calls.clone();
    let mock = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let mock_calls = mock_calls.clone();
            async move {
                mock_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["stream"], false);
                assert!(body["messages"][1]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| part["type"] == "image_url"));
                Json(json!({
                    "choices": [{"message": {"content": "A terminal shows one failed test."}}]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let client = Client::builder().build().unwrap();
    let source = ProviderProfile {
        file_name: "vision.json".to_string(),
        display_name: "Vision Model".to_string(),
        source: ProviderProfileSource::Native,
        model: "vision-model".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: format!("http://{address}"),
        auth_token: Some("vision-secret".to_string()),
        api_key: None,
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::OpenAiChat,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url: format!("http://{address}/chat/completions"),
        client: client.clone(),
    };
    let target = ProviderProfile {
        file_name: "target.json".to_string(),
        display_name: "Text Model".to_string(),
        source: ProviderProfileSource::Native,
        model: "text-model".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: "https://example.invalid".to_string(),
        auth_token: Some("target-secret".to_string()),
        api_key: None,
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::OpenAiChat,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig {
            mode: VisionMode::Proxy,
            profile: Some("vision.json".to_string()),
        },
        upstream_url: "https://example.invalid/chat/completions".to_string(),
        client: client.clone(),
    };
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client,
            proxy_url: None,
        })),
        fallback_api_key: None,
        upstream_url: "https://example.invalid/gemini".to_string(),
        model: "fallback".to_string(),
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(InteractionContinuationCache::default())),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: vec![target.clone(), source],
            active_file: target.file_name.clone(),
            source: ProviderProfileSource::Native,
        })),
        shutdown_tx,
        settings_dir: PathBuf::new(),
        providers_dir: PathBuf::new(),
        bridge_state_path: PathBuf::new(),
        image_output_dir: env::temp_dir(),
        image_model: DEFAULT_IMAGE_MODEL.to_string(),
        image_upstream_url: DEFAULT_IMAGE_UPSTREAM.to_string(),
        local_bridge_base_url: "http://127.0.0.1:18787".to_string(),
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let original = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "What failed?"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}}
            ]
        }]
    });

    for _ in 0..2 {
        let mut request = original.clone();
        apply_vision_proxy(&state, &target, &mut request)
            .await
            .unwrap();
        let serialized = request.to_string();
        assert!(!serialized.contains("aGVsbG8="));
        assert!(serialized.contains("one failed test"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn mcp_generate_image_calls_gemini_saves_file_and_returns_preview() {
    let captured = Arc::new(tokio::sync::Mutex::new(None::<Value>));
    let captured_for_handler = captured.clone();
    let mock = Router::new().route(
        "/generate",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = captured_for_handler.clone();
            async move {
                assert_eq!(
                    headers
                        .get("x-goog-api-key")
                        .and_then(|value| value.to_str().ok()),
                    Some("image-secret")
                );
                *captured.lock().await = Some(body);
                Json(json!({
                    "status": "completed",
                    "steps": [{
                        "type": "model_output",
                        "content": [{
                            "type": "image",
                            "mime_type": "image/png",
                            "data": "aGVsbG8="
                        }]
                    }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let client = Client::builder().build().unwrap();
    let output_dir = env::temp_dir().join(format!("claude-bridge-images-{}", Uuid::new_v4()));
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = Arc::new(AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client,
            proxy_url: None,
        })),
        fallback_api_key: Some("image-secret".to_string()),
        upstream_url: "https://example.invalid/gemini".to_string(),
        model: "fallback".to_string(),
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(InteractionContinuationCache::default())),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: Vec::new(),
            active_file: String::new(),
            source: ProviderProfileSource::Native,
        })),
        shutdown_tx,
        settings_dir: PathBuf::new(),
        providers_dir: PathBuf::new(),
        bridge_state_path: PathBuf::new(),
        image_output_dir: output_dir.clone(),
        image_model: DEFAULT_IMAGE_MODEL.to_string(),
        image_upstream_url: format!("http://{address}/generate"),
        local_bridge_base_url: "http://127.0.0.1:18787".to_string(),
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    });

    let response = mcp(
        State(state),
        HeaderMap::new(),
        Json(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "generate_image",
                "arguments": {
                    "prompt": "一只可爱的小狗",
                    "aspect_ratio": "16:9",
                    "image_size": "4K"
                }
            }
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), MAX_IMAGE_RESPONSE_BYTES)
        .await
        .unwrap();
    let result: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(result["result"]["content"][1]["type"], "image");
    assert_eq!(result["result"]["content"][1]["data"], "aGVsbG8=");
    let path = PathBuf::from(
        result["result"]["structuredContent"]["path"]
            .as_str()
            .unwrap(),
    );
    assert!(path.starts_with(&output_dir));
    assert_eq!(fs::read(&path).unwrap(), b"hello");
    let request = captured.lock().await.clone().unwrap();
    assert_eq!(request["model"], DEFAULT_IMAGE_MODEL);
    assert_eq!(request["response_format"]["aspect_ratio"], "16:9");
    assert_eq!(request["response_format"]["image_size"], "4K");
    assert_eq!(request["generation_config"]["thinking_level"], "high");

    fs::remove_dir_all(&output_dir).unwrap();
    server.abort();
}

#[tokio::test]
async fn configured_kimi_formula_is_exposed_and_executed_only_when_enabled() {
    let captured = Arc::new(tokio::sync::Mutex::new(None::<Value>));
    let captured_for_fiber = captured.clone();
    let mock = Router::new()
        .route(
            "/v1/formulas/moonshot/web-search:latest/tools",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer formula-secret")
                );
                Json(json!({
                    "tools": [{
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "description": "Search the web",
                            "parameters": {
                                "type": "object",
                                "properties": {"query": {"type": "string"}},
                                "required": ["query"]
                            }
                        }
                    }]
                }))
            }),
        )
        .route(
            "/v1/formulas/moonshot/web-search:latest/fibers",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = captured_for_fiber.clone();
                async move {
                    assert_eq!(
                        headers
                            .get(AUTHORIZATION)
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer formula-secret")
                    );
                    *captured.lock().await = Some(body);
                    Json(json!({"context": {"encrypted_output": "encrypted-search-result"}}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = test_provider_profile(
        Client::builder().no_proxy().build().unwrap(),
        format!("http://{address}/anthropic/v1/messages"),
    );
    profile.file_name = "kimi-k3.json".to_string();
    profile.model = "kimi-k3".to_string();
    profile.base_url = format!("http://{address}/anthropic");
    profile.auth_token = Some("formula-secret".to_string());
    profile.openai_capabilities.chat_dialect = OpenAiChatDialect::Kimi;
    profile.openai_capabilities.kimi_formula_tools = vec!["moonshot/web-search:latest".to_string()];
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client: Client::builder().build().unwrap(),
            proxy_url: None,
        })),
        fallback_api_key: Some("local-token".to_string()),
        upstream_url: "https://example.invalid".to_string(),
        model: "bridge-router".to_string(),
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(InteractionContinuationCache::default())),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: vec![profile],
            active_file: "kimi-k3.json".to_string(),
            source: ProviderProfileSource::Native,
        })),
        shutdown_tx,
        settings_dir: PathBuf::new(),
        providers_dir: PathBuf::new(),
        bridge_state_path: PathBuf::new(),
        image_output_dir: env::temp_dir(),
        image_model: DEFAULT_IMAGE_MODEL.to_string(),
        image_upstream_url: DEFAULT_IMAGE_UPSTREAM.to_string(),
        local_bridge_base_url: "http://127.0.0.1:18787".to_string(),
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

    let tools = configured_kimi_formula_tools(&state).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].mcp_definition["name"], "web_search");
    let result = execute_kimi_formula(&state, "web_search", Some(&json!({"query": "Moonshot AI"})))
        .await
        .unwrap();
    assert_eq!(kimi_formula_result_text(&result), "encrypted-search-result");
    let captured = captured.lock().await;
    assert_eq!(captured.as_ref().unwrap()["name"], "web_search");
    assert_eq!(
        captured.as_ref().unwrap()["arguments"],
        "{\"query\":\"Moonshot AI\"}"
    );
    server.abort();
}

#[test]
fn mcp_origin_allows_only_local_browser_origins() {
    let mut headers = HeaderMap::new();
    assert!(valid_mcp_origin(&headers));
    headers.insert(ORIGIN, "http://localhost:18787".parse().unwrap());
    assert!(valid_mcp_origin(&headers));
    headers.insert(ORIGIN, "https://example.com".parse().unwrap());
    assert!(!valid_mcp_origin(&headers));
}

#[tokio::test]
async fn bounded_response_reader_rejects_chunked_oversized_bodies() {
    let mock = Router::new().route(
        "/oversized",
        get(|| async {
            let chunks = stream::iter([
                Ok::<String, Infallible>("1234".to_string()),
                Ok::<String, Infallible>("5678".to_string()),
            ]);
            Response::builder().body(Body::from_stream(chunks)).unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let response = Client::builder()
        .build()
        .unwrap()
        .get(format!("http://{address}/oversized"))
        .send()
        .await
        .unwrap();

    let error = read_response_bytes_limited(response, 7).await.unwrap_err();
    assert!(error.contains("exceeds 7 bytes"));
    server.abort();
}

#[tokio::test]
async fn vision_timeout_covers_response_body_reading() {
    let mock = Router::new().route(
        "/vision",
        post(|| async {
            let delayed = stream::once(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok::<String, Infallible>(
                    json!({"choices": [{"message": {"content": "late"}}]}).to_string(),
                )
            });
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from_stream(delayed))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let client = Client::builder().build().unwrap();
    let url = format!("http://{address}/vision");
    let source = test_provider_profile(client.clone(), url.clone());

    let error = send_vision_request_with_timeout(
        client.post(url).json(&json!({"stream": false})),
        &source,
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
    assert!(error.message.contains("timed out"));
    server.abort();
}

#[test]
fn provider_capabilities_control_optional_openai_fields() {
    let root = env::temp_dir().join(format!("claude-bridge-capabilities-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("strict.json"),
        serde_json::to_vec_pretty(&json!({
            "model": "strict-model",
            "base_url": "https://strict.example/v1",
            "api_key": "secret",
            "capabilities": {
                "stream_options": false,
                "parallel_tool_calls": false,
                "reasoning_effort": false,
                "reasoning_fields": ["analysis"],
                "thinking_tags": false,
                "tool_result_media": "inline",
                "tool_schema": "preserve",
                "max_tokens_field": "max_completion_tokens"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();
    let profile = &loaded.profiles[0];
    assert_eq!(profile.openai_capabilities.reasoning_fields, ["analysis"]);
    assert!(!profile.openai_capabilities.thinking_tags);
    assert_eq!(
        profile.openai_capabilities.tool_result_media,
        ToolResultMediaMode::Inline
    );
    assert_eq!(
        provider_profile_json(profile, &profile.file_name)["capabilities"]["tool_schema"],
        "preserve"
    );

    let request = json!({
        "stream": true,
        "max_tokens": 123,
        "thinking": {"type": "adaptive"},
        "messages": [{"role": "user", "content": "hello"}],
        "tools": [{
            "name": "inspect",
            "input_schema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {}
            }
        }],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true}
    });
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "strict-model",
        &signatures,
        &profile.openai_capabilities,
    )
    .unwrap();

    assert!(translated.get("stream_options").is_none());
    assert!(translated.get("parallel_tool_calls").is_none());
    assert!(translated.get("reasoning_effort").is_none());
    assert!(translated.get("max_tokens").is_none());
    assert_eq!(translated["max_completion_tokens"], 123);
    assert_eq!(
        translated["tools"][0]["function"]["parameters"]["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
}

#[test]
fn gemini_capabilities_enable_max_thinking_without_deprecated_sampling() {
    let request = json!({
        "stream": true,
        "max_tokens": 65_536,
        "temperature": 0.7,
        "top_p": 0.9,
        "messages": [{"role": "user", "content": "inspect the repository"}]
    });
    let capabilities = OpenAiCapabilities {
        default_reasoning_effort: Some("high".to_string()),
        include_thoughts: true,
        sampling_parameters: false,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "gemini-3.6-flash",
        &signatures,
        &capabilities,
    )
    .unwrap();

    assert!(translated.get("reasoning_effort").is_none());
    assert_eq!(
        translated["extra_body"]["google"]["thinking_config"]["include_thoughts"],
        true
    );
    assert_eq!(
        translated["extra_body"]["google"]["thinking_config"]["thinking_level"],
        "high"
    );
    assert!(translated.get("temperature").is_none());
    assert!(translated.get("top_p").is_none());
}

#[test]
fn rejects_invalid_provider_capability_types() {
    let profile = json!({"capabilities": {"tool_schema": 7}});
    let error =
        parse_openai_capabilities(profile.as_object().unwrap(), "invalid.json").unwrap_err();
    assert!(error.contains("tool_schema"));
    assert!(error.contains("non-empty string"));
}

#[test]
fn native_local_gemini_profile_uses_bridge_managed_credential() {
    let root = env::temp_dir().join(format!("claude-bridge-native-gemini-{}", Uuid::new_v4()));
    let providers_dir = root.join("bridge-providers");
    let settings_dir = root.join(".claude");
    fs::create_dir_all(&providers_dir).unwrap();
    fs::create_dir_all(&settings_dir).unwrap();
    fs::write(
        providers_dir.join("gemini.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "Google Gemini",
            "model": "gemini-3.6-flash",
            "base_url": "http://127.0.0.1:18787",
            "protocol": "gemini"
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded =
        load_provider_profiles(&providers_dir, &settings_dir, "http://127.0.0.1:18787").unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(loaded.profiles.len(), 1);
    assert_eq!(loaded.profiles[0].transport, ProviderTransport::LocalGemini);
    assert_eq!(
        loaded.profiles[0].openai_capabilities.tool_result_media,
        ToolResultMediaMode::Inline
    );
    assert!(loaded.profiles[0].auth_token.is_none());
    assert!(loaded.profiles[0].api_key.is_none());
}

#[test]
fn persists_and_loads_gemini_proxy_modes() {
    let state_path =
        env::temp_dir().join(format!("claude-bridge-proxy-state-{}.json", Uuid::new_v4()));

    persist_bridge_state(&state_path, "settings - test.json", None).unwrap();
    assert_eq!(load_persisted_gemini_proxy(&state_path), Some(None));

    persist_bridge_state(
        &state_path,
        "settings - test.json",
        Some("http://127.0.0.1:8080"),
    )
    .unwrap();
    assert_eq!(
        load_persisted_gemini_proxy(&state_path),
        Some(Some("http://127.0.0.1:8080".to_string()))
    );

    fs::remove_file(state_path).unwrap();
}

#[test]
fn translates_responses_tools_and_outputs() {
    let request = json!({
        "model": "gemini-3.6-flash",
        "instructions": "Be concise.",
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "List files"}]},
            {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"}
        ],
        "tools": [{
            "type": "function",
            "name": "shell",
            "description": "Run a command",
            "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}
        }],
        "reasoning": {"effort": "high"}
    });

    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_request(&request, "fallback", &signatures).unwrap();
    assert_eq!(translated["model"], "gemini-3.6-flash");
    assert_eq!(translated["messages"][0]["role"], "system");
    assert_eq!(translated["messages"][2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(translated["messages"][3]["role"], "tool");
    assert_eq!(translated["tools"][0]["function"]["name"], "shell");
    assert_eq!(translated["reasoning_effort"], "high");
}

#[test]
fn omits_orphan_responses_tool_outputs() {
    let request = json!({
        "input": [
            {"type": "function_call_output", "call_id": "missing", "output": "ignored"},
            {"role": "user", "content": [{"type": "input_text", "text": "continue"}]}
        ]
    });
    let signatures = RwLock::new(IndexMap::new());

    let translated = translate_request(&request, "fallback", &signatures).unwrap();
    let messages = translated["messages"].as_array().unwrap();

    assert!(messages.iter().all(|message| message["role"] != "tool"));
}

#[test]
fn produces_completed_response_event() {
    let request = json!({"model": "gemini-3.6-flash", "input": "hello"});
    let upstream = json!({
        "choices": [{"message": {"role": "assistant", "content": "OK"}}],
        "usage": {"prompt_tokens": 2, "completion_tokens": 1}
    });

    let signatures = RwLock::new(IndexMap::new());
    let events =
        translate_response_events(&request, &upstream, "gemini-3.6-flash", &signatures).unwrap();
    assert!(!events.is_empty());
}

#[test]
fn preserves_gemini_thought_signature_for_tool_roundtrip() {
    let signatures = RwLock::new(IndexMap::new());
    let first_request = json!({"model": "gemini-3.6-flash", "input": "run tool"});
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
                    "extra_content": {
                        "google": {"thought_signature": "encrypted-signature"}
                    }
                }]
            }
        }]
    });

    translate_response_events(&first_request, &upstream, "gemini-3.6-flash", &signatures).unwrap();

    let second_request = json!({
        "model": "gemini-3.6-flash",
        "input": [
            {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"}
        ]
    });
    let translated = translate_request(&second_request, "gemini-3.6-flash", &signatures).unwrap();
    assert_eq!(
        translated["messages"][0]["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
        "encrypted-signature"
    );
}

#[test]
fn preserves_gemini_text_thought_signature_for_next_turn() {
    let signatures = RwLock::new(IndexMap::new());
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "<thought>brief analysis</thought>OK",
                "extra_content": {
                    "google": {
                        "thought": true,
                        "thought_signature": "text-signature"
                    }
                }
            },
            "finish_reason": "stop"
        }]
    });
    let response =
        translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();
    assert_eq!(response["content"][0]["type"], "thinking");
    assert_eq!(response["content"][1]["text"], "OK");

    let next_request = json!({
        "messages": [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": response["content"].clone()},
            {"role": "user", "content": "continue"}
        ]
    });
    let translated =
        translate_anthropic_request(&next_request, "gemini-3.6-flash", &signatures).unwrap();
    assert_eq!(
        translated["messages"][1]["extra_content"]["google"]["thought_signature"],
        "text-signature"
    );
}

#[test]
fn translates_anthropic_messages_and_tools() {
    let request = json!({
        "model": "claude-sonnet-4-5",
        "system": [{"type": "text", "text": "You are a coding agent."}],
        "max_tokens": 4096,
        "stream": true,
        "messages": [
            {"role": "system", "content": "Runtime system context."},
            {"role": "user", "content": [{"type": "text", "text": "List files"}]},
            {"role": "assistant", "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "shell",
                "input": {"cmd": "dir"}
            }]},
            {"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "a.txt"
            }]}
        ],
        "tools": [{
            "name": "shell",
            "description": "Run a command",
            "input_schema": {
                "type": "object",
                "properties": {"cmd": {"type": "string"}},
                "required": ["cmd"]
            }
        }],
        "tool_choice": {"type": "auto"},
        "thinking": {"type": "adaptive"}
    });

    let signatures = RwLock::new(IndexMap::new());
    let translated =
        translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
    assert_eq!(translated["model"], "gemini-3.6-flash");
    assert_eq!(translated["stream"], true);
    assert_eq!(translated["stream_options"]["include_usage"], true);
    assert_eq!(translated["messages"][0]["role"], "system");
    assert_eq!(translated["messages"][1]["role"], "system");
    assert_eq!(
        translated["messages"][3]["tool_calls"][0]["function"]["name"],
        "shell"
    );
    assert_eq!(translated["messages"][4]["role"], "tool");
    assert_eq!(
        translated["tools"][0]["function"]["parameters"]["type"],
        "object"
    );
    assert_eq!(translated["tool_choice"], "auto");
    assert_eq!(translated["reasoning_effort"], "high");
}

#[test]
fn deepseek_chat_replays_reasoning_and_uses_v4_thinking_contract() {
    let request = json!({
        "messages": [
            {"role": "user", "content": "Inspect the repository"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "I should inspect Cargo.toml first."},
                {"type": "text", "text": "I will inspect it."},
                {"type": "tool_use", "id": "toolu_ds_1", "name": "read_file", "input": {"path": "Cargo.toml"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_ds_1", "content": "[package]"}
            ]}
        ],
        "thinking": {"type": "enabled", "budget_tokens": 16384},
        "output_config": {"effort": "max"},
        "tool_choice": {"type": "auto"}
    });
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::DeepSeek,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());

    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "deepseek-v4-flash",
        &signatures,
        &capabilities,
    )
    .unwrap();

    assert_eq!(translated["thinking"]["type"], "enabled");
    assert_eq!(translated["reasoning_effort"], "max");
    assert!(translated.get("tool_choice").is_none());
    assert_eq!(
        translated["messages"][1]["reasoning_content"],
        "I should inspect Cargo.toml first."
    );
}

#[test]
fn deepseek_chat_preserves_fast_high_and_max_reasoning_modes() {
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::DeepSeek,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());
    for (effort, thinking_type, expected_effort, keeps_tool_choice) in [
        ("low", "disabled", None, true),
        ("medium", "enabled", Some("high"), false),
        ("high", "enabled", Some("high"), false),
        ("xhigh", "enabled", Some("max"), false),
        ("max", "enabled", Some("max"), false),
    ] {
        let request = json!({
            "messages": [{"role": "user", "content": "Inspect one file"}],
            "thinking": {"type": "enabled"},
            "output_config": {"effort": effort},
            "tool_choice": {"type": "auto"}
        });
        let translated = translate_anthropic_request_with_capabilities(
            &request,
            "deepseek-v4-flash",
            &signatures,
            &capabilities,
        )
        .unwrap();

        assert_eq!(translated["thinking"]["type"], thinking_type);
        assert_eq!(
            translated.get("reasoning_effort").and_then(Value::as_str),
            expected_effort
        );
        assert_eq!(translated.get("tool_choice").is_some(), keeps_tool_choice);
    }
}

#[test]
fn deepseek_chat_requires_32k_budget_before_max_effort() {
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::DeepSeek,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());
    let default_request =
        json!({"messages": [{"role": "user", "content": "Inspect the repository"}]});
    let default_translated = translate_anthropic_request_with_capabilities(
        &default_request,
        "deepseek-v4-flash",
        &signatures,
        &capabilities,
    )
    .unwrap();
    assert_eq!(default_translated["thinking"]["type"], "enabled");
    assert_eq!(default_translated["reasoning_effort"], "high");

    for (budget, expected_effort) in [(16_384, "high"), (32_767, "high"), (32_768, "max")] {
        let request = json!({
            "messages": [{"role": "user", "content": "Inspect the repository"}],
            "thinking": {"type": "enabled", "budget_tokens": budget}
        });
        let translated = translate_anthropic_request_with_capabilities(
            &request,
            "deepseek-v4-flash",
            &signatures,
            &capabilities,
        )
        .unwrap();

        assert_eq!(translated["reasoning_effort"], expected_effort);
    }
}

#[test]
fn deepseek_chat_replay_respects_complete_tools_contract() {
    let request = json!({
        "messages": [
            {"role": "user", "content": "Start"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Do not replay ordinary-turn reasoning."},
                {"type": "text", "text": "Ready."}
            ]},
            {"role": "user", "content": "Inspect files"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "First complete tool reasoning."},
                {"type": "tool_use", "id": "toolu_ds_a", "name": "read_file", "input": {"path": "Cargo.toml"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_ds_a", "content": "[package]"}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Second complete tool reasoning."},
                {"type": "tool_use", "id": "toolu_ds_b", "name": "read_file", "input": {"path": "README.md"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_ds_b", "content": "# Bridge"}
            ]}
        ],
        "output_config": {"effort": "high"}
    });
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::DeepSeek,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "deepseek-v4-flash",
        &signatures,
        &capabilities,
    )
    .unwrap();
    let replayed: Vec<&str> = translated["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message.get("reasoning_content").and_then(Value::as_str))
        .collect();

    assert_eq!(
        replayed,
        vec![
            "First complete tool reasoning.",
            "Second complete tool reasoning."
        ]
    );
    let (messages, tokens) = chat_replayed_reasoning_stats(&translated);
    assert_eq!(messages, 2);
    assert!(tokens > 0);

    let mut request_with_tools = request.clone();
    request_with_tools["tools"] = json!([{
        "name": "read_file",
        "description": "Read a file",
        "input_schema": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }
    }]);
    let translated_with_tools = translate_anthropic_request_with_capabilities(
        &request_with_tools,
        "deepseek-v4-flash",
        &signatures,
        &capabilities,
    )
    .unwrap();
    let replayed_with_tools: Vec<&str> = translated_with_tools["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message.get("reasoning_content").and_then(Value::as_str))
        .collect();
    assert_eq!(
        replayed_with_tools,
        vec![
            "Do not replay ordinary-turn reasoning.",
            "First complete tool reasoning.",
            "Second complete tool reasoning."
        ]
    );
}

#[test]
fn deepseek_anthropic_route_applies_fast_and_default_reasoning_policy() {
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::DeepSeek,
        ..OpenAiCapabilities::default()
    };
    let mut fast_request = json!({
        "messages": [
            {"role": "user", "content": "Inspect one file"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Historical reasoning."},
                {"type": "text", "text": "Done."}
            ]}
        ],
        "thinking": {"type": "enabled", "budget_tokens": 16_384},
        "output_config": {
            "effort": "low",
            "format": {"type": "json_schema", "schema": {"type": "object"}}
        },
        "max_tokens": 1_024
    });
    assert!(!deepseek_anthropic_reasoning_diagnostics(&fast_request)
        .iter()
        .any(|message| message.contains("Raised DeepSeek Anthropic max_tokens")));
    let policy =
        apply_deepseek_anthropic_reasoning_policy(&mut fast_request, &capabilities).unwrap();
    assert!(!policy.thinking_enabled);
    assert_eq!(fast_request["thinking"]["type"], "disabled");
    assert!(fast_request["thinking"].get("budget_tokens").is_none());
    assert!(fast_request["output_config"].get("effort").is_none());
    assert_eq!(
        fast_request["output_config"]["format"]["type"],
        "json_schema"
    );
    assert_eq!(fast_request["max_tokens"], 1_024);
    let (messages, tokens) = deepseek_anthropic_reasoning_stats(&fast_request);
    assert_eq!(messages, 1);
    assert!(tokens > 0);

    let mut default_request =
        json!({"messages": [{"role": "user", "content": "Inspect the repository"}]});
    let policy =
        apply_deepseek_anthropic_reasoning_policy(&mut default_request, &capabilities).unwrap();
    assert!(policy.thinking_enabled);
    assert_eq!(default_request["thinking"]["type"], "enabled");
    assert_eq!(default_request["output_config"]["effort"], "high");

    let mut constrained_request = json!({
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "thinking": {"type": "enabled", "budget_tokens": 16_384},
        "max_tokens": 16_384
    });
    let diagnostics = deepseek_anthropic_reasoning_diagnostics(&constrained_request);
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("Raised DeepSeek Anthropic max_tokens")));
    apply_deepseek_anthropic_reasoning_policy(&mut constrained_request, &capabilities).unwrap();
    assert_eq!(
        constrained_request["max_tokens"],
        16_384 + ANTHROPIC_THINKING_OUTPUT_HEADROOM_TOKENS
    );

    let mut safe_request = json!({
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "thinking": {"type": "enabled", "budget_tokens": 16_384},
        "max_tokens": 16_385
    });
    apply_deepseek_anthropic_reasoning_policy(&mut safe_request, &capabilities).unwrap();
    assert_eq!(safe_request["max_tokens"], 16_385);
}

#[test]
fn qwen_chat_maps_thinking_history_budget_and_structured_output() {
    let request = json!({
        "messages": [
            {"role": "user", "content": "Extract metadata"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "I need the file."},
                {"type": "tool_use", "id": "toolu_qwen_1", "name": "read_file", "input": {"path": "Cargo.toml"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_qwen_1", "content": "[package]"}
            ]}
        ],
        "thinking": {"type": "enabled", "budget_tokens": 12000},
        "output_config": {
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }
        }
    });
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::Qwen,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());

    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "qwen3.8-max",
        &signatures,
        &capabilities,
    )
    .unwrap();

    assert_eq!(translated["enable_thinking"], true);
    assert_eq!(translated["thinking_budget"], 12000);
    assert_eq!(translated["preserve_thinking"], true);
    assert!(translated.get("reasoning_effort").is_none());
    assert_eq!(
        translated["messages"][1]["reasoning_content"],
        "I need the file."
    );
    assert_eq!(translated["response_format"]["type"], "json_schema");
    assert_eq!(
        translated["response_format"]["json_schema"]["schema"]["type"],
        "object"
    );
}

#[test]
fn qwen_anthropic_normalizes_effort_budget_and_max_tokens() {
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::Qwen,
        ..OpenAiCapabilities::default()
    };
    let mut request = json!({
        "system": "Return the result as JSON.",
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "thinking": {"type": "adaptive", "budget_tokens": 31_999},
        "output_config": {
            "effort": "high",
            "format": {"type": "json_schema", "schema": {"type": "object"}}
        },
        "max_tokens": 31_999
    });
    let diagnostics = qwen_anthropic_reasoning_diagnostics(&request);
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("adaptive")));
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("'high' to Qwen reasoning effort 'medium'")));
    assert!(diagnostics
        .iter()
        .any(|message| message.contains("Raised Qwen Anthropic max_tokens")));
    assert!(!diagnostics
        .iter()
        .any(|message| message.contains("keyword 'JSON'")));

    let policy = apply_qwen_anthropic_reasoning_policy(&mut request, &capabilities).unwrap();
    assert_eq!(policy.effort, Some("medium"));
    assert_eq!(request["thinking"]["type"], "enabled");
    assert_eq!(request["thinking"]["budget_tokens"], 31_999);
    assert_eq!(request["output_config"]["effort"], "medium");
    assert_eq!(request["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        request["max_tokens"],
        31_999 + ANTHROPIC_THINKING_OUTPUT_HEADROOM_TOKENS
    );

    let mut default_request =
        json!({"messages": [{"role": "user", "content": "Inspect one file"}]});
    let default_policy =
        apply_qwen_anthropic_reasoning_policy(&mut default_request, &capabilities).unwrap();
    assert_eq!(default_policy.effort, Some("medium"));
    assert_eq!(default_request["output_config"]["effort"], "medium");

    // Claude Code's strongest thinking trigger budgets 31,999 tokens. That
    // ceiling must reach Qwen's maximum xhigh effort instead of stopping
    // at medium, and a healthy max_tokens must stay untouched.
    let mut ultrathink_request = json!({
        "messages": [{"role": "user", "content": "Think it through"}],
        "thinking": {"type": "enabled", "budget_tokens": 31_999},
        "max_tokens": 32_000
    });
    let ultrathink_policy =
        apply_qwen_anthropic_reasoning_policy(&mut ultrathink_request, &capabilities).unwrap();
    assert_eq!(ultrathink_policy.effort, Some("xhigh"));
    assert_eq!(ultrathink_request["max_tokens"], 32_000);

    let mut ordinary_request = json!({
        "messages": [{"role": "user", "content": "Think it through"}],
        "thinking": {"type": "enabled", "budget_tokens": 8_192},
        "max_tokens": 16_000
    });
    let ordinary_policy =
        apply_qwen_anthropic_reasoning_policy(&mut ordinary_request, &capabilities).unwrap();
    assert_eq!(ordinary_policy.effort, Some("medium"));
    assert_eq!(ordinary_request["max_tokens"], 16_000);
}

#[test]
fn qwen_chat_caps_normal_effort_but_preserves_explicit_maximum() {
    let capabilities = OpenAiCapabilities {
        chat_dialect: OpenAiChatDialect::Qwen,
        ..OpenAiCapabilities::default()
    };
    let signatures = RwLock::new(IndexMap::new());
    let translate = |effort: &str| {
        let request = json!({
            "messages": [{"role": "user", "content": "Inspect the repository"}],
            "thinking": {"type": "enabled", "budget_tokens": 31_999},
            "output_config": {"effort": effort}
        });
        translate_anthropic_request_with_capabilities(
            &request,
            "qwen3.8-max",
            &signatures,
            &capabilities,
        )
        .unwrap()
    };

    let high = translate("high");
    assert_eq!(high["enable_thinking"], true);
    assert_eq!(high["thinking_budget"], QWEN_MEDIUM_CHAT_BUDGET_TOKENS);

    let default_request = json!({"messages": [{"role": "user", "content": "Inspect one file"}]});
    let default = translate_anthropic_request_with_capabilities(
        &default_request,
        "qwen3.8-max",
        &signatures,
        &capabilities,
    )
    .unwrap();
    assert_eq!(default["enable_thinking"], true);
    assert_eq!(default["thinking_budget"], QWEN_MEDIUM_CHAT_BUDGET_TOKENS);

    let maximum = translate("max");
    assert_eq!(maximum["enable_thinking"], true);
    assert_eq!(maximum["thinking_budget"], 31_999);

    let disabled = translate("none");
    assert_eq!(disabled["enable_thinking"], false);
    assert!(disabled.get("thinking_budget").is_none());
    assert!(disabled.get("preserve_thinking").is_none());
}

#[test]
fn qwen_structured_output_diagnostic_checks_prompt_text_only() {
    let mut request = json!({
        "messages": [{"role": "user", "content": "Return a typed object."}],
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}}
    });
    assert!(qwen_anthropic_reasoning_diagnostics(&request)
        .iter()
        .any(|message| message.contains("keyword 'JSON'")));

    request["messages"][0]["content"] = json!("Return a JSON object.");
    assert!(!qwen_anthropic_reasoning_diagnostics(&request)
        .iter()
        .any(|message| message.contains("keyword 'JSON'")));
}

#[test]
fn kimi_chat_replays_reasoning_and_maps_k3_agent_contract() {
    let request = json!({
        "system": "You are a coding agent.",
        "metadata": {"user_id": "private-user@example.com"},
        "messages": [
            {"role": "user", "content": "Inspect Cargo.toml"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "I should inspect the file first."},
                {"type": "tool_use", "id": "toolu_kimi_1", "name": "read_file", "input": {"path": "Cargo.toml"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_kimi_1", "content": "[package]"}
            ]}
        ],
        "max_tokens": 131072,
        "temperature": 0.2,
        "top_p": 0.8,
        "thinking": {"type": "disabled", "budget_tokens": 12000},
        "output_config": {
            "effort": "medium",
            "format": {
                "type": "json_schema",
                "name": "package",
                "schema": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }
        },
        "tool_choice": {"type": "any"}
    });
    let capabilities = OpenAiCapabilities::for_openai_base_url("https://api.moonshot.ai/v1");
    let signatures = RwLock::new(IndexMap::new());

    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "kimi-k3",
        &signatures,
        &capabilities,
    )
    .unwrap();

    assert_eq!(capabilities.chat_dialect, OpenAiChatDialect::Kimi);
    assert_eq!(translated["max_completion_tokens"], 131072);
    assert!(translated.get("max_tokens").is_none());
    assert!(translated.get("temperature").is_none());
    assert!(translated.get("top_p").is_none());
    assert!(translated.get("thinking").is_none());
    assert_eq!(translated["reasoning_effort"], "high");
    assert_eq!(translated["tool_choice"], "required");
    assert_eq!(
        translated["messages"][2]["reasoning_content"],
        "I should inspect the file first."
    );
    assert_eq!(translated["response_format"]["type"], "json_schema");
    assert_eq!(
        translated["response_format"]["json_schema"]["schema"]["type"],
        "object"
    );
    assert!(translated["prompt_cache_key"]
        .as_str()
        .unwrap()
        .starts_with("claude-bridge-"));
    assert!(translated["safety_identifier"]
        .as_str()
        .unwrap()
        .starts_with("user_"));
    assert!(!translated["safety_identifier"]
        .as_str()
        .unwrap()
        .contains("private-user"));

    let mut continued = request.clone();
    continued["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "user", "content": "Continue"}));
    assert_eq!(
        kimi_prompt_cache_key(&request),
        kimi_prompt_cache_key(&continued)
    );
}

#[test]
fn omits_empty_tool_calls_from_text_only_assistant_messages() {
    let request = json!({
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "hello"}]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "internal"},
                {"type": "text", "text": "done"}
            ]},
            {"role": "user", "content": [{"type": "text", "text": "continue"}]}
        ]
    });

    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_request(&request, "qwen3.8-max", &signatures).unwrap();
    let assistant = translated["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    assert_eq!(assistant["content"], "done");
    assert!(assistant.get("tool_calls").is_none());
}

#[test]
fn omits_orphan_anthropic_tool_results() {
    let request = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "missing", "content": "ignored"},
                {"type": "text", "text": "continue"}
            ]
        }]
    });
    let signatures = RwLock::new(IndexMap::new());

    let translated = translate_anthropic_request(&request, "qwen", &signatures).unwrap();
    let messages = translated["messages"].as_array().unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
}

#[test]
fn sanitizes_anthropic_tool_json_schema_recursively() {
    let tool = json!({
        "name": "inspect",
        "input_schema": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "root",
            "$comment": "unsupported",
            "properties": {
                "target": {
                    "$comment": "nested",
                    "properties": {
                        "path": {"type": "string", "$id": "path"}
                    }
                },
                "options": {
                    "oneOf": [
                        {"type": "string", "$schema": "nested"},
                        {"items": {"type": "integer", "$comment": "item"}}
                    ]
                }
            },
            "$defs": {
                "metadata": {"type": "string", "$comment": "definition"}
            },
            "additionalProperties": {
                "$schema": "nested",
                "properties": {"value": {"type": "string"}}
            }
        }
    });

    let translated = translate_anthropic_tool(&tool).unwrap();
    let schema = &translated["function"]["parameters"];

    assert_eq!(schema["type"], "object");
    assert!(schema.get("$schema").is_none());
    assert!(schema.get("$id").is_none());
    assert!(schema.get("$comment").is_none());
    assert_eq!(schema["properties"]["target"]["type"], "object");
    assert!(schema["properties"]["target"].get("$comment").is_none());
    assert!(schema["properties"]["target"]["properties"]["path"]
        .get("$id")
        .is_none());
    assert!(schema["properties"]["options"]["oneOf"][0]
        .get("$schema")
        .is_none());
    assert!(schema["properties"]["options"]["oneOf"][1]["items"]
        .get("$comment")
        .is_none());
    assert!(schema["$defs"]["metadata"].get("$comment").is_none());
    assert!(schema["additionalProperties"].get("$schema").is_none());
    assert_eq!(schema["additionalProperties"]["type"], "object");
}

#[test]
fn translates_multimodal_tool_results_and_pdf_user_documents() {
    let request = json!({
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_media",
                    "name": "capture",
                    "input": {}
                }]
            },
            {
                "role": "user",
                "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_media",
                    "content": [
                        {"type": "text", "text": "Screenshot captured"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "aW1hZ2U="
                            }
                        }
                    ]
                },
                {
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": "https://example.com/screenshot.png"
                    }
                },
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": "cGRm"
                    }
                }
                ]
            }
        ]
    });

    let signatures = RwLock::new(IndexMap::new());
    let translated =
        translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
    let messages = translated["messages"].as_array().unwrap();

    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["content"][0]["text"], "Screenshot captured");
    assert_eq!(
        messages[1]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(
        messages[2]["content"][0]["image_url"]["url"],
        "https://example.com/screenshot.png"
    );
    assert_eq!(
        messages[2]["content"][1]["image_url"]["url"],
        "data:application/pdf;base64,cGRm"
    );
}

#[test]
fn produces_anthropic_tool_use_and_stream_events() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I will inspect it.",
                "tool_calls": [{
                    "id": "toolu_1",
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "arguments": "{\"cmd\":\"dir\"}"
                    },
                    "extra_content": {
                        "google": {"thought_signature": "signature-1"}
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4}
    });

    let signatures = RwLock::new(IndexMap::new());
    let message = translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();
    assert_eq!(message["stop_reason"], "tool_use");
    assert_eq!(message["content"][1]["type"], "tool_use");
    assert_eq!(message["content"][1]["input"]["cmd"], "dir");
    assert_eq!(
        signatures
            .read()
            .unwrap()
            .get("toolu_1")
            .map(String::as_str),
        Some("signature-1")
    );
}

#[test]
fn emits_mixed_anthropic_tool_results_before_user_content() {
    let request = json!({
        "model": "gemini-3.6-flash",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "first_tool",
                        "input": {}
                    },
                    {
                        "type": "tool_use",
                        "id": "toolu_2",
                        "name": "second_tool",
                        "input": {}
                    }
                ]
            },
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Text before results"},
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "first result"
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "AA=="
                        }
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_2",
                        "content": "second result"
                    },
                    {"type": "text", "text": "Text after results"}
                ]
            }
        ]
    });

    let signatures = RwLock::new(IndexMap::new());
    let translated =
        translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
    let messages = translated["messages"].as_array().unwrap();

    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "toolu_1");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "toolu_2");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"][0]["text"], "Text before results");
    assert_eq!(messages[3]["content"][1]["type"], "image_url");
    assert_eq!(messages[3]["content"][2]["text"], "Text after results");
}

#[test]
fn evicts_only_oldest_thought_signatures_at_capacity() {
    let signatures = RwLock::new(IndexMap::new());
    for index in 0..THOUGHT_SIGNATURE_CAPACITY {
        remember_thought_signature(
            &signatures,
            &format!("call_{index}"),
            &format!("signature_{index}"),
        );
    }

    remember_thought_signature(&signatures, "call_new", "signature_new");
    let cache = signatures.read().unwrap();
    assert_eq!(
        cache.len(),
        THOUGHT_SIGNATURE_CAPACITY - THOUGHT_SIGNATURE_EVICTION_BATCH + 1
    );
    assert!(!cache.contains_key("call_0"));
    assert!(!cache.contains_key("call_511"));
    assert!(cache.contains_key("call_512"));
    assert_eq!(
        cache.get("call_new").map(String::as_str),
        Some("signature_new")
    );
}

#[test]
fn recovers_poisoned_thought_signature_cache() {
    let signatures = RwLock::new(IndexMap::new());
    let poison_result = std::panic::catch_unwind(|| {
        let _guard = signatures.write().unwrap();
        panic!("poison thought signature cache");
    });
    assert!(poison_result.is_err());

    remember_thought_signature(&signatures, "call_1", "signature_1");
    assert_eq!(
        recalled_thought_signature(&signatures, "call_1").as_deref(),
        Some("signature_1")
    );
}

#[test]
fn token_estimate_gives_non_ascii_text_more_headroom() {
    let request = json!({
        "messages": [{
            "role": "user",
            "content": "汉".repeat(100)
        }]
    });
    let serialized = request.to_string();
    let old_byte_quarter_estimate = serialized.len().div_ceil(4);
    let estimate = estimate_anthropic_input_tokens(&request);

    assert!(estimate > old_byte_quarter_estimate);
    assert!(estimate >= 150);
}

#[test]
fn counts_serialized_json_bytes_without_allocating_a_full_copy() {
    let request = json!({
        "escaped": "quote=\" slash=\\ newline=\n",
        "unicode": "中文🙂",
        "values": [true, false, null, 123, -4.5]
    });
    let serialized = request.to_string();
    let expected_ascii = serialized.bytes().filter(u8::is_ascii).count();
    let expected_non_ascii = serialized.len() - expected_ascii;

    assert_eq!(
        count_serialized_json_bytes(&request),
        (expected_ascii, expected_non_ascii)
    );
}

#[test]
fn decodes_sse_across_utf8_and_network_chunk_boundaries() {
    let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\r\n\r\ndata: [DONE]\n\n";
    let mut decoder = SseDataDecoder::default();
    let mut payloads = Vec::new();
    for byte in frame.as_bytes().chunks(1) {
        payloads.extend(decoder.push_bytes(byte).unwrap());
    }
    payloads.extend(decoder.finish().unwrap());

    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains("你好"));
    assert_eq!(payloads[1], "[DONE]");
}

#[test]
fn bounds_sse_frames_and_replaces_invalid_utf8() {
    let mut decoder = SseDataDecoder::default();
    let oversized = vec![b'x'; MAX_UPSTREAM_SSE_BUFFER_BYTES + 1];
    assert!(decoder.push_bytes(&oversized).is_err());

    let mut decoder = SseDataDecoder::default();
    let payloads = decoder
        .push_bytes(b"data: {\"text\":\"bad \xff byte\"}\n\n")
        .unwrap();
    assert_eq!(payloads.len(), 1);
    assert!(payloads[0].contains('\u{fffd}'));
    assert!(serde_json::from_str::<Value>(&payloads[0]).is_ok());
}

#[tokio::test]
async fn upstream_byte_stream_emits_complete_anthropic_sequence_at_done() {
    let body = collect_translated_sse(
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
            "data: this must be ignored after done\n\n"
        )
        .to_string(),
    )
    .await;
    let values = anthropic_sse_event_values(&body);
    let types = values
        .iter()
        .filter_map(|value| value.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>();

    assert_eq!(
        types,
        [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );
    assert!(body.contains("Hello"));
    assert!(!body.contains("must be ignored"));
}

#[tokio::test]
async fn upstream_byte_stream_finishes_cleanly_at_eof_with_finish_reason() {
    let body = collect_translated_sse(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Complete\"},\"finish_reason\":\"stop\"}]}\n\n"
                .to_string(),
        )
        .await;
    let values = anthropic_sse_event_values(&body);

    assert!(values.iter().any(|value| value["type"] == "message_stop"));
    assert!(!values.iter().any(|value| value["type"] == "error"));
}

#[tokio::test]
async fn empty_keepalive_and_usage_only_tail_complete_the_stream() {
    let body = collect_translated_sse(
            concat!(
                "data:\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Complete\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3}}\n\n"
            )
            .to_string(),
        )
        .await;
    let values = anthropic_sse_event_values(&body);

    assert!(values.iter().any(|value| value["type"] == "message_stop"));
    assert!(!values.iter().any(|value| value["type"] == "error"));
    let message_delta = values
        .iter()
        .find(|value| value["type"] == "message_delta")
        .unwrap();
    assert_eq!(message_delta["usage"]["output_tokens"], 3);
}

#[tokio::test]
async fn truncated_upstream_byte_stream_injects_error_without_message_stop() {
    let body = collect_translated_sse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Partial\"},\"finish_reason\":null}]}\n\n"
            .to_string(),
    )
    .await;
    let values = anthropic_sse_event_values(&body);

    assert!(values.iter().any(|value| value["type"] == "error"));
    assert!(!values.iter().any(|value| value["type"] == "message_stop"));
}

#[tokio::test]
async fn upstream_byte_stream_read_failure_is_exposed_as_an_error_event() {
    let byte_stream = stream::iter([Err::<Vec<u8>, &'static str>("mock stream failure")]);
    let event_stream = anthropic_upstream_event_stream(
        byte_stream,
        "integration-model".to_string(),
        Arc::new(RwLock::new(IndexMap::new())),
        0,
        OpenAiCapabilities::default(),
    );
    let response = Sse::new(event_stream).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_UPSTREAM_RESPONSE_BYTES)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let values = anthropic_sse_event_values(&body);
    let error = values
        .iter()
        .find(|value| value["type"] == "error")
        .unwrap();

    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("mock stream failure"));
    assert!(!values.iter().any(|value| value["type"] == "message_stop"));
}

#[tokio::test]
async fn late_thinking_closes_an_open_text_block_before_starting() {
    let body = collect_translated_sse(
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Answer first\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Late thought\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        )
        .await;
    let values = anthropic_sse_event_values(&body);
    let block_events = values
        .iter()
        .filter(|value| {
            matches!(
                value.get("type").and_then(Value::as_str),
                Some("content_block_start" | "content_block_stop")
            )
        })
        .map(|value| {
            (
                value["type"].as_str().unwrap().to_string(),
                value["index"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        block_events,
        [
            ("content_block_start".to_string(), 0),
            ("content_block_stop".to_string(), 0),
            ("content_block_start".to_string(), 1),
            ("content_block_stop".to_string(), 1),
        ]
    );
}

#[test]
fn emits_text_delta_before_upstream_stream_finishes() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures, 42);

    assert_eq!(translator.start_events().unwrap().len(), 1);
    assert_eq!(translator.input_tokens, 42);
    let first_delta = translator
            .process_payload(
                r#"{"choices":[{"delta":{"role":"assistant","content":"Hello "},"finish_reason":null}]}"#,
            )
            .unwrap();
    assert_eq!(first_delta.len(), 2);
    assert!(!translator.finished);

    let second_delta = translator
            .process_payload(
                r#"{"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            )
            .unwrap();
    assert_eq!(second_delta.len(), 1);
    assert_eq!(translator.input_tokens, 10);
    assert_eq!(translator.output_tokens, 2);
    assert_eq!(translator.finish().unwrap().len(), 3);
}

#[test]
fn streams_thinking_blocks_and_closes_them_before_text() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures, 0);

    let thinking_events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"reasoning_content":"Inspecting context"},"finish_reason":null}]}"#,
            )
            .unwrap();
    let thinking_debug = format!("{thinking_events:?}");
    assert_eq!(thinking_events.len(), 2);
    assert!(thinking_debug.contains("thinking"));
    assert!(thinking_debug.contains("thinking_delta"));
    assert!(thinking_debug.contains("Inspecting context"));
    assert_eq!(translator.thinking_block_index, Some(0));

    let text_events = translator
        .process_payload(r#"{"choices":[{"delta":{"content":"Done"},"finish_reason":"stop"}]}"#)
        .unwrap();
    let text_debug = format!("{text_events:?}");
    assert_eq!(text_events.len(), 3);
    assert!(text_debug.contains("content_block_stop"));
    assert!(text_debug.contains("text_delta"));
    assert_eq!(translator.thinking_block_index, None);
    assert_eq!(translator.text_block_index, Some(1));
}

#[test]
fn prepends_non_streaming_thinking_content() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "reasoning_content": "I should inspect the repository.",
                "content": "I found the issue."
            },
            "finish_reason": "stop"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());

    let message = translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

    assert_eq!(message["content"][0]["type"], "thinking");
    assert_eq!(
        message["content"][0]["thinking"],
        "I should inspect the repository."
    );
    assert_eq!(message["content"][1]["type"], "text");
}

#[test]
fn extracts_tagged_thinking_across_stream_chunks() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("reasoning-model".to_string(), signatures, 0);

    let opening = translator
        .process_payload(r#"{"choices":[{"delta":{"content":"<thou"},"finish_reason":null}]}"#)
        .unwrap();
    assert!(opening.is_empty());

    let thinking = translator
        .process_payload(
            r#"{"choices":[{"delta":{"content":"ght>inspect files</thou"},"finish_reason":null}]}"#,
        )
        .unwrap();
    let thinking_debug = format!("{thinking:?}");
    assert!(thinking_debug.contains("thinking_delta"));
    assert!(thinking_debug.contains("inspect files"));

    let answer = translator
            .process_payload(
                r#"{"choices":[{"delta":{"content":"ght>Done","extra_content":{"google":{"thought_signature":"stream-text-signature"}}},"finish_reason":"stop"}]}"#,
            )
            .unwrap();
    let answer_debug = format!("{answer:?}");
    assert!(answer_debug.contains("content_block_stop"));
    assert!(answer_debug.contains("signature_delta"));
    assert!(answer_debug.contains("stream-text-signature"));
    assert!(answer_debug.contains("text_delta"));
    assert!(answer_debug.contains("Done"));
}

#[test]
fn extracts_tagged_thinking_non_streaming_and_allows_opt_out() {
    let upstream = json!({
        "choices": [{
            "message": {"content": "<think>Inspect repository</think>Answer"},
            "finish_reason": "stop"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_response_with_capabilities(
        &upstream,
        "reasoning-model",
        &signatures,
        &OpenAiCapabilities::default(),
    )
    .unwrap();
    assert_eq!(translated["content"][0]["type"], "thinking");
    assert_eq!(translated["content"][0]["thinking"], "Inspect repository");
    assert_eq!(translated["content"][1]["text"], "Answer");

    let capabilities = OpenAiCapabilities {
        thinking_tags: false,
        ..OpenAiCapabilities::default()
    };
    let preserved = translate_anthropic_response_with_capabilities(
        &upstream,
        "literal-tag-model",
        &signatures,
        &capabilities,
    )
    .unwrap();
    assert_eq!(
        preserved["content"][0]["text"],
        "<think>Inspect repository</think>Answer"
    );
}

#[test]
fn standard_tool_results_keep_tool_content_text_only_and_move_media_to_user() {
    let request = json!({
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_media",
                    "name": "capture",
                    "input": {}
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_media",
                    "content": [
                        {"type": "text", "text": "Screenshot captured"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "aW1hZ2U="
                            }
                        }
                    ]
                }]
            }
        ]
    });
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_request_with_capabilities(
        &request,
        "strict-openai-model",
        &signatures,
        &OpenAiCapabilities::default(),
    )
    .unwrap();

    assert!(translated["messages"][1]["content"].is_string());
    assert!(translated["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("[Image result attached]"));
    assert_eq!(translated["messages"][2]["role"], "user");
    assert_eq!(translated["messages"][2]["content"][0]["type"], "image_url");

    let gemini = translate_anthropic_request(&request, "gemini", &signatures).unwrap();
    assert!(gemini["messages"][1]["content"].is_array());
}

#[test]
fn conservatively_repairs_common_tool_argument_json_defects() {
    assert_eq!(parse_tool_arguments(r#"{"a":1,}"#).unwrap()["a"], 1);
    assert_eq!(
        parse_tool_arguments("{\"text\":\"line 1\nline 2\"}").unwrap()["text"],
        "line 1\nline 2"
    );
    assert_eq!(
        parse_tool_arguments(r#"{"path":"src""#).unwrap()["path"],
        "src"
    );
    assert!(parse_tool_arguments(r#"{"path":"unterminated"#).is_err());
}

#[test]
fn streams_repaired_tool_arguments_instead_of_the_malformed_source() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("small-tool-model".to_string(), signatures, 0);
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"id":"call_repair","function":{"name":"inspect","arguments":"{\"path\":\"src\""}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

    let events = translator.finish().unwrap();
    let debug = format!("{events:?}");
    assert!(debug.contains(r#"partial_json\":\"{\\\"path\\\":\\\"src\\\"}\""#));
}

#[test]
fn generates_distinct_ids_for_parallel_streamed_tool_calls_with_blank_ids() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("qwen-tool-model".to_string(), signatures, 0);
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","type":"function","function":{"name":"first","arguments":"{}"}},{"index":1,"id":"   ","type":"function","function":{"name":"second","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

    let ids = translator
        .tool_calls
        .values()
        .map(|call| call.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| id.starts_with("toolu_")));
    assert_ne!(ids[0], ids[1]);

    let debug = format!("{:?}", translator.finish().unwrap());
    assert!(ids.iter().all(|id| debug.contains(id)));
}

#[test]
fn generates_distinct_ids_for_parallel_non_streaming_tool_calls_with_blank_ids() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "",
                        "type": "function",
                        "function": {"name": "first", "arguments": "{}"}
                    },
                    {
                        "id": "   ",
                        "type": "function",
                        "function": {"name": "second", "arguments": "{}"}
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());

    let message = translate_anthropic_response(&upstream, "qwen-tool-model", &signatures).unwrap();
    let ids = message["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .map(|block| block["id"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| id.starts_with("toolu_")));
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn maps_openai_http_failures_to_anthropic_retry_contracts() {
    assert_eq!(
        openai_error_contract(StatusCode::TOO_MANY_REQUESTS, "quota"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error")
    );
    assert_eq!(
        openai_error_contract(StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
        (StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error")
    );
    let overloaded = StatusCode::from_u16(529).unwrap();
    assert_eq!(
        openai_error_contract(overloaded, "overloaded"),
        (overloaded, "overloaded_error")
    );
    assert_eq!(
        openai_error_contract(StatusCode::INTERNAL_SERVER_ERROR, "context length exceeded"),
        (StatusCode::BAD_REQUEST, "invalid_request_error")
    );
    assert_eq!(
        openai_error_contract(StatusCode::BAD_GATEWAY, "upstream unavailable").1,
        "api_error"
    );
}

#[test]
fn clean_stream_eof_requires_done_or_finish_reason() {
    assert!(!stream_eof_is_complete(false, None, false));
    assert!(stream_eof_is_complete(true, None, false));
    assert!(stream_eof_is_complete(false, Some("stop"), false));
    assert!(stream_eof_is_complete(false, None, true));
}

#[test]
fn field_driven_response_matrix_handles_openai_variants() {
    let signatures = RwLock::new(IndexMap::new());
    let cases = [
        (
            "object tool arguments",
            json!({
                "choices": [{
                    "message": {
                        "content": null,
                        "tool_calls": [{
                            "id": "call_object",
                            "function": {"name": "inspect", "arguments": {"path": "src"}}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"input_tokens": 7, "output_tokens": 3}
            }),
            "tool_use",
        ),
        (
            "legacy function_call",
            json!({
                "choices": [{
                    "message": {
                        "content": null,
                        "function_call": {"name": "inspect", "arguments": "{\"path\":\"src\"}"}
                    },
                    "finish_reason": "function_call"
                }]
            }),
            "tool_use",
        ),
        (
            "standard refusal",
            json!({
                "choices": [{
                    "message": {"content": null, "refusal": "Request refused."},
                    "finish_reason": "stop"
                }]
            }),
            "text",
        ),
    ];

    for (case, upstream, expected_block_type) in cases {
        let message = translate_anthropic_response(&upstream, case, &signatures).unwrap();
        assert_eq!(
            message["content"][0]["type"], expected_block_type,
            "failed compatibility case: {case}"
        );
        if case == "object tool arguments" {
            assert_eq!(message["content"][0]["input"]["path"], "src");
            assert_eq!(message["usage"]["input_tokens"], 7);
            assert_eq!(message["usage"]["output_tokens"], 3);
        }
        if case == "standard refusal" {
            assert_eq!(message["stop_reason"], "refusal");
        }
    }
}

#[test]
fn configured_reasoning_field_and_usage_aliases_work_in_streams() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let capabilities = OpenAiCapabilities {
        reasoning_fields: vec!["analysis".to_string()],
        ..OpenAiCapabilities::default()
    };
    let mut translator = AnthropicStreamTranslator::with_capabilities(
        "custom-model".to_string(),
        signatures,
        0,
        capabilities,
    );

    let events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"analysis":"Checking","content":null},"finish_reason":null}],"usage":{"input_tokens":11,"output_tokens":4}}"#,
            )
            .unwrap();
    let debug = format!("{events:?}");
    assert!(debug.contains("thinking_delta"));
    assert!(debug.contains("Checking"));
    assert_eq!(translator.input_tokens, 11);
    assert_eq!(translator.output_tokens, 4);

    let refusal_events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"refusal":"Not allowed."},"finish_reason":"content_filter"}]}"#,
            )
            .unwrap();
    assert!(format!("{refusal_events:?}").contains("Not allowed."));
    assert!(format!("{:?}", translator.finish().unwrap()).contains("refusal"));
}

#[test]
fn turns_prompt_feedback_into_anthropic_refusal() {
    let upstream = json!({
        "promptFeedback": {"blockReason": "SAFETY"},
        "choices": []
    });
    let signatures = RwLock::new(IndexMap::new());

    let message = translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

    assert_eq!(message["stop_reason"], "refusal");
    assert_eq!(
        message["content"][0]["text"],
        "Gemini Safety Intercept: Request was blocked by safety guardrails (Reason: SAFETY)."
    );
    assert!(safe_error_message(&upstream).contains("Reason: SAFETY"));
}

#[test]
fn streams_prompt_feedback_as_anthropic_refusal() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator = AnthropicStreamTranslator::new("gemini".to_string(), signatures, 0);

    let events = translator
        .process_payload(r#"{"promptFeedback":{"blockReason":"SAFETY"},"choices":[]}"#)
        .unwrap();
    let finish = translator.finish().unwrap();

    assert!(format!("{events:?}").contains("Gemini Safety Intercept"));
    assert!(format!("{finish:?}").contains("refusal"));
}

#[test]
fn streams_thinking_regardless_of_model_name() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator = AnthropicStreamTranslator::new("deepseek-chat".to_string(), signatures, 0);

    let thinking_events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"reasoning_content":"Reasoning step"},"finish_reason":null}]}"#,
            )
            .unwrap();
    let debug = format!("{thinking_events:?}");
    assert!(debug.contains("thinking_delta"));
    assert!(debug.contains("Reasoning step"));
    assert_eq!(translator.thinking_block_index, Some(0));
}

#[test]
fn translates_gemini_response_fields_without_model_name_gating() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "reasoning_content": "Deep reasoning",
                "content": "Result",
                "tool_calls": [{
                    "id": "call_g",
                    "type": "function",
                    "function": {"name": "shell", "arguments": "{\"cmd\":\"dir\"}"},
                    "extra_content": {
                        "google": {"thought_signature": "sig-from-field"}
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());
    // A Claude-named model must not change field-driven translation.
    let message =
        translate_anthropic_response(&upstream, "claude-sonnet-4-5", &signatures).unwrap();
    assert_eq!(message["content"][0]["type"], "thinking");
    assert_eq!(message["content"][1]["type"], "text");
    assert_eq!(message["content"][2]["type"], "tool_use");
    assert_eq!(
        signatures.read().unwrap().get("call_g").map(String::as_str),
        Some("sig-from-field")
    );
}

#[test]
fn turns_prompt_feedback_into_refusal_without_model_name_gating() {
    let upstream = json!({
        "promptFeedback": {"blockReason": "SAFETY"},
        "choices": []
    });
    let signatures = RwLock::new(IndexMap::new());
    // The blockReason field identifies Gemini; the model name is irrelevant.
    let message = translate_anthropic_response(&upstream, "gpt-5.2", &signatures).unwrap();
    assert_eq!(message["stop_reason"], "refusal");
    assert_eq!(message["content"][0]["type"], "text");
}

#[test]
fn aggregates_streamed_tool_call_and_preserves_signature() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures.clone(), 0);

    let events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"id":"toolu_stream_1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":"},"extra_content":{"google":{"thought_signature":"stream-signature"}}}]},"finish_reason":null}]}"#,
            )
            .unwrap();
    assert!(events.is_empty());
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"\"pwd\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

    let finish_events = translator.finish().unwrap();
    assert_eq!(finish_events.len(), 5);
    assert_eq!(
        signatures
            .read()
            .unwrap()
            .get("toolu_stream_1")
            .map(String::as_str),
        Some("stream-signature")
    );
}

#[test]
fn separates_sequential_anonymous_streamed_tool_calls() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("nonstandard-model".to_string(), signatures, 0);

    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"first","arguments":"{}"}}]},"finish_reason":null}]}"#,
            )
            .unwrap();
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"second","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

    assert_eq!(translator.tool_calls.len(), 2);
    assert_eq!(translator.tool_calls[0].name, "first");
    assert_eq!(translator.tool_calls[1].name, "second");
}

#[test]
fn merges_repeated_anonymous_tool_names_while_arguments_are_incomplete() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("nonstandard-model".to_string(), signatures, 0);

    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"inspect","arguments":"{\"path\":\"src\""}}]},"finish_reason":null}]}"#,
            )
            .unwrap();
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"inspect","arguments":",\"depth\":2}"}}]},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

    assert_eq!(translator.tool_calls.len(), 1);
    assert_eq!(translator.tool_calls[0].name, "inspect");
    assert_eq!(
        parse_tool_arguments(&translator.tool_calls[0].arguments).unwrap()["path"],
        "src"
    );
    assert_eq!(
        parse_tool_arguments(&translator.tool_calls[0].arguments).unwrap()["depth"],
        2
    );

    let mut sequential = AnthropicStreamTranslator::new(
        "nonstandard-model".to_string(),
        Arc::new(RwLock::new(IndexMap::new())),
        0,
    );
    for arguments in ["{}", "{\"path\":\"other\"}"] {
        sequential
                .process_payload(&format!(
                    r#"{{"choices":[{{"delta":{{"tool_calls":[{{"function":{{"name":"inspect","arguments":{}}}}}]}}}}]}}"#,
                    serde_json::to_string(arguments).unwrap()
                ))
                .unwrap();
    }
    assert_eq!(sequential.tool_calls.len(), 2);
}

#[test]
fn max_tokens_suppresses_truncated_non_streaming_tool_call() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "toolu_truncated",
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "arguments": "{\"cmd\":\"unterminated"
                    },
                    "extra_content": {
                        "google": {"thought_signature": "must-not-be-cached"}
                    }
                }]
            },
            "finish_reason": "length"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());

    let message = translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();

    assert_eq!(message["stop_reason"], "max_tokens");
    assert_eq!(message["content"], json!([]));
    assert!(!signatures.read().unwrap().contains_key("toolu_truncated"));
}

#[test]
fn max_tokens_suppresses_truncated_streaming_tool_call() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("gemini-3.6-flash".to_string(), signatures.clone(), 12);
    translator
            .process_payload(
                r#"{"choices":[{"delta":{"tool_calls":[{"id":"toolu_truncated","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"unterminated"},"extra_content":{"google":{"thought_signature":"must-not-be-cached"}}}]},"finish_reason":"length"}]}"#,
            )
            .unwrap();

    let events = translator.finish().unwrap();

    assert_eq!(events.len(), 2);
    assert!(!signatures.read().unwrap().contains_key("toolu_truncated"));
}

#[test]
fn skips_invalid_completed_tool_arguments_without_losing_valid_content() {
    let upstream = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I found two actions.",
                "tool_calls": [
                    {
                        "id": "toolu_valid",
                        "type": "function",
                        "function": {
                            "name": "inspect",
                            "arguments": "{\"path\":\"src\"}"
                        }
                    },
                    {
                        "id": "toolu_invalid",
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"cmd\":\"unterminated"
                        },
                        "extra_content": {
                            "google": {"thought_signature": "must-not-be-cached"}
                        }
                    }
                ]
            },
            "finish_reason": "stop"
        }]
    });
    let signatures = RwLock::new(IndexMap::new());

    let message = translate_anthropic_response(&upstream, "gemini-3.6-flash", &signatures).unwrap();
    let content = message["content"].as_array().unwrap();

    assert_eq!(content[0]["text"], "I found two actions.");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["id"], "toolu_valid");
    assert_eq!(content.len(), 2);
    assert_eq!(message["stop_reason"], "tool_use");
    assert!(!signatures.read().unwrap().contains_key("toolu_invalid"));
}

#[test]
fn skips_invalid_streamed_tool_arguments_without_losing_valid_content() {
    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut translator =
        AnthropicStreamTranslator::new("openrouter-model".to_string(), signatures.clone(), 0);
    let mut events = translator
            .process_payload(
                r#"{"choices":[{"delta":{"content":"I found two actions.","tool_calls":[{"index":0,"id":"toolu_valid","function":{"name":"inspect","arguments":"{\"path\":\"src\"}"}},{"index":1,"id":"toolu_invalid","function":{"name":"shell","arguments":"{\"cmd\":\"unterminated"},"extra_content":{"google":{"thought_signature":"must-not-be-cached"}}}]},"finish_reason":"stop"}]}"#,
            )
            .unwrap();

    events.extend(translator.finish().unwrap());
    let debug = format!("{events:?}");
    assert!(debug.contains("I found two actions."));
    assert!(debug.contains("toolu_valid"));
    assert!(!debug.contains("toolu_invalid"));
    assert!(!signatures.read().unwrap().contains_key("toolu_invalid"));
}

#[test]
fn maps_content_filter_to_refusal() {
    assert_eq!(
        anthropic_stop_reason(Some("content_filter"), false),
        "refusal"
    );
    assert_eq!(anthropic_stop_reason(Some("SAFETY"), true), "refusal");
    assert_eq!(anthropic_stop_reason(Some("length"), true), "max_tokens");
    assert_eq!(
        safe_error_message(&json!({"detail": {"message": "provider detail"}})),
        "provider detail"
    );
}

#[test]
fn forwarding_prefers_bearer_and_supplies_default_version() {
    let client = Client::builder().build().unwrap();
    let profile = ProviderProfile {
        file_name: "test.json".to_string(),
        display_name: "Test".to_string(),
        source: ProviderProfileSource::Native,
        model: "test-model".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: "https://example.invalid".to_string(),
        auth_token: Some("bearer-secret".to_string()),
        api_key: Some("api-key-secret".to_string()),
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::Anthropic,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url: "https://example.invalid/v1/messages".to_string(),
        client: client.clone(),
    };
    let request = apply_anthropic_forward_headers(
        client.post("https://example.invalid/v1/messages"),
        &profile,
        &HeaderMap::new(),
    )
    .build()
    .unwrap();

    assert_eq!(
        request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer bearer-secret")
    );
    assert!(!request.headers().contains_key("x-api-key"));
    assert_eq!(
        request
            .headers()
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok()),
        Some(DEFAULT_ANTHROPIC_VERSION)
    );
    assert!(!request.headers().contains_key("anthropic-beta"));
    assert!(!request.headers().contains_key("x-dashscope-session-cache"));
}

#[test]
fn qwen_anthropic_forwards_session_cache_header_for_official_domains() {
    let base_url = "https://workspace.cn-beijing.maas.aliyuncs.com/apps/anthropic";
    let capabilities = OpenAiCapabilities::for_anthropic_base_url(base_url);
    assert!(capabilities.responses_session_cache);

    let client = Client::builder().build().unwrap();
    let profile = ProviderProfile {
        file_name: "qwen.json".to_string(),
        display_name: "Qwen3.8 Max".to_string(),
        source: ProviderProfileSource::Native,
        model: "qwen3.8-max".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: base_url.to_string(),
        auth_token: None,
        api_key: Some("secret".to_string()),
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::Anthropic,
        openai_capabilities: capabilities,
        vision: VisionConfig::default(),
        upstream_url: format!("{base_url}/v1/messages"),
        client: client.clone(),
    };
    let request = apply_anthropic_forward_headers(
        client.post(&profile.upstream_url),
        &profile,
        &HeaderMap::new(),
    )
    .build()
    .unwrap();
    assert_eq!(
        request
            .headers()
            .get("x-dashscope-session-cache")
            .and_then(|value| value.to_str().ok()),
        Some("enable")
    );

    let generic = OpenAiCapabilities::for_anthropic_base_url("https://openrouter.ai/api");
    assert!(!generic.responses_session_cache);
}

#[tokio::test]
async fn anthropic_forwarding_rejects_non_object_request_bodies() {
    let profile = ProviderProfile {
        file_name: "anthropic.json".to_string(),
        display_name: "Anthropic".to_string(),
        source: ProviderProfileSource::Native,
        model: "claude-test".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: false,
        base_url: "https://example.invalid".to_string(),
        auth_token: None,
        api_key: Some("secret".to_string()),
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::Anthropic,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url: "https://example.invalid/v1/messages".to_string(),
        client: Client::builder().build().unwrap(),
    };

    let response = forward_anthropic_profile(profile, &HeaderMap::new(), json!(5)).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn routes_known_non_claude_providers_through_openai_chat() {
    let cases = [
        (
            "https://dashscope.aliyuncs.com/apps/anthropic",
            "qwen3.8-max",
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
        (
            "https://api.deepseek.com/anthropic",
            "deepseek-v4-pro",
            "https://api.deepseek.com/chat/completions",
        ),
        (
            "https://api.moonshot.cn/anthropic",
            "kimi-k3",
            "https://api.moonshot.cn/v1/chat/completions",
        ),
    ];

    for (base_url, model, expected_url) in cases {
        let (transport, upstream_url) =
            resolve_provider_transport(base_url, model, false, None, None).unwrap();
        assert_eq!(transport, ProviderTransport::OpenAiChat);
        assert_eq!(upstream_url, expected_url);
    }
}

#[test]
fn keeps_claude_and_unknown_providers_on_anthropic_by_default() {
    let (claude_transport, claude_url) = resolve_provider_transport(
        "https://openrouter.ai/api",
        "anthropic/claude-opus-5",
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(claude_transport, ProviderTransport::Anthropic);
    assert_eq!(claude_url, "https://openrouter.ai/api/v1/messages");

    let (unknown_transport, unknown_url) = resolve_provider_transport(
        "https://provider.example/anthropic",
        "custom-model",
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(unknown_transport, ProviderTransport::Anthropic);
    assert_eq!(
        unknown_url,
        "https://provider.example/anthropic/v1/messages"
    );
}

#[test]
fn explicit_transport_and_upstream_url_override_auto_routing() {
    let (transport, upstream_url) = resolve_provider_transport(
        "https://provider.example/anthropic",
        "custom-model",
        false,
        Some("openai-chat"),
        Some("https://gateway.example/chat/completions"),
    )
    .unwrap();
    assert_eq!(transport, ProviderTransport::OpenAiChat);
    assert_eq!(upstream_url, "https://gateway.example/chat/completions");
}

#[test]
fn neutralizes_claude_identity_and_appends_runtime_identity() {
    let original = "You are Claude Code, Anthropic's official CLI for Claude.";
    let mut request = json!({
        "system": format!("<system-reminder>Runtime context.</system-reminder>\n{original}"),
        "messages": [{"role": "user", "content": "Who are you?"}]
    });

    assert!(append_bridge_identity(&mut request, "Google Gemini (gemini-3.6-flash)").unwrap());
    assert!(!append_bridge_identity(&mut request, "Google Gemini (gemini-3.6-flash)").unwrap());

    let system = request["system"].as_str().unwrap();
    assert!(system.contains(
            "You are \"Google Gemini (gemini-3.6-flash)\", the upstream model serving this route, operating inside the Claude Code client."
        ));
    assert!(!system.contains(original));
    assert_eq!(system.matches(BRIDGE_IDENTITY_MARKER).count(), 1);
    assert!(system.contains(
        "actual model and first-person assistant identity is \"Google Gemini (gemini-3.6-flash)\""
    ));
    assert!(system.contains("answer truthfully based on what you actually are"));
    assert!(system.contains("请根据你的真实身份如实回答"));

    let signatures = RwLock::new(IndexMap::new());
    let translated =
        translate_anthropic_request(&request, "gemini-3.6-flash", &signatures).unwrap();
    let translated_system = translated["messages"][0]["content"].as_str().unwrap();
    assert!(translated_system.contains(
            "You are \"Google Gemini (gemini-3.6-flash)\", the upstream model serving this route, operating inside the Claude Code client."
        ));
    assert!(translated_system.contains(BRIDGE_IDENTITY_MARKER));
    let translated_messages = translated["messages"].as_array().unwrap();
    let reminder = translated_messages.last().unwrap();
    assert_eq!(reminder["role"], "system");
    assert!(reminder["content"]
        .as_str()
        .unwrap()
        .starts_with(BRIDGE_IDENTITY_MARKER));
}

#[test]
fn preserves_system_blocks_and_cache_control_when_appending_identity() {
    let mut request = json!({
        "system": [{
            "type": "text",
            "text": "You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.\nCached Claude Code instructions.",
            "cache_control": {"type": "ephemeral"}
        }, {
            "type": "text",
            "text": "You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers."
        }],
        "messages": []
    });

    append_bridge_identity(&mut request, "DeepSeek V3").unwrap();
    let blocks = request["system"].as_array().unwrap();

    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
    assert!(blocks[0]["text"].as_str().unwrap().starts_with(
            "You are \"DeepSeek V3\", the upstream model serving this route, operating inside the Claude Code client."
        ));
    assert!(blocks[0]["text"]
        .as_str()
        .unwrap()
        .ends_with("Cached Claude Code instructions."));
    assert!(blocks[1]["text"].as_str().unwrap().starts_with(
            "You are \"DeepSeek V3\", the upstream model serving this route. You operate inside the Claude Code client."
        ));
    assert!(blocks[2]["text"]
        .as_str()
        .unwrap()
        .contains("actual model and first-person assistant identity is \"DeepSeek V3\""));
}

#[test]
fn keeps_claude_identity_and_unrelated_claude_code_references_unchanged() {
    assert!(is_claude_identity("Anthropic Claude Sonnet 4"));
    assert!(!is_claude_identity("qwen3.8-max"));

    let claude_declaration =
        "You are Claude Code, Anthropic's official CLI for Claude.".to_string();
    let mut actual_claude = claude_declaration.clone();
    assert!(!neutralize_claude_identity_declaration(
        &mut actual_claude,
        "Anthropic Claude Sonnet 4"
    ));
    assert_eq!(actual_claude, claude_declaration);

    let mut unrelated = "Use Claude Code tools exactly as documented.".to_string();
    assert!(!neutralize_claude_identity_declaration(
        &mut unrelated,
        "Google Gemini"
    ));
    assert_eq!(unrelated, "Use Claude Code tools exactly as documented.");
}

#[test]
fn neutralizes_subagent_personas_environment_and_attribution() {
    let mut text = concat!(
            "You are a file search specialist for Claude Code, Anthropic's official CLI for Claude.\n",
            "You are an agent for Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.\n",
            "# Environment\n",
            "You are powered by the model named Claude Sonnet 4.5. The exact model ID is claude-sonnet-4-5-20250929.\n",
            "End git commit messages with: Co-Authored-By: Claude <noreply@anthropic.com>"
        )
        .to_string();

    assert!(neutralize_claude_identity_declaration(
        &mut text,
        "qwen3.8-max"
    ));
    assert!(text.contains(
            "You are a file search specialist for \"qwen3.8-max\", the upstream model serving this route, operating inside the Claude Code client."
        ));
    assert!(text.contains(
            "You are an agent for \"qwen3.8-max\", the upstream model serving this route, operating inside the Claude Code client."
        ));
    assert!(text.contains("You are powered by qwen3.8-max."));
    assert!(!text.contains("claude-sonnet-4-5"));
    assert!(text.contains("Co-Authored-By: qwen3.8-max <noreply@anthropic.com>"));
    assert!(!text.contains("Anthropic's official CLI"));
    assert!(text.contains("# Environment"));
}

#[test]
fn neutralizes_reworded_persona_sentences() {
    let mut text = concat!(
        "You are Claude Code, a helpful coding assistant built by Anthropic.\n",
        "You are a Claude agent that helps with programming tasks.\n",
        "Claude Code is available as a CLI in the terminal.\n",
        "Other instructions stay."
    )
    .to_string();

    assert!(neutralize_claude_identity_declaration(
        &mut text,
        "DeepSeek V3"
    ));
    assert_eq!(
            text.matches(
                "You are \"DeepSeek V3\", the upstream model serving this route. You operate inside the Claude Code client. Answer questions about your identity truthfully based on what you actually are."
            )
            .count(),
            2
        );
    // Factual capability sentences must survive the rewrite.
    assert!(text
        .contains("Claude Code is available as a CLI in the terminal.\nOther instructions stay."));
    assert!(!text.contains("You are Claude Code"));
    assert!(!text.contains("You are a Claude agent"));
}

#[test]
fn rewrites_powered_by_line_with_model_names_containing_periods() {
    let mut text = concat!(
            "# Environment\n",
            "You are powered by the model named Claude Sonnet 4.5. The exact model ID is claude-sonnet-4-5-20250929.\n",
            "Today's date is 2026-08-06."
        )
        .to_string();

    assert!(neutralize_powered_by_line(&mut text, "kimi-k3"));
    assert!(text.contains("You are powered by kimi-k3."));
    assert!(!text.contains("Claude Sonnet 4.5"));
    assert!(!text.contains("exact model ID"));
    assert!(text.contains("Today's date is 2026-08-06."));
}

#[test]
fn strips_bracket_suffix_from_model_identity_fallback() {
    let client = Client::builder().build().unwrap();
    let profile = ProviderProfile {
        file_name: "settings - ds4.json".to_string(),
        display_name: "DeepSeek".to_string(),
        source: ProviderProfileSource::Legacy,
        model: "deepseek-v4-pro[1m]".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: "https://example.invalid".to_string(),
        auth_token: None,
        api_key: Some("secret".to_string()),
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::OpenAiChat,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url: "https://example.invalid/v1/chat/completions".to_string(),
        client,
    };

    assert_eq!(
        upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
        Some("deepseek-v4-pro")
    );
}

#[test]
fn passes_identity_questions_through_to_the_upstream_model() {
    // The bridge no longer answers identity questions itself. Every
    // phrasing must reach the upstream model verbatim; only the system
    // prompt is adapted.
    let questions = [
        "who are you?",
        "What model are you?",
        "are you Claude?",
        "你是谁？",
        "你是？",
        "您是？",
        "你是什么模型？",
        "妳系边个？",
    ];
    for question in questions {
        let mut request = json!({
            "system": "You are Claude Code, Anthropic's official CLI for Claude.",
            "messages": [{"role": "user", "content": question}]
        });
        let original_messages = request["messages"].clone();

        append_bridge_identity(&mut request, "qwen3.8-max").unwrap();

        assert_eq!(
            request["messages"], original_messages,
            "user message must pass through unchanged: {question}"
        );
        let system = request["system"].as_str().unwrap();
        assert!(!system.contains("Anthropic's official CLI"));
        assert!(system.contains("\"qwen3.8-max\""));
    }
}

#[test]
fn creates_system_prompt_when_identity_is_the_only_system_instruction() {
    let mut request = json!({"messages": []});

    append_bridge_identity(&mut request, "Kimi K2").unwrap();

    assert!(request["system"]
        .as_str()
        .unwrap()
        .contains("actual model and first-person assistant identity is \"Kimi K2\""));
}

#[test]
fn selects_profile_identity_and_honors_disable_switch() {
    let client = Client::builder().build().unwrap();
    let mut profile = ProviderProfile {
        file_name: "settings - deepseek.json".to_string(),
        display_name: "DeepSeek".to_string(),
        source: ProviderProfileSource::Legacy,
        model: "deepseek-chat".to_string(),
        context_window: None,
        upstream_identity: Some("  DeepSeek\nV3  ".to_string()),
        identity_override: true,
        base_url: "https://example.invalid".to_string(),
        auth_token: Some("secret".to_string()),
        api_key: None,
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::OpenAiChat,
        openai_capabilities: OpenAiCapabilities::default(),
        vision: VisionConfig::default(),
        upstream_url: "https://example.invalid/v1/chat/completions".to_string(),
        client,
    };

    assert_eq!(
        upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
        Some("DeepSeek V3")
    );
    profile.upstream_identity = None;
    profile.local_gemini = true;
    assert_eq!(
        upstream_identity_label(&profile, "gemini-3.6-flash").as_deref(),
        Some("Google Gemini (gemini-3.6-flash)")
    );
    profile.identity_override = false;
    assert_eq!(upstream_identity_label(&profile, "gemini-3.6-flash"), None);
}

fn interactions_test_profile(file_name: &str) -> ProviderProfile {
    let mut capabilities = OpenAiCapabilities::gemini_interactions();
    capabilities.gemini_builtin_tools = vec![
        "google_search".to_string(),
        "url_context".to_string(),
        "code_execution".to_string(),
    ];
    ProviderProfile {
        file_name: file_name.to_string(),
        display_name: "Gemini Interactions".to_string(),
        source: ProviderProfileSource::Native,
        model: "gemini-3.6-flash".to_string(),
        context_window: None,
        upstream_identity: None,
        identity_override: true,
        base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
        auth_token: None,
        api_key: Some("test-key".to_string()),
        proxy_url: None,
        local_gemini: false,
        transport: ProviderTransport::GeminiInteractions,
        openai_capabilities: capabilities,
        vision: VisionConfig::default(),
        upstream_url: "https://generativelanguage.googleapis.com/v1beta/interactions".to_string(),
        client: Client::builder().build().unwrap(),
    }
}

#[test]
fn detects_and_removes_only_mixed_interaction_server_tools() {
    let mut request = json!({
        "tools": [
            {"type": "function", "name": "read_file"},
            {"type": "google_search"},
            {"type": "url_context"}
        ]
    });
    assert!(interaction_request_has_mixed_tools(&request));
    assert!(is_mixed_interaction_tools_error(
            400,
            "Set tool_config.include_server_side_tool_invocations when combining built-in tools and function calling"
        ));
    assert!(!is_mixed_interaction_tools_error(
        500,
        "built-in tool function"
    ));
    assert!(remove_interaction_server_tools(&mut request));
    assert!(!interaction_request_has_mixed_tools(&request));
    assert_eq!(request["tools"].as_array().unwrap().len(), 1);
    assert_eq!(request["tools"][0]["name"], "read_file");
}

#[test]
fn configures_google_maps_and_native_file_search() {
    let raw = json!({
        "capabilities": {
            "gemini_builtin_tools": ["google_maps"],
            "gemini_file_search_store_names": ["fileSearchStores/project-docs"]
        }
    });
    let capabilities = parse_openai_capabilities_with_defaults(
        raw.as_object().unwrap(),
        "gemini-interactions.json",
        OpenAiCapabilities::gemini_interactions(),
    )
    .unwrap();
    assert_eq!(capabilities.gemini_builtin_tools, ["google_maps"]);
    assert_eq!(
        capabilities.gemini_file_search_store_names,
        ["fileSearchStores/project-docs"]
    );
    let tools = translated_interaction_tools(&json!({}), &capabilities);
    assert_eq!(tools[0]["type"], "google_maps");
    assert_eq!(tools[1]["type"], "file_search");
    assert_eq!(
        tools[1]["file_search_store_names"][0],
        "fileSearchStores/project-docs"
    );
}

#[test]
fn maps_structured_output_effort_service_tier_and_document_sources() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let request = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "document", "source": {"type": "url", "url": "https://example.com/report.pdf"}},
                {"type": "document", "source": {"type": "text", "media_type": "text/plain", "data": "plain report"}},
                {"type": "document", "source": {"type": "content", "content": [{"type": "text", "text": "block report"}]}}
            ]
        }],
        "output_config": {
            "effort": "xhigh",
            "format": {
                "type": "json_schema",
                "schema": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"]
                }
            }
        },
        "service_tier": "standard_only"
    });
    let translated = translate_gemini_interactions_request(
        &request,
        &profile,
        &RwLock::new(InteractionContinuationCache::default()),
    )
    .unwrap();
    assert_eq!(translated["generation_config"]["thinking_level"], "high");
    assert_eq!(translated["service_tier"], "standard");
    assert_eq!(
        translated["response_format"]["mime_type"],
        "application/json"
    );
    assert!(translated["response_format"]["schema"]
        .get("$schema")
        .is_none());
    let content = translated["input"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "document");
    assert_eq!(content[0]["uri"], "https://example.com/report.pdf");
    assert_eq!(content[0]["mime_type"], "application/pdf");
    assert_eq!(
        BASE64_STANDARD
            .decode(content[1]["data"].as_str().unwrap())
            .unwrap(),
        b"plain report"
    );
    assert_eq!(
        BASE64_STANDARD
            .decode(content[2]["data"].as_str().unwrap())
            .unwrap(),
        b"block report"
    );
}

#[test]
fn exposes_explicit_diagnostics_for_unmapped_anthropic_fields() {
    let request = json!({
        "metadata": {"user_id": "test"},
        "container": "container-1",
        "temperature": 0.7,
        "service_tier": "auto",
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}}]
        }]
    });
    let diagnostics = gemini_interaction_request_diagnostics(&request).join("\n");
    assert!(diagnostics.contains("metadata"));
    assert!(diagnostics.contains("container"));
    assert!(diagnostics.contains("temperature"));
    assert!(diagnostics.contains("default standard tier"));
    assert!(diagnostics.contains("disable_parallel_tool_use"));
    assert!(diagnostics.contains("cache_control"));
}

#[test]
fn builds_google_native_count_tokens_request_with_prompt_and_tools() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let request = json!({
        "system": "You are a coding agent.",
        "messages": [
            {"role": "user", "content": "Read Cargo.toml"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "call-1", "name": "read_file", "input": {"path": "Cargo.toml"}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call-1", "content": "package data"}]}
        ],
        "tools": [{
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        }],
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}}
    });
    let translated = gemini_count_tokens_request(&request, &profile).unwrap();
    let generate = &translated["generateContentRequest"];
    assert_eq!(generate["model"], "models/gemini-3.6-flash");
    assert_eq!(
        generate["systemInstruction"]["parts"][0]["text"],
        "You are a coding agent."
    );
    assert_eq!(
        generate["contents"][1]["parts"][0]["functionCall"]["name"],
        "read_file"
    );
    assert_eq!(
        generate["contents"][2]["parts"][0]["functionResponse"]["name"],
        "read_file"
    );
    assert_eq!(
        generate["tools"][0]["functionDeclarations"][0]["name"],
        "read_file"
    );
    assert_eq!(
        generate["generationConfig"]["responseMimeType"],
        "application/json"
    );
}

#[tokio::test]
async fn anthropic_count_tokens_uses_kimi_native_estimator() {
    let captured = Arc::new(tokio::sync::Mutex::new(None::<Value>));
    let captured_for_handler = captured.clone();
    let mock = Router::new().route(
        "/v1/tokenizers/estimate-token-count",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = captured_for_handler.clone();
            async move {
                assert_eq!(
                    headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer secret")
                );
                *captured.lock().await = Some(body);
                Json(json!({"data": {"total_tokens": 654}}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = test_provider_profile(
        Client::builder().no_proxy().build().unwrap(),
        format!("http://{address}/anthropic/v1/messages"),
    );
    profile.file_name = "kimi-k3.json".to_string();
    profile.model = "kimi-k3".to_string();
    profile.context_window = Some(1_048_576);
    profile.base_url = format!("http://{address}/anthropic");
    profile.transport = ProviderTransport::Anthropic;
    profile.openai_capabilities.chat_dialect = OpenAiChatDialect::Kimi;
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = Arc::new(AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client: Client::builder().build().unwrap(),
            proxy_url: None,
        })),
        fallback_api_key: Some("local-token".to_string()),
        upstream_url: "https://example.invalid".to_string(),
        model: "bridge-router".to_string(),
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(InteractionContinuationCache::default())),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: vec![profile],
            active_file: "kimi-k3.json".to_string(),
            source: ProviderProfileSource::Native,
        })),
        shutdown_tx,
        settings_dir: PathBuf::new(),
        providers_dir: PathBuf::new(),
        bridge_state_path: PathBuf::new(),
        image_output_dir: env::temp_dir(),
        image_model: DEFAULT_IMAGE_MODEL.to_string(),
        image_upstream_url: DEFAULT_IMAGE_UPSTREAM.to_string(),
        local_bridge_base_url: "http://127.0.0.1:18787".to_string(),
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    });
    let response = anthropic_count_tokens(
        State(state),
        HeaderMap::new(),
        Json(json!({
            "system": "Count this too.",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{
                "name": "read_file",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            }]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-claude-bridge-token-count")
            .unwrap(),
        "kimi-native"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["input_tokens"], 654);
    let captured = captured.lock().await;
    let captured = captured.as_ref().unwrap();
    assert_eq!(captured["model"], "kimi-k3");
    assert!(captured["messages"].is_array());
    assert_eq!(captured["tools"][0]["function"]["name"], "read_file");
    assert!(captured.get("stream").is_none());
    server.abort();
}

#[tokio::test]
async fn anthropic_count_tokens_uses_google_native_endpoint() {
    let captured = Arc::new(tokio::sync::Mutex::new(None::<Value>));
    let captured_for_handler = captured.clone();
    let mock = Router::new().route(
        "/v1beta/models/gemini-3.6-flash:countTokens",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = captured_for_handler.clone();
            async move {
                assert_eq!(
                    headers
                        .get("x-goog-api-key")
                        .and_then(|value| value.to_str().ok()),
                    Some("test-key")
                );
                *captured.lock().await = Some(body);
                Json(json!({"totalTokens": 321}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = interactions_test_profile("gemini-interactions.json");
    profile.base_url = format!("http://{address}/v1beta");
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = Arc::new(AppState {
        gemini_transport: Arc::new(RwLock::new(GeminiTransport {
            client: Client::builder().build().unwrap(),
            proxy_url: None,
        })),
        fallback_api_key: Some("local-token".to_string()),
        upstream_url: "https://example.invalid".to_string(),
        model: "gemini-3.6-flash".to_string(),
        thought_signatures: Arc::new(RwLock::new(IndexMap::new())),
        interaction_continuations: Arc::new(RwLock::new(InteractionContinuationCache::default())),
        vision_cache: Arc::new(tokio::sync::Mutex::new(IndexMap::new())),
        routing: Arc::new(RwLock::new(ProviderRoutingState {
            profiles: vec![profile],
            active_file: "gemini-interactions.json".to_string(),
            source: ProviderProfileSource::Native,
        })),
        shutdown_tx,
        settings_dir: PathBuf::new(),
        providers_dir: PathBuf::new(),
        bridge_state_path: PathBuf::new(),
        image_output_dir: env::temp_dir(),
        image_model: DEFAULT_IMAGE_MODEL.to_string(),
        image_upstream_url: DEFAULT_IMAGE_UPSTREAM.to_string(),
        local_bridge_base_url: "http://127.0.0.1:18787".to_string(),
        admin_state_lock: Arc::new(tokio::sync::Mutex::new(())),
    });
    let response = anthropic_count_tokens(
        State(state),
        HeaderMap::new(),
        Json(json!({
            "system": "Count this too.",
            "messages": [{"role": "user", "content": "hello"}]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-claude-bridge-token-count")
            .unwrap(),
        "google-native"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["input_tokens"], 321);
    assert!(
        captured.lock().await.as_ref().unwrap()["generateContentRequest"]["systemInstruction"]
            ["parts"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Count this too.")
    );
    server.abort();
}

#[test]
fn returns_bounded_gemini_server_tool_metadata_and_standard_usage_counts() {
    let upstream = json!({
        "id": "interaction-server-tools-1",
        "status": "completed",
        "steps": [
            {"type": "google_search_call", "id": "search-1", "arguments": {"query": "Rust"}},
            {"type": "google_search_result", "call_id": "search-1", "result": [{"search_suggestions": ["Rust language"]}]},
            {"type": "url_context_call", "id": "fetch-1", "arguments": {"urls": ["https://example.com"]}},
            {"type": "url_context_result", "call_id": "fetch-1", "result": [{"url": "https://example.com", "status": "success"}]},
            {"type": "model_output", "content": [{"type": "text", "text": "Done."}]}
        ],
        "usage": {"total_input_tokens": 12, "total_output_tokens": 4}
    });
    let translated = translate_gemini_interactions_response(&upstream, "gemini-3.6-flash").unwrap();
    assert_eq!(translated.message["content"][0]["text"], "Done.");
    assert_eq!(
        translated.message["usage"]["server_tool_use"]["web_search_requests"],
        1
    );
    assert_eq!(
        translated.message["usage"]["server_tool_use"]["web_fetch_requests"],
        1
    );
    assert_eq!(
        translated.message["provider_metadata"]["google"]["interaction_server_tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn retries_known_mixed_tools_rejection_with_function_tools_only() {
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    let mock = Router::new().route(
            "/interactions",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    let mut requests = captured.lock().await;
                    requests.push(body);
                    if requests.len() == 1 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error": {"message": "tool_config.include_server_side_tool_invocations is required when combining built-in tools and function calling"}})),
                        )
                            .into_response();
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "id": "interaction-retry-1",
                            "status": "completed",
                            "steps": [{"type": "model_output", "content": [{"type": "text", "text": "Recovered."}]}],
                            "usage": {"total_input_tokens": 5, "total_output_tokens": 2}
                        })),
                    )
                        .into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = interactions_test_profile("gemini-interactions.json");
    profile.upstream_url = format!("http://{address}/interactions");
    let response = forward_gemini_interactions_profile(
        profile,
        json!({
            "stream": false,
            "messages": [{"role": "user", "content": "Read Cargo.toml"}],
            "tools": [{
                "name": "read_file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            }]
        }),
        Arc::new(RwLock::new(InteractionContinuationCache::default())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 4);
    assert_eq!(requests[1]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(requests[1]["tools"][0]["type"], "function");
    server.abort();
}

#[tokio::test]
async fn retries_unimplemented_interaction_continuation_with_full_history() {
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    let mock = Router::new().route(
            "/interactions",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    let mut requests = captured.lock().await;
                    requests.push(body);
                    if requests.len() == 1 {
                        return (
                            StatusCode::NOT_IMPLEMENTED,
                            Json(json!({"error": {"message": "previous_interaction_id is not implemented for this interaction"}})),
                        )
                            .into_response();
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "id": "interaction-recovered-2",
                            "status": "completed",
                            "steps": [{"type": "model_output", "content": [{"type": "text", "text": "Recovered history."}]}],
                            "usage": {"total_input_tokens": 9, "total_output_tokens": 3}
                        })),
                    )
                        .into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = interactions_test_profile("gemini-interactions.json");
    profile.openai_capabilities.gemini_builtin_tools.clear();
    profile.upstream_url = format!("http://{address}/interactions");
    let continuations = Arc::new(RwLock::new(InteractionContinuationCache::default()));
    let first = json!({"messages": [{"role": "user", "content": "Remember alpha."}]});
    let assistant_content = vec![json!({"type": "text", "text": "Stored alpha."})];
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &first,
        "interaction-prior-1",
        &assistant_content,
        &[],
    );
    let request = json!({
        "stream": false,
        "messages": [
            {"role": "user", "content": "Remember alpha."},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": "What did I ask you to remember?"}
        ]
    });
    let response = forward_gemini_interactions_profile(profile, request, continuations).await;
    assert_eq!(response.status(), StatusCode::OK);
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["previous_interaction_id"],
        "interaction-prior-1"
    );
    assert!(requests[1].get("previous_interaction_id").is_none());
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 3);
    server.abort();
}

#[test]
fn builds_stateful_interactions_delta_only_after_exact_transcript_match() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let first = json!({
        "system": "You are a coding assistant.",
        "max_tokens": 4096,
        "stream": true,
        "messages": [{"role": "user", "content": "Remember alpha."}],
        "tools": [{
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        }]
    });
    let initial = translate_gemini_interactions_request(&first, &profile, &continuations).unwrap();
    assert_eq!(initial["store"], true);
    assert!(initial.get("previous_interaction_id").is_none());
    assert_eq!(initial["generation_config"]["thinking_level"], "high");
    assert_eq!(initial["generation_config"]["thinking_summaries"], "auto");
    assert_eq!(initial["tools"].as_array().unwrap().len(), 4);
    assert_eq!(initial["input"][0]["type"], "user_input");

    let assistant_content = vec![json!({"type": "text", "text": "Stored alpha."})];
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &first,
        "interaction-1",
        &assistant_content,
        &[],
    );
    let second = json!({
        "system": "You are a coding assistant.",
        "max_tokens": 4096,
        "stream": true,
        "messages": [
            {"role": "user", "content": "Remember alpha."},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": "What did I ask you to remember?"}
        ]
    });
    let continued =
        translate_gemini_interactions_request(&second, &profile, &continuations).unwrap();
    assert_eq!(continued["previous_interaction_id"], "interaction-1");
    assert_eq!(continued["input"].as_array().unwrap().len(), 1);
    assert_eq!(
        continued["input"][0]["content"][0]["text"],
        "What did I ask you to remember?"
    );
    assert!(continued.get("system_instruction").is_none());

    let recovered = translate_gemini_interactions_request_with_continuation(
        &second,
        &profile,
        &continuations,
        false,
    )
    .unwrap();
    assert!(recovered.get("previous_interaction_id").is_none());
    assert_eq!(recovered["input"].as_array().unwrap().len(), 3);

    let mut edited = second.clone();
    edited["messages"][1]["content"][0]["text"] = json!("Edited history.");
    let fallback =
        translate_gemini_interactions_request(&edited, &profile, &continuations).unwrap();
    assert!(fallback.get("previous_interaction_id").is_none());
    assert_eq!(fallback["input"].as_array().unwrap().len(), 3);
}

#[test]
fn continues_interactions_tool_result_by_opaque_call_id() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let first = json!({
        "messages": [{"role": "user", "content": "Read Cargo.toml"}]
    });
    let assistant_content = vec![json!({
        "type": "tool_use",
        "id": "call-opaque-1",
        "name": "read_file",
        "input": {"path": "Cargo.toml"}
    })];
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &first,
        "interaction-tool-1",
        &assistant_content,
        &[("call-opaque-1".to_string(), "read_file".to_string())],
    );
    let result_request = json!({
        "messages": [
            {"role": "user", "content": "Read Cargo.toml"},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "call-opaque-1",
                "content": "package data"
            }]}
        ]
    });
    let translated =
        translate_gemini_interactions_request(&result_request, &profile, &continuations).unwrap();
    assert_eq!(translated["previous_interaction_id"], "interaction-tool-1");
    assert_eq!(translated["input"][0]["type"], "function_result");
    assert_eq!(translated["input"][0]["call_id"], "call-opaque-1");
    assert_eq!(translated["input"][0]["name"], "read_file");
    assert_eq!(translated["input"][0]["result"], "package data");
}

#[test]
fn continues_interactions_tool_result_from_matching_transcript() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let first = json!({
        "messages": [{"role": "user", "content": "Read Cargo.toml"}]
    });
    let assistant_content = vec![json!({
        "type": "tool_use",
        "id": "call-transcript-1",
        "name": "read_file",
        "input": {"path": "Cargo.toml"}
    })];
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &first,
        "interaction-transcript-1",
        &assistant_content,
        &[],
    );
    let result_request = json!({
        "messages": [
            {"role": "user", "content": "Read Cargo.toml"},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "call-transcript-1",
                "content": "package data"
            }]}
        ]
    });
    let translated =
        translate_gemini_interactions_request(&result_request, &profile, &continuations).unwrap();
    assert_eq!(
        translated["previous_interaction_id"],
        "interaction-transcript-1"
    );
    assert_eq!(translated["input"][0]["type"], "function_result");
    assert_eq!(translated["input"][0]["name"], "read_file");
}

#[test]
fn textifies_interactions_tool_history_when_continuation_is_unavailable() {
    let profile = interactions_test_profile("gemini-interactions.json");
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let request = json!({
        "messages": [
            {"role": "user", "content": "Read Cargo.toml"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "private summary", "signature": "sig"},
                {
                    "type": "tool_use",
                    "id": "call-missing-1",
                    "name": "read_file",
                    "input": {"path": "Cargo.toml"}
                }
            ]},
            {"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "call-missing-1",
                "content": "package data"
            }]},
            {"role": "assistant", "content": [{
                "type": "text",
                "text": "[Tool call: read_file (call ID: legacy)]\nArguments: {\"path\":\"Cargo.toml\"}"
            }]},
            {"role": "user", "content": "Continue with a real tool if needed."}
        ]
    });
    let translated =
        translate_gemini_interactions_request(&request, &profile, &continuations).unwrap();
    assert!(translated.get("previous_interaction_id").is_none());
    let input = translated["input"].as_array().unwrap();
    assert!(input.iter().all(|step| {
        !matches!(
            step.get("type").and_then(Value::as_str),
            Some("function_call" | "function_result" | "thought")
        )
    }));
    let replay = serde_json::to_string(input).unwrap();
    assert!(!replay.contains("Tool call"));
    assert!(replay.contains("An earlier read_file operation produced this result"));
    assert!(replay.contains("package data"));
    assert!(!replay.contains("private summary"));
    assert!(translated["system_instruction"]
        .as_str()
        .unwrap()
        .contains("invoke one of the provided function tools"));
}

#[test]
fn translates_interactions_stream_and_remembers_tool_continuation() {
    let continuations = Arc::new(RwLock::new(InteractionContinuationCache::default()));
    let request = json!({
        "messages": [{"role": "user", "content": "Use lookup"}],
        "stream": true
    });
    let mut translator = GeminiInteractionsStreamTranslator::new(
        "gemini-3.6-flash".to_string(),
        "gemini-interactions.json".to_string(),
        request,
        continuations.clone(),
        10,
    );
    assert_eq!(translator.start_events().unwrap().len(), 1);
    translator
            .process_payload(r#"{"event_type":"interaction.created","interaction":{"id":"interaction-stream-1","status":"in_progress"}}"#)
            .unwrap();
    translator
        .process_payload(r#"{"event_type":"step.start","index":0,"step":{"type":"thought"}}"#)
        .unwrap();
    translator
            .process_payload(r#"{"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"Checking."}}}"#)
            .unwrap();
    translator
            .process_payload(r#"{"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"sig-1"}}"#)
            .unwrap();
    translator
        .process_payload(r#"{"event_type":"step.stop","index":0}"#)
        .unwrap();
    translator
            .process_payload(r#"{"event_type":"step.start","index":1,"step":{"type":"function_call","id":"call-stream-1","name":"lookup","arguments":{}}}"#)
            .unwrap();
    translator
            .process_payload(r#"{"event_type":"step.delta","index":1,"delta":{"type":"arguments_delta","arguments":"{\"key\":\"alpha\"}"}}"#)
            .unwrap();
    translator
        .process_payload(r#"{"event_type":"step.stop","index":1}"#)
        .unwrap();
    {
        let cache = continuations.read().unwrap();
        let call = cache
            .calls
            .get("gemini-interactions.json\0call-stream-1")
            .unwrap();
        assert_eq!(call.interaction_id, "interaction-stream-1");
        assert_eq!(call.name, "lookup");
    }
    translator
            .process_payload(r#"{"event_type":"interaction.completed","interaction":{"id":"interaction-stream-1","status":"requires_action","usage":{"total_input_tokens":10,"total_output_tokens":5}}}"#)
            .unwrap();
    assert_eq!(translator.finish().unwrap().len(), 2);
    assert_eq!(translator.assistant_content[0]["type"], "thinking");
    assert_eq!(translator.assistant_content[0]["signature"], "sig-1");
    assert_eq!(translator.assistant_content[1]["type"], "tool_use");
    assert_eq!(translator.assistant_content[1]["input"]["key"], "alpha");
    let cache = continuations.read().unwrap();
    let call = cache
        .calls
        .get("gemini-interactions.json\0call-stream-1")
        .unwrap();
    assert_eq!(call.interaction_id, "interaction-stream-1");
    assert_eq!(call.name, "lookup");
}

fn responses_test_profile(file_name: &str, base_url: &str, model: &str) -> ProviderProfile {
    let mut profile = test_provider_profile(
        Client::builder().build().unwrap(),
        openai_responses_endpoint(base_url),
    );
    profile.file_name = file_name.to_string();
    profile.base_url = base_url.to_string();
    profile.model = model.to_string();
    profile.transport = ProviderTransport::OpenAiResponses;
    profile.openai_capabilities = OpenAiCapabilities::for_responses_base_url(base_url);
    profile
}

#[test]
fn deepseek_responses_fixture_preserves_reasoning_custom_tools_and_stateless_history() {
    let mut profile = responses_test_profile(
        "deepseek-responses.json",
        "https://api.deepseek.com/v1",
        "deepseek-v4-flash",
    );
    profile.openai_capabilities.responses_builtin_tools = vec!["web_search".to_string()];
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let request = json!({
        "system": "You are a coding agent.",
        "messages": [
            {"role": "user", "content": "Patch the project"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "I need to apply a focused patch."},
                {"type": "text", "text": "I will apply the focused change."},
                {"type": "tool_use", "id": "call_ds_patch", "name": "apply_patch", "input": {"patch": "*** Begin Patch\n*** End Patch"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_ds_patch", "content": "Done!"}
            ]}
        ],
        "thinking": {"type": "enabled"},
        "output_config": {"effort": "max"},
        "tools": [{
            "name": "apply_patch",
            "description": "Apply a patch",
            "input_schema": {"type": "object", "properties": {"patch": {"type": "string"}}}
        }]
    });

    let translated = translate_anthropic_to_responses(&request, &profile, &continuations).unwrap();
    assert_eq!(translated["store"], false);
    assert!(translated.get("previous_response_id").is_none());
    assert_eq!(translated["reasoning"]["effort"], "max");
    assert!(translated["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "reasoning"));
    let input = translated["input"].as_array().unwrap();
    let reasoning_index = input
        .iter()
        .position(|item| item["type"] == "reasoning")
        .unwrap();
    let visible_text_index = input
        .iter()
        .position(|item| item["role"] == "assistant")
        .unwrap();
    let custom_call_index = input
        .iter()
        .position(|item| item["type"] == "custom_tool_call")
        .unwrap();
    assert!(reasoning_index < visible_text_index && visible_text_index < custom_call_index);
    assert!(translated["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "custom_tool_call_output"));
    assert!(translated["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["type"] == "custom" && tool["name"] == "apply_patch"));
    assert!(translated["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["type"] == "web_search"));
}

#[test]
fn qwen_responses_fixture_uses_exact_stateful_tool_continuation() {
    let profile = responses_test_profile(
        "qwen-responses.json",
        "https://workspace.cn-beijing.maas.aliyuncs.com/v1",
        "qwen3.8-max",
    );
    assert!(profile.openai_capabilities.responses_stateful);
    assert!(profile.openai_capabilities.responses_session_cache);
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let first_request = json!({
        "system": "You are a coding agent.",
        "messages": [{"role": "user", "content": "Read Cargo.toml"}]
    });
    let assistant_content = vec![
        json!({"type": "thinking", "thinking": "I should inspect the file."}),
        json!({"type": "tool_use", "id": "call_qwen_read", "name": "read_file", "input": {"path": "Cargo.toml"}}),
    ];
    remember_interaction_continuation(
        &continuations,
        &profile.file_name,
        &first_request,
        "resp_qwen_1",
        &assistant_content,
        &[("call_qwen_read".to_string(), "read_file".to_string())],
    );
    let request = json!({
        "system": "You are a coding agent.",
        "messages": [
            {"role": "user", "content": "Read Cargo.toml"},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_qwen_read", "content": "[package]"}]}
        ]
    });

    let translated = translate_anthropic_to_responses(&request, &profile, &continuations).unwrap();
    assert_eq!(translated["store"], true);
    assert_eq!(translated["previous_response_id"], "resp_qwen_1");
    assert!(translated.get("instructions").is_none());
    assert_eq!(translated["input"].as_array().unwrap().len(), 1);
    assert_eq!(translated["input"][0]["type"], "function_call_output");
    assert_eq!(translated["input"][0]["call_id"], "call_qwen_read");
}

#[test]
fn qwen_responses_preserves_native_effort_levels_and_uses_safe_default() {
    let profile = responses_test_profile(
        "qwen-responses.json",
        "https://workspace.cn-beijing.maas.aliyuncs.com/v1",
        "qwen3.8-max",
    );
    let continuations = RwLock::new(InteractionContinuationCache::default());
    let translate = |request: Value| {
        translate_anthropic_to_responses(&request, &profile, &continuations).unwrap()
    };

    let explicit_high = translate(json!({
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "output_config": {"effort": "high"}
    }));
    assert_eq!(explicit_high["reasoning"]["effort"], "high");

    // 31,999 is Claude Code's strongest thinking budget and must reach
    // Qwen's xhigh tier on every transport, not stop one level below.
    let budget_only = translate(json!({
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "thinking": {"type": "enabled", "budget_tokens": 31_999}
    }));
    assert_eq!(budget_only["reasoning"]["effort"], "xhigh");

    let large_but_not_maximum = translate(json!({
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "thinking": {"type": "enabled", "budget_tokens": 16_000}
    }));
    assert_eq!(large_but_not_maximum["reasoning"]["effort"], "high");

    let default = translate(json!({
        "messages": [{"role": "user", "content": "Inspect one file"}]
    }));
    assert_eq!(default["reasoning"]["effort"], "medium");

    let disabled = translate(json!({
        "messages": [{"role": "user", "content": "Answer directly"}],
        "thinking": {"type": "disabled"},
        "output_config": {"effort": "max"}
    }));
    assert_eq!(disabled["reasoning"]["effort"], "none");
}

#[test]
fn responses_fixture_maps_semantic_output_server_tools_and_detailed_usage() {
    let upstream = json!({
        "id": "resp_ds_2",
        "status": "completed",
        "output": [
            {"type": "reasoning", "id": "rs_1", "content": [{"type": "reasoning_text", "text": "I should search first."}]},
            {"type": "web_search_call", "id": "ws_1", "status": "completed", "arguments": {"query": "DeepSeek API"}},
            {"type": "message", "id": "msg_1", "content": [{"type": "output_text", "text": "Found it."}]},
            {"type": "function_call", "call_id": "call_1", "name": "read_file", "arguments": "{\"path\":\"Cargo.toml\"}"}
        ],
        "usage": {
            "input_tokens": 100,
            "input_tokens_details": {"cached_tokens": 40},
            "output_tokens": 20,
            "output_tokens_details": {"reasoning_tokens": 12}
        }
    });
    let translated =
        translate_openai_responses_response(&upstream, "deepseek-v4-flash", 1).unwrap();
    assert_eq!(translated.message["content"][0]["type"], "thinking");
    assert_eq!(translated.message["content"][1]["text"], "Found it.");
    assert_eq!(
        translated.message["content"][2]["input"]["path"],
        "Cargo.toml"
    );
    assert_eq!(translated.message["stop_reason"], "tool_use");
    assert_eq!(translated.message["usage"]["cache_read_input_tokens"], 40);
    assert_eq!(translated.message["usage"]["reasoning_tokens"], 12);
    assert_eq!(
        translated.message["usage"]["server_tool_use"]["web_search_requests"],
        1
    );
    assert_eq!(
        translated.message["provider_metadata"]["openai"]["responses_server_tools"][0]["type"],
        "web_search_call"
    );
}

#[test]
fn chat_usage_fixture_maps_cache_creation_cache_read_and_reasoning_tokens() {
    let upstream = json!({
        "choices": [{
            "message": {"content": "Done."},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 90,
            "prompt_tokens_details": {"cached_tokens": 30},
            "cache_creation_input_tokens": 10,
            "completion_tokens": 18,
            "completion_tokens_details": {"reasoning_tokens": 11}
        }
    });
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_response_with_capabilities(
        &upstream,
        "qwen3.8-max",
        &signatures,
        &OpenAiCapabilities::default(),
    )
    .unwrap();
    assert_eq!(translated["usage"]["input_tokens"], 90);
    assert_eq!(translated["usage"]["output_tokens"], 18);
    assert_eq!(translated["usage"]["cache_read_input_tokens"], 30);
    assert_eq!(translated["usage"]["cache_creation_input_tokens"], 10);
    assert_eq!(translated["usage"]["reasoning_tokens"], 11);
}

#[test]
fn kimi_usage_fixtures_map_top_level_cached_tokens_for_unary_and_streaming() {
    let capabilities = OpenAiCapabilities::for_openai_base_url("https://api.moonshot.ai/v1");
    let upstream = json!({
        "choices": [{
            "message": {"reasoning_content": "Check the result.", "content": "Done."},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 20,
            "total_tokens": 140,
            "cached_tokens": 72
        }
    });
    let signatures = RwLock::new(IndexMap::new());
    let translated = translate_anthropic_response_with_capabilities(
        &upstream,
        "kimi-k3",
        &signatures,
        &capabilities,
    )
    .unwrap();
    assert_eq!(translated["usage"]["cache_read_input_tokens"], 72);
    assert_eq!(translated["content"][0]["type"], "thinking");

    let signatures = Arc::new(RwLock::new(IndexMap::new()));
    let mut stream = AnthropicStreamTranslator::with_capabilities(
        "kimi-k3".to_string(),
        signatures,
        1,
        capabilities,
    );
    stream.start_events().unwrap();
    stream
        .process_payload(
            r#"{"choices":[{"delta":{"reasoning_content":"Think."},"finish_reason":null}]}"#,
        )
        .unwrap();
    stream
            .process_payload(
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":20,"cached_tokens":72}}"#,
            )
            .unwrap();
    assert_eq!(stream.cache_read_input_tokens, Some(72));
    assert_eq!(stream.input_tokens, 120);
    assert_eq!(stream.output_tokens, 20);
    stream.finish().unwrap();
}

#[test]
fn provider_diagnostics_expose_chat_and_responses_downgrades() {
    let request = json!({
        "metadata": {"user_id": "test"},
        "service_tier": "auto",
        "thinking": {"type": "enabled", "budget_tokens": 12000},
        "tool_choice": {"type": "auto"},
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}},
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}}]},
            {"role": "assistant", "content": "Hello."},
            {"role": "user", "content": "Continue."}
        ]
    });
    let deepseek = OpenAiCapabilities::for_openai_base_url("https://api.deepseek.com/v1");
    let chat =
        openai_request_diagnostics(&request, &deepseek, ProviderTransport::OpenAiChat).join("\n");
    assert!(chat.contains("metadata"));
    assert!(chat.contains("service_tier"));
    assert!(chat.contains("cache_control"));
    assert!(chat.contains("Suppressed Anthropic tool_choice"));
    assert!(chat.contains("Downgraded Anthropic json_schema"));

    let mut deepseek_fast_request = request.clone();
    deepseek_fast_request["output_config"]["effort"] = json!("low");
    let deepseek_fast = openai_request_diagnostics(
        &deepseek_fast_request,
        &deepseek,
        ProviderTransport::OpenAiChat,
    )
    .join("\n");
    assert!(deepseek_fast.contains("thinking.type=disabled"));
    assert!(!deepseek_fast.contains("Suppressed Anthropic tool_choice"));

    let mut kimi_request = request.clone();
    kimi_request["thinking"]["type"] = json!("disabled");
    kimi_request["output_config"]["effort"] = json!("medium");
    let kimi = OpenAiCapabilities::for_openai_base_url("https://api.moonshot.ai/v1");
    let kimi =
        openai_request_diagnostics(&kimi_request, &kimi, ProviderTransport::OpenAiChat).join("\n");
    assert!(kimi.contains("always reasons"));
    assert!(kimi.contains("reasoning_effort 'high'"));

    let responses = OpenAiCapabilities::for_responses_base_url("https://api.deepseek.com/v1");
    let responses =
        openai_request_diagnostics(&request, &responses, ProviderTransport::OpenAiResponses)
            .join("\n");
    assert!(responses.contains("thinking budget"));
    assert!(responses.contains("stateless"));
}

#[test]
fn responses_stream_fixture_covers_thinking_tool_call_and_continuation_state() {
    let continuations = Arc::new(RwLock::new(InteractionContinuationCache::default()));
    let request = json!({
        "messages": [{"role": "user", "content": "Inspect Cargo.toml"}],
        "stream": true
    });
    let mut translator = OpenAiResponsesStreamTranslator::new(
        "qwen3.8-max".to_string(),
        "qwen-responses.json".to_string(),
        request,
        continuations.clone(),
        10,
    );
    assert_eq!(translator.start_events().unwrap().len(), 1);
    translator.process_payload(r#"{"type":"response.created","response":{"id":"resp_stream_1","status":"in_progress"}}"#).unwrap();
    translator.process_payload(r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#).unwrap();
    translator.process_payload(r#"{"type":"response.reasoning_text.delta","output_index":0,"delta":"I should inspect it."}"#).unwrap();
    translator.process_payload(r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#).unwrap();
    translator.process_payload(r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_stream_1","name":"read_file"}}"#).unwrap();
    translator.process_payload(r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"Cargo.toml\"}"}"#).unwrap();
    translator.process_payload(r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_stream_1","name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#).unwrap();
    translator.process_payload(r#"{"type":"response.completed","response":{"id":"resp_stream_1","status":"completed","usage":{"input_tokens":10,"output_tokens":5,"output_tokens_details":{"reasoning_tokens":3}}}}"#).unwrap();
    assert_eq!(translator.finish().unwrap().len(), 2);
    assert_eq!(translator.assistant_content[0]["type"], "thinking");
    assert_eq!(translator.assistant_content[1]["type"], "tool_use");
    assert_eq!(
        translator.assistant_content[1]["input"]["path"],
        "Cargo.toml"
    );
    let cache = continuations.read().unwrap();
    let call = cache
        .calls
        .get("qwen-responses.json\0call_stream_1")
        .unwrap();
    assert_eq!(call.interaction_id, "resp_stream_1");
}

#[tokio::test]
async fn qwen_responses_forward_enables_official_session_cache_header() {
    let captured = Arc::new(tokio::sync::Mutex::new(None::<HeaderMap>));
    let captured_for_handler = captured.clone();
    let mock = Router::new().route(
            "/v1/responses",
            post(move |headers: HeaderMap, Json(_body): Json<Value>| {
                let captured = captured_for_handler.clone();
                async move {
                    *captured.lock().await = Some(headers);
                    Json(json!({
                        "id": "resp_qwen_header",
                        "status": "completed",
                        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
                        "usage": {"input_tokens": 2, "output_tokens": 1}
                    }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let mut profile = responses_test_profile(
        "qwen-responses.json",
        "https://workspace.cn-beijing.maas.aliyuncs.com/v1",
        "qwen3.8-max",
    );
    profile.upstream_url = format!("http://{address}/v1/responses");
    profile.client = Client::builder().no_proxy().build().unwrap();
    let response = forward_openai_responses_profile(
        profile,
        json!({"messages": [{"role": "user", "content": "hello"}]}),
        Arc::new(RwLock::new(InteractionContinuationCache::default())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = captured.lock().await.clone().unwrap();
    assert_eq!(headers["x-dashscope-session-cache"], "enable");
    assert_eq!(headers["authorization"], "Bearer secret");
    server.abort();
}
