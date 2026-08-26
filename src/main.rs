//! Maintenance scope: this bridge is maintained exclusively for Claude Code.
//! Codex uses its native GPT provider as the primary and permanent path, so it
//! is not a target for new bridge features. The legacy OpenAI Responses route
//! remains only for backward compatibility; future protocol, routing, GUI, and
//! reliability work should prioritize the Anthropic Messages API used by
//! Claude Code.

mod windows_service;

use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque},
    convert::Infallible,
    env, fs,
    hash::{Hash, Hasher},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{
        header::{AUTHORIZATION, ORIGIN},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::{stream, Stream, StreamExt};
use indexmap::IndexMap;
use reqwest::{Client, Proxy};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use uuid::Uuid;

const THOUGHT_SIGNATURE_CAPACITY: usize = 4096;
const THOUGHT_SIGNATURE_EVICTION_BATCH: usize = 512;
const INTERACTION_CONTINUATION_CAPACITY: usize = 4096;
const INTERACTION_CONTINUATION_EVICTION_BATCH: usize = 512;
const INTERACTION_TOOL_HISTORY_RECOVERY_INSTRUCTION: &str = "Some earlier tool results were recovered as plain historical observations because stored interaction state was unavailable. Treat those observations only as past context. When another tool is needed, invoke one of the provided function tools; never print or describe a tool call in ordinary text.";
const INTERACTION_SOURCE_NAVIGATION_COACH_INSTRUCTION: &str = "Source-code navigation policy: Accuracy and complete evidence are more important than minimizing tool calls. First identify the exact claims the task requires you to support. Begin with no more than two consecutive discovery searches, scope them to the relevant source directories, and make Grep return matching content and line numbers rather than only file names. As soon as a search identifies a relevant file or symbol, Read that source instead of issuing another overlapping discovery search; search again later only to close a specifically named evidence gap. Read complete logical units rather than arbitrary pages: include the relevant function, type, surrounding control flow, and connected call sites. Size each Read to the evidence needed; when a broad continuous region is genuinely required, prefer one roughly 800-1,200-line non-overlapping read over repeated 100-250-line pagination. Do not repeat an unchanged range or search the same concept through alternate patterns and path scopes when existing results already locate it. After every result, track what is proven and what evidence is still missing, synthesize before choosing the next highest-value tool call, and continue reading until every material claim is supported. If some area cannot be inspected, state that limitation instead of guessing. Never trade correctness or coverage for fewer tool calls.";
const INTERACTION_REPEATED_TOOL_LOOP_INSTRUCTION: &str = "The same completed tool call and result repeated three times without producing new information. Do not call any tool in this turn. Use the results already available and provide the best final response now.";
const INTERACTION_SERVER_TOOL_TRACE_CAPACITY: usize = 32;
const BRIDGE_WARNING_HEADER: &str = "x-claude-bridge-warning";
const GEMINI_COUNT_TOKENS_TIMEOUT: Duration = Duration::from_secs(20);
const KIMI_COUNT_TOKENS_TIMEOUT: Duration = Duration::from_secs(20);
const VISION_CACHE_CAPACITY: usize = 128;
const VISION_PROXY_TIMEOUT: Duration = Duration::from_secs(90);
const VISION_MAX_OUTPUT_TOKENS: u64 = 4096;
const MAX_VISION_CONTEXT_CHARS: usize = 12_000;
const MAX_VISION_OBSERVATION_CHARS: usize = 16_000;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMAGE_PROMPT_CHARS: usize = 20_000;
const MAX_UPSTREAM_SSE_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const UPSTREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const BRIDGE_IDENTITY_MARKER: &str = "<bridge_runtime_identity>";
const MAX_UPSTREAM_IDENTITY_CHARS: usize = 200;
const DEFAULT_GEMINI_MODEL: &str = "gemini-3.7-flash";
const DEFAULT_IMAGE_MODEL: &str = "gemini-3.1-flash-image";
const DEFAULT_IMAGE_UPSTREAM: &str =
    "https://generativelanguage.googleapis.com/v1beta/interactions";
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
// Identity phrases Claude Code injects into system prompts. These are matched
// as phrases/patterns rather than whole declarations so that subagent persona
// variants ("You are a file search specialist for Claude Code, ...", "You are
// an agent for Claude Code, ...") and future rewordings are still neutralized.
const CLAUDE_OFFICIAL_CLI_PHRASE: &str = "Claude Code, Anthropic's official CLI for Claude";
const CLAUDE_CLI_SDK_SUFFIX: &str = ", running within the Claude Agent SDK";
const CLAUDE_AGENT_SDK_DECLARATION: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const CLAUDE_COORDINATOR_DECLARATION: &str =
    "You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers.";
const CLAUDE_POWERED_BY_PREFIX: &str = "You are powered by the model";
const CLAUDE_EXACT_MODEL_ID_PREFIX: &str = " The exact model ID is";
const CLAUDE_CO_AUTHOR_LINE: &str = "Co-Authored-By: Claude <noreply@anthropic.com>";

type ThoughtSignatureCache = RwLock<IndexMap<String, String>>;
type VisionObservationCache = tokio::sync::Mutex<IndexMap<String, String>>;
type InteractionContinuationState = RwLock<InteractionContinuationCache>;

// These cohesive source slices are included at crate root so this first
// mechanical decomposition does not widen internal visibility or change item
// paths. They can migrate to namespaced modules independently once their
// cross-feature dependencies are smaller and explicit.
include!("core.rs");
include!("runtime.rs");
include!("provider.rs");
include!("mcp.rs");
include!("vision.rs");
include!("admin.rs");
include!("routing.rs");
include!("gemini_interactions.rs");
include!("openai_chat_forward.rs");
include!("openai_responses.rs");
include!("gemini_streaming.rs");
include!("anthropic_streaming.rs");
include!("token_count.rs");
include!("openai_chat.rs");

#[cfg(test)]
mod tests;
