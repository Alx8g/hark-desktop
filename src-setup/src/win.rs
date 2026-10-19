use std::os::windows::process::CommandExt;
use std::{
    ffi::OsStr,
    fs, io,
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tao::{
    dpi::{LogicalSize, PhysicalPosition},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows},
    window::{Icon, Window, WindowBuilder},
};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

const VERSION: &str = env!("HARK_VERSION");
/// The standard installer, built by `tauri build` and embedded by build.rs.
static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.exe"));
/// ui/setup.html with its fonts inlined by build.rs.
const PAGE: &str = include_str!(concat!(env!("OUT_DIR"), "/setup.html"));
/// Where the standard installer records the install. Per user, so no admin rights.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Hark";
const MAIN_BINARY: &str = "hark-windows.exe";
/// The icon tauri-winres embeds (IDI_APPLICATION's number, so Windows finds it first).
const ICON_ID: u16 = 32512;
/// The card's size in logical pixels.
const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 320.0;

enum Msg {
    /// A message from the card: ready, drag, install, open, close or standard.
    Page(String),
    Show,
    Progress(f64),
    Installed,
    Failed,
    Quit,
}

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `/S` (any case) skips the card, for scripts and IT: every argument goes to the standard installer.
    if args.iter().any(|a| a.eq_ignore_ascii_case("/S")) {
        let was_running = hark_running();
        let code = run_standard(&args).ok().and_then(|s| s.code()).unwrap_or(1);
        if was_running && !hark_running() {
            start_hark(&["--minimized"]);
        }
        std::process::exit(code);
    }
    if PAYLOAD.is_empty() {
        alert("This copy of Hark Desktop's setup has nothing to install. Download it again from harkdesk.top.");
        std::process::exit(1);
    }
    // card() only returns if the card can't be shown (WebView2 missing, say). The standard installer has its own
    // window and installs WebView2 when it's missing, so it takes over.
    let Err(e) = card();
    log(&format!("no card ({e}): running the standard installer"));
    let _ = run_standard::<&str>(&[]);
    remove_profile();
}

/// WebView2's profile for the card. It holds nothing worth keeping and grows to tens of megabytes, so the setup
/// deletes it on the way out.
fn profile_dir() -> PathBuf {
    std::env::temp_dir().join("hark-desktop-setup")
}

/// WebView2's own processes let go of the profile a moment after the card closes; keep trying for a few seconds.
fn remove_profile() {
    let dir = profile_dir();
    for _ in 0..40 {
        if !dir.exists() || fs::remove_dir_all(&dir).is_ok() {
            log("removed WebView2's profile");
            return;
        }
        thread::sleep(Duration::from_millis(150));
    }
    log("WebView2's profile is still in use: left in Temp");
}

fn card() -> Result<std::convert::Infallible, Box<dyn std::error::Error>> {
    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let icon = Icon::from_resource(ICON_ID, None).ok();
    let window = WindowBuilder::new()
        .with_title("Hark Setup")
        .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
        .with_resizable(false)
        .with_maximizable(false)
        .with_decorations(false)
        .with_undecorated_shadow(true)
        .with_visible(false)
        .with_window_icon(icon.clone())
        .with_taskbar_icon(icon)
        .build(&event_loop)?;
    round_corners(window.hwnd());
    if let Some(m) = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
    {
        let (at, size, outer) = (m.position(), m.size(), window.outer_size());
        let x = at.x + (size.width as i32 - outer.width as i32) / 2;
        let y = at.y + ((size.height as i32 - outer.height as i32) as f64 * 0.44) as i32;
        window.set_outer_position(PhysicalPosition::new(x, y));
    }

    // WebView2 needs a folder for its profile. Left to itself it makes one next to this program, in Downloads.
    let mut context = WebContext::new(Some(profile_dir()));
    let from_page = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_html(PAGE)
        .with_background_color((0xb6, 0xcf, 0xe7, 0xff))
        .with_hotkeys_zoom(false)
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_ipc_handler(move |req| {
            let _ = from_page.send_event(Msg::Page(req.into_body()));
        })
        .build(&window)?;
    // Held in an Option so the card can be closed before its profile is deleted (see LoopDestroyed).
    let mut webview = Some(webview);

    // The card shows itself once its fonts are in; this is the backstop if that message never comes.
    let backstop = proxy.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(4));
        let _ = backstop.send_event(Msg::Show);
    });

    let (mut busy, mut done) = (false, false);
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &context;
        match event {
            Event::UserEvent(Msg::Page(m)) => match m.as_str() {
                "ready" => show(&window),
                "drag" => {
                    let _ = window.drag_window();
                }
                "install" if !busy && !done => {
                    busy = true;
                    let p = proxy.clone();
                    thread::spawn(move || install(&p));
                }
                "open" if done => {
                    start_hark(&[]);
                    *flow = ControlFlow::Exit;
                }
                "close" if !busy => *flow = ControlFlow::Exit,
                "standard" if !busy => {
                    busy = true;
                    window.set_visible(false);
                    let p = proxy.clone();
                    thread::spawn(move || {
                        let _ = run_standard::<&str>(&[]);
                        let _ = p.send_event(Msg::Quit);
                    });
                }
                _ => {}
            },
            Event::UserEvent(Msg::Show) => show(&window),
            Event::UserEvent(Msg::Progress(p)) => js(&webview, &format!("progress({p:.3})")),
            Event::UserEvent(Msg::Installed) => {
                (busy, done) = (false, true);
                js(&webview, "installed()");
            }
            Event::UserEvent(Msg::Failed) => {
                busy = false;
                js(&webview, "failed()");
            }
            Event::UserEvent(Msg::Quit) => *flow = ControlFlow::Exit,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } if !busy => *flow = ControlFlow::Exit,
            // The last event before the process ends: close the card out of sight, then tidy up after WebView2.
            Event::LoopDestroyed => {
                window.set_visible(false);
                drop(webview.take());
                remove_profile();
            }
            _ => {}
        }
    })
}

