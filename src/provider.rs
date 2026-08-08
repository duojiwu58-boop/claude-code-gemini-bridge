fn is_provider_profile_file_name(file_name: &str) -> bool {
    let file_name = file_name.to_ascii_lowercase();
    file_name.starts_with("settings - ") && file_name.ends_with(".json")
}

fn is_native_provider_file_name(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().ends_with(".json")
        && !file_name.to_ascii_lowercase().ends_with(".example.json")
}

fn load_provider_profiles(
    providers_dir: &Path,
    legacy_settings_dir: &Path,
    local_bridge_base_url: &str,
) -> Result<LoadedProviderProfiles, String> {
    let native_paths = provider_profile_paths(providers_dir, is_native_provider_file_name)?;
    if !native_paths.is_empty() {
        let mut profiles = load_native_provider_profiles(native_paths, local_bridge_base_url)?;
        let mut legacy_profiles =
            load_legacy_provider_profiles(legacy_settings_dir, local_bridge_base_url)?;
        legacy_profiles.retain(|legacy| {
            !profiles.iter().any(|native| {
                native.file_name.eq_ignore_ascii_case(&legacy.file_name)
                    || (native.model.eq_ignore_ascii_case(&legacy.model)
                        && normalize_base_url(&native.base_url)
                            == normalize_base_url(&legacy.base_url))
            })
        });
        let source = if legacy_profiles.is_empty() {
            ProviderProfileSource::Native
        } else {
            ProviderProfileSource::Mixed
        };
        profiles.extend(legacy_profiles);
        validate_vision_profiles(&profiles)?;
        return Ok(LoadedProviderProfiles { profiles, source });
    }

    let profiles = load_legacy_provider_profiles(legacy_settings_dir, local_bridge_base_url)?;
    validate_vision_profiles(&profiles)?;
    Ok(LoadedProviderProfiles {
        profiles,
        source: ProviderProfileSource::Legacy,
    })
}

fn provider_profile_paths(
    directory: &Path,
    predicate: fn(&str) -> bool,
) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(directory)
        .map_err(|err| format!("Cannot read '{}': {err}", directory.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if predicate(file_name) {
            paths.push(path);
        }
    }
    paths.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(paths)
}

fn read_profile_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("Cannot read '{}': {err}", path.display()))?;
    let json_text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(json_text)
        .map_err(|err| format!("Invalid JSON in '{}': {err}", path.display()))
}

fn profile_string(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        object
            .get(*name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn profile_u64(
    object: &Map<String, Value>,
    names: &[&str],
    file_name: &str,
) -> Result<Option<u64>, String> {
    for name in names {
        if let Some(value) = object.get(*name) {
            return value
                .as_u64()
                .filter(|value| *value > 0)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "Provider profile '{file_name}' field '{name}' must be a positive integer"
                    )
                });
        }
    }
    Ok(None)
}

fn parse_vision_config(
    profile: &Map<String, Value>,
    file_name: &str,
) -> Result<VisionConfig, String> {
    let Some(value) = profile.get("vision") else {
        return Ok(VisionConfig::default());
    };
    let object = value.as_object().ok_or_else(|| {
        format!("Provider profile '{file_name}' field 'vision' must be a JSON object")
    })?;
    let configured_mode = match object.get("mode") {
        None => "native",
        Some(Value::String(mode)) if !mode.trim().is_empty() => mode.trim(),
        Some(_) => {
            return Err(format!(
                "Provider profile '{file_name}' field 'vision.mode' must be a non-empty string"
            ))
        }
    };
    let mode = match configured_mode {
        "native" => VisionMode::Native,
        "proxy" => VisionMode::Proxy,
        other => {
            return Err(format!(
                "Provider profile '{file_name}' vision mode '{other}' is unsupported (expected native or proxy)"
            ))
        }
    };
    let source_profile = profile_string(object, &["profile"]);
    if mode == VisionMode::Native && source_profile.is_some() {
        return Err(format!(
            "Provider profile '{file_name}' cannot set vision.profile when vision.mode is native"
        ));
    }
    if object.get("profile").is_some() && source_profile.is_none() {
        return Err(format!(
            "Provider profile '{file_name}' field 'vision.profile' must be a non-empty string"
        ));
    }
    Ok(VisionConfig {
        mode,
        profile: source_profile,
    })
}

