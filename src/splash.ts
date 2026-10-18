import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./frame";
import { platform } from "./platform";

// Sunrise. Says nothing while hark.com loads normally (a few seconds); after
// 4 seconds it says it is connecting, after 12 that it is slow, and straight
// away if the computer is offline. lib.rs sends "hark-loaded" once Hark has
// painted, then shows Hark underneath this window and closes it.
const body = document.body;
const text = document.getElementById("text") as HTMLElement;
const win = getCurrentWindow();
const HIDE = platform === "windows" ? "Hide to tray" : platform === "macos" ? "Hide" : "Dismiss";

let timers: number[] = [];
let loaded = false;

function say(html: string): void {
  text.innerHTML = html;
}

function clearTimers(): void {
  timers.forEach((t) => window.clearTimeout(t));
  timers = [];
}

function offline(): void {
  if (loaded) return;
  clearTimers();
  body.classList.remove("rise");
  body.classList.add("dusk");
  say(`<h1>You’re offline</h1><p>Hark opens by itself as soon as you’re back online.</p><div class="acts"><button class="pill-btn primary" data-act="retry">Try again</button><button class="pill-btn" data-act="hide">${HIDE}</button></div>`);
}

function start(): void {
  clearTimers();
  say("");
  body.classList.remove("dusk");
  if (!navigator.onLine) {
    offline();
    return;
  }
  requestAnimationFrame(() => requestAnimationFrame(() => body.classList.add("rise")));
  timers.push(window.setTimeout(() => say('<div class="status">Connecting to hark.com</div>'), 4000));
  timers.push(window.setTimeout(() => say('<h1>Taking longer than usual</h1><p>hark.com is slow to answer. It may be your connection, or Hark itself.</p><div class="acts"><button class="pill-btn primary" data-act="retry">Try again</button><button class="pill-btn" data-act="browser">Open in browser</button></div>'), 12000));
}

function retry(): void {
  void invoke("splash_retry");
  start();
}

document.addEventListener("click", (e) => {
  const act = (e.target as Element).closest("[data-act]")?.getAttribute("data-act");
  if (act === "retry") retry();
  else if (act === "browser") void openUrl("https://hark.com/");
  else if (act === "hide") void invoke("splash_hide");
  else if (act === "minimize") void win.minimize();
  else if (act === "maximize") void win.toggleMaximize();
});
window.addEventListener("offline", offline);
window.addEventListener("online", () => {
  if (body.classList.contains("dusk")) retry();
});

void listen("hark-loaded", () => {
  loaded = true;
  clearTimers();
  say("");
  body.classList.remove("dusk");
  body.classList.add("rise", "arrive");
  window.setTimeout(() => body.classList.add("out"), 380);
});

start();
