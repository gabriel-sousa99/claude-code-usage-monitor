use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::models::UsageData;
use crate::native_interop::{
    self, Color, TIMER_COUNTDOWN, TIMER_POLL, TIMER_RESET_POLL,
    WM_APP_TRAY, WM_APP_USAGE_UPDATED,
};
use crate::tray_icon;
use crate::poller;
use crate::theme;

/// Wrapper to make HWND sendable across threads (safe for PostMessage usage)
#[derive(Clone, Copy)]
struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<HWND>,
    tray_notify_hwnd: Option<HWND>,
    win_event_hook: Option<HWINEVENTHOOK>,
    is_dark: bool,
    embedded: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,
    color_scheme_mode: ColorSchemeMode,

    session_percent: f64,
    session_text: String,
    weekly_percent: f64,
    weekly_text: String,

    data: Option<UsageData>,

    poll_interval_ms: u32,
    retry_count: u32,
    last_poll_ok: bool,

    tray_offset: i32,
    dragging: bool,
    drag_start_mouse_x: i32,
    drag_start_offset: i32,

    widget_visible: bool,
    widget_width_px: i32,
    widget_height_px: i32,
    background_color: Color,
    font_color: Color,
    indicator_color: Color,
    border_width_px: i32,
    panel_margin_px: i32,
    panel_padding_px: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSchemeMode {
    Auto,
    Light,
    Dark,
    Custom,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum SettingsColorSchemeMode {
    Auto,
    Light,
    Dark,
}

impl From<SettingsColorSchemeMode> for ColorSchemeMode {
    fn from(value: SettingsColorSchemeMode) -> Self {
        match value {
            SettingsColorSchemeMode::Auto => Self::Auto,
            SettingsColorSchemeMode::Light => Self::Light,
            SettingsColorSchemeMode::Dark => Self::Dark,
        }
    }
}

impl ColorSchemeMode {
    fn to_settings(self) -> Option<SettingsColorSchemeMode> {
        match self {
            Self::Auto => Some(SettingsColorSchemeMode::Auto),
            Self::Light => Some(SettingsColorSchemeMode::Light),
            Self::Dark => Some(SettingsColorSchemeMode::Dark),
            Self::Custom => None,
        }
    }
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

const POLL_1_MIN: u32 = 60_000;
const POLL_5_MIN: u32 = 300_000;
const POLL_15_MIN: u32 = 900_000;
const POLL_1_HOUR: u32 = 3_600_000;

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_RESET_POSITION: u16 = 30;
const IDM_SCHEME_AUTO: u16 = 270;
const IDM_SCHEME_LIGHT: u16 = 271;
const IDM_SCHEME_DARK: u16 = 272;
const IDM_LANG_SYSTEM: u16 = 40;
const IDM_LANG_ENGLISH: u16 = 41;
const IDM_LANG_PORTUGUESE_BRAZIL: u16 = 47;
const IDM_SIZE_AUTO: u16 = 200;
const IDM_SIZE_COMPACT: u16 = 201;
const IDM_SIZE_DEFAULT: u16 = 202;
const IDM_SIZE_LARGE: u16 = 203;
const IDM_SIZE_XL: u16 = 204;
const IDM_BG_CHARCOAL: u16 = 210;
const IDM_BG_SLATE: u16 = 211;
const IDM_BG_BLACK: u16 = 212;
const IDM_FONT_IVORY: u16 = 220;
const IDM_FONT_SOFT: u16 = 221;
const IDM_FONT_WHITE: u16 = 222;
const IDM_IND_CORAL: u16 = 230;
const IDM_IND_MINT: u16 = 231;
const IDM_IND_BLUE: u16 = 232;
const IDM_BORDER_NONE: u16 = 240;
const IDM_BORDER_THIN: u16 = 241;
const IDM_BORDER_MEDIUM: u16 = 242;
const IDM_BORDER_THICK: u16 = 243;
const IDM_PADDING_0: u16 = 250;
const IDM_PADDING_4: u16 = 251;
const IDM_PADDING_8: u16 = 252;
const IDM_MARGIN_0: u16 = 260;
const IDM_MARGIN_2: u16 = 261;
const IDM_MARGIN_4: u16 = 262;

const DIVIDER_HIT_ZONE: i32 = 13; // LEFT_DIVIDER_W + DIVIDER_RIGHT_MARGIN

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const DEFAULT_BORDER_PX: i32 = 1;
const DEFAULT_PANEL_MARGIN_PX: i32 = 1;
const DEFAULT_PANEL_PADDING_PX: i32 = 0;

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);
static CUSTOM_WIDGET_WIDTH_PX: AtomicI32 = AtomicI32::new(0);
static CUSTOM_WIDGET_HEIGHT_PX: AtomicI32 = AtomicI32::new(0);
static CUSTOM_BORDER_WIDTH_PX: AtomicI32 = AtomicI32::new(DEFAULT_BORDER_PX);
static CUSTOM_PANEL_MARGIN_PX: AtomicI32 = AtomicI32::new(DEFAULT_PANEL_MARGIN_PX);
static CUSTOM_PANEL_PADDING_PX: AtomicI32 = AtomicI32::new(DEFAULT_PANEL_PADDING_PX);

/// Scale a base pixel value (designed at 96 DPI) to the current DPI.
fn sc(px: i32) -> i32 {
    let dpi = CURRENT_DPI.load(Ordering::Relaxed);
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

fn load_embedded_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }

        let mut large_icon = HICON::default();
        let mut small_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR::from_raw(exe_buf.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );

        if extracted == 0 {
            (HICON::default(), HICON::default())
        } else {
            (large_icon, small_icon)
        }
    }
}

unsafe impl Send for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn settings_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("settings.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    tray_offset: i32,
    #[serde(default = "default_poll_interval")]
    poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default = "default_border_width")]
    border_width_px: i32,
    #[serde(default = "default_panel_margin")]
    panel_margin_px: i32,
    #[serde(default = "default_panel_padding")]
    panel_padding_px: i32,
    #[serde(default = "default_widget_visible")]
    widget_visible: bool,
    #[serde(default)]
    widget_width_px: i32,
    #[serde(default)]
    widget_height_px: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_color_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    font_color_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    indicator_color_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color_scheme_mode: Option<SettingsColorSchemeMode>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            tray_offset: 0,
            poll_interval_ms: default_poll_interval(),
            language: None,
            widget_visible: true,
            widget_width_px: 0,
            widget_height_px: 0,
            background_color_hex: None,
            font_color_hex: None,
            indicator_color_hex: None,
            color_scheme_mode: Some(SettingsColorSchemeMode::Auto),
            border_width_px: default_border_width(),
            panel_margin_px: default_panel_margin(),
            panel_padding_px: default_panel_padding(),
        }
    }
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn default_widget_visible() -> bool {
    true
}

fn default_border_width() -> i32 {
    DEFAULT_BORDER_PX
}

