// The LibrePaper desktop app, as a browser client.
//
// The desktop app loads this page in its own window and answers requests at a
// base URL it publishes as `window.__LIBREPAPER_DESKTOP__`. It adds the local
// credential and the page's origin itself, so this file holds no address, no
// token and no pairing. Outside the desktop app there is nothing to call and
// every feature call throws `DesktopOnly`. This module answers "run this job
// on the desktop app", nothing about routing or fallback order.
//
// Every dependency this file needs from the ambient browser -- fetch,
// storage, wall clock, the wait between polls -- is reached through `deps`
// rather than the global directly, so `web/tests/integration/latex-local.mjs` can run
// this exact code under Node with no browser at all. `_testing.inject`
// overrides them; `_testing.reset` restores the defaults and clears every
// module-level variable so checks do not leak into each other.

import { bytesOf, toArrayBuffer } from "../bytes.js";
import { named } from "../latex/errors.js";

/** @typedef {{ fetch: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>, storage: Storage | null, now: () => number, wait: (ms: number, signal?: AbortSignal) => Promise<void>, location: () => Location | null, replaceHash: (hash: string) => void }} DesktopDeps */
/** @typedef {{ entrypoint?: string, format?: string, profile?: string | null, parameters?: Record<string, string | number | boolean | null>, policy?: string, kind?: string, inputRevision?: string, inputDigest?: string, renderScope?: string, executionMode?: string, dataInputs?: string[], livePreview?: boolean, [key: string]: unknown }} RenderOptions */
/** @typedef {{ entrypoint?: string, binding?: string, bindingId?: string, idempotencyKey?: string, id?: string, inputDigest?: string, inputRevision?: string, generation?: number, deadlineSeconds?: number, snapshot?: string, [key: string]: unknown }} DesktopJob */
/** @typedef {{ main: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array | ArrayBuffer | string>, digests?: Record<string, string>, files?: Record<string, unknown> }} DesktopTree */
/** @typedef {{ path: string, sha256: string, size: number }} ManifestFile */
/** @typedef {{ signal?: AbortSignal, onProgress?: (progress: { done: number, total: number, scope?: string, stage?: string }) => void, onLog?: (line: string) => void }} BuildCallbacks */
/** @typedef {{ status: string, exit?: number, stage?: string, log_tail?: string, outputs?: Record<string, { size: number, sha256: string }>, diagnostics?: unknown[], provenance?: Record<string, unknown>, error?: string }} DesktopJobStatus */
/** @typedef {{ binding_id: string, main: string, format: string, profile?: string | null, parameters?: Record<string, string | number | boolean | null>, policy?: string, idempotency_key?: string | null, execution_mode?: string, render_scope?: string, data_inputs?: string[], shared_inventory_complete?: boolean }} QuartoJobDetails */
/** @typedef {{ main: string, format: string, binding_id: string }} CalepinJobDetails */
/** @typedef {{ protocol: number, kind: string, project: string | null, origin: string, snapshot: string, generation: number, manifest: ManifestFile[], quarto: QuartoJobDetails, [key: string]: unknown }} QuartoRequestBody */
/** @typedef {{ protocol: number, kind: string, project: string | null, origin: string, snapshot: string, generation: number, manifest: ManifestFile[], options: { deadline_seconds: number }, workspace?: { mode: "bound" | "snapshot", binding_id: string }, [key: string]: unknown }} JobEnvelope */

export const QUARTO_JOB_KINDS = Object.freeze(["render", "refresh", "frozen"]);

/** Where to get the desktop app. Every place that asks for it links here. */
export const INSTALL_URL = "https://librepaper.org/desktop-app.html";

/** What a feature call says in a plain browser. */
export const DESKTOP_ONLY_MESSAGE = "This needs the LibrePaper desktop app.";

const UNREACHABLE_MESSAGE = "The LibrePaper desktop app could not reach its local service. Restart the desktop app.";

// Feature routes and machine management routes, under the desktop base.
const LOCAL_BASE = "librepaper/local/";
const MANAGE_BASE = "companion/api/";

const BINDINGS_KEY = "librepaper-local-quarto-bindings";

const POLL_FAST_MS = 500;
const POLL_SLOW_MS = 1000;
const POLL_SLOW_AFTER_MS = 10 * 1000;

function realWait(ms, signal) {
  return new Promise((resolve) => {
    if (signal?.aborted) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
  });
}

function defaultDeps() {
  return {
    fetch: (input, init) => globalThis.fetch(input, init),
    storage: typeof localStorage !== "undefined" ? localStorage : null,
    now: () => Date.now(),
    wait: realWait,
    location: () => (typeof location !== "undefined" ? location : null),
    replaceHash(hash) {
      if (typeof history === "undefined" || typeof location === "undefined") return;
      const url = new URL(location.href);
      url.hash = hash;
      history.replaceState(null, "", url.href);
    },
  };
}

/** @type {DesktopDeps} */
let deps = defaultDeps();

export const _testing = {
  inject(overrides) {
    Object.assign(deps, overrides);
  },
  reset() {
    deps = defaultDeps();
    current = { project: null, origin: "" };
    capabilitiesCache = null;
  },
};

/** The desktop app's page global, or null in an ordinary browser.
 * @returns {{ base: string, version: string, zoom?: number } | null} */
export function desktop() {
  return globalThis.window?.__LIBREPAPER_DESKTOP__ ?? null;
}