fn resolve_vision_provider(
    profiles: &[ProviderProfile],
    target: &ProviderProfile,
) -> Result<Option<ProviderProfile>, String> {
    if target.vision.mode == VisionMode::Native {
        return Ok(None);
    }
    let source = if let Some(file_name) = target.vision.profile.as_deref() {
        profiles
            .iter()
            .find(|profile| profile.file_name.eq_ignore_ascii_case(file_name))
    } else {
        profiles
            .iter()
            .find(|profile| default_gemini_vision_profile(profile, target, true))
            .or_else(|| {
                profiles
                    .iter()
                    .find(|profile| default_gemini_vision_profile(profile, target, false))
            })
    }
    .ok_or_else(|| {
        target.vision.profile.as_ref().map_or_else(
            || format!(
                "Provider profile '{}' enables vision proxy but no native local or official Google Gemini profile is available",
                target.file_name
            ),
            |file_name| format!(
                "Provider profile '{}' references missing vision profile '{file_name}'",
                target.file_name
            ),
        )
    })?;
    if source.file_name.eq_ignore_ascii_case(&target.file_name) {
        return Err(format!(
            "Provider profile '{}' cannot use itself as its vision provider",
            target.file_name
        ));
    }
    if source.vision.mode != VisionMode::Native {
        return Err(format!(
            "Vision provider '{}' must use vision.mode 'native'; proxy chains are not supported",
            source.file_name
        ));
    }
    Ok(Some(source.clone()))
}

fn default_gemini_vision_profile(
    candidate: &ProviderProfile,
    target: &ProviderProfile,
    require_local: bool,
) -> bool {
    if candidate.vision.mode != VisionMode::Native
        || candidate.file_name.eq_ignore_ascii_case(&target.file_name)
    {
        return false;
    }
    if require_local {
        return candidate.local_gemini;
    }
    !candidate.local_gemini
        && url::Url::parse(&candidate.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| host.eq_ignore_ascii_case("generativelanguage.googleapis.com"))
}

fn validate_vision_profiles(profiles: &[ProviderProfile]) -> Result<(), String> {
    for profile in profiles {
        resolve_vision_provider(profiles, profile)?;
    }
    Ok(())
}

fn capability_bool(
    object: &Map<String, Value>,
    names: &[&str],
    default: bool,
    file_name: &str,
) -> Result<bool, String> {
    for name in names {
        if let Some(value) = object.get(*name) {
            return value.as_bool().ok_or_else(|| {
                format!("Provider profile '{file_name}' capability '{name}' must be a boolean")
            });
        }
    }
    Ok(default)
}

fn capability_string(
    object: &Map<String, Value>,
    names: &[&str],
    file_name: &str,
) -> Result<Option<String>, String> {
    for name in names {
        if let Some(value) = object.get(*name) {
            return value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "Provider profile '{file_name}' capability '{name}' must be a non-empty string"
                    )
                });
        }
    }
    Ok(None)
}

fn capability_string_array(
    object: &Map<String, Value>,
    names: &[&str],
    default: Vec<String>,
    file_name: &str,
) -> Result<Vec<String>, String> {
    for name in names {
        let Some(value) = object.get(*name) else {
            continue;
        };
        let values = value.as_array().ok_or_else(|| {
            format!("Provider profile '{file_name}' capability '{name}' must be an array")
        })?;
        return values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        format!(
                            "Provider profile '{file_name}' capability '{name}' must contain only non-empty strings"
                        )
                    })
            })
            .collect();
    }
    Ok(default)
}

#[cfg(test)]
fn parse_openai_capabilities(
    profile: &Map<String, Value>,
    file_name: &str,
) -> Result<OpenAiCapabilities, String> {
    parse_openai_capabilities_with_defaults(profile, file_name, OpenAiCapabilities::default())
}

