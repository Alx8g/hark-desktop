# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private reporting instead: open the Security tab of this repository and choose "Report a vulnerability". Say what you found, how to reproduce it, and which system you tested on. You will get a first reply within a week.

## What counts

The app loads hark.com in a web view and adds a small native shell around it. These are in scope:

- A way for a web page to do native work it was not given: call a Tauri command, read or write files, open an arbitrary URL, or start a process.
- A way to make the app navigate inside the app to a host outside the allow-list, or to load local app pages from a remote origin.
- A way to read another app's data, or the stored session, through this app.
- Anything in the build or release workflow that could ship a tampered binary.

These are out of scope: bugs in hark.com itself (report those to Hark), and problems that need an attacker who already controls your computer.

## How the app limits what hark.com can do

- hark.com pages may only emit and listen to Tauri events (`src-tauri/capabilities/remote.json`). They cannot call commands.
- The Rust side validates every event and performs the native action itself.
- Navigation outside a short allow-list of hosts (Hark, Google and Apple sign-in, Cloudflare, Stripe) opens in your default browser.
- The app never sees your password. Sign-in happens on Hark's own pages.

## Supported versions

Only the latest release is supported.
