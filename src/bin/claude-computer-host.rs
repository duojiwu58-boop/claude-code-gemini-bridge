use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{self, BufRead, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

#[path = "../computer_desktop.rs"]
mod computer_desktop;

const PROTOCOL_VERSION: &str = "gemini-computer-v1";
const VIEWPORT_WIDTH: u64 = 1440;
const VIEWPORT_HEIGHT: u64 = 900;
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_COMPLETED_BATCHES: usize = 128;

struct BrowserSession {
    child: Child,
    port: u16,
    websocket_url: String,
    target_pid: Option<u64>,
    session_id: String,
    last_allowed_url: String,
    held_keys: HashSet<String>,
}

type CdpSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct CdpClient {
    socket: CdpSocket,
    next_id: u64,
}

impl CdpClient {
    async fn connect(websocket_url: &str) -> Result<Self, String> {
        let (socket, _) = connect_async(websocket_url)
            .await
            .map_err(|error| format!("Cannot connect to browser DevTools: {error}"))?;
        Ok(Self { socket, next_id: 1 })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let payload = json!({"id": id, "method": method, "params": params}).to_string();
        self.socket
            .send(Message::Text(payload.into()))
            .await
            .map_err(|error| format!("Cannot send DevTools command {method}: {error}"))?;
        loop {
            let message = tokio::time::timeout(Duration::from_secs(30), self.socket.next())
                .await
                .map_err(|_| format!("DevTools command {method} timed out"))?
                .ok_or_else(|| "Browser DevTools connection closed".to_string())?
                .map_err(|error| format!("Cannot read DevTools response: {error}"))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| format!("Invalid DevTools JSON: {error}"))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("DevTools {method} failed: {error}"));
            }
            return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
        }
    }
}

fn argument_value(name: &str) -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

fn find_browser() -> Result<PathBuf, String> {
    if let Some(path) = argument_value("--browser-path")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Ok(path);
    }
    let mut candidates = Vec::new();
    for root in [
        env::var_os("PROGRAMFILES"),
        env::var_os("PROGRAMFILES(X86)"),
        env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        let root = PathBuf::from(root);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "Microsoft Edge or Google Chrome was not found".to_string())
}

fn isolated_profile_dir(session_id: &str) -> Result<PathBuf, String> {
    let root = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_string())?;
    let path = root
        .join(r"ClaudeCodeBridge\ComputerHost\BrowserProfiles")
        .join(session_id);
    fs::create_dir_all(&path).map_err(|error| {
        format!(
            "Cannot create isolated browser profile '{}': {error}",
            path.display()
        )
    })?;
    Ok(path)
}

fn unused_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Cannot reserve a DevTools port: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

fn is_loopback_url(value: &str) -> bool {
    url::Url::parse(value).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some_and(|host| match host {
                url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            })
    })
}