fn default_panel_margin() -> i32 {
    DEFAULT_PANEL_MARGIN_PX
}

fn default_panel_padding() -> i32 {
    DEFAULT_PANEL_PADDING_PX
}

fn load_settings() -> SettingsFile {
    let content = match std::fs::read_to_string(settings_path()) {
        Ok(c) => c,
        Err(_) => return SettingsFile::default(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_settings(settings: &SettingsFile) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

fn save_state_settings() {
    let state = lock_state();
    if let Some(s) = state.as_ref() {
        save_settings(&SettingsFile {
            tray_offset: s.tray_offset,
            poll_interval_ms: s.poll_interval_ms,
            language: s
                .language_override
                .map(|language| match language {
                    LanguageId::English => "en",
                    LanguageId::PortugueseBrazil => "pt-BR",
                }
                .to_string()),
            widget_visible: s.widget_visible,
            widget_width_px: s.widget_width_px,
            widget_height_px: s.widget_height_px,
            background_color_hex: Some(color_to_hex(s.background_color)),
            font_color_hex: Some(color_to_hex(s.font_color)),
            indicator_color_hex: Some(color_to_hex(s.indicator_color)),
            color_scheme_mode: s.color_scheme_mode.to_settings(),
            border_width_px: s.border_width_px,
            panel_margin_px: s.panel_margin_px,
            panel_padding_px: s.panel_padding_px,
        });
    }
}

fn color_to_hex(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

fn color_palette_for_mode(mode: ColorSchemeMode, is_dark_system: bool) -> (Color, Color, Color) {
    let use_dark_palette = match mode {
        ColorSchemeMode::Auto => is_dark_system,
        ColorSchemeMode::Light => false,
        ColorSchemeMode::Dark => true,
        ColorSchemeMode::Custom => is_dark_system,
    };

    if use_dark_palette {
        (
            Color::from_hex("#161616"),
            Color::from_hex("#EAEAEA"),
            Color::from_hex("#D97757"),
        )
    } else {
        (
            Color::from_hex("#F4F6F8"),
            Color::from_hex("#1F2933"),
            Color::from_hex("#0A84FF"),
        )
    }
}

fn apply_color_scheme_to_state(state: &mut AppState) {
    if state.color_scheme_mode == ColorSchemeMode::Custom {
        return;
    }

    let (bg, font, indicator) = color_palette_for_mode(state.color_scheme_mode, state.is_dark);
    state.background_color = bg;
    state.font_color = font;
    state.indicator_color = indicator;
}

fn shade_color(color: Color, delta: i16) -> Color {
    let adj = |v: u8| -> u8 { (v as i16 + delta).clamp(0, 255) as u8 };
    Color::new(adj(color.r), adj(color.g), adj(color.b))
}

fn tray_icon_data_from_state() -> (Option<f64>, String) {
    let state = lock_state();
    match state.as_ref() {
        Some(s) if s.last_poll_ok => {
            let tooltip = format!("5h: {} | 7d: {}", s.session_text, s.weekly_text);
            (Some(s.session_percent), tooltip)
        }
        _ => (None, "Claude Code Usage Monitor".to_string()),
    }
}

fn toggle_widget_visibility(hwnd: HWND) {
    let new_visible = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.widget_visible = !s.widget_visible;
            s.widget_visible
        } else {
            return;
        }
    };
    save_state_settings();
    unsafe {
        if new_visible {
            position_at_taskbar();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            render_layered();
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn refresh_usage_texts(state: &mut AppState) {
    if !state.last_poll_ok {
        return;
    }

    let strings = state.language.strings();
    let Some((session_text, weekly_text)) = state.data.as_ref().map(|data| {
        (
            poller::format_line(&data.session, strings),
            poller::format_line(&data.weekly, strings),
        )
    }) else {
        return;
    };

    state.session_text = session_text;
    state.weekly_text = weekly_text;
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
    refresh_usage_texts(state);
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";

/// Returns true only if the startup registry value points to this executable.
fn is_startup_enabled() -> bool {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            return false;
        }

        // Query the size of the value
        let mut data_size: u32 = 0;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_size),
        );
        if result.is_err() || data_size == 0 {
            let _ = RegCloseKey(hkey);
            return false;
        }

        // Read the value
        let mut buf = vec![0u8; data_size as usize];
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut data_size),
        );
        let _ = RegCloseKey(hkey);
        if result.is_err() {
            return false;
        }

        // Convert the registry value (UTF-16) to a string
        let wide_slice =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, data_size as usize / 2);
        let reg_value = String::from_utf16_lossy(wide_slice)
            .trim_end_matches('\0')
            .to_string();

        // Get the current executable path
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return false;
        }
        let current_exe = String::from_utf16_lossy(&exe_buf[..len]);

        // Case-insensitive comparison (Windows paths are case-insensitive)
        reg_value.eq_ignore_ascii_case(&current_exe)
    }
}

fn set_startup_enabled(enable: bool) {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if result.is_err() {
            return;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        if enable {
            let mut exe_buf = [0u16; 260];
            let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
            if len > 0 {
                // Write the wide string including null terminator
                let byte_len = ((len + 1) * 2) as u32;
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        exe_buf.as_ptr() as *const u8,
                        byte_len as usize,
                    )),
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }
}

// Dimensions matching the C# version
const SEGMENT_W: i32 = 10;
const SEGMENT_H: i32 = 12;
const SEGMENT_GAP: i32 = 1;
const SEGMENT_COUNT: i32 = 10;
const CORNER_RADIUS: i32 = 4;

const LEFT_DIVIDER_W: i32 = 3;
const DIVIDER_RIGHT_MARGIN: i32 = 10;
const LABEL_WIDTH: i32 = 18;
const LABEL_RIGHT_MARGIN: i32 = 8;
const BAR_RIGHT_MARGIN: i32 = 8;
const TEXT_WIDTH: i32 = 62;
const RIGHT_MARGIN: i32 = 12;
const WIDGET_HEIGHT: i32 = 42;

fn base_widget_width() -> i32 {
    sc(LEFT_DIVIDER_W)
        + sc(DIVIDER_RIGHT_MARGIN)
        + sc(LABEL_WIDTH)
        + sc(LABEL_RIGHT_MARGIN)
        + (sc(SEGMENT_W) + sc(SEGMENT_GAP)) * SEGMENT_COUNT
        - sc(SEGMENT_GAP)
        + sc(BAR_RIGHT_MARGIN)
        + sc(TEXT_WIDTH)
        + sc(RIGHT_MARGIN)
}

fn total_widget_width() -> i32 {
    let panel_extra = (CUSTOM_PANEL_MARGIN_PX.load(Ordering::Relaxed)
        + CUSTOM_PANEL_PADDING_PX.load(Ordering::Relaxed))
        .max(0)
        * 2;
    let custom = CUSTOM_WIDGET_WIDTH_PX.load(Ordering::Relaxed);
    let min_width = base_widget_width() + panel_extra;
    if custom > 0 {
        custom.max(min_width)
    } else {
        min_width
    }
}

