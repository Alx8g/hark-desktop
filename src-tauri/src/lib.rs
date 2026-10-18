//! Hark Desktop: a featherweight Tauri v2 shell around the real hark.com app,
//! for Windows, macOS and Linux. No Electron, no bundled Chromium: the
//! operating system's own web view renders Hark (WebView2, WKWebView or
//! WebKitGTK) and Rust owns everything native (tray residence, notifications,
//! Handoff plan progress, autostart, global shortcut, downloads).
//! Page-side behaviour lives in `bridge.js`, injected at document start.
//!
//! Windows is the reference platform: it gets the custom frameless, rounded
//! window. macOS keeps a native window frame with a transparent title bar and
//! Linux keeps the window manager's frame, because transparent undecorated
//! windows are not dependable everywhere.
//!
//! Local windows: `splash` (the loading screen, drawn at the main window's own
//! size and place), `quick` (Ctrl+Shift+H), `island` (a running Handoff plan at
//! the top of the screen) and `settings`.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{
    AppHandle, Emitter, Listener, Manager, PhysicalPosition, Url, UserAttentionType, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_store::StoreExt;
use tauri_plugin_window_state::StateFlags;

const HARK_URL: &str = "https://hark.com/";
const BRIDGE_JS: &str = include_str!("bridge.js");
const TRAY_ID: &str = "main-tray";
const STORE_FILE: &str = "hark-settings.json";
/* The quick ask window: a transparent margin round the panel holds its
 * shadow. quick.ts resizes the height as the text grows; the width must match. */
const QUICK_SIZE: (f64, f64) = (700.0, 226.0);
/* The plan island window: room for the opened-up pill and its shadow. */
const ISLAND_SIZE: (f64, f64) = (520.0, 200.0);
/* Only Windows overrides the user agent (it presents as Edge, which is what
 * WebView2 is). macOS and Linux keep their web view's own agent string. */
#[cfg(windows)]
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0 ",
    "HarkDesktop/0.1.0"
);

/* Handed to the page before bridge.js runs, so the page only draws the
 * Windows-style window controls and rounded frame where they belong. */
const PLATFORM: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/* The quick-ask shortcut: Cmd+Shift+H on macOS, Ctrl+Shift+H elsewhere. */
const QUICK_SHORTCUT: &str = "CommandOrControl+Shift+H";
fn quick_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::SUPER | Modifiers::SHIFT
    } else {
        Modifiers::CONTROL | Modifiers::SHIFT
    }
}
/* Menu labels for the shortcuts above. */
fn quick_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Shift+H"
    } else {
        "Ctrl+Shift+H"
    }
}
fn settings_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+,"
    } else {
        "Ctrl+,"
    }
}

/* Windows 11 rounds frameless windows by only ~8px, which looks sharp next
 * to Hark's ~20px cards and full pills. The 20px silhouette is drawn by the
 * page itself (`html` clip-path in shell.css / bridge.js), so the compositor
 * anti-aliases it and it follows every resize frame.
 *
 * That only works if the OS window itself is a plain transparent rectangle.
 * Do NOT clip the top-level window with SetWindowRgn: a region is a 1-bit
 * mask (jagged corners), it makes Windows paint the legacy non-client frame
 * (pale edges, old caption buttons flashing through while resizing), and it
 * cuts away Tauri's corner resize grips. (The one region we do use, on the web
 * view's child window, is explained at `corners` below.)
 *
 * `shadow(false)` also matters: tao's frameless "shadow" insets the client
 * area by the invisible resize frame and lets DWM draw a 1px border and its
 * own 8px corner mask into it, all of which fights a transparent window. */
fn frameless<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    if cfg!(windows) {
        builder.decorations(false).transparent(true).shadow(false)
    } else if cfg!(target_os = "macos") {
        /* A transparent borderless window, rounded by the page's CSS, keeps
         * macOS' own shadow (it follows the window's alpha shape). */
        builder.decorations(false).transparent(true).shadow(true)
    } else {
        /* Linux: compositing (and so transparency) is not guaranteed, so these
         * small windows are plain borderless rectangles. */
        builder.decorations(false)
    }
}

/* Settings and the offline notice. Windows draws its own frameless window; on
 * macOS and Linux they are ordinary windows in the operating system's frame
 * (title bar, resize handles and a close button that behave as users expect). */
fn panel<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    if cfg!(windows) {
        frameless(builder)
    } else {
        builder
    }
}

/* The main window. Windows draws its own frame (see `frameless`). macOS keeps
 * its native frame with a transparent title bar and no title text, over a
 * background that matches the top of Hark's sky. Linux keeps the window
 * manager's frame. Neither of those needs the page to draw window controls. */
#[cfg(windows)]
fn main_chrome<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    frameless(builder)
}

#[cfg(target_os = "macos")]
fn main_chrome<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
        .title_bar_style(tauri::TitleBarStyle::Transparent)
        .hidden_title(true)
        .background_color(tauri::window::Color(127, 176, 221, 255))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main_chrome<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

#[cfg(windows)]
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
#[cfg(windows)]
const DWMWA_BORDER_COLOR: u32 = 34;
#[cfg(windows)]
const DWMWCP_DONOTROUND: u32 = 1;
#[cfg(windows)]
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

/* Windows 11 only: on Windows 10 these attributes just fail and are ignored. */
#[cfg(windows)]
fn dwm_set(hwnd: isize, attribute: u32, value: u32) {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    unsafe {
        DwmSetWindowAttribute(
            hwnd as *mut core::ffi::c_void,
            attribute,
            &value as *const u32 as *const core::ffi::c_void,
            core::mem::size_of::<u32>() as u32,
        );
    }
}

/* At rest, tell DWM to leave the window alone: no corner mask, no accent
 * border and (with it) no drop shadow around the transparent rectangle. */
#[cfg(windows)]
fn polish_frame(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as isize;
    dwm_set(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND);
    dwm_set(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
}

#[cfg(not(windows))]
fn polish_frame(_window: &WebviewWindow) {}

/* While an edge is being dragged the page always lags the window by a frame
 * or two. Shrinking, the window edge then crops the page's still-too-wide
 * previous frame, so its smooth rounded corners are cut off and the window
 * looks square ("cropping an image"). The page cannot repaint any faster, and
 * Windows' own rounding is only 8px (and brings back a shadow), so keep the
 * corner shape at the OS level, in step with every size change.
 *
 * The window region is a deliberately *looser* corner (RADIUS 17 vs the
 * page's 20) so the page's anti-aliased curve always lies entirely inside it:
 * at rest the region is invisible and the corners are the page's smooth ones;
 * only while the page lags does the region's (1-bit) edge show, and then as a
 * 17px corner instead of a cropped one.
 *
 * Applied straight from the Resized event with plain Win32 calls: no
 * cross-thread getters and no redraw (`SetWindowRgn(.., FALSE)`), which is
 * what made the old region-based version lag. Maximized and full-screen
 * windows get no region so they sit flush against the monitor. */
#[cfg(windows)]
mod corners {
    use core::ffi::c_void;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateRoundRectRgn(l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> *mut c_void;
        fn DeleteObject(object: *mut c_void) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowRgn(hwnd: *mut c_void, region: *mut c_void, redraw: i32) -> i32;
        fn IsZoomed(hwnd: *mut c_void) -> i32;
        fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn FindWindowExW(
            parent: *mut c_void,
            after: *mut c_void,
            class: *const u16,
            title: *const u16,
        ) -> *mut c_void;
        fn MonitorFromWindow(hwnd: *mut c_void, flags: u32) -> *mut c_void;
        fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
    }

    /// Logical px. Must stay below the page's 20px radius (see bridge.js,
    /// shell.css): a looser corner never clips the page's anti-aliased edge.
    const RADIUS: f64 = 17.0;

    /// Size the region to a `width` x `height` window (physical px).
    ///
    /// The region goes on the web view's own child window, NOT the top-level
    /// window: Tauri's resize grips are a sibling child whose 4px corner boxes
    /// sit exactly in the corners a region would cut away, so a top-level
    /// region would make corner-dragging impossible. Clipping only the page
    /// keeps every grip, and click-through never happens on the corners.
    pub fn apply(hwnd: isize, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let top = hwnd as *mut c_void;
        unsafe {
            let class: Vec<u16> = "WRY_WEBVIEW\0".encode_utf16().collect();
            let webview = FindWindowExW(
                top,
                core::ptr::null_mut(),
                class.as_ptr(),
                core::ptr::null(),
            );
            if webview.is_null() {
                return;
            }
            if IsZoomed(top) != 0 || fills_monitor(top, width, height) {
                SetWindowRgn(webview, core::ptr::null_mut(), 0);
                return;
            }
            let dpi = GetDpiForWindow(top).max(96) as f64;
            let diameter = (2.0 * RADIUS * dpi / 96.0).round() as i32;
            // The right/bottom edges are exclusive, hence +1.
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
            if region.is_null() {
                return;
            }
            // On success the system owns the region; on failure we must free it.
            if SetWindowRgn(webview, region, 0) == 0 {
                DeleteObject(region);
            }
        }
    }

    /// Size the region to the window's current size (used once at creation).
    pub fn apply_current(hwnd: isize) {
        let mut r = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            if GetWindowRect(hwnd as *mut c_void, &mut r) != 0 {
                apply(hwnd, r.right - r.left, r.bottom - r.top);
            }
        }
    }

    unsafe fn fills_monitor(h: *mut c_void, width: i32, height: i32) -> bool {
        const MONITOR_DEFAULTTONEAREST: u32 = 2;
        let monitor = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return false;
        }
        let mut info = MonitorInfo {
            size: core::mem::size_of::<MonitorInfo>() as u32,
            monitor: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            work: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            flags: 0,
        };
        GetMonitorInfoW(monitor, &mut info) != 0
            && width >= info.monitor.right - info.monitor.left
            && height >= info.monitor.bottom - info.monitor.top
    }
}

#[cfg(windows)]
fn hold_corners(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as isize;
    corners::apply_current(hwnd);
    window.on_window_event(move |event| {
        if let WindowEvent::Resized(size) = event {
            corners::apply(hwnd, size.width as i32, size.height as i32);
        }
    });
}

#[cfg(not(windows))]
fn hold_corners(_window: &WebviewWindow) {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub autostart: bool,
    pub close_to_tray: bool,
    pub notifications: bool,
    pub taskbar_progress: bool,
    /* Enter in the quick ask sends the message; off, Hark opens with it drafted. */
    pub quick_sends: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            /* Windows has always started with the session (hidden, to the tray).
             * On macOS and Linux that is opt-in. */
            autostart: cfg!(windows),
            /* Not every Linux desktop has a tray (GNOME needs an extension), so
             * closing the window must not hide it somewhere unreachable there. */
            close_to_tray: !cfg!(target_os = "linux"),
            notifications: true,
            taskbar_progress: true,
            quick_sends: true,
        }
    }
}

