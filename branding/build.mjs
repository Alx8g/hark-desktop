// Renders the brand art (installer sidebar and header, social banner) from branding/src/*.html
// using any installed Chrome, Chromium or Edge. No dependencies; needs Node 22 or newer.
//
//   node branding/build.mjs            render everything
//   BROWSER=/path/to/chrome node ...   pick a specific browser
//   ONLY=tray-layers node ...           render only the outputs whose path contains this
//
// The installer images are written as 24-bit BMP, the only format NSIS accepts.
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const JOBS = [
  // The mark first: every other page masks it. `mask` crops the glyph, centres it and writes a 512px white-on-transparent PNG.
  { page: "branding/src/mark.html", w: 1024, h: 1024, scale: 1, out: "branding/mark.png", transparent: true, mask: true },
  { page: "branding/src/appicon.html", w: 1024, h: 1024, scale: 1, out: "src-tauri/icons/source.png", transparent: true },
  // Layers lib.rs stacks at runtime to fill the tray icon with sky while a Handoff plan runs.
  ...["base", "sky", "done", "glyph", "dot"].map((l) => ({ page: `branding/src/tray-layer.html?l=${l}`, w: 64, h: 64, scale: 1, out: `src-tauri/icons/tray-layers/${l}.png`, transparent: true })),
  { page: "branding/src/tray-template.html", w: 44, h: 44, scale: 1, out: "src-tauri/icons/tray-template.png", transparent: true },
  { page: "branding/src/tray-template-dot.html", w: 44, h: 44, scale: 1, out: "src-tauri/icons/tray-template-dot.png", transparent: true },
  { page: "branding/src/sidebar.html", w: 164, h: 314, scale: 4, out: "src-tauri/windows/sidebar.bmp", bmp: true },
  { page: "branding/src/header.html", w: 150, h: 57, scale: 4, out: "src-tauri/windows/header.bmp", bmp: true },
  { page: "branding/src/social.html", w: 1280, h: 640, scale: 1, out: "docs/assets/social.png" },
  { page: "branding/src/dmg.html", w: 660, h: 400, scale: 1, out: "src-tauri/macos/dmg-background.png" },
  { page: "branding/src/favicon.html", w: 64, h: 64, scale: 2, out: "docs/assets/favicon.png", transparent: true },
];

const BROWSERS = [
  process.env.BROWSER,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);

const browser = BROWSERS.find((p) => existsSync(p));
if (!browser) throw new Error("No Chrome, Chromium or Edge found. Set BROWSER=/path/to/browser.");