async fn browser_target(client: &Client, port: u16) -> Result<String, String> {
    for _ in 0..200 {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .send()
            .await
        {
            if let Ok(targets) = response.json::<Value>().await {
                if let Some(url) = targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
                    .and_then(|target| target.get("webSocketDebuggerUrl"))
                    .and_then(Value::as_str)
                {
                    return Ok(url.to_string());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("Browser did not expose a page DevTools target within 20 seconds".to_string())
}

async fn browser_process_id(client: &Client, port: u16) -> Option<u64> {
    let version = client
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    let websocket_url = version.get("webSocketDebuggerUrl")?.as_str()?;
    let mut cdp = CdpClient::connect(websocket_url).await.ok()?;
    let process_info = cdp
        .call("SystemInfo.getProcessInfo", json!({}))
        .await
        .ok()?;
    process_info
        .get("processInfo")?
        .as_array()?
        .iter()
        .find(|process| process.get("type").and_then(Value::as_str) == Some("browser"))?
        .get("id")?
        .as_u64()
}

async fn start_browser(
    client: &Client,
    session_id: &str,
    initial_url: &str,
) -> Result<BrowserSession, String> {
    if !is_loopback_url(initial_url) {
        return Err("Computer Host refuses a non-loopback initial URL".to_string());
    }
    let browser = find_browser()?;
    let profile = isolated_profile_dir(session_id)?;
    let port = unused_loopback_port()?;
    let child = Command::new(&browser)
        .args([
            format!("--remote-debugging-port={port}"),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-sync".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-component-update".to_string(),
            "--window-size=1440,900".to_string(),
            "--force-device-scale-factor=1".to_string(),
            "--proxy-server=http://127.0.0.1:9".to_string(),
            "--proxy-bypass-list=<local>;localhost;127.0.0.1;[::1]".to_string(),
            "about:blank".to_string(),
        ])
        .spawn()
        .map_err(|error| {
            format!(
                "Cannot start isolated browser '{}': {error}",
                browser.display()
            )
        })?;
    let websocket_url = match browser_target(client, port).await {
        Ok(value) => value,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            return Err(error);
        }
    };
    let target_pid = browser_process_id(client, port).await;
    let mut session = BrowserSession {
        child,
        port,
        websocket_url,
        target_pid,
        session_id: session_id.to_string(),
        last_allowed_url: initial_url.to_string(),
        held_keys: HashSet::new(),
    };
    let setup = async {
        let mut cdp = session.connect(client).await?;
        cdp.call("Page.enable", json!({})).await?;
        cdp.call("Runtime.enable", json!({})).await?;
        cdp.call("Emulation.setDeviceMetricsOverride", json!({
            "width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT, "deviceScaleFactor": 1, "mobile": false,
            "screenWidth": VIEWPORT_WIDTH, "screenHeight": VIEWPORT_HEIGHT
        })).await?;
        cdp.call("Page.navigate", json!({"url": initial_url}))
            .await?;
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = setup {
        session.stop(client).await;
        return Err(error);
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    Ok(session)
}

impl BrowserSession {
    async fn connect(&mut self, client: &Client) -> Result<CdpClient, String> {
        match CdpClient::connect(&self.websocket_url).await {
            Ok(cdp) => Ok(cdp),
            Err(_) => {
                self.websocket_url = browser_target(client, self.port).await?;
                CdpClient::connect(&self.websocket_url).await
            }
        }
    }

    async fn stop(&mut self, client: &Client) {
        if let Ok(mut cdp) = self.connect(client).await {
            let _ = cdp.call("Browser.close", json!({})).await;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes[16..20].try_into().ok()?) as u64,
        u32::from_be_bytes(bytes[20..24].try_into().ok()?) as u64,
    ))
}

async fn screenshot(cdp: &mut CdpClient) -> Result<Value, String> {
    let result = cdp
        .call(
            "Page.captureScreenshot",
            json!({"format": "png", "fromSurface": true, "optimizeForSpeed": true}),
        )
        .await?;
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "DevTools screenshot has no PNG data".to_string())?;
    let bytes = BASE64_STANDARD
        .decode(data)
        .map_err(|error| format!("Invalid screenshot base64: {error}"))?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Err(format!(
            "Screenshot is {} bytes, exceeding the {} byte limit",
            bytes.len(),
            MAX_SCREENSHOT_BYTES
        ));
    }
    let (width, height) = png_dimensions(&bytes)
        .ok_or_else(|| "DevTools screenshot is not a valid PNG".to_string())?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(
        json!({"mime_type": "image/png", "data": data, "width": width, "height": height, "bytes": bytes.len(), "sha256": sha256}),
    )
}

async fn environment_state(cdp: &mut CdpClient, session: &BrowserSession) -> Result<Value, String> {
    let result = cdp.call("Runtime.evaluate", json!({"expression": "JSON.stringify({current_url:location.href,window_title:document.title})", "returnByValue": true})).await?;
    let serialized = result
        .pointer("/result/value")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let mut state: Value = serde_json::from_str(serialized).unwrap_or_else(|_| json!({}));
    state["environment"] = json!("browser");
    state["target_pid"] = session.target_pid.map_or(Value::Null, Value::from);
    state["viewport"] =
        json!({"width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT, "device_scale_factor": 1});
    Ok(state)
}

fn scaled_coordinate(value: u64, extent: u64) -> f64 {
    ((value.min(999) as f64 / 999.0) * (extent.saturating_sub(1) as f64))
        .clamp(0.0, extent.saturating_sub(1) as f64)
}

#[allow(clippy::too_many_arguments)]
async fn mouse_event(
    cdp: &mut CdpClient,
    event_type: &str,
    x: f64,
    y: f64,
    button: &str,
    click_count: u64,
    delta_x: f64,
    delta_y: f64,
) -> Result<(), String> {
    let mut params =
        json!({"type": event_type, "x": x, "y": y, "button": button, "clickCount": click_count});
    if event_type == "mouseWheel" {
        params["deltaX"] = json!(delta_x);
        params["deltaY"] = json!(delta_y);
    }
    cdp.call("Input.dispatchMouseEvent", params)
        .await
        .map(|_| ())
}

fn key_description(input: &str) -> (String, String, u64) {
    let normalized = input.trim();
    match normalized.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => ("Control".into(), "ControlLeft".into(), 17),
        "shift" => ("Shift".into(), "ShiftLeft".into(), 16),
        "alt" => ("Alt".into(), "AltLeft".into(), 18),
        "meta" | "win" | "windows" => ("Meta".into(), "MetaLeft".into(), 91),
        "enter" | "return" => ("Enter".into(), "Enter".into(), 13),
        "tab" => ("Tab".into(), "Tab".into(), 9),
        "escape" | "esc" => ("Escape".into(), "Escape".into(), 27),
        "backspace" => ("Backspace".into(), "Backspace".into(), 8),
        "delete" => ("Delete".into(), "Delete".into(), 46),
        "arrowup" | "up" => ("ArrowUp".into(), "ArrowUp".into(), 38),
        "arrowdown" | "down" => ("ArrowDown".into(), "ArrowDown".into(), 40),
        "arrowleft" | "left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37),
        "arrowright" | "right" => ("ArrowRight".into(), "ArrowRight".into(), 39),
        "home" => ("Home".into(), "Home".into(), 36),
        "end" => ("End".into(), "End".into(), 35),
        "pageup" => ("PageUp".into(), "PageUp".into(), 33),
        "pagedown" => ("PageDown".into(), "PageDown".into(), 34),
        value if value.len() == 1 => {
            let character = value.chars().next().unwrap();
            if character.is_ascii_alphabetic() {
                (
                    value.into(),
                    format!("Key{}", character.to_ascii_uppercase()),
                    character.to_ascii_uppercase() as u64,
                )
            } else if character.is_ascii_digit() {
                (value.into(), format!("Digit{character}"), character as u64)
            } else {
                (value.into(), String::new(), character as u64)
            }
        }
        _ => (normalized.to_string(), normalized.to_string(), 0),
    }
}

fn modifier_mask(keys: &HashSet<String>) -> u64 {
    let lower = keys
        .iter()
        .map(|key| key.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    (if lower.contains("alt") { 1 } else { 0 })
        | (if lower.contains("control") || lower.contains("ctrl") {
            2
        } else {
            0
        })
        | (if lower.contains("meta") || lower.contains("win") || lower.contains("windows") {
            4
        } else {
            0
        })
        | (if lower.contains("shift") { 8 } else { 0 })
}

async fn dispatch_key(
    cdp: &mut CdpClient,
    held_keys: &mut HashSet<String>,
    input: &str,
    down: bool,
) -> Result<(), String> {
    let (key, code, virtual_key) = key_description(input);
    let canonical = key.clone();
    if down {
        held_keys.insert(canonical.clone());
    }
    let params = json!({"type": if down {"keyDown"} else {"keyUp"}, "key": key, "code": code,
        "windowsVirtualKeyCode": virtual_key, "nativeVirtualKeyCode": virtual_key, "modifiers": modifier_mask(held_keys)});
    let result = cdp.call("Input.dispatchKeyEvent", params).await.map(|_| ());
    if !down {
        held_keys.remove(&canonical);
    }
    result
}

async fn execute_action(
    cdp: &mut CdpClient,
    session: &mut BrowserSession,
    call: &Value,
    approved: bool,
) -> Result<(), String> {
    let name = call
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Action has no name".to_string())?;
    let args = call
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Action '{name}' arguments are invalid"))?;
    let safety = call
        .pointer("/safety_decision/decision")
        .or_else(|| call.pointer("/arguments/safety_decision/decision"))
        .and_then(Value::as_str);
    if safety == Some("blocked") {
        return Err(format!("Gemini safety blocked '{name}'"));
    }
    if safety == Some("require_confirmation") && !approved {
        return Err(format!("'{name}' requires user confirmation"));
    }
    let xy = |x_name: &str, y_name: &str| -> Result<(f64, f64), String> {
        let x = args
            .get(x_name)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("'{name}' requires {x_name}"))?;
        let y = args
            .get(y_name)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("'{name}' requires {y_name}"))?;
        Ok((
            scaled_coordinate(x, VIEWPORT_WIDTH),
            scaled_coordinate(y, VIEWPORT_HEIGHT),
        ))
    };
    match name {
        "click" | "double_click" | "triple_click" | "middle_click" | "right_click" => {
            let (x, y) = xy("x", "y")?;
            let button = if name == "middle_click" {
                "middle"
            } else if name == "right_click" {
                "right"
            } else {
                "left"
            };
            let count = if name == "double_click" {
                2
            } else if name == "triple_click" {
                3
            } else {
                1
            };
            mouse_event(cdp, "mousePressed", x, y, button, count, 0.0, 0.0).await?;
            mouse_event(cdp, "mouseReleased", x, y, button, count, 0.0, 0.0).await?;
        }
        "mouse_down" | "mouse_up" => {
            let (x, y) = xy("x", "y")?;
            mouse_event(
                cdp,
                if name == "mouse_down" {
                    "mousePressed"
                } else {
                    "mouseReleased"
                },
                x,
                y,
                "left",
                1,
                0.0,
                0.0,
            )
            .await?;
        }
        "move" => {
            let (x, y) = xy("x", "y")?;
            mouse_event(cdp, "mouseMoved", x, y, "none", 0, 0.0, 0.0).await?;
        }
        "type" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "type requires text".to_string())?;
            cdp.call("Input.insertText", json!({"text": text})).await?;
            if args.get("press_enter").and_then(Value::as_bool) == Some(true) {
                dispatch_key(cdp, &mut session.held_keys, "Enter", true).await?;
                dispatch_key(cdp, &mut session.held_keys, "Enter", false).await?;
            }
        }
        "drag_and_drop" => {
            let (start_x, start_y) = xy("start_x", "start_y")?;
            let (end_x, end_y) = xy("end_x", "end_y")?;
            mouse_event(cdp, "mouseMoved", start_x, start_y, "none", 0, 0.0, 0.0).await?;
            mouse_event(cdp, "mousePressed", start_x, start_y, "left", 1, 0.0, 0.0).await?;
            for step in 1..=8 {
                let ratio = step as f64 / 8.0;
                mouse_event(
                    cdp,
                    "mouseMoved",
                    start_x + (end_x - start_x) * ratio,
                    start_y + (end_y - start_y) * ratio,
                    "left",
                    1,
                    0.0,
                    0.0,
                )
                .await?;
            }
            mouse_event(cdp, "mouseReleased", end_x, end_y, "left", 1, 0.0, 0.0).await?;
        }
        "wait" => {
            tokio::time::sleep(Duration::from_secs(
                args.get("seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .min(30),
            ))
            .await
        }
        "press_key" | "key_down" | "key_up" => {
            let key = args
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{name} requires key"))?;
            if name != "key_up" {
                dispatch_key(cdp, &mut session.held_keys, key, true).await?;
            }
            if name != "key_down" {
                dispatch_key(cdp, &mut session.held_keys, key, false).await?;
            }
        }
        "hotkey" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(|| "hotkey requires keys".to_string())?;
            for key in keys {
                dispatch_key(
                    cdp,
                    &mut session.held_keys,
                    key.as_str().unwrap_or_default(),
                    true,
                )
                .await?;
            }
            for key in keys.iter().rev() {
                dispatch_key(
                    cdp,
                    &mut session.held_keys,
                    key.as_str().unwrap_or_default(),
                    false,
                )
                .await?;
            }
        }
        "take_screenshot" => {}
        "scroll" => {
            let (x, y) = xy("x", "y")?;
            let magnitude = args
                .get("magnitude_in_pixels")
                .and_then(Value::as_u64)
                .unwrap_or(300) as f64;
            let (dx, dy) = match args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down")
            {
                "up" => (0.0, -magnitude),
                "left" => (-magnitude, 0.0),
                "right" => (magnitude, 0.0),
                _ => (0.0, magnitude),
            };
            mouse_event(cdp, "mouseWheel", x, y, "none", 0, dx, dy).await?;
        }
        "navigate" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "navigate requires url".to_string())?;
            if !is_loopback_url(url) {
                return Err("Host blocked navigation outside the loopback allowlist".to_string());
            }
            cdp.call("Page.navigate", json!({"url": url})).await?;
        }
        "go_back" => {
            cdp.call("Runtime.evaluate", json!({"expression": "history.back()"}))
                .await?;
        }
        "go_forward" => {
            cdp.call(
                "Runtime.evaluate",
                json!({"expression": "history.forward()"}),
            )
            .await?;
        }
        _ => return Err(format!("Unsupported Computer Use action '{name}'")),
    }
    tokio::time::sleep(Duration::from_millis(350)).await;
    let state = environment_state(cdp, session).await?;
    let current_url = state
        .get("current_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !is_loopback_url(current_url) && current_url != "about:blank" {
        cdp.call("Page.navigate", json!({"url": session.last_allowed_url}))
            .await?;
        return Err(format!(
            "Host blocked a transition outside the loopback allowlist: {current_url}"
        ));
    }
    if is_loopback_url(current_url) {
        session.last_allowed_url = current_url.to_string();
    }
    Ok(())
}

