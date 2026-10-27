// Whether the pages are light or dark. The choice is the person's: "system"
// follows the computer, the other two do not. The mode is what that choice
// resolves to right now, and it is the only thing the styles look at.
//
// Nothing here may need a document or a window at load, so the unit tests can
// import it under node.
import { read, write, THEME, PALETTE, PREVIEW_PAPER } from "./storage.js";
import { desktop, shellAction } from "./desktop/client.js";

export const CHOICES = ["system", "light", "dark"];

// The colour themes. "librepaper" is the default and has no file of its own.
export const PALETTES = ["librepaper", "flexoki", "solarized", "gruvbox", "catppuccin", "nord"];
export const PALETTE_NAMES = { librepaper: "LibrePaper", flexoki: "Flexoki", solarized: "Solarized", gruvbox: "Gruvbox", catppuccin: "Catppuccin", nord: "Nord" };

// Whether the HTML preview's paper follows the theme or stays light.
export const PAPERS = ["theme", "light"];

const QUERY = "(prefers-color-scheme: dark)";
const modeListeners = new Set();
const choiceListeners = new Set();
const paletteListeners = new Set();
const paperListeners = new Set();
let started = false;
let applied = null;
// The choice made on this page. Storage is best effort: a browser that refuses
// the write still gets the theme it asked for, until another tab writes one.
let chosen = null;
// The colour theme chosen on this page, with the same fallback as the mode.
let chosenPalette = null;
// The preview paper chosen on this page, with the same fallback.
let chosenPaper = null;
// The one media query list, made on first use and held for the module's
// lifetime. The change listener in start() hangs off it, and a list that
// nothing holds was seen to stop reporting changes in headless Chromium: the
// browser test flipped the emulated scheme and the mode never followed.
let media = null;

function query() {
  if (!media && typeof globalThis.matchMedia === "function") media = globalThis.matchMedia(QUERY);
  return media;
}

export function choice() {
  const stored = chosen ?? read(THEME, "system");
  return CHOICES.includes(stored) ? stored : "system";
}

export function palette() {
  const stored = chosenPalette ?? read(PALETTE, "librepaper");
  return PALETTES.includes(stored) ? stored : "librepaper";
}

export function paper() {
  const stored = chosenPaper ?? read(PREVIEW_PAPER, "theme");
  return PAPERS.includes(stored) ? stored : "theme";
}

function resolve() {
  const current = choice();
  if (current !== "system") return current;
  return query()?.matches ? "dark" : "light";
}

export function mode() {
  return resolve();
}

// The paper the HTML preview is drawn on: light when the choice says so,
// otherwise the mode.
export function paperMode() {
  return paper() === "light" ? "light" : mode();
}

// Applies the mode to the page and tells everybody who asked, but only when it
// moved, so a system change while the choice is fixed is silent.
function apply() {
  const now = resolve();
  const root = globalThis.document?.documentElement;
  if (root) {
    root.dataset.mode = now;
    root.dataset.palette = palette();
  }
  if (now === applied) return;
  applied = now;
  for (const listener of [...modeListeners]) listener(now);
}

function announce() {
  const now = choice();
  for (const listener of [...choiceListeners]) listener(now);
}

function announcePalette() {
  const now = palette();
  for (const listener of [...paletteListeners]) listener(now);
}

function announcePaper() {
  const now = paper();
  for (const listener of [...paperListeners]) listener(now);
}

// The desktop app colours its own title bar from this. It is a courtesy, so a
// failure is dropped and nothing waits on it.
function tellShell() {
  if (desktop()) shellAction("theme", { theme: choice() }).catch(() => {});
}

export function setChoice(next) {
  if (!CHOICES.includes(next)) return;
  chosen = next;
  write(THEME, next);
  apply();
  announce();
  tellShell();
}

export function setPalette(next) {
  if (!PALETTES.includes(next)) return;
  chosenPalette = next;
  write(PALETTE, next);
  apply();
  announcePalette();
}

export function setPaper(next) {
  if (!PAPERS.includes(next)) return;
  chosenPaper = next;
  write(PREVIEW_PAPER, next);
  announcePaper();
}

// Hears the colour theme now and whenever one is made.
export function onPalette(listener) {
  paletteListeners.add(listener);
  listener(palette());
  return () => paletteListeners.delete(listener);
}

// Hears the preview paper now and whenever one is made.
export function onPaper(listener) {
  paperListeners.add(listener);
  listener(paper());
  return () => paperListeners.delete(listener);
}

// Hears the mode now and whenever it moves.
export function onMode(listener) {
  modeListeners.add(listener);
  listener(mode());
  return () => modeListeners.delete(listener);
}

// Hears the choice now and whenever one is made, on this page or in another
// tab of this browser.
export function onChoice(listener) {
  choiceListeners.add(listener);
  listener(choice());
  return () => choiceListeners.delete(listener);
}

export function start() {
  if (started) return;
  started = true;
  apply();
  query()?.addEventListener?.("change", () => {
    if (choice() === "system") apply();
  });
  // Another tab or window of this browser made a choice, and this page
  // follows it. A null key is the whole store being cleared.
  globalThis.addEventListener?.("storage", (event) => {
    // A palette change from another tab. The mode does not move, so the mode
    // listeners stay quiet.
    if (event.key === PALETTE) {
      chosenPalette = null;
      apply();
      announcePalette();
      return;
    }
    // A preview paper change from another tab. Only the preview listens to it.
    if (event.key === PREVIEW_PAPER) {
      chosenPaper = null;
      announcePaper();
      return;
    }
    if (event.key !== null && event.key !== THEME) return;
    chosen = null;
    chosenPalette = null;
    chosenPaper = null;
    apply();
    announce();
    announcePalette();
    announcePaper();
  });
  tellShell();
}