fn parse_openai_capabilities_with_defaults(
    profile: &Map<String, Value>,
    file_name: &str,
    defaults: OpenAiCapabilities,
) -> Result<OpenAiCapabilities, String> {
    let Some(value) = profile.get("capabilities") else {
        return Ok(defaults);
    };
    let object = value.as_object().ok_or_else(|| {
        format!("Provider profile '{file_name}' field 'capabilities' must be a JSON object")
    })?;

    let reasoning_fields = match object
        .get("reasoning_fields")
        .or_else(|| object.get("reasoningFields"))
    {
        None => defaults.reasoning_fields,
        Some(Value::String(field)) if !field.trim().is_empty() => {
            vec![field.trim().to_string()]
        }
        Some(Value::Array(fields)) => fields
            .iter()
            .map(|field| {
                field
                    .as_str()
                    .map(str::trim)
                    .filter(|field| !field.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        format!(
                            "Provider profile '{file_name}' capability 'reasoning_fields' must contain only non-empty strings"
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(format!(
                "Provider profile '{file_name}' capability 'reasoning_fields' must be a string or an array of strings"
            ))
        }
    };

    let mut default_reasoning_effort = capability_string(
        object,
        &["default_reasoning_effort", "defaultReasoningEffort"],
        file_name,
    )?
    .or_else(|| defaults.default_reasoning_effort.clone());
    if default_reasoning_effort.as_deref().is_some_and(|effort| {
        !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        )
    }) {
        return Err(format!(
            "Provider profile '{file_name}' capability 'default_reasoning_effort' must be none, minimal, low, medium, high, xhigh, or max"
        ));
    }

    let chat_dialect = match capability_string(
        object,
        &["chat_dialect", "chatDialect"],
        file_name,
    )?
    .as_deref()
    .unwrap_or(defaults.chat_dialect.as_str())
    {
        "generic" => OpenAiChatDialect::Generic,
        "deepseek" => OpenAiChatDialect::DeepSeek,
        "qwen" => OpenAiChatDialect::Qwen,
        "kimi" => OpenAiChatDialect::Kimi,
        other => {
            return Err(format!(
                "Provider profile '{file_name}' capability 'chat_dialect' has unsupported value '{other}' (expected generic, deepseek, qwen, or kimi)"
            ))
        }
    };
    if chat_dialect == OpenAiChatDialect::Kimi && default_reasoning_effort.is_none() {
        default_reasoning_effort = Some("max".to_string());
    }

    let default_tool_schema = if chat_dialect == OpenAiChatDialect::Kimi {
        "preserve"
    } else {
        defaults.tool_schema.as_str()
    };
    let tool_schema = match capability_string(
        object,
        &["tool_schema", "toolSchema"],
        file_name,
    )?
        .as_deref()
        .unwrap_or(default_tool_schema)
    {
        "sanitize" => ToolSchemaMode::Sanitize,
        "preserve" => ToolSchemaMode::Preserve,
        other => {
            return Err(format!(
                "Provider profile '{file_name}' capability 'tool_schema' has unsupported value '{other}' (expected sanitize or preserve)"
            ))
        }
    };
    let default_max_tokens_field = if chat_dialect == OpenAiChatDialect::Kimi {
        "max_completion_tokens"
    } else {
        defaults.max_tokens_field.as_str()
    };
    let max_tokens_field = match capability_string(
        object,
        &["max_tokens_field", "maxTokensField"],
        file_name,
    )?
    .as_deref()
    .unwrap_or(default_max_tokens_field)
    {
        "max_tokens" => MaxTokensField::MaxTokens,
        "max_completion_tokens" => MaxTokensField::MaxCompletionTokens,
        "omit" => MaxTokensField::Omit,
        other => {
            return Err(format!(
                "Provider profile '{file_name}' capability 'max_tokens_field' has unsupported value '{other}' (expected max_tokens, max_completion_tokens, or omit)"
            ))
        }
    };
    let tool_result_media = match capability_string(
        object,
        &["tool_result_media", "toolResultMedia"],
        file_name,
    )?
    .as_deref()
    .unwrap_or(defaults.tool_result_media.as_str())
    {
        "separate_user" => ToolResultMediaMode::SeparateUser,
        "inline" => ToolResultMediaMode::Inline,
        other => {
            return Err(format!(
                "Provider profile '{file_name}' capability 'tool_result_media' has unsupported value '{other}' (expected separate_user or inline)"
            ))
        }
    };
    let gemini_builtin_tools = capability_string_array(
        object,
        &["gemini_builtin_tools", "geminiBuiltinTools"],
        defaults.gemini_builtin_tools.clone(),
        file_name,
    )?;
    for tool in &gemini_builtin_tools {
        if !matches!(
            tool.as_str(),
            "google_search" | "url_context" | "code_execution" | "google_maps"
        ) {
            return Err(format!(
                "Provider profile '{file_name}' capability 'gemini_builtin_tools' contains unsupported tool '{tool}' (expected google_search, url_context, code_execution, or google_maps)"
            ));
        }
    }
    let gemini_file_search_store_names = capability_string_array(
        object,
        &[
            "gemini_file_search_store_names",
            "geminiFileSearchStoreNames",
        ],
        defaults.gemini_file_search_store_names.clone(),
        file_name,
    )?;
    let responses_builtin_tools = capability_string_array(
        object,
        &["responses_builtin_tools", "responsesBuiltinTools"],
        defaults.responses_builtin_tools.clone(),
        file_name,
    )?;
    let kimi_formula_tools = capability_string_array(
        object,
        &["kimi_formula_tools", "kimiFormulaTools"],
        defaults.kimi_formula_tools.clone(),
        file_name,
    )?;
    for formula in &kimi_formula_tools {
        if !is_supported_kimi_formula(formula) {
            return Err(format!(
                "Provider profile '{file_name}' capability 'kimi_formula_tools' contains unsupported formula '{formula}'"
            ));
        }
    }
    if !kimi_formula_tools.is_empty() && chat_dialect != OpenAiChatDialect::Kimi {
        return Err(format!(
            "Provider profile '{file_name}' configures kimi_formula_tools but is not a Kimi profile"
        ));
    }

    Ok(OpenAiCapabilities {
        chat_dialect,
        stream_options: capability_bool(
            object,
            &["stream_options", "streamOptions"],
            defaults.stream_options,
            file_name,
        )?,
        parallel_tool_calls: capability_bool(
            object,
            &["parallel_tool_calls", "parallelToolCalls"],
            defaults.parallel_tool_calls,
            file_name,
        )?,
        reasoning_effort: capability_bool(
            object,
            &["reasoning_effort", "reasoningEffort"],
            defaults.reasoning_effort,
            file_name,
        )?,
        default_reasoning_effort,
        reasoning_fields,
        thinking_tags: capability_bool(
            object,
            &["thinking_tags", "thinkingTags"],
            defaults.thinking_tags,
            file_name,
        )?,
        include_thoughts: capability_bool(
            object,
            &["include_thoughts", "includeThoughts"],
            defaults.include_thoughts,
            file_name,
        )?,
        sampling_parameters: capability_bool(
            object,
            &["sampling_parameters", "samplingParameters"],
            if chat_dialect == OpenAiChatDialect::Kimi {
                false
            } else {
                defaults.sampling_parameters
            },
            file_name,
        )?,
        tool_result_media,
        tool_schema,
        max_tokens_field,
        responses_stateful: capability_bool(
            object,
            &["responses_stateful", "responsesStateful"],
            defaults.responses_stateful,
            file_name,
        )?,
        responses_session_cache: capability_bool(
            object,
            &["responses_session_cache", "responsesSessionCache"],
            defaults.responses_session_cache,
            file_name,
        )?,
        responses_builtin_tools,
        responses_apply_patch_custom: capability_bool(
            object,
            &["responses_apply_patch_custom", "responsesApplyPatchCustom"],
            defaults.responses_apply_patch_custom,
            file_name,
        )?,
        kimi_formula_tools,
        gemini_builtin_tools,
        gemini_file_search_store_names,
    })
}

fn openai_capabilities_json(capabilities: &OpenAiCapabilities) -> Value {
    json!({
        "chat_dialect": capabilities.chat_dialect.as_str(),
        "stream_options": capabilities.stream_options,
        "parallel_tool_calls": capabilities.parallel_tool_calls,
        "reasoning_effort": capabilities.reasoning_effort,
        "default_reasoning_effort": capabilities.default_reasoning_effort,
        "reasoning_fields": capabilities.reasoning_fields,
        "thinking_tags": capabilities.thinking_tags,
        "include_thoughts": capabilities.include_thoughts,
        "sampling_parameters": capabilities.sampling_parameters,
        "tool_result_media": capabilities.tool_result_media.as_str(),
        "tool_schema": capabilities.tool_schema.as_str(),
        "max_tokens_field": capabilities.max_tokens_field.as_str(),
        "responses_stateful": capabilities.responses_stateful,
        "responses_session_cache": capabilities.responses_session_cache,
        "responses_builtin_tools": capabilities.responses_builtin_tools,
        "responses_apply_patch_custom": capabilities.responses_apply_patch_custom,
        "kimi_formula_tools": capabilities.kimi_formula_tools,
        "gemini_builtin_tools": capabilities.gemini_builtin_tools,
        "gemini_file_search_store_names": capabilities.gemini_file_search_store_names
    })
}

fn build_provider_client(file_name: &str, proxy_url: Option<&str>) -> Result<Client, String> {
    let mut client_builder = Client::builder()
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
        .timeout(UPSTREAM_REQUEST_TIMEOUT);
    if let Some(proxy_url) = proxy_url {
        client_builder = client_builder.proxy(
            Proxy::all(proxy_url)
                .map_err(|err| format!("Invalid proxy in '{file_name}': {err}"))?,
        );
    } else {
        client_builder = client_builder.no_proxy();
    }
    client_builder
        .build()
        .map_err(|err| format!("Cannot create HTTP client for '{file_name}': {err}"))
}

fn load_native_provider_profiles(
    paths: Vec<PathBuf>,
    local_bridge_base_url: &str,
) -> Result<Vec<ProviderProfile>, String> {
    let mut profiles = Vec::new();
    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("Invalid profile file name '{}'", path.display()))?
            .to_string();
        let settings = read_profile_json(&path)?;
        let object = settings
            .as_object()
            .ok_or_else(|| format!("Provider profile '{file_name}' must be a JSON object"))?;
        if object
            .get("enabled")
            .and_then(Value::as_bool)
            .is_some_and(|enabled| !enabled)
        {
            continue;
        }

        let model = profile_string(object, &["model"])
            .ok_or_else(|| format!("Provider profile '{file_name}' has no model"))?;
        let context_window = profile_u64(object, &["context_window", "contextWindow"], &file_name)?;
        let display_name = profile_string(object, &["name"]).unwrap_or_else(|| model.clone());
        let base_url = profile_string(object, &["base_url", "baseURL"])
            .ok_or_else(|| format!("Provider profile '{file_name}' has no base_url"))?;
        let protocol = profile_string(object, &["protocol"])
            .unwrap_or_else(|| "openai".to_string())
            .to_ascii_lowercase();
        let transport = match protocol.as_str() {
            "openai" | "openai-chat" | "chat-completions" => ProviderTransport::OpenAiChat,
            "openai-responses" | "responses" => ProviderTransport::OpenAiResponses,
            "anthropic" | "messages" => ProviderTransport::Anthropic,
            "gemini" | "local-gemini" => ProviderTransport::LocalGemini,
            "gemini-interactions" | "interactions" => ProviderTransport::GeminiInteractions,
            other => {
                return Err(format!(
                    "Provider profile '{file_name}' has unsupported protocol '{other}' (expected openai, openai-responses, anthropic, gemini-interactions, or gemini)"
                ))
            }
        };
        let capability_defaults = match transport {
            ProviderTransport::LocalGemini => OpenAiCapabilities::local_gemini(),
            ProviderTransport::GeminiInteractions => OpenAiCapabilities::gemini_interactions(),
            ProviderTransport::OpenAiChat => OpenAiCapabilities::for_openai_base_url(&base_url),
            ProviderTransport::OpenAiResponses => {
                OpenAiCapabilities::for_responses_base_url(&base_url)
            }
            ProviderTransport::Anthropic => OpenAiCapabilities::for_anthropic_base_url(&base_url),
        };
        let openai_capabilities =
            parse_openai_capabilities_with_defaults(object, &file_name, capability_defaults)?;
        let vision = parse_vision_config(object, &file_name)?;
        if transport == ProviderTransport::LocalGemini
            && normalize_base_url(&base_url) != normalize_base_url(local_bridge_base_url)
        {
            return Err(format!(
                "Provider profile '{file_name}' uses protocol 'gemini' but base_url is not the local bridge URL '{local_bridge_base_url}'"
            ));
        }
        let upstream_url =
            profile_string(object, &["endpoint"]).unwrap_or_else(|| match transport {
                ProviderTransport::OpenAiChat => openai_compatible_chat_endpoint(&base_url),
                ProviderTransport::OpenAiResponses => openai_responses_endpoint(&base_url),
                ProviderTransport::Anthropic => anthropic_messages_endpoint(&base_url),
                ProviderTransport::GeminiInteractions => gemini_interactions_endpoint(&base_url),
                ProviderTransport::LocalGemini => base_url.clone(),
            });
        let api_key = profile_string(object, &["api_key", "apiKey"]);
        let api_key_env = profile_string(object, &["api_key_env", "apiKeyEnv"]);
        let api_key = match (api_key, api_key_env) {
            (Some(api_key), _) => Some(api_key),
            (None, Some(variable)) => Some(
                env::var(&variable)
                    .map_err(|_| format!("Provider profile '{file_name}' requires environment variable '{variable}'"))?
                    .trim()
                    .to_string(),
            ),
            (None, None) => None,
        }
        .filter(|value| !value.is_empty());
        if transport != ProviderTransport::LocalGemini && api_key.is_none() {
            return Err(format!(
                "Provider profile '{file_name}' has no API credential"
            ));
        }
        let proxy_url = profile_string(object, &["proxy", "proxy_url"]);
        let client = build_provider_client(&file_name, proxy_url.as_deref())?;
        let upstream_identity = profile_string(object, &["identity"]);
        let auth_scheme =
            profile_string(object, &["auth_scheme", "authScheme"]).unwrap_or_else(|| {
                if matches!(
                    transport,
                    ProviderTransport::OpenAiChat | ProviderTransport::OpenAiResponses
                ) {
                    "bearer".to_string()
                } else {
                    "x-api-key".to_string()
                }
            });
        if !matches!(auth_scheme.as_str(), "bearer" | "x-api-key") {
            return Err(format!(
                "Provider profile '{file_name}' field 'auth_scheme' must be bearer or x-api-key"
            ));
        }
        if transport == ProviderTransport::GeminiInteractions && auth_scheme != "x-api-key" {
            return Err(format!(
                "Provider profile '{file_name}' uses Gemini Interactions and cannot override auth_scheme"
            ));
        }
        let identity_override = object
            .get("identity_override")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        profiles.push(ProviderProfile {
            file_name,
            display_name,
            source: ProviderProfileSource::Native,
            model,
            context_window,
            upstream_identity,
            identity_override,
            base_url,
            auth_token: if auth_scheme == "bearer" {
                api_key.clone()
            } else {
                None
            },
            api_key: if auth_scheme == "x-api-key" {
                api_key
            } else {
                None
            },
            proxy_url,
            local_gemini: transport == ProviderTransport::LocalGemini,
            transport,
            openai_capabilities,
            vision,
            upstream_url,
            client,
        });
    }
    Ok(profiles)
}