fn show(window: &Window) {
    if !window.is_visible() {
        window.set_visible(true);
        window.set_focus();
    }
}

fn js(webview: &Option<WebView>, script: &str) {
    if let Some(webview) = webview {
        let _ = webview.evaluate_script(script);
    }
}

/// Runs the standard installer silently, easing the bar along while it works, then checks the install is really
/// there before the card says so.
fn install(proxy: &EventLoopProxy<Msg>) {
    let started = Instant::now();
    // The standard installer closes a running Hark to replace it. Note that, to put Hark back afterwards.
    let was_running = hark_running();
    let result = (|| -> io::Result<bool> {
        let (dir, exe) = unpack()?;
        let run = || -> io::Result<ExitStatus> {
            let mut child = Command::new(&exe).arg("/S").spawn()?;
            loop {
                if let Some(status) = child.try_wait()? {
                    return Ok(status);
                }
                // A silent install reports nothing, and usually takes about two seconds.
                let t = started.elapsed().as_secs_f64();
                let _ = proxy.send_event(Msg::Progress(0.9 * (1.0 - (-t / 1.4).exp())));
                thread::sleep(Duration::from_millis(100));
            }
        };
        let status = run();
        let _ = fs::remove_dir_all(&dir);
        let status = status?;
        let verified = status.success() && installed_ok();
        log(&format!(
            "standard installer exit {:?}, verified {verified}, {} ms",
            status.code(),
            started.elapsed().as_millis()
        ));
        Ok(verified)
    })();
    // A very fast install still shows the bar for a moment, so it reads as an install.
    if let Some(rest) = Duration::from_millis(1200).checked_sub(started.elapsed()) {
        thread::sleep(rest);
    }
    if let Err(e) = &result {
        log(&format!("install failed: {e}"));
    }
    // Back in the tray where it was; Open Hark then just brings it forward.
    if was_running && !hark_running() {
        start_hark(&["--minimized"]);
    }
    let _ = proxy.send_event(if matches!(result, Ok(true)) {
        Msg::Installed
    } else {
        Msg::Failed
    });
}

/// Writes the standard installer to its own temporary folder, named as on the release page.
fn unpack() -> io::Result<(PathBuf, PathBuf)> {
    let dir = std::env::temp_dir().join(format!("hark-setup-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let exe = dir.join(format!("Hark_{VERSION}_x64-setup.exe"));
    if let Err(e) = fs::write(&exe, PAYLOAD) {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok((dir, exe))
}

fn run_standard<S: AsRef<OsStr>>(args: &[S]) -> io::Result<ExitStatus> {
    let (dir, exe) = unpack()?;
    let status = Command::new(&exe).args(args).status();
    let _ = fs::remove_dir_all(&dir);
    status
}

fn uninstall_value(name: &str) -> Option<String> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(UNINSTALL_KEY)
        .ok()?;
    key.get_value::<String, _>(name)
        .ok()
        .map(|v| v.trim_matches('"').to_string())
}

fn install_dir() -> PathBuf {
    uninstall_value("InstallLocation")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Hark")))
        .unwrap_or_default()
}

/// This version is recorded as installed and its program is on disk.
fn installed_ok() -> bool {
    uninstall_value("DisplayVersion").as_deref() == Some(VERSION)
        && install_dir().join(MAIN_BINARY).is_file()
}

/// Starts Hark. If it's already running, Hark hands the start to the running copy, which shows its window.
fn start_hark(args: &[&str]) {
    let dir = install_dir();
    // Hark outlives the setup, so it must not hold on to the setup's output (a log file or a script's pipe), and it
    // leaves whatever job the setup was started in when Windows allows that, so closing that launcher can't take
    // Hark down with it.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let start = |flags: u32| {
        Command::new(dir.join(MAIN_BINARY))
            .args(args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    match start(CREATE_BREAKAWAY_FROM_JOB).or_else(|_| start(0)) {
        Ok(_) => log(&format!("started Hark {args:?}")),
        Err(e) => log(&format!("couldn't start Hark: {e}")),
    }
}

fn hark_running() -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            let name = &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len())];
            found = String::from_utf16_lossy(name).eq_ignore_ascii_case(MAIN_BINARY);
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

/// Windows 11 rounds undecorated windows only when asked. Windows 10 ignores this.
fn round_corners(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let preference = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &preference as *const _ as _,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

fn alert(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide("Hark Setup").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// HARK_SETUP_LOG=1 prints what the setup does to stderr (redirect it to a file: release builds have no console).
fn log(line: &str) {
    if std::env::var_os("HARK_SETUP_LOG").is_some() {
        eprintln!("[setup] {line}");
    }
}
