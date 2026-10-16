// Progressive enhancement only: the page works without this script.
// 1. Find the repository from the GitHub Pages address, so the links need no hard-coded owner.
// 2. Highlight the visitor's system and label the main button.
// 3. Point each download button at the right file from the latest release, when GitHub answers.
// No analytics, no cookies, no third-party scripts. The one request is to GitHub's public API,
// the same company that serves this page.
(function () {
  "use strict";

  // The site is served from more than one address (a custom domain and the Pages preview address), so
  // the repository is named here rather than guessed from the address bar.
  var REPO_OVERRIDE = "https://github.com/Alx8g/hark-desktop";

  function repoUrl() {
    if (REPO_OVERRIDE) return REPO_OVERRIDE.replace(/\/$/, "");
    var m = location.hostname.match(/^([^.]+)\.github\.io$/);
    var name = location.pathname.split("/")[1];
    return m && name ? "https://github.com/" + m[1] + "/" + name : "";
  }

  var repo = repoUrl();
  var links = document.querySelectorAll("[data-repo], [data-repo-path]");
  for (var i = 0; i < links.length; i++) {
    var a = links[i];
    if (!repo) {
      // Opened from disk or a custom domain without an override: leave links inert rather than wrong.
      a.removeAttribute("href");
      a.setAttribute("aria-disabled", "true");
      continue;
    }
    a.setAttribute("href", repo + (a.getAttribute("data-repo-path") || ""));
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
  var main = document.getElementById("cta-main");

  if (NAMES[os]) {
    main.textContent = "Download for " + NAMES[os];
    var card = document.querySelector('.dl [data-os="' + os + '"]');
    if (card) card.setAttribute("data-current", "");
  } else if (os === "mobile") {
    main.textContent = "Get it on your computer";
  }

  // Match release files by extension. First match per system wins.
  var PATTERNS = {
    windows: [/-setup\.exe$/i, /\.msi$/i],
    macos: [/\.dmg$/i],
    linux: [/\.appimage$/i, /\.deb$/i, /\.rpm$/i],
  };

  if (!repo) return;
  fetch(repo.replace("https://github.com/", "https://api.github.com/repos/") + "/releases/latest", {
    headers: { Accept: "application/vnd.github+json" },
  })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
    .then(function (release) {
      var assets = release.assets || [];
      Object.keys(PATTERNS).forEach(function (key) {
        var hit = null;
        PATTERNS[key].some(function (re) {
          hit = assets.filter(function (a) { return re.test(a.name); })[0] || null;
          return hit;
        });
        var btn = document.querySelector('[data-dl="' + key + '"]');
        if (hit && btn) btn.setAttribute("href", hit.browser_download_url);
        if (hit && key === os) main.setAttribute("href", hit.browser_download_url);
      });
    })
    .catch(function () { /* keep the releases page link */ });

})();
