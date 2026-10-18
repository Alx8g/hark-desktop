import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./platform";
import "./appearance";

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
const more = byId("more");
const sliver = byId("sliver");
const arc = byId("arc");
const announce = byId("announce");
const RING = 2 * Math.PI * 19;
/* Past this many steps, segments would be slivers: draw one continuous bar. */
const MAX_SEGMENTS = 24;
/* Room for the opened-up pill (island.html: 430 wide, top 10, about 120 tall). */
const OPEN_RECT = { w: 430, h: 136 };

let plan: Plan = { phase: "idle", title: "", done: 0, total: 0, others: 0 };
let lastDone = -1;
let lastTitle = "";
let pointer = false;
let tuckTimer = 0;
let dwellTimer = 0;

const steps = (n: number) => `${n} step${n === 1 ? "" : "s"}`;

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
  const total = Math.max(1, p.total);
  const finished = p.phase === "done" ? total : Math.min(p.done, total);
  if (total > MAX_SEGMENTS) {
    segs.classList.add("continuous");
    if (segs.childElementCount !== 1) segs.innerHTML = "<i></i>";
    const bar = segs.firstElementChild as HTMLElement;
    bar.className = "";
    bar.style.width = `${(finished / total) * 100}%`;
    return;
  }
  segs.classList.remove("continuous");
  if (segs.childElementCount !== total) segs.innerHTML = "<i></i>".repeat(total);
  Array.from(segs.children).forEach((seg, i) => {
    (seg as HTMLElement).removeAttribute("style");
    seg.className = i < finished ? "d" : i === finished ? "c" : "";
  });
}

function setOpen(open: boolean): void {
  isl.classList.toggle("open-up", open);
  // Out of the accessibility tree and unclickable while folded away.
  more.toggleAttribute("inert", !open || plan.phase !== "running");
}

function render(next: Plan): void {
  const before = plan;
  plan = next;
  if (next.phase === "idle") {
    window.clearTimeout(tuckTimer);
    window.clearTimeout(dwellTimer);
    isl.classList.add("gone");
    isl.setAttribute("aria-hidden", "true");
    setOpen(false);
    lastDone = -1;
    lastTitle = "";
    report();
    return;
  }
  isl.classList.remove("gone");
  isl.removeAttribute("aria-hidden");
  isl.classList.toggle("done", next.phase === "done");
  title.textContent = next.title;
  title.title = next.title;
  const frac = next.phase === "done" ? 1 : next.total ? next.done / next.total : 0;
  arc.setAttribute("stroke-dasharray", `${RING * frac} ${RING}`);
  sliver.style.width = `${frac * 100}%`;
  count.textContent = `${next.done} of ${next.total}`;
  const others = next.others > 0 ? ` · ${next.others} other plan${next.others === 1 ? "" : "s"} running` : "";
  const step = Math.min(next.done + 1, next.total);
  sub.textContent =
    next.phase === "done"
      ? next.total === 1
        ? "Done · 1 step finished"
        : `Done · all ${steps(next.total)} finished`
      : `Step ${step} of ${next.total}${others}`;
  paintSegments(next);
  setOpen(isl.classList.contains("open-up"));
  const samePlan = next.title === lastTitle;
  if (next.phase === "running") {
    if (!samePlan || next.done !== lastDone || before.phase !== "running") {
      peek(2200);
      announce.textContent = `${next.title}: step ${step} of ${next.total}`;
    }
    // The ring pulses only when this plan moves on, not when another takes its place.
    if (samePlan && lastDone !== -1 && next.done > lastDone) {
      isl.classList.remove("bump");
      void isl.offsetWidth;
      isl.classList.add("bump");
    }
    lastDone = next.done;
  } else {
    if (before.phase !== "done" || !samePlan) announce.textContent = `${next.title} is done`;
    lastDone = -1;
    peek(8000);
  }
  lastTitle = next.title;
  report();
}

/* Tell lib.rs where the pill is, so only the pill catches the mouse. It is
   "solid" (takes clicks) only once a pointer has rested on it. */
let sent = "";
function send(rect: { x: number; y: number; w: number; h: number }, solid: boolean): void {
  const key = [rect.x, rect.y, rect.w, rect.h].map(Math.round).join(",") + (solid ? "!" : "");
  if (key === sent) return;
  sent = key;
  void invoke("island_rect", { ...rect, solid });
}
function measure(): void {
  const r = isl.getBoundingClientRect();
  const top = Math.max(0, r.top);
  const shown = plan.phase !== "idle" && !isl.classList.contains("tucked");
  const solid = shown && isl.classList.contains("open-up");
  send({ x: r.left, y: top, w: r.width, h: Math.max(0, r.bottom - top) }, solid);
}
function report(): void {
  requestAnimationFrame(measure);
  window.setTimeout(measure, 560);
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
    // Brushing past on the way to a tab beneath shouldn't open it or take the click.
    const tucked = isl.classList.contains("tucked");
    dwellTimer = window.setTimeout(() => {
      isl.classList.remove("tucked");
      setOpen(true);
      // Say where it is going straight away, so the pointer counts as over it while it opens.
      if (plan.phase === "running") send({ x: (window.innerWidth - OPEN_RECT.w) / 2, y: 0, ...OPEN_RECT }, true);
      report();
    }, tucked ? 350 : 250);
  } else {
    setOpen(false);
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

// The event is only a cue: the state itself always comes from lib.rs.
const refresh = () => void invoke<Plan>("island_state").then(render);
void listen("plan-state", refresh);
refresh();
