import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./platform";

// The plan island: a running Handoff plan at the top centre of the screen.
// Between steps it tucks up into the screen edge, leaving a thin line of sky.
// It drops down when a step finishes, when the plan is done, or when pointed at.
// lib.rs owns the window: it shows it, lets clicks through everywhere except
// the pill, and tells the page when the pointer is over it.
type Plan = { phase: "running" | "done" | "idle"; title: string; done: number; total: number; others: number };

const byId = (id: string) => document.getElementById(id) as HTMLElement;
const isl = byId("isl");
const title = byId("title");
const sub = byId("sub");
const count = byId("count");
const segs = byId("segs");
const sliver = byId("sliver");
const arc = byId("arc");
const RING = 2 * Math.PI * 19;

let plan: Plan = { phase: "idle", title: "", done: 0, total: 0, others: 0 };
let lastDone = -1;
let pointer = false;
let tuckTimer = 0;
let dwellTimer = 0;

function tuckLater(ms: number): void {
  window.clearTimeout(tuckTimer);
  tuckTimer = window.setTimeout(() => {
    if (pointer || plan.phase === "idle") return;
    isl.classList.add("tucked");
    report();
  }, ms);
}

function peek(ms: number): void {
  isl.classList.remove("tucked");
  tuckLater(ms);
}

function paintSegments(p: Plan): void {
  if (segs.childElementCount !== p.total) segs.innerHTML = "<i></i>".repeat(p.total);
  Array.from(segs.children).forEach((seg, i) => {
    seg.className = p.phase === "done" || i < p.done ? "d" : i === p.done ? "c" : "";
  });
}

function render(next: Plan): void {
  const before = plan.phase;
  plan = next;
  if (next.phase === "idle") {
    window.clearTimeout(tuckTimer);
    isl.classList.add("gone");
    isl.classList.remove("open-up");
    lastDone = -1;
    report();
    return;
  }
  isl.classList.remove("gone");
  isl.classList.toggle("done", next.phase === "done");
  title.textContent = next.title;
  const frac = next.phase === "done" ? 1 : next.total ? next.done / next.total : 0;
  arc.setAttribute("stroke-dasharray", `${RING * frac} ${RING}`);
  sliver.style.width = `${frac * 100}%`;
  count.textContent = `${next.done} of ${next.total}`;
  const others = next.others > 0 ? ` · and ${next.others} more` : "";
  sub.textContent = next.phase === "done" ? `Done · all ${next.total} steps finished` : `Step ${Math.min(next.done + 1, next.total)} of ${next.total}${others}`;
  paintSegments(next);
  if (next.phase === "running") {
    const stepped = next.done !== lastDone;
    if (stepped || before !== "running") peek(2200);
    if (stepped && lastDone !== -1) {
      isl.classList.remove("bump");
      void isl.offsetWidth;
      isl.classList.add("bump");
    }
    lastDone = next.done;
  } else {
    lastDone = -1;
    peek(8000);
  }
  report();
}

/* Tell lib.rs where the pill is, so only the pill catches the mouse. */
let sent = "";
function report(): void {
  const send = () => {
    const r = isl.getBoundingClientRect();
    const top = Math.max(0, r.top);
    const solid = plan.phase !== "idle" && !isl.classList.contains("tucked");
    const key = [r.left, top, r.width, r.bottom - top, solid].map((v) => (typeof v === "number" ? Math.round(v) : v)).join(",");
    if (key === sent) return;
    sent = key;
    void invoke("island_rect", { x: r.left, y: top, w: r.width, h: Math.max(0, r.bottom - top), solid });
  };
  requestAnimationFrame(send);
  window.setTimeout(send, 560);
}
isl.addEventListener("transitionend", report);
// A report can be lost while the window is still starting up: repeat it now and then.
window.setInterval(() => {
  if (plan.phase === "idle") return;
  sent = "";
  report();
}, 3000);

void listen<{ inside: boolean }>("island-pointer", (e) => {
  pointer = e.payload.inside;
  window.clearTimeout(dwellTimer);
  if (pointer) {
    window.clearTimeout(tuckTimer);
    // Brushing past the top edge on the way to a tab shouldn't open it: wait a moment first.
    const tucked = isl.classList.contains("tucked");
    dwellTimer = window.setTimeout(() => {
      isl.classList.remove("tucked");
      isl.classList.add("open-up");
      report();
    }, tucked ? 350 : 0);
  } else {
    isl.classList.remove("open-up");
    if (plan.phase !== "idle") tuckLater(plan.phase === "done" ? 2500 : 900);
    report();
  }
});

isl.addEventListener("click", (e) => {
  const act = (e.target as Element).closest("[data-act]")?.getAttribute("data-act");
  if (act === "open") void invoke("island_open");
  if (act === "hide") void invoke("island_hide_plan");
  if (act === "dismiss") void invoke("island_dismiss");
});

void listen<Plan>("plan-state", (e) => render(e.payload));
void invoke<Plan>("island_state").then(render);
