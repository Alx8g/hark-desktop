use base64::Engine;
use std::{env, fs, path::PathBuf};

/// "No admin rights needed", plus sharp text on high-DPI screens. Without a requested execution level, Windows
/// treats any program with "setup" in its name as an installer and asks for admin rights before it starts.
const MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#;

fn main() {
    let here = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = here.join("..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let read = |p: &str| {
        println!("cargo:rerun-if-changed={}", root.join(p).display());
        fs::read(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
    };

    // The app's version, from the one place it is set. HARK_SETUP_VERSION overrides it when the setup is rebuilt for
    // an earlier release (setup.yml), so it checks for the version it carries.
    let conf: serde_json::Value =
        serde_json::from_slice(&read("src-tauri/tauri.conf.json")).unwrap();
    println!("cargo:rerun-if-env-changed=HARK_SETUP_VERSION");
    let version = env::var("HARK_SETUP_VERSION").unwrap_or_else(|_| {
        conf["version"]
            .as_str()
            .expect("tauri.conf.json version")
            .to_string()
    });
    println!("cargo:rustc-env=HARK_VERSION={version}");

    // The standard installer this setup carries and runs. Release builds point HARK_SETUP_PAYLOAD at the NSIS
    // setup.exe that tauri build made; without it the setup still builds (for checks) but has nothing to install.
    println!("cargo:rerun-if-env-changed=HARK_SETUP_PAYLOAD");
    let payload = out.join("payload.exe");
    match env::var_os("HARK_SETUP_PAYLOAD") {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&p).display());
            fs::copy(&p, &payload).unwrap_or_else(|e| panic!("HARK_SETUP_PAYLOAD: {e}"));
        }
        None => fs::write(&payload, b"").unwrap(),
    }

    // The card, with its fonts and the h mark inlined: WebView2 gets it as one HTML string.
    let b64 = |p: &str| base64::engine::general_purpose::STANDARD.encode(read(p));
    let page = String::from_utf8(read("src-setup/ui/setup.html"))
        .unwrap()
        .replace("__INTER__", &b64("docs/assets/fonts/inter-tight.woff2"))
        .replace("__NEWS__", &b64("docs/assets/fonts/newsreader-opsz.woff2"))
        .replace(
            "__NEWSI__",
            &b64("docs/assets/fonts/newsreader-italic.woff2"),
        )
        .replace("__MARK__", &b64("branding/mark.png"));
    fs::write(out.join("setup.html"), page).unwrap();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let nums: Vec<u64> = version
            .split(['.', '-', '+'])
            .take(3)
            .map(|n| n.parse().unwrap_or(0))
            .collect();
        let packed = (nums[0] << 48)
            | (nums.get(1).copied().unwrap_or(0) << 32)
            | (nums.get(2).copied().unwrap_or(0) << 16);
        let mut res = tauri_winres::WindowsResource::new();
        res.set_icon("../src-tauri/icons/icon.ico")
            .set("ProductName", "Hark Desktop")
            .set("FileDescription", "Hark Desktop Setup")
            .set("CompanyName", "Hark Desktop contributors")
            .set(
                "LegalCopyright",
                "MIT licensed. Unofficial app, not made by Hark.",
            )
            .set("OriginalFilename", "Hark-Desktop-Setup.exe")
            .set("ProductVersion", &version)
            .set("FileVersion", &version)
            .set_version_info(tauri_winres::VersionInfo::PRODUCTVERSION, packed)
            .set_version_info(tauri_winres::VersionInfo::FILEVERSION, packed)
            .set_manifest(MANIFEST);
        res.compile().expect("Windows resources");
    }
}
