// The local LibrePaper app, as a browser client.
//
// The browser can discover a reachable local service; it cannot enumerate
// installed applications or prove one is absent (SPEC "What detection
// means"). So this file never tries harder than one bounded probe of one
// documented endpoint, remembers the outcome for a while, and lets the
// controller decide what to try next -- this module answers "is local
// available" and "run this job on it", nothing about routing or fallback
// order (that is `route.js`, package B2).
//
// State that must survive a reload lives in `localStorage`, plain and
// unencrypted, because the token it stores is scoped to loopback and to one
// project/origin pair (see SPEC "Connection boundary") -- it is not a secret
// worth more protection than that, and pretending otherwise would not change
// what a script running on this origin can already do.
//
// Every dependency this file needs from the ambient browser -- fetch,
// storage, wall clock, the wait between polls -- is reached through `deps`
// rather than the global directly, so `web/checks/latex-local.mjs` can run
// this exact code under Node with no browser at all. `_testing.inject`
// overrides them; `_testing.reset` restores the defaults and clears every
// module-level variable so checks do not leak into each other.

export const DEFAULT_ADDRESS = "http://127.0.0.1:8763/";
export const QUARTO_PROTOCOL = 1;
export const QUARTO_JOB_KINDS = Object.freeze(["render", "refresh", "frozen"]);

const ADDRESS_KEY = "librepaper-local-address";
const PAIRINGS_KEY = "librepaper-local-pairings";
const BINDINGS_KEY = "librepaper-local-quarto-bindings";

const NEGATIVE_MIN_MS = 60 * 1000;
const NEGATIVE_MAX_MS = 10 * 60 * 1000;
const HEALTH_TIMEOUT_MS = 2000;
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
    fetch: (...args) => globalThis.fetch(...args),
    storage: typeof localStorage !== "undefined" ? localStorage : null,
    now: () => Date.now(),
    wait: realWait,
  };
}

let deps = defaultDeps();

export const _testing = {
  inject(overrides) { Object.assign(deps, overrides); },
  reset() {
    deps = defaultDeps();
    negative = null;
    current = { project: null, origin: "" };
    lastInstance = null;
    addressSpaceSupported = true;
    listeners.clear();
    currentStatus = initialStatus();
  },
};

// -------------------------------------------------------------- storage

function store() {
  return deps.storage || null;
}

