// Whether the pages are light or dark. The choice is the person's: "system"
// follows the computer, the other two do not. The mode is what that choice
// resolves to right now, and it is the only thing the styles look at.
//
// Nothing here may need a document or a window at load, so the unit tests can
// import it under node.
import { read, write, THEME } from "./storage.js";
import { desktop, shellAction } from "./desktop/client.js";

export const CHOICES = ["system", "light", "dark"];

const QUERY = "(prefers-color-scheme: dark)";
const listeners = new Set();
let started = false;
let applied = null;

function query() {
  return typeof globalThis.matchMedia === "function" ? globalThis.matchMedia(QUERY) : null;
}

export function choice() {
  const stored = read(THEME, "system");
  return CHOICES.includes(stored) ? stored : "system";
}

function resolve() {
  const chosen = choice();
  if (chosen !== "system") return chosen;
  return query()?.matches ? "dark" : "light";
}

export function mode() {
  return resolve();
}

// Applies the mode to the page and tells everybody who asked, but only when it
// moved, so a system change while the choice is fixed is silent.
function apply() {
  const now = resolve();
  const root = globalThis.document?.documentElement;
  if (root) root.dataset.mode = now;
  if (now === applied) return;
  applied = now;
  for (const listener of [...listeners]) listener(now);
}

// The desktop app colours its own title bar from this. It is a courtesy, so a
// failure is dropped and nothing waits on it.
function tellShell() {
  if (desktop()) shellAction("theme", { theme: choice() }).catch(() => {});
}

export function setChoice(next) {
  if (!CHOICES.includes(next)) return;
  write(THEME, next);
  apply();
  tellShell();
}

export function onMode(listener) {
  listeners.add(listener);
  listener(mode());
  return () => listeners.delete(listener);
}

export function start() {
  if (started) return;
  started = true;
  apply();
  query()?.addEventListener?.("change", () => {
    if (choice() === "system") apply();
  });
  tellShell();
}