pub struct AppState {
    settings: Mutex<Settings>,
    quitting: AtomicBool,
    /* Hark has painted once and the loading screen is gone. */
    first_load_done: AtomicBool,
    /* Keep Hark in the tray when it has loaded: started at sign-in, or the
     * loading screen was closed. */
    start_hidden: AtomicBool,
    /* The main window's last navigation has finished, well or badly. */
    page_finished: AtomicBool,
    /* ...and bridge.js reported from it, so it really is hark.com. */
    page_ok: AtomicBool,
    /* Quick-ask text that arrived before Hark had loaded. Also guards the
     * hand-over in reveal_main, so no text is lost in between. */
    pending_compose: Mutex<Option<String>>,
    unread: AtomicI64,
    plans: Mutex<Plans>,
    island_rect: Mutex<IslandRect>,
    tray_menu: Mutex<Option<TrayMenu>>,
    appearance: Mutex<Appearance>,
    notified: Mutex<Vec<Instant>>,
    /* One worker brings the island and tray up to date (refresh_plans_soon). */
    refresh: (Mutex<bool>, Condvar),
    /* Whether the island is on screen; the pointer watcher sleeps when not. */
    island_shown: (Mutex<bool>, Condvar),
    island_geo: Mutex<IslandGeo>,
    /* Cleared if the island window could not be built: notify instead. */
    island_ok: AtomicBool,
    /* What the tray last showed, so unchanged updates are skipped. */
    tray_key: Mutex<String>,
    /* The last taskbar progress, put back when Plan progress is turned on. */
    last_progress: Mutex<Option<u64>>,
    /* The last Hark (or sign-in) page that finished loading in the main window. */
    last_page: Mutex<Option<Url>>,
    /* What Hark's page was last told about being seen (sync_shell), -1 at first. */
    shell_key: AtomicI64,
}

/* Where the island window is, in the same units as the pointer: physical
 * pixels on Windows and Linux, points on macOS. rect_scale turns the page's
 * CSS pixels into those units; cursor_scale turns the pointer into them. */
#[derive(Debug, Clone, Copy)]
struct IslandGeo {
    x: f64,
    y: f64,
    rect_scale: f64,
    cursor_scale: f64,
}

impl Default for IslandGeo {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            rect_scale: 1.0,
            cursor_scale: 1.0,
        }
    }
}

/* ------------------------------ appearance ------------------------------ */

/* Hark's theme ("light" or "dark") and the sky image it is showing, reported
 * by bridge.js and kept between runs, so the shell's own windows match Hark
 * before it has even loaded. */
#[derive(Debug, Clone, Default, Serialize)]
struct Appearance {
    theme: String,
    sky: Option<String>,
}

/* Only the small inline image Hark keeps in its stylesheet is accepted. */
fn valid_sky(sky: &str) -> bool {
    let body = [
        "data:image/webp;base64,",
        "data:image/png;base64,",
        "data:image/jpeg;base64,",
    ]
    .iter()
    .find_map(|prefix| sky.strip_prefix(prefix));
    match body {
        Some(body) => {
            sky.len() < 60_000
                && !body.is_empty()
                && body
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        }
        None => false,
    }
}

fn valid_theme(theme: &str) -> Option<String> {
    matches!(theme, "light" | "dark").then(|| theme.to_string())
}

/* Runs before each local page's own scripts: sets data-theme and the sky on
 * <html> as soon as it exists, so the page paints in Hark's look straight
 * away (src/appearance.ts keeps it up to date afterwards). */
fn appearance_script(app: &AppHandle) -> String {
    let appearance = app
        .state::<AppState>()
        .appearance
        .lock()
        .map(|a| a.clone())
        .unwrap_or_default();
    let json = serde_json::to_string(&appearance).unwrap_or_else(|_| "{}".to_string());
    format!(
        r#"window.__HARK_APPEARANCE__ = {json};
(function (a) {{
  function apply() {{
    var root = document.documentElement;
    if (!root) return false;
    if (a.theme) root.setAttribute("data-theme", a.theme);
    if (a.sky) {{ root.style.setProperty("--sky-image", 'url("' + a.sky + '") center / cover no-repeat'); root.setAttribute("data-sky", ""); }}
    return true;
  }}
  if (!apply()) {{ var wait = new MutationObserver(function () {{ if (apply()) wait.disconnect(); }}); wait.observe(document, {{ childList: true }}); }}
}})(window.__HARK_APPEARANCE__);"#
    )
}

fn load_appearance(app: &AppHandle) -> Appearance {
    let Ok(store) = app.store(STORE_FILE) else {
        return Appearance::default();
    };
    let text = |key: &str| store.get(key).and_then(|v| v.as_str().map(str::to_string));
    Appearance {
        theme: text("theme")
            .and_then(|t| valid_theme(&t))
            .unwrap_or_default(),
        sky: text("sky").filter(|s| valid_sky(s)),
    }
}

/* ------------------------------ plan state ------------------------------ */

/* One Handoff plan, as bridge.js reads it from Hark's own plan buttons. */
#[derive(Debug, Clone, Default, Deserialize)]
struct PlanInfo {
    title: String,
    done: u64,
    total: u64,
}

impl PlanInfo {
    fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.done as f64 / self.total as f64
        }
    }
}

/* What the island and the tray show. phase: "running", "done" or "idle". */
#[derive(Debug, Clone, Serialize)]
struct PlanView {
    phase: &'static str,
    title: String,
    done: u64,
    total: u64,
    others: usize,
}

impl PlanView {
    fn idle() -> Self {
        Self {
            phase: "idle",
            title: String::new(),
            done: 0,
            total: 0,
            others: 0,
        }
    }
}

#[derive(Default)]
struct Plans {
    active: Vec<PlanInfo>,
    /* The last plan to finish, until it is opened, dismissed or seen in Hark. */
    finished: Option<PlanInfo>,
    /* "Hide until it's done" was pressed for this plan. */
    hidden: Option<String>,
}

impl Plans {
    /* The plan furthest along, or the one that just finished. The island
     * leaves out a hidden plan; the tray icon still counts it. */
    fn view(&self, include_hidden: bool) -> PlanView {
        let shown: Vec<&PlanInfo> = self
            .active
            .iter()
            .filter(|p| include_hidden || self.hidden.as_deref() != Some(p.title.as_str()))
            .collect();
        if let Some(best) = shown
            .iter()
            .max_by(|a, b| a.fraction().total_cmp(&b.fraction()))
        {
            return PlanView {
                phase: "running",
                title: best.title.clone(),
                done: best.done,
                total: best.total,
                others: shown.len() - 1,
            };
        }
        match &self.finished {
            Some(p) => PlanView {
                phase: "done",
                title: p.title.clone(),
                done: p.total,
                total: p.total,
                others: 0,
            },
            None => PlanView::idle(),
        }
    }
}

/* Where the island's pill sits inside its window (CSS px), and whether it is
 * fully on screen (not tucked away) and so should catch the mouse. */
#[derive(Debug, Clone, Copy, Default)]
struct IslandRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    solid: bool,
}

/* The tray menu items that change while Hark runs. */
struct TrayMenu {
    menu: Menu<Wry>,
    open: MenuItem<Wry>,
    plan: MenuItem<Wry>,
    plan_gap: PredefinedMenuItem<Wry>,
    plan_shown: bool,
}

fn load_settings(app: &AppHandle) -> Settings {
    let mut settings = Settings::default();
    if let Ok(store) = app.store(STORE_FILE) {
        let grab = |key: &str, slot: &mut bool| {
            if let Some(Value::Bool(v)) = store.get(key) {
                *slot = v;
            }
        };
        grab("autostart", &mut settings.autostart);
        grab("close_to_tray", &mut settings.close_to_tray);
        grab("notifications", &mut settings.notifications);
        grab("taskbar_progress", &mut settings.taskbar_progress);
        grab("quick_sends", &mut settings.quick_sends);
    }
    settings
}

fn persist_settings(app: &AppHandle, settings: &Settings) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set("autostart", json!(settings.autostart));
        store.set("close_to_tray", json!(settings.close_to_tray));
        store.set("notifications", json!(settings.notifications));
        store.set("taskbar_progress", json!(settings.taskbar_progress));
        store.set("quick_sends", json!(settings.quick_sends));
        let _ = store.save();
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let manager = app.autolaunch();
    let _ = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
}

fn settings_snapshot(app: &AppHandle) -> Settings {
    app.state::<AppState>()
        .settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

/* ------------------------------- commands ------------------------------- */

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    settings_snapshot(&app)
}

#[tauri::command]
fn set_setting(app: AppHandle, key: String, value: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    let snapshot = {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        match key.as_str() {
            "autostart" => settings.autostart = value,
            "close_to_tray" => settings.close_to_tray = value,
            "notifications" => settings.notifications = value,
            "taskbar_progress" => settings.taskbar_progress = value,
            "quick_sends" => settings.quick_sends = value,
            other => return Err(format!("unknown setting: {other}")),
        }
        settings.clone()
    };
    if key == "autostart" {
        apply_autostart(&app, value);
    }
    if key == "taskbar_progress" {
        let last = app
            .state::<AppState>()
            .last_progress
            .lock()
            .map(|v| *v)
            .unwrap_or(None);
        set_taskbar_progress(&app, if value { last } else { None });
        refresh_plans_soon(&app);
    }
    persist_settings(&app, &snapshot);
    Ok(())
}

#[tauri::command]
fn test_notification(app: AppHandle) {
    notify(
        &app,
        "Hark",
        "Notifications are on. Briefs and finished plans will land here.",
        true,
    );
}

/* Commands that show, hide, build or close a window are async: a synchronous
 * command runs on the main thread inside WebView2's own callback, where
 * building a window deadlocks on Windows. */
#[tauri::command(async)]
fn reload_main(app: AppHandle) {
    show_main(&app);
    if let Some(main) = app.get_webview_window("main") {
        if let Ok(url) = Url::parse(HARK_URL) {
            let _ = main.navigate(url);
        }
    }
}

/* Has Hark painted? The loading screen checks before it fades, so only the
 * shell (not a stray event) can end it. */
#[tauri::command]
fn app_loaded(app: AppHandle) -> bool {
    app.state::<AppState>()
        .first_load_done
        .load(Ordering::SeqCst)
}

#[tauri::command(async)]
fn quick_compose(app: AppHandle, text: String) {
    if let Some(quick) = app.get_webview_window("quick") {
        let _ = quick.hide();
    }
    let state = app.state::<AppState>();
    let loaded = {
        // Checked and stored under the same lock reveal_main takes the text with.
        let Ok(mut pending) = state.pending_compose.lock() else {
            return;
        };
        let loaded = state.first_load_done.load(Ordering::SeqCst);
        if !loaded && !text.is_empty() {
            *pending = Some(match pending.take() {
                Some(earlier) => format!("{earlier}\n\n{text}"),
                None => text.clone(),
            });
        }
        loaded
    };
    show_main(&app);
    if loaded {
        // Give the window a beat to foreground before the page receives the text.
        deliver_compose(&app, text, 180);
    }
}

/* Hand quick-ask text to Hark's message box; bridge.js sends it too unless
 * "Send from quick ask" is off. */
fn deliver_compose(app: &AppHandle, text: String, delay_ms: u64) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        let send = settings_snapshot(&handle).quick_sends;
        let _ = handle.emit_to(
            "main",
            "hark-compose",
            json!({ "text": text, "send": send }),
        );
    });
}

/* Load hark.com again in the hidden main window ("Try again", or a retry). */
fn retry_load(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        if let Ok(url) = Url::parse(HARK_URL) {
            let _ = main.navigate(url);
        }
    }
}

#[tauri::command(async)]
fn splash_retry(app: AppHandle) {
    retry_load(&app);
}

/* Loading screen closed, or "Hide to tray": Hark keeps loading in the tray. */
#[tauri::command(async)]
fn splash_hide(app: AppHandle) {
    // "Hide to tray" (Windows) and "Hide" (macOS) mean it. On Linux, where the
    // tray may not exist, the window's own rule decides (see the run loop).
    if settings_snapshot(&app).close_to_tray || !cfg!(target_os = "linux") {
        app.state::<AppState>()
            .start_hidden
            .store(true, Ordering::SeqCst);
    }
    if let Some(splash) = app.get_webview_window("splash") {
        let _ = splash.close();
    }
}

#[tauri::command]
fn island_state(app: AppHandle) -> PlanView {
    // Idle while the island is on its way out, so the pill lifts away.
    if island_wanted(&app) {
        island_view(&app)
    } else {
        PlanView::idle()
    }
}

