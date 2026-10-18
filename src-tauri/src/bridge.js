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
  if (window.__HARK_DESKTOP__) return;
  window.__HARK_DESKTOP__ = true;

  /* Needs nothing from Tauri or the DOM, so start before either is ready. */
  installFrame();

  var ALLOWED_HOSTS = [
    "hark.com", "accounts.google.com", "appleid.apple.com",
    "challenges.cloudflare.com", "js.stripe.com", "checkout.stripe.com", "m.stripe.network"
  ];
  var ALLOWED_SUFFIXES = [".hark.com", ".google.com", ".gstatic.com", ".apple.com", ".stripe.com", ".cloudflare.com"];

  function hostAllowed(host) {
    if (!host) return true;
    if (ALLOWED_HOSTS.indexOf(host) >= 0) return true;
    for (var i = 0; i < ALLOWED_SUFFIXES.length; i++) {
      if (host.slice(-ALLOWED_SUFFIXES[i].length) === ALLOWED_SUFFIXES[i]) return true;
    }
    return false;
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
        requestAnimationFrame(function () { requestAnimationFrame(fn); });
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
      try { T.event.emit(name, payload || {}); } catch (e) { /* shell gone */ }
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
    installMessages(emit);
    installProgress(emit);
    installLinks(emit);
    installKeys(emit);
    installOffline();
    installShellEvents(T);
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
  function frameCss() {
    var rules = [];
    if (isWindows()) {   /* only the frameless Windows window needs a page-drawn silhouette */
      rules.push(
        ":root{--hark-zoom:1;--window-radius:calc(20px / var(--hark-zoom))}",
        "html{min-height:100%!important;clip-path:inset(0 round var(--window-radius))!important}",
        "html[data-square]{clip-path:none!important}",
        "html::after{content:'';position:fixed;inset:0;z-index:2147483647;pointer-events:none;border-radius:var(--window-radius);box-shadow:inset 0 0 0 1px rgba(29,29,31,.16)}",
        "html[data-square]::after{display:none}",
        "html[data-scrolls]{height:100%!important;overflow:hidden!important}",
        "html[data-scrolls] body{height:100%!important;overflow-x:hidden!important;overflow-y:auto!important}"
      );
    }
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
      "html{background:linear-gradient(168deg,#7fb0dd 0%,#b6cfe7 45%,#d9e9f6 70%,#ffeed9 120%)}",
      "[data-app-download-banner]{display:none!important}",
      "::-webkit-scrollbar{width:10px;height:10px}",
      "::-webkit-scrollbar-thumb{background:rgba(29,29,31,.22);border-radius:999px;border:2px solid transparent;background-clip:content-box}",
      "::-webkit-scrollbar-thumb:hover{background:rgba(29,29,31,.38);border:2px solid transparent;background-clip:content-box}",
      "::-webkit-scrollbar-track{background:transparent}",
      "::selection{background:rgba(127,176,221,.45)}",
      "#hark-offline{position:fixed;left:50%;bottom:28px;transform:translateX(-50%);z-index:2147483646;background:rgba(255,255,255,.98);color:#1d1d1f;border-radius:18px;padding:14px 20px;font:500 14px 'GT Standard L','Segoe UI',sans-serif;box-shadow:0 12px 40px rgba(29,29,31,.18);display:none;align-items:center;gap:12px}",
      "#hark-offline button{appearance:none;border:0;cursor:pointer;background:#1d1d1f;color:#f9f9fa;font:inherit;font-size:13px;font-weight:500;border-radius:9999px;padding:8px 14px}"
    ];
    if (isWindows()) {
      /* The window controls live in the page on Windows only; Search and Settings
       * slide left to make room. caption pill matches Hark's Search pill exactly:
       * 48px tall at top:14px, right inset 16px. */
      css.push(
        "@media (min-width:760px){",
        'button[aria-label="Settings"],button[aria-label="Search the page and your mail"]{translate:-140px 0}',
        "}",
        "#hark-caption{position:fixed;top:14px;right:16px;z-index:2147483646;display:flex;gap:2px;padding:4px;border-radius:9999px;background:rgba(255,255,255,.55);backdrop-filter:blur(16px) saturate(1.4);-webkit-backdrop-filter:blur(16px) saturate(1.4);box-shadow:inset 0 0 0 1px rgba(29,29,31,.08),0 4px 18px rgba(29,29,31,.08);font-family:'GT Standard L','Segoe UI Variable Text','Segoe UI',sans-serif}",
        "#hark-caption button{appearance:none;border:0;background:transparent;width:40px;height:40px;border-radius:9999px;display:grid;place-items:center;color:#1d1d1f;cursor:pointer;padding:0}",
        "#hark-caption button:hover{background:rgba(29,29,31,.07)}",
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
      '<button data-act="close" class="hark-close" title="Close (Hark keeps running in the tray)" aria-label="Close window">' + svgIcon('<path d="M3 3l10 10M13 3L3 13" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>') + "</button>";
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
      return true;
    }
    document.addEventListener("mousedown", function (e) {
      if (e.button === 0 && inDragArea(e)) emit("hark-window", { action: "drag" });
    }, true);
    document.addEventListener("dblclick", function (e) {
      if (inDragArea(e)) emit("hark-window", { action: "maximize" });
    }, true);
  }

  /* ---------- web Notification API -> native Windows toasts ---------- */
  function installNotifications(emit) {
    function NativeNotification(title, options) {
      emit("hark-notify", { title: String(title || "Hark"), body: (options && options.body) || "" });
      this.title = title; this.body = (options && options.body) || "";
    }
    NativeNotification.permission = "granted";
    NativeNotification.requestPermission = function (cb) {
      var p = Promise.resolve("granted");
      if (typeof cb === "function") p.then(cb);
      return p;
    };
    try { window.Notification = NativeNotification; } catch (e) { /* sealed */ }
    if (window.ServiceWorkerRegistration && ServiceWorkerRegistration.prototype.showNotification) {
      ServiceWorkerRegistration.prototype.showNotification = function (title, options) {
        emit("hark-notify", { title: String(title || "Hark"), body: (options && options.body) || "" });
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
   * and the furthest-along percentage for the taskbar button (hark-progress). */
  var PLAN_LABEL = /^(.*)\.\s*Plan,\s*(\d+)\s*of\s*(\d+)\s*steps done/;
  function readPlans() {
    var plans = [];
    document.querySelectorAll("button[aria-label]").forEach(function (btn) {
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
    var running = {};
    function scan() {
      var best = -1;
      var active = [];
      var nowRunning = {};
      readPlans().forEach(function (p) {
        if (p.done < p.total) {
          if (!nowRunning[p.title]) active.push(p);
          nowRunning[p.title] = true;
          best = Math.max(best, Math.round((p.done / p.total) * 100));
        } else if (running[p.title]) {
          emit("hark-plan-done", { title: p.title, total: p.total });
        }
      });
      running = nowRunning;
      var sent = JSON.stringify(active);
      if (sent !== lastSent) {
        lastSent = sent;
        emit("hark-plans", { plans: active });
      }
      if (best === -1) {
        var bar = document.querySelector('[role="progressbar"][aria-valuenow]');
        if (bar) {
          var now = parseFloat(bar.getAttribute("aria-valuenow"));
          var max = parseFloat(bar.getAttribute("aria-valuemax") || "100");
          if (isFinite(now) && max > 0) best = Math.round((now / max) * 100);
        }
      }
      if (best !== lastPct) {
        lastPct = best;
        emit("hark-progress", { value: best < 0 ? null : best });
      }
    }
    setInterval(scan, 1500);
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
        e.preventDefault();
        emit("hark-external", { url: href });
        return;
      }
      if (hostAllowed(url.hostname) && anchor.target !== "_blank") return;
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

  /* ---------- offline pill (Hark-styled, non-destructive) ---------- */
  function installOffline() {
    var pill = document.createElement("div");
    pill.id = "hark-offline";
    pill.innerHTML = "<span>You're offline. Hark picks up where you left off when you're back.</span>";
    var btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = "Retry";
    btn.addEventListener("click", function () { location.reload(); });
    pill.appendChild(btn);
    document.body.appendChild(pill);
    function update() { pill.style.display = navigator.onLine ? "none" : "flex"; }
    window.addEventListener("offline", update);
    window.addEventListener("online", function () { update(); location.reload(); });
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
   * message, add the new text below it rather than replacing it. */
  function insertComposerText(text, tries) {
    var el = document.querySelector('[data-testid="chat-input"]');
    if (!el) {
      if (!tries) clickAria("Chat");
      if ((tries || 0) < 30) setTimeout(function () { insertComposerText(text, (tries || 0) + 1); }, 100);
      return;
    }
    el.focus();
    var existing = (el.value || "").replace(/\s+$/, "");
    var value = existing ? existing + "\n\n" + text : text;
    var setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value");
    if (setter && setter.set) setter.set.call(el, value);
    else el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    try { el.setSelectionRange(value.length, value.length); } catch (e) { /* not a text field */ }
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
  function installShellEvents(T) {
    T.event.listen("hark-focus-composer", function () { focusComposer(); });
    T.event.listen("hark-compose", function (ev) {
      var text = ev && ev.payload && ev.payload.text ? String(ev.payload.text) : "";
      if (text) insertComposerText(text);
      else focusComposer();
    });
    T.event.listen("hark-open-plan", function (ev) {
      var title = ev && ev.payload ? String(ev.payload.title || "") : "";
      if (title && !openPlan(title)) { if (!clickAria("Chat")) location.assign("/chat"); }
    });
    T.event.listen("hark-navigate", function (ev) {
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
