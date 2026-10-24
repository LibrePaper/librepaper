// The diagnostics report: plain text a person can read, check and paste.
//
// It is assembled from fields chosen one by one. Document text, file names,
// slugs, share keys, tokens and ids never go in. Ids appear only as a short
// hash, enough to match a server log line and not enough to find the id.
// Nothing here is sent anywhere.

import { desktop } from "../desktop/client.js";
import { PREFIX } from "../build-preferences.js";
import { PANES } from "../panes.js";
import { EDITOR_BRACKETS, EDITOR_CHECK, EDITOR_COMPLETION, EDITOR_FONT, EDITOR_LINE_HEIGHT, EDITOR_SIZE, EDITOR_SPELLCHECK, KEYMAP, KEYS, SOURCE_SIDE, keyFor, read } from "../storage.js";
import { entries } from "./log.js";
import { snapshot } from "./providers.js";
import { scrubMessage } from "./redact.js";

const BUILD_LOG_LINES = 400;

/// Plain text from sections of `{ title, lines }`. A line is a string or a
/// `[key, value]` pair, shown as `key: value`.
export function formatReport(sections) {
  const out = ["LibrePaper diagnostics", new Date().toISOString(), ""];
  for (const { title, lines } of sections) {
    out.push(`## ${title}`);
    for (const line of lines) out.push(Array.isArray(line) ? `${line[0]}: ${line[1]}` : line);
    out.push("");
  }
  return out.join("\n");
}

/// Whether the build log provider has text to offer.
export function hasBuildLog() {
  const text = snapshot().buildLog;
  return typeof text === "string" && text.length > 0;
}

const megabytes = (bytes) => (typeof bytes === "number" ? `${Math.round(bytes / 1048576)} MB` : "unknown");
const yesNo = (value) => (value ? "yes" : "no");

/// The first ten hex characters of the SHA-256 of an id.
async function shortHash(id) {
  try {
    const bytes = new TextEncoder().encode(String(id));
    const digest = await globalThis.crypto.subtle.digest("SHA-256", bytes);
    return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("").slice(0, 10);
  } catch {
    return "unavailable";
  }
}

const wasmThreads = () => {
  try {
    new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
    return true;
  } catch {
    return false;
  }
};

/// Provider values flattened to `[key, value]` pairs, nested objects as `a.b`.
function flatten(value, prefix = "") {
  const pairs = [];
  for (const [name, item] of Object.entries(value || {})) {
    const key = prefix ? `${prefix}.${name}` : name;
    if (item && typeof item === "object" && !Array.isArray(item)) pairs.push(...flatten(item, key));
    else if (typeof item === "string") pairs.push([key, scrubMessage(item, 200)]);
    else pairs.push([key, Array.isArray(item) ? `${item.length} items` : String(item)]);
  }
  return pairs;
}

async function serverVersion() {
  try {
    const response = await globalThis.fetch("/api/config");
    const config = await response.json();
    return typeof config?.version === "string" && config.version ? config.version : "unknown";
  } catch {
    return "unknown";
  }
}

function appSection(server) {
  const shell = desktop();
  return {
    title: "App",
    lines: [
      ["Version", typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : ""],
      ["Commit", typeof __APP_COMMIT__ === "string" ? __APP_COMMIT__ : ""],
      ["Server version", server],
      ["Deployment", globalThis.location?.origin || ""],
      ["Desktop app", shell ? `yes ${shell.version}` : "no"],
    ],
  };
}

async function environmentSection() {
  let estimate = null;
  try {
    estimate = await globalThis.navigator?.storage?.estimate?.();
  } catch { /* storage is not available */ }
  /** @type {Partial<Navigator>} */
  const nav = globalThis.navigator || {};
  const lines = [["User agent", nav.userAgent || ""]];
  if (nav.platform) lines.push(["Platform", nav.platform]);
  lines.push(
    ["Language", nav.language || ""],
    ["Screen", globalThis.screen ? `${screen.width}x${screen.height}` : "unknown"],
    ["Window", `${globalThis.innerWidth ?? "?"}x${globalThis.innerHeight ?? "?"}`],
    ["Pixel ratio", String(globalThis.devicePixelRatio ?? "unknown")],
    ["Cross-origin isolated", yesNo(globalThis.crossOriginIsolated)],
    ["SharedArrayBuffer", yesNo(typeof SharedArrayBuffer !== "undefined")],
    ["Wasm threads", yesNo(wasmThreads())],
    ["Storage used", megabytes(estimate?.usage)],
    ["Storage quota", megabytes(estimate?.quota)],
    ["Online", yesNo(nav.onLine)],
  );
  return { title: "Environment", lines };
}