async fn handle_browser_start(
    client: &Client,
    browser: &mut Option<BrowserSession>,
    command: &Value,
) -> Result<Value, String> {
    if browser.is_some() {
        return Err("A browser session is already active".to_string());
    }
    let session_id = command
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Start command has no session_id".to_string())?;
    let url = command
        .get("local_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Start command has no local_url".to_string())?;
    let mut session = start_browser(client, session_id, url).await?;
    let initial_result = async {
        let mut cdp = session.connect(client).await?;
        let shot = screenshot(&mut cdp).await?;
        let state = environment_state(&mut cdp, &session).await?;
        Ok::<_, String>((shot, state))
    }
    .await;
    let (shot, state) = match initial_result {
        Ok(result) => result,
        Err(error) => {
            session.stop(client).await;
            return Err(error);
        }
    };
    *browser = Some(session);
    Ok(
        json!({"status": "success", "environment": "browser", "viewport": {"width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT, "device_scale_factor": 1}, "screenshot": shot, "environment_state": state}),
    )
}

async fn handle_browser_batch(
    client: &Client,
    browser: &mut Option<BrowserSession>,
    command: &Value,
) -> Result<Value, String> {
    let session = browser
        .as_mut()
        .ok_or_else(|| "No browser session is active".to_string())?;
    if command.get("session_id").and_then(Value::as_str) != Some(&session.session_id) {
        return Err("Batch session_id does not match the browser".to_string());
    }
    let approved = command
        .get("approved_by_user")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let calls = command
        .get("calls")
        .and_then(Value::as_array)
        .ok_or_else(|| "Batch has no calls".to_string())?;
    let mut cdp = session.connect(client).await?;
    let mut results = Vec::new();
    let mut failed = false;
    for call in calls {
        let call_id = call
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or("computer_call_unknown");
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown_computer_action");
        if failed {
            results.push(json!({"call_id": call_id, "name": name, "status": "skipped", "error": "Not executed because a previous action in the batch failed"}));
            continue;
        }
        let action_result = execute_action(&mut cdp, session, call, approved).await;
        let shot = screenshot(&mut cdp).await;
        match (action_result, shot) {
            (Ok(()), Ok(shot)) => results.push(
                json!({"call_id": call_id, "name": name, "status": "success", "screenshot": shot}),
            ),
            (action, shot) => {
                failed = true;
                let error = action
                    .err()
                    .or_else(|| shot.err())
                    .unwrap_or_else(|| "Unknown action failure".to_string());
                results.push(
                    json!({"call_id": call_id, "name": name, "status": "error", "error": error}),
                );
            }
        }
    }
    let state = environment_state(&mut cdp, session).await?;
    Ok(
        json!({"status": if failed {"error"} else {"success"}, "environment": "browser", "viewport": {"width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT, "device_scale_factor": 1}, "results": results, "environment_state": state}),
    )
}

async fn desktop_screenshot(hwnd: u64) -> Result<Value, String> {
    let shot = tokio::task::spawn_blocking(move || computer_desktop::capture_window(hwnd))
        .await
        .map_err(|error| format!("Desktop capture worker failed: {error}"))??;
    Ok(computer_desktop::screenshot_json(shot))
}

fn command_hwnd(command: &Value) -> Result<u64, String> {
    command
        .get("target_hwnd")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .filter(|value| *value != 0)
        .ok_or_else(|| "Desktop start requires a GUI-selected target_hwnd".to_string())
}

async fn handle_desktop_start(
    desktop: &mut Option<computer_desktop::DesktopSession>,
    command: &Value,
) -> Result<Value, String> {
    if desktop.is_some() {
        return Err("A desktop session is already active".to_string());
    }
    let session_id = command
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Start command has no session_id".to_string())?;
    let hwnd = command_hwnd(command)?;
    let mut session = computer_desktop::start_session(session_id.to_string(), hwnd)?;
    let shot = match desktop_screenshot(hwnd).await {
        Ok(shot) => shot,
        Err(error) => {
            session.release_inputs();
            return Err(error);
        }
    };
    session.capture_width = shot.get("width").and_then(Value::as_u64).unwrap_or(1) as u32;
    session.capture_height = shot.get("height").and_then(Value::as_u64).unwrap_or(1) as u32;
    let state = session.environment_state()?;
    let viewport = json!({"width": session.capture_width, "height": session.capture_height,
        "device_scale_factor": state.get("dpi").and_then(Value::as_u64).unwrap_or(96) as f64 / 96.0});
    *desktop = Some(session);
    Ok(
        json!({"status": "success", "environment": "desktop", "viewport": viewport,
        "screenshot": shot, "environment_state": state}),
    )
}

async fn handle_desktop_batch(
    desktop: &mut Option<computer_desktop::DesktopSession>,
    command: &Value,
) -> Result<Value, String> {
    let session = desktop
        .as_mut()
        .ok_or_else(|| "No desktop session is active".to_string())?;
    if command.get("session_id").and_then(Value::as_str) != Some(&session.session_id) {
        return Err("Batch session_id does not match the desktop window".to_string());
    }
    let approved = command
        .get("approved_by_user")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let calls = command
        .get("calls")
        .and_then(Value::as_array)
        .ok_or_else(|| "Batch has no calls".to_string())?;
    let mut results = Vec::new();
    let mut failed = false;
    for call in calls {
        let call_id = call
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or("computer_call_unknown");
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown_computer_action");
        if failed {
            results.push(
                json!({"call_id": call_id, "name": name, "status": "skipped",
                "error": "Not executed because a previous action in the batch failed"}),
            );
            continue;
        }
        let action_result = session.execute_action(call, approved);
        let shot = desktop_screenshot(session.capture_target_hwnd()).await;
        match (action_result, shot) {
            (Ok(()), Ok(shot)) => {
                session.capture_width =
                    shot.get("width").and_then(Value::as_u64).unwrap_or(1) as u32;
                session.capture_height =
                    shot.get("height").and_then(Value::as_u64).unwrap_or(1) as u32;
                results.push(json!({"call_id": call_id, "name": name, "status": "success", "screenshot": shot}));
            }
            (action, shot) => {
                failed = true;
                let error = action
                    .err()
                    .or_else(|| shot.err())
                    .unwrap_or_else(|| "Unknown desktop action failure".to_string());
                results.push(
                    json!({"call_id": call_id, "name": name, "status": "error", "error": error}),
                );
            }
        }
    }
    let state_result = session.environment_state();
    let terminal = state_result.is_err();
    let state = state_result.unwrap_or_else(|error| json!({"terminal_error": error}));
    let viewport = json!({"width": session.capture_width, "height": session.capture_height,
        "device_scale_factor": state.get("dpi").and_then(Value::as_u64).unwrap_or(96) as f64 / 96.0});
    Ok(
        json!({"status": if failed {"error"} else {"success"}, "environment": "desktop",
        "viewport": viewport, "results": results, "environment_state": state, "terminal": terminal}),
    )
}

struct DirectSession {
    session_id: String,
    environment: String,
    sequence: u64,
    started_at: Instant,
    current_url: Option<String>,
    target_label: String,
}

struct McpHost {
    client: Client,
    browser: Option<BrowserSession>,
    desktop: Option<computer_desktop::DesktopSession>,
    session: Option<DirectSession>,
    completed: HashMap<String, Value>,
    completed_order: VecDeque<String>,
}

fn computer_start_tool() -> Value {
    json!({
        "name": "computer_start",
        "title": "Start Gemini Computer Use",
        "description": "Start a localhost-only isolated browser or ask the signed-in user to select one desktop window, then return its initial screenshot.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "environment": {"type": "string", "enum": ["browser", "desktop"], "default": "browser"},
                "local_url": {"type": "string", "description": "Initial http:// or https:// loopback URL; required for browser."}
            },
            "additionalProperties": false
        },
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
    })
}

fn computer_action_batch_tool() -> Value {
    json!({
        "name": "computer_action_batch",
        "title": "Execute a Gemini Computer Use action batch",
        "description": "Execute a bridge-authenticated batch of native Gemini UI calls strictly in order. Do not construct this input manually; it is emitted by the bridge from Gemini Computer Use output.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "protocol_version": {"type": "string", "const": PROTOCOL_VERSION},
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

fn computer_cancel_tool() -> Value {
    json!({
        "name": "computer_cancel",
        "title": "Stop Gemini Computer Use",
        "description": "Immediately stop the active isolated Computer Use session.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn computer_loopback_url(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|error| format!("Invalid local_url: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.host().is_some_and(|host| match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        })
    {
        return Err("Computer Use version 1 only permits http(s) loopback URLs (localhost, 127.0.0.1, or ::1)".to_string());
    }
    Ok(url.to_string())
}

fn computer_coordinate(
    arguments: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<u64, String> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .filter(|value| *value <= 999)
        .ok_or_else(|| {
            format!("Computer action field '{name}' must be an integer from 0 through 999")
        })
}

fn validate_computer_action(call: &Value, environment: &str) -> Result<(), String> {
    let call_id = call
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Computer action call_id is required".to_string())?;
    let name = call
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Computer action '{call_id}' has no name"))?;
    let arguments = call
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Computer action '{call_id}' arguments must be an object"))?;
    if arguments
        .get("intent")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(format!(
            "Computer action '{call_id}' is missing its Gemini intent"
        ));
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
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "Computer type action requires text".to_string())?;
            if text.encode_utf16().count() > 4_000 {
                return Err(
                    "Computer type text exceeds the 4000 UTF-16 unit safety limit".to_string(),
                );
            }
            if arguments
                .get("press_enter")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err("Computer type press_enter must be boolean".to_string());
            }
        }
        "drag_and_drop" => {
            coordinate_pair("start_x", "start_y")?;
            coordinate_pair("end_x", "end_y")?;
        }
        "wait" => {
            if arguments
                .get("seconds")
                .is_some_and(|value| value.as_u64().is_none_or(|seconds| seconds > 30))
            {
                return Err(
                    "Computer wait seconds must be an integer from 0 through 30".to_string()
                );
            }
        }
        "press_key" | "key_down" | "key_up" => {
            arguments
                .get("key")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Computer {name} requires key"))?;
        }
        "hotkey" => {
            arguments
                .get("keys")
                .and_then(Value::as_array)
                .filter(|keys| {
                    !keys.is_empty()
                        && keys
                            .iter()
                            .all(|key| key.as_str().is_some_and(|value| !value.is_empty()))
                })
                .ok_or_else(|| {
                    "Computer hotkey requires a non-empty string array 'keys'".to_string()
                })?;
        }
        "take_screenshot" | "go_back" | "go_forward" => {}
        "scroll" => {
            coordinate_pair("x", "y")?;
            if !matches!(
                arguments.get("direction").and_then(Value::as_str),
                Some("up" | "down" | "left" | "right")
            ) {
                return Err(
                    "Computer scroll direction must be up, down, left, or right".to_string()
                );
            }
            if arguments
                .get("magnitude_in_pixels")
                .is_some_and(|value| value.as_u64().is_none_or(|pixels| pixels > 999))
            {
                return Err(
                    "Computer scroll magnitude_in_pixels must be an integer from 0 through 999"
                        .to_string(),
                );
            }
        }
        "navigate" => {
            let target = arguments
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "Computer navigate requires url".to_string())?;
            computer_loopback_url(target)?;
        }
        _ => return Err(format!("Unsupported Gemini Computer Use action '{name}'")),
    }
    if environment == "desktop" && matches!(name, "go_back" | "navigate" | "go_forward") {
        return Err(format!(
            "Gemini desktop environment does not support '{name}'"
        ));
    }
    Ok(())
}

fn computer_batch_requires_confirmation(
    calls: &[Value],
    current_url: Option<&str>,
) -> Result<Option<String>, String> {
    let on_loopback = current_url
        .and_then(|value| url::Url::parse(value).ok())
        .is_some_and(|url| {
            url.host().is_some_and(|host| match host {
                url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            })
        });
    let mut reasons = Vec::new();
    for call in calls {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let decision = call
            .pointer("/safety_decision/decision")
            .or_else(|| call.pointer("/arguments/safety_decision/decision"))
            .and_then(Value::as_str);
        if decision == Some("blocked") {
            return Err(format!(
                "Gemini safety policy blocked Computer Use action '{name}'"
            ));
        }
        if decision == Some("require_confirmation") {
            let explanation = call
                .pointer("/safety_decision/explanation")
                .or_else(|| call.pointer("/arguments/safety_decision/explanation"))
                .and_then(Value::as_str)
                .unwrap_or("Gemini requires confirmation");
            reasons.push(format!("{name}: {explanation}"));
            continue;
        }
        let locally_low_risk = matches!(name, "take_screenshot" | "wait" | "move")
            || (on_loopback
                && matches!(
                    name,
                    "click" | "double_click" | "triple_click" | "scroll" | "go_back" | "go_forward"
                ));
        if !locally_low_risk {
            reasons.push(format!(
                "{name}: local policy requires a real user confirmation"
            ));
        }
    }
    Ok((!reasons.is_empty()).then(|| reasons.join("\n")))
}

fn computer_result_content(mut result: Value) -> Result<Value, String> {
    let is_error = result.get("status").and_then(Value::as_str) != Some("success");
    let result_object = result
        .as_object_mut()
        .ok_or_else(|| "Computer Host returned a non-object result".to_string())?;
    let mut screenshots = Vec::new();
    if let Some(results) = result_object
        .get_mut("results")
        .and_then(Value::as_array_mut)
    {
        for item in results {
            if let Some(screenshot) = item.get_mut("screenshot").and_then(Value::as_object_mut) {
                if let Some(data) = screenshot
                    .remove("data")
                    .and_then(|value| value.as_str().map(str::to_string))
                {
                    screenshot.insert("content_index".to_string(), json!(screenshots.len() + 1));
                    screenshots.push(data);
                }
            }
        }
    }
    if let Some(screenshot) = result_object
        .get_mut("screenshot")
        .and_then(Value::as_object_mut)
    {
        if let Some(data) = screenshot
            .remove("data")
            .and_then(|value| value.as_str().map(str::to_string))
        {
            screenshot.insert("content_index".to_string(), json!(screenshots.len() + 1));
            screenshots.push(data);
        }
    }
    let text = serde_json::to_string(&result)
        .map_err(|error| format!("Cannot serialize Computer Host result: {error}"))?;
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend(
        screenshots
            .into_iter()
            .map(|data| json!({"type": "image", "data": data, "mimeType": "image/png"})),
    );
    Ok(json!({"content": content, "structuredContent": result, "isError": is_error}))
}

fn mcp_tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{"type": "text", "text": message.into()}],
        "isError": true
    })
}

