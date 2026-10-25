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
// rather than the global directly, so `web/tests/integration/build-local.mjs` can run
// this exact code under Node with no browser at all. `_testing.inject`
// overrides them; `_testing.reset` restores the defaults and clears every
// module-level variable so checks do not leak into each other.

import { bytesOf, toArrayBuffer } from "../bytes.js";
import { named } from "../latex/errors.js";

/** @typedef {{ fetch: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>, storage: Storage | null, now: () => number, wait: (ms: number, signal?: AbortSignal) => Promise<void>, location: () => Location | null, replaceHash: (hash: string) => void }} DesktopDeps */
/** @typedef {{ generation?: number, deadlineSeconds?: number, snapshot?: string }} DesktopJob */
/** @typedef {{ main: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array | ArrayBuffer | string>, digests?: Record<string, string>, files?: Record<string, unknown> }} DesktopTree */
/** @typedef {{ path: string, sha256: string, size: number }} ManifestFile */
/** @typedef {{ signal?: AbortSignal, onProgress?: (progress: { done: number, total: number, scope?: string }) => void }} BuildCallbacks */
/** @typedef {{ status: string, exit?: number, outputs?: Record<string, { size: number, sha256: string }>, error?: string }} DesktopJobStatus */

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

// The shell answers 502 when the local service cannot be reached, which is
// `Unreachable`; any other non-2xx is `Refused`, carrying the service's own
// JSON `error` message when it gave one, plus its `status`, its `kind` and
// the parsed `body`.
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

function safeSha256(value, name = "digest") {
  const text = String(value || "");
  if (!/^[0-9a-f]{64}$/i.test(text)) throw new Error(`invalid ${name}`);
  return text.toLowerCase();
}

function safeSize(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > 64 * 1024 * 1024) {
    throw new Error("invalid file size");
  }
  return value;
}

// Turn a shared tree's `texts`/`assets` maps into `[relativePath, bytes]`
// pairs, validating and de-duplicating paths.
/** @param {DesktopTree} tree @returns {Array<[string, Uint8Array]>} */
function collectTreeFiles(tree) {
  const files = [];
  const seen = new Set();
  const add = (path, bytes) => {
    const safe = relativePath(path);
    if (seen.has(safe)) throw new Error(`duplicate project path: ${safe}`);
    seen.add(safe);
    files.push([safe, bytes]);
  };
  for (const [path, text] of Object.entries(tree?.texts || {})) add(path, bytesOf(text));
  for (const [path, bytes] of Object.entries(tree?.assets || {})) add(path, bytesOf(bytes));
  return files;
}

/** @param {Array<[string, Uint8Array]>} files @returns {Promise<ManifestFile[]>} */
async function manifestOf(files) {
  const manifest = [];
  for (const [path, bytes] of files) {
    manifest.push({ path, sha256: await sha256hex(bytes), size: bytes.byteLength });
  }
  return manifest;
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

// -------------------------------------------------------------- jobs

/** The outputs each format has a command for on this computer. */
export const LOCAL_OUTPUTS = Object.freeze({
  latex: Object.freeze(["pdf"]),
  typst: Object.freeze(["pdf"]),
  markdown: Object.freeze(["html", "pdf", "docx"]),
  quarto: Object.freeze(["html", "pdf", "docx"]),
});

/** The key the desktop app files a renderer under. */
export const rendererKey = (format, output) => `${format}.${output}`;

/** Render one document with the command the desktop app holds for its
 * format and output. The page sends a job and never a command. Resolves with
 * the result file, the SyncTeX file when the tool wrote one, and the log.
 * @param {{ job?: DesktopJob, tree: DesktopTree, format: string, output: string, entrypoint?: string, bindingId?: string }} input
 * @param {BuildCallbacks} [callbacks] */
export async function runRender({ job = {}, tree, format, output, entrypoint = "", bindingId: binding = "" }, { signal, onProgress } = {}) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const request = {
    protocol: 3,
    project: current.project,
    origin: current.origin,
    snapshot: String(job.snapshot || ""),
    generation: Number.isFinite(job.generation) ? Math.max(0, Math.floor(job.generation)) : 0,
    format,
    output,
    entrypoint: relativePath(entrypoint || tree.main),
    binding_id: binding || HOSTED_BINDING,
    manifest,
    options: {
      deadline_seconds: Number.isFinite(job.deadlineSeconds) ? Math.max(1, Math.floor(job.deadlineSeconds)) : 300,
    },
  };
  const scope = `local ${format}`;
  onProgress?.({ done: 0, total: 1, scope });
  let finished;
  try {
    finished = await submitAndAwait(formOf(request, files), signal);
  } catch (error) {
    // A saved folder this desktop app no longer grants (revoked, or another
    // install) is forgotten, and the render falls back to the hosted
    // workspace instead of failing every time.
    if (request.binding_id === HOSTED_BINDING || error?.name !== "Refused" || ![403, 404].includes(error?.status)) throw error;
    setBindingId("");
    return runRender({ job, tree, format, output, entrypoint }, { signal, onProgress });
  }
  const { id, status } = finished;
  const artifact = await fetchVerifiedOutput(id, "artifact", status);
  const synctex = await fetchVerifiedOutput(id, "synctex", status);
  const log = await fetchVerifiedOutput(id, "log", status);
  onProgress?.({ done: 1, total: 1, scope });
  return {
    ok: status.status === "done" && status.exit === 0 && artifact != null,
    artifact,
    synctex,
    log: log ? new TextDecoder().decode(log) : "",
    exit: status.exit,
    error: status.error,
    provenance: { backend: "local", format, output },
  };
}

