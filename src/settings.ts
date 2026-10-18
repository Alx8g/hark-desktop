import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./frame";
import { platform, type Platform } from "./platform";
import "./appearance";

// The page ships with the Windows wording; the other desktops say their own words.
const WORDS: Record<Platform, Record<string, string>> = {
  windows: {},
  macos: {
    "autostart-title": "Open at login",
    "autostart-help": "Hark starts in the menu bar when you log in.",
    "tray-title": "Keep running in the menu bar",
    "tray-help": "Closing the window hides Hark. Quit with ⌘Q.",
    "progress-help": "Show running plans at the top of the screen and in the Dock.",
  },
  linux: {
    "autostart-title": "Start at login",
    "autostart-help": "Hark starts hidden when you log in.",
    "tray-title": "Keep running in the system tray",
    "tray-help": "Needs a system tray (on GNOME, an AppIndicator extension).",
    "progress-help": "Show running plans in the tray and on the launcher icon.",
  },
};

function localise(): void {
  for (const [id, text] of Object.entries(WORDS[platform])) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
  }
  if (platform === "macos") {
    document.querySelectorAll("kbd.mod").forEach((k) => (k.textContent = "⌘"));
  }
  if (platform !== "windows") {
    // F11 and dragging the top of the window are Windows-only; the others have a native frame.
    document.getElementById("drag-tip")?.remove();
    if (platform === "macos") document.getElementById("f11")?.remove();
  }
}
localise();

type Settings = {
  autostart: boolean;
  close_to_tray: boolean;
  notifications: boolean;
  taskbar_progress: boolean;
  quick_sends: boolean;
};

const binds: Array<[string, keyof Settings]> = [
  ["autostart", "autostart"],
  ["tray", "close_to_tray"],
  ["notifications", "notifications"],
  ["progress", "taskbar_progress"],
  ["quick-sends", "quick_sends"],
];

async function init() {
  const settings = await invoke<Settings>("get_settings");
  for (const [id, key] of binds) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.setAttribute("aria-checked", String(settings[key]));
    el.addEventListener("click", async () => {
      const next = el.getAttribute("aria-checked") !== "true";
      el.setAttribute("aria-checked", String(next));
      try {
        await invoke("set_setting", { key, value: next });
      } catch {
        el.setAttribute("aria-checked", String(!next)); // not saved: show what is really set
      }
    });
  }
  const versionEl = document.getElementById("version");
  if (versionEl) {
    versionEl.textContent = `Hark Desktop ${await getVersion()} · Unofficial client · your account stays on hark.com`;
  }
}

document.getElementById("close")?.addEventListener("click", () => {
  void getCurrentWindow().close();
});
document.getElementById("test-toast")?.addEventListener("click", () => {
  void invoke("test_notification");
});
document.getElementById("reload")?.addEventListener("click", () => {
  void invoke("reload_main");
});
document.getElementById("open-hark")?.addEventListener("click", () => {
  void openUrl("https://hark.com/");
});

void init();