fn total_widget_height() -> i32 {
    let panel_extra = (CUSTOM_PANEL_MARGIN_PX.load(Ordering::Relaxed)
        + CUSTOM_PANEL_PADDING_PX.load(Ordering::Relaxed))
        .max(0)
        * 2;
    let custom = CUSTOM_WIDGET_HEIGHT_PX.load(Ordering::Relaxed);
    let min_height = sc(36) + panel_extra;
    let base = sc(WIDGET_HEIGHT) + panel_extra;
    if custom > 0 {
        custom.max(min_height)
    } else {
        base
    }
}

pub fn run() {
    // Enable Per-Monitor DPI Awareness V2 for crisp rendering at any scale factor
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running
    let mutex_name = native_interop::wide_str("Global\\ClaudeCodeUsageMonitor");
    let _mutex = unsafe {
        let handle = CreateMutexW(None, false, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    diagnose::log("startup aborted: another instance is already running");
                    return;
                }
                h
            }
            Err(error) => {
                diagnose::log_error("startup aborted: unable to create single-instance mutex", error);
                return;
            }
        }
    };

    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitor");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = load_embedded_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let settings = load_settings();
        let language_override = Some(LanguageId::PortugueseBrazil);
        let language = LanguageId::PortugueseBrazil;
        CUSTOM_WIDGET_WIDTH_PX.store(settings.widget_width_px, Ordering::Relaxed);
        CUSTOM_WIDGET_HEIGHT_PX.store(settings.widget_height_px, Ordering::Relaxed);
        CUSTOM_BORDER_WIDTH_PX.store(settings.border_width_px, Ordering::Relaxed);
        CUSTOM_PANEL_MARGIN_PX.store(settings.panel_margin_px, Ordering::Relaxed);
        CUSTOM_PANEL_PADDING_PX.store(settings.panel_padding_px, Ordering::Relaxed);

        let has_custom_colors = settings.background_color_hex.is_some()
            || settings.font_color_hex.is_some()
            || settings.indicator_color_hex.is_some();

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str(language.strings().window_title);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            total_widget_width(),
            total_widget_height(),
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .unwrap();

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(large_icon.0 as isize),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(small_icon.0 as isize),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));

        let is_dark = theme::is_dark_mode();
        let color_scheme_mode = settings
            .color_scheme_mode
            .map(ColorSchemeMode::from)
            .unwrap_or(if has_custom_colors {
                ColorSchemeMode::Custom
            } else {
                ColorSchemeMode::Auto
            });
        let (scheme_bg, scheme_font, scheme_indicator) =
            color_palette_for_mode(color_scheme_mode, is_dark);
        let background_color = settings
            .background_color_hex
            .as_deref()
            .map(Color::from_hex)
            .unwrap_or(scheme_bg);
        let font_color = settings
            .font_color_hex
            .as_deref()
            .map(Color::from_hex)
            .unwrap_or(scheme_font);
        let indicator_color = settings
            .indicator_color_hex
            .as_deref()
            .map(Color::from_hex)
            .unwrap_or(scheme_indicator);
        let mut embedded = false;

        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                embedded: false,
                language_override,
                language,
                color_scheme_mode,
                session_percent: 0.0,
                session_text: "--".to_string(),
                weekly_percent: 0.0,
                weekly_text: "--".to_string(),
                data: None,
                poll_interval_ms: settings.poll_interval_ms,
                retry_count: 0,
                last_poll_ok: false,
                tray_offset: settings.tray_offset,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_offset: 0,
                widget_visible: settings.widget_visible,
                widget_width_px: settings.widget_width_px,
                widget_height_px: settings.widget_height_px,
                background_color,
                font_color,
                indicator_color,
                border_width_px: settings.border_width_px,
                panel_margin_px: settings.panel_margin_px,
                panel_padding_px: settings.panel_padding_px,
            });
        }

        // Try to embed in taskbar
        if let Some(taskbar_hwnd) = native_interop::find_taskbar() {
            diagnose::log(format!("taskbar found hwnd={:?}", taskbar_hwnd));
            native_interop::embed_in_taskbar(hwnd, taskbar_hwnd);
            embedded = true;

            let mut state = lock_state();
            let s = state.as_mut().unwrap();
            s.taskbar_hwnd = Some(taskbar_hwnd);
            s.embedded = true;

            let tray_notify = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd");
            s.tray_notify_hwnd = tray_notify;
            if tray_notify.is_some() {
                diagnose::log("TrayNotifyWnd found");
            } else {
                diagnose::log("TrayNotifyWnd not found");
            }

            if let Some(tray_hwnd) = tray_notify {
                let thread_id = native_interop::get_window_thread_id(tray_hwnd);
                let hook = native_interop::set_tray_event_hook(thread_id, on_tray_location_changed);
                s.win_event_hook = hook;
                if hook.is_some() {
                    diagnose::log("tray event hook installed");
                } else {
                    diagnose::log("tray event hook could not be installed");
                }
            }
        } else {
            diagnose::log("taskbar not found; using fallback popup window");
        }

        // If not embedded, fall back to topmost popup with SetLayeredWindowAttributes
        if !embedded {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }

        // Register system tray icon
        let (tray_pct, tray_tooltip) = tray_icon_data_from_state();
        tray_icon::add(hwnd, tray_pct, &tray_tooltip);

        // Position and show (only if widget_visible preference is true)
        position_at_taskbar();
        if settings.widget_visible {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        diagnose::log("window shown");

        // Initial render via UpdateLayeredWindow (for embedded) or InvalidateRect (fallback)
        render_layered();

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(hwnd, TIMER_POLL, initial_poll_ms, None);

        // Initial poll
        let send_hwnd = SendHwnd::from_hwnd(hwnd);
        std::thread::spawn(move || {
            diagnose::log("initial poll thread started");
            do_poll(send_hwnd);
        });

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Render widget content and push to the layered window via UpdateLayeredWindow.
/// Renders fully opaque with the actual taskbar background colour so that
/// ClearType sub-pixel font rendering can be used for crisp, OS-native text.
fn render_layered() {
    refresh_dpi();
    let (
        hwnd_val,
        is_dark,
        embedded,
        strings,
        session_pct,
        session_text,
        weekly_pct,
        weekly_text,
        panel_bg,
        panel_text,
        panel_indicator,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.hwnd,
                s.is_dark,
                s.embedded,
                s.language.strings(),
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
                s.background_color,
                s.font_color,
                s.indicator_color,
            ),
            None => return,
        }
    };

    let hwnd = hwnd_val.to_hwnd();

    // For non-embedded fallback, just invalidate and let WM_PAINT handle it
    if !embedded {
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
        return;
    }

    let width = total_widget_width();
    let height = total_widget_height();

    let accent = panel_indicator;
    let track = shade_color(panel_indicator, -88);
    let text_color = panel_text;
    let bg_color = if is_dark {
        Color::from_hex("#1C1C1C")
    } else {
        Color::from_hex("#F3F3F3")
    };

    unsafe {
        let screen_dc = GetDC(hwnd);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();

        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }

        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;

        // Render once with the actual taskbar background colour.
        // Using an opaque background lets us use CLEARTYPE_QUALITY for
        // sub-pixel font rendering that matches the rest of the OS.
        paint_content(
            mem_dc,
            width,
            height,
            is_dark,
            &bg_color,
            &panel_bg,
            &text_color,
            &accent,
            &track,
            strings,
            session_pct,
            &session_text,
            weekly_pct,
            &weekly_text,
        );

        // Background pixels → alpha 1 (nearly invisible but still hittable for right-click).
        // Content pixels → fully opaque (preserves ClearType sub-pixel rendering).
        let bg_bgr = bg_color.to_colorref();
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        for px in pixel_data.iter_mut() {
            let rgb = *px & 0x00FFFFFF;
            if rgb == bg_bgr {
                *px = 0x01000000;
            } else {
                *px = rgb | 0xFF000000;
            }
        }

        // Push to window via UpdateLayeredWindow
        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };

        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(
    hdc: HDC,
    width: i32,
    height: i32,
    _is_dark: bool,
    bg: &Color,
    panel_bg: &Color,
    text_color: &Color,
    accent: &Color,
    track: &Color,
    strings: Strings,
    session_pct: f64,
    session_text: &str,
    weekly_pct: f64,
    weekly_text: &str,
) {
    unsafe {
        let client_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

        let bg_brush = CreateSolidBrush(COLORREF(bg.to_colorref()));
        FillRect(hdc, &client_rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        let panel_border = shade_color(*panel_bg, 20);
        let border_px = CUSTOM_BORDER_WIDTH_PX.load(Ordering::Relaxed).clamp(0, 6);
        let panel_margin = CUSTOM_PANEL_MARGIN_PX.load(Ordering::Relaxed).max(0);
        let panel_padding = CUSTOM_PANEL_PADDING_PX.load(Ordering::Relaxed).max(0);
        let panel_rect = RECT {
            left: panel_margin,
            top: panel_margin,
            right: width - panel_margin,
            bottom: height - panel_margin,
        };
        let panel_radius = sc(10);
        draw_rounded_rect(hdc, &panel_rect, panel_bg, panel_radius);
        if border_px > 0 {
            draw_rounded_outline(hdc, &panel_rect, &panel_border, panel_radius, border_px);
        }

        let content_x =
            panel_margin + panel_padding + sc(LEFT_DIVIDER_W) + sc(DIVIDER_RIGHT_MARGIN);
        let row_gap = sc(8);
        let rows_h = sc(SEGMENT_H) * 2 + row_gap;
        let row1_y = ((height - rows_h) / 2).max(panel_margin + panel_padding);
        let row2_y = row1_y + sc(SEGMENT_H) + row_gap;

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));

        let font_name = native_interop::wide_str("Segoe UI Semibold");
        let font = CreateFontW(
            sc(-11),
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(hdc, font);

        draw_row(
            hdc,
            content_x,
            row1_y,
            strings.session_window,
            session_pct,
            session_text,
            accent,
            track,
        );
        draw_row(
            hdc,
            content_x,
            row2_y,
            strings.weekly_window,
            weekly_pct,
            weekly_text,
            accent,
            track,
        );

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

fn do_poll(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    match poller::poll() {
        Ok(data) => {
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.session_percent = data.session.percentage;
                s.weekly_percent = data.weekly.percentage;
                // Stop fast-poll if reset data is now fresh
                if !poller::is_past_reset(&data) {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    }
                }

                s.data = Some(data);
                s.last_poll_ok = true;
                refresh_usage_texts(s);

                // Recovered from errors — restore normal poll interval
                if s.retry_count > 0 {
                    s.retry_count = 0;
                    let interval = s.poll_interval_ms;
                    unsafe {
                        SetTimer(hwnd, TIMER_POLL, interval, None);
                    }
                }
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(_e) => {
            // Show refresh indicator — retry will recover silently
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.session_text = "...".to_string();
                s.weekly_text = "...".to_string();
                s.last_poll_ok = false;

                // Exponential backoff retry: 30s, 60s, 120s, ... up to poll_interval
                s.retry_count = s.retry_count.saturating_add(1);
                let backoff = RETRY_BASE_MS
                    .saturating_mul(1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX));
                let retry_ms = backoff.min(s.poll_interval_ms);

                unsafe {
                    // Kill the 5-second reset poll so it doesn't bypass backoff
                    let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    SetTimer(hwnd, TIMER_POLL, retry_ms, None);
                }
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn schedule_countdown_timer() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    let data = match &s.data {
        Some(d) => d,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();

    // If a reset time has passed, poll every 5s to pick up fresh data
    if poller::is_past_reset(data) {
        unsafe {
            SetTimer(hwnd, TIMER_RESET_POLL, 5_000, None);
        }
    }

    let session_delay = poller::time_until_display_change(data.session.resets_at);
    let weekly_delay = poller::time_until_display_change(data.weekly.resets_at);

    let min_delay = match (session_delay, weekly_delay) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(hwnd, TIMER_COUNTDOWN, ms, None);
    }
}

fn check_theme_change() {
    let new_dark = theme::is_dark_mode();
    let changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if s.is_dark != new_dark {
                s.is_dark = new_dark;
                apply_color_scheme_to_state(s);
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if changed {
        render_layered();
    }
}

fn check_language_change() {
    let _ = ();
}

fn update_display() {
    let mut state = lock_state();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    // Don't overwrite error text with stale cached data
    if !s.last_poll_ok {
        return;
    }

    refresh_usage_texts(s);
}

fn position_at_taskbar() {
    refresh_dpi();
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    // Don't fight the user's drag
    if s.dragging {
        return;
    }

    let hwnd = s.hwnd.to_hwnd();
    let embedded = s.embedded;
    let tray_offset = s.tray_offset;

    let taskbar_hwnd = match s.taskbar_hwnd {
        Some(h) => h,
        None => {
            diagnose::log("position_at_taskbar skipped: no taskbar handle");
            return;
        }
    };

    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => {
            diagnose::log("position_at_taskbar skipped: unable to query taskbar rect");
            return;
        }
    };

    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    let mut tray_left = taskbar_rect.right;
    let anchor_top = taskbar_rect.top;
    let anchor_height = taskbar_height;

    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }

    let widget_width = total_widget_width();

    let widget_height = total_widget_height();
    let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
    if embedded {
        // Child window: coordinates relative to parent (taskbar)
        let x = tray_left - taskbar_rect.left - widget_width - tray_offset;
        native_interop::move_window(hwnd, x, y - taskbar_rect.top, widget_width, widget_height);
        diagnose::log(format!(
            "positioned embedded widget at x={x} y={} w={widget_width} h={widget_height}",
            y - taskbar_rect.top
        ));
    } else {
        // Topmost popup: screen coordinates
        let x = tray_left - widget_width - tray_offset;
        native_interop::move_window(hwnd, x, y, widget_width, widget_height);
        diagnose::log(format!(
            "positioned fallback widget at x={x} y={y} w={widget_width} h={widget_height}"
        ));
    }
}

fn compute_anchor_y(anchor_top: i32, anchor_height: i32, widget_height: i32) -> i32 {
    let anchor_bottom = anchor_top + anchor_height;
    (anchor_bottom - widget_height).max(anchor_top)
}

/// WinEvent callback for tray icon location changes
unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    static LAST_REPOSITION: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    let is_tray = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| s.tray_notify_hwnd)
            .map(|h| h == hwnd)
            .unwrap_or(false)
    };

    if is_tray {
        let should_reposition = {
            let mut last = LAST_REPOSITION.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if last
                .map(|t| now.duration_since(t).as_millis() > 500)
                .unwrap_or(true)
            {
                *last = Some(now);
                true
            } else {
                false
            }
        };
        if should_reposition {
            position_at_taskbar();
            render_layered();
        }
    }
}

