import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./platform";

const input = document.getElementById("input") as HTMLTextAreaElement | null;

function submit() {
  const text = input?.value.trim() ?? "";
  void invoke("quick_compose", { text });
  if (input) input.value = "";
}

input?.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    submit();
  }
  if (e.key === "Escape") void getCurrentWindow().hide();
});
document.getElementById("send")?.addEventListener("click", submit);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") void getCurrentWindow().hide();
});
window.addEventListener("focus", () => input?.focus());
input?.focus();
