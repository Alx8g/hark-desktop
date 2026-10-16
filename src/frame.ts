import "./platform";

// Frameless windows round themselves in CSS (see shell.css). A maximized or
// full-screen window has to sit flush against the monitor, so drop the
// rounding whenever the window fills the screen it is on.
const root = document.documentElement;

function fills(width: number, height: number): boolean {
  return window.outerWidth >= width - 2 && window.outerHeight >= height - 2;
}

function sync(): void {
  const { availWidth, availHeight, width, height } = window.screen;
  root.toggleAttribute("data-square", fills(availWidth, availHeight) || fills(width, height));
}

window.addEventListener("resize", sync);
sync();