const TYPES = { ".html": "text/html", ".css": "text/css", ".png": "image/png", ".svg": "image/svg+xml", ".js": "text/javascript" };
const server = createServer((req, res) => {
  const file = normalize(join(root, decodeURIComponent(new URL(req.url, "http://x").pathname)));
  if (!file.startsWith(root) || !existsSync(file)) { res.writeHead(404).end(); return; }
  res.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" }).end(readFileSync(file));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

const profile = mkdtempSync(join(tmpdir(), "hark-brand-"));
const port = 9400 + Math.floor(Math.random() * 400);
const proc = spawn(browser, ["--headless=new", "--disable-gpu", "--hide-scrollbars", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "about:blank"], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 80 && !target; i++) {
  try { target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page"); } catch { /* still starting */ }
  if (!target) await sleep(250);
}
if (!target) throw new Error("Browser did not start.");

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let nextId = 0;
const waiting = new Map();
ws.addEventListener("message", (m) => {
  const d = JSON.parse(m.data);
  if (d.id && waiting.has(d.id)) { waiting.get(d.id)(d); waiting.delete(d.id); }
});
const send = (method, params = {}) => new Promise((res) => { const id = ++nextId; waiting.set(id, res); ws.send(JSON.stringify({ id, method, params })); });

/** 24-bit bottom-up BMP from top-down RGBA pixels. */
function encodeBmp(width, height, rgba) {
  const stride = Math.ceil((width * 3) / 4) * 4;
  const out = Buffer.alloc(54 + stride * height);
  out.write("BM", 0); out.writeUInt32LE(out.length, 2); out.writeUInt32LE(54, 10);
  out.writeUInt32LE(40, 14); out.writeInt32LE(width, 18); out.writeInt32LE(height, 22);
  out.writeUInt16LE(1, 26); out.writeUInt16LE(24, 28); out.writeUInt32LE(stride * height, 34);
  out.writeInt32LE(2835, 38); out.writeInt32LE(2835, 42);
  for (let y = 0; y < height; y++) {
    const row = 54 + (height - 1 - y) * stride;
    for (let x = 0; x < width; x++) {
      const s = (y * width + x) * 4, d = row + x * 3;
      out[d] = rgba[s + 2]; out[d + 1] = rgba[s + 1]; out[d + 2] = rgba[s];
    }
  }
  return out;
}

try {
  for (const job of JOBS) {
    if (process.env.ONLY && !job.out.includes(process.env.ONLY)) continue;
    if (!existsSync(join(root, job.page.split("?")[0]))) { console.log(`skip ${job.page} (not written yet)`); continue; }
    await send("Page.enable");
    await send("Emulation.setDeviceMetricsOverride", { width: job.w, height: job.h, deviceScaleFactor: job.scale, mobile: false });
    await send("Emulation.setDefaultBackgroundColorOverride", job.transparent ? { color: { r: 0, g: 0, b: 0, a: 0 } } : {});
    await send("Page.navigate", { url: `${base}/${job.page}` });
    await sleep(500);
    await send("Runtime.evaluate", { awaitPromise: true, expression: "document.fonts.ready.then(() => new Promise(r => setTimeout(r, 400)))" });
    const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    const png = Buffer.from(shot.result.data, "base64");
    const outFile = join(root, job.out);
    mkdirSync(dirname(outFile), { recursive: true });

    if (job.mask) {
      const cut = await send("Runtime.evaluate", {
        awaitPromise: true,
        returnByValue: true,
        expression: `(async () => {
          const img = new Image(); img.src = "data:image/png;base64,${shot.result.data}"; await img.decode();
          const c = document.createElement("canvas"); c.width = img.width; c.height = img.height;
          const g = c.getContext("2d"); g.drawImage(img, 0, 0);
          const d = g.getImageData(0, 0, c.width, c.height).data;
          let x0 = c.width, y0 = c.height, x1 = 0, y1 = 0;
          for (let y = 0; y < c.height; y++) for (let x = 0; x < c.width; x++) if (d[(y * c.width + x) * 4 + 3] > 8) { if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y; }
          const w = x1 - x0 + 1, h = y1 - y0 + 1, box = 512, fit = box * 0.98, k = Math.min(fit / w, fit / h);
          const o = document.createElement("canvas"); o.width = box; o.height = box;
          const q = o.getContext("2d"); q.imageSmoothingQuality = "high";
          q.drawImage(c, x0, y0, w, h, (box - w * k) / 2, (box - h * k) / 2, w * k, h * k);
          return o.toDataURL("image/png").split(",")[1];
        })()`,
      });
      writeFileSync(outFile, Buffer.from(cut.result.result.value, "base64"));
      console.log(`wrote ${job.out} (512x512 mask)`);
      continue;
    }

    if (!job.bmp) { writeFileSync(outFile, png); console.log(`wrote ${job.out} (${job.w}x${job.h})`); continue; }

    // Step the oversampled render down by halves for clean edges, then read the pixels back.
    const px = await send("Runtime.evaluate", {
      awaitPromise: true,
      returnByValue: true,
      expression: `(async () => {
        const img = new Image(); img.src = "data:image/png;base64,${shot.result.data}"; await img.decode();
        let c = document.createElement("canvas"); c.width = img.width; c.height = img.height;
        let g = c.getContext("2d"); g.drawImage(img, 0, 0);
        while (c.width > ${job.w}) {
          const n = document.createElement("canvas"); n.width = Math.max(${job.w}, c.width / 2); n.height = Math.max(${job.h}, c.height / 2);
          const k = n.getContext("2d"); k.imageSmoothingQuality = "high"; k.drawImage(c, 0, 0, n.width, n.height); c = n;
        }
        const d = c.getContext("2d").getImageData(0, 0, ${job.w}, ${job.h}).data;
        let s = ""; for (let i = 0; i < d.length; i += 8192) s += String.fromCharCode(...d.subarray(i, i + 8192));
        return btoa(s);
      })()`,
    });
    const rgba = Buffer.from(px.result.result.value, "base64");
    writeFileSync(outFile, encodeBmp(job.w, job.h, rgba));
    console.log(`wrote ${job.out} (${job.w}x${job.h} BMP)`);
  }
} finally {
  // The website carries its own copy of the mark.
  try { mkdirSync(join(root, "docs/assets"), { recursive: true }); copyFileSync(join(root, "branding/mark.png"), join(root, "docs/assets/mark.png")); } catch { /* mark not rendered */ }
  ws.close(); proc.kill(); server.close();
  await sleep(300);
  try { rmSync(profile, { recursive: true, force: true }); } catch { /* browser still releasing files */ }
}
