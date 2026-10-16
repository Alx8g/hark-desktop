// Which desktop are we on? Read from the web view's user agent (WebView2,
// WKWebView and WebKitGTK each identify themselves). Sets <html data-platform>
// so shell.css can adapt, and exposes the few words that differ per platform.
export type Platform = "windows" | "macos" | "linux";

function detect(): Platform {
  const ua = navigator.userAgent;
  if (/Windows/i.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/i.test(ua)) return "macos";
  return "linux";
}

export const platform: Platform = detect();
document.documentElement.dataset.platform = platform;

// Settings and Offline use the operating system's own window frame everywhere
// except Windows, where Hark draws a frameless one (see `panel` in lib.rs).
if (platform !== "windows") document.documentElement.toggleAttribute("data-native", true);
