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
use std::sync::Mutex;
use std::time::Duration;

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
    /* Quick-ask text that arrived before Hark had loaded. */
    pending_compose: Mutex<Option<String>>,
    unread: AtomicI64,
    plans: Mutex<Plans>,
    island_rect: Mutex<IslandRect>,
    tray_menu: Mutex<Option<TrayMenu>>,
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
    }
    settings
}

fn persist_settings(app: &AppHandle, settings: &Settings) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set("autostart", json!(settings.autostart));
        store.set("close_to_tray", json!(settings.close_to_tray));
        store.set("notifications", json!(settings.notifications));
        store.set("taskbar_progress", json!(settings.taskbar_progress));
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
            other => return Err(format!("unknown setting: {other}")),
        }
        settings.clone()
    };
    if key == "autostart" {
        apply_autostart(&app, value);
    }
    if key == "taskbar_progress" {
        if !value {
            set_taskbar_progress(&app, None);
        }
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

#[tauri::command]
fn reload_main(app: AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
        if let Ok(url) = Url::parse(HARK_URL) {
            let _ = main.navigate(url);
        }
    }
}

#[tauri::command]
fn quick_compose(app: AppHandle, text: String) {
    if let Some(quick) = app.get_webview_window("quick") {
        let _ = quick.hide();
    }
    let state = app.state::<AppState>();
    if !state.first_load_done.load(Ordering::SeqCst) {
        // Hark is still loading: the text is handed over once it has painted.
        if !text.is_empty() {
            if let Ok(mut slot) = state.pending_compose.lock() {
                *slot = Some(text);
            }
        }
        show_main(&app);
        return;
    }
    show_main(&app);
    // Give the window a beat to foreground before the page receives the text.
    deliver_compose(&app, text, 180);
}

fn deliver_compose(app: &AppHandle, text: String, delay_ms: u64) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        let _ = handle.emit_to("main", "hark-compose", json!({ "text": text }));
    });
}

/* Loading screen "Try again": load hark.com again in the hidden main window. */
#[tauri::command]
fn splash_retry(app: AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        if let Ok(url) = Url::parse(HARK_URL) {
            let _ = main.navigate(url);
        }
    }
}

/* Loading screen closed, or "Hide to tray": Hark keeps loading in the tray. */
#[tauri::command]
fn splash_hide(app: AppHandle) {
    app.state::<AppState>()
        .start_hidden
        .store(true, Ordering::SeqCst);
    if let Some(splash) = app.get_webview_window("splash") {
        let _ = splash.close();
    }
}

#[tauri::command]
fn island_state(app: AppHandle) -> PlanView {
    island_view(&app)
}

#[tauri::command]
fn island_rect(app: AppHandle, x: f64, y: f64, w: f64, h: f64, solid: bool) {
    if let Ok(mut rect) = app.state::<AppState>().island_rect.lock() {
        *rect = IslandRect { x, y, w, h, solid };
    }
}

/* The island's Open: bring Hark forward on that plan. */
#[tauri::command]
fn island_open(app: AppHandle) {
    let title = island_view(&app).title;
    open_plan(&app, title);
}

#[tauri::command]
fn island_hide_plan(app: AppHandle) {
    let title = island_view(&app).title;
    if let Ok(mut plans) = app.state::<AppState>().plans.lock() {
        plans.hidden = Some(title);
    }
    refresh_plans_soon(&app);
}

#[tauri::command]
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
        if state.page_finished.load(Ordering::SeqCst) {
            splash_retry(app.clone());
        }
        open_splash(app);
        return;
    }
    show_main_now(app);
}

fn show_main_now(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.request_user_attention(None);
    }
}

/* Closing the window keeps Hark in the tray. The first time, say so. */
fn hide_to_tray(app: &AppHandle, main: &WebviewWindow) {
    let _ = main.hide();
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
    .title("Hark Settings")
    .inner_size(540.0, 660.0)
    .min_inner_size(460.0, 520.0)
    .center()
    .resizable(true)
    .build()
    {
        polish_frame(&win);
        hold_corners(&win);
    }
}

/* The quick ask opens in the upper part of the screen the pointer is on. */
fn place_quick(app: &AppHandle, quick: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let area = monitor.work_area();
    let width = (QUICK_SIZE.0 * monitor.scale_factor()).round() as i32;
    let x = area.position.x + (area.size.width as i32 - width) / 2;
    let y = area.position.y + (f64::from(area.size.height) * 0.18).round() as i32;
    let _ = quick.set_position(PhysicalPosition::new(x, y));
}