fn load_legacy_provider_profiles(
    settings_dir: &Path,
    local_bridge_base_url: &str,
) -> Result<Vec<ProviderProfile>, String> {
    let paths = provider_profile_paths(settings_dir, is_provider_profile_file_name)?;

    let mut profiles = Vec::new();
    for path in paths {
        match load_legacy_provider_profile(&path, local_bridge_base_url) {
            Ok(profile) => profiles.push(profile),
            Err(message) => warn!(
                path = %path.display(),
                error = %message,
                "Skipping invalid optional legacy provider profile"
            ),
        }
    }

    Ok(profiles)
}

fn load_legacy_provider_profile(
    path: &Path,
    local_bridge_base_url: &str,
) -> Result<ProviderProfile, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid profile file name '{}'", path.display()))?
        .to_string();
    let settings = read_profile_json(path)?;
    let env = settings
        .get("env")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Profile '{file_name}' has no env object"))?;
    let get_env = |name: &str| {
        env.get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let base_url = get_env("ANTHROPIC_BASE_URL")
        .ok_or_else(|| format!("Profile '{file_name}' has no ANTHROPIC_BASE_URL"))?;
    let model = get_env("ANTHROPIC_MODEL")
        .or_else(|| get_env("ANTHROPIC_DEFAULT_SONNET_MODEL"))
        .ok_or_else(|| format!("Profile '{file_name}' has no model"))?;
    let upstream_identity = get_env("CLAUDE_BRIDGE_UPSTREAM_IDENTITY");
    let identity_override = get_env("CLAUDE_BRIDGE_IDENTITY_OVERRIDE")
        .map(|value| identity_override_enabled(&value))
        .unwrap_or(true);
    let auth_token = get_env("ANTHROPIC_AUTH_TOKEN");
    let api_key = get_env("ANTHROPIC_API_KEY");
    if auth_token.is_none() && api_key.is_none() {
        return Err(format!("Profile '{file_name}' has no API credential"));
    }
    let proxy_url = get_env("HTTPS_PROXY")
        .or_else(|| get_env("HTTP_PROXY"))
        .or_else(|| get_env("ALL_PROXY"));
    let client = build_provider_client(&file_name, proxy_url.as_deref())?;
    let local_gemini = normalize_base_url(&base_url) == normalize_base_url(local_bridge_base_url);
    let (transport, upstream_url) = resolve_provider_transport(
        &base_url,
        &model,
        local_gemini,
        get_env("CLAUDE_BRIDGE_TRANSPORT").as_deref(),
        get_env("CLAUDE_BRIDGE_UPSTREAM_URL").as_deref(),
    )
    .map_err(|err| format!("Invalid transport in profile '{file_name}': {err}"))?;

    let openai_capabilities = if local_gemini {
        OpenAiCapabilities::local_gemini()
    } else if transport == ProviderTransport::OpenAiChat {
        OpenAiCapabilities::for_openai_base_url(&base_url)
    } else {
        OpenAiCapabilities::default()
    };

    Ok(ProviderProfile {
        display_name: file_name.trim_end_matches(".json").to_string(),
        source: ProviderProfileSource::Legacy,
        file_name,
        model,
        context_window: None,
        upstream_identity,
        identity_override,
        base_url,
        auth_token,
        api_key,
        proxy_url,
        local_gemini,
        transport,
        openai_capabilities,
        vision: VisionConfig::default(),
        upstream_url,
        client,
    })
}

fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn resolve_provider_transport(
    base_url: &str,
    model: &str,
    local_gemini: bool,
    configured_transport: Option<&str>,
    configured_upstream_url: Option<&str>,
) -> Result<(ProviderTransport, String), String> {
    if local_gemini {
        return Ok((ProviderTransport::LocalGemini, base_url.to_string()));
    }

    let configured = configured_transport
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("auto")
        .to_ascii_lowercase();
    match configured.as_str() {
        "auto" => {
            if !is_claude_identity(model) {
                if let Some(upstream_url) = known_openai_chat_endpoint(base_url) {
                    return Ok((ProviderTransport::OpenAiChat, upstream_url));
                }
            }
            Ok((
                ProviderTransport::Anthropic,
                anthropic_messages_endpoint(base_url),
            ))
        }
        "anthropic" => Ok((
            ProviderTransport::Anthropic,
            configured_upstream_url
                .map(str::to_owned)
                .unwrap_or_else(|| anthropic_messages_endpoint(base_url)),
        )),
        "openai" | "openai-chat" | "chat-completions" => Ok((
            ProviderTransport::OpenAiChat,
            configured_upstream_url
                .map(str::to_owned)
                .unwrap_or_else(|| openai_chat_endpoint(base_url)),
        )),
        "openai-responses" | "responses" => Ok((
            ProviderTransport::OpenAiResponses,
            configured_upstream_url
                .map(str::to_owned)
                .unwrap_or_else(|| openai_responses_endpoint(base_url)),
        )),
        "gemini-interactions" | "interactions" => Ok((
            ProviderTransport::GeminiInteractions,
            configured_upstream_url
                .map(str::to_owned)
                .unwrap_or_else(|| gemini_interactions_endpoint(base_url)),
        )),
        other => Err(format!(
            "unsupported CLAUDE_BRIDGE_TRANSPORT '{other}' (expected auto, anthropic, gemini-interactions, openai-chat, or openai-responses)"
        )),
    }
}

