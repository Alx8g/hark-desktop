# Hark Desktop

An unofficial, lightweight desktop app for [Hark](https://hark.com). It is the real Hark in a small native window, with a tray icon, native notifications and a global shortcut, for Windows, macOS and Linux.

Website: [harkdesk.top](https://harkdesk.top/)

Not affiliated with Hark. Your account, data and subscription stay on hark.com. The app never sees your password: you sign in on Hark's own pages, exactly as in a browser.

**Platform status.** Windows is the tested platform and everything below was measured there. The macOS and Linux builds are new: they are built by CI and have not yet been run by the maintainer on real hardware, so expect rough edges and please report them. See [Platforms](#platforms).

## Install

Download the build for your system from the [latest release](../../releases/latest).

| System | File | Notes |
| --- | --- | --- |
| Windows 10 and 11 | `Hark_<version>_x64-setup.exe` | Per-user install, no admin rights needed. Tested on Windows 11; Windows 10 needs the WebView2 runtime, which the installer fetches if it is missing. The `.msi` installs for all users and needs admin rights. |
| macOS 11 or newer | `.dmg` (Apple silicon or Intel) | Not signed. Open it with a right click, then Open, the first time. |
| Linux | `.deb` or `.rpm` (about 3 MB), or `.AppImage` (about 78 MB) | The deb and rpm use your system's WebKitGTK 4.1. The AppImage carries its own, which is why it is larger. A tray icon needs AppIndicator support. |

The builds are unsigned, so Windows SmartScreen warns on the first run: choose More info, then Run anyway.

First launch opens hark.com so you can sign in. After that you stay signed in, and closing the window hides Hark instead of quitting, like a messaging app.

## What you get

- **The real Hark.** The app loads hark.com, so Chat, Projects, Panels, Action Buttons and Handoff arrive the day Hark ships them.
- **Notifications for Hark's replies** while the window is hidden or unfocused. Your own messages never notify.
- **Handoff plans at the top of the screen.** While a plan runs and Hark isn't in front, a small island at the top centre shows its name and its steps. Between steps it tucks into the screen edge as a thin line of sky; point at it and it opens up, with Open and Hide. When the plan finishes it turns to Hark's sky with a tick until you open it. The taskbar or Dock icon shows the progress too. On Linux, where see-through windows aren't dependable, the tray icon and a notification do this job.
- **Tray or menu bar icon** with an unread dot and a grouped menu: Open Hark, Ask Hark, the running plan, Today's brief, Projects, Settings, Quit. On Windows and Linux, Hark's sky rises inside the icon as a plan's steps finish.
- **Quick ask from anywhere.** `Ctrl+Shift+H` (`Cmd+Shift+H` on macOS) opens a little piece of Hark's sky with a message bar, near the top of the screen you're on. Enter sends it to Hark and brings Hark forward on the reply. To check it first, turn off Send from quick ask in Settings and Enter drafts it in Hark instead. Esc or a click elsewhere closes it, and what you typed is still there next time.
- **Opens where you left it.** While hark.com loads, the window shows Hark's own sky brightening from dawn, then Hark fades in. If loading is slow or you're offline, it says so and offers Try again.
- **Looks like your Hark.** The app's own screens follow Hark's light or dark theme and use the sky Hark is showing, which changes with the time of day. The plan island stays out of the way while something runs full screen.
- **Starts with your session, hidden,** so your morning brief can find you. On Windows this is on by default; on macOS and Linux it is opt-in. Every behaviour is switchable in Desktop settings (`Ctrl+,`).
- **Sign-in screen that is only the sign-in.** Hark's signed-out page is a long marketing page. The desktop app shows just the sign-in, with no scrolling.
- **Links stay sensible.** Hark and the Google, Apple, Stripe and Cloudflare pages its sign-in and payments use open in the app. Everything else opens in your default browser. Downloads go to your Downloads folder with a notification and never overwrite a file; programs and installers open in your browser instead, so its download checks apply.

## Shortcuts

| Keys | Does |
| --- | --- |
| `Ctrl+Shift+H` (`Cmd+Shift+H`), global | Quick ask from anywhere. If Hark is focused, jumps to the message box. |
| `Ctrl+,` (`Cmd+,`) | Desktop settings |
| `Ctrl+R` (`Cmd+R`) | Reload Hark |
| `Ctrl +`, `-`, `0` | Zoom, remembered |
| `F11` | Full screen (Windows and Linux) |

## Platforms

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Status | Tested | Built by CI, not yet run by the maintainer | Built by CI, not yet run by the maintainer |
| Web view | WebView2 | WKWebView | WebKitGTK |
| Main window | Frameless, with Hark-style window controls and rounded corners drawn by the page | Native window with a transparent title bar | The window manager's frame |
| Closing the window | Hides to the tray | Hides to the menu bar | Quits (many Linux desktops have no tray) |
| Start at login | On by default | Off | Off |

If something differs from this table on your machine, please [open an issue](../../issues/new/choose) and say which desktop you use.

## How fast is it

Measured on Windows 11 (Intel i9-12900H, 32 GB, WebView2 155, signed-in account, release build). One run on one machine, so read these as orders of magnitude.

| | |
| --- | --- |
| Installer size | about 2.3 MB (a typical Electron wrapper is 80 to 200 MB) |
| Loading screen visible | about 0.6 s after launch |
| Hark shown | median 3.1 s: as soon as Hark's own interface has painted, without waiting for the rest of the page |
| Visible and idle | about 15 to 25% of one core. hark.com draws its animated sky continuously while it is on screen. |
| Hidden in the tray, or minimized | under 1% of one core: the page stops drawing until you come back, and stays connected |
| Plan island showing | under 1% of one core |
| Memory | 350 to 450 MB private: the Rust shell is about 13 MB and the rest is WebView2's GPU and renderer processes, about what Edge spends on this page. No second Chromium is bundled. |
| Resizing | about 170% of one core, only while you drag. The page catches up about 75 ms after the window. |
| Quick ask after `Ctrl+Shift+H` | median 16 ms |
| Main window back from the tray | median 47 ms |

## Security

hark.com pages get the smallest capability Tauri can give: they may emit and listen to events, and nothing else. They cannot call commands, read files or open URLs themselves. The Rust side validates every event and does the native work. Navigation to any host outside the allow-list is handed to your default browser. See [SECURITY.md](SECURITY.md) to report a vulnerability.

Settings are stored in the app data folder (on Windows, `%APPDATA%\com.harkwindows.app\hark-settings.json`). The web view profile with your Hark session lives beside them.

## Build from source

You need Node 22 or newer with [pnpm](https://pnpm.io), a stable Rust toolchain, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your system (on Windows that is the WebView2 runtime and the MSVC build tools).

```
pnpm install
pnpm tauri dev      # live-reloading development build
pnpm tauri build    # release build and installers
pnpm build          # type-check and build the local pages only
```

Release builds use `lto = true` and `codegen-units = 1` to keep the binary small, so `pnpm tauri build` takes several minutes. Use `pnpm tauri dev` while you work.

[CONTRIBUTING.md](CONTRIBUTING.md) covers the code layout, the rules for the window frame, and how releases are cut.

## Known limits

- No native drop shadow on the frameless Windows window. A transparent window cannot carry the system's rectangular shadow cleanly.
- While you drag a window edge on a heavy page, the strip the page has not repainted yet is briefly see-through.
- Google and Apple sign-in run as an in-window redirect rather than a popup, which matches Hark's web flow.
- Notifications depend on Hark's current page structure. The app only notifies for a message it positively recognises as Hark's. If Hark restyles its chat, notifications can go quiet, but your own messages will not be announced.
- There is no auto-updater yet. Download a new release to update.
- Builds are not code-signed or notarised.

## Licence

[MIT](LICENSE). Hark and the Hark logo belong to their owners. This project is not endorsed by or affiliated with Hark.