// Asks the desktop shell itself, not the local service, to do something only
// the shell can: the clipboard, the log folder, which server the app opens.
export async function shellAction(name, body) {
  const shell = desktop();
  if (!shell) throw named("DesktopOnly", DESKTOP_ONLY_MESSAGE);
  const response = await fetch(`${shell.base}shell/${name}`, {
    method: "POST",
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.ok) return;
  let message = "";
  try { message = (await response.json())?.error || ""; } catch { /* no body */ }
  throw new Error(message || `The desktop app could not do that (${response.status}).`);
}

// Shows a system notification from the desktop app. The browser has none.
export async function notify(title, body) {
  await shellAction("notify", { title, body });
}

// The desktop app's own part of the diagnostics report, as text.
export async function desktopReport() {
  const shell = desktop();
  if (!shell) throw named("DesktopOnly", DESKTOP_ONLY_MESSAGE);
  const response = await fetch(`${shell.base}shell/diagnostics-report`, { method: "POST" });
  if (response.ok) return String((await response.json())?.text ?? "");
  let message = "";
  try { message = (await response.json())?.error || ""; } catch { /* no body */ }
  throw new Error(message || `The desktop app could not do that (${response.status}).`);
}

// -------------------------------------------------------------- storage

function store() {
  return deps.storage || null;
}

function readRaw(key) {
  const s = store();
  if (!s) return null;
  // Only a string is a stored value: a storage shim that answers with a
  // promise or an object would otherwise be read as one.
  try {
    const value = s.getItem(key);
    return typeof value === "string" ? value : null;
  } catch { return null; }
}

function writeRaw(key, value) {
  const s = store();
  if (!s) return;
  try { s.setItem(key, value); } catch { /* quota or a disabled store; not fatal */ }
}

function readJSON(key, fallback) {
  const raw = readRaw(key);
  if (!raw) return fallback;
  try { return JSON.parse(raw); } catch { return fallback; }
}

function writeJSON(key, value) {
  writeRaw(key, JSON.stringify(value));
}

// The binding every document has without anyone granting one: the desktop app
// renders it in a workspace of its own, written from the files the browser
// sends with each job. A project folder chosen in the desktop app settings
// replaces it for a project that keeps data the document does not share.
export const HOSTED_BINDING = "hosted";

function projectKey() {
  return `${current.origin}|${current.project}`;
}

export function bindingId() {
  const all = readJSON(BINDINGS_KEY, {});
  return String(all[projectKey()] || HOSTED_BINDING);
}

export function setBindingId(id) {
  const all = readJSON(BINDINGS_KEY, {});
  const value = String(id || "").trim();
  if (value) all[projectKey()] = value;
  else delete all[projectKey()];
  writeJSON(BINDINGS_KEY, all);
  return value;
}

let current = { project: null, origin: "" };

/** @param {{ project?: string | null, origin?: string }} [options] */
export function configure({ project, origin } = {}) {
  current = {
    project: project !== undefined ? project : current.project,
    origin: origin !== undefined ? origin : current.origin,
  };
}

// -------------------------------------------------------------- wire helpers

/** The desktop global, or a `DesktopOnly` error in a plain browser. */
function requireDesktop() {
  const shell = desktop();
  if (!shell) throw named("DesktopOnly", DESKTOP_ONLY_MESSAGE);
  return shell;
}

const unavailable = () => named("Unreachable", UNREACHABLE_MESSAGE);

async function sha256hex(bytes) {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

// Shared by `send()` and `localPreviewPage()`. The shell answers 502 when the
// local service cannot be reached, which is `Unreachable`; any other non-2xx
// is `Refused`, carrying the service's own JSON `error` message when it gave
// one, plus its `status`, its `kind` and the parsed `body`. A caller that
// must recognise a 404 specially (`localPreviewPage`'s "not rendered yet")
// checks that before calling this, since a 404 would
// otherwise land in `Refused`.
async function throwOnFailure(response) {
  if (response.ok) return;
  let data = null;
  try {
    data = await response.clone().json();
  } catch { /* not a JSON body; keep the generic message */ }
  const kind = typeof data?.kind === "string" ? data.kind : "";
  // A 502 with a `kind` is Git failing to reach its remote, which the service
  // reports itself. Without one the shell could not reach the service.
  if (response.status === 502 && !kind) throw unavailable();
  const message = typeof data?.error === "string"
    ? data.error
    : `The LibrePaper desktop app refused the request (${response.status})`;
  throw named("Refused", message, { status: response.status, kind, body: data || {} });
}

/// The one place a feature request is sent to the desktop app. A network
/// failure or a 502 is `Unreachable`, any other non-2xx is `Refused` with the
/// service's own message when it gave one.
/** @param {string} method @param {string} path @param {{ jsonBody?: unknown, formBody?: FormData, signal?: AbortSignal }} [options] */
async function send(method, path, { jsonBody, formBody, signal } = {}) {
  const shell = requireDesktop();
  const headers = { Accept: "application/json" };
  let body;
  if (formBody !== undefined) {
    // WebKitGTK crashes the desktop app when a request through its URL
    // scheme carries a Blob, and a form's files are Blobs. The multipart body
    // is encoded here into plain bytes, with its boundary in the content type.
    const encoded = new Response(formBody);
    headers["Content-Type"] = encoded.headers.get("content-type") || "multipart/form-data";
    body = await encoded.arrayBuffer();
  } else if (jsonBody !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(jsonBody);
  }
  let response;
  try {
    response = await deps.fetch(shell.base + LOCAL_BASE + path, { method, mode: "cors", credentials: "omit", headers, body, signal });
  } catch {
    if (signal?.aborted) throw named("Canceled", "Canceled");
    throw unavailable();
  }
  await throwOnFailure(response);
  return response;
}

async function bytesOfResponse(response) {
  return new Uint8Array(await response.arrayBuffer());
}

/** Call a machine management route.
 * @param {string} path @param {{ method?: string, body?: unknown }} [options] */
export async function manage(path, { method = "GET", body } = {}) {
  if (typeof path !== "string" || !path.startsWith("/") || path.startsWith("//") || path.includes("\\")) {
    throw new TypeError("Invalid management path");
  }
  const shell = requireDesktop();
  /** @type {Record<string, string>} */
  const headers = { Accept: "application/json" };
  let payload;
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
    payload = JSON.stringify(body);
  }
  let response;
  try {
    response = await deps.fetch(`${shell.base}${MANAGE_BASE}${path.slice(1)}`, {
      method, mode: "cors", credentials: "omit", cache: "no-store", headers, body: payload,
    });
  } catch {
    throw unavailable();
  }
  if (response.status === 502) throw unavailable();
  if (response.status === 403) throw named("Refused", "This is not available for this site.", { status: 403 });
  if (!response.ok) {
    let message = `The desktop app request failed (${response.status}).`;
    try {
      const data = await response.json();
      if (typeof data?.error === "string" && data.error) message = data.error;
    } catch { /* an empty or non-JSON error response has no detail */ }
    throw named("RequestFailed", message, { status: response.status });
  }
  if (response.status === 204) return null;
  const type = response.headers?.get?.("content-type") || "";
  return type.includes("json") ? response.json() : response.text();
}

// The desktop app's `--settings` argument opens the page with a
// `settings=local` fragment. The field is stripped from the address bar
// whatever it says. Returns whether Settings was requested.
export function intake() {
  const loc = deps.location();
  const hash = String(loc?.hash || "").replace(/^#/, "");
  if (!hash) return false;
  const params = new URLSearchParams(hash);
  if (params.get("settings") !== "local") return false;
  params.delete("settings");
  deps.replaceHash(params.toString());
  return true;
}

// The tools found on this computer, fetched once per page load. A rescan
// replaces the answer.
let capabilitiesCache = null;

export async function capabilities({ rescan = false } = {}) {
  if (capabilitiesCache && !rescan) return capabilitiesCache;
  const method = rescan ? "POST" : "GET";
  const path = rescan ? "capabilities/rescan" : "capabilities";
  const response = await send(method, path);
  capabilitiesCache = await response.json();
  return capabilitiesCache;
}

/** Forgets the tools found, so the next `capabilities()` asks the desktop app again. */
export function forgetCapabilities() {
  capabilitiesCache = null;
}

/** Whether the desktop app found Calepin. */
export async function calepinAvailable() {
  const found = await capabilities();
  return Boolean(found?.calepin?.found);
}

/** Search the running Zotero desktop library through the desktop app.
 * Results remain transient: this client does not place queries or metadata in
 * browser storage. */
export async function searchZotero(query) {
  const path = `zotero/search?${new URLSearchParams({ q: String(query || "") })}`;
  const response = await send("GET", path);
  return response.json();
}

/** Fetch one Zotero item as deterministic BibTeX. */
export async function zoteroItem(key) {
  const value = String(key || "");
  if (!/^[A-Za-z0-9]{1,32}$/.test(value)) throw named("InvalidRequest", "Invalid Zotero item key");
  const response = await send("GET", `zotero/items/${value}`);
  return response.json();
}

/** Ask the desktop app to open its native directory chooser and bind the
 * selected project folder. The absolute path never crosses the wire. */
export async function chooseFolderBinding({ entrypoint = "" } = {}) {
  const response = await send("POST", "bindings/folder", {
    jsonBody: { project: current.project, entrypoint: String(entrypoint || "") },
  });
  const data = await response.json();
  if (data?.id) setBindingId(data.id);
  return data;
}

/// One typed Git operation of the desktop app's local service, for the
/// current project. The answer is the parsed JSON. A refusal carries `status`,
/// `kind` and `body`, so callers can tell a moved branch from a failed login.
export async function gitRequest(op, body = {}) {
  const response = await send("POST", `git/${op}`, { jsonBody: { ...body, project: current.project } });
  return response.json();
}

// -------------------------------------------------------------- multipart job bodies

function formOf(jobRequest, files) {
  const form = new FormData();
  form.append("job", new Blob([JSON.stringify(jobRequest)], { type: "application/json" }), "job.json");
  for (const [path, bytes] of files) {
    form.append("file", new Blob([toArrayBuffer(bytes)]), path);
  }
  return form;
}

function relativePath(path) {
  const value = String(path || "").replaceAll("\\", "/");
  const parts = value.split("/");
  if (!value || value.startsWith("/") || /^[A-Za-z]:/.test(value) ||
      /[\u0000-\u001f\u007f]/.test(value) || parts.some((part) => !part || part === "." || part === "..")) {
    throw new Error(`invalid project path: ${path}`);
  }
  return value;
}

function boundedString(value, name, max) {
  const text = String(value || "");
  if (text.length < 1 || text.length > max) throw new Error(`invalid Quarto ${name}`);
  return text;
}

function safeSha256(value, name = "digest") {
  const text = String(value || "");
  if (!/^[0-9a-f]{64}$/i.test(text)) throw new Error(`invalid Quarto ${name}`);
  return text.toLowerCase();
}

function safeSize(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > 64 * 1024 * 1024) {
    throw new Error("invalid Quarto file size");
  }
  return value;
}

/** @param {string} kind @param {string | undefined} policy */
function quartoPolicy(kind, policy) {
  if (!QUARTO_JOB_KINDS.includes(kind)) throw new Error(`unsupported Quarto job kind: ${kind}`);
  const allowed = ["project-defaults", "refresh-computations", "frozen"];
  const value = policy || (kind === "refresh" ? "refresh-computations" : kind === "frozen" ? "frozen" : "project-defaults");
  if (!allowed.includes(value)) throw new Error(`unsupported Quarto render policy: ${value}`);
  return value;
}

// Same top-level envelope the bridge expects for every preview/render request
// on either engine (`protocol`, `kind`, `project`, `origin`, `snapshot`,
// `generation`, `manifest`, `options`) -- `quartoRequest` layers `quarto`
// alongside it, `buildCalepinForm` layers `engine`/`calepin` the same way,
// and `kind` stays the value the Rust `JobRequest` already deserializes
// today for both.
/** @param {{ job: DesktopJob, manifest: ManifestFile[], inputRevision?: string }} input */
function jobEnvelope({ job, manifest, inputRevision }) {
  return {
    protocol: 2,
    kind: "quarto",
    project: current.project,
    origin: current.origin,
    snapshot: String(inputRevision || job.inputRevision || ""),
    generation: Number.isFinite(job.generation) ? Math.max(0, Math.floor(job.generation)) : 0,
    manifest,
    // No `max_passes`: it bounded latexmk's reruns, and LaTeX is built in
    // the browser now. Nothing on either side has read it since.
    options: {
      deadline_seconds: Number.isFinite(job.deadlineSeconds) ? Math.max(1, Math.floor(job.deadlineSeconds)) : 300,
    },
  };
}

/** Build the JSON part of a Quarto request without accepting shell fragments. */
/** @param {{ job?: DesktopJob, entrypoint?: string, format?: string, profile?: string | null, parameters?: Record<string, string | number | boolean | null>, policy?: string, kind?: string, inputRevision?: string, inputDigest?: string, files?: ManifestFile[] }} [options] */
export function quartoRequest({ job = {}, entrypoint, format = "html", profile = null, parameters = {}, policy, kind = "render", inputRevision = "", inputDigest = "", files = [] } = {}) {
  const main = relativePath(entrypoint || job.entrypoint || "");
  if (!main.endsWith(".qmd") && !main.endsWith(".md")) throw new Error("Quarto entrypoint must be a .qmd or .md file");
  if (!["html", "pdf", "docx", "revealjs"].includes(String(format))) throw new Error("invalid Quarto output format");
  const binding = boundedString(job.binding || job.bindingId || "", "binding");
  if (!/^[A-Za-z0-9._:-]+$/.test(binding)) throw new Error("invalid Quarto binding");
  if (profile != null && (!/^[A-Za-z0-9._-]+$/.test(String(profile)) || String(profile).length > 128)) {
    throw new Error("invalid Quarto profile");
  }
  const idempotency = String(job.idempotencyKey || job.id || "");
  if (idempotency && (idempotency.length > 256 || !/^[A-Za-z0-9._:-]+$/.test(idempotency))) {
    throw new Error("invalid Quarto idempotency key");
  }
  const sourceDigest = inputDigest || job.inputDigest || "";
  if (sourceDigest) safeSha256(sourceDigest, "shared tree digest");
  if (Object.keys(parameters || {}).length > 128) throw new Error("too many Quarto parameters");
  /** @type {Record<string, string | number | boolean | null>} */
  const publicParameters = {};
  for (const [key, value] of Object.entries(parameters || {})) {
    if (key.length > 128 || !/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(key)) throw new Error(`invalid Quarto parameter: ${key}`);
    if (typeof value === "number" && (!Number.isFinite(value) || Object.is(value, -0) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) {
      throw new Error(`Quarto parameter number is outside the portable range: ${key}`);
    }
    if (["string", "number", "boolean"].includes(typeof value) || value === null) {
      if (new TextEncoder().encode(String(value)).byteLength > 16 * 1024) throw new Error(`Quarto parameter is too long: ${key}`);
      Object.defineProperty(publicParameters, key, { value, enumerable:true, configurable:true, writable:true });
    } else throw new Error(`invalid Quarto parameter: ${key}`);
  }
  const manifest = [];
  const seen = new Set();
  for (const file of files) {
    const path = relativePath(file.path);
    if (seen.has(path)) throw new Error(`duplicate Quarto project path: ${path}`);
    seen.add(path);
    manifest.push({ path, sha256: safeSha256(file.sha256, "file digest"), size: safeSize(file.size) });
  }
  return {
    ...jobEnvelope({ job, manifest, inputRevision }),
    quarto: {
      binding_id: binding,
      main,
      format: String(format),
      profile: profile == null ? null : String(profile),
      // Preserve scalar JSON types. Rust serializes these exact values for
      // Quarto's -P arguments and hashes the typed map, so 1, true, null and
      // the string "1" remain distinct cache identities.
      parameters: publicParameters,
      policy: quartoPolicy(kind, policy),
      idempotency_key: idempotency || null,
      shared_tree_sha256: sourceDigest ? safeSha256(sourceDigest, "shared tree digest") : null,
      execution_mode: "working-tree",
      render_scope: "document",
      data_inputs: [],
      shared_inventory_complete: false,
    },
  };
}

// Turn a shared tree's `texts`/`assets` maps into `[relativePath, bytes]`
// pairs, validating and de-duplicating paths the same way for every caller
// that walks a tree -- the Quarto job/preview form, and `syncWorkspace`.
/** @param {DesktopTree} tree @returns {Array<[string, Uint8Array]>} */
function collectTreeFiles(tree) {
  const files = [];
  const seen = new Set();
  const add = (path, bytes) => {
    const safe = relativePath(path);
    if (seen.has(safe)) throw new Error(`duplicate Quarto project path: ${safe}`);
    seen.add(safe);
    files.push([safe, bytes]);
  };
  for (const [path, text] of Object.entries(tree?.texts || {})) add(path, bytesOf(text));
  for (const [path, bytes] of Object.entries(tree?.assets || {})) add(path, bytesOf(bytes));
  return files;
}

export const CALEPIN_FORMATS = Object.freeze(["html", "pdf"]);

/** Validate the Calepin-specific options (`entrypoint`, `format`), the same
 * way `quartoRequest` validates a Quarto job's shape before anything is
 * built from the tree. */
/** @param {{ entrypoint?: string, format?: string }} [options] */
function calepinOptions({ entrypoint, format = "html" } = {}) {
  const main = relativePath(entrypoint || "");
  if (!main.endsWith(".typ")) throw new Error("Calepin entrypoint must be a .typ file");
  if (!CALEPIN_FORMATS.includes(String(format))) throw new Error("invalid Calepin output format");
  return { main, format: String(format) };
}

/** @param {{ job?: DesktopJob, tree: DesktopTree, options?: RenderOptions }} input */
async function buildCalepinForm({ job = {}, tree, options = {} }) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const { main, format } = calepinOptions(options);
  if (!manifest.some((file) => file.path === main)) {
    throw new Error(`Calepin project is missing its entrypoint: ${main}`);
  }
  const binding = boundedString(job.binding || job.bindingId || "", "binding");
  if (!/^[A-Za-z0-9._:-]+$/.test(binding)) throw new Error("invalid Calepin binding");
  /** @type {JobEnvelope & { engine: "calepin", calepin: CalepinJobDetails }} */
  const request = {
    ...jobEnvelope({ job, manifest, inputRevision: options.inputRevision }),
    engine: "calepin",
    calepin: { binding_id: binding, main, format },
  };
  request.workspace = options.livePreview
    ? { mode: "bound", binding_id: binding }
    : { mode: "snapshot", binding_id: binding };
  return formOf(request, files);
}

/** @param {{ job?: DesktopJob, tree: DesktopTree, options?: RenderOptions }} input */
async function buildQuartoForm({ job, tree, options = {} }) {
  if (options.renderScope === "project") {
    throw new Error("Quarto website and book project renders are not supported; render one document instead");
  }
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  /** @type {QuartoRequestBody} */
  const request = quartoRequest({
    job, entrypoint: options.entrypoint || tree?.main, format: options.format || "html",
    profile: options.profile, parameters: options.parameters, policy: options.policy,
    kind: options.kind || "render", inputRevision: options.inputRevision, inputDigest: options.inputDigest,
    files: manifest,
  });
  const source = await sourceSnapshot(
    request.snapshot,
    request.quarto.main,
    manifest,
  );
  if (source) request.source = source;
  // One-shot renders use a fresh temporary project copy. The binding
  // authorizes which local inputs may be copied; only live previews run in
  // the bound working directory.
  request.workspace = options.livePreview
    ? { mode: "bound", binding_id: request.quarto.binding_id }
    : { mode: "snapshot", binding_id: request.quarto.binding_id };
  request.quarto.execution_mode = options.livePreview ? "working-tree" : "temporary-copy";
  if (!manifest.some((file) => file.path === request.quarto.main)) {
    throw new Error(`Quarto project is missing its entrypoint: ${request.quarto.main}`);
  }
  if (options.executionMode === "isolated-snapshot") {
    request.quarto.execution_mode = "isolated-snapshot";
    request.quarto.shared_inventory_complete = true;
    request.quarto.data_inputs = (options.dataInputs || []).map(relativePath);
  }
  return formOf(request, files);
}

/** @param {Array<[string, Uint8Array]>} files @returns {Promise<ManifestFile[]>} */
async function manifestOf(files) {
  const manifest = [];
  for (const [path, bytes] of files) {
    manifest.push({ path, sha256: await sha256hex(bytes), size: bytes.byteLength });
  }
  return manifest;
}

// Identity of the immutable project materialized for a runner. The CRDT tree
// digest and exact byte inventory have different meanings and travel as such.
async function sourceSnapshot(treeDigest, main, manifest) {
  const digest = String(treeDigest || "");
  if (!/^[0-9a-f]{64}$/.test(digest)) return null;
  const canonical = [...manifest].sort((left, right) => left.path.localeCompare(right.path));
  return {
    tree_sha256: digest,
    main_path: relativePath(main),
    manifest_sha256: await sha256hex(new TextEncoder().encode(JSON.stringify(canonical))),
  };
}

// -------------------------------------------------------------- job polling

async function pollJob(id, signal) {
  const start = deps.now();
  for (;;) {
    if (signal?.aborted) {
      throw named("Canceled", "Canceled");
    }
    const response = await send("GET", `jobs/${id}`, { signal });
    const status = await response.json();
    if (status.status === "done" || status.status === "failed" || status.status === "canceled") return status;
    const waitMs = deps.now() - start > POLL_SLOW_AFTER_MS ? POLL_SLOW_MS : POLL_FAST_MS;
    await deps.wait(waitMs, signal);
  }
}

async function submitAndAwait(form, signal) {
  const submitted = await send("POST", "jobs", { formBody: form }).then((r) => r.json());
  let status;
  try {
    status = await pollJob(submitted.id, signal);
  } catch (error) {
    if (signal?.aborted) {
      await send("POST", `jobs/${submitted.id}/cancel`).catch(() => {});
    }
    throw error;
  }
  if (status.status === "canceled") {
    throw named("Canceled", "Canceled");
  }
  return { id: submitted.id, status };
}

/** @param {string} id @param {string} name @param {DesktopJobStatus} status */
async function fetchOutput(id, name, status) {
  if (!status.outputs?.[name]) return null;
  const response = await send("GET", `jobs/${id}/files/${encodeURIComponent(name)}`, { });
  return bytesOfResponse(response);
}

/** @param {string} id @param {string} name @param {DesktopJobStatus} status */
async function fetchVerifiedOutput(id, name, status) {
  const descriptor = status.outputs?.[name];
  if (!descriptor) return null;
  const expectedSize = safeSize(Number(descriptor.size));
  const expectedDigest = safeSha256(descriptor.sha256, `${name} output digest`);
  const bytes = await fetchOutput(id, name, status);
  if (!bytes || bytes.byteLength !== expectedSize) {
    throw named("Refused", `Local builder returned the wrong size for ${name}`);
  }
  if (await sha256hex(bytes) !== expectedDigest) {
    throw named("Refused", `Local builder returned the wrong digest for ${name}`);
  }
  return bytes;
}

/** @param {Uint8Array} bytes */
function base64Of(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

// -------------------------------------------------------------- jobs

/** @param {{ job?: DesktopJob, tree: DesktopTree, builder: string, engine?: string, output?: string, options?: Record<string, unknown>, bindingId?: string }} input @param {BuildCallbacks} [callbacks] */
export async function runBuild({ job = {}, tree, builder, engine, output = "pdf", options = {}, bindingId = "" }, { signal, onProgress } = {}) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const request = {
    protocol: 2, kind: "build", project: current.project, origin: current.origin,
    snapshot: String(job.snapshot || ""), generation: Number(job.generation || 0),
    builder, workspace: { mode: "snapshot", ...(bindingId ? { binding_id: bindingId } : {}) }, entrypoint: relativePath(tree.main), output,
    options: { ...options, ...(engine ? { engine } : {}) }, manifest,
  };
  const source = await sourceSnapshot(job.snapshot, tree.main, manifest);
  if (source) request.source = source;
  onProgress?.({ done: 0, total: 1, scope: `local ${builder}` });
  const { id, status: jobStatus } = await submitAndAwait(formOf(request, files), signal);
  if (builder === "quarto") return collectQuartoOutputs(id, jobStatus, { format: output }, { onProgress }, job);
  const artifact = await fetchVerifiedOutput(id, output, jobStatus) || await fetchVerifiedOutput(id, `artifact.${output}`, jobStatus);
  const log = await fetchVerifiedOutput(id, "log", jobStatus);
  onProgress?.({ done: 1, total: 1, scope: `local ${builder}` });
  return { ok: jobStatus.status === "done" && jobStatus.exit === 0 && artifact != null, kind: output, pdf: output === "pdf" ? artifact : null, artifact, log: log ? new TextDecoder().decode(log) : "", diagnostics: jobStatus.diagnostics || [], exit: jobStatus.exit, error: jobStatus.error, provenance: { ...(jobStatus.provenance || {}), backend: "local", builder } };
}

/** @param {string} id @param {DesktopJobStatus} status @param {{ format?: string, policy?: string, inputRevision?: string }} options @param {BuildCallbacks} [callbacks] @param {DesktopJob} [job] */
async function collectQuartoOutputs(id, status, options, { onProgress, onLog } = {}, job = {}) {
  if (status.log_tail) for (const line of String(status.log_tail).split("\n")) onLog?.(line);
  const format = options.format || "html";
  const artifactName = `artifact.${format}`;
  const manifestBytes = await fetchOutput(id, "manifest.json", status) ||
    await fetchOutput(id, "quarto-bundle.json", status);
  const logBytes = await fetchOutput(id, "log", status);
  let manifest = null;
  if (manifestBytes) {
    try { manifest = JSON.parse(new TextDecoder().decode(manifestBytes)); } catch { onLog?.("Quarto collector returned an invalid manifest"); }
  }
  const blobs = new Map();
  let artifact = null;
  let closureError = null;
  if (manifest) {
    if (!manifest.artifact || typeof manifest.artifact !== "object") {
      closureError = "Quarto bundle is missing its artifact descriptor";
    }
    const descriptors = [];
    if (!closureError) {
      try {
        const artifactPath = relativePath(manifest.artifact.entrypoint);
        const artifactDigest = safeSha256(manifest.artifact.sha256, "artifact digest");
        const expectedKind = format === "pdf" ? "pdf" : format === "docx" ? "docx" : "html";
        if (String(manifest.artifact.kind || "").toLowerCase() !== expectedKind) throw new Error("Quarto artifact kind does not match its requested format");
        descriptors.push({ artifact: true, path: artifactPath, sha256: artifactDigest, size: safeSize(Number(manifest.artifact.size)), mime: manifest.artifact.mime || (format === "pdf" ? "application/pdf" : format === "docx" ? "application/vnd.openxmlformats-officedocument.wordprocessingml.document" : "text/html; charset=utf-8") });
        if (!Array.isArray(manifest.assets)) throw new Error("Quarto bundle assets must be an array");
        const descriptorPaths = new Set([artifactPath]);
        for (const asset of manifest.assets) {
          const path = relativePath(asset?.path);
          if (descriptorPaths.has(path)) throw new Error(`duplicate Quarto bundle path: ${path}`);
          descriptorPaths.add(path);
          descriptors.push({ ...asset, path, artifact: false, sha256: safeSha256(asset?.sha256, "asset digest"), size: safeSize(Number(asset?.size)) });
        }
      } catch (error) {
        closureError = error.message;
      }
    }
    const totalLimit = 64 * 1024 * 1024;
    let totalBytes = 0;
    for (const descriptor of descriptors) {
      if (closureError) break;
      const outputName = descriptor.artifact ? artifactName : `asset:${descriptor.path}`;
      const output = status.outputs?.[outputName];
      if (!output) {
        closureError = `Quarto bundle is missing required output: ${outputName}`;
        break;
      }
      const advertisedSize = Number(output.size);
      if (!Number.isSafeInteger(advertisedSize) || advertisedSize < 0 || advertisedSize > totalLimit || totalBytes + advertisedSize > totalLimit) {
        closureError = `Quarto bundle output exceeds the size limit: ${outputName}`;
        break;
      }
      const bytes = await fetchOutput(id, outputName, status);
      if (!bytes) {
        closureError = `Quarto bundle output could not be downloaded: ${outputName}`;
        break;
      }
      if (bytes.byteLength !== advertisedSize) {
        closureError = `Quarto bundle output has the wrong size: ${outputName}`;
        break;
      }
      if (bytes.byteLength !== descriptor.size) {
        closureError = `Quarto bundle descriptor has the wrong size: ${outputName}`;
        break;
      }
      totalBytes += bytes.byteLength;
      if (descriptor.sha256) {
        const actual = await sha256hex(bytes);
        if (actual !== descriptor.sha256) {
          closureError = `Quarto bundle output has the wrong digest: ${outputName}`;
          break;
        }
        if (descriptor.artifact) artifact = bytes;
        const mime = descriptor.mime || "application/octet-stream";
        const existing = blobs.get(descriptor.sha256);
        if (existing && existing.mime !== mime) {
          closureError = `Quarto bundle digest has conflicting MIME types: ${descriptor.sha256}`;
          break;
        }
        if (!existing) blobs.set(descriptor.sha256, { sha256: descriptor.sha256, mime, data: base64Of(bytes) });
      }
    }
  }
  if (!manifest && status.status === "done" && status.exit === 0) closureError = "Quarto render did not return a bundle manifest";
  if (closureError) onLog?.(closureError);
  onProgress?.({ done: 1, total: 1, scope: "local Quarto render", stage: "collecting" });
  return {
    id,
    ok: status.status === "done" && status.exit === 0 && artifact != null && !closureError,
    artifact,
    kind: format === "pdf" ? "pdf" : format === "docx" ? "docx" : "html",
    manifest,
    publish: manifest && !closureError ? { manifest, blobs: [...blobs.values()], select: true } : null,
    diagnostics: status.diagnostics || [],
    logs: logBytes ? new TextDecoder().decode(logBytes) : status.log_tail || "",
    exit: status.exit,
    stage: status.stage || (status.status === "done" ? "published" : status.status),
    provenance: {
      ...(status.provenance || {}),
      backend: "local",
      policy: options.policy || "project-defaults",
      input_revision: options.inputRevision || job.inputRevision || null,
    },
    ...(status.error ? { error: status.error } : {}),
    ...(closureError ? { error: closureError } : {}),
  };
}

/** Start a live preview on either engine. For `quarto` this produces the
 * Quarto preview JSON; for `calepin`, `options` is the
 * `{ entrypoint, format }` pair validated by `calepinOptions`. */
/** @param {{ engine?: string, job?: DesktopJob, tree: DesktopTree, options?: RenderOptions }} input @returns {Promise<{ id: string, url: string }>} */
export async function startLocalPreview({ engine = "quarto", job = {}, tree, options = {} }) {
  const form = engine === "calepin"
    ? await buildCalepinForm({ job, tree, options: { ...options, livePreview: true } })
    : await buildQuartoForm({ job, tree, options: { ...options, livePreview: true } });
  const jobPart = form.get("job");
  if (!(jobPart instanceof Blob)) throw new Error("local preview request is missing its job manifest");
  /** @type {JobEnvelope & { quarto?: QuartoJobDetails, calepin?: CalepinJobDetails }} */
  const built = JSON.parse(await jobPart.text());
  // Unconditionally protocol 2, like `runBuild`: the bridge has no other
  // shape to decode a preview from.
  const builder = engine === "calepin" ? "calepin" : "quarto";
  const details = engine === "calepin" ? built.calepin : built.quarto;
  if (!details) throw new Error(`local preview request is missing ${builder} job details`);
  const output = details.format || options.format || "html";
  const entrypoint = details.main || options.entrypoint;
  const previewOptions = engine === "quarto" && built.quarto
    ? {
        ...(built.quarto.profile ? { profile: built.quarto.profile } : {}),
        parameters: built.quarto.parameters || {},
        policy: built.quarto.policy || "project-defaults",
      }
    : {};
  const request = {
    protocol: 2, kind: "preview", project: built.project, origin: built.origin,
    snapshot: built.snapshot, generation: built.generation, builder,
    workspace: { mode: "bound", binding_id: details.binding_id }, entrypoint, output,
    manifest: built.manifest,
    options: previewOptions,
  };
  const source = await sourceSnapshot(built.snapshot, entrypoint, built.manifest);
  if (source) request.source = source;
  const response = await send("POST", "previews", { jsonBody: request });
  return response.json();
}
/** @param {string} id @returns {Promise<void>} */
export async function stopLocalPreview(id) {
  await send("DELETE", `previews/${encodeURIComponent(id)}`);
}
/** @param {string} id @returns {Promise<{ state?: string, log_tail?: string, error?: string | null }>} */
export async function localPreviewStatus(id) {
  const response = await send("GET", `previews/${encodeURIComponent(id)}`);
  return response.json();
}

// Every response on this route -- 200, 304 and 404 alike -- carries
// `x-librepaper-rendering: true|false` saying whether Quarto is currently
// re-rendering (the intermediate output itself is never served; this header
// is the only signal a poller has that a newer page is on the way). Read the
// header name case-insensitively since callers may hand this a plain object
// rather than a real `Headers` instance; a missing header is `null` rather
// than a guessed boolean.
function headerOf(response, name) {
  const headers = response?.headers;
  if (!headers || typeof headers.get !== "function") return null;
  let raw = headers.get(name);
  if (raw == null) {
    // Fall back to scanning for a differently-cased key when the shim's
    // `get` is not itself case-insensitive.
    const variants = [name, name.toLowerCase(), name.toUpperCase(),
      name.replace(/(^|-)([a-z])/g, (_, sep, ch) => sep + ch.toUpperCase())];
    for (const variant of variants) {
      raw = headers.get(variant);
      if (raw != null) break;
    }
  }
  return raw == null ? null : String(raw);
}

function renderingHeaderOf(response) {
  const raw = headerOf(response, "x-librepaper-rendering");
  if (raw == null) return null;
  const value = raw.trim().toLowerCase();
  if (value === "true") return true;
  if (value === "false") return false;
  return null;
}

function kindHeaderOf(response) {
  const raw = headerOf(response, "x-librepaper-kind");
  if (raw == null) return null;
  const value = raw.trim().toLowerCase();
  return value === "pdf" || value === "html" ? value : null;
}

// The live preview's own rendered page. `etag`, when given, is sent as
// `If-None-Match`: a 304 (nothing new since that version) resolves `null`
// rather than re-fetching bytes nothing needs. A 404 means the preview has
// not produced a first render yet, which the poller treats as "not yet" --
// distinguished by `name` from every other failure, none of which are.
/** @param {string} id @param {{ etag?: string | null }} [options] @returns {Promise<{kind?: "html" | "pdf", html?: string, bytes?: Uint8Array, etag?: string | null, rendering?: boolean}>} */
export async function localPreviewPage(id, { etag } = {}) {
  const shell = requireDesktop();
  const url = `${shell.base}${LOCAL_BASE}previews/${encodeURIComponent(id)}/page`;
  const headers = { Accept: "text/html, application/pdf" };
  if (etag) headers["If-None-Match"] = etag;
  let response;
  try {
    response = await deps.fetch(url, { method: "GET", mode: "cors", credentials: "omit", headers });
  } catch {
    throw unavailable();
  }
  const rendering = renderingHeaderOf(response);
  if (response.status === 304) return { rendering };
  if (response.status === 404) {
    throw named("NotRendered", "not rendered yet", { rendering });
  }
  await throwOnFailure(response);
  const etagOut = response.headers?.get?.("etag") || null;
  const contentType = response.headers?.get?.("content-type") || "";
  const isPdf = kindHeaderOf(response) === "pdf" || /application\/pdf/i.test(contentType);
  if (isPdf) {
    const bytes = await bytesOfResponse(response);
    return { kind: "pdf", bytes, etag: etagOut, rendering };
  }
  const html = await response.text();
  return { kind: "html", html, etag: etagOut, rendering };
}
// -------------------------------------------------------------- workspace sync

/** @param {DesktopTree} tree */
async function buildWorkspaceForm(tree) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const form = new FormData();
  form.append("manifest", new Blob([JSON.stringify(manifest)], { type: "application/json" }), "manifest.json");
  for (const [path, bytes] of files) form.append("file", new Blob([toArrayBuffer(bytes)]), path);
  return form;
}

