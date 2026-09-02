//! Desktop-side implementation for the per-user Computer Host.
//!
//! The service never sends an arbitrary coordinate target to this module. A session is bound to
//! one explicitly selected top-level HWND and its PID, and every input is revalidated immediately
//! before SendInput is called.

use std::{
    collections::HashSet,
    mem::size_of,
    thread,
    time::{Duration, Instant},
};

use png::{BitDepth, ColorType, Encoder};
use serde_json::{json, Value};
use windows::{
    core::{factory, Interface, HSTRING, PWSTR},
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
                D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
            Dxgi::{Common::DXGI_SAMPLE_DESC, IDXGIDevice},
        },
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            StationsAndDesktops::{
                CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS,
                DESKTOP_SWITCHDESKTOP, UOI_NAME,
            },
            Threading::{
                OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
            WinRT::{
                Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
                Graphics::Capture::IGraphicsCaptureItemInterop,
                RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
            },
        },
        UI::{
            HiDpi::{
                GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
                KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
                MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
                MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY, VK_BACK,
                VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_LEFT,
                VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE,
                VK_TAB, VK_UP,
            },
            WindowsAndMessaging::{
                EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetSystemMetrics,
                GetWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
                GetWindowThreadProcessId, IsChild, IsIconic, IsWindow, IsWindowVisible,
                MessageBoxW, SetForegroundWindow, WindowFromPoint, GA_ROOTOWNER, GW_OWNER,
                IDCANCEL, IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
                MB_YESNOCANCEL, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN,
            },
        },
    },
};

const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const SENSITIVE_MARKERS: &[&str] = &[
    "1password",
    "bitwarden",
    "keepass",
    "lastpass",
    "dashlane",
    "password safe",
    "password manager",
    "credential",
    "windows security",
    "securityhealth",
    "uac",
    "credentialuibroker",
    "consent.exe",
    "logonui",
    "program manager",
    "progman",
    "workerw",
    "shell_traywnd",
];

#[derive(Clone, Debug)]
pub struct DesktopWindow {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
    pub class_name: String,
    pub process_path: String,
    pub rect: RECT,
    pub dpi: u32,
    pub eligible: bool,
    pub blocked_reason: Option<String>,
}

pub struct DesktopSession {
    pub session_id: String,
    pub hwnd: u64,
    pub pid: u32,
    pub process_path: String,
    pub capture_width: u32,
    pub capture_height: u32,
    held_keys: HashSet<u16>,
    mouse_down: bool,
}

pub struct DesktopScreenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn hwnd_value(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}
fn hwnd_from_value(value: u64) -> HWND {
    HWND(value as usize as *mut core::ffi::c_void)
}

pub fn enable_per_monitor_v2() -> Result<(), String> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .map_err(|error| format!("Cannot enable PerMonitorV2 DPI awareness: {error}"))
}

fn window_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = vec![0u16; 256];
    let copied = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn visual_window_rect(hwnd: HWND) -> Result<RECT, String> {
    let mut rect = RECT::default();
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if dwm.is_err() {
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .map_err(|error| format!("Cannot read window bounds: {error}"))?;
    }
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Err("The selected window has invalid bounds".to_string());
    }
    Ok(rect)
}

fn process_path(pid: u32) -> Result<String, String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| format!("Cannot inspect process {pid}: {error}"))?;
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    unsafe {
        let _ = CloseHandle(process);
    }
    result.map_err(|error| format!("Cannot query process {pid} path: {error}"))?;
    Ok(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn process_is_elevated(pid: u32) -> Result<bool, String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| format!("Cannot inspect process {pid}: {error}"))?;
    let mut token = HANDLE::default();
    let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    unsafe {
        let _ = CloseHandle(process);
    }
    opened.map_err(|error| format!("Cannot inspect process {pid} token: {error}"))?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    result.map_err(|error| format!("Cannot read process {pid} elevation: {error}"))?;
    Ok(elevation.TokenIsElevated != 0)
}

