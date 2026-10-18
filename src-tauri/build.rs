// The app's own commands are listed here so the permission system checks them
// too: each local window is granted only the ones it uses (capabilities/*.json),
// and hark.com is granted none.
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_settings",
            "set_setting",
            "test_notification",
            "reload_main",
            "app_loaded",
            "quick_compose",
            "splash_retry",
            "splash_hide",
            "island_state",
            "island_rect",
            "island_open",
            "island_hide_plan",
            "island_dismiss",
        ]),
    ))
    .expect("failed to run tauri-build");
}
