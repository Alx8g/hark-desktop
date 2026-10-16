import "./frame";

// Rotates calm status lines while Rust waits for hark.com's first paint.
const lines = [
  "Getting your brief ready…",
  "Checking what's new…",
  "Almost there…",
];
const el = document.getElementById("status");
let i = 0;
window.setInterval(() => {
  i = Math.min(i + 1, lines.length - 1);
  if (el) el.textContent = lines[i];
}, 2600);