fn anthropic_messages_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/v1/messages") {
        base_url.to_string()
    } else {
        format!("{base_url}/v1/messages")
    }
}

fn gemini_interactions_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/interactions") {
        base_url.to_string()
    } else {
        format!("{base_url}/interactions")
    }
}

fn openai_chat_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else if base_url.ends_with("/v1") || base_url.ends_with("/compatible-mode/v1") {
        format!("{base_url}/chat/completions")
    } else {
        format!("{base_url}/v1/chat/completions")
    }
}

fn openai_responses_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/responses") {
        base_url.to_string()
    } else if base_url.ends_with("/v1") || base_url.ends_with("/compatible-mode/v1") {
        format!("{base_url}/responses")
    } else {
        format!("{base_url}/v1/responses")
    }
}

/// Native provider files use the same `base_url` value shown in an OpenAI SDK
/// example. SDK base URLs already include any provider-specific version path,
/// so only the method path is appended here.
fn openai_compatible_chat_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{base_url}/chat/completions")
    }
}

fn known_openai_chat_endpoint(base_url: &str) -> Option<String> {
    let normalized = normalize_base_url(base_url);

    if normalized == "https://api.deepseek.com/anthropic" {
        return Some("https://api.deepseek.com/chat/completions".to_string());
    }
    if normalized == "https://api.moonshot.cn/anthropic" {
        return Some("https://api.moonshot.cn/v1/chat/completions".to_string());
    }
    if normalized == "https://api.moonshot.ai/anthropic" {
        return Some("https://api.moonshot.ai/v1/chat/completions".to_string());
    }
    if let Some(prefix) = normalized.strip_suffix("/apps/anthropic") {
        if prefix == "https://coding.dashscope.aliyuncs.com"
            || prefix == "https://coding-intl.dashscope.aliyuncs.com"
        {
            return Some(format!("{prefix}/v1/chat/completions"));
        }
        if prefix.ends_with("dashscope.aliyuncs.com") || prefix.ends_with("maas.aliyuncs.com") {
            return Some(format!("{prefix}/compatible-mode/v1/chat/completions"));
        }
    }
    None
}