// Pushes the shared tree to the desktop app's hosted workspace, or to the
// project folder the user chose, so a running preview sees the current files,
// without going through the job queue. Same file layout as a job's multipart
// body, but the JSON part is named `manifest` (a bare array) rather than `job`.
/** @param {{ tree: DesktopTree }} input */
export async function syncWorkspace({ tree }) {
  const form = await buildWorkspaceForm(tree);
  const params = new URLSearchParams({ project: String(current.project) });
  const binding = bindingId();
  if (binding !== HOSTED_BINDING) params.set("binding", binding);
  try {
    const response = await send("PUT", `workspace?${params}`, { formBody: form });
    return response.json();
  } catch (error) {
    // A saved folder this desktop app no longer grants (revoked, or another
    // install) is forgotten, and the preview falls back to the hosted
    // workspace instead of failing on every sync.
    if (binding === HOSTED_BINDING || error?.name !== "Refused" || ![403, 404].includes(error?.status)) throw error;
    setBindingId("");
    return syncWorkspace({ tree });
  }
}

/* -------------------------------------------- Backups */

/** Read account-wide backup state from the desktop app. The account id
 * is the signed-in server account, never the currently open project. */
export async function backups(accountId) {
  const query = new URLSearchParams({ account: String(accountId || "") });
  const response = await send("GET", `backups?${query}`);
  return response.json();
}