impl McpHost {
    fn new() -> Result<Self, String> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(40))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            browser: None,
            desktop: None,
            session: None,
            completed: HashMap::new(),
            completed_order: VecDeque::new(),
        })
    }

    async fn cleanup(&mut self) {
        if let Some(mut session) = self.browser.take() {
            session.stop(&self.client).await;
        }
        if let Some(mut session) = self.desktop.take() {
            session.release_inputs();
        }
        self.session = None;
        self.completed.clear();
        self.completed_order.clear();
    }

    async fn start(&mut self, arguments: Option<&Value>) -> Result<Value, String> {
        let arguments = arguments
            .and_then(Value::as_object)
            .ok_or_else(|| "computer_start arguments must be an object".to_string())?;
        if self.session.is_some() || self.browser.is_some() || self.desktop.is_some() {
            return Err(
                "Only one Computer Use session may be active; cancel it before starting another"
                    .to_string(),
            );
        }
        let environment = arguments
            .get("environment")
            .and_then(Value::as_str)
            .unwrap_or("browser");
        if !matches!(environment, "browser" | "desktop") {
            return Err("computer_start environment must be browser or desktop".to_string());
        }
        let session_id = format!("cus_{}", Uuid::new_v4().simple());
        let mut command = json!({
            "protocol_version": PROTOCOL_VERSION,
            "type": "start",
            "session_id": session_id,
            "environment": environment,
            "viewport": {"width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT, "device_scale_factor": 1}
        });
        let (local_url, target_label) = if environment == "browser" {
            let local_url = computer_loopback_url(
                arguments
                    .get("local_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "computer_start requires local_url for browser".to_string())?,
            )?;
            command["local_url"] = json!(local_url);
            (Some(local_url.clone()), local_url)
        } else {
            let window = computer_desktop::select_window_interactively()?;
            command["target_hwnd"] = json!(window.hwnd.to_string());
            (
                None,
                format!("{} (PID {})", window.title.trim(), window.pid),
            )
        };
        let mut result = if environment == "desktop" {
            handle_desktop_start(&mut self.desktop, &command).await?
        } else {
            handle_browser_start(&self.client, &mut self.browser, &command).await?
        };
        if result.get("status").and_then(Value::as_str) != Some("success") {
            self.cleanup().await;
            return Err(result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Computer Host failed to start Computer Use")
                .to_string());
        }
        result["kind"] = json!("computer_start_result");
        result["protocol_version"] = json!(PROTOCOL_VERSION);
        result["session_id"] = json!(session_id);
        result["sequence"] = json!(0);
        self.session = Some(DirectSession {
            session_id,
            environment: environment.to_string(),
            sequence: 0,
            started_at: Instant::now(),
            current_url: local_url,
            target_label,
        });
        computer_result_content(result)
    }

    async fn action_batch(&mut self, arguments: Option<&Value>) -> Result<Value, String> {
        let arguments = arguments
            .and_then(Value::as_object)
            .ok_or_else(|| "computer_action_batch arguments must be an object".to_string())?;
        if arguments.get("protocol_version").and_then(Value::as_str) != Some(PROTOCOL_VERSION) {
            return Err("Unsupported Computer Use protocol_version".to_string());
        }
        let session_id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "computer_action_batch requires session_id".to_string())?;
        let batch_id = arguments
            .get("batch_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "computer_action_batch requires batch_id".to_string())?;
        let sequence = arguments
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "computer_action_batch requires sequence".to_string())?;
        let environment = arguments
            .get("environment")
            .and_then(Value::as_str)
            .ok_or_else(|| "computer_action_batch requires environment".to_string())?;
        let calls = arguments
            .get("calls")
            .and_then(Value::as_array)
            .filter(|calls| !calls.is_empty())
            .ok_or_else(|| "computer_action_batch requires calls".to_string())?;
        for call in calls {
            validate_computer_action(call, environment)?;
        }
        let command_id = format!("batch:{session_id}:{batch_id}");
        if let Some(result) = self.completed.get(&command_id).cloned() {
            return computer_result_content(result);
        }
        let session = self
            .session
            .as_ref()
            .filter(|session| session.session_id == session_id)
            .ok_or_else(|| {
                "Computer Use session is not active or session_id does not match".to_string()
            })?;
        if session.environment != environment {
            return Err("Computer Use environment does not match the active session".to_string());
        }
        if sequence != session.sequence + 1 {
            return Err(format!(
                "Computer Use sequence must be {}, received {sequence}",
                session.sequence + 1
            ));
        }
        if sequence > 50 {
            return Err("Computer Use reached the 50-step safety limit".to_string());
        }
        if session.started_at.elapsed() > Duration::from_secs(900) {
            return Err("Computer Use session exceeded the 15-minute safety timeout".to_string());
        }
        let confirmation_reason =
            computer_batch_requires_confirmation(calls, session.current_url.as_deref())?;
        let approved = if let Some(reason) = confirmation_reason.as_ref() {
            let intents = calls
                .iter()
                .filter_map(|call| {
                    call.get("intent")
                        .or_else(|| call.pointer("/arguments/intent"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let message = format!(
                "Claude Code 请求执行以下 Computer Use 动作。\n\n目标：{}\n\n意图：\n{}\n\n需要确认：\n{}\n\n选择“是”仅允许本批动作；选择“否”拒绝。",
                session.target_label, intents, reason
            );
            if !computer_desktop::confirm_interactively(
                "Claude Code Computer Use - 安全确认",
                &message,
            ) {
                return Err("Computer Use action was rejected by the user".to_string());
            }
            true
        } else {
            false
        };
        let mut command = Value::Object(arguments.clone());
        command["type"] = json!("action_batch");
        command["approved_by_user"] = json!(approved);
        let mut result = if environment == "desktop" {
            handle_desktop_batch(&mut self.desktop, &command).await?
        } else {
            handle_browser_batch(&self.client, &mut self.browser, &command).await?
        };
        result["kind"] = json!("computer_action_batch_result");
        result["protocol_version"] = json!(PROTOCOL_VERSION);
        result["session_id"] = json!(session_id);
        result["batch_id"] = json!(batch_id);
        result["sequence"] = json!(sequence);
        if approved {
            if let Some(results) = result.get_mut("results").and_then(Value::as_array_mut) {
                for (call, item) in calls.iter().zip(results.iter_mut()) {
                    let required = call
                        .pointer("/safety_decision/decision")
                        .or_else(|| call.pointer("/arguments/safety_decision/decision"))
                        .and_then(Value::as_str)
                        == Some("require_confirmation");
                    if required && item.get("status").and_then(Value::as_str) == Some("success") {
                        item["safety_acknowledgement"] = json!(true);
                    }
                }
            }
        }
        let terminal = result.get("terminal").and_then(Value::as_bool) == Some(true);
        if let Some(session) = self.session.as_mut() {
            session.sequence = sequence;
            if let Some(current_url) = result
                .pointer("/environment_state/current_url")
                .and_then(Value::as_str)
            {
                session.current_url = Some(current_url.to_string());
            }
        }
        self.completed.insert(command_id.clone(), result.clone());
        self.completed_order.push_back(command_id);
        while self.completed_order.len() > MAX_COMPLETED_BATCHES {
            if let Some(oldest) = self.completed_order.pop_front() {
                self.completed.remove(&oldest);
            }
        }
        let content = computer_result_content(result)?;
        if terminal {
            self.cleanup().await;
        }
        Ok(content)
    }

    async fn cancel(&mut self) -> Value {
        self.cleanup().await;
        json!({
            "content": [{"type": "text", "text": "Computer Use session stopped"}],
            "structuredContent": {"kind": "computer_cancel_result", "status": "success"},
            "isError": false
        })
    }

    async fn call_tool(&mut self, request: &Value) -> Result<Value, String> {
        match request.pointer("/params/name").and_then(Value::as_str) {
            Some("computer_start") => self.start(request.pointer("/params/arguments")).await,
            Some("computer_action_batch") => {
                self.action_batch(request.pointer("/params/arguments"))
                    .await
            }
            Some("computer_cancel") => Ok(self.cancel().await),
            Some(name) => Err(format!("Unknown Computer Host tool '{name}'")),
            None => Err("Tool name is required".to_string()),
        }
    }
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

async fn handle_mcp_request(host: &mut McpHost, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(jsonrpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid JSON-RPC request",
        ));
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?;
    let response = match method {
        "initialize" => {
            let requested = request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let protocol_version = match requested {
                "2025-11-25" | "2025-06-18" | "2025-03-26" => requested,
                _ => MCP_PROTOCOL_VERSION,
            };
            jsonrpc_result(
                id,
                json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "claude-code-gemini-computer", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Use computer_start, then execute bridge-generated computer_action_batch calls. The server is local, single-session, and user-confirmed for sensitive actions."
                }),
            )
        }
        "ping" => jsonrpc_result(id, json!({})),
        "tools/list" => jsonrpc_result(
            id,
            json!({"tools": [computer_start_tool(), computer_action_batch_tool(), computer_cancel_tool()]}),
        ),
        "tools/call" => match host.call_tool(request).await {
            Ok(result) => jsonrpc_result(id, result),
            Err(message) => jsonrpc_result(id, mcp_tool_error(message)),
        },
        _ => jsonrpc_error(id, -32601, "Method not found"),
    };
    Some(response)
}

async fn run_stdio_mcp() -> Result<(), String> {
    // The embedded manifest already requests PerMonitorV2. Windows returns
    // access denied when code tries to set the same awareness after startup.
    let _ = computer_desktop::enable_per_monitor_v2();
    let mut host = McpHost::new()?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("Cannot read MCP stdin: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > 16 * 1024 * 1024 {
            let response = jsonrpc_error(Value::Null, -32600, "MCP request is too large");
            writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .map_err(|error| format!("Cannot write MCP stdout: {error}"))?;
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let response = jsonrpc_error(Value::Null, -32700, format!("Parse error: {error}"));
                writeln!(stdout, "{response}")
                    .and_then(|_| stdout.flush())
                    .map_err(|error| format!("Cannot write MCP stdout: {error}"))?;
                continue;
            }
        };
        if let Some(response) = handle_mcp_request(&mut host, &request).await {
            writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .map_err(|error| format!("Cannot write MCP stdout: {error}"))?;
        }
    }
    host.cleanup().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run_stdio_mcp().await {
        eprintln!("Computer Host failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_server_exposes_only_direct_computer_tools() {
        let tools = [
            computer_start_tool(),
            computer_action_batch_tool(),
            computer_cancel_tool(),
        ];
        assert_eq!(
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            ["computer_start", "computer_action_batch", "computer_cancel"]
        );
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["environment"]["enum"],
            json!(["browser", "desktop"])
        );
    }

    #[test]
    fn browser_navigation_is_strictly_loopback() {
        for allowed in [
            "http://localhost:3000/",
            "https://127.0.0.1/test",
            "http://[::1]:8765/",
        ] {
            computer_loopback_url(allowed).unwrap();
        }
        for denied in [
            "https://example.com/",
            "file:///C:/Windows/System32/notepad.exe",
            "javascript:alert(1)",
        ] {
            assert!(computer_loopback_url(denied).is_err(), "{denied}");
        }
    }

    #[test]
    fn action_validation_and_confirmation_are_host_owned() {
        let screenshot = json!({
            "call_id": "call_1",
            "name": "take_screenshot",
            "arguments": {"intent": "inspect the selected window"}
        });
        validate_computer_action(&screenshot, "desktop").unwrap();
        assert_eq!(
            computer_batch_requires_confirmation(&[screenshot], None).unwrap(),
            None
        );

        let typing = json!({
            "call_id": "call_2",
            "name": "type",
            "arguments": {"text": "hello", "intent": "enter a value"}
        });
        validate_computer_action(&typing, "desktop").unwrap();
        assert!(computer_batch_requires_confirmation(&[typing], None)
            .unwrap()
            .is_some());

        let blocked = json!({
            "call_id": "call_3",
            "name": "click",
            "arguments": {"x": 1, "y": 2, "intent": "unsafe click"},
            "safety_decision": {"decision": "blocked"}
        });
        assert!(computer_batch_requires_confirmation(&[blocked], None).is_err());
    }
}
