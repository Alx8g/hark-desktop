//! Hark Desktop: a featherweight Tauri v2 shell around the real hark.com app,
//! for Windows, macOS and Linux. No Electron, no bundled Chromium: the
//! operating system's own web view renders Hark (WebView2, WKWebView or
//! WebKitGTK) and Rust owns everything native (tray residence, notifications,
//! taskbar/dock progress, autostart, global shortcut, downloads).
//! Page-side behaviour lives in `bridge.js`, injected at document start.
//!
//! Windows is the reference platform: it gets the custom frameless, rounded
//! window. macOS keeps a native window frame with a transparent title bar and
//! Linux keeps the window manager's frame, because transparent undecorated
//! windows are not dependable everywhere.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{
    AppHandle, Emitter, Listener, Manager, Url, UserAttentionType, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_store::StoreExt;

const HARK_URL: &str = "https://hark.com/";
const BRIDGE_JS: &str = include_str!("bridge.js");
const TRAY_ID: &str = "main-tray";
const STORE_FILE: &str = "hark-settings.json";
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
    first_load_done: AtomicBool,
    start_hidden: AtomicBool,
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
    if key == "taskbar_progress" && !value {
        set_taskbar_progress(&app, None);
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
    if let Some(offline) = app.get_webview_window("offline") {
        let _ = offline.close();
    }
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
    show_main(&app);
    // Give the window a beat to foreground before the page receives the text.
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(180));
        let _ = handle.emit_to("main", "hark-compose", json!({ "text": text }));
    });
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

fn show_main(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
        let _ = main.request_user_attention(None);
    }
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
    use tauri::menu::{PredefinedMenuItem, Submenu};

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

fn toggle_quick(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("quick") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
        return;
    }
    /* The dock draws its own rounded card and soft shadow inside the window's
     * transparent padding, so there is no full-window clip here. */
    if let Ok(win) = frameless(WebviewWindowBuilder::new(
        app,
        "quick",
        WebviewUrl::App("quick.html".into()),
    ))
    .title("Ask Hark")
    .always_on_top(true)
    .skip_taskbar(true)
    .inner_size(600.0, 176.0)
    .center()
    .resizable(false)
    .build()
    {
        polish_frame(&win);
    }
}

fn open_offline(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("offline") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    if let Ok(win) = panel(WebviewWindowBuilder::new(
        app,
        "offline",
        WebviewUrl::App("offline.html".into()),
    ))
    .title("Hark — Offline")
    .inner_size(520.0, 420.0)
    .center()
    .resizable(false)
    .build()
    {
        polish_frame(&win);
    }
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

fn build_splash(app: &AppHandle) {
    if let Ok(win) = frameless(WebviewWindowBuilder::new(
        app,
        "splash",
        WebviewUrl::App("index.html".into()),
    ))
    .title("Hark")
    .inner_size(400.0, 270.0)
    .center()
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .build()
    {
        polish_frame(&win);
    }
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
        if payload.event() != tauri::webview::PageLoadEvent::Finished {
            return;
        }
        let app = window.app_handle();
        let state = app.state::<AppState>();
        if state.first_load_done.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(splash) = app.get_webview_window("splash") {
            let _ = splash.close();
        }
        if !state.start_hidden.load(Ordering::SeqCst) {
            show_main(app);
        }
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
    Ok(())
}

/* --------------------------------- tray --------------------------------- */

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Hark", true, None::<&str>)?;
    let quick = MenuItem::with_id(app, "quick", "Ask Hark…", true, Some(quick_label()))?;
    let brief = MenuItem::with_id(app, "brief", "Latest brief", true, None::<&str>)?;
    let projects = MenuItem::with_id(app, "projects", "Projects", true, None::<&str>)?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        "Desktop settings…",
        true,
        Some(settings_label()),
    )?;
    let browser = MenuItem::with_id(
        app,
        "browser",
        "Open hark.com in browser",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Hark", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open, &quick, &brief, &projects, &settings, &browser, &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Hark")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quick" => toggle_quick(app),
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
    #[cfg(target_os = "macos")]
    {
        if let Some(icon) = tray_icon(false) {
            builder = builder.icon(icon);
        }
        builder = builder.icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/* Windows and Linux show the coloured app tile. The macOS menu bar wants a
 * one-colour "template" glyph (black + alpha) that the system tints for light
 * and dark menu bars, so the unread state is a dot in that glyph, not a colour. */
fn tray_icon(unread: bool) -> Option<tauri::image::Image<'static>> {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = if unread {
        include_bytes!("../icons/tray-template-dot.png")
    } else {
        include_bytes!("../icons/tray-template.png")
    };
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = if unread {
        include_bytes!("../icons/tray-dot.png")
    } else {
        include_bytes!("../icons/tray.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}

fn update_tray(app: &AppHandle, unread: i64) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let tooltip = if unread > 0 {
        format!("Hark — {unread} unread")
    } else {
        "Hark".to_string()
    };
    let _ = tray.set_tooltip(Some(tooltip));
    if let Some(icon) = tray_icon(unread > 0) {
        let _ = tray.set_icon(Some(icon));
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_badge_count(if unread > 0 { Some(unread) } else { None });
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
        update_tray(&h, count);
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
                    let _ = main.hide();
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
        .plugin(tauri_plugin_window_state::Builder::new().build())
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
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_setting,
            test_notification,
            reload_main,
            quick_compose
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
                build_splash(handle);
            }
            build_main_window(handle)?;
            install_bridge_listeners(handle);

            let _ = handle.global_shortcut().register(QUICK_SHORTCUT);
            #[cfg(target_os = "macos")]
            install_macos_menu(handle)?;

            // If the first load never lands (offline at launch), stop hiding
            // behind the splash and explain instead.
            let slow = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(25));
                let state = slow.state::<AppState>();
                if state.first_load_done.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(splash) = slow.get_webview_window("splash") {
                    let _ = splash.close();
                }
                open_offline(&slow);
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
                        let _ = main.hide();
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