/** Ask the desktop app to choose and retain a native backup destination. */
export async function chooseBackupFolder(accountId) {
  const response = await send("POST", "backups/folder", {
    jsonBody: { account_id: String(accountId || "") },
  });
  return response.json();
}

/** Authorize access through the signed-in browser. `approveCode` is the one
 * browser-server request; `stillCurrent` checks its account and UI scope. */
export async function authorizeBackups(accountId, approveCode, stillCurrent = () => true) {
  const response = await send("POST", "backups/authorize", {
    jsonBody: { account_id: String(accountId || "") },
  });
  const { user_code: userCode, authorization_id: authorizationId } = await response.json();
  if (!userCode || !authorizationId || !stillCurrent()) throw named("Canceled", "The backup authorization scope changed.");
  await approveCode(userCode, stillCurrent);
  if (!stillCurrent()) throw named("Canceled", "The backup authorization scope changed.");
  const completed = await send("POST", "backups/authorize/complete", {
    jsonBody: { account_id: String(accountId || ""), authorization_id: String(authorizationId) },
  });
  if (!stillCurrent()) throw named("Canceled", "The backup authorization scope changed.");
  return completed.json();
}

/** Update account-wide backup scheduling. */
export async function updateBackups(accountId, { enabled, frequency_minutes }) {
  const response = await send("PUT", "backups", {
    jsonBody: { account_id: String(accountId || ""), enabled: Boolean(enabled), frequency_minutes },
  });
  return response.json();
}

