import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./frame";
import { platform } from "./platform";

// "Hide to tray" is the Windows wording; macOS has no tray to hide into, and
// Linux desktops often lack one.
const hide = document.getElementById("hide");
if (hide && platform === "macos") hide.textContent = "Hide";
if (hide && platform === "linux") hide.textContent = "Dismiss";

document.getElementById("retry")?.addEventListener("click", () => {
  void invoke("reload_main");
});
document.getElementById("hide")?.addEventListener("click", () => {
  void getCurrentWindow().hide();
});
window.addEventListener("online", () => {
  void invoke("reload_main");
});
