// Progressive enhancement only: the page is complete without this script.
// 1. Name the repository once, so every GitHub link points at it.
// 2. Highlight the visitor's system and label the main buttons.
// 3. Point each download button at the right file from the latest release, when GitHub answers,
//    and say which version that is, with a link to what's new in it.
// 4. Fade sections in as they scroll into view, and flatten the hero window as you scroll.
// No analytics, no cookies, no third-party scripts. The one request is to GitHub's public API.
(function () {
  "use strict";

  document.documentElement.classList.add("js");

  // The site is served from more than one address (a custom domain and the Pages preview address), so
  // the repository is named here rather than guessed from the address bar.
  var REPO = "https://github.com/Alx8g/hark-desktop";

  var links = document.querySelectorAll("[data-repo], [data-repo-path]");
  for (var i = 0; i < links.length; i++) {
    links[i].setAttribute("href", REPO + (links[i].getAttribute("data-repo-path") || ""));
  }

  function detect() {
    var p = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "").toLowerCase();
    var ua = (navigator.userAgent || "").toLowerCase();
    if (/android|iphone|ipad|ipod/.test(ua)) return "mobile";
    if (/win/.test(p) || /windows/.test(ua)) return "windows";
    if (/mac/.test(p) || /macintosh|mac os x/.test(ua)) return "macos";
    if (/linux|x11|cros/.test(p + ua)) return "linux";
    return "";
  }

  var NAMES = { windows: "Windows", macos: "macOS", linux: "Linux" };
  var os = detect();
  var mains = [document.getElementById("cta-main"), document.getElementById("cta-bottom")];

  mains.forEach(function (b) {
    if (!b) return;
    if (NAMES[os]) b.textContent = "Download for " + NAMES[os];
    else if (os === "mobile") b.textContent = "Get it on your computer";
  });
  var card = NAMES[os] && document.querySelector('.os[data-os="' + os + '"]');
  if (card) card.setAttribute("data-current", "");

  // Match release files by extension. First match per system wins.
  var PATTERNS = {
    windows: [/^Hark-Desktop-Setup\.exe$/i, /-setup\.exe$/i, /\.msi$/i],
    macos: [/\.dmg$/i],
    linux: [/\.appimage$/i, /\.deb$/i, /\.rpm$/i],
  };

  fetch(REPO.replace("https://github.com/", "https://api.github.com/repos/") + "/releases/latest", {
    headers: { Accept: "application/vnd.github+json" },
  })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
    .then(function (release) {
      showRelease(release);
      var assets = release.assets || [];
      Object.keys(PATTERNS).forEach(function (key) {
        var hit = null;
        PATTERNS[key].some(function (re) {
          hit = assets.filter(function (a) { return re.test(a.name); })[0] || null;
          return hit;
        });
        if (!hit) return;
        var btn = document.querySelector('[data-dl="' + key + '"]');
        if (btn) btn.setAttribute("href", hit.browser_download_url);
        if (key === os) mains.forEach(function (b) { if (b) b.setAttribute("href", hit.browser_download_url); });
      });
    })
    .catch(function () { /* no release yet: the buttons keep their link to the releases page */ });

  // "Version 0.2.0 · 18 October 2026 · What's new", built from text nodes only.
  function showRelease(release) {
    var line = document.getElementById("release");
    var tag = String(release.tag_name || "");
    var page = String(release.html_url || "");
    if (!line || !/^v?\d+\.\d+\.\d+$/.test(tag) || page.indexOf(REPO + "/releases/") !== 0) return;
    var when = release.published_at ? new Date(release.published_at) : null;
    var date = when && !isNaN(when) ? when.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" }) : "";
    line.textContent = "Version " + tag.replace(/^v/, "") + (date ? " · " + date : "") + " · ";
    var link = document.createElement("a");
    link.href = page;
    link.textContent = "What’s new";
    line.appendChild(link);
    line.hidden = false;
  }

  // ---------- motion ----------
  var still = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var items = document.querySelectorAll("[data-reveal]");

  if (still || !("IntersectionObserver" in window)) {
    for (var j = 0; j < items.length; j++) items[j].classList.add("in");
  } else {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (!e.isIntersecting) return;
        e.target.classList.add("in");
        io.unobserve(e.target);
      });
    }, { threshold: 0.12, rootMargin: "0px 0px -6% 0px" });
    for (var k = 0; k < items.length; k++) io.observe(items[k]);
  }

  // The hero window starts tilted back and settles flat as the page scrolls (--p goes from 0 to 1).
  var stage = document.getElementById("stage");
  if (stage && !still) {
    var queued = false;
    var update = function () {
      queued = false;
      var top = stage.getBoundingClientRect().top;
      var vh = window.innerHeight || 800;
      var p = 1 - Math.min(Math.max((top - vh * 0.05) / (vh * 0.8), 0), 1);
      stage.style.setProperty("--p", p.toFixed(3));
    };
    window.addEventListener("scroll", function () {
      if (!queued) { queued = true; window.requestAnimationFrame(update); }
    }, { passive: true });
    window.addEventListener("resize", update);
    update();
  }
})();