/** Run an account-wide backup immediately. */
export async function runBackups(accountId) {
  const response = await send("POST", "backups/run", {
    jsonBody: { account_id: String(accountId || "") },
  });
  return response.json();
}

/** Change the command that runs Quarto or Calepin. The desktop app asks the
 * person to confirm first and refuses with 403 when they cancel. */
export async function setIntegration(name, { path, args }) {
  const response = await send("PUT", `integrations/${encodeURIComponent(name)}`, { jsonBody: { path, args } });
  return response.json();
}

/* --------------------------------------------------- Connecting an agent */

/** One call to the desktop app's assistant surface, JSON in and
 * out. `agents`, `startAssistant`, `assistantStatus` and `stopAssistant` are
 * all this same shape; only the method, path and body differ. */
async function assistantCall(method, path, jsonBody) {
  const response = await send(method, path, jsonBody ? { jsonBody } : {});
  return response.json();
}

/** Which coding agents this computer has, and which documents are already
 * connected. The browser cannot read a PATH, so this is the only way the
 * sidebar can offer a real choice instead of setup instructions. */
export async function agents() {
  return assistantCall("GET", "agents");
}

/** Start the sidebar assistant, driving the chosen installed agent directly
 * against the document link. The link crosses loopback once, here, without
 * its key: the agent token carries the access, and no connection name, config
 * file, command line or transcript holds the key. */