fn sensitive_reason(title: &str, class_name: &str, path: &str) -> Option<String> {
    let combined = format!("{title}\n{class_name}\n{path}").to_ascii_lowercase();
    SENSITIVE_MARKERS
        .iter()
        .find(|marker| combined.contains(**marker))
        .map(|marker| format!("Sensitive application marker '{marker}' is blocked"))
}

unsafe extern "system" fn enumerate_callback(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    windows.push(hwnd);
    true.into()
}

pub fn list_windows() -> Vec<DesktopWindow> {
    let mut handles = Vec::<HWND>::new();
    let _ = unsafe {
        EnumWindows(
            Some(enumerate_callback),
            LPARAM((&mut handles as *mut Vec<HWND>) as isize),
        )
    };
    let own_pid = std::process::id();
    handles
        .into_iter()
        .filter_map(|hwnd| {
            if !unsafe { IsWindowVisible(hwnd).as_bool() } || unsafe { IsIconic(hwnd).as_bool() } {
                return None;
            }
            let title = window_text(hwnd);
            if title.trim().is_empty() {
                return None;
            }
            let pid = window_pid(hwnd);
            if pid == 0 || pid == own_pid {
                return None;
            }
            let class_name = window_class(hwnd);
            let mut cloaked = 0u32;
            if unsafe {
                DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED,
                    (&mut cloaked as *mut u32).cast(),
                    size_of::<u32>() as u32,
                )
            }
            .is_ok()
                && cloaked != 0
            {
                return None;
            }
            let rect = visual_window_rect(hwnd).ok()?;
            let path_result = process_path(pid);
            let process_path = path_result.clone().unwrap_or_default();
            let mut blocked_reason = path_result.err();
            if blocked_reason.is_none() {
                blocked_reason = sensitive_reason(&title, &class_name, &process_path);
            }
            if blocked_reason.is_none() {
                blocked_reason = match process_is_elevated(pid) {
                    Ok(true) => Some("Elevated windows are blocked by UIPI policy".to_string()),
                    Ok(false) => None,
                    Err(error) => Some(error),
                };
            }
            Some(DesktopWindow {
                hwnd: hwnd_value(hwnd),
                pid,
                title,
                class_name,
                process_path,
                rect,
                dpi: unsafe { GetDpiForWindow(hwnd) },
                eligible: blocked_reason.is_none(),
                blocked_reason,
            })
        })
        .collect()
}

/// Ask the signed-in user to bind the session to one eligible top-level window.
/// This dialog is intentionally owned by the stdio MCP process rather than the model center.
pub fn select_window_interactively() -> Result<DesktopWindow, String> {
    let windows = list_windows()
        .into_iter()
        .filter(|window| window.eligible)
        .collect::<Vec<_>>();
    if windows.is_empty() {
        return Err("No eligible non-elevated desktop window is available".to_string());
    }
    let total = windows.len();
    for (index, window) in windows.into_iter().enumerate() {
        let message = HSTRING::from(format!(
            "Claude Code 请求操作一个桌面窗口。\n\n窗口：{}\n进程：{}\n窗口类：{}\nPID：{}\nDPI：{}\n\n候选窗口 {} / {}\n\n选择“是”绑定此窗口；选择“否”查看下一个；选择“取消”拒绝。",
            window.title,
            window.process_path,
            window.class_name,
            window.pid,
            window.dpi,
            index + 1,
            total
        ));
        let caption = HSTRING::from("Claude Code Computer Use - 选择窗口");
        let answer = unsafe {
            MessageBoxW(
                None,
                &message,
                &caption,
                MB_YESNOCANCEL | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
            )
        };
        if answer == IDYES {
            return Ok(window);
        }
        if answer == IDCANCEL {
            return Err("Desktop window selection was cancelled by the user".to_string());
        }
    }
    Err("Desktop window selection was declined by the user".to_string())
}

