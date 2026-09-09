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

const ADDRESS_KEY = "librepaper-local-address";
const PAIRINGS_KEY = "librepaper-local-pairings";

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

async function submitAndAwait(pairing, form, signal) {
  const submitted = await send("POST", "jobs", { token: pairing.token, formBody: form }).then((r) => r.json());
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
  const response = await send("GET", `jobs/${id}/files/${name}`, { token });
  return bytesOfResponse(response);
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