export async function startAssistant({ link = "", conversation = "", chatToken = "", agentToken = "", agent = "", access = "" } = {}) {
  return assistantCall("POST", "assistant", {
    link: String(link || ""), conversation: String(conversation || ""),
    chat_token: String(chatToken || ""), agent_token: String(agentToken || ""), agent: String(agent || ""),
    access: String(access || ""),
  });
}

/** Replace the active runner's short-lived document grant. The browser calls
 * this while its own session remains active; the desktop app never receives a
 * browser cookie and the runner reads the current grant on reconnect. */
export async function renewAssistant({ link = "", conversation = "", agentToken = "" } = {}) {
  return assistantCall("POST", "assistant/renew", {
    link: String(link || ""), conversation: String(conversation || ""),
    agent_token: String(agentToken || ""),
  });
}

/** Whether the sidebar assistant is attached to this conversation. Not
 * running is an answer, not an error: it is the ordinary state before the
 * first start. Which agent and access level that is does not come back
 * here; the browser already remembers its own choice. */
export async function assistantStatus({ link = "", conversation = "" } = {}) {
  return assistantCall("POST", "assistant/status", { link: String(link || ""), conversation: String(conversation || "") });
}

/** Detach the sidebar assistant. Document changes already made are not
 * undone by stopping; only the attachment ends. */
export async function stopAssistant({ link = "", conversation = "" } = {}) {
  return assistantCall("POST", "assistant/stop", { link: String(link || ""), conversation: String(conversation || "") });
}