#[tauri::command]
fn island_rect(app: AppHandle, x: f64, y: f64, w: f64, h: f64, solid: bool) {
    if let Ok(mut rect) = app.state::<AppState>().island_rect.lock() {
        *rect = IslandRect { x, y, w, h, solid };
    }
}

/* The island's Open: bring Hark forward on that plan. */
#[tauri::command(async)]
fn island_open(app: AppHandle) {
    let title = island_view(&app).title;
    open_plan(&app, title);
}

#[tauri::command(async)]
fn island_hide_plan(app: AppHandle) {
    let title = island_view(&app).title;
    if let Ok(mut plans) = app.state::<AppState>().plans.lock() {
        plans.hidden = Some(title);
    }
    refresh_plans_soon(&app);
}

#[tauri::command(async)]
fn island_dismiss(app: AppHandle) {
    if let Ok(mut plans) = app.state::<AppState>().plans.lock() {
        plans.finished = None;
    }
    refresh_plans_soon(&app);
}

/* ---------------------------- native helpers ---------------------------- */

fn notify(app: &AppHandle, title: &str, body: &str, force: bool) {
    if !force && !settings_snapshot(app).notifications {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
    if let Some(main) = app.get_webview_window("main") {
        let focused = main.is_focused().unwrap_or(false);
        let visible = main.is_visible().unwrap_or(false);
        if !focused || !visible {
            let _ = main.request_user_attention(Some(UserAttentionType::Informational));
        }
    }
}

fn set_taskbar_progress(app: &AppHandle, value: Option<u64>) {
    if let Some(main) = app.get_webview_window("main") {
        let state = match value {
            Some(pct) => ProgressBarState {
                status: Some(ProgressBarStatus::Normal),
                progress: Some(pct.min(100)),
            },
            None => ProgressBarState {
                status: Some(ProgressBarStatus::None),
                progress: None,
            },
        };
        let _ = main.set_progress_bar(state);
    }
}

/* Bring Hark forward. While hark.com is still loading, the loading screen
 * stands in for it (and loading is retried if the last attempt failed). */
fn show_main(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.first_load_done.load(Ordering::SeqCst) {
        state.start_hidden.store(false, Ordering::SeqCst);
        if state.page_finished.load(Ordering::SeqCst) && !state.page_ok.load(Ordering::SeqCst) {
            retry_load(app);
        }
        open_splash(app);
        return;
    }
    show_main_now(app);
}

/* With HARK_DESKTOP_LOG set, key moments go to stderr: how the smoke test
 * sees inside the app on machines nobody can sit at. Off otherwise. */
fn trace(what: &str) {
    if std::env::var_os("HARK_DESKTOP_LOG").is_some() {
        eprintln!("[hark-desktop] {what}");
    }
}

/* Tell Hark's page whether you can see it ("rest") and whether you are using
 * it ("front"). At rest, bridge.js pauses Hark's animations, so the animated
 * sky costs next to nothing in the tray, while the page stays fully awake for
 * messages, notifications and plans. (Hiding the page from WebView2 instead
 * makes it stall for minutes at a time.) "front" decides notifications: the
 * page can't tell for itself, as WebView2 reports focus to a hidden window. */
fn sync_shell(app: &AppHandle, force: bool) {
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let shown = main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false);
    let front = shown && main.is_focused().unwrap_or(false);
    let key = i64::from(shown) | (i64::from(front) << 1);
    if app
        .state::<AppState>()
        .shell_key
        .swap(key, Ordering::SeqCst)
        == key
        && !force
    {
        return;
    }
    let _ = app.emit_to(
        "main",
        "hark-shell",
        json!({ "rest": !shown, "front": front }),
    );
}

fn show_main_now(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.request_user_attention(None);
    }
    sync_shell(app, false);
}

/* Closing the window keeps Hark in the tray. The first time, say so. */
fn hide_to_tray(app: &AppHandle, main: &WebviewWindow) {
    let _ = main.hide();
    sync_shell(app, false);
    if let Ok(store) = app.store(STORE_FILE) {
        if store.get("told_about_tray").and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        store.set("told_about_tray", json!(true));
        let _ = store.save();
    }
    let body = if cfg!(target_os = "macos") {
        "Hark keeps running in the menu bar. Quit it from there, or with Cmd+Q."
    } else {
        "Hark keeps running in the tray. Click its icon to come back, or quit from its menu."
    };
    notify(app, "Hark is still running", body, true);
}

fn toggle_main(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let visible = main.is_visible().unwrap_or(false);
        let focused = main.is_focused().unwrap_or(false);
        if visible && focused {
            let _ = main.hide();
            sync_shell(app, false);
        } else {
            show_main(app);
        }
    }
}

fn focus_composer(app: &AppHandle) {
    show_main(app);
    let _ = app.emit_to("main", "hark-focus-composer", json!({}));
}

fn quit(app: &AppHandle) {
    app.state::<AppState>()
        .quitting
        .store(true, Ordering::SeqCst);
    app.exit(0);
}

/* macOS expects an app menu: Cmd+Q must really quit (the exit guard above would
 * otherwise keep Hark alive in the menu bar) and the Edit items are what make
 * copy/paste/undo work inside the web view. */
#[cfg(target_os = "macos")]
fn install_macos_menu(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::Submenu;

    let quit_item = MenuItem::with_id(app, "app-quit", "Quit Hark", true, Some("Cmd+Q"))?;
    let settings = MenuItem::with_id(app, "app-settings", "Settings…", true, Some("Cmd+,"))?;
    let app_menu = Submenu::with_items(
        app,
        "Hark",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Hark Desktop"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let menu = Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| match event.id().as_ref() {
        "app-quit" => quit(app),
        "app-settings" => open_settings(app),
        _ => {}
    });
    Ok(())
}

fn open_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    if let Ok(win) = panel(WebviewWindowBuilder::new(
        app,
        "settings",
        WebviewUrl::App("settings.html".into()),
    ))
    .title("Hark Desktop settings")
    .initialization_script(appearance_script(app))
    .inner_size(540.0, 690.0)
    .min_inner_size(460.0, 520.0)
    .center()
    .resizable(true)
    .build()
    {
        polish_frame(&win);
        hold_corners(&win);
    }
}

/* The pointer, in the units monitor_from_point expects (points on macOS). */
fn pointer_for_monitor_lookup(app: &AppHandle) -> Option<(f64, f64)> {
    let p = app.cursor_position().ok()?;
    if cfg!(target_os = "macos") {
        let scale = app.primary_monitor().ok().flatten()?.scale_factor();
        Some((p.x / scale, p.y / scale))
    } else {
        Some((p.x, p.y))
    }
}

/* Put a window of `width` (logical px) centred across a monitor's work area,
 * `down` of the way down it. Windows and Linux in physical pixels; macOS in
 * points, which is what its monitor and pointer coordinates are based on. */
fn place_on(window: &WebviewWindow, monitor: &tauri::Monitor, width: f64, down: f64) {
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    if cfg!(target_os = "macos") {
        let x =
            f64::from(area.position.x) / scale + (f64::from(area.size.width) / scale - width) / 2.0;
        let y = f64::from(area.position.y) / scale + f64::from(area.size.height) / scale * down;
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    } else {
        let w = (width * scale).round() as i32;
        let x = area.position.x + (area.size.width as i32 - w) / 2;
        let y = area.position.y + (f64::from(area.size.height) * down).round() as i32;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

/* The quick ask opens in the upper part of the screen the pointer is on. */
fn place_quick(app: &AppHandle, quick: &WebviewWindow) {
    let monitor = pointer_for_monitor_lookup(app)
        .and_then(|(x, y)| app.monitor_from_point(x, y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        place_on(quick, &monitor, QUICK_SIZE.0, 0.18);
    }
}

fn toggle_quick(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("quick") {
        if win.is_visible().unwrap_or(false) {
            // The page fades out and hides itself, so its last frame is never
            // the first thing seen next time. Hide it here if it doesn't.
            let _ = app.emit_to("quick", "quick-hide", json!({}));
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                if win.is_visible().unwrap_or(false) {
                    let _ = win.hide();
                }
            });
        } else {
            place_quick(app, &win);
            let _ = win.show();
            let _ = win.set_focus();
        }
        return;
    }
    /* The panel draws its own rounded card and soft shadow inside the window's
     * transparent margin (quick.html), so there is no full-window frame here. */
    if let Ok(win) = frameless(WebviewWindowBuilder::new(
        app,
        "quick",
        WebviewUrl::App("quick.html".into()),
    ))
    .title("Ask Hark")
    .always_on_top(true)
    .skip_taskbar(true)
    .inner_size(QUICK_SIZE.0, QUICK_SIZE.1)
    .resizable(false)
    .visible(false)
    .initialization_script(appearance_script(app))
    .build()
    {
        polish_frame(&win);
        place_quick(app, &win);
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/* ------------------------------ plan island ------------------------------ */

fn island_view(app: &AppHandle) -> PlanView {
    app.state::<AppState>()
        .plans
        .lock()
        .map(|p| p.view(false))
        .unwrap_or_else(|_| PlanView::idle())
}

fn main_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .map(|w| {
            w.is_visible().unwrap_or(false)
                && w.is_focused().unwrap_or(false)
                && !w.is_minimized().unwrap_or(false)
        })
        .unwrap_or(false)
}

/* The island shows while a plan runs (or has just finished) and Hark itself
 * is not in front. Linux has no dependable transparency, so it relies on the
 * tray icon and a notification instead. */
fn island_wanted(app: &AppHandle) -> bool {
    !cfg!(target_os = "linux")
        && settings_snapshot(app).taskbar_progress
        && island_view(app).phase != "idle"
        && !main_in_front(app)
        && !screen_busy()
}

/* Someone is gaming, presenting or watching something full screen (or the
 * screen is locked): the island keeps out of the way, as Windows' own
 * notifications do. The same signal Windows uses for them. */
#[cfg(windows)]
fn screen_busy() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn SHQueryUserNotificationState(state: *mut i32) -> i32;
    }
    // QUNS_NOT_PRESENT 1, QUNS_BUSY 2, QUNS_RUNNING_D3D_FULL_SCREEN 3, QUNS_PRESENTATION_MODE 4
    let mut state = 0i32;
    unsafe { SHQueryUserNotificationState(&mut state) == 0 && matches!(state, 1..=4) }
}

#[cfg(not(windows))]
fn screen_busy() -> bool {
    false
}

fn open_plan(app: &AppHandle, title: String) {
    if let Ok(mut plans) = app.state::<AppState>().plans.lock() {
        if plans.finished.as_ref().map(|p| p.title == title) == Some(true) {
            plans.finished = None;
        }
    }
    show_main(app);
    if !title.is_empty() {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            let _ = handle.emit_to("main", "hark-open-plan", json!({ "title": title }));
        });
    }
    refresh_plans_soon(app);
}

/* A plan is running or has just finished, and plan progress is on. */
fn plan_active(app: &AppHandle) -> bool {
    settings_snapshot(app).taskbar_progress && island_view(app).phase != "idle"
}

/* Ask the refresh worker to bring the island and the tray up to date. Cheap
 * and safe from any thread; bursts of requests collapse into one refresh. */
fn refresh_plans_soon(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (dirty, wake) = &state.refresh;
    if let Ok(mut dirty) = dirty.lock() {
        *dirty = true;
        wake.notify_one();
    };
}

/* The only place the island and tray are changed, one refresh at a time, so
 * two refreshes can never race (two islands, or an older view winning). It
 * runs off the main thread, where building the island window is safe. */