fn identity_override_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn build_gemini_client(
    proxy_url: Option<&str>,
    timeout: Option<Duration>,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
        .timeout(timeout.unwrap_or(UPSTREAM_REQUEST_TIMEOUT));
    builder = match proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        Some(proxy_url) => builder.proxy(
            Proxy::all(proxy_url)
                .map_err(|err| format!("Invalid Gemini proxy '{proxy_url}': {err}"))?,
        ),
        None => builder.no_proxy(),
    };
    builder
        .build()
        .map_err(|err| format!("Cannot create Gemini HTTP client: {err}"))
}

async fn read_response_bytes_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!("Upstream response body exceeds {limit} bytes"));
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| format!("Cannot read upstream response body: {err}"))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(format!("Upstream response body exceeds {limit} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_response_text_limited(response: reqwest::Response) -> Result<String, String> {
    let body = read_response_bytes_limited(response, MAX_UPSTREAM_RESPONSE_BYTES).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

async fn read_response_json_limited(response: reqwest::Response) -> Result<Value, String> {
    let body = read_response_bytes_limited(response, MAX_UPSTREAM_RESPONSE_BYTES).await?;
    serde_json::from_slice(&body).map_err(|err| format!("Invalid JSON in upstream response: {err}"))
}

fn provider_config_stamp(providers_dir: &Path, legacy_settings_dir: &Path) -> String {
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(providers_dir) {
        paths.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_native_provider_file_name)
        }));
    }
    if let Ok(entries) = fs::read_dir(legacy_settings_dir) {
        paths.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_provider_profile_file_name)
        }));
    }
    paths.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());

    let mut hasher = DefaultHasher::new();
    for path in &paths {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        file_name.hash(&mut hasher);
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                duration.as_nanos().hash(&mut hasher);
            }
        }
        if let Ok(contents) = fs::read(path) {
            contents.hash(&mut hasher);
        }
    }
    format!("{}:{:016x}", paths.len(), hasher.finish())
}

