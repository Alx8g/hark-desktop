/* Hark Desktop: page bridge.
 * Injected by the Rust shell into every hark.com page (Tauri initialization
 * script). It makes the real Hark web app behave like a native desktop app:
 * native notifications, tray unread counts, Handoff plan progress (plan
 * island, tray icon, taskbar), external-link handling, zoom and offline handling. On Windows,
 * where the window is frameless, it also draws caption controls in Hark's own
 * frosted-pill language and the rounded window silhouette; macOS and Linux
 * keep the operating system's own window frame.
 * window.__HARK_PLATFORM__ ("windows" | "macos" | "linux") is set by the shell
 * just before this script runs.
 * Selectors use Hark's stable data-testid / aria-label hooks only
 * (chat-input, message-bubble, message-list) — never hashed class names.
 * All native work is done in Rust; this script only emits Tauri events. */
(function () {
  "use strict";
  /* Hark's own page only: never the blank frames inside it, which have no
   * theme, plans or messages and would report misleading state. */
  if (window.top !== window.self) return;
  if (window.__HARK_DESKTOP__) return;
  window.__HARK_DESKTOP__ = true;

  /* Needs nothing from Tauri or the DOM, so start before either is ready. */
  installFrame();

  /* Mirrors IN_APP_HOSTS in lib.rs, which is what actually enforces it. */
  var ALLOWED_HOSTS = [
    "hark.com", "www.hark.com", "accounts.google.com", "appleid.apple.com",
    "checkout.stripe.com", "billing.stripe.com", "hooks.stripe.com", "challenges.cloudflare.com"
  ];

  function hostAllowed(host) {
    return ALLOWED_HOSTS.indexOf(host) >= 0 || host.slice(-9) === ".hark.com";
  }

  function whenTauri(fn) {
    var tries = 0;
    (function poll() {
      if (window.__TAURI__ && window.__TAURI__.event) return fn(window.__TAURI__);
      if (++tries > 200) return; /* not running inside the desktop shell */
      setTimeout(poll, 50);
    })();
  }

  /* Hark is usable long before its "load" event (a slow resource can hold that
   * back for many seconds), so wait for its interface instead: the message box,
   * the message list or the sign-in field, then two frames so it has painted.
   * Gives up waiting after 3 seconds. */
  function whenPainted(fn) {
    var started = Date.now();
    (function look() {
      var ui = document.querySelector('[data-testid="chat-input"], [data-testid="message-list"], ' + LOGIN_FIELD);
      if (ui || Date.now() - started > 3000) {
        /* A hidden window may not draw frames at all: don't wait on them forever. */
        var done = false;
        var go = function () { if (!done) { done = true; fn(); } };
        requestAnimationFrame(function () { requestAnimationFrame(go); });
        setTimeout(go, 150);
        return;
      }
      setTimeout(look, 80);
    })();
  }

  function onReady(fn) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", fn);
    else fn();
  }

  whenTauri(function (T) {
    var emit = function (name, payload) {
      try {
        var sent = T.event.emit(name, payload || {});
        if (sent && sent.catch) sent.catch(function () { /* not allowed on this page */ });
      } catch (e) { /* shell gone */ }
    };
    onReady(function () { init(T, emit); });
  });

  /* A function, not a var: installFrame() runs before the first var is assigned. */
  function isWindows() { return (window.__HARK_PLATFORM__ || "windows") === "windows"; }
  function isMac() { return window.__HARK_PLATFORM__ === "macos"; }

  function init(T, emit) {
    installStyles();
    if (isWindows()) { installCaption(emit); installDrag(emit); }
    installNotifications(emit);
    installAppearance(emit);
    installMessages(emit);
    installProgress(emit);
    installLinks(emit);
    installKeys(emit);
    installOffline();
    installShellEvents(T, emit);
    emit("hark-ready", { href: location.href });
    /* The loading screen waits for this: Hark's own interface is on screen. */
    whenPainted(function () { emit("hark-painted", { href: location.href }); });
    /* React re-renders can drop our fixed nodes; keep them present. */
    setInterval(function () {
      if (isWindows() && !document.getElementById("hark-caption")) installCaption(emit);
      if (!document.getElementById("hark-desktop-css")) installStyles();
      putFrameStyle();
      queuePageSync();
    }, 2500);
  }

  /* ---------- rounded window silhouette ----------
   * The OS window is a transparent rectangle (see `frameless` in lib.rs), so
   * the page clips itself to a 20px rounded rect and the compositor
   * anti-aliases the corners. Never clip the window with SetWindowRgn: it is
   * a 1-bit mask and makes Windows paint its legacy frame. Radius is divided
   * by the page zoom so it stays 20px on screen at any Ctrl +/- level. */
  /* A function, not a var: installFrame() runs before this point is reached.
   * The clip is <html>'s own box, so that box must always be exactly the
   * window: never shorter (min-height) and never scrolling away with the
   * document (data-scrolls moves the scrolling onto <body>). */
  /* Hark's sky placeholder is a small inline image in its stylesheet. Only that
   * shape is ever stored or passed on. A function, not a var: it runs from
   * installFrame() before the rest of this script has been read. */
  function skyUrl(value) {
    /* A bare data URL (as stored), or url(...) with any background values after it. */
    var m = String(value || "").match(/^\s*(?:url\(\s*["']?)?(data:image\/(?:webp|png|jpeg);base64,[A-Za-z0-9+\/=]+)/);
    return m && m[1].length < 60000 ? m[1] : "";
  }
  function storedSky() {
    try { return skyUrl(localStorage.getItem("harkDesktopSky")); } catch (e) { return ""; }
  }

  function frameCss() {
    var rules = [];
    if (isWindows()) {   /* only the frameless Windows window needs a page-drawn silhouette */
      rules.push(
        ":root{--hark-zoom:1;--window-radius:calc(20px / var(--hark-zoom))}",
        "html{min-height:100%!important;clip-path:inset(0 round var(--window-radius))!important}",
        "html[data-square]{clip-path:none!important}",
        "html::after{content:'';position:fixed;inset:0;z-index:2147483647;pointer-events:none;border-radius:var(--window-radius);box-shadow:inset 0 0 0 1px rgba(29,29,31,.16)}",
        "html[data-theme=dark]::after{box-shadow:inset 0 0 0 1px rgba(245,245,247,.14)}",
        "html[data-square]::after{display:none}",
        "html[data-scrolls]{height:100%!important;overflow:hidden!important}",
        "html[data-scrolls] body{height:100%!important;overflow-x:hidden!important;overflow-y:auto!important}"
      );
    }
    /* Before Hark paints (and on every reload), show the sky Hark showed last
     * time instead of a foreign colour. Falls back to Hark's light sky. */
    var sky = storedSky();
    rules.push(sky
      ? "html{background:#1d2a44 url(\"" + sky + "\") center/cover no-repeat}"
      : "html{background:linear-gradient(168deg,#7fb0dd 0%,#b6cfe7 45%,#d9e9f6 70%,#ffeed9 120%)}html[data-theme=dark]{background:#161f39}");
    rules.push(
      /* Signed-out screen is just the hero: no scrolling, so no scrollbar either. */
      "html[data-hark-login],html[data-hark-login] body{height:100%!important;overflow:hidden!important}",
      "[data-hark-hide]{display:none!important}"
    );
    return rules.join("\n");
  }

  function putFrameStyle() {
    var root = document.documentElement;
    if (!root) return false;
    if (!document.getElementById("hark-frame-css")) {
      var style = document.createElement("style");
      style.id = "hark-frame-css";
      style.textContent = frameCss();
      root.appendChild(style); /* on <html>, so a re-rendered <head> cannot drop it */
    }
    return true;
  }

  /* A maximized or full-screen window sits flush against the monitor, so the
   * rounding is dropped whenever the window fills the screen it is on. */
  function syncSquare() {
    var root = document.documentElement;
    if (!root || !isWindows()) return;
    var s = window.screen;
    function fills(w, h) { return window.outerWidth >= w - 2 && window.outerHeight >= h - 2; }
    root.toggleAttribute("data-square", fills(s.availWidth, s.availHeight) || fills(s.width, s.height));
  }

  /* The signed-out screen is Hark's whole marketing site stacked under the
   * sign-in hero. People who see it already have an account, so the app shows
   * only the hero and does not scroll. Recognised by its sign-in field
   * (aria-label, not a hashed class), on "/" only. */
  var LOGIN_FIELD = 'input[aria-label="Email address or phone number"]';

  function syncPage() {
    var root = document.documentElement, body = document.body;
    if (!root || !body) return;
    var login = location.pathname === "/" && !!document.querySelector(LOGIN_FIELD);
    root.toggleAttribute("data-hark-login", login);
    var hidden = document.querySelectorAll("[data-hark-hide]");
    if (login) {
      /* "Learn more" only scrolls down to the marketing page, which is locked off. */
      var buttons = body.querySelectorAll("button");
      for (var i = 0; i < buttons.length; i++) {
        if ((buttons[i].textContent || "").trim() === "Learn more") buttons[i].setAttribute("data-hark-hide", "");
      }
      if (window.scrollY) window.scrollTo(0, 0);
    } else {
      for (var j = 0; j < hidden.length; j++) hidden[j].removeAttribute("data-hark-hide");
    }
    /* Any other page whose document scrolls (terms, Stripe checkout...) scrolls
     * <body> instead, keeping <html> (and so the rounded clip) fixed to the window. */
    var height = root.hasAttribute("data-scrolls") ? body.scrollHeight : root.scrollHeight;
    root.toggleAttribute("data-scrolls", isWindows() && !login && height > window.innerHeight + 1);
  }

  var pageSyncQueued = false;
  function queuePageSync() {
    if (pageSyncQueued) return;
    pageSyncQueued = true;
    requestAnimationFrame(function () { pageSyncQueued = false; syncPage(); });
  }

  function installFrame() {
    function ready() {
      putFrameStyle();
      syncSquare();
      new MutationObserver(queuePageSync).observe(document.documentElement, { childList: true, subtree: true });
      queuePageSync();
    }
    window.addEventListener("resize", function () { syncSquare(); queuePageSync(); });
    window.addEventListener("scroll", function () {
      /* keyboard focus or a script must not push the locked hero out of place */
      if (document.documentElement.hasAttribute("data-hark-login") && window.scrollY) window.scrollTo(0, 0);
    }, { passive: true });
    document.addEventListener("DOMContentLoaded", function () { syncSquare(); queuePageSync(); });
    if (document.documentElement) {
      ready();
    } else {
      /* document-start: <html> does not exist yet */
      var waiting = new MutationObserver(function () {
        if (document.documentElement) { waiting.disconnect(); ready(); }
      });
      waiting.observe(document, { childList: true });
    }
  }

  /* ---------- theme polish (Hark's own tokens, sampled live) ---------- */
  function installStyles() {
    if (document.getElementById("hark-desktop-css")) return;
    var css = [
      "[data-app-download-banner]{display:none!important}",
      "::-webkit-scrollbar{width:10px;height:10px}",
      "::-webkit-scrollbar-thumb{background:color-mix(in srgb,var(--text-primary,#1d1d1f) 22%,transparent);border-radius:999px;border:2px solid transparent;background-clip:content-box}",
      "::-webkit-scrollbar-thumb:hover{background:color-mix(in srgb,var(--text-primary,#1d1d1f) 38%,transparent);border:2px solid transparent;background-clip:content-box}",
      "::-webkit-scrollbar-track{background:transparent}",
      "::selection{background:rgba(127,176,221,.45)}",
      "#hark-offline{position:fixed;left:50%;bottom:28px;transform:translateX(-50%);z-index:2147483646;background:var(--regular-solid-bg,rgba(255,255,255,.98));color:var(--text-primary,#1d1d1f);border-radius:18px;padding:14px 20px;font:500 14px 'GT Standard L','Segoe UI',sans-serif;box-shadow:0 12px 40px rgba(29,29,31,.18);display:none;align-items:center;gap:12px}",
      "#hark-offline button{appearance:none;border:0;cursor:pointer;background:var(--fill-primary,#1d1d1f);color:var(--text-primary-on,#f9f9fa);font:inherit;font-size:13px;font-weight:500;border-radius:9999px;padding:8px 14px}"
    ];
    if (isWindows()) {
      /* The window controls live in the page on Windows only; Search and Settings
       * slide left to make room. caption pill matches Hark's Search pill exactly:
       * 48px tall at top:14px, right inset 16px. */
      css.push(
        "@media (min-width:760px){",
        'button[aria-label="Settings"],button[aria-label="Search the page and your mail"]{translate:-140px 0}',
        "}",
        "#hark-caption{position:fixed;top:14px;right:16px;z-index:2147483646;display:flex;gap:2px;padding:4px;border-radius:9999px;background:var(--thin-bg,rgba(255,255,255,.55));backdrop-filter:blur(16px) saturate(1.4);-webkit-backdrop-filter:blur(16px) saturate(1.4);box-shadow:inset 0 0 0 1px var(--stroke-secondary,rgba(29,29,31,.08)),0 4px 18px rgba(0,0,0,.08);font-family:'GT Standard L','Segoe UI Variable Text','Segoe UI',sans-serif}",
        "#hark-caption button{appearance:none;border:0;background:transparent;width:40px;height:40px;border-radius:9999px;display:grid;place-items:center;color:var(--text-primary,#1d1d1f);cursor:pointer;padding:0}",
        "#hark-caption button:hover{background:color-mix(in srgb,currentColor 9%,transparent)}",
        "#hark-caption button.hark-close:hover{background:#e81123;color:#fff}"
      );
    }
    css = css.join("\n");
    var style = document.createElement("style");
    style.id = "hark-desktop-css";
    style.textContent = css;
    (document.head || document.documentElement).appendChild(style);
    /* Restore saved zoom early to avoid a visible jump. */
    var zoom = parseFloat(localStorage.getItem("harkDesktopZoom") || "1");
    if (zoom && zoom !== 1) applyZoom(zoom);
  }

  /* ---------- frameless caption pill (sized to Hark's Search pill) ---------- */
  function svgIcon(paths) {
    return '<svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden="true">' + paths + "</svg>";
  }
  function installCaption(emit) {
    if (!document.body || document.getElementById("hark-caption")) return;
    var pill = document.createElement("div");
    pill.id = "hark-caption";
    pill.setAttribute("role", "group");
    pill.setAttribute("aria-label", "Window controls");
    pill.innerHTML =
      '<button data-act="minimize" title="Minimize" aria-label="Minimize window">' + svgIcon('<path d="M2 8h12" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>') + "</button>" +
      '<button data-act="maximize" title="Maximize or restore" aria-label="Maximize or restore window">' + svgIcon('<rect x="2.6" y="2.6" width="10.8" height="10.8" rx="2.4" stroke="currentColor" stroke-width="1.5"/>') + "</button>" +
      '<button data-act="close" class="hark-close" title="Close" aria-label="Close window">' + svgIcon('<path d="M3 3l10 10M13 3L3 13" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>') + "</button>";
    pill.addEventListener("click", function (e) {
      var btn = e.target.closest("button[data-act]");
      if (btn) emit("hark-window", { action: btn.getAttribute("data-act") });
    });
    document.body.appendChild(pill);
  }

  /* ---------- drag by the whole top strip, like a real titlebar ----------
   * Any mousedown in the top 60px that does not land on an interactive
   * control (Chat/Projects, Search, avatar, Home, composer, links, the
   * scrollbar) moves the window; double-click there toggles maximize. */
  function installDrag(emit) {
    var INTERACTIVE = 'button, a, input, textarea, select, [contenteditable], [role="button"], [role="textbox"], [role="menuitem"]';
    function inDragArea(e) {
      if (e.clientY < 0 || e.clientY > 60) return false;
      if (e.clientX < 0 || e.clientX > window.innerWidth - 16) return false; /* scrollbar */
      var t = e.target;
      if (!(t instanceof Element)) return false;
      if (t.closest("#hark-caption")) return !t.closest("button");
      if (t.closest(INTERACTIVE)) return false;
      /* Something Hark made clickable without saying so (a div with a click handler). */
      if (getComputedStyle(t).cursor === "pointer") return false;
      return true;
    }
    document.addEventListener("mousedown", function (e) {
      if (e.button === 0 && inDragArea(e)) emit("hark-window", { action: "drag" });
    }, true);
    document.addEventListener("dblclick", function (e) {
      if (inDragArea(e)) emit("hark-window", { action: "maximize" });
    }, true);
  }

  /* ---------- Hark's theme and sky -> the shell's own windows ----------
   * The loading screen, quick ask and settings follow Hark's light or dark
   * theme and use the sky image Hark is showing (a time-of-day placeholder in
   * its stylesheet). The sky is also kept here for the next reload. */
  function installAppearance(emit) {
    var last = "";
    function report() {
      var root = document.documentElement;
      var theme = root.getAttribute("data-theme") === "dark" ? "dark" : "light";
      var sky = skyUrl(getComputedStyle(root).getPropertyValue("--sky-placeholder"));
      if (sky) { try { localStorage.setItem("harkDesktopSky", sky); } catch (e) { /* storage full or blocked */ } }
      var key = theme + "|" + sky;
      if (key === last) return;
      last = key;
      emit("hark-appearance", { theme: theme, sky: sky || null });
    }
    whenPainted(report);
    new MutationObserver(report).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
    setInterval(report, 10 * 60 * 1000); /* the sky follows the time of day */
  }

  /* ---------- web Notification API -> native notifications ---------- */
  function installNotifications(emit) {
    function NativeNotification(title, options) {
      var body = options && options.body ? String(options.body) : "";
      emit("hark-notify", { title: String(title || "Hark"), body: body });
      this.title = String(title || "");
      this.body = body;
      this.onclick = this.onclose = this.onerror = this.onshow = null;
    }
    NativeNotification.permission = "granted";
    NativeNotification.maxActions = 0;
    NativeNotification.requestPermission = function (cb) {
      var p = Promise.resolve("granted");
      if (typeof cb === "function") p.then(cb);
      return p;
    };
    /* The parts of the real API a page may call: nothing to do for a native one. */
    NativeNotification.prototype.close = function () {};
    NativeNotification.prototype.addEventListener = function () {};
    NativeNotification.prototype.removeEventListener = function () {};
    NativeNotification.prototype.dispatchEvent = function () { return true; };
    try { window.Notification = NativeNotification; } catch (e) { /* sealed */ }
    if (window.ServiceWorkerRegistration && ServiceWorkerRegistration.prototype.showNotification) {
      ServiceWorkerRegistration.prototype.showNotification = function (title, options) {
        emit("hark-notify", { title: String(title || "Hark"), body: options && options.body ? String(options.body) : "" });
        return Promise.resolve();
      };
    }
  }

  /* ---------- new agent messages -> toast + tray unread ----------
   * Who sent a bubble is read from HOW Hark draws it, not from exact theme colours. (Matching exact colours broke
   * silently once: light mode fills bubbles with rgba(255,255,255,a) but dark mode with rgba(29,29,31,a), so nothing
   * matched and the user's own messages were toasted as if Hark had written them.)
   *   - the bubble's fill ([data-continuous-fill]) is more transparent for the user's own messages (alpha ~0.3) than
   *     for Hark's replies (~0.7 dark, ~0.98 light);
   *   - Hark's replies render as markdown blocks, the user's as a single plain text span.
   * Returns "user" | "agent" | "unknown". Only a bubble positively recognised as "agent" ever raises a toast, so a
   * future restyle can at worst silence toasts, never toast the user's own words. */
  function bubbleFillAlpha(bubble) {
    var fill = bubble.querySelector("[data-continuous-fill]");
    if (!fill) return null;
    var m = getComputedStyle(fill).backgroundColor.match(/^rgba?\(([^)]+)\)$/);
    if (!m) return null;
    var parts = m[1].split(",");
    var a = parts.length > 3 ? parseFloat(parts[3]) : 1;
    return isNaN(a) ? null : a;
  }
  function classifyBubble(bubble, lastSent) {
    var text = (bubble.innerText || "").trim().replace(/\s+/g, " ");
    if (lastSent && text && text.indexOf(lastSent.slice(0, 60)) === 0) return "user"; /* typed in this window */
    var alpha = bubbleFillAlpha(bubble);
    if (alpha !== null) {
      if (alpha <= 0.45) return "user";
      if (alpha >= 0.6) return "agent";
    }
    if (bubble.querySelector('[class*="space-y-"], p, ul, ol, pre, blockquote, table, h1, h2, h3')) return "agent";
    return "unknown";
  }

  function installMessages(emit) {
    var seen = new Set();
    var unread = 0;
    var lastSentText = "";
    var pendingToasts = [];
    var toastTimer = null;

    function seed() {
      document.querySelectorAll('[data-testid="message-bubble"][data-bubble-id]').forEach(function (b) {
        seen.add(b.getAttribute("data-bubble-id"));
      });
    }
    seed();

    /* Remember what was in the composer when a send happened (Enter, the send button, or a form submit). */
    function rememberDraft() {
      var el = document.querySelector('[data-testid="chat-input"]');
      var v = el ? (el.value || "").trim().replace(/\s+/g, " ") : "";
      if (v) lastSentText = v;
    }
    document.addEventListener("keydown", function (e) {
      var el = document.querySelector('[data-testid="chat-input"]');
      if (e.key === "Enter" && !e.isComposing && el && document.activeElement === el) rememberDraft();
    }, true);
    document.addEventListener("click", function (e) {
      if (e.target && e.target.closest && e.target.closest("button")) rememberDraft();
    }, true);
    document.addEventListener("submit", rememberDraft, true);

    function setUnread(n) {
      if (n === unread) return;
      unread = n;
      emit("hark-unread", { count: unread });
    }
    function flushToasts() {
      toastTimer = null;
      if (!pendingToasts.length) return;
      var first = pendingToasts[0];
      emit("hark-notify", {
        title: "Hark",
        body: pendingToasts.length === 1 ? first : first + "  ·  +" + (pendingToasts.length - 1) + " more"
      });
      pendingToasts = [];
    }
    function onNewBubble(bubble) {
      var id = bubble.getAttribute("data-bubble-id");
      if (!id || seen.has(id)) return;
      seen.add(id);
      if (seen.size > 600) { /* keep memory bounded on very long threads */
        var keep = Array.from(seen).slice(-300);
        seen = new Set(keep);
      }
      setTimeout(function () {
        if (classifyBubble(bubble, lastSentText) !== "agent") return; /* yours, or not sure: stay quiet */
        var text = (bubble.innerText || "").trim().replace(/\s+/g, " ");
        if (!text) return;
        if (document.hidden || !document.hasFocus()) {
          setUnread(unread + 1);
          pendingToasts.push(text.slice(0, 140));
          if (!toastTimer) toastTimer = setTimeout(flushToasts, 2500);
        }
      }, 350);
    }

    var observer = new MutationObserver(function (mutations) {
      for (var m = 0; m < mutations.length; m++) {
        mutations[m].addedNodes.forEach(function (node) {
          if (!(node instanceof Element)) return;
          if (node.matches && node.matches('[data-testid="message-bubble"]')) onNewBubble(node);
          if (node.querySelectorAll) {
            node.querySelectorAll('[data-testid="message-bubble"]').forEach(onNewBubble);
          }
        });
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });

    function clearUnread() { if (!document.hidden) setUnread(0); }
    document.addEventListener("visibilitychange", clearUnread);
    window.addEventListener("focus", clearUnread);
  }

  /* ---------- Handoff plans -> plan island, tray icon and taskbar progress ----------
   * Hark labels each plan's button "<title>. Plan, <done> of <total> steps done".
   * The shell gets the running plans (hark-plans), each finish (hark-plan-done)
   * and the furthest-along percentage for the taskbar button (hark-progress).
   * A plan is known by its title. One that leaves the page (another page is
   * open, or a long list unloads its card) is remembered for a while, and so
   * is everything across a reload, so its finish is still reported, once. */
  var PLAN_LABEL = /^(.*)\.\s*Plan,\s*(\d+)\s*of\s*(\d+)\s*steps?\s+done/;
  var PLAN_BUTTONS = 'button[aria-label*=". Plan, "]';
  var PLAN_MEMORY = 20 * 60 * 1000;
  function readPlans() {
    var plans = [];
    document.querySelectorAll(PLAN_BUTTONS).forEach(function (btn) {
      var m = (btn.getAttribute("aria-label") || "").match(PLAN_LABEL);
      if (!m) return;
      var total = parseInt(m[3], 10);
      if (!total) return;
      plans.push({ title: m[1].trim(), done: Math.min(parseInt(m[2], 10), total), total: total });
    });
    return plans;
  }
  function installProgress(emit) {
    var lastPct = -1; /* -1 = no active plan */
    var lastSent = "";
    /* title -> { done, total, seen } for every plan last known to be running */
    var running = {};
    try { running = JSON.parse(sessionStorage.getItem("harkDesktopPlans") || "{}") || {}; } catch (e) { running = {}; }
    var stored = "";
    function remember() {
      var next = JSON.stringify(running);
      if (next === stored) return;
      stored = next;
      try { sessionStorage.setItem("harkDesktopPlans", next); } catch (e) { /* storage blocked */ }
    }
    function scan() {
      var now = Date.now();
      var onPage = {};
      readPlans().forEach(function (p) {
        var seen = onPage[p.title];
        /* The same title twice (an old finished run and a new one): the running one counts. */
        if (!seen || (seen.done >= seen.total && p.done < p.total) || (p.done < p.total && p.done > seen.done)) onPage[p.title] = p;
      });
      Object.keys(onPage).forEach(function (title) {
        var p = onPage[title];
        if (p.done < p.total) {
          running[title] = { done: p.done, total: p.total, seen: now };
        } else if (running[title]) {
          delete running[title];
          emit("hark-plan-done", { title: title, total: p.total });
        }
      });
      var active = [];
      var best = -1;
      Object.keys(running).forEach(function (title) {
        var r = running[title];
        if (!onPage[title] && now - r.seen > PLAN_MEMORY) { delete running[title]; return; }
        active.push({ title: title, done: r.done, total: r.total });
        best = Math.max(best, Math.round((r.done / r.total) * 100));
      });
      remember();
      var sent = JSON.stringify(active);
      if (sent !== lastSent) {
        lastSent = sent;
        emit("hark-plans", { plans: active });
      }
      if (best !== lastPct) {
        lastPct = best;
        emit("hark-progress", { value: best < 0 ? null : best });
      }
    }
    /* Rescan when a plan's label changes or a plan appears, at most a few times a
     * second; the slow timer catches anything missed and forgets old plans. */
    var queued = null;
    function soon() { if (!queued) queued = setTimeout(function () { queued = null; scan(); }, 300); }
    new MutationObserver(soon).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["aria-label"] });
    setInterval(scan, 5000);
    scan();
  }

  /* ---------- links: Hark stays in-app, the rest opens in your browser ---------- */
  function installLinks(emit) {
    function handle(e) {
      var anchor = e.target && e.target.closest ? e.target.closest("a[href]") : null;
      if (!anchor) return;
      var href = anchor.getAttribute("href") || "";
      if (!href || href.charAt(0) === "#") return; /* incl. Hark's #hark:home */
      if (/^(mailto:|tel:)/i.test(href)) {
        e.preventDefault();
        emit("hark-external", { url: href });
        return;
      }
      var url;
      try { url = new URL(href, location.href); } catch (err) { return; }
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        /* blob: and data: links are Hark's own downloads; any other scheme is dropped. */
        if (url.protocol !== "blob:" && url.protocol !== "data:") e.preventDefault();
        return;
      }
      if (url.protocol === "https:" && hostAllowed(url.hostname) && anchor.target !== "_blank") return;
      e.preventDefault();
      emit("hark-external", { url: url.href });
    }
    document.addEventListener("click", handle, true);
    document.addEventListener("auxclick", function (e) { if (e.button === 1) handle(e); }, true);
  }

  /* ---------- keyboard: settings, zoom, reload, fullscreen ---------- */
  function applyZoom(zoom) {
    document.documentElement.style.zoom = String(zoom);
    document.documentElement.style.setProperty("--hark-zoom", String(zoom));
  }
  function setZoom(zoom) {
    zoom = Math.min(2, Math.max(0.5, Math.round(zoom * 20) / 20));
    applyZoom(zoom);
    localStorage.setItem("harkDesktopZoom", String(zoom));
  }
  function installKeys(emit) {
    document.addEventListener("keydown", function (e) {
      var mod = e.ctrlKey || e.metaKey;
      if (mod && e.key === ",") { e.preventDefault(); emit("hark-settings", {}); }
      else if (mod && (e.key === "=" || e.key === "+")) { e.preventDefault(); setZoom(parseFloat(localStorage.getItem("harkDesktopZoom") || "1") + 0.1); }
      else if (mod && e.key === "-") { e.preventDefault(); setZoom(parseFloat(localStorage.getItem("harkDesktopZoom") || "1") - 0.1); }
      else if (mod && e.key === "0") { e.preventDefault(); setZoom(1); }
      else if (mod && (e.key === "r" || e.key === "R")) { e.preventDefault(); location.reload(); }
      else if (e.key === "F11") { e.preventDefault(); emit("hark-window", { action: "fullscreen" }); }
    });
  }

  /* ---------- offline pill (Hark-styled, non-destructive) ----------
   * Hark reconnects by itself when the connection comes back, so nothing is
   * reloaded behind your back (a draft or a reply in progress would be lost). */
  function installOffline() {
    var pill = document.createElement("div");
    pill.id = "hark-offline";
    pill.setAttribute("role", "status");
    var note = document.createElement("span");
    note.textContent = "You\u2019re offline. Hark picks up where you left off when you\u2019re back.";
    pill.appendChild(note);
    var btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = "Try again";
    btn.addEventListener("click", function () {
      if (navigator.onLine) location.reload();
      else note.textContent = "Still offline. Check your connection.";
    });
    pill.appendChild(btn);
    document.body.appendChild(pill);
    function update() {
      pill.style.display = navigator.onLine ? "none" : "flex";
      if (!navigator.onLine) note.textContent = "You\u2019re offline. Hark picks up where you left off when you\u2019re back.";
    }
    window.addEventListener("offline", update);
    window.addEventListener("online", update);
    update();
  }

  /* ---------- commands coming from the shell (tray, shortcuts, quick box) ---------- */
  function focusComposer() {
    var el = document.querySelector('[data-testid="chat-input"]');
    if (el) { el.focus(); return true; }
    return false;
  }
  /* Put quick-ask text in Hark's message box. If Hark is on another page, go to
   * Chat first and wait for the box; if the box already holds a half-written
   * message, add the new text below it rather than replacing it. The text is
   * kept in sessionStorage until it is in, so going to Chat (even by a full page
   * load) loses nothing; if it can't be put in, the shell is told and puts it
   * back in the quick ask. */
  var COMPOSE_KEY = "harkDesktopCompose";
  function putText(el, text) {
    el.focus();
    if (el.isContentEditable) {
      var existing = (el.innerText || "").replace(/\s+$/, "");
      var range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      var sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      return document.execCommand("insertText", false, existing ? "\n\n" + text : text);
    }
    var current = (el.value || "").replace(/\s+$/, "");
    var value = current ? current + "\n\n" + text : text;
    /* Set it the way typing does, so React sees the change. */
    var proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    var setter = Object.getOwnPropertyDescriptor(proto, "value");
    if (setter && setter.set) setter.set.call(el, value);
    else el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    try { el.setSelectionRange(value.length, value.length); } catch (e) { /* not a text field */ }
    return true;
  }
  function insertComposerText(emit, text, tries) {
    tries = tries || 0;
    var el = document.querySelector('[data-testid="chat-input"]');
    if (!el) {
      if (!tries) {
        try { sessionStorage.setItem(COMPOSE_KEY, text); } catch (e) { /* storage blocked */ }
        clickAria("Chat");
      }
      if (tries < 80) setTimeout(function () { insertComposerText(emit, text, tries + 1); }, 100);
      else { try { sessionStorage.removeItem(COMPOSE_KEY); } catch (e) { /* storage blocked */ } emit("hark-compose-result", { ok: false, text: text }); }
      return;
    }
    var ok = false;
    try { ok = putText(el, text); } catch (e) { ok = false; }
    try { sessionStorage.removeItem(COMPOSE_KEY); } catch (e) { /* storage blocked */ }
    emit("hark-compose-result", ok ? { ok: true } : { ok: false, text: text });
  }
  /* Text that was on its way in when the page loaded again. */
  function resumeCompose(emit) {
    var text = "";
    try { text = sessionStorage.getItem(COMPOSE_KEY) || ""; } catch (e) { text = ""; }
    if (text) insertComposerText(emit, text, 1);
  }
  /* The plan island's Open: press that plan's own button in Hark. */
  function openPlan(title) {
    var plans = document.querySelectorAll("button[aria-label]");
    for (var i = 0; i < plans.length; i++) {
      var m = (plans[i].getAttribute("aria-label") || "").match(PLAN_LABEL);
      if (m && m[1].trim() === title) { plans[i].click(); return true; }
    }
    return false;
  }
  function clickAria(label) {
    var el = document.querySelector('button[aria-label="' + label + '"]');
    if (el) { el.click(); return true; }
    return false;
  }
  function installShellEvents(T, emit) {
    function on(name, fn) {
      try {
        var p = T.event.listen(name, fn);
        if (p && p.catch) p.catch(function () { /* not allowed on this page */ });
      } catch (e) { /* shell gone */ }
    }
    resumeCompose(emit);
    on("hark-focus-composer", function () { focusComposer(); });
    on("hark-compose", function (ev) {
      var text = ev && ev.payload && ev.payload.text ? String(ev.payload.text) : "";
      if (text) insertComposerText(emit, text);
      else focusComposer();
    });
    on("hark-open-plan", function (ev) {
      var title = ev && ev.payload ? String(ev.payload.title || "") : "";
      if (title && !openPlan(title)) { if (!clickAria("Chat")) location.assign("/chat"); }
    });
    on("hark-navigate", function (ev) {
      var target = ev && ev.payload ? ev.payload.target : "";
      if (target === "chat") { if (!clickAria("Chat")) location.assign("/chat"); }
      else if (target === "projects") clickAria("Projects");
      else if (target === "search") clickAria("Search the page and your mail");
      else if (target === "home") {
        var list = document.querySelector('[data-testid="message-list"]');
        if (list) list.scrollTo({ top: list.scrollHeight, behavior: "smooth" });
        focusComposer();
      }
    });
  }
})();