fn start_refresh_worker(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        {
            let state = app.state::<AppState>();
            let (dirty, wake) = &state.refresh;
            let Ok(mut pending) = dirty.lock() else {
                return;
            };
            while !*pending {
                pending = match wake.wait(pending) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            *pending = false;
        }
        if app.state::<AppState>().quitting.load(Ordering::SeqCst) {
            return;
        }
        refresh_plans(&app);
    });
}

fn set_island_shown(app: &AppHandle, shown: bool) {
    let state = app.state::<AppState>();
    let (flag, wake) = &state.island_shown;
    if let Ok(mut flag) = flag.lock() {
        *flag = shown;
        wake.notify_all();
    };
}

fn island_is_shown(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .island_shown
        .0
        .lock()
        .map(|f| *f)
        .unwrap_or(false)
}

/* Bring the island and the tray in line with the plans Hark is running.
 * Called by the refresh worker only. */
fn refresh_plans(app: &AppHandle) {
    update_tray(app);
    if cfg!(target_os = "linux") || !plan_active(app) {
        hide_island(app);
        return;
    }
    // Built (hidden) as soon as a plan is active, even while something is full
    // screen, so the pointer watcher is there to notice when that ends.
    let Some(island) = ensure_island(app) else {
        return;
    };
    if !island_wanted(app) {
        hide_island(app);
        return;
    }
    if !island_is_shown(app) {
        set_click_through(&island, true);
        place_island(app, &island);
        let _ = island.show();
        set_island_shown(app, true);
    }
    let _ = app.emit_to("island", "plan-state", island_view(app));
}

/* Let the pill lift away, then hide the window. */
fn hide_island(app: &AppHandle) {
    if !island_is_shown(app) {
        return;
    }
    let _ = app.emit_to("island", "plan-state", PlanView::idle());
    std::thread::sleep(Duration::from_millis(450));
    if island_wanted(app) {
        return;
    }
    if let Some(island) = app.get_webview_window("island") {
        let _ = island.hide();
    }
    set_island_shown(app, false);
}

fn ensure_island(app: &AppHandle) -> Option<WebviewWindow> {
    if cfg!(target_os = "linux") {
        return None;
    }
    if let Some(win) = app.get_webview_window("island") {
        return Some(win);
    }
    /* Never focusable: it must not take the keyboard from whatever you're doing. */
    let built = frameless(WebviewWindowBuilder::new(
        app,
        "island",
        WebviewUrl::App("island.html".into()),
    ))
    .title("Hark plan")
    .inner_size(ISLAND_SIZE.0, ISLAND_SIZE.1)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .focused(false)
    .focusable(false)
    .visible(false)
    .initialization_script(appearance_script(app));
    // macOS: a click on the island works even while Hark is in the background.
    #[cfg(target_os = "macos")]
    let built = built.accept_first_mouse(true);
    let Ok(win) = built.build() else {
        app.state::<AppState>()
            .island_ok
            .store(false, Ordering::SeqCst);
        return None;
    };
    polish_frame(&win);
    let _ = win.set_ignore_cursor_events(true);
    watch_island_pointer(app.clone());
    Some(win)
}

/* Top centre of the screen Hark's window is on. */
fn place_island(app: &AppHandle, island: &WebviewWindow) {
    let monitor = app
        .get_webview_window("main")
        .and_then(|m| m.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    place_on(island, &monitor, ISLAND_SIZE.0, 0.0);
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let geo = if cfg!(target_os = "macos") {
        let primary = app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.scale_factor())
            .unwrap_or(scale);
        IslandGeo {
            x: f64::from(area.position.x) / scale
                + (f64::from(area.size.width) / scale - ISLAND_SIZE.0) / 2.0,
            y: f64::from(area.position.y) / scale,
            rect_scale: 1.0,
            cursor_scale: primary,
        }
    } else {
        let w = (ISLAND_SIZE.0 * scale).round() as i32;
        IslandGeo {
            x: f64::from(area.position.x + (area.size.width as i32 - w) / 2),
            y: f64::from(area.position.y),
            rect_scale: scale,
            cursor_scale: 1.0,
        }
    };
    if let Ok(mut slot) = app.state::<AppState>().island_geo.lock() {
        *slot = geo;
    }
}

/* Whether the island lets clicks through to what is beneath it. On Windows
 * this sets the window style directly: going through tao re-shows the window
 * on every change, which can hand it the keyboard. */
#[cfg(windows)]
fn set_click_through(island: &WebviewWindow, through: bool) {
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: *mut core::ffi::c_void, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: *mut core::ffi::c_void, index: i32, value: isize) -> isize;
    }
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x20;
    let Ok(hwnd) = island.hwnd() else { return };
    let hwnd = hwnd.0;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let next = if through {
            style | WS_EX_TRANSPARENT
        } else {
            style & !WS_EX_TRANSPARENT
        };
        if next != style {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        }
    }
}

#[cfg(not(windows))]
fn set_click_through(island: &WebviewWindow, through: bool) {
    let _ = island.set_ignore_cursor_events(through);
}

/* The pointer, in island units (see IslandGeo). Windows reads it directly:
 * it is safe from any thread and needs no trip to the main thread. */
#[cfg(windows)]
fn pointer(_app: &AppHandle) -> Option<(f64, f64)> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| (f64::from(p.x), f64::from(p.y)))
}

#[cfg(not(windows))]
fn pointer(app: &AppHandle) -> Option<(f64, f64)> {
    app.cursor_position().ok().map(|p| (p.x, p.y))
}

/* Whether the pointer is over the pill. The pill hangs from the top of the
 * screen, so the gap between the screen edge and the pill counts as the pill:
 * pointing at the very edge opens it, and it stays open while the pointer is
 * anywhere between the edge and its bottom. A tucked pill is a few pixels
 * tall, so the edge gets a little more. */
fn over_pill(geo: &IslandGeo, rect: &IslandRect, px: f64, py: f64) -> bool {
    let (px, py) = (px / geo.cursor_scale, py / geo.cursor_scale);
    let left = geo.x + rect.x * geo.rect_scale;
    let right = left + rect.w * geo.rect_scale;
    let bottom = geo.y + ((rect.y + rect.h) * geo.rect_scale).max(8.0 * geo.rect_scale);
    px >= left && px <= right && py >= geo.y - 1.0 && py <= bottom
}

/* The island window lets every click through except on the pill itself. A
 * see-through window can't feel the mouse while it ignores it, so this
 * watches the pointer instead: it tells the page when the pointer is over the
 * pill (it opens up after a moment), and lets the pill take clicks only once
 * it is fully on screen. A tucked island never blocks the tabs beneath it.
 * While the island is hidden this sleeps, waking once a second only while a
 * plan is active, to notice a full-screen app closing. */