/// Display a one-shot safety approval owned by the stdio MCP process.
pub fn confirm_interactively(title: &str, message: &str) -> bool {
    let caption = HSTRING::from(title);
    let message = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            &message,
            &caption,
            MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
        ) == IDYES
    }
}

pub fn find_window(hwnd: u64) -> Result<DesktopWindow, String> {
    list_windows()
        .into_iter()
        .find(|item| item.hwnd == hwnd)
        .ok_or_else(|| "The selected desktop window is no longer available".to_string())
}

pub fn start_session(session_id: String, hwnd: u64) -> Result<DesktopSession, String> {
    ensure_default_input_desktop()?;
    let window = find_window(hwnd)?;
    if let Some(reason) = window.blocked_reason {
        return Err(reason);
    }
    Ok(DesktopSession {
        session_id,
        hwnd,
        pid: window.pid,
        process_path: window.process_path,
        capture_width: (window.rect.right - window.rect.left) as u32,
        capture_height: (window.rect.bottom - window.rect.top) as u32,
        held_keys: HashSet::new(),
        mouse_down: false,
    })
}

fn ensure_default_input_desktop() -> Result<(), String> {
    let access = windows::Win32::System::StationsAndDesktops::DESKTOP_ACCESS_FLAGS(
        DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0,
    );
    let desktop = unsafe { OpenInputDesktop(Default::default(), false, access) }
        .map_err(|_| "Input desktop is unavailable (the workstation may be locked or on the UAC secure desktop)".to_string())?;
    let desktop_handle = HANDLE(desktop.0);
    let mut needed = 0u32;
    let _ =
        unsafe { GetUserObjectInformationW(desktop_handle, UOI_NAME, None, 0, Some(&mut needed)) };
    let mut bytes = vec![0u8; needed.max(2) as usize];
    let result = unsafe {
        GetUserObjectInformationW(
            desktop_handle,
            UOI_NAME,
            Some(bytes.as_mut_ptr().cast()),
            bytes.len() as u32,
            Some(&mut needed),
        )
    };
    unsafe {
        let _ = CloseDesktop(desktop);
    }
    result.map_err(|error| format!("Cannot inspect the input desktop: {error}"))?;
    let words =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), bytes.len() / 2) };
    let end = words
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(words.len());
    let name = String::from_utf16_lossy(&words[..end]);
    if !name.eq_ignore_ascii_case("default") {
        return Err(format!(
            "Desktop '{name}' is blocked; only the normal interactive desktop is allowed"
        ));
    }
    Ok(())
}

impl DesktopSession {
    fn hwnd(&self) -> HWND {
        hwnd_from_value(self.hwnd)
    }