/// Main window procedure
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            // For non-embedded fallback, paint normally
            let embedded = {
                let state = lock_state();
                state.as_ref().map(|s| s.embedded).unwrap_or(false)
            };
            if embedded {
                // Layered windows don't use WM_PAINT; just validate the region
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            } else {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(hdc, hwnd);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG | WM_SETTINGCHANGE => {
            if msg == WM_DPICHANGED_MSG {
                let new_dpi = (wparam.0 & 0xFFFF) as u32;
                CURRENT_DPI.store(new_dpi, Ordering::Relaxed);
            }
            if msg == WM_SETTINGCHANGE {
                check_theme_change();
                check_language_change();
            }
            refresh_dpi();
            position_at_taskbar();
            render_layered();
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                TIMER_COUNTDOWN => {
                    update_display();
                    render_layered();
                    schedule_countdown_timer();
                }
                TIMER_RESET_POLL => {
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            check_language_change();
            render_layered();
            schedule_countdown_timer();
            let (pct, tooltip) = tray_icon_data_from_state();
            tray_icon::update(hwnd, pct, &tooltip);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            // Always show resize cursor while dragging or when hovering divider zone
            let hit_test = (lparam.0 & 0xFFFF) as u16;
            if is_dragging {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            if hit_test == 1 {
                // HTCLIENT
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if pt.x < sc(DIVIDER_HIT_ZONE) {
                    let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                    SetCursor(cursor);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let client_x = (lparam.0 & 0xFFFF) as i16 as i32;
            if client_x < sc(DIVIDER_HIT_ZONE) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.dragging = true;
                    s.drag_start_mouse_x = pt.x;
                    s.drag_start_offset = s.tray_offset;
                }
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);

                let mut state = lock_state();
                let s = match state.as_mut() {
                    Some(s) => s,
                    None => return LRESULT(0),
                };

                // Moving mouse left = positive delta = larger offset (further left)
                let delta = s.drag_start_mouse_x - pt.x;
                let mut new_offset = s.drag_start_offset + delta;

                // Clamp: offset >= 0 (can't go right of default)
                if new_offset < 0 {
                    new_offset = 0;
                }

                // Clamp: don't go past left edge of taskbar
                if let Some(taskbar_hwnd) = s.taskbar_hwnd {
                    if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                        let mut tray_left = taskbar_rect.right;
                        if let Some(tray_hwnd) =
                            native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd")
                        {
                            if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd)
                            {
                                tray_left = tray_rect.left;
                            }
                        }
                        let widget_width = total_widget_width();
                        let max_offset = if s.embedded {
                            tray_left - taskbar_rect.left - widget_width
                        } else {
                            tray_left - taskbar_rect.left - widget_width
                        };
                        if new_offset > max_offset {
                            new_offset = max_offset;
                        }
                    }
                }

                s.tray_offset = new_offset;

                // Move window directly
                let hwnd_val = s.hwnd.to_hwnd();
                if let Some(taskbar_hwnd) = s.taskbar_hwnd {
                    if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                        let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
                        let mut tray_left = taskbar_rect.right;
                        let anchor_top = taskbar_rect.top;
                        let anchor_height = taskbar_height;
                        if let Some(tray_hwnd) =
                            native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd")
                        {
                            if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd)
                            {
                                tray_left = tray_rect.left;
                            }
                        }
                        let widget_width = total_widget_width();
                        let widget_height = total_widget_height();
                        let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
                        if s.embedded {
                            let x = tray_left - taskbar_rect.left - widget_width - new_offset;
                            native_interop::move_window(
                                hwnd_val,
                                x,
                                y - taskbar_rect.top,
                                widget_width,
                                widget_height,
                            );
                        } else {
                            let x = tray_left - widget_width - new_offset;
                            native_interop::move_window(
                                hwnd_val,
                                x,
                                y,
                                widget_width,
                                widget_height,
                            );
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        let offset = s.tray_offset;
                        Some(offset)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if was_dragging.is_some() {
                let _ = ReleaseCapture();
                save_state_settings();
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            match id {
                1 => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                        }
                    }
                    render_layered();
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                2 => {
                    let hook = {
                        let state = lock_state();
                        state.as_ref().and_then(|s| s.win_event_hook)
                    };
                    if let Some(h) = hook {
                        native_interop::unhook_win_event(h);
                    }
                    PostQuitMessage(0);
                }
                IDM_RESET_POSITION => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.tray_offset = 0;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_START_WITH_WINDOWS => {
                    set_startup_enabled(!is_startup_enabled());
                }
                IDM_FREQ_1MIN | IDM_FREQ_5MIN | IDM_FREQ_15MIN | IDM_FREQ_1HOUR => {
                    let new_interval = match id {
                        IDM_FREQ_1MIN => POLL_1_MIN,
                        IDM_FREQ_5MIN => POLL_5_MIN,
                        IDM_FREQ_15MIN => POLL_15_MIN,
                        IDM_FREQ_1HOUR => POLL_1_HOUR,
                        _ => POLL_15_MIN,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.poll_interval_ms = new_interval;
                        }
                    }
                    save_state_settings();
                    // Reset the poll timer with the new interval
                    SetTimer(hwnd, TIMER_POLL, new_interval, None);
                }
                IDM_LANG_SYSTEM | IDM_LANG_ENGLISH | IDM_LANG_PORTUGUESE_BRAZIL => {
                    let language_override = match id {
                        IDM_LANG_SYSTEM => None,
                        IDM_LANG_ENGLISH => Some(LanguageId::English),
                        IDM_LANG_PORTUGUESE_BRAZIL => Some(LanguageId::PortugueseBrazil),
                        _ => None,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_language_to_state(s, language_override);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                IDM_SIZE_AUTO | IDM_SIZE_COMPACT | IDM_SIZE_DEFAULT | IDM_SIZE_LARGE
                | IDM_SIZE_XL => {
                    let (width, height) = match id {
                        IDM_SIZE_AUTO => (0, 0),
                        IDM_SIZE_COMPACT => (320, 38),
                        IDM_SIZE_DEFAULT => (360, 44),
                        IDM_SIZE_LARGE => (400, 50),
                        IDM_SIZE_XL => (440, 56),
                        _ => (0, 0),
                    };
                    apply_widget_size_preset(hwnd, width, height);
                }
                IDM_BG_CHARCOAL | IDM_BG_SLATE | IDM_BG_BLACK => {
                    let bg = match id {
                        IDM_BG_CHARCOAL => Color::from_hex("#161616"),
                        IDM_BG_SLATE => Color::from_hex("#1E242B"),
                        IDM_BG_BLACK => Color::from_hex("#0F0F0F"),
                        _ => Color::from_hex("#161616"),
                    };
                    apply_widget_color_preset(Some(bg), None, None);
                }
                IDM_FONT_IVORY | IDM_FONT_SOFT | IDM_FONT_WHITE => {
                    let fg = match id {
                        IDM_FONT_IVORY => Color::from_hex("#EAEAEA"),
                        IDM_FONT_SOFT => Color::from_hex("#C9D1D9"),
                        IDM_FONT_WHITE => Color::from_hex("#FFFFFF"),
                        _ => Color::from_hex("#EAEAEA"),
                    };
                    apply_widget_color_preset(None, Some(fg), None);
                }
                IDM_IND_CORAL | IDM_IND_MINT | IDM_IND_BLUE => {
                    let ind = match id {
                        IDM_IND_CORAL => Color::from_hex("#D97757"),
                        IDM_IND_MINT => Color::from_hex("#5AC79B"),
                        IDM_IND_BLUE => Color::from_hex("#5B8DEF"),
                        _ => Color::from_hex("#D97757"),
                    };
                    apply_widget_color_preset(None, None, Some(ind));
                }
                IDM_SCHEME_AUTO | IDM_SCHEME_LIGHT | IDM_SCHEME_DARK => {
                    let mode = match id {
                        IDM_SCHEME_AUTO => ColorSchemeMode::Auto,
                        IDM_SCHEME_LIGHT => ColorSchemeMode::Light,
                        IDM_SCHEME_DARK => ColorSchemeMode::Dark,
                        _ => ColorSchemeMode::Auto,
                    };
                    apply_widget_color_scheme(mode);
                }
                IDM_BORDER_NONE | IDM_BORDER_THIN | IDM_BORDER_MEDIUM | IDM_BORDER_THICK => {
                    let border = match id {
                        IDM_BORDER_NONE => 0,
                        IDM_BORDER_THIN => 1,
                        IDM_BORDER_MEDIUM => 2,
                        IDM_BORDER_THICK => 3,
                        _ => DEFAULT_BORDER_PX,
                    };
                    apply_widget_frame_preset(hwnd, Some(border), None, None);
                }
                IDM_PADDING_0 | IDM_PADDING_4 | IDM_PADDING_8 => {
                    let padding = match id {
                        IDM_PADDING_0 => 0,
                        IDM_PADDING_4 => 4,
                        IDM_PADDING_8 => 8,
                        _ => DEFAULT_PANEL_PADDING_PX,
                    };
                    apply_widget_frame_preset(hwnd, None, None, Some(padding));
                }
                IDM_MARGIN_0 | IDM_MARGIN_2 | IDM_MARGIN_4 => {
                    let margin = match id {
                        IDM_MARGIN_0 => 0,
                        IDM_MARGIN_2 => 2,
                        IDM_MARGIN_4 => 4,
                        _ => DEFAULT_PANEL_MARGIN_PX,
                    };
                    apply_widget_frame_preset(hwnd, None, Some(margin), None);
                }
                id if id == tray_icon::IDM_TOGGLE_WIDGET => {
                    toggle_widget_visibility(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ if msg == WM_APP_TRAY => {
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::ToggleWidget => {
                    toggle_widget_visibility(hwnd);
                }
                tray_icon::TrayAction::ShowContextMenu => {
                    show_context_menu(hwnd);
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let hook = {
                let state = lock_state();
                state.as_ref().and_then(|s| s.win_event_hook)
            };
            if let Some(h) = hook {
                native_interop::unhook_win_event(h);
            }
            tray_icon::remove(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let (
            current_interval,
            strings,
            language_override,
            widget_visible,
            widget_width_px,
            widget_height_px,
            background_color,
            font_color,
            indicator_color,
            border_width_px,
            panel_margin_px,
            panel_padding_px,
            color_scheme_mode,
        ) = {
            let state = lock_state();
            match state.as_ref() {
                Some(s) => (
                    s.poll_interval_ms,
                    s.language.strings(),
                    s.language_override,
                    s.widget_visible,
                    s.widget_width_px,
                    s.widget_height_px,
                    s.background_color,
                    s.font_color,
                    s.indicator_color,
                    s.border_width_px,
                    s.panel_margin_px,
                    s.panel_padding_px,
                    s.color_scheme_mode,
                ),
                None => (
                    POLL_15_MIN,
                    LanguageId::PortugueseBrazil.strings(),
                    None,
                    true,
                    0,
                    0,
                    Color::from_hex("#161616"),
                    Color::from_hex("#EAEAEA"),
                    Color::from_hex("#D97757"),
                    DEFAULT_BORDER_PX,
                    DEFAULT_PANEL_MARGIN_PX,
                    DEFAULT_PANEL_PADDING_PX,
                    ColorSchemeMode::Auto,
                ),
            }
        };

        let menu = CreatePopupMenu().unwrap();

        let refresh_str = native_interop::wide_str(strings.refresh);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            1,
            PCWSTR::from_raw(refresh_str.as_ptr()),
        );

        // Update Frequency submenu
        let freq_menu = CreatePopupMenu().unwrap();
        let freq_items: [(u16, u32, &str); 4] = [
            (IDM_FREQ_1MIN, POLL_1_MIN, strings.one_minute),
            (IDM_FREQ_5MIN, POLL_5_MIN, strings.five_minutes),
            (IDM_FREQ_15MIN, POLL_15_MIN, strings.fifteen_minutes),
            (IDM_FREQ_1HOUR, POLL_1_HOUR, strings.one_hour),
        ];
        for (id, interval, label) in freq_items {
            let label_str = native_interop::wide_str(label);
            let flags = if interval == current_interval {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                freq_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let freq_label = native_interop::wide_str(strings.update_frequency);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            freq_menu.0 as usize,
            PCWSTR::from_raw(freq_label.as_ptr()),
        );

        // Settings submenu
        let settings_menu = CreatePopupMenu().unwrap();

        let startup_str = native_interop::wide_str(strings.start_with_windows);
        let startup_flags = if is_startup_enabled() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            startup_flags,
            IDM_START_WITH_WINDOWS as usize,
            PCWSTR::from_raw(startup_str.as_ptr()),
        );

        let reset_pos_str = native_interop::wide_str(strings.reset_position);
        let _ = AppendMenuW(
            settings_menu,
            MENU_ITEM_FLAGS(0),
            IDM_RESET_POSITION as usize,
            PCWSTR::from_raw(reset_pos_str.as_ptr()),
        );

        let language_menu = CreatePopupMenu().unwrap();
        let system_label = native_interop::wide_str(strings.system_default);
        let system_flags = if language_override.is_none() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            language_menu,
            system_flags,
            IDM_LANG_SYSTEM as usize,
            PCWSTR::from_raw(system_label.as_ptr()),
        );

        for language in LanguageId::ALL {
            let id = match language {
                LanguageId::English => IDM_LANG_ENGLISH,
                LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
            };
            let label_str = native_interop::wide_str(language.native_name());
            let flags = if language_override == Some(language) {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                language_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let language_label = native_interop::wide_str(strings.language);
        let _ = AppendMenuW(
            settings_menu,
            MF_POPUP,
            language_menu.0 as usize,
            PCWSTR::from_raw(language_label.as_ptr()),
        );

        let customize_menu = CreatePopupMenu().unwrap();

        let size_menu = CreatePopupMenu().unwrap();
        let size_items = [
            (IDM_SIZE_AUTO, "Tamanho: Automatico", widget_width_px == 0 && widget_height_px == 0),
            (IDM_SIZE_COMPACT, "Tamanho: 320x38", widget_width_px == 320 && widget_height_px == 38),
            (IDM_SIZE_DEFAULT, "Tamanho: 360x44", widget_width_px == 360 && widget_height_px == 44),
            (IDM_SIZE_LARGE, "Tamanho: 400x50", widget_width_px == 400 && widget_height_px == 50),
            (IDM_SIZE_XL, "Tamanho: 440x56", widget_width_px == 440 && widget_height_px == 56),
        ];
        for (id, label, checked) in size_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                size_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let size_label = native_interop::wide_str("Tamanho (px)");
        let _ = AppendMenuW(
            customize_menu,
            MF_POPUP,
            size_menu.0 as usize,
            PCWSTR::from_raw(size_label.as_ptr()),
        );

        let colors_menu = CreatePopupMenu().unwrap();
        let bg_items = [
            (IDM_BG_CHARCOAL, "Fundo: Carvao", background_color == Color::from_hex("#161616")),
            (IDM_BG_SLATE, "Fundo: Ardósia", background_color == Color::from_hex("#1E242B")),
            (IDM_BG_BLACK, "Fundo: Preto", background_color == Color::from_hex("#0F0F0F")),
        ];
        for (id, label, checked) in bg_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                colors_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let font_items = [
            (IDM_FONT_IVORY, "Fonte: Marfim", font_color == Color::from_hex("#EAEAEA")),
            (IDM_FONT_SOFT, "Fonte: Suave", font_color == Color::from_hex("#C9D1D9")),
            (IDM_FONT_WHITE, "Fonte: Branca", font_color == Color::from_hex("#FFFFFF")),
        ];
        for (id, label, checked) in font_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                colors_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let indicator_items = [
            (IDM_IND_CORAL, "Indicadores: Coral", indicator_color == Color::from_hex("#D97757")),
            (IDM_IND_MINT, "Indicadores: Menta", indicator_color == Color::from_hex("#5AC79B")),
            (IDM_IND_BLUE, "Indicadores: Azul", indicator_color == Color::from_hex("#5B8DEF")),
        ];
        for (id, label, checked) in indicator_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                colors_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let colors_label = native_interop::wide_str("Cores");
        let _ = AppendMenuW(
            customize_menu,
            MF_POPUP,
            colors_menu.0 as usize,
            PCWSTR::from_raw(colors_label.as_ptr()),
        );

        let border_menu = CreatePopupMenu().unwrap();
        let border_items = [
            (IDM_BORDER_NONE, "Borda: 0px", border_width_px == 0),
            (IDM_BORDER_THIN, "Borda: 1px", border_width_px == 1),
            (IDM_BORDER_MEDIUM, "Borda: 2px", border_width_px == 2),
            (IDM_BORDER_THICK, "Borda: 3px", border_width_px == 3),
        ];
        for (id, label, checked) in border_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                border_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let border_label = native_interop::wide_str("Borda");
        let _ = AppendMenuW(
            customize_menu,
            MF_POPUP,
            border_menu.0 as usize,
            PCWSTR::from_raw(border_label.as_ptr()),
        );

        let spacing_menu = CreatePopupMenu().unwrap();
        let padding_items = [
            (IDM_PADDING_0, "Preenchimento: 0px", panel_padding_px == 0),
            (IDM_PADDING_4, "Preenchimento: 4px", panel_padding_px == 4),
            (IDM_PADDING_8, "Preenchimento: 8px", panel_padding_px == 8),
        ];
        for (id, label, checked) in padding_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                spacing_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let margin_items = [
            (IDM_MARGIN_0, "Margem: 0px", panel_margin_px == 0),
            (IDM_MARGIN_2, "Margem: 2px", panel_margin_px == 2),
            (IDM_MARGIN_4, "Margem: 4px", panel_margin_px == 4),
        ];
        for (id, label, checked) in margin_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                spacing_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let spacing_label = native_interop::wide_str("Espacamento");
        let _ = AppendMenuW(
            customize_menu,
            MF_POPUP,
            spacing_menu.0 as usize,
            PCWSTR::from_raw(spacing_label.as_ptr()),
        );

        let scheme_menu = CreatePopupMenu().unwrap();
        let scheme_items = [
            (IDM_SCHEME_AUTO, "Tema: Automatico", color_scheme_mode == ColorSchemeMode::Auto),
            (IDM_SCHEME_LIGHT, "Tema: Claro", color_scheme_mode == ColorSchemeMode::Light),
            (IDM_SCHEME_DARK, "Tema: Escuro", color_scheme_mode == ColorSchemeMode::Dark),
        ];
        for (id, label, checked) in scheme_items {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                scheme_menu,
                if checked { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let scheme_label = native_interop::wide_str("Tema de Cores");
        let _ = AppendMenuW(
            customize_menu,
            MF_POPUP,
            scheme_menu.0 as usize,
            PCWSTR::from_raw(scheme_label.as_ptr()),
        );

        let customize_label = native_interop::wide_str("Personalizacao");
        let _ = AppendMenuW(
            settings_menu,
            MF_POPUP,
            customize_menu.0 as usize,
            PCWSTR::from_raw(customize_label.as_ptr()),
        );

        let settings_label = native_interop::wide_str(strings.settings);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            settings_menu.0 as usize,
            PCWSTR::from_raw(settings_label.as_ptr()),
        );

        let widget_label = native_interop::wide_str(strings.show_widget);
        let widget_flags = if widget_visible { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
        let _ = AppendMenuW(
            menu,
            widget_flags,
            tray_icon::IDM_TOGGLE_WIDGET as usize,
            PCWSTR::from_raw(widget_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let exit_str = native_interop::wide_str(strings.exit);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            2,
            PCWSTR::from_raw(exit_str.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

/// Paint for non-embedded fallback (normal WM_PAINT path)
fn paint(hdc: HDC, hwnd: HWND) {
    let (
        is_dark,
        strings,
        session_pct,
        session_text,
        weekly_pct,
        weekly_text,
        panel_bg,
        panel_text,
        panel_indicator,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.is_dark,
                s.language.strings(),
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
                s.background_color,
                s.font_color,
                s.indicator_color,
            ),
            None => return,
        }
    };

    let accent = panel_indicator;
    let track = shade_color(panel_indicator, -88);
    let text_color = panel_text;
    let bg_color = if is_dark {
        Color::from_hex("#1C1C1C")
    } else {
        Color::from_hex("#F3F3F3")
    };

    unsafe {
        let mut client_rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut client_rect);
        let width = client_rect.right - client_rect.left;
        let height = client_rect.bottom - client_rect.top;

        if width <= 0 || height <= 0 {
            return;
        }

        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
        let old_bmp = SelectObject(mem_dc, mem_bmp);

        paint_content(
            mem_dc,
            width,
            height,
            is_dark,
            &bg_color,
            &panel_bg,
            &text_color,
            &accent,
            &track,
            strings,
            session_pct,
            &session_text,
            weekly_pct,
            &weekly_text,
        );

        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
    }
}

fn apply_widget_size_preset(hwnd: HWND, width_px: i32, height_px: i32) {
    CUSTOM_WIDGET_WIDTH_PX.store(width_px, Ordering::Relaxed);
    CUSTOM_WIDGET_HEIGHT_PX.store(height_px, Ordering::Relaxed);
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.widget_width_px = width_px;
            s.widget_height_px = height_px;
        }
    }
    save_state_settings();
    position_at_taskbar();
    render_layered();
    let _ = hwnd;
}

fn apply_widget_color_preset(
    background: Option<Color>,
    font: Option<Color>,
    indicator: Option<Color>,
) {
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.color_scheme_mode = ColorSchemeMode::Custom;
            if let Some(color) = background {
                s.background_color = color;
            }
            if let Some(color) = font {
                s.font_color = color;
            }
            if let Some(color) = indicator {
                s.indicator_color = color;
            }
        }
    }
    save_state_settings();
    render_layered();
}

fn apply_widget_color_scheme(mode: ColorSchemeMode) {
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.color_scheme_mode = mode;
            apply_color_scheme_to_state(s);
        }
    }
    save_state_settings();
    render_layered();
}

fn apply_widget_frame_preset(hwnd: HWND, border_px: Option<i32>, margin_px: Option<i32>, padding_px: Option<i32>) {
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if let Some(value) = border_px {
                let clamped = value.clamp(0, 6);
                s.border_width_px = clamped;
                CUSTOM_BORDER_WIDTH_PX.store(clamped, Ordering::Relaxed);
            }
            if let Some(value) = margin_px {
                let clamped = value.clamp(0, 16);
                s.panel_margin_px = clamped;
                CUSTOM_PANEL_MARGIN_PX.store(clamped, Ordering::Relaxed);
            }
            if let Some(value) = padding_px {
                let clamped = value.clamp(0, 16);
                s.panel_padding_px = clamped;
                CUSTOM_PANEL_PADDING_PX.store(clamped, Ordering::Relaxed);
            }
        }
    }
    save_state_settings();
    position_at_taskbar();
    render_layered();
    let _ = hwnd;
}

fn draw_row(
    hdc: HDC,
    x: i32,
    y: i32,
    label: &str,
    percent: f64,
    text: &str,
    accent: &Color,
    track: &Color,
) {
    let seg_w = sc(SEGMENT_W);
    let seg_h = sc(SEGMENT_H);
    let seg_gap = sc(SEGMENT_GAP);
    let corner_r = sc(CORNER_RADIUS);

    unsafe {
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + sc(LABEL_WIDTH),
            bottom: y + seg_h,
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let bar_x = x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
        let percent_clamped = percent.clamp(0.0, 100.0);

        for i in 0..SEGMENT_COUNT {
            let seg_x = bar_x + i * (seg_w + seg_gap);
            let seg_start = (i as f64) * 10.0;
            let seg_end = seg_start + 10.0;

            let seg_rect = RECT {
                left: seg_x,
                top: y,
                right: seg_x + seg_w,
                bottom: y + seg_h,
            };

            if percent_clamped >= seg_end {
                draw_rounded_rect(hdc, &seg_rect, accent, corner_r);
            } else if percent_clamped <= seg_start {
                draw_rounded_rect(hdc, &seg_rect, track, corner_r);
            } else {
                draw_rounded_rect(hdc, &seg_rect, track, corner_r);
                let fraction = (percent_clamped - seg_start) / 10.0;
                let fill_width = (seg_w as f64 * fraction) as i32;
                if fill_width > 0 {
                    let fill_rect = RECT {
                        left: seg_x,
                        top: y,
                        right: seg_x + fill_width,
                        bottom: y + seg_h,
                    };
                    let rgn = CreateRoundRectRgn(
                        seg_rect.left,
                        seg_rect.top,
                        seg_rect.right + 1,
                        seg_rect.bottom + 1,
                        corner_r * 2,
                        corner_r * 2,
                    );
                    let _ = SelectClipRgn(hdc, rgn);
                    let brush = CreateSolidBrush(COLORREF(accent.to_colorref()));
                    FillRect(hdc, &fill_rect, brush);
                    let _ = DeleteObject(brush);
                    let _ = SelectClipRgn(hdc, HRGN::default());
                    let _ = DeleteObject(rgn);
                }
            }
        }

        let text_x = bar_x + SEGMENT_COUNT * (seg_w + seg_gap) - seg_gap + sc(BAR_RIGHT_MARGIN);
        let mut text_wide: Vec<u16> = text.encode_utf16().collect();
        let mut text_rect = RECT {
            left: text_x,
            top: y,
            right: text_x + sc(TEXT_WIDTH),
            bottom: y + seg_h,
        };
        let _ = DrawTextW(
            hdc,
            &mut text_wide,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
}

fn draw_rounded_rect(hdc: HDC, rect: &RECT, color: &Color, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let rgn = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let _ = FillRgn(hdc, rgn, brush);
        let _ = DeleteObject(rgn);
        let _ = DeleteObject(brush);
    }
}

fn draw_rounded_outline(hdc: HDC, rect: &RECT, color: &Color, radius: i32, width: i32) {
    unsafe {
        let rgn = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let _ = FrameRgn(hdc, rgn, brush, width.max(1), width.max(1));
        let _ = DeleteObject(brush);
        let _ = DeleteObject(rgn);
    }
}
