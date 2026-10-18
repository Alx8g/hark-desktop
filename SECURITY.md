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

- hark.com may only emit and listen to Tauri events (`src-tauri/capabilities/remote.json`, `https://hark.com` only). It cannot call the app's commands or any plugin. The Google, Apple and Stripe pages Hark sends you to may only emit events, so the window controls drawn on them work (`sign-in.json`). The app's own windows are each granted only the commands they use (`src-tauri/capabilities/*.json`, `src-tauri/build.rs`).
- The page script runs in Hark's top-level page only, never in frames inside it.
- The Rust side validates every event before acting on it. Text from the page is cleaned and capped before it is shown in the tray, the plan island or a notification, and notifications are rate-limited.
- Links handed to the operating system are limited to web pages, email and phone numbers. Other schemes, local files and other apps' links are dropped.
- Only Hark and the exact sign-in, payment and challenge pages its flows use open inside the app, over https. Everything else opens in your default browser.
- Downloads keep the name the site suggests, are made safe for your file system, and never overwrite a file. Files that can run code (programs, installers, scripts, shortcuts, disk images) open in your browser instead, where its download checks apply.
- The app's own pages run under a content security policy, and the app never sees your password: sign-in happens on Hark's own pages.

## Supported versions

Only the latest release is supported.