    pub fn validate_target(&self) -> Result<RECT, String> {
        ensure_default_input_desktop()?;
        let hwnd = self.hwnd();
        if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return Err("The selected HWND was destroyed".to_string());
        }
        if window_pid(hwnd) != self.pid {
            return Err("The selected HWND changed ownership; the session was stopped".to_string());
        }
        if unsafe { IsIconic(hwnd).as_bool() } {
            return Err("The selected window is minimized".to_string());
        }
        if process_is_elevated(self.pid)? {
            return Err("The selected window became elevated and is blocked".to_string());
        }
        if let Some(reason) =
            sensitive_reason(&window_text(hwnd), &window_class(hwnd), &self.process_path)
        {
            return Err(reason);
        }
        visual_window_rect(hwnd)
    }

    fn is_related_window(&self, candidate: HWND) -> bool {
        if candidate.0.is_null() {
            return false;
        }
        if candidate == self.hwnd() || unsafe { IsChild(self.hwnd(), candidate).as_bool() } {
            return true;
        }
        if window_pid(candidate) != self.pid {
            return false;
        }
        let root_owner = unsafe { GetAncestor(candidate, GA_ROOTOWNER) };
        if root_owner == self.hwnd() {
            return true;
        }
        let mut current = candidate;
        for _ in 0..16 {
            current = match unsafe { GetWindow(current, GW_OWNER) } {
                Ok(hwnd) => hwnd,
                Err(_) => break,
            };
            if current.0.is_null() {
                break;
            }
            if current == self.hwnd() {
                return true;
            }
        }
        false
    }

    pub fn capture_target_hwnd(&self) -> u64 {
        let foreground = unsafe { GetForegroundWindow() };
        if self.is_related_window(foreground)
            && unsafe { IsWindowVisible(foreground).as_bool() }
            && !unsafe { IsIconic(foreground).as_bool() }
        {
            hwnd_value(foreground)
        } else {
            self.hwnd
        }
    }

    fn screen_point(&self, normalized_x: u64, normalized_y: u64) -> Result<(i32, i32), String> {
        self.validate_target()?;
        let capture_hwnd = hwnd_from_value(self.capture_target_hwnd());
        let rect = visual_window_rect(capture_hwnd)?;
        let width = (rect.right - rect.left).max(1) as i64;
        let height = (rect.bottom - rect.top).max(1) as i64;
        let x = rect.left as i64 + ((normalized_x.min(999) as i64 * (width - 1)) / 999);
        let y = rect.top as i64 + ((normalized_y.min(999) as i64 * (height - 1)) / 999);
        let hit = unsafe {
            WindowFromPoint(POINT {
                x: x as i32,
                y: y as i32,
            })
        };
        if !self.is_related_window(hit) {
            return Err("The requested point is covered by or belongs to a window outside the selected HWND scope".to_string());
        }
        Ok((x as i32, y as i32))
    }

    fn focus_for_keyboard(&self) -> Result<(), String> {
        self.validate_target()?;
        let foreground = unsafe { GetForegroundWindow() };
        if !self.is_related_window(foreground) {
            let _ = unsafe { SetForegroundWindow(self.hwnd()) };
            thread::sleep(Duration::from_millis(80));
        }
        if !self.is_related_window(unsafe { GetForegroundWindow() }) {
            return Err(
                "Windows refused to focus the selected HWND; keyboard input was not sent"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn environment_state(&self) -> Result<Value, String> {
        let rect = self.validate_target()?;
        Ok(json!({
            "window_title": window_text(self.hwnd()), "target_pid": self.pid,
            "target_hwnd": self.hwnd.to_string(), "capture_hwnd": self.capture_target_hwnd().to_string(),
            "process_path": self.process_path,
            "bounds": {"left": rect.left, "top": rect.top, "width": rect.right - rect.left, "height": rect.bottom - rect.top},
            "dpi": unsafe { GetDpiForWindow(self.hwnd()) },
        }))
    }

    pub fn execute_action(&mut self, call: &Value, approved: bool) -> Result<(), String> {
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
        self.validate_target()?;
        let xy = |x: &str, y: &str| -> Result<(i32, i32), String> {
            self.screen_point(
                args.get(x)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("'{name}' requires {x}"))?,
                args.get(y)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("'{name}' requires {y}"))?,
            )
        };
        match name {
            "click" | "double_click" | "triple_click" | "middle_click" | "right_click" => {
                let (x, y) = xy("x", "y")?;
                let count = if name == "double_click" {
                    2
                } else if name == "triple_click" {
                    3
                } else {
                    1
                };
                let (down, up) = if name == "middle_click" {
                    (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP)
                } else if name == "right_click" {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                };
                send_mouse_move(x, y)?;
                for _ in 0..count {
                    send_mouse_flags(down)?;
                    send_mouse_flags(up)?;
                }
            }
            "mouse_down" | "mouse_up" => {
                let (x, y) = xy("x", "y")?;
                send_mouse_move(x, y)?;
                send_mouse_flags(if name == "mouse_down" {
                    MOUSEEVENTF_LEFTDOWN
                } else {
                    MOUSEEVENTF_LEFTUP
                })?;
                self.mouse_down = name == "mouse_down";
            }
            "move" => {
                let (x, y) = xy("x", "y")?;
                send_mouse_move(x, y)?;
            }
            "drag_and_drop" => {
                let (sx, sy) = xy("start_x", "start_y")?;
                let (ex, ey) = xy("end_x", "end_y")?;
                send_mouse_move(sx, sy)?;
                send_mouse_flags(MOUSEEVENTF_LEFTDOWN)?;
                self.mouse_down = true;
                for step in 1..=10 {
                    let x = sx + ((ex - sx) * step / 10);
                    let y = sy + ((ey - sy) * step / 10);
                    if !self.is_related_window(unsafe { WindowFromPoint(POINT { x, y }) }) {
                        let _ = send_mouse_flags(MOUSEEVENTF_LEFTUP);
                        self.mouse_down = false;
                        return Err("Drag path left the selected HWND scope".to_string());
                    }
                    send_mouse_move(x, y)?;
                    thread::sleep(Duration::from_millis(15));
                }
                send_mouse_flags(MOUSEEVENTF_LEFTUP)?;
                self.mouse_down = false;
            }
            "scroll" => {
                let (x, y) = xy("x", "y")?;
                send_mouse_move(x, y)?;
                let magnitude = args
                    .get("magnitude_in_pixels")
                    .and_then(Value::as_u64)
                    .unwrap_or(300)
                    .max(1);
                let ticks = magnitude.div_ceil(60).min(20) as i32 * 120;
                match args
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or("down")
                {
                    "up" => send_mouse_wheel(ticks, false)?,
                    "down" => send_mouse_wheel(-ticks, false)?,
                    "left" => send_mouse_wheel(-ticks, true)?,
                    "right" => send_mouse_wheel(ticks, true)?,
                    _ => return Err("Invalid scroll direction".to_string()),
                }
            }
            "type" => {
                self.focus_for_keyboard()?;
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "type requires text".to_string())?;
                self.send_unicode(text)?;
                if args.get("press_enter").and_then(Value::as_bool) == Some(true) {
                    send_virtual_key(VK_RETURN.0, true)?;
                    send_virtual_key(VK_RETURN.0, false)?;
                }
            }
            "press_key" | "key_down" | "key_up" => {
                self.focus_for_keyboard()?;
                let key = args
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{name} requires key"))?;
                let vk = virtual_key(key)?;
                if name != "key_up" {
                    send_virtual_key(vk, true)?;
                    self.held_keys.insert(vk);
                }
                if name != "key_down" {
                    send_virtual_key(vk, false)?;
                    self.held_keys.remove(&vk);
                }
            }
            "hotkey" => {
                self.focus_for_keyboard()?;
                let keys = args
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "hotkey requires keys".to_string())?;
                let keys = keys
                    .iter()
                    .map(|v| virtual_key(v.as_str().unwrap_or_default()))
                    .collect::<Result<Vec<_>, _>>()?;
                for vk in &keys {
                    send_virtual_key(*vk, true)?;
                    self.held_keys.insert(*vk);
                }
                for vk in keys.iter().rev() {
                    send_virtual_key(*vk, false)?;
                    self.held_keys.remove(vk);
                }
            }
            "wait" => thread::sleep(Duration::from_secs(
                args.get("seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .min(30),
            )),
            "take_screenshot" => {}
            _ => return Err(format!("Unsupported desktop Computer Use action '{name}'")),
        }
        thread::sleep(Duration::from_millis(200));
        self.validate_target().map(|_| ())
    }

    pub fn release_inputs(&mut self) {
        for vk in self.held_keys.drain() {
            let _ = send_virtual_key(vk, false);
        }
        if self.mouse_down {
            let _ = send_mouse_flags(MOUSEEVENTF_LEFTUP);
            self.mouse_down = false;
        }
    }

    fn send_unicode(&self, text: &str) -> Result<(), String> {
        for unit in text.encode_utf16() {
            if !self.is_related_window(unsafe { GetForegroundWindow() }) {
                return Err("Keyboard focus left the selected HWND while typing; remaining text was not sent".to_string());
            }
            if unit == b'\r' as u16 {
                continue;
            }
            if unit == b'\n' as u16 {
                send_virtual_key(VK_RETURN.0, true)?;
                send_virtual_key(VK_RETURN.0, false)?;
            } else {
                send_inputs(&[
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(0),
                                wScan: unit,
                                dwFlags: KEYEVENTF_UNICODE,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(0),
                                wScan: unit,
                                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    },
                ])?;
            }
            // RichEdit-based targets can consume synthetic key messages more slowly than
            // SendInput accepts them. Keep producer throughput below the target queue rate so the
            // screenshot returned for this action reflects the complete text.
            thread::sleep(Duration::from_millis(6));
        }
        thread::sleep(Duration::from_millis(250));
        Ok(())
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(format!(
            "SendInput sent {sent} of {} events (UIPI may have blocked input)",
            inputs.len()
        ));
    }
    Ok(())
}