// The digests the desktop app last acknowledged for one project folder. A new
// binding, or any answer that is not a 200, starts the next call from nothing,
// so the next upload sends every file again.
let acknowledged = { key: "", digests: new Map() };

function folderForm(manifest, files) {
  const form = new FormData();
  form.append("manifest", new Blob([JSON.stringify({ manifest })], { type: "application/json" }), "manifest.json");
  for (const [path, bytes] of files) form.append("file", new Blob([toArrayBuffer(bytes)]), path);
  return form;
}

/** Bring the granted project folder in step with the project. The manifest
 * lists every file, and the bytes go only for files the desktop app has not
 * acknowledged yet. Resolves with the desktop app's answer (`kept`, `trashed`,
 * `renamed`), or null after forgetting a folder that is no longer granted.
 * @param {{ tree: DesktopTree, bindingId: string }} input
 * @param {{ signal?: AbortSignal }} [options] */
export async function syncFolder({ tree, bindingId: binding }, { signal } = {}) {
  const id = String(binding || "");
  if (!id || id === HOSTED_BINDING) throw new Error("no project folder is granted");
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const key = `${projectKey()}|${id}`;
  if (acknowledged.key !== key) acknowledged = { key, digests: new Map() };
  const known = acknowledged.digests;
  const path = `folder?project=${encodeURIComponent(current.project || "")}&binding=${encodeURIComponent(id)}`;
  const changed = new Set(manifest.filter((entry) => known.get(entry.path) !== entry.sha256).map((entry) => entry.path));
  const upload = (paths) => send("PUT", path, { formBody: folderForm(manifest, files.filter(([name]) => paths.has(name))), signal });
  let response;
  try {
    try {
      response = await upload(changed);
    } catch (error) {
      if (error?.name !== "Refused" || error?.status !== 409) throw error;
      const missing = Array.isArray(error.body?.missing) ? error.body.missing : [];
      response = await upload(new Set([...changed, ...missing]));
    }
  } catch (error) {
    acknowledged = { key: "", digests: new Map() };
    if (error?.name === "Refused" && [403, 404].includes(error?.status)) {
      setBindingId("");
      return null;
    }
    throw error;
  }
  acknowledged = { key, digests: new Map(manifest.map((entry) => [entry.path, entry.sha256])) };
  return response.json();
}

/** Change the command that renders one output. The desktop app shows the
 * whole command in its own dialog and refuses with 403 when the person
 * cancels. @param {string} key @param {{ command: string[], result: string }} record */
export async function setRenderer(key, { command, result }) {
  const response = await send("PUT", `renderers/${encodeURIComponent(key)}`, { jsonBody: { command, result } });
  return response.json();
}

/** Return one output to the built-in command. Needs no confirmation. */
export async function resetRenderer(key) {
  const response = await send("PUT", `renderers/${encodeURIComponent(key)}`, { jsonBody: { reset: true } });
  return response.json();
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