fn watch_island_pointer(app: AppHandle) {
    std::thread::spawn(move || {
        let mut inside = false;
        let mut catching = false;
        let mut busy = screen_busy();
        let mut checked = Instant::now();
        loop {
            if app.state::<AppState>().quitting.load(Ordering::SeqCst) {
                break;
            }
            if checked.elapsed() >= Duration::from_secs(1) {
                checked = Instant::now();
                let now_busy = screen_busy();
                if now_busy != busy {
                    busy = now_busy;
                    refresh_plans_soon(&app);
                }
            }
            if !island_is_shown(&app) {
                if inside {
                    inside = false;
                    let _ = app.emit_to("island", "island-pointer", json!({ "inside": false }));
                }
                catching = false;
                let active = plan_active(&app);
                let state = app.state::<AppState>();
                let (flag, wake) = &state.island_shown;
                if let Ok(shown) = flag.lock() {
                    if !*shown {
                        if active {
                            drop(wake.wait_timeout(shown, Duration::from_secs(1)));
                        } else {
                            drop(wake.wait(shown));
                        }
                    }
                }
                continue;
            }
            let Some(island) = app.get_webview_window("island") else {
                break;
            };
            let state = app.state::<AppState>();
            let geo = state.island_geo.lock().map(|g| *g).unwrap_or_default();
            let mut rect = state.island_rect.lock().map(|r| *r).unwrap_or_default();
            if rect.w <= 0.0 {
                // The page hasn't said where the pill is yet: assume it is tucked, centred.
                rect = IslandRect {
                    x: (ISLAND_SIZE.0 - 336.0) / 2.0,
                    y: 0.0,
                    w: 336.0,
                    h: 6.0,
                    solid: false,
                };
            }
            if let Some((px, py)) = pointer(&app) {
                let now_inside = over_pill(&geo, &rect, px, py);
                if now_inside != inside {
                    inside = now_inside;
                    let _ = app.emit_to("island", "island-pointer", json!({ "inside": inside }));
                }
                let catch = inside && rect.solid;
                if catch != catching {
                    catching = catch;
                    set_click_through(&island, !catch);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/* Links handed to the operating system: web pages, email and phone only.
 * Anything else (file:, search-ms:, ms-settings:, other apps' schemes) is
 * dropped, so a page can never launch another program through the shell. */
fn external_url(raw: &str) -> Option<Url> {
    if raw.len() > 2048 {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let ok = match url.scheme() {
        "https" | "http" => url.host_str().is_some(),
        "mailto" | "tel" => true,
        _ => false,
    };
    ok.then_some(url)
}

fn open_external(app: &AppHandle, raw: &str) {
    if let Some(url) = external_url(raw) {
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    }
}

/* Pages that may open inside the app: Hark, and the sign-in, payment and
 * challenge pages its flows pass through. Exact hosts only, over https: a
 * suffix such as ".google.com" would also admit pages anyone can publish
 * there, shown in a window with no address bar. Mirrors bridge.js. */
const IN_APP_HOSTS: [&str; 8] = [
    "hark.com",
    "www.hark.com",
    "accounts.google.com",
    "appleid.apple.com",
    "checkout.stripe.com",
    "billing.stripe.com",
    "hooks.stripe.com",
    "challenges.cloudflare.com",
];

fn in_app_page(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .map(|h| IN_APP_HOSTS.contains(&h) || h.ends_with(".hark.com"))
            .unwrap_or(false)
}

/* What happens to a navigation in Hark's window. Windows reports only the
 * page's own navigations here. macOS and Linux report every frame inside the
 * page too (an embedded video, say), with nothing to tell them apart, so
 * there a web address is let through: if it turns out to be the page itself
 * leaving Hark, on_page_load catches it when it starts loading. */
#[derive(Debug, PartialEq, Eq)]
enum Nav {
    Allow,
    OpenOutside,
    Block,
}

const FRAMES_REPORTED: bool = !cfg!(windows);

fn navigation(url: &Url, frames_reported: bool) -> Nav {
    if host_allowed(url) {
        Nav::Allow
    } else if !frames_reported {
        Nav::OpenOutside
    } else if matches!(url.scheme(), "http" | "https") {
        Nav::Allow
    } else {
        Nav::Block
    }
}

/* Top-level navigation in the main window: an in-app page, the browser's own
 * blank page, or an in-memory file Hark itself made (a blob: download). */
fn host_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => in_app_page(url),
        "about" => matches!(url.as_str(), "about:blank" | "about:srcdoc"),
        "blob" => url.as_str().starts_with("blob:https://hark.com/"),
        _ => false,
    }
}

/* ------------------------------- windows -------------------------------- */

/* The loading screen is the Hark window itself: it opens at the main window's
 * own size and place (restored by the window-state plugin), and Hark's sky
 * rises from dawn to day while hark.com loads (index.html, splash.ts). */
fn open_splash(app: &AppHandle) {
    if let Some(splash) = app.get_webview_window("splash") {
        let _ = splash.unminimize();
        let _ = splash.show();
        let _ = splash.set_focus();
        return;
    }
    // Built before the main window at start-up, so it pays WebView2's one-off
    // start-up cost and appears as early as possible. Same default size as main.
    let (width, height) = default_window_size(app);
    let Ok(splash) = frameless(WebviewWindowBuilder::new(
        app,
        "splash",
        WebviewUrl::App("index.html".into()),
    ))
    .title("Hark")
    .inner_size(width, height)
    .center()
    .visible(false)
    .initialization_script(appearance_script(app))
    .build() else {
        return;
    };
    polish_frame(&splash);
    hold_corners(&splash);
    let bounds = app
        .get_webview_window("main")
        .and_then(|m| {
            Some((
                m.outer_position().ok()?,
                m.outer_size().ok()?,
                m.is_maximized().unwrap_or(false),
            ))
        })
        .or_else(|| saved_main_bounds(app));
    if let Some((position, size, maximized)) = bounds {
        // Move first, so a maximized window maximizes on the right screen.
        let _ = splash.set_position(position);
        if maximized {
            let _ = splash.maximize();
        } else {
            let _ = splash.set_size(size);
        }
    }
    // Hark may have finished loading while this was being built.
    if app
        .state::<AppState>()
        .first_load_done
        .load(Ordering::SeqCst)
    {
        let _ = splash.close();
        show_main_now(app);
        return;
    }
    let _ = splash.show();
    let _ = splash.set_focus();
}

/* Hark's default window size (1440 x 920), made to fit the screen. */
fn default_window_size(app: &AppHandle) -> (f64, f64) {
    let Some(monitor) = app.primary_monitor().ok().flatten() else {
        return (1440.0, 920.0);
    };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let fit = |want: f64, room: u32| want.min(f64::from(room) / scale - 48.0).max(600.0);
    (fit(1440.0, area.size.width), fit(920.0, area.size.height))
}

/* Where the main window will open, read from the window-state plugin's file
 * before the main window exists. Ignored if it is no longer on any screen. */
/* A window rectangle (x, y, width, height) moved, and shrunk if it must be,
 * to sit wholly inside a screen's work area (also x, y, width, height). */
fn fit_rect(area: (i32, i32, u32, u32), window: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    let (ax, ay, aw, ah) = area;
    let (x, y, w, h) = window;
    let (w, h) = (w.min(aw), h.min(ah));
    let max_x = ax + i32::try_from(aw - w).unwrap_or(0);
    let max_y = ay + i32::try_from(ah - h).unwrap_or(0);
    (x.clamp(ax, max_x), y.clamp(ay, max_y), w, h)
}

fn work_area(monitor: &tauri::Monitor) -> (i32, i32, u32, u32) {
    let area = monitor.work_area();
    (
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
    )
}

/* A restored window can hang off the screen (moved there, or saved on a
 * bigger screen). Hark's window controls are at the top right on Windows, so
 * keep the whole window on the screen it is mostly on. */
fn keep_on_screen(window: &WebviewWindow) {
    if window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size), Ok(Some(monitor))) = (
        window.outer_position(),
        window.outer_size(),
        window.current_monitor(),
    ) else {
        return;
    };
    let now = (pos.x, pos.y, size.width, size.height);
    let fitted = fit_rect(work_area(&monitor), now);
    if fitted != now {
        let _ = window.set_size(tauri::PhysicalSize::new(fitted.2, fitted.3));
        let _ = window.set_position(PhysicalPosition::new(fitted.0, fitted.1));
    }
}

fn saved_main_bounds(
    app: &AppHandle,
) -> Option<(PhysicalPosition<i32>, tauri::PhysicalSize<u32>, bool)> {
    let file = app.path().app_config_dir().ok()?.join(".window-state.json");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let main = saved.get("main")?;
    let number = |key: &str| main.get(key).and_then(Value::as_i64);
    let (x, y) = (
        i32::try_from(number("x")?).ok()?,
        i32::try_from(number("y")?).ok()?,
    );
    let (width, height) = (
        u32::try_from(number("width")?)
            .ok()
            .filter(|w| (200..20_000).contains(w))?,
        u32::try_from(number("height")?)
            .ok()
            .filter(|h| (200..20_000).contains(h))?,
    );
    let mut centre = (
        f64::from(x) + f64::from(width) / 2.0,
        f64::from(y) + f64::from(height) / 2.0,
    );
    if cfg!(target_os = "macos") {
        // macOS looks monitors up in points.
        let scale = app.primary_monitor().ok().flatten()?.scale_factor();
        centre = (centre.0 / scale, centre.1 / scale);
    }
    let monitor = app.monitor_from_point(centre.0, centre.1).ok().flatten()?;
    let maximized = main.get("maximized").and_then(Value::as_bool) == Some(true);
    // The loading screen opens where Hark will, kept on screen the same way.
    let (x, y, width, height) = fit_rect(work_area(&monitor), (x, y, width, height));
    Some((
        PhysicalPosition::new(x, y),
        tauri::PhysicalSize::new(width, height),
        maximized,
    ))
}

/* Hark has painted: bring the daylight up, show Hark underneath the loading
 * screen and fade the loading screen away. Or stay in the tray. */
fn reveal_main(app: &AppHandle) {
    trace("reveal_main");
    let state = app.state::<AppState>();
    // Mark it loaded and take any waiting quick-ask text under one lock, so a
    // quick ask arriving at this moment is either taken here or sent directly.
    let pending = {
        let Ok(mut pending) = state.pending_compose.lock() else {
            return;
        };
        if state.first_load_done.swap(true, Ordering::SeqCst) {
            return;
        }
        pending.take()
    };
    let splash = app.get_webview_window("splash");
    if state.start_hidden.load(Ordering::SeqCst) {
        if let Some(splash) = splash {
            let _ = splash.close();
        }
        // Started in the tray (at sign-in, say): loaded and painted, now rest.
        sync_shell(app, false);
        return;
    }
    let Some(splash) = splash.filter(|s| s.is_visible().unwrap_or(false)) else {
        show_main_now(app);
        if let Some(text) = pending {
            deliver_compose(app, text, 400);
        }
        return;
    };
    // A minimized loading screen reports a far-off position: never copy that.
    let minimized = splash.is_minimized().unwrap_or(false);
    if !minimized {
        // The loading screen may have been moved or resized: Hark opens where it is.
        if let Some(main) = app.get_webview_window("main") {
            if splash.is_maximized().unwrap_or(false) {
                if let Ok(position) = splash.outer_position() {
                    let _ = main.set_position(position);
                }
                let _ = main.maximize();
            } else if cfg!(windows) {
                if let (Ok(position), Ok(size)) = (splash.outer_position(), splash.outer_size()) {
                    let _ = main.set_position(position);
                    let _ = main.set_size(size);
                }
            }
        }
    }
    let _ = splash.set_always_on_top(true);
    let _ = app.emit_to("splash", "hark-loaded", json!({}));
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(360)); // full daylight
        if minimized {
            if let Some(main) = handle.get_webview_window("main") {
                let _ = main.show();
                let _ = main.minimize();
            }
        } else {
            show_main_now(&handle);
        }
        std::thread::sleep(Duration::from_millis(380)); // the fade
        if let Some(splash) = handle.get_webview_window("splash") {
            let _ = splash.close();
        }
        if let Some(text) = pending {
            deliver_compose(&handle, text, 200);
        }
    });
}

fn build_main_window(app: &AppHandle) -> tauri::Result<()> {
    let url = Url::parse(HARK_URL).expect("static HARK_URL parses");
    let nav_app = app.clone();
    let new_win_app = app.clone();
    let download_app = app.clone();
    let bridge = format!("window.__HARK_PLATFORM__ = {PLATFORM:?};\n{BRIDGE_JS}");
    let builder = main_chrome(WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::External(url),
    ))
    .title("Hark")
    .inner_size(default_window_size(app).0, default_window_size(app).1)
    /* The signed-out screen is a locked, non-scrolling hero (bridge.js);
     * its sign-in form and legal text need ~660px, so don't let the window
     * shrink below that or the bottom of it becomes unreachable. */
    .min_inner_size(900.0, 560.0)
    .center()
    .visible(false)
    .focused(true)
    .initialization_script(bridge)
    .on_navigation(move |url: &Url| {
        let nav = navigation(url, FRAMES_REPORTED);
        trace(&format!("navigation {nav:?} {}://{}", url.scheme(), url.host_str().unwrap_or("")));
        match nav {
            Nav::Allow => true,
            Nav::OpenOutside => {
                open_external(&nav_app, url.as_str());
                false
            }
            Nav::Block => false,
        }
    })
    .on_new_window(move |url: Url, _features| {
        if in_app_page(&url) {
            // Auth redirects (Google / Apple) complete in-place.
            if let Some(main) = new_win_app.get_webview_window("main") {
                let _ = main.navigate(url);
            }
        } else {
            open_external(&new_win_app, url.as_str());
        }
        tauri::webview::NewWindowResponse::Deny
    })
    .on_page_load(|window, payload| {
        // The loading screen waits for bridge.js to say Hark has painted
        // ("hark-painted"); this notes whether the last load has ended, and
        // whether bridge.js has spoken from it (see "hark-ready").
        let app = window.app_handle().clone();
        let state = app.state::<AppState>();
        let finished = payload.event() == tauri::webview::PageLoadEvent::Finished;
        let url = payload.url().clone();
        trace(&format!(
            "page {} {}://{}{}",
            if finished { "finished" } else { "started" },
            url.scheme(),
            url.host_str().unwrap_or(""),
            url.path()
        ));
        // The page itself is leaving Hark (macOS and Linux let it through
        // navigation(), see there): open it in the browser and go back.
        if !host_allowed(&url) && matches!(url.scheme(), "http" | "https") {
            if !finished {
                open_external(&app, url.as_str());
                let back = state
                    .last_page
                    .lock()
                    .ok()
                    .and_then(|p| p.clone())
                    .or_else(|| Url::parse(HARK_URL).ok());
                if let Some(back) = back {
                    let _ = window.navigate(back);
                }
            }
            return;
        }
        if finished && host_allowed(&url) && url.scheme() == "https" {
            if let Ok(mut last) = state.last_page.lock() {
                *last = Some(url.clone());
            }
        }
        state.page_finished.store(finished, Ordering::SeqCst);
        if !finished {
            state.page_ok.store(false, Ordering::SeqCst);
            return;
        }
        // Landed somewhere bridge.js can't report from (a sign-in page, say):
        // show it rather than keep the loading screen up.
        let elsewhere = payload.url().host_str() != Some("hark.com");
        if elsewhere && payload.url().scheme() == "https" {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                reveal_main(&app);
            });
        }
    })
    .on_download(move |_webview, event| {
        match event {
            tauri::webview::DownloadEvent::Requested { url, destination } => {
                // The name the site suggests (Content-Disposition), else the URL's.
                let suggested = destination
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.trim().is_empty())
                    .or_else(|| {
                        url.path_segments()
                            .and_then(|mut s| s.next_back())
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                let name = safe_file_name(&suggested);
                // Anything that can run code goes through the browser, where
                // SmartScreen and the browser's own download checks apply.
                if runs_code(&name) {
                    open_external(&download_app, url.as_str());
                    notify(
                        &download_app,
                        "Hark",
                        &format!("{name} opened in your browser, which checks programs before saving them"),
                        false,
                    );
                    return false;
                }
                if let Ok(dir) = download_app.path().download_dir() {
                    *destination = unique_path(&dir, &name);
                }
            }
            tauri::webview::DownloadEvent::Finished { path, success, .. } => {
                let name = path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Download".to_string());
                let body = if success {
                    format!("{name} saved to Downloads")
                } else {
                    format!("{name} didn't finish downloading")
                };
                notify(&download_app, "Hark", &body, false);
            }
            _ => {}
        }
        true
    });
    #[cfg(windows)]
    let builder = builder.user_agent(USER_AGENT);
    let window = builder.build()?;
    polish_frame(&window);
    hold_corners(&window);
    keep_on_screen(&window);
    // Hark in front: hide the plan island, and a finished plan counts as seen.
    let focus_app = app.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Focused(focused) => {
            // WebView2 reports focus even to a hidden window (started in the
            // tray, say): only a window really on screen counts as seen.
            let on_screen = focus_app
                .get_webview_window("main")
                .map(|m| m.is_visible().unwrap_or(false) && !m.is_minimized().unwrap_or(false))
                .unwrap_or(false);
            sync_shell(&focus_app, false);
            if *focused && on_screen {
                if let Ok(mut plans) = focus_app.state::<AppState>().plans.lock() {
                    plans.finished = None;
                }
            }
            refresh_plans_soon(&focus_app);
        }
        // Minimized, or back from it (the taskbar button, Win+D...).
        WindowEvent::Resized(_) => sync_shell(&focus_app, false),
        _ => {}
    });
    start_refresh_worker(app);
    Ok(())
}