fn mouse_input(
    dx: i32,
    dy: i32,
    data: u32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_mouse_move(x: i32, y: i32) -> Result<(), String> {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(2);
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(2);
    let nx = (((x - vx) as i64 * 65535) / (vw - 1) as i64).clamp(0, 65535) as i32;
    let ny = (((y - vy) as i64 * 65535) / (vh - 1) as i64).clamp(0, 65535) as i32;
    send_inputs(&[mouse_input(
        nx,
        ny,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )])
}

fn send_mouse_flags(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<(), String> {
    send_inputs(&[mouse_input(0, 0, 0, flags)])
}

fn send_mouse_wheel(delta: i32, horizontal: bool) -> Result<(), String> {
    send_inputs(&[mouse_input(
        0,
        0,
        delta as u32,
        if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        },
    )])
}

fn send_virtual_key(vk: u16, down: bool) -> Result<(), String> {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if down {
                    Default::default()
                } else {
                    KEYEVENTF_KEYUP
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_inputs(&[input])
}

fn virtual_key(name: &str) -> Result<u16, String> {
    let lower = name.to_ascii_lowercase();
    let vk = match lower.as_str() {
        "enter" | "return" => VK_RETURN.0,
        "tab" => VK_TAB.0,
        "escape" | "esc" => VK_ESCAPE.0,
        "backspace" => VK_BACK.0,
        "delete" | "del" => VK_DELETE.0,
        "space" => VK_SPACE.0,
        "left" | "arrowleft" => VK_LEFT.0,
        "right" | "arrowright" => VK_RIGHT.0,
        "up" | "arrowup" => VK_UP.0,
        "down" | "arrowdown" => VK_DOWN.0,
        "home" => VK_HOME.0,
        "end" => VK_END.0,
        "pageup" => VK_PRIOR.0,
        "pagedown" => VK_NEXT.0,
        "control" | "ctrl" => VK_CONTROL.0,
        "shift" => VK_LSHIFT.0,
        "alt" => VK_LMENU.0,
        "meta" | "win" | "windows" => VK_LWIN.0,
        value if value.len() == 1 => value.as_bytes()[0].to_ascii_uppercase() as u16,
        value if value.starts_with('f') => {
            let number = value[1..]
                .parse::<u16>()
                .map_err(|_| format!("Unsupported key '{name}'"))?;
            if !(1..=24).contains(&number) {
                return Err(format!("Unsupported key '{name}'"));
            }
            VK_F1.0 + number - 1
        }
        _ => return Err(format!("Unsupported key '{name}'")),
    };
    Ok(vk)
}

pub fn capture_window(hwnd_value: u64) -> Result<DesktopScreenshot, String> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
        .map_err(|error| format!("Cannot initialize WinRT: {error}"))?;
    struct RoGuard;
    impl Drop for RoGuard {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    let _apartment = RoGuard;
    let hwnd = hwnd_from_value(hwnd_value);
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err("The selected HWND was destroyed".to_string());
    }
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|error| format!("Windows Graphics Capture is unavailable: {error}"))?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }
        .map_err(|error| format!("Cannot create a Windows Graphics Capture item: {error}"))?;
    let size = item
        .Size()
        .map_err(|error| format!("Cannot query capture size: {error}"))?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err("Windows Graphics Capture returned an empty target".to_string());
    }

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|error| format!("Cannot create D3D11 capture device: {error}"))?;
    let device = device.ok_or_else(|| "D3D11 did not return a device".to_string())?;
    let context = context.ok_or_else(|| "D3D11 did not return a device context".to_string())?;
    let dxgi: IDXGIDevice = device
        .cast()
        .map_err(|error| format!("Cannot obtain DXGI device: {error}"))?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|error| format!("Cannot create WinRT D3D device: {error}"))?;
    let winrt_device: IDirect3DDevice = inspectable
        .cast()
        .map_err(|error| format!("Cannot cast WinRT D3D device: {error}"))?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        SizeInt32 {
            Width: size.Width,
            Height: size.Height,
        },
    )
    .map_err(|error| format!("Cannot create WGC frame pool: {error}"))?;
    let capture = pool
        .CreateCaptureSession(&item)
        .map_err(|error| format!("Cannot create WGC session: {error}"))?;
    let _ = capture.SetIsCursorCaptureEnabled(true);
    capture
        .StartCapture()
        .map_err(|error| format!("Cannot start WGC: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let frame = loop {
        match pool.TryGetNextFrame() {
            Ok(frame) => break frame,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(15));
            }
            Err(error) => return Err(format!("WGC did not produce a frame: {error}")),
        }
    };
    let content = frame
        .ContentSize()
        .map_err(|error| format!("Cannot read WGC content size: {error}"))?;
    let surface = frame
        .Surface()
        .map_err(|error| format!("Cannot read WGC surface: {error}"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|error| format!("Cannot access WGC DXGI surface: {error}"))?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
        .map_err(|error| format!("Cannot access WGC texture: {error}"))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    desc.SampleDesc = DXGI_SAMPLE_DESC {
        Count: 1,
        Quality: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
        .map_err(|error| format!("Cannot create WGC staging texture: {error}"))?;
    let staging = staging.ok_or_else(|| "D3D11 did not return a staging texture".to_string())?;
    unsafe {
        context.CopyResource(&staging, &texture);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .map_err(|error| format!("Cannot map WGC staging texture: {error}"))?;
    let width = (content.Width.max(0) as u32).min(desc.Width);
    let height = (content.Height.max(0) as u32).min(desc.Height);
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        let source = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            )
        };
        let target = &mut rgba[y * width as usize * 4..(y + 1) * width as usize * 4];
        for (bgra, rgba) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
            rgba[0] = bgra[2];
            rgba[1] = bgra[1];
            rgba[2] = bgra[0];
            rgba[3] = 255;
        }
    }
    unsafe {
        context.Unmap(&staging, 0);
    }
    let _ = frame.Close();
    let _ = capture.Close();
    let _ = pool.Close();
    let mut png = Vec::new();
    {
        let mut encoder = Encoder::new(&mut png, width, height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("Cannot encode WGC PNG header: {error}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| format!("Cannot encode WGC PNG: {error}"))?;
    }
    if png.len() > MAX_CAPTURE_BYTES {
        return Err(format!(
            "WGC screenshot exceeded the {} byte limit",
            MAX_CAPTURE_BYTES
        ));
    }
    Ok(DesktopScreenshot { png, width, height })
}

pub fn screenshot_json(shot: DesktopScreenshot) -> Value {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(&shot.png));
    json!({"mime_type": "image/png", "data": STANDARD.encode(&shot.png), "sha256": hash,
        "width": shot.width, "height": shot.height, "source": "windows_graphics_capture"})
}