function readRaw(key) {
  const s = store();
  if (!s) return null;
  try { return s.getItem(key); } catch { return null; }
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

export function address() {
  return readRaw(ADDRESS_KEY) || DEFAULT_ADDRESS;
}

export function setAddress(url) {
  writeRaw(ADDRESS_KEY, url);
  resetNegativeCache();
  setStatus({ address: url, state: "unknown", checkedAt: null, error: null, instructions: instructionsFor("unknown") });
}

export function bindingId() {
  const all = readJSON(BINDINGS_KEY, {});
  return String(all[pairingKey()] || "");
}

export function setBindingId(id) {
  const all = readJSON(BINDINGS_KEY, {});
  const value = String(id || "").trim();
  if (value) all[pairingKey()] = value;
  else delete all[pairingKey()];
  writeJSON(BINDINGS_KEY, all);
  return value;
}

// -------------------------------------------------------------- pairings

let current = { project: null, origin: "" };

export function configure({ project, origin } = {}) {
  current = {
    project: project !== undefined ? project : current.project,
    origin: origin !== undefined ? origin : current.origin,
  };
}

function pairingKey() {
  return `${current.origin}|${current.project}`;
}

function getPairing() {
  const all = readJSON(PAIRINGS_KEY, {});
  return all[pairingKey()] || null;
}

function setPairing(entry) {
  const all = readJSON(PAIRINGS_KEY, {});
  all[pairingKey()] = entry;
  writeJSON(PAIRINGS_KEY, all);
}

function dropPairing() {
  const all = readJSON(PAIRINGS_KEY, {});
  if (!(pairingKey() in all)) return;
  delete all[pairingKey()];
  writeJSON(PAIRINGS_KEY, all);
}

function requirePairing() {
  const pairing = getPairing();
  if (!pairing || !pairing.token) {
    const error = new Error("No local pairing for this project");
    error.name = "Unauthorized";
    throw error;
  }
  return pairing;
}

// -------------------------------------------------------------- status

function instructionsFor(state) {
  switch (state) {
    case "unreachable":
      return "Local LibrePaper is unavailable. Run `librepaper local start` on this computer, or set a custom address in Settings.";
    case "denied":
      return "Your browser blocked access to the local app. Allow local network access for this site and retry.";
    case "unauthorized":
      return "Run `librepaper local start` on this computer and enter the pairing code it prints.";
    case "connected":
      return "Local LibrePaper is connected.";
    case "reachable":
      return "Local LibrePaper responded but could not be verified yet. Retry the connection.";
    case "incompatible":
      return "The local LibrePaper app speaks a protocol this browser does not support. Update LibrePaper and retry.";
    default:
      return "";
  }
}

function initialStatus() {
  return {
    state: "unknown",
    address: address(),
    protocol: null,
    version: null,
    capabilities: null,
    checkedAt: null,
    error: null,
    instructions: instructionsFor("unknown"),
  };
}

let currentStatus = initialStatus();
const listeners = new Set();
let lastInstance = null;

export function status() {
  return currentStatus;
}

export function subscribe(listener) {
  listeners.add(listener);
  listener(currentStatus);
  return () => listeners.delete(listener);
}

function setStatus(patch) {
  currentStatus = { ...currentStatus, ...patch };
  for (const listener of listeners) listener(currentStatus);
  return currentStatus;
}

// -------------------------------------------------------------- negative cache
//
// A failed probe (unreachable or denied) is not retried on every keystroke --
// SPEC "Do not repeatedly probe or launch the app on every keystroke." The
// backoff starts at a minute and doubles, capped at ten minutes, until an
// explicit retry/connect or a successful probe clears it.

let negative = null; // { until, backoffMs }

function resetNegativeCache() {
  negative = null;
}

function noteNegative(nowMs) {
  const backoffMs = negative ? Math.min(negative.backoffMs * 2, NEGATIVE_MAX_MS) : NEGATIVE_MIN_MS;
  negative = { until: nowMs + backoffMs, backoffMs };
}

// -------------------------------------------------------------- wire helpers

let addressSpaceSupported = true;

async function healthFetch(addr) {
  const url = addr + "librepaper/local/v1/health";
  const init = {
    method: "GET",
    mode: "cors",
    credentials: "omit",
    signal: typeof AbortSignal !== "undefined" && AbortSignal.timeout ? AbortSignal.timeout(HEALTH_TIMEOUT_MS) : undefined,
  };
  if (addressSpaceSupported) {
    try {
      return await deps.fetch(url, { ...init, targetAddressSpace: "loopback" });
    } catch (error) {
      // Chrome's Local Network Access option is not yet universal; a
      // `TypeError` naming the option means this browser predates it, and we
      // fall back below rather than treat every browser as unsupported.
      if (error instanceof TypeError && /targetAddressSpace/i.test(String(error?.message || ""))) {
        addressSpaceSupported = false;
      } else {
        throw error;
      }
    }
  }
  return deps.fetch(url, init);
}

function bytesOf(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (typeof value === "string") return new TextEncoder().encode(value);
  throw new TypeError("expected bytes");
}

function toArrayBuffer(bytes) {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

async function sha256hex(bytes) {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// The one place a request is sent to the bridge once paired. Classifies
/// every failure the SPEC names: a network failure is `Unreachable`, a 401
/// drops the pairing and is `Unauthorized`, any other non-2xx is `Refused`
/// with the server's own message when it gave one.
async function send(method, path, { token, jsonBody, formBody, signal } = {}) {
  const addr = address();
  const url = addr + "librepaper/local/v1/" + path;
  const headers = { Accept: "application/json" };
  if (token) headers.Authorization = `Bearer ${token}`;
  let body;
  if (formBody !== undefined) {
    body = formBody;
  } else if (jsonBody !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(jsonBody);
  }
  let response;
  try {
    response = await deps.fetch(url, { method, mode: "cors", credentials: "omit", headers, body, signal });
  } catch (error) {
    if (signal?.aborted) {
      const canceled = new Error("Canceled");
      canceled.name = "Canceled";
      throw canceled;
    }
    const wrapped = new Error(String(error?.message || error));
    wrapped.name = "Unreachable";
    throw wrapped;
  }
  if (response.status === 401) {
    dropPairing();
    const error = new Error("Local LibrePaper rejected the stored pairing");
    error.name = "Unauthorized";
    throw error;
  }
  if (!response.ok) {
    let message = `Local LibrePaper refused the request (${response.status})`;
    try {
      const data = await response.clone().json();
      if (data && typeof data.error === "string") message = data.error;
    } catch { /* not a JSON body; keep the generic message */ }
    const error = new Error(message);
    error.name = "Refused";
    error.status = response.status;
    throw error;
  }
  return response;
}

async function bytesOfResponse(response) {
  return new Uint8Array(await response.arrayBuffer());
}

// -------------------------------------------------------------- probe

export async function probe({ force = false } = {}) {
  const startMs = deps.now();
  if (!force && negative && startMs < negative.until) {
    return currentStatus;
  }
  const addr = address();
  let response;
  try {
    response = await healthFetch(addr);
  } catch (error) {
    const message = String(error?.message || error);
    const denied = /permission|blocked|private network/i.test(message);
    const state = denied ? "denied" : "unreachable";
    noteNegative(deps.now());
    return setStatus({
      state, address: addr, protocol: null, version: null, capabilities: null,
      checkedAt: deps.now(), error: message, instructions: instructionsFor(state),
    });
  }
  if (!response.ok) {
    noteNegative(deps.now());
    return setStatus({
      state: "unreachable", address: addr, protocol: null, version: null, capabilities: null,
      checkedAt: deps.now(), error: `health responded ${response.status}`, instructions: instructionsFor("unreachable"),
    });
  }
  let body;
  try {
    body = await response.json();
  } catch {
    noteNegative(deps.now());
    return setStatus({
      state: "unreachable", address: addr, protocol: null, version: null, capabilities: null,
      checkedAt: deps.now(), error: "malformed health response", instructions: instructionsFor("unreachable"),
    });
  }
  if (body?.service !== "librepaper-local" || !Array.isArray(body?.protocol)) {
    noteNegative(deps.now());
    return setStatus({
      state: "unreachable", address: addr, protocol: null, version: body?.version || null, capabilities: null,
      checkedAt: deps.now(), error: "unrecognised local service", instructions: instructionsFor("unreachable"),
    });
  }
  lastInstance = body.instance || null;
  if (!body.protocol.includes(1)) {
    resetNegativeCache();
    return setStatus({
      state: "incompatible", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
      checkedAt: deps.now(), error: "protocol mismatch", instructions: instructionsFor("incompatible"),
    });
  }
  resetNegativeCache();
  const pairing = getPairing();
  if (!pairing || !pairing.token) {
    return setStatus({
      state: "unauthorized", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
      checkedAt: deps.now(), error: null, instructions: instructionsFor("unauthorized"),
    });
  }
  try {
    const capsResponse = await send("GET", "capabilities", { token: pairing.token });
    const caps = await capsResponse.json();
    return setStatus({
      state: "connected", address: addr, protocol: body.protocol, version: body.version || null, capabilities: caps,
      checkedAt: deps.now(), error: null, instructions: instructionsFor("connected"),
    });
  } catch (error) {
    if (error?.name === "Unauthorized") {
      return setStatus({
        state: "unauthorized", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
        checkedAt: deps.now(), error: null, instructions: instructionsFor("unauthorized"),
      });
    }
    // The health check just succeeded, so the service is up; a hiccup
    // verifying the token is not the same claim as "unreachable".
    return setStatus({
      state: "reachable", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
      checkedAt: deps.now(), error: String(error?.message || error), instructions: instructionsFor("reachable"),
    });
  }
}

export async function retry() {
  resetNegativeCache();
  return probe({ force: true });
}

// -------------------------------------------------------------- connect/disconnect

export async function connect(code) {
  const addr = address();
  const healthResponse = await healthFetch(addr);
  if (healthResponse.ok) {
    try {
      const body = await healthResponse.json();
      lastInstance = body?.instance || null;
    } catch { /* connect still proceeds; probe() will report the real state */ }
  }
  const response = await send("POST", "connect", {
    jsonBody: { origin: current.origin, project: current.project, code },
  });
  const data = await response.json();
  setPairing({ token: data.token, expires: data.expires, instance: lastInstance });
  resetNegativeCache();
  return probe({ force: true });
}

export async function disconnect() {
  const pairing = getPairing();
  if (pairing?.token) {
    try { await send("POST", "disconnect", { token: pairing.token }); } catch { /* revoking a dead pairing is not an error */ }
  }
  dropPairing();
  resetNegativeCache();
  return probe({ force: true });
}

export async function capabilities({ rescan = false } = {}) {
  const pairing = requirePairing();
  const method = rescan ? "POST" : "GET";
  const path = rescan ? "capabilities/rescan" : "capabilities";
  const response = await send(method, path, { token: pairing.token });
  const data = await response.json();
  setStatus({ capabilities: data });
  return data;
}

/// The one-line pairing instruction, unchanged from what `openApp()` and the
/// status line already show for an unpaired local app -- reused verbatim by
/// `latex.js`'s "a package the bundle index says the mirror does not have"
/// failure message so a reader sees the exact same call to action wherever
/// it appears.
export function pairingInstruction() {
  return instructionsFor("unauthorized");
}

export function openApp() {
  try {
    if (typeof window !== "undefined" && window.document?.body) {
      const iframe = window.document.createElement("iframe");
      iframe.style.display = "none";
      iframe.src = "librepaper://local/open";
      window.document.body.appendChild(iframe);
      setTimeout(() => iframe.remove(), 3000);
    }
  } catch { /* no protocol handler registered, or no DOM at all; the CLI instructions still apply */ }
  return instructionsFor(currentStatus.state === "connected" ? "connected" : "unauthorized");
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

function quartoPolicy(kind, policy) {
  if (!QUARTO_JOB_KINDS.includes(kind)) throw new Error(`unsupported Quarto job kind: ${kind}`);
  const allowed = ["project-defaults", "refresh-computations", "frozen"];
  const value = policy || (kind === "refresh" ? "refresh-computations" : kind === "frozen" ? "frozen" : "project-defaults");
  if (!allowed.includes(value)) throw new Error(`unsupported Quarto render policy: ${value}`);
  return value;
}

/** Build the JSON part of a Quarto request without accepting shell fragments. */
export function quartoRequest({ job = {}, entrypoint, format = "html", profile = null, parameters = {}, policy, kind = "render", inputRevision = "", inputDigest = "", files = [] } = {}) {
  const main = relativePath(entrypoint || job.entrypoint || "");
  if (!main.endsWith(".qmd")) throw new Error("Quarto entrypoint must be a .qmd file");
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
    protocol: QUARTO_PROTOCOL,
    kind: "quarto",
    project: current.project,
    origin: current.origin,
    snapshot: String(inputRevision || job.inputRevision || ""),
    generation: Number.isFinite(job.generation) ? Math.max(0, Math.floor(job.generation)) : 0,
    manifest,
    options: {
      deadline_seconds: Number.isFinite(job.deadlineSeconds) ? Math.max(1, Math.floor(job.deadlineSeconds)) : 300,
      max_passes: Number.isFinite(job.maxPasses) ? Math.max(1, Math.floor(job.maxPasses)) : 8,
    },
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
    },
  };
}

async function buildQuartoForm({ job, tree, options = {} }) {
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
  const manifest = await manifestOf(files);
  const request = quartoRequest({
    job, entrypoint: options.entrypoint || tree?.main, format: options.format || "html",
    profile: options.profile, parameters: options.parameters, policy: options.policy,
    kind: options.kind || "render", inputRevision: options.inputRevision, inputDigest: options.inputDigest,
    files: manifest,
  });
  if (!manifest.some((file) => file.path === request.quarto.main)) {
    throw new Error(`Quarto project is missing its entrypoint: ${request.quarto.main}`);
  }
  if (options.renderScope === "project") request.quarto.render_scope = "project";
  if (options.executionMode === "isolated-snapshot") {
    request.quarto.execution_mode = "isolated-snapshot";
    request.quarto.shared_inventory_complete = true;
    request.quarto.data_inputs = (options.dataInputs || []).map(relativePath);
  }
  return formOf(request, files);
}

async function manifestOf(files) {
  const manifest = [];
  for (const [path, bytes] of files) {
    manifest.push({ path, sha256: await sha256hex(bytes), size: bytes.byteLength });
  }
  return manifest;
}

async function buildBiberForm(request) {
  const stem = request.stem;
  const files = [[`${stem}.bcf`, bytesOf(request.bcf)]];
  for (const [path, bytes] of Object.entries(request.files || {})) files.push([path, bytesOf(bytes)]);
  const manifest = await manifestOf(files);
  const jobRequest = {
    protocol: 1, kind: "biber",
    project: current.project, origin: current.origin,
    snapshot: request.job.snapshot, generation: request.job.generation,
    stem, manifest,
  };
  return formOf(jobRequest, files);
}

async function buildTexForm({ job, tree, engine, main }) {
  const files = [];
  for (const [path, text] of Object.entries(tree.texts || {})) files.push([path, bytesOf(text)]);
  for (const [path, bytes] of Object.entries(tree.assets || {})) files.push([path, bytesOf(bytes)]);
  const manifest = await manifestOf(files);
  const jobRequest = {
    protocol: 1, kind: "tex",
    project: current.project, origin: current.origin,
    snapshot: job.snapshot, generation: job.generation,
    engine, main, manifest,
  };
  return formOf(jobRequest, files);
}

// -------------------------------------------------------------- job polling

async function pollJob(id, token, signal) {
  const start = deps.now();
  for (;;) {
    if (signal?.aborted) {
      await send("POST", `jobs/${id}/cancel`, { token }).catch(() => {});
      const error = new Error("Canceled");
      error.name = "Canceled";
      throw error;
    }
    const response = await send("GET", `jobs/${id}`, { token, signal });
    const status = await response.json();
    if (status.status === "done" || status.status === "failed" || status.status === "canceled") return status;
    const waitMs = deps.now() - start > POLL_SLOW_AFTER_MS ? POLL_SLOW_MS : POLL_FAST_MS;
    await deps.wait(waitMs, signal);
  }
}

async function submitAndAwait(pairing, form, signal, { retryPost = false } = {}) {
  let submitted;
  try {
    submitted = await send("POST", "jobs", { token: pairing.token, formBody: form }).then((r) => r.json());
  } catch (error) {
    // A lost response is the only safe case for retrying a render.  Reuse the
    // same FormData object: its idempotency key and complete project snapshot
    // are identical, so the bridge can return the existing job instead of
    // executing user code a second time.
    if (!retryPost || error?.name !== "Unreachable" || signal?.aborted) throw error;
    await deps.wait(250, signal);
    if (signal?.aborted) {
      const canceled = new Error("Canceled");
      canceled.name = "Canceled";
      throw canceled;
    }
    submitted = await send("POST", "jobs", { token: pairing.token, formBody: form }).then((r) => r.json());
  }
  const status = await pollJob(submitted.id, pairing.token, signal);
  if (status.status === "canceled") {
    const error = new Error("Canceled");
    error.name = "Canceled";
    throw error;
  }
  return { id: submitted.id, status };
}

async function fetchOutput(id, token, name, status) {
  if (!status.outputs?.[name]) return null;
  const response = await send("GET", `jobs/${id}/files/${encodeURIComponent(name)}`, { token });
  return bytesOfResponse(response);
}

function base64Of(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return typeof btoa === "function" ? btoa(binary) : Buffer.from(bytes).toString("base64");
}

// -------------------------------------------------------------- jobs

export async function runBiber(request, { signal, onProgress } = {}) {
  const pairing = requirePairing();
  onProgress?.({ done: 0, total: 1, scope: "local Biber" });
  const form = await buildBiberForm(request);
  const { id, status } = await submitAndAwait(pairing, form, signal);
  const bbl = await fetchOutput(id, pairing.token, "bbl", status);
  const blgBytes = (await fetchOutput(id, pairing.token, "blg", status)) || new Uint8Array();
  onProgress?.({ done: 1, total: 1, scope: "local Biber" });
  return {
    ok: status.status === "done" && status.exit === 0 && bbl != null,
    bbl,
    blg: new TextDecoder().decode(blgBytes),
    exit: status.exit,
    tool: { name: "biber", version: status.provenance?.tools?.biber || null, backend: "local" },
    incompatible: !!status.incompatible,
    ...(status.error ? { error: status.error } : {}),
  };
}

export async function runTex({ job, tree, engine, main }, { signal, onProgress } = {}) {
  const pairing = requirePairing();
  onProgress?.({ done: 0, total: 1, scope: "local compilation" });
  const form = await buildTexForm({ job, tree, engine, main });
  const { id, status } = await submitAndAwait(pairing, form, signal);
  const pdf = await fetchOutput(id, pairing.token, "pdf", status);
  const synctex = await fetchOutput(id, pairing.token, "synctex", status);
  const logBytes = (await fetchOutput(id, pairing.token, "log", status)) || new Uint8Array();
  onProgress?.({ done: 1, total: 1, scope: "local compilation" });
  return {
    ok: status.status === "done" && status.exit === 0 && pdf != null,
    pdf, synctex,
    log: new TextDecoder().decode(logBytes),
    diagnostics: status.diagnostics || [],
    exit: status.exit,
    provenance: { ...(status.provenance || {}), backend: "local" },
    ...(status.error ? { error: status.error } : {}),
  };
}

// Quarto has a separate capability and job contract from TeX.  The bridge
// receives a structured request and a project snapshot; it never receives a
// command line, executable path, or arbitrary environment from the browser.
export async function runQuarto({ job = {}, tree, options = {} }, { signal, onProgress, onLog } = {}) {
  const pairing = requirePairing();
  const stableKey = String(job.idempotencyKey || job.id || globalThis.crypto?.randomUUID?.() || `quarto-${deps.now()}`);
  const stableJob = { ...job, id: job.id || stableKey, idempotencyKey: job.idempotencyKey || stableKey };
  onProgress?.({ done: 0, total: 1, scope: "local Quarto render", stage: "preparing" });
  const form = await buildQuartoForm({ job: stableJob, tree, options });
  const { id, status } = await submitAndAwait(pairing, form, signal, { retryPost: true });
  if (status.log_tail) for (const line of String(status.log_tail).split("\n")) onLog?.(line);
  const format = options.format || "html";
  const artifactName = `artifact.${format}`;
  const manifestBytes = await fetchOutput(id, pairing.token, "manifest.json", status) ||
    await fetchOutput(id, pairing.token, "quarto-bundle.json", status);
  const logBytes = await fetchOutput(id, pairing.token, "log", status);
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
      const bytes = await fetchOutput(id, pairing.token, outputName, status);
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

export async function cancelQuarto(jobId) {
  const pairing = requirePairing();
  if (!jobId) throw new Error("a Quarto job id is required");
  await send("POST", `jobs/${encodeURIComponent(jobId)}/cancel`, { token: pairing.token });
  return true;
}

export async function startQuartoPreview(input) {
  const pairing = requirePairing();
  const form = await buildQuartoForm(input);
  const request = JSON.parse(await form.get("job").text());
  const response = await send("POST", "previews", { token:pairing.token, jsonBody:request });
  return response.json();
}
export async function stopQuartoPreview(id) {
  const pairing = requirePairing();
  await send("DELETE", `previews/${encodeURIComponent(id)}`, { token:pairing.token });
}
export async function quartoPreviewStatus(id) {
  const pairing = requirePairing();
  const response = await send("GET", `previews/${encodeURIComponent(id)}`, { token:pairing.token });
  return response.json();
}