async function identitySection(state) {
  const lines = [];
  if (state.account?.id) lines.push(["Account", await shortHash(state.account.id)]);
  if (state.document?.id) lines.push(["Document", await shortHash(state.document.id)]);
  for (const [name, value] of [
    ["Format", state.document?.format],
    ["Role", state.document?.role],
    ["May edit", state.document?.mayEdit],
    ["Signed in", state.account?.signedIn],
  ]) {
    if (value !== undefined) lines.push([name, String(value)]);
  }
  return { title: "Identity", lines: lines.length ? lines : ["none"] };
}

function changedSettings() {
  const lines = [];
  const differs = (name, key, fallback) => {
    const value = read(key, fallback);
    if (value !== fallback) lines.push([name, String(value)]);
  };
  differs("Keymap", KEYMAP, "default");
  differs("Source side", SOURCE_SIDE, "left");
  differs("Editor size", EDITOR_SIZE, 14);
  differs("Editor line height", EDITOR_LINE_HEIGHT, "normal");
  // The font name is not reported, only its id, which is one of the built-in choices.
  const font = read(EDITOR_FONT, null);
  const fontId = font && typeof font === "object" ? font.id : null;
  if (fontId && fontId !== "default") lines.push(["Editor font", String(fontId)]);
  differs("Autocomplete", EDITOR_COMPLETION, true);
  differs("Close brackets", EDITOR_BRACKETS, true);
  differs("Spellcheck", EDITOR_SPELLCHECK, true);
  differs("Code check", EDITOR_CHECK, true);
  for (const pane of Object.values(PANES)) {
    const value = read(pane.key, null);
    if (value !== null) lines.push([`Pane ${pane.name}`, String(value)]);
  }
  try {
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (!key?.startsWith(`${PREFIX}:`)) continue;
      let parts;
      try {
        parts = JSON.parse(key.slice(PREFIX.length + 1));
      } catch {
        continue;
      }
      if (!Array.isArray(parts)) continue;
      const value = JSON.parse(localStorage.getItem(key) || "null") || {};
      lines.push([`Build ${scrubMessage(parts[2], 40)}`, `selection=${value.selection ?? "-"} backend=${value.backend ?? "-"}`]);
    }
  } catch { /* unreadable storage: report what was read */ }
  return { title: "Settings changed from defaults", lines: lines.length ? lines : ["none"] };
}

function problemsSection() {
  const lines = [];
  for (const entry of entries().slice(-200)) {
    const time = new Date(entry.at).toISOString();
    if (entry.kind === "request") {
      lines.push(`${time} ${entry.method} ${entry.route} ${entry.status} ${entry.ms}ms id=${entry.requestId}${entry.error ? ` ${entry.error}` : ""}`);
      continue;
    }
    const message = [entry.name, entry.message].filter(Boolean).join(": ");
    lines.push(`${time} ${entry.kind} ${entry.where || ""}: ${message}`);
    for (const frame of entry.stack || []) lines.push(`  ${frame}`);
  }
  return { title: "Recent problems", lines: lines.length ? lines : ["none"] };
}

/// Every share key this page could hold: the stored ones, and the one for the
/// document in the address bar.
function shareKeys() {
  const found = new Set(Object.values(read(KEYS, {})));
  const slug = /^\/docs\/([^/]+)/.exec(globalThis.location?.pathname || "")?.[1];
  if (slug) found.add(keyFor(slug));
  return [...found].filter((key) => typeof key === "string" && key.length >= 8);
}

/// The whole report as text. The build log is included only when asked for,
/// because it can quote the document. `desktopText` is the desktop app's own
/// part, already scrubbed by the shell.
export async function collectReport({ includeBuildLog = false, desktopText = "" } = {}) {
  const state = snapshot();
  const sections = [
    { title: "Note", lines: ["Nothing here is sent anywhere. Check it before you share it."] },
    appSection(await serverVersion()),
    await environmentSection(),
    await identitySection(state),
    { title: "Sync", lines: state.sync ? flatten(state.sync) : ["none"] },
    { title: "Engines", lines: state.engines ? flatten(state.engines) : ["none"] },
    changedSettings(),
    problemsSection(),
  ];
  if (desktopText) sections.push({ title: "Desktop app", lines: [desktopText] });
  if (includeBuildLog && typeof state.buildLog === "string" && state.buildLog) {
    const tail = state.buildLog.split("\n").slice(-BUILD_LOG_LINES);
    sections.push({ title: "Build log (contains document text)", lines: [tail.join("\n")] });
  }
  let text = formatReport(sections);
  for (const key of shareKeys()) text = text.split(key).join("");
  return text;
}
