# Contributing

Thanks for helping. Small, focused pull requests are easiest to review.

## Set up

You need Node 22 or newer with [pnpm](https://pnpm.io), a stable Rust toolchain, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your system.

```
pnpm install
pnpm tauri dev
```

Before you open a pull request, run the same checks CI runs:

```
pnpm typecheck
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Use [Conventional Commits](https://www.conventionalcommits.org) for commit messages and pull request titles: `fix:` for behaviour that already existed, `feat:` only for a new user-facing capability, and `!` or a `BREAKING CHANGE:` footer only for a real compatibility break.

## Where things live

| Path | What it is |
| --- | --- |
| `src-tauri/src/lib.rs` | The Rust shell: windows, tray, notifications, autostart, shortcuts, downloads, navigation rules |
| `src-tauri/src/bridge.js` | Script injected into every hark.com page. It only emits events; Rust does the native work |
| `src-tauri/capabilities/` | What local pages and hark.com pages are allowed to call |
| `src/`, `*.html` | The local pages (splash, settings, quick ask, offline), built with Vite |
| `src-tauri/windows/` | Windows installer wording and art |
| `branding/` | Brand sources. `node branding/build.mjs` renders the installer art and the social banner |
| `docs/` | The project website at harkdesk.top. Cloudflare Pages publishes this folder on every push to `main`; there is no build step. `_headers` and `_redirects` configure it. |

## Rules that save you a day

**Keep hark.com pages on events only.** `src-tauri/capabilities/remote.json` lets hark.com emit and listen to events and nothing else. Do not widen it. If the page needs something native, add an event in `bridge.js` and have Rust validate it and do the work.

**Use stable hooks in `bridge.js`.** Select by `data-testid` and `aria-label`, never by hashed class names, which change on every Hark deploy.

**Platform code stays behind a platform check.** Windows is the reference platform. In Rust use `cfg!` or `#[cfg]`; in `bridge.js` use `isWindows()` and `isMac()`, which read `window.__HARK_PLATFORM__`; in the local pages use `data-platform` on `<html>`. A change that helps one system must not alter the others.

### The Windows window frame

Frameless Windows windows are built with `frameless()` in `lib.rs`: decorations off, transparent, no native shadow. The rounded 20px corners are drawn by the page (`html` `clip-path` in `src/shell.css` for local pages, injected by `bridge.js` for hark.com), so the compositor anti-aliases them. Several things look like shortcuts and are not:

- **Do not round the top-level window with `SetWindowRgn`.** A region is a 1-bit mask, so the corners are jagged. It also makes Windows paint its old non-client frame (pale edges, old caption buttons flashing while you resize) and it cuts away the corner boxes of Tauri's resize grips, so corner dragging stops working.
- **Do not use `shadow(true)` on a frameless window.** tao then insets the client area by the invisible resize frame and lets DWM draw a 1px border and its own 8px corner mask into it.
- **Do not switch on Windows' own corner rounding**, not even during a drag. It is only 8px, so the corner visibly jumps from 20px to 8px, and it brings back the system shadow and a bright sliver in each corner.

There is one deliberate region, in `lib.rs` (`corners` and `hold_corners`). It is a rounded region on the web view's own child window (`WRY_WEBVIEW`), not the top-level window, so the resize grips (a sibling window) keep working. Its corner is 17px, looser than the page's 20px, so the page's anti-aliased curve always sits inside it and the region is invisible at rest. It exists because the page lags the window by a frame or two while you drag an edge: when shrinking, the window edge crops the page's previous, wider frame and its rounded corners would vanish. It is applied straight from the `Resized` event with plain Win32 calls and removed when the window is maximized or full screen.

If you change the corner radius, change it in three places: `--window-radius` in `src/shell.css`, `frameCss()` in `bridge.js`, and `RADIUS` in `lib.rs` (keep the Rust value a few pixels smaller).

On macOS the main window keeps its native frame with a transparent title bar, and on Linux it keeps the window manager's frame. Neither uses the page-drawn corners.

### Notifications

`classifyBubble()` in `bridge.js` decides whether a chat bubble is yours or Hark's, from the bubble fill's opacity and whether it holds rendered markdown. It only notifies for a bubble it positively recognises as Hark's. Do not match exact theme colours: light and dark mode use different ones, and matching one set once made your own messages pop up as notifications.

## Brand art and the installer

Run `node branding/build.mjs` after changing anything in `branding/src/`. It needs Chrome, Chromium or Edge. It cuts the mark (a serif "h") out of `branding/src/mark.html`, then renders the app icon master, the tray icons, the installer images in `src-tauri/windows/`, the disk image background, and the favicon and social banner in `docs/assets/`. The installer wording is in `src-tauri/windows/hooks.nsh`.

If the mark or the app icon changes, regenerate the per-platform icon files from the master afterwards:

```
pnpm tauri icon src-tauri/icons/source.png
```

That command also writes Android and iOS folders, which this desktop app does not use; delete `src-tauri/icons/android` and `src-tauri/icons/ios`.

To try an installer without touching a real install, build it under a different identifier:

```
pnpm tauri build --bundles nsis --config '{"identifier":"com.example.installtest","productName":"HarkInstallTest"}'
```

## Releases

Maintainers push a tag such as `v0.2.0`. The release workflow builds Windows, macOS (Apple silicon and Intel) and Linux packages and attaches them to a draft GitHub release. Bump `version` in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` first; they must match.

Installed copies update themselves from the latest published release. The workflow signs the update packages with the `TAURI_SIGNING_PRIVATE_KEY` repository secret and attaches `latest.json`; the matching public key is `plugins.updater.pubkey` in `tauri.conf.json`. Update packages are only made in the release workflow (`--config src-tauri/tauri.updater.conf.json`), so local builds need no key. A draft release is not seen by the app until it is published as the latest release. If the key is ever replaced, copies already installed can no longer update and need a manual install once.

## Testing on macOS and Linux

The maintainers develop on Windows. If you run Hark Desktop on macOS or Linux, reports and fixes are especially welcome: say which system, desktop environment and version you use.