/* A file name Windows, macOS and Linux all accept. */
fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "hark-download".to_string()
    } else {
        trimmed.chars().take(180).collect()
    }
}

/* File types that run code when opened. */
fn runs_code(name: &str) -> bool {
    const TYPES: [&str; 44] = [
        "exe",
        "msi",
        "msix",
        "msixbundle",
        "appx",
        "appxbundle",
        "bat",
        "cmd",
        "com",
        "cpl",
        "hta",
        "jar",
        "js",
        "jse",
        "lnk",
        "msc",
        "ps1",
        "psm1",
        "reg",
        "scr",
        "url",
        "vbe",
        "vbs",
        "wsf",
        "wsh",
        "iso",
        "img",
        "vhd",
        "vhdx",
        "dll",
        "sys",
        "application",
        "appref-ms",
        "pif",
        "inf",
        "scf",
        "settingcontent-ms",
        "library-ms",
        "sh",
        "app",
        "dmg",
        "pkg",
        "command",
        "appimage",
    ];
    name.rsplit_once('.')
        .map(|(_, ext)| TYPES.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/* Never overwrite a file already in Downloads: "name (1).ext", "name (2).ext"... */
fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    (1..1000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/* --------------------------------- tray --------------------------------- */

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Hark", true, None::<&str>)?;
    let quick = MenuItem::with_id(app, "quick", "Ask Hark…", true, Some(quick_label()))?;
    let plan = MenuItem::with_id(app, "plan", "Plan", true, None::<&str>)?;
    let plan_gap = PredefinedMenuItem::separator(app)?;
    let brief = MenuItem::with_id(app, "brief", "Today's brief", true, None::<&str>)?;
    let projects = MenuItem::with_id(app, "projects", "Projects", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some(settings_label()))?;
    let browser = MenuItem::with_id(
        app,
        "browser",
        "Open hark.com in your browser",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Hark", true, None::<&str>)?;
    /* The running plan is inserted after the first separator (see update_tray). */
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &quick,
            &PredefinedMenuItem::separator(app)?,
            &brief,
            &projects,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &browser,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Hark")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quick" => toggle_quick(app),
            "plan" => {
                let title = app
                    .state::<AppState>()
                    .plans
                    .lock()
                    .map(|p| p.view(true).title)
                    .unwrap_or_default();
                open_plan(app, title);
            }
            "brief" => {
                show_main(app);
                let _ = app.emit_to("main", "hark-navigate", json!({ "target": "home" }));
            }
            "projects" => {
                show_main(app);
                let _ = app.emit_to("main", "hark-navigate", json!({ "target": "projects" }));
            }
            "settings" => open_settings(app),
            "browser" => {
                let _ = app.opener().open_url(HARK_URL, None::<&str>);
            }
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = tray_icon(&PlanView::idle(), false) {
        builder = builder.icon(icon);
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }
    builder.build(app)?;
    if let Ok(mut slot) = app.state::<AppState>().tray_menu.lock() {
        *slot = Some(TrayMenu {
            menu,
            open,
            plan,
            plan_gap,
            plan_shown: false,
        });
    }
    Ok(())
}

/* The macOS menu bar wants a one-colour "template" glyph (black + alpha) that
 * the system tints for light and dark menu bars, so unread is a dot in the
 * glyph, not a colour, and plan progress lives on the island and the Dock. */
#[cfg(target_os = "macos")]
fn tray_icon(_plan: &PlanView, unread: bool) -> Option<Image<'static>> {
    let bytes: &[u8] = if unread {
        include_bytes!("../icons/tray-template-dot.png")
    } else {
        include_bytes!("../icons/tray-template.png")
    };
    Image::from_bytes(bytes).ok()
}

/* Windows and Linux show the coloured app tile. While a Handoff plan runs,
 * Hark's sky rises inside it as the steps finish; a finished plan fills it
 * with warm light; unread adds the orange dot. Stacked from the layers in
 * icons/tray-layers (rendered by branding/build.mjs). */
#[cfg(not(target_os = "macos"))]
fn tray_icon(plan: &PlanView, unread: bool) -> Option<Image<'static>> {
    struct Layers {
        base: Vec<u8>,
        sky: Vec<u8>,
        done: Vec<u8>,
        glyph: Vec<u8>,
        dot: Vec<u8>,
        width: u32,
        height: u32,
    }
    static LAYERS: std::sync::OnceLock<Option<Layers>> = std::sync::OnceLock::new();
    let layers = LAYERS
        .get_or_init(|| {
            let load = |bytes: &[u8]| {
                Image::from_bytes(bytes)
                    .ok()
                    .map(|i| (i.rgba().to_vec(), i.width(), i.height()))
            };
            let (base, width, height) = load(include_bytes!("../icons/tray-layers/base.png"))?;
            let layer = |bytes: &[u8]| {
                load(bytes)
                    .filter(|(_, w, h)| *w == width && *h == height)
                    .map(|(px, _, _)| px)
            };
            Some(Layers {
                base,
                sky: layer(include_bytes!("../icons/tray-layers/sky.png"))?,
                done: layer(include_bytes!("../icons/tray-layers/done.png"))?,
                glyph: layer(include_bytes!("../icons/tray-layers/glyph.png"))?,
                dot: layer(include_bytes!("../icons/tray-layers/dot.png"))?,
                width,
                height,
            })
        })
        .as_ref()?;
    let (w, h) = (layers.width as usize, layers.height as usize);
    let filled = match plan.phase {
        "done" => h,
        // A running plan always shows a little sky, even before its first step.
        "running" if plan.total > 0 => {
            let rows = (plan.done as f64 / plan.total as f64 * h as f64).round() as usize;
            rows.max(h / 8).min(h)
        }
        _ => 0,
    };
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let layer = if plan.phase == "done" {
            &layers.done
        } else if y >= h - filled {
            &layers.sky
        } else {
            &layers.base
        };
        pixels.extend_from_slice(&layer[y * w * 4..(y + 1) * w * 4]);
    }
    blend_over(&mut pixels, &layers.glyph);
    if unread {
        blend_over(&mut pixels, &layers.dot);
    }
    Some(Image::new_owned(pixels, layers.width, layers.height))
}

/* Straight-alpha "source over" of one RGBA layer onto another of the same size. */
#[cfg(not(target_os = "macos"))]
fn blend_over(dst: &mut [u8], src: &[u8]) {
    let (dst, _) = dst.as_chunks_mut::<4>();
    let (src, _) = src.as_chunks::<4>();
    for (d, s) in dst.iter_mut().zip(src) {
        let sa = f32::from(s[3]) / 255.0;
        if sa <= 0.0 {
            continue;
        }
        let da = f32::from(d[3]) / 255.0;
        let oa = sa + da * (1.0 - sa);
        for c in 0..3 {
            let v = (f32::from(s[c]) * sa + f32::from(d[c]) * da * (1.0 - sa)) / oa;
            d[c] = v.round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (oa * 255.0).round() as u8;
    }
}

/* Text from the page shown natively (island, tray, tooltip, notifications):
 * no control or text-direction characters, and a sensible length. */
fn clean_text(text: &str, max: usize) -> String {
    let kept: String = text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let trimmed = kept.trim();
    if trimmed.chars().count() > max {
        format!("{}…", trimmed.chars().take(max - 1).collect::<String>())
    } else {
        trimmed.to_string()
    }
}

/* A plan as reported by the page, within limits the UI can draw. */
fn clean_plan(title: &str, done: u64, total: u64) -> Option<PlanInfo> {
    let title = clean_text(title, 120);
    if title.is_empty() || total == 0 {
        return None;
    }
    let total = total.min(64);
    Some(PlanInfo {
        title,
        done: done.min(total),
        total,
    })
}

fn short_title(title: &str) -> String {
    if title.chars().count() > 42 {
        format!("{}…", title.chars().take(41).collect::<String>())
    } else {
        title.to_string()
    }
}

/* Tray icon, tooltip and menu, the unread badge, and the running plan.
 * Called by the refresh worker only; unchanged updates are skipped. */
fn update_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let unread = state.unread.load(Ordering::SeqCst);
    let plan = if settings_snapshot(app).taskbar_progress {
        state
            .plans
            .lock()
            .map(|p| p.view(true))
            .unwrap_or_else(|_| PlanView::idle())
    } else {
        PlanView::idle()
    };
    let key = format!(
        "{}|{}|{}|{}|{unread}",
        plan.phase, plan.title, plan.done, plan.total
    );
    if let Ok(mut last) = state.tray_key.lock() {
        if *last == key {
            return;
        }
        *last = key;
    }
    let title = short_title(&plan.title);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tooltip = match plan.phase {
            "running" => format!(
                "Hark · {title}, step {} of {}",
                plan.done.saturating_add(1).min(plan.total),
                plan.total
            ),
            "done" => format!("Hark · {title} is done"),
            _ if unread > 0 => format!("Hark · {unread} unread"),
            _ => "Hark".to_string(),
        };
        let _ = tray.set_tooltip(Some(tooltip));
        if let Some(icon) = tray_icon(&plan, unread > 0) {
            // macOS: keep it a template glyph, which the menu bar tints.
            #[cfg(target_os = "macos")]
            let _ = tray.set_icon_with_as_template(Some(icon), true);
            #[cfg(not(target_os = "macos"))]
            let _ = tray.set_icon(Some(icon));
        }
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_badge_count(if unread > 0 { Some(unread) } else { None });
    }
    // Take what is needed out of the lock: each menu change waits on the main thread.
    let (menu, open, plan_item, plan_gap, shown) = {
        let Ok(slot) = state.tray_menu.lock() else {
            return;
        };
        let Some(t) = slot.as_ref() else {
            return;
        };
        (
            t.menu.clone(),
            t.open.clone(),
            t.plan.clone(),
            t.plan_gap.clone(),
            t.plan_shown,
        )
    };
    let _ = open.set_text(if unread > 0 {
        format!("Open Hark ({unread} unread)")
    } else {
        "Open Hark".to_string()
    });
    let show_plan = plan.phase != "idle";
    if show_plan {
        let _ = plan_item.set_text(match plan.phase {
            "done" => format!("{title} · done"),
            _ => format!("{title} · {} of {}", plan.done, plan.total),
        });
    }
    if show_plan != shown {
        let changed = if show_plan {
            menu.insert(&plan_item, 3)
                .and_then(|_| menu.insert(&plan_gap, 4))
        } else {
            menu.remove(&plan_item).and_then(|_| menu.remove(&plan_gap))
        };
        if changed.is_ok() {
            if let Ok(mut slot) = state.tray_menu.lock() {
                if let Some(t) = slot.as_mut() {
                    t.plan_shown = show_plan;
                }
            }
        }
    }
}

