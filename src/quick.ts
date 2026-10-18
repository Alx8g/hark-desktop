import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import "./platform";

// Ctrl+Shift+H: a little piece of Hark's sky with a greeting and a message bar.
// Enter hands the text to Hark; Esc, clicking away or switching apps closes it.
// Whatever you typed stays, selected, for the next time it opens.
const win = getCurrentWindow();
const panel = document.getElementById("panel") as HTMLElement;
const hello = document.getElementById("hello") as HTMLElement;
const input = document.getElementById("input") as HTMLTextAreaElement;
const send = document.getElementById("send") as HTMLButtonElement;

/* Must match QUICK_SIZE in lib.rs. */
const WIDTH = 700;

function greeting(): string {
  const hour = new Date().getHours();
  if (hour >= 5 && hour < 12) return "Good morning. <i>What's first?</i>";
  if (hour >= 12 && hour < 18) return "Good afternoon. <i>What's next?</i>";
  return "Good evening. <i>What's on your mind?</i>";
}

let height = 0;
function fit(): void {
  input.style.height = "32px";
  const full = input.scrollHeight;
  input.style.height = `${Math.min(Math.max(32, full), 148)}px`;
  panel.classList.toggle("multi", full > 40);
  const pad = parseFloat(getComputedStyle(document.body).paddingTop) + parseFloat(getComputedStyle(document.body).paddingBottom);
  const next = Math.ceil(panel.offsetHeight + pad);
  if (next !== height) {
    height = next;
    void win.setSize(new LogicalSize(WIDTH, height));
  }
}

function paint(): void {
  send.classList.toggle("on", input.value.trim().length > 0);
}

function hide(): void {
  void win.hide();
}

function submit(): void {
  const text = input.value.trim();
  void invoke("quick_compose", { text });
  input.value = "";
  paint();
  fit();
}

function arrive(): void {
  hello.innerHTML = greeting();
  panel.classList.remove("show");
  void panel.offsetWidth;
  panel.classList.add("show");
  input.focus();
  input.select();
}

input.addEventListener("input", () => {
  paint();
  fit();
});
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    submit();
  }
});
send.addEventListener("click", submit);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") hide();
});
// The margin round the panel is part of the window; a click there means "away".
document.body.addEventListener("mousedown", (e) => {
  if (!panel.contains(e.target as Node)) hide();
});
window.addEventListener("blur", hide);
window.addEventListener("focus", arrive);

arrive();
paint();
fit();
void document.fonts.ready.then(fit);