fn toggle_quick(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("quick") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
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

/* Off the calling thread: building the island window from a synchronous
 * command or a window event can deadlock WebView2 on Windows. */
fn refresh_plans_soon(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || refresh_plans(&handle));
}

/* Bring the island and the tray in line with the plans Hark is running. */
fn refresh_plans(app: &AppHandle) {
    update_tray(app);
    let view = island_view(app);
    if island_wanted(app) {
        if let Some(island) = ensure_island(app) {
            if !island.is_visible().unwrap_or(false) {
                place_island(app, &island);
                let _ = island.show();
            }
            let _ = app.emit_to("island", "plan-state", &view);
        }
        return;
    }
    let Some(island) = app.get_webview_window("island") else {
        return;
    };
    let _ = app.emit_to("island", "plan-state", PlanView::idle());
    if island.is_visible().unwrap_or(false) {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(450)); // let the pill lift away
            if !island_wanted(&handle) {
                if let Some(island) = handle.get_webview_window("island") {
                    let _ = island.hide();
                }
            }
        });
    }
}

fn ensure_island(app: &AppHandle) -> Option<WebviewWindow> {
    if cfg!(target_os = "linux") {
        return None;
    }
    if let Some(win) = app.get_webview_window("island") {
        return Some(win);
    }
    /* Never focusable: it must not take the keyboard from whatever you're doing. */
    let win = frameless(WebviewWindowBuilder::new(
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
    .build()
    .ok()?;
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
    let area = monitor.work_area();
    let width = (ISLAND_SIZE.0 * monitor.scale_factor()).round() as i32;
    let x = area.position.x + (area.size.width as i32 - width) / 2;
    let _ = island.set_position(PhysicalPosition::new(x, area.position.y));
}

/* The island window lets every click through except on the pill itself. A
 * see-through window can't feel the mouse while it ignores it, so this
 * watches the pointer instead: it tells the page when the pointer is over the
 * pill (it opens up after a moment), and lets the pill take clicks only once
 * it is fully on screen. A tucked island never blocks the tabs beneath it. */
fn watch_island_pointer(app: AppHandle) {
    std::thread::spawn(move || {
        let mut inside = false;
        let mut catching = false;
        loop {
            if app.state::<AppState>().quitting.load(Ordering::SeqCst) {
                break;
            }
            let Some(island) = app.get_webview_window("island") else {
                break;
            };
            if !island.is_visible().unwrap_or(false) {
                if inside {
                    inside = false;
                    let _ = app.emit_to("island", "island-pointer", json!({ "inside": false }));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let mut rect = app
                .state::<AppState>()
                .island_rect
                .lock()
                .map(|r| *r)
                .unwrap_or_default();
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
            if let (Ok(cursor), Ok(origin), Ok(scale)) = (
                app.cursor_position(),
                island.outer_position(),
                island.scale_factor(),
            ) {
                let left = f64::from(origin.x) + rect.x * scale;
                let top = f64::from(origin.y) + rect.y * scale;
                // A tucked pill is a few pixels tall: give the screen edge a little more.
                let height = (rect.h * scale).max(8.0 * scale);
                let now_inside = rect.w > 0.0
                    && cursor.x >= left
                    && cursor.x <= left + rect.w * scale
                    && cursor.y >= top
                    && cursor.y <= top + height;
                if now_inside != inside {
                    inside = now_inside;
                    let _ = app.emit_to("island", "island-pointer", json!({ "inside": inside }));
                }
                let catch = inside && rect.solid;
                if catch != catching {
                    catching = catch;
                    let _ = island.set_ignore_cursor_events(!catch);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/* Hosts allowed to navigate inside the app: Hark itself plus the identity,
 * challenge and payment providers its flows bounce through. Everything else
 * opens in the user's default browser. Mirrors bridge.js. */
fn host_allowed(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    const HOSTS: [&str; 7] = [
        "hark.com",
        "accounts.google.com",
        "appleid.apple.com",
        "challenges.cloudflare.com",
        "js.stripe.com",
        "checkout.stripe.com",
        "m.stripe.network",
    ];
    const SUFFIXES: [&str; 6] = [
        ".hark.com",
        ".google.com",
        ".gstatic.com",
        ".apple.com",
        ".stripe.com",
        ".cloudflare.com",
    ];
    HOSTS.contains(&host) || SUFFIXES.iter().any(|s| host.ends_with(s))
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
    let Ok(splash) = frameless(WebviewWindowBuilder::new(
        app,
        "splash",
        WebviewUrl::App("index.html".into()),
    ))
    .title("Hark")
    .inner_size(1440.0, 920.0)
    .center()
    .visible(false)
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
        if maximized {
            let _ = splash.maximize();
        } else {
            let _ = splash.set_position(position);
            let _ = splash.set_size(size);
        }
    }
    let _ = splash.show();
    let _ = splash.set_focus();
}

/* Where the main window will open, read from the window-state plugin's file
 * before the main window exists. Ignored if it is no longer on any screen. */
fn saved_main_bounds(
    app: &AppHandle,
) -> Option<(PhysicalPosition<i32>, tauri::PhysicalSize<u32>, bool)> {
    let file = app.path().app_config_dir().ok()?.join(".window-state.json");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let main = saved.get("main")?;
    let number = |key: &str| main.get(key).and_then(Value::as_i64);
    let (x, y) = (number("x")? as i32, number("y")? as i32);
    let (width, height) = (number("width")? as u32, number("height")? as u32);
    let centre = (
        f64::from(x) + f64::from(width) / 2.0,
        f64::from(y) + f64::from(height) / 2.0,
    );
    app.monitor_from_point(centre.0, centre.1).ok().flatten()?;
    let maximized = main.get("maximized").and_then(Value::as_bool) == Some(true);
    Some((
        PhysicalPosition::new(x, y),
        tauri::PhysicalSize::new(width, height),
        maximized,
    ))
}

/* Hark has painted: bring the daylight up, show Hark underneath the loading
 * screen and fade the loading screen away. Or stay in the tray. */
fn reveal_main(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.first_load_done.swap(true, Ordering::SeqCst) {
        return;
    }
    let pending = state
        .pending_compose
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let splash = app.get_webview_window("splash");
    if state.start_hidden.load(Ordering::SeqCst) {
        if let Some(splash) = splash {
            let _ = splash.close();
        }
        return;
    }
    let Some(splash) = splash.filter(|s| s.is_visible().unwrap_or(false)) else {
        show_main_now(app);
        if let Some(text) = pending {
            deliver_compose(app, text, 400);
        }
        return;
    };
    // The loading screen may have been moved or resized: Hark opens where it is.
    if let Some(main) = app.get_webview_window("main") {
        if splash.is_maximized().unwrap_or(false) {
            let _ = main.maximize();
        } else if cfg!(windows) {
            if let (Ok(position), Ok(size)) = (splash.outer_position(), splash.outer_size()) {
                let _ = main.set_position(position);
                let _ = main.set_size(size);
            }
        }
    }
    let minimized = splash.is_minimized().unwrap_or(false);
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
    .inner_size(1440.0, 920.0)
    /* The signed-out screen is a locked, non-scrolling hero (bridge.js);
     * its sign-in form and legal text need ~660px, so don't let the window
     * shrink below that or the bottom of it becomes unreachable. */
    .min_inner_size(900.0, 680.0)
    .center()
    .visible(false)
    .focused(true)
    .initialization_script(bridge)
    .on_navigation(move |url: &Url| {
        if host_allowed(url) {
            true
        } else {
            let _ = nav_app.opener().open_url(url.as_str(), None::<&str>);
            false
        }
    })
    .on_new_window(move |url: Url, _features| {
        if host_allowed(&url) {
            // Auth redirects (Google / Apple) complete in-place.
            if let Some(main) = new_win_app.get_webview_window("main") {
                let _ = main.navigate(url);
            }
        } else {
            let _ = new_win_app.opener().open_url(url.as_str(), None::<&str>);
        }
        tauri::webview::NewWindowResponse::Deny
    })
    .on_page_load(|window, payload| {
        // The loading screen waits for bridge.js to say Hark has painted
        // ("hark-painted"); this only notes whether the last load has ended.
        let finished = payload.event() == tauri::webview::PageLoadEvent::Finished;
        window
            .app_handle()
            .state::<AppState>()
            .page_finished
            .store(finished, Ordering::SeqCst);
    })
    .on_download(move |_webview, event| {
        match event {
            tauri::webview::DownloadEvent::Requested { url, destination } => {
                let name = url
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("hark-download")
                    .to_string();
                if let Ok(dir) = download_app.path().download_dir() {
                    *destination = dir.join(name);
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
    // Hark in front: hide the plan island, and a finished plan counts as seen.
    let focus_app = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            if *focused {
                if let Ok(mut plans) = focus_app.state::<AppState>().plans.lock() {
                    plans.finished = None;
                }
            }
            refresh_plans_soon(&focus_app);
        }
    });
    Ok(())
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
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
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

fn short_title(title: &str) -> String {
    if title.chars().count() > 42 {
        format!("{}…", title.chars().take(41).collect::<String>())
    } else {
        title.to_string()
    }
}

/* Tray icon, tooltip and menu, the unread badge, and the running plan. */
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
    let title = short_title(&plan.title);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tooltip = match plan.phase {
            "running" => format!(
                "Hark · {title}, step {} of {}",
                (plan.done + 1).min(plan.total),
                plan.total
            ),
            "done" => format!("Hark · {title} is done"),
            _ if unread > 0 => format!("Hark · {unread} unread"),
            _ => "Hark".to_string(),
        };
        let _ = tray.set_tooltip(Some(tooltip));
        if let Some(icon) = tray_icon(&plan, unread > 0) {
            let _ = tray.set_icon(Some(icon));
        }
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_badge_count(if unread > 0 { Some(unread) } else { None });
    }
    let Ok(mut slot) = state.tray_menu.lock() else {
        return;
    };
    let Some(tray_menu) = slot.as_mut() else {
        return;
    };
    let _ = tray_menu.open.set_text(if unread > 0 {
        format!("Open Hark ({unread} unread)")
    } else {
        "Open Hark".to_string()
    });
    let show_plan = plan.phase != "idle";
    if show_plan {
        let _ = tray_menu.plan.set_text(match plan.phase {
            "done" => format!("{title} · done"),
            _ => format!("{title} · {} of {}", plan.done, plan.total),
        });
    }
    if show_plan != tray_menu.plan_shown {
        if show_plan {
            let _ = tray_menu.menu.insert(&tray_menu.plan, 3);
            let _ = tray_menu.menu.insert(&tray_menu.plan_gap, 4);
        } else {
            let _ = tray_menu.menu.remove(&tray_menu.plan);
            let _ = tray_menu.menu.remove(&tray_menu.plan_gap);
        }
        tray_menu.plan_shown = show_plan;
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
        let title = v.get("title").and_then(Value::as_str).unwrap_or("Hark");
        let body = v.get("body").and_then(Value::as_str).unwrap_or("");
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
        update_tray(&h);
    });

    // The loading screen hands over once Hark has painted. If the paint signal
    // never comes but the page is up, show it anyway after a few seconds.
    let h = app.clone();
    app.listen("hark-painted", move |_event| reveal_main(&h));
    let h = app.clone();
    app.listen("hark-ready", move |_event| {
        let h = h.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(8));
            reveal_main(&h);
        });
    });

    let h = app.clone();
    app.listen("hark-plans", move |event| {
        let v = parse_payload(&event);
        let active: Vec<PlanInfo> = v
            .get("plans")
            .cloned()
            .and_then(|p| serde_json::from_value(p).ok())
            .unwrap_or_default();
        if let Ok(mut plans) = h.state::<AppState>().plans.lock() {
            plans.active = active;
        }
        refresh_plans_soon(&h);
    });

    let h = app.clone();
    app.listen("hark-plan-done", move |event| {
        let v = parse_payload(&event);
        let title = v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
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
        let island = !cfg!(target_os = "linux") && settings_snapshot(&h).taskbar_progress;
        if !looking && !island && !title.is_empty() {
            notify(&h, "Plan finished", &title, false);
        }
        refresh_plans_soon(&h);
    });

    let h = app.clone();
    app.listen("hark-progress", move |event| {
        if !settings_snapshot(&h).taskbar_progress {
            return;
        }
        let v = parse_payload(&event);
        let value = v.get("value").and_then(Value::as_u64);
        set_taskbar_progress(&h, value);
    });

    let h = app.clone();
    app.listen("hark-external", move |event| {
        let v = parse_payload(&event);
        if let Some(url) = v.get("url").and_then(Value::as_str) {
            let _ = h.opener().open_url(url, None::<&str>);
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
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
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
            pending_compose: Mutex::new(None),
            unread: AtomicI64::new(0),
            plans: Mutex::new(Plans::default()),
            island_rect: Mutex::new(IslandRect::default()),
            tray_menu: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_setting,
            test_notification,
            reload_main,
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
            apply_autostart(handle, settings.autostart);

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
                if !splash_up && state.page_finished.load(Ordering::SeqCst) {
                    splash_retry(retry.clone());
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
            tauri::RunEvent::ExitRequested { api, .. } => {
                let state = app.state::<AppState>();
                if !state.quitting.load(Ordering::SeqCst) && settings_snapshot(app).close_to_tray {
                    api.prevent_exit();
                }
            }
            /* Clicking the Dock icon while Hark is hidden in the menu bar. */
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => show_main(app),
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
