import { listen } from "@tauri-apps/api/event";

// Hark's theme (light or dark) and the sky image it is showing. lib.rs hands
// the last known values to every local window before it paints
// (window.__HARK_APPEARANCE__, set by an initialization script) and sends
// "hark-appearance" when Hark's change. shell.css does the rest through
// html[data-theme] and the --sky-image custom property.
export type Appearance = { theme?: string; sky?: string | null };

/* Only a small inline image from Hark's own stylesheet; anything else is ignored. */
const SKY = /^data:image\/(?:webp|png|jpeg);base64,[A-Za-z0-9+/=]+$/;

export function applyAppearance(next: Appearance | undefined): void {
  if (!next) return;
  const root = document.documentElement;
  if (next.theme === "dark" || next.theme === "light") root.dataset.theme = next.theme;
  if (next.sky && SKY.test(next.sky) && next.sky.length < 60000) {
    root.style.setProperty("--sky-image", `url("${next.sky}") center / cover no-repeat`);
    root.toggleAttribute("data-sky", true);
  }
}

applyAppearance((window as Window & { __HARK_APPEARANCE__?: Appearance }).__HARK_APPEARANCE__);
void listen<Appearance>("hark-appearance", (e) => applyAppearance(e.payload));
