// How the source text is set for the person at this browser: its font, its
// size and its line height. They are habits of the reader rather than of the
// document, so they are remembered in this browser only.
//
// Nothing here may need a document or a window at load, so the unit tests can
// import it under node.
import { read, write, EDITOR_SIZE, EDITOR_FONT, EDITOR_LINE_HEIGHT } from "./storage.js";

export const DEFAULT_SIZE = 14;
export const MIN_SIZE = 10;
export const MAX_SIZE = 28;

export const DEFAULT_STACK = '"JetBrains Mono", "IBM Plex Mono", "Geist Mono", ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace';

export const FONTS = [
  { id: "default", name: "Default" },
  { id: "jetbrains-mono", name: "JetBrains Mono", family: "JetBrains Mono" },
  { id: "fira-code", name: "Fira Code", family: "Fira Code" },
  { id: "source-code-pro", name: "Source Code Pro", family: "Source Code Pro" },
  { id: "ibm-plex-mono", name: "IBM Plex Mono", family: "IBM Plex Mono" },
  { id: "cascadia-code", name: "Cascadia Code", family: "Cascadia Code" },
  { id: "opendyslexic-mono", name: "OpenDyslexic Mono", family: "OpenDyslexic Mono" },
  { id: "sans", name: "System sans serif", stack: 'system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif' },
  { id: "serif", name: "System serif", stack: 'ui-serif, Georgia, Cambria, "Times New Roman", serif' },
  { id: "custom", name: "Another font" },
];

export const LINE_HEIGHTS = { compact: 1.35, normal: 1.6, wide: 1.9 };

const sizeListeners = new Set();
const fontListeners = new Set();
const lineHeightListeners = new Set();
const TYPE_KEYS = [EDITOR_SIZE, EDITOR_FONT, EDITOR_LINE_HEIGHT];
let started = false;
// The values set on this page. Storage is best effort: a browser that refuses
// the write still gets what it asked for, until another tab writes one.
let chosenSize = null;
let chosenFont = null;
let chosenLineHeight = null;

/// A size in pixels kept between MIN_SIZE and MAX_SIZE. Anything that is not a finite number is DEFAULT_SIZE.
export function clampSize(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return DEFAULT_SIZE;
  return Math.min(MAX_SIZE, Math.max(MIN_SIZE, value));
}

/// The size this browser remembers, or DEFAULT_SIZE. Rounded to a whole pixel and clamped.
export function size() {
  const stored = chosenSize ?? read(EDITOR_SIZE, DEFAULT_SIZE);
  return clampSize(typeof stored === "number" ? Math.round(stored) : DEFAULT_SIZE);
}

/// Remembers a size, clamped and rounded to a whole pixel, and tells the size listeners.
export function setSize(next) {
  chosenSize = Math.round(clampSize(next));
  write(EDITOR_SIZE, chosenSize);
  announceSize();
}

// Hears the size now and whenever it changes. Returns the function that stops it.
export function onSize(listener) {
  sizeListeners.add(listener);
  listener(size());
  return () => sizeListeners.delete(listener);
}

// The font this browser remembers, as { id, custom }. A value this version does
// not offer reads back as the default font.
export function font() {
  const stored = chosenFont ?? read(EDITOR_FONT, null);
  const id = stored && typeof stored === "object" ? stored.id : null;
  if (!FONTS.some((entry) => entry.id === id)) return { id: "default", custom: "" };
  return { id, custom: cleanFamily(String(stored.custom ?? "")) };
}

/// Remembers a font. An id this version does not offer changes nothing. A custom
/// name is cleaned before it is kept.
export function setFont(id, custom = "") {
  if (!FONTS.some((entry) => entry.id === id)) return;
  chosenFont = { id, custom: cleanFamily(String(custom)) };
  write(EDITOR_FONT, chosenFont);
  apply();
  announceFont();
}

// Hears the font now and whenever it changes. Returns the function that stops it.
export function onFont(listener) {
  fontListeners.add(listener);
  listener(font());
  return () => fontListeners.delete(listener);
}

/// A font name made safe to put in a CSS font-family list: letters, digits, spaces, hyphens, underscores and periods, with single spaces and at most 64 characters.
export function cleanFamily(name) {
  const kept = String(name).replace(/[^\p{L}\p{Nd} _.-]/gu, "").replace(/ +/g, " ").trim();
  return [...kept].slice(0, 64).join("");
}

/// The CSS font-family value for a font choice, with the default stack as the fallback.
export function fontStack(choice) {
  const entry = FONTS.find((item) => item.id === choice.id);
  if (!entry || entry.id === "default") return DEFAULT_STACK;
  if (entry.id === "custom") {
    const name = cleanFamily(choice.custom ?? "");
    return name ? `"${name}", ${DEFAULT_STACK}` : DEFAULT_STACK;
  }
  if (entry.stack) return entry.stack;
  return `"${entry.family}", ${DEFAULT_STACK}`;
}

/// The line height this browser remembers: "compact", "normal" or "wide".
export function lineHeight() {
  const stored = chosenLineHeight ?? read(EDITOR_LINE_HEIGHT, "normal");
  return isLineHeight(stored) ? stored : "normal";
}

/// Remembers a line height. A name this version does not offer changes nothing.
export function setLineHeight(next) {
  if (!isLineHeight(next)) return;
  chosenLineHeight = next;
  write(EDITOR_LINE_HEIGHT, next);
  apply();
  announceLineHeight();
}

// Hears the line height now and whenever it changes. Returns the function that stops it.
export function onLineHeight(listener) {
  lineHeightListeners.add(listener);
  listener(lineHeight());
  return () => lineHeightListeners.delete(listener);
}

// Starts following the other tabs of this browser. Idempotent.
export function start() {
  if (started) return;
  started = true;
  apply();
  // A change made in another tab arrives here and is followed. A null key is
  // the whole store being cleared, which drops every copy on this page.
  globalThis.addEventListener?.("storage", (event) => {
    const key = event.key;
    if (key !== null && !TYPE_KEYS.includes(key)) return;
    const touched = (name) => key === null || key === name;
    if (touched(EDITOR_SIZE)) chosenSize = null;
    if (touched(EDITOR_FONT)) chosenFont = null;
    if (touched(EDITOR_LINE_HEIGHT)) chosenLineHeight = null;
    apply();
    if (touched(EDITOR_SIZE)) announceSize();
    if (touched(EDITOR_FONT)) announceFont();
    if (touched(EDITOR_LINE_HEIGHT)) announceLineHeight();
  });
}

function isLineHeight(value) {
  return typeof value === "string" && Object.hasOwn(LINE_HEIGHTS, value);
}

// Puts the font and the line height on the page for the stylesheet to read. The
// font has a name of its own rather than replacing the theme's --font-editor,
// which the theme declares again on whichever element carries it. The default
// font removes its property, so the theme's own font applies.
function apply() {
  const root = globalThis.document?.documentElement;
  if (!root) return;
  const choice = font();
  if (choice.id === "default") root.style.removeProperty("--editor-font-family");
  else root.style.setProperty("--editor-font-family", fontStack(choice));
  root.style.setProperty("--editor-line-height", String(LINE_HEIGHTS[lineHeight()]));
}

function announceSize() {
  const now = size();
  for (const listener of [...sizeListeners]) listener(now);
}

function announceFont() {
  const now = font();
  for (const listener of [...fontListeners]) listener(now);
}

function announceLineHeight() {
  const now = lineHeight();
  for (const listener of [...lineHeightListeners]) listener(now);
}