/* --------------------------- bridge event wiring --------------------------- */

fn parse_payload(event: &tauri::Event) -> Value {
    serde_json::from_str(event.payload()).unwrap_or(Value::Null)
}

fn install_bridge_listeners(app: &AppHandle) {
    let h = app.clone();
    app.listen("hark-notify", move |event| {
        let v = parse_payload(&event);
        let title = clean_text(v.get("title").and_then(Value::as_str).unwrap_or("Hark"), 64);
        let title = if title.is_empty() {
            "Hark".to_string()
        } else {
            title
        };
        let body = clean_text(v.get("body").and_then(Value::as_str).unwrap_or(""), 240);
        let body = body.as_str();
        let title = title.as_str();
        // A page can't flood the desktop: at most 6 notifications a minute.
        {
            let state = h.state::<AppState>();
            let Ok(mut recent) = state.notified.lock() else {
                return;
            };
            let now = std::time::Instant::now();
            recent.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            if recent.len() >= 6 {
                return;
            }
            recent.push(now);
        }
        // Skip toasts while the user is already looking at Hark.
        let looking = h
            .get_webview_window("main")
            .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
            .unwrap_or(false);
        if !looking && !body.is_empty() {
            notify(&h, title, body, false);
        }
    });

    let h = app.clone();
    app.listen("hark-unread", move |event| {
        let v = parse_payload(&event);
        let count = v.get("count").and_then(Value::as_i64).unwrap_or(0);
        h.state::<AppState>().unread.store(count, Ordering::SeqCst);
        refresh_plans_soon(&h);
    });

    // The loading screen hands over once Hark has painted. If the paint signal
    // never comes but the page is up, show it anyway after a few seconds.
    let h = app.clone();
    app.listen("hark-painted", move |_event| {
        trace("hark-painted");
        reveal_main(&h);
    });
    let h = app.clone();
    app.listen("hark-ready", move |_event| {
        trace("hark-ready");
        h.state::<AppState>().page_ok.store(true, Ordering::SeqCst);
        // A page that has just loaded knows nothing yet: tell it, once it listens.
        let told = h.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            sync_shell(&told, true);
        });
        let h = h.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(8));
            reveal_main(&h);
        });
    });

    let h = app.clone();
    app.listen("hark-appearance", move |event| {
        let v = parse_payload(&event);
        let theme = v
            .get("theme")
            .and_then(Value::as_str)
            .and_then(valid_theme)
            .unwrap_or_default();
        let sky = v
            .get("sky")
            .and_then(Value::as_str)
            .filter(|s| valid_sky(s))
            .map(str::to_string);
        let state = h.state::<AppState>();
        let next = {
            let Ok(mut current) = state.appearance.lock() else {
                return;
            };
            if current.theme == theme && (sky.is_none() || current.sky == sky) {
                return;
            }
            current.theme = theme;
            if sky.is_some() {
                current.sky = sky;
            }
            current.clone()
        };
        if let Ok(store) = h.store(STORE_FILE) {
            store.set("theme", json!(next.theme));
            if let Some(sky) = &next.sky {
                store.set("sky", json!(sky));
            }
            let _ = store.save();
        }
        for label in ["splash", "quick", "island", "settings"] {
            let _ = h.emit_to(label, "hark-appearance", &next);
        }
    });

    let h = app.clone();
    app.listen("hark-plans", move |event| {
        let v = parse_payload(&event);
        let active: Vec<PlanInfo> = v
            .get("plans")
            .cloned()
            .and_then(|p| serde_json::from_value::<Vec<PlanInfo>>(p).ok())
            .unwrap_or_default()
            .into_iter()
            .take(8)
            .filter_map(|p| clean_plan(&p.title, p.done, p.total))
            .collect();
        if let Ok(mut plans) = h.state::<AppState>().plans.lock() {
            // "Hide until it's done" ends if that plan stops running (cancelled).
            let hidden_still_running = plans
                .hidden
                .as_ref()
                .map(|t| active.iter().any(|p| &p.title == t))
                .unwrap_or(true);
            if !hidden_still_running
                && plans
                    .finished
                    .as_ref()
                    .map(|f| Some(&f.title) == plans.hidden.as_ref())
                    != Some(true)
            {
                plans.hidden = None;
            }
            plans.active = active;
        }
        refresh_plans_soon(&h);
    });

    let h = app.clone();
    app.listen("hark-plan-done", move |event| {
        let v = parse_payload(&event);
        let title = clean_text(v.get("title").and_then(Value::as_str).unwrap_or(""), 120);
        let total = v.get("total").and_then(Value::as_u64).unwrap_or(0).min(64);
        let looking = main_in_front(&h);
        if let Ok(mut plans) = h.state::<AppState>().plans.lock() {
            if plans.hidden.as_deref() == Some(title.as_str()) {
                plans.hidden = None;
            }
            plans.finished = if looking {
                None
            } else {
                Some(PlanInfo {
                    title: title.clone(),
                    done: total,
                    total,
                })
            };
        }
        // Where the island can't show it, a notification says which plan finished.
        let island = !cfg!(target_os = "linux")
            && settings_snapshot(&h).taskbar_progress
            && h.state::<AppState>().island_ok.load(Ordering::SeqCst);
        if !looking && !island && !title.is_empty() {
            notify(&h, "Plan finished", &title, false);
        }
        refresh_plans_soon(&h);
    });

    let h = app.clone();
    app.listen("hark-progress", move |event| {
        let v = parse_payload(&event);
        let value = v.get("value").and_then(Value::as_u64).map(|p| p.min(100));
        // Kept even while Plan progress is off, to show at once when it is turned on.
        if let Ok(mut last) = h.state::<AppState>().last_progress.lock() {
            *last = value;
        }
        if settings_snapshot(&h).taskbar_progress {
            set_taskbar_progress(&h, value);
        }
    });

    // Hark couldn't take quick-ask text (no message box): give it back.
    let h = app.clone();
    app.listen("hark-compose-result", move |event| {
        let v = parse_payload(&event);
        if v.get("ok").and_then(Value::as_bool) != Some(false) {
            return;
        }
        let text: String = v
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(20_000)
            .collect();
        if text.trim().is_empty() {
            return;
        }
        let h = h.clone();
        std::thread::spawn(move || {
            if h.get_webview_window("quick").is_none() {
                toggle_quick(&h);
                std::thread::sleep(Duration::from_millis(700));
            } else if let Some(quick) = h.get_webview_window("quick") {
                place_quick(&h, &quick);
                let _ = quick.show();
                let _ = quick.set_focus();
            }
            let _ = h.emit_to("quick", "quick-restore", json!({ "text": text }));
        });
    });

    let h = app.clone();
    app.listen("hark-external", move |event| {
        let v = parse_payload(&event);
        if let Some(url) = v.get("url").and_then(Value::as_str) {
            open_external(&h, url);
        }
    });

    let h = app.clone();
    app.listen("hark-settings", move |_event| {
        open_settings(&h);
    });

    let h = app.clone();
    app.listen("hark-window", move |event| {
        let v = parse_payload(&event);
        let action = v.get("action").and_then(Value::as_str).unwrap_or("");
        let Some(main) = h.get_webview_window("main") else {
            return;
        };
        match action {
            "minimize" => {
                let _ = main.minimize();
            }
            "maximize" => {
                if main.is_maximized().unwrap_or(false) {
                    let _ = main.unmaximize();
                } else {
                    let _ = main.maximize();
                }
            }
            "close" => {
                if settings_snapshot(&h).close_to_tray {
                    hide_to_tray(&h, &main);
                } else {
                    quit(&h);
                }
            }
            "drag" => {
                let _ = main.start_dragging();
            }
            "fullscreen" => {
                let fs = main.is_fullscreen().unwrap_or(false);
                let _ = main.set_fullscreen(!fs);
            }
            _ => {}
        }
    });
}