fn read_state_object(state_path: &Path) -> Map<String, Value> {
    fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn load_persisted_gemini_proxy(state_path: &Path) -> Option<Option<String>> {
    let value = fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let proxy = value.as_object()?.get("gemini_proxy")?;
    match proxy {
        Value::Null => Some(None),
        Value::String(value) => Some((!value.trim().is_empty()).then(|| value.trim().to_string())),
        _ => None,
    }
}

fn persist_bridge_state(
    state_path: &Path,
    active_profile: &str,
    proxy_url: Option<&str>,
) -> Result<(), String> {
    // Preserve unrelated keys (for example the recorded listen address) that
    // other writers stored in the state file.
    let mut state_json = read_state_object(state_path);
    state_json.insert(
        "active_profile".to_string(),
        Value::String(active_profile.to_string()),
    );
    state_json.insert(
        "gemini_proxy".to_string(),
        proxy_url
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    write_state_atomically(state_path, &Value::Object(state_json).to_string())
}

fn record_listen_in_state(state_path: &Path, listen: &str) -> Result<(), String> {
    let mut state_json = read_state_object(state_path);
    state_json.insert("listen".to_string(), Value::String(listen.to_string()));
    write_state_atomically(state_path, &Value::Object(state_json).to_string())
}

fn write_state_atomically(state_path: &Path, contents: &str) -> Result<(), String> {
    let file_name = state_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bridge-state.json");
    let temporary_path =
        state_path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
    let write_result = (|| -> std::io::Result<()> {
        let mut temporary = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(contents.as_bytes())?;
        temporary.sync_all()?;
        drop(temporary);
        replace_file_atomically(&temporary_path, state_path)
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "Cannot atomically persist bridge state to '{}': {err}",
            state_path.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

async fn provider_config_stamp_async(
    providers_dir: PathBuf,
    legacy_settings_dir: PathBuf,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || provider_config_stamp(&providers_dir, &legacy_settings_dir))
        .await
        .map_err(|err| format!("Cannot inspect provider configuration: {err}"))
}

async fn persist_bridge_state_async(
    state_path: PathBuf,
    active_profile: String,
    proxy_url: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        persist_bridge_state(&state_path, &active_profile, proxy_url.as_deref())
    })
    .await
    .map_err(|err| format!("Cannot join bridge-state writer: {err}"))?
}

fn current_gemini_transport(state: &AppState) -> Result<GeminiTransport, String> {
    state
        .gemini_transport
        .read()
        .map(|transport| transport.clone())
        .map_err(|_| "Cannot read Gemini proxy state".to_string())
}

fn select_initial_profile(profiles: &[ProviderProfile], state_path: &Path) -> String {
    let persisted = fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("active_profile")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    if let Some(file_name) = persisted {
        if profiles
            .iter()
            .any(|profile| profile.file_name == file_name)
        {
            return file_name;
        }
    }
    profiles
        .iter()
        .find(|profile| profile.local_gemini)
        .or_else(|| profiles.first())
        .map(|profile| profile.file_name.clone())
        .unwrap_or_default()
}

fn active_provider_profile(state: &AppState) -> Option<ProviderProfile> {
    let routing = state.routing.read().ok()?;
    routing
        .profiles
        .iter()
        .find(|profile| profile.file_name == routing.active_file)
        .cloned()
}