/* ---------------------------------- run ---------------------------------- */

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let start_hidden = std::env::args().any(|a| a == "--minimized");

    tauri::Builder::default()
        // Single instance first: relaunching focuses the resident window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        /* Remember where windows were, but not whether they were showing (Hark
         * decides that), and not for the windows that place themselves. */
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    StateFlags::all()
                        & !(StateFlags::VISIBLE | StateFlags::FULLSCREEN | StateFlags::DECORATIONS),
                )
                .with_denylist(&["splash", "quick", "island"])
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if shortcut.matches(quick_modifiers(), Code::KeyH) {
                        let visible_focused = app
                            .get_webview_window("main")
                            .map(|w| {
                                w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false)
                            })
                            .unwrap_or(false);
                        if visible_focused {
                            focus_composer(app);
                        } else {
                            toggle_quick(app);
                        }
                    }
                })
                .build(),
        )
        .manage(AppState {
            settings: Mutex::new(Settings::default()),
            quitting: AtomicBool::new(false),
            first_load_done: AtomicBool::new(false),
            start_hidden: AtomicBool::new(start_hidden),
            page_finished: AtomicBool::new(false),
            page_ok: AtomicBool::new(false),
            pending_compose: Mutex::new(None),
            unread: AtomicI64::new(0),
            plans: Mutex::new(Plans::default()),
            island_rect: Mutex::new(IslandRect::default()),
            tray_menu: Mutex::new(None),
            appearance: Mutex::new(Appearance::default()),
            notified: Mutex::new(Vec::new()),
            refresh: (Mutex::new(false), Condvar::new()),
            island_shown: (Mutex::new(false), Condvar::new()),
            island_geo: Mutex::new(IslandGeo::default()),
            island_ok: AtomicBool::new(true),
            tray_key: Mutex::new(String::new()),
            last_progress: Mutex::new(None),
            last_page: Mutex::new(None),
            shell_key: AtomicI64::new(-1),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_setting,
            test_notification,
            reload_main,
            app_loaded,
            quick_compose,
            splash_retry,
            splash_hide,
            island_state,
            island_rect,
            island_open,
            island_hide_plan,
            island_dismiss
        ])
        .setup(move |app| {
            let handle = app.handle();
            // Replace the placeholder settings with the persisted ones, then
            // apply side effects (autostart registration) before first paint.
            let settings = load_settings(handle);
            {
                let state = handle.state::<AppState>();
                if let Ok(mut slot) = state.settings.lock() {
                    *slot = settings.clone();
                };
            }
            persist_settings(handle, &settings);
            // Only touch the sign-in entry when it disagrees with the setting, so a
            // second copy of the app (a test build, a portable exe) never repoints it.
            let registered = handle.autolaunch().is_enabled().unwrap_or(false);
            if registered != settings.autostart {
                apply_autostart(handle, settings.autostart);
            }
            let appearance = load_appearance(handle);
            if let Ok(mut slot) = handle.state::<AppState>().appearance.lock() {
                *slot = appearance;
            }

            build_tray(handle)?;
            if !start_hidden {
                open_splash(handle);
            }
            build_main_window(handle)?;
            install_bridge_listeners(handle);

            let _ = handle.global_shortcut().register(QUICK_SHORTCUT);
            #[cfg(target_os = "macos")]
            install_macos_menu(handle)?;

            // Started at sign-in and the first load failed (offline, say):
            // try again quietly every 30 seconds instead of popping anything up.
            let retry = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                let state = retry.state::<AppState>();
                if state.first_load_done.load(Ordering::SeqCst)
                    || state.quitting.load(Ordering::SeqCst)
                {
                    break;
                }
                let splash_up = retry
                    .get_webview_window("splash")
                    .map(|s| s.is_visible().unwrap_or(false))
                    .unwrap_or(false);
                if !splash_up
                    && state.page_finished.load(Ordering::SeqCst)
                    && !state.page_ok.load(Ordering::SeqCst)
                {
                    retry_load(&retry);
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Hark Desktop")
        .run(|app, event| match event {
            tauri::RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                let state = app.state::<AppState>();
                if state.quitting.load(Ordering::SeqCst) {
                    return;
                }
                if settings_snapshot(app).close_to_tray {
                    api.prevent_close();
                    if let Some(main) = app.get_webview_window("main") {
                        hide_to_tray(app, &main);
                    }
                } else {
                    quit(app);
                }
            }
            // The loading screen closing, by its own button or by the system
            // (Alt+F4, the window menu): Hark keeps loading in the tray.
            tauri::RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { .. },
                ..
            } if label == "splash" => {
                let state = app.state::<AppState>();
                if !state.first_load_done.load(Ordering::SeqCst)
                    && !state.quitting.load(Ordering::SeqCst)
                    && !state.start_hidden.load(Ordering::SeqCst)
                {
                    if settings_snapshot(app).close_to_tray {
                        state.start_hidden.store(true, Ordering::SeqCst);
                    } else {
                        // No tray to keep loading in (the Linux default): closing quits.
                        quit(app);
                    }
                }
            }
            tauri::RunEvent::ExitRequested { api, .. } => {
                let state = app.state::<AppState>();
                if !state.quitting.load(Ordering::SeqCst) && settings_snapshot(app).close_to_tray {
                    api.prevent_exit();
                }
            }
            /* Clicking the Dock icon: bring Hark back if it is hidden or minimized.
             * (The island and Settings count as visible windows, so the flag
             * AppKit passes can't be relied on.) */
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                let hidden = app
                    .get_webview_window("main")
                    .map(|m| !m.is_visible().unwrap_or(false) || m.is_minimized().unwrap_or(false))
                    .unwrap_or(true);
                if hidden {
                    show_main(app);
                }
            }
            _ => {}
        });
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    fn pixel(icon: &Image<'_>, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * icon.width() + x) * 4) as usize;
        let p = icon.rgba();
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    fn running(done: u64, total: u64) -> PlanView {
        PlanView {
            phase: "running",
            title: "Test plan".into(),
            done,
            total,
            others: 0,
        }
    }

    /* Points inside the tile but clear of the h: near the left edge, high and low. */
    const HIGH: (u32, u32) = (4, 24);
    const LOW: (u32, u32) = (4, 52);

    #[test]
    fn tray_icon_fills_with_sky_from_the_bottom() {
        let idle = tray_icon(&PlanView::idle(), false).expect("idle icon");
        let half = tray_icon(&running(4, 8), false).expect("running icon");
        let dark = |p: [u8; 4]| p[3] > 200 && p[2] < 110;
        let sky = |p: [u8; 4]| p[3] > 200 && p[2] > 180 && p[2] > p[0];
        assert!(
            dark(pixel(&idle, LOW.0, LOW.1)),
            "idle tile is dark at the bottom"
        );
        assert!(
            dark(pixel(&idle, HIGH.0, HIGH.1)),
            "idle tile is dark at the top"
        );
        assert!(
            sky(pixel(&half, LOW.0, LOW.1)),
            "half-way plan has sky at the bottom"
        );
        assert!(
            dark(pixel(&half, HIGH.0, HIGH.1)),
            "half-way plan is still dark at the top"
        );
    }

    #[test]
    fn running_plan_shows_some_sky_before_its_first_step() {
        let start = tray_icon(&running(0, 8), false).expect("icon");
        let bottom = pixel(&start, 32, 61);
        assert!(bottom[2] > 180, "a little sky along the bottom edge");
    }

    #[test]
    fn finished_plan_is_light_all_over_and_unread_adds_the_dot() {
        let done = PlanView {
            phase: "done",
            ..running(8, 8)
        };
        let icon = tray_icon(&done, true).expect("icon");
        for (x, y) in [HIGH, LOW] {
            let p = pixel(&icon, x, y);
            assert!(p[0] > 150 && p[1] > 150 && p[2] > 150, "light at {x},{y}");
        }
        let dot = pixel(&icon, 52, 12);
        assert!(
            dot[0] > 200 && dot[1] < 140 && dot[2] < 80,
            "orange unread dot"
        );
    }

    #[test]
    fn only_web_mail_and_phone_links_reach_the_system() {
        for ok in [
            "https://example.com/a",
            "http://example.com",
            "mailto:a@b.c",
            "tel:+6421000000",
        ] {
            assert!(external_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            r"search-ms:query=x&crumb=location:\\evil\share",
            "ms-settings:privacy",
            "ms-msdt:/id",
            "javascript:alert(1)",
            "data:text/html,hi",
            "https://",
            "not a url",
        ] {
            assert!(external_url(bad).is_none(), "{bad}");
        }
        assert!(external_url(&format!("https://example.com/{}", "a".repeat(3000))).is_none());
    }

    #[test]
    fn only_hark_and_its_sign_in_pages_open_inside_the_app() {
        let url = |s: &str| Url::parse(s).expect("url");
        for ok in [
            "https://hark.com/chat",
            "https://app.hark.com/",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://appleid.apple.com/auth/authorize",
            "https://checkout.stripe.com/c/pay",
            "about:blank",
            "blob:https://hark.com/1234",
        ] {
            assert!(host_allowed(&url(ok)), "{ok}");
        }
        for bad in [
            "http://hark.com/",
            "https://sites.google.com/view/anything",
            "https://evilhark.com/",
            "https://hark.com.evil.example/",
            "file:///C:/",
            "data:text/html,hi",
            "blob:https://evil.example/1",
            "about:config",
        ] {
            assert!(!host_allowed(&url(bad)), "{bad}");
        }
        assert!(
            !in_app_page(&url("about:blank")),
            "a popup never blanks Hark's window"
        );
    }

    #[test]
    fn downloads_get_safe_unique_names_and_programs_go_to_the_browser() {
        assert_eq!(safe_file_name("C:evil.dll"), "C_evil.dll");
        assert_eq!(safe_file_name(r"..\..\x.txt"), "_.._x.txt");
        assert_eq!(safe_file_name("  report.pdf. "), "report.pdf");
        assert_eq!(safe_file_name(""), "hark-download");
        assert!(runs_code("setup.EXE") && runs_code("a.ps1") && runs_code("x.lnk"));
        assert!(!runs_code("report.pdf") && !runs_code("photo.jpg") && !runs_code("noext"));
        let dir = std::env::temp_dir().join(format!("hark-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("notes.txt"), b"x").expect("file");
        std::fs::write(dir.join("notes (1).txt"), b"x").expect("file");
        assert_eq!(unique_path(&dir, "notes.txt"), dir.join("notes (2).txt"));
        assert_eq!(unique_path(&dir, "new.txt"), dir.join("new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_text_is_cleaned_and_plans_are_kept_drawable() {
        assert_eq!(clean_text("  Plan\u{202e}gnp.exe\n ", 120), "Plangnp.exe");
        assert_eq!(clean_text(&"x".repeat(200), 10).chars().count(), 10);
        let plan = clean_plan("Big", 900, 100_000).expect("plan");
        assert_eq!((plan.done, plan.total), (64, 64));
        assert!(clean_plan("", 1, 2).is_none() && clean_plan("x", 0, 0).is_none());
    }

    #[test]
    fn pointing_anywhere_from_the_screen_edge_to_the_pill_counts() {
        let geo = IslandGeo {
            x: 700.0,
            y: 0.0,
            rect_scale: 1.0,
            cursor_scale: 1.0,
        };
        // Open pill: 430 wide, hanging 10 px below the screen edge.
        let open = IslandRect {
            x: 45.0,
            y: 10.0,
            w: 430.0,
            h: 120.0,
            solid: true,
        };
        assert!(
            over_pill(&geo, &open, 960.0, 2.0),
            "the edge above the pill"
        );
        assert!(over_pill(&geo, &open, 960.0, 60.0), "the pill itself");
        assert!(!over_pill(&geo, &open, 960.0, 140.0), "below the pill");
        assert!(!over_pill(&geo, &open, 700.0, 30.0), "beside the pill");
        // Tucked pill: a few pixels at the very top.
        let tucked = IslandRect {
            x: 92.0,
            y: 0.0,
            w: 336.0,
            h: 5.0,
            solid: false,
        };
        assert!(over_pill(&geo, &tucked, 960.0, 0.0));
        assert!(over_pill(&geo, &tucked, 960.0, 7.0));
        assert!(!over_pill(&geo, &tucked, 960.0, 20.0));
        // At 150%, page pixels become 1.5 screen pixels.
        let scaled = IslandGeo {
            rect_scale: 1.5,
            ..geo
        };
        assert!(over_pill(&scaled, &open, 700.0 + 45.0 * 1.5 + 5.0, 150.0));
    }

    #[test]
    fn embedded_frames_never_open_the_browser() {
        let url = |s: &str| Url::parse(s).expect("url");
        let video = url("https://www.youtube.com/embed/abc");
        // Windows sees only the page: leaving Hark opens the browser.
        assert_eq!(navigation(&video, false), Nav::OpenOutside);
        // macOS and Linux see frames too: let it load (a real page change is caught on load).
        assert_eq!(navigation(&video, true), Nav::Allow);
        assert_eq!(navigation(&url("https://hark.com/chat"), true), Nav::Allow);
        assert_eq!(navigation(&url("https://hark.com/chat"), false), Nav::Allow);
        assert_eq!(navigation(&url("file:///C:/x"), true), Nav::Block);
        assert_eq!(navigation(&url("ms-settings:privacy"), true), Nav::Block);
    }

    #[test]
    fn windows_are_kept_inside_the_screen() {
        let area = (0, 0, 1920, 1032);
        // Hanging off the right and bottom: moved back in, size kept.
        assert_eq!(fit_rect(area, (594, 269, 1507, 949)), (413, 83, 1507, 949));
        // Already inside: unchanged.
        assert_eq!(fit_rect(area, (100, 100, 800, 600)), (100, 100, 800, 600));
        // Bigger than the screen: shrunk to it.
        assert_eq!(fit_rect(area, (-50, -50, 2560, 1440)), (0, 0, 1920, 1032));
        // A second screen to the left.
        assert_eq!(
            fit_rect((-1920, 0, 1920, 1080), (-100, 10, 800, 600)),
            (-800, 10, 800, 600)
        );
    }

    #[test]
    fn quick_ask_sends_unless_turned_off() {
        assert!(Settings::default().quick_sends);
    }

    #[test]
    fn only_inline_sky_images_are_accepted() {
        assert!(valid_sky("data:image/webp;base64,UklGRpQC+/="));
        assert!(!valid_sky("data:image/svg+xml;base64,PHN2Zz4="));
        assert!(!valid_sky("data:image/webp;base64,abc\"); x: y"));
        assert!(!valid_sky("https://example.com/sky.webp"));
        assert!(!valid_sky(&format!(
            "data:image/webp;base64,{}",
            "A".repeat(70_000)
        )));
    }

    #[test]
    fn island_follows_the_plan_furthest_along_and_skips_a_hidden_one() {
        let plan = |title: &str, done, total| PlanInfo {
            title: title.into(),
            done,
            total,
        };
        let mut plans = Plans {
            active: vec![plan("Slow", 1, 8), plan("Quick", 3, 4)],
            ..Plans::default()
        };
        let view = plans.view(false);
        assert_eq!(
            (view.phase, view.title.as_str(), view.others),
            ("running", "Quick", 1)
        );
        plans.hidden = Some("Quick".into());
        assert_eq!(plans.view(false).title, "Slow");
        assert_eq!(plans.view(true).title, "Quick");
        plans.active.clear();
        assert_eq!(plans.view(false).phase, "idle");
        plans.finished = Some(plan("Quick", 4, 4));
        assert_eq!(plans.view(false).phase, "done");
    }
}
