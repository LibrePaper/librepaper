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

import { bytesOf, toArrayBuffer } from "../bytes.js";
import { named } from "./errors.js";

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
const RECONNECT_INTERVAL_MS = 15 * 1000;

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
    current = { project: null, origin: "", active: true };
    lastInstance = null;
    addressSpaceSupported = true;
    listeners.clear();
    subscriberCount = 0;
    currentStatus = initialStatus();
    cancelAutoReconnect();
  },
};

// -------------------------------------------------------------- storage

function store() {
  return deps.storage || null;
}

function readRaw(key) {
  const s = store();
  if (!s) return null;
  // Only a string is a stored value: a storage shim that answers with a
  // promise or an object would otherwise be read as an address.
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

function removeRaw(key) {
  const s = store();
  if (!s) return;
  try { s.removeItem(key); } catch { /* a disabled store; not fatal */ }
}

function readJSON(key, fallback) {
  const raw = readRaw(key);
  if (!raw) return fallback;
  try { return JSON.parse(raw); } catch { return fallback; }
}

function writeJSON(key, value) {
  writeRaw(key, JSON.stringify(value));
}

// A stored address is used only when it is one: an http(s) URL with a host,
// ending in a slash so paths append to it. Anything else -- including the
// "[object Promise]" an earlier Settings panel once saved after rendering an
// async facade into its field -- is ignored, and the default stands.
function validAddress(value) {
  if (typeof value !== "string" || !value.trim()) return null;
  try {
    const url = new URL(value.trim());
    if (!/^https?:$/.test(url.protocol) || !url.host) return null;
    return url.pathname.endsWith("/") ? url.href : `${url.href}/`;
  } catch {
    return null;
  }
}

function randomUrlToken(bytes = 18) {
  const values = new Uint8Array(bytes);
  crypto.getRandomValues(values);
  return btoa(String.fromCharCode(...values)).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

async function sha256Hex(value) {
  const bytes = new TextEncoder().encode(value);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function address() {
  return validAddress(readRaw(ADDRESS_KEY)) || DEFAULT_ADDRESS;
}

export function setAddress(url) {
  const value = validAddress(url);
  if (value) writeRaw(ADDRESS_KEY, value);
  else removeRaw(ADDRESS_KEY);
  resetNegativeCache();
  setStatus({ address: address(), state: "unknown", checkedAt: null, error: null, instructions: instructionsFor("unknown") });
}

// The binding every document has without anyone granting one: the local app
// renders it in a workspace of its own, written from the files the browser
// sends with each job. An explicit `bind-quarto` id, entered in the render
// settings, replaces it for a project that keeps data the document does not
// share.
export const HOSTED_BINDING = "hosted";

export function bindingId() {
  const all = readJSON(BINDINGS_KEY, {});
  return String(all[pairingKey()] || HOSTED_BINDING);
}

export function setBindingId(id) {
  const all = readJSON(BINDINGS_KEY, {});
  const value = String(id || "").trim();
  if (value) all[pairingKey()] = value;
  else delete all[pairingKey()];
  writeJSON(BINDINGS_KEY, all);
  return value;
}

/** Whether this browser should quietly re-use an existing project pairing. */
// -------------------------------------------------------------- pairings

let current = { project: null, origin: "", active: true };

export function configure({ project, origin, active = true } = {}) {
  const previous = pairingKey();
  cancelAutoReconnect();
  current = {
    project: project !== undefined ? project : current.project,
    origin: origin !== undefined ? origin : current.origin,
    active,
  };
  if (previous !== pairingKey()) { lastInstance = null; negative = null; setStatus(initialStatus()); }
  scheduleAutoReconnect();
}

function pairingKey() {
  return `${current.origin}|${current.project}`;
}

export function hasPairing() { return Boolean(getPairing()?.token); }

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
    throw named("Unauthorized", "No local pairing for this project");
  }
  return pairing;
}

// -------------------------------------------------------------- status

function instructionsFor(state) {
  switch (state) {
    case "unreachable":
      return `No local LibrePaper at ${address()}. Start it with \`librepaper local start\`, or fix the address in Settings.`;
    case "denied":
      return "Your browser blocked access to the local app. Allow local network access for this site and retry.";
    case "unauthorized":
      return "Local LibrePaper is running but has not allowed this site yet. Choose Render locally and click Allow in the window that opens, or enter the pairing code it printed.";
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
let autoReconnectTimer = null;
let subscriberCount = 0;

function cancelAutoReconnect() {
  if (autoReconnectTimer) clearTimeout(autoReconnectTimer);
  autoReconnectTimer = null;
}

function scheduleAutoReconnect() {
  if (autoReconnectTimer || !current.active || subscriberCount === 0 || !getPairing()) return;
  autoReconnectTimer = setTimeout(async () => {
    autoReconnectTimer = null;
    if (!current.active || subscriberCount === 0 || !getPairing()) return;
    await probe().catch(() => {});
    scheduleAutoReconnect();
  }, RECONNECT_INTERVAL_MS);
  autoReconnectTimer.unref?.();
}

export function status() {
  return currentStatus;
}

export function subscribe(listener) {
  listeners.add(listener);
  subscriberCount = listeners.size;
  listener(currentStatus);
  scheduleAutoReconnect();
  return () => {
    if (!listeners.delete(listener)) return;
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (!subscriberCount) cancelAutoReconnect();
  };
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

async function sha256hex(bytes) {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

// Shared by `send()` and `localPreviewPage()`: a 401 drops the stored
// pairing and is `Unauthorized`; any other non-2xx is `Refused`, carrying the
// server's own JSON `error` message when it gave one. A caller that must
// recognise a 404 specially (`localPreviewPage`'s "not rendered yet") checks
// that before calling this, since a 404 would otherwise land in `Refused`.
async function throwOnFailure(response) {
  if (response.status === 401) {
    dropPairing();
    throw named("Unauthorized", "Local LibrePaper rejected the stored pairing");
  }
  if (!response.ok) {
    let message = `Local LibrePaper refused the request (${response.status})`;
    try {
      const data = await response.clone().json();
      if (data && typeof data.error === "string") message = data.error;
    } catch { /* not a JSON body; keep the generic message */ }
    throw named("Refused", message, { status: response.status });
  }
}

/// The one place a request is sent to the bridge once paired. Classifies
/// every failure the SPEC names: a network failure is `Unreachable`, a 401
/// drops the pairing and is `Unauthorized`, any other non-2xx is `Refused`
/// with the server's own message when it gave one.
async function send(method, path, { token, jsonBody, formBody, signal } = {}) {
  const scope = pairingKey();
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
      throw named("Canceled", "Canceled");
    }
    throw named("Unreachable", String(error?.message || error));
  }
  if (scope !== pairingKey() || addr !== address()) throw named("Canceled", "The local project changed.");
  await throwOnFailure(response);
  return response;
}

async function bytesOfResponse(response) {
  return new Uint8Array(await response.arrayBuffer());
}

// -------------------------------------------------------------- probe

export async function probe({ force = false, pairedOnly = false } = {}) {
  if (pairedOnly && !getPairing()) return currentStatus;
  const scope = pairingKey();
  const scopedStatus = (patch) => scope === pairingKey() ? setStatus(patch) : currentStatus;
  const startMs = deps.now();
  if (!force && negative && startMs < negative.until) {
    return currentStatus;
  }
  const addr = address();
  // Every "no usable local app yet" outcome below shares this shape -- only
  // the message and (once the health body has been read) the reported
  // version vary -- so it is noted in the backoff cache and reported once.
  const markUnreachable = (error, version = null) => {
    noteNegative(deps.now());
    return scopedStatus({
      state: "unreachable", address: addr, protocol: null, version, capabilities: null,
      checkedAt: deps.now(), error, instructions: instructionsFor("unreachable"),
    });
  };
  let response;
  try {
    response = await healthFetch(addr);
  } catch (error) {
    const message = String(error?.message || error);
    const denied = /permission|blocked|private network/i.test(message);
    const state = denied ? "denied" : "unreachable";
    noteNegative(deps.now());
    return scopedStatus({
      state, address: addr, protocol: null, version: null, capabilities: null,
      checkedAt: deps.now(), error: message, instructions: instructionsFor(state),
    });
  }
  if (scope !== pairingKey() || addr !== address()) return currentStatus;
  if (!response.ok) {
    return markUnreachable(`health responded ${response.status}`);
  }
  let body;
  try {
    body = await response.json();
  } catch {
    return markUnreachable("malformed health response");
  }
  if (scope !== pairingKey() || addr !== address()) return currentStatus;
  if (body?.service !== "librepaper-local" || !Array.isArray(body?.protocol)) {
    return markUnreachable("unrecognised local service", body?.version || null);
  }
  if (lastInstance && body.instance && lastInstance !== body.instance) {
    setStatus({ state: "reachable", capabilities: null, instance: body.instance });
  }
  lastInstance = body.instance || null;
  if (!body.protocol.includes(1)) {
    resetNegativeCache();
    return scopedStatus({
      state: "incompatible", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
      checkedAt: deps.now(), error: "protocol mismatch", instructions: instructionsFor("incompatible"),
    });
  }
  resetNegativeCache();
  // Both an unpaired app and a pairing the bridge no longer honours report
  // the same "unauthorized" status once the health check itself succeeded.
  const markUnauthorized = () => scopedStatus({
    state: "unauthorized", address: addr, protocol: body.protocol, version: body.version || null, capabilities: null,
    checkedAt: deps.now(), error: null, instructions: instructionsFor("unauthorized"),
  });
  const pairing = getPairing();
  if (!pairing || !pairing.token) {
    return markUnauthorized();
  }
  try {
    const capsResponse = await send("GET", "capabilities", { token: pairing.token });
    const caps = await capsResponse.json();
    const connectedStatus = scopedStatus({
      state: "connected", address: addr, protocol: body.protocol, version: body.version || null, capabilities: caps, instance: lastInstance,
      checkedAt: deps.now(), error: null, instructions: instructionsFor("connected"),
    });
    scheduleAutoReconnect();
    return connectedStatus;
  } catch (error) {
    if (error?.name === "Unauthorized") {
      return markUnauthorized();
    }
    // The health check just succeeded, so the service is up; a hiccup
    // verifying the token is not the same claim as "unreachable".
    return scopedStatus({
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

// The one-click pairing: the local app serves a consent page on its own
// loopback origin, this opens it in a popup naming this site and document,
// and the pairing comes back by postMessage once the person clicks Allow.
// Nothing to read off a terminal. Resolves with the status after the pairing
// is stored and verified, or the unchanged status if the window was closed
// without allowing.
export function pairViaApp({ timeoutMs = 5 * 60 * 1000 } = {}) {
  const addr = address();
  const appOrigin = new URL(addr).origin;
  const scope = pairingKey();
  const url = `${addr}librepaper/local/v1/pair?origin=${encodeURIComponent(current.origin)}&project=${encodeURIComponent(current.project)}`;
  const sameOrigin = (a, b) => String(a || "").replace(/\/+$/, "").toLowerCase() === String(b || "").replace(/\/+$/, "").toLowerCase();
  return new Promise((resolve, reject) => {
    const popup = window.open(url, "librepaper-local-pair", "popup,width=480,height=400");
    if (!popup) {
      reject(new Error("The browser blocked the window that asks the local app for permission. Allow popups for this site and try again."));
      return;
    }
    let settled = false;
    let watch = null;
    let timer = null;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      window.removeEventListener("message", onMessage);
      clearInterval(watch);
      clearTimeout(timer);
      resolve(value);
    };
    const onMessage = (event) => {
      if (event.origin !== appOrigin || event.source !== popup || scope !== pairingKey()) return;
      const data = event.data;
      if (!data || data.type !== "librepaper-local-pairing") return;
      if (data.project !== current.project || !sameOrigin(data.origin, current.origin)) return;
      setPairing({ token: data.token, expires: data.expires, instance: data.instance || null });
      resetNegativeCache();
      finish(probe({ force: true }));
    };
    window.addEventListener("message", onMessage);
    watch = setInterval(() => { if (popup.closed) setTimeout(() => finish(currentStatus), 300); }, 400);
    timer = setTimeout(() => finish(currentStatus), timeoutMs);
  });
}

// Start a consent request through an installed companion protocol handler.
// The verifier stays in this page; the deep link contains only its SHA-256
// challenge. The companion registers the request when it opens its consent UI.
let connectionAttempt = null;

export function connectViaApp(options = {}) {
  if (connectionAttempt) return connectionAttempt;
  connectionAttempt = runAppConnection(options).finally(() => { connectionAttempt = null; });
  return connectionAttempt;
}

async function runAppConnection({ timeoutMs = 120 * 1000, pollMs = 700, signal } = {}) {
  const { origin, project } = current;
  if (!origin || !project) throw named("Unauthorized", "Open a document before enabling local rendering.");
  const addr = address();
  const scope = pairingKey();
  const checkScope = () => {
    if (signal?.aborted || scope !== pairingKey() || addr !== address()) {
      throw named("Canceled", "The document or companion address changed. Connect again in the current document.");
    }
  };
  const paired = getPairing();
  const request = randomUrlToken(24);
  const verifier = randomUrlToken(32);
  const challenge = await sha256Hex(verifier);
  checkScope();
  // Existing grants need only a launch, not another consent ceremony.
  launchLink(paired ? "librepaper://launch" : `librepaper://connect?${new URLSearchParams({ origin, project, request, challenge })}`);
  const started = deps.now();
  while (deps.now() - started < timeoutMs) {
    await deps.wait(pollMs, signal);
    checkScope();
    if (paired) {
      const status = await probe({ force: true });
      checkScope();
      if (status.state === "connected") return status;
      if (status.state === "unauthorized") throw named("Unauthorized", "This document's permission expired or was revoked. Enable local rendering again to approve it.");
      if (status.state === "denied" || status.state === "incompatible") throw named("Refused", status.instructions);
      continue;
    }
    let response;
    try {
      response = await deps.fetch(`${addr}librepaper/local/v1/connect/claim`, {
        method: "POST", mode: "cors", credentials: "omit",
        headers: { "Content-Type": "application/json" },
        signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(2000)]) : AbortSignal.timeout(2000),
        body: JSON.stringify({ request, origin, project, verifier }),
      });
    } catch (error) {
      checkScope();
      if (/permission|blocked|private network/i.test(String(error?.message || error))) {
        throw named("Refused", "Allow this site's local-network permission in your browser, then retry.");
      }
      continue; // The installed companion may still be starting.
    }
    checkScope();
    if (response.status === 202 || response.status === 404) continue;
    if (response.status === 403) throw named("Unauthorized", "This connection request expired or was refused. Enable local rendering again.");
    if (!response.ok) throw named("Refused", `The companion could not connect (${response.status}). Retry or open companion settings.`);
    const data = await response.json();
    checkScope();
    if (typeof data.token !== "string" || !data.token || !Number.isFinite(data.expires)) {
      throw named("Refused", "The companion returned an invalid connection response.");
    }
    setPairing({ token: data.token, expires: data.expires, instance: data.instance || null });
    resetNegativeCache();
    return probe({ force: true });
  }
  throw named("Unreachable", "The companion did not connect. Install or open it, approve the local permission window, then retry.");
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

// Reads the last probed capabilities rather than fetching -- callers that
// need a fresh answer call `capabilities()` first, the same way the rest of
// this module treats `status()` as a cache of the last probe.
export function calepinAvailable() {
  const calepin = status().capabilities?.calepin;
  return !!(calepin && calepin.found);
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

/** Ask the companion to open its native directory chooser and bind the
 * selected project folder. The absolute path never crosses the wire. */
export async function chooseFolderBinding({ entrypoint = "" } = {}) {
  const pairing = requirePairing();
  const response = await send("POST", "bindings/folder", {
    token: pairing.token,
    jsonBody: { project: current.project, entrypoint: String(entrypoint || "") },
  });
  const data = await response.json();
  if (data?.id) setBindingId(data.id);
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

function launchLink(url) {
  if (typeof window === "undefined" || !window.document?.body) {
    throw named("Unreachable", "Open LibrePaper in a browser to launch the companion.");
  }
  const frame = window.document.createElement("iframe");
  frame.hidden = true;
  frame.src = url;
  window.document.body.appendChild(frame);
  const timer = setTimeout(() => frame.remove(), 3000);
  timer.unref?.();
}

export function openApp() {
  try { launchLink("librepaper://launch"); }
  catch { /* Keep this synchronous compatibility entrypoint usable without a DOM. */ }
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

// Same top-level envelope the bridge expects for every preview/render request
// on either engine (`protocol`, `kind`, `project`, `origin`, `snapshot`,
// `generation`, `manifest`, `options`) -- `quartoRequest` layers `quarto`
// alongside it, `buildCalepinForm` layers `engine`/`calepin` the same way,
// and `kind` stays the value the Rust `JobRequest` already deserializes
// today for both.
function jobEnvelope({ job, manifest, inputRevision }) {
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
  };
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
    },
  };
}

// Turn a shared tree's `texts`/`assets` maps into `[relativePath, bytes]`
// pairs, validating and de-duplicating paths the same way for every caller
// that walks a tree -- the Quarto job/preview form, and `syncWorkspace`.
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
function calepinOptions({ entrypoint, format = "html" } = {}) {
  const main = relativePath(entrypoint || "");
  if (!main.endsWith(".typ")) throw new Error("Calepin entrypoint must be a .typ file");
  if (!CALEPIN_FORMATS.includes(String(format))) throw new Error("invalid Calepin output format");
  return { main, format: String(format) };
}

async function buildCalepinForm({ job = {}, tree, options = {} }) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const { main, format } = calepinOptions(options);
  if (!manifest.some((file) => file.path === main)) {
    throw new Error(`Calepin project is missing its entrypoint: ${main}`);
  }
  const binding = boundedString(job.binding || job.bindingId || "", "binding");
  if (!/^[A-Za-z0-9._:-]+$/.test(binding)) throw new Error("invalid Calepin binding");
  const request = {
    ...jobEnvelope({ job, manifest, inputRevision: options.inputRevision }),
    engine: "calepin",
    calepin: { binding_id: binding, main, format },
  };
  return formOf(request, files);
}

async function buildQuartoForm({ job, tree, options = {} }) {
  const files = collectTreeFiles(tree);
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
      throw named("Canceled", "Canceled");
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
      throw named("Canceled", "Canceled");
    }
    submitted = await send("POST", "jobs", { token: pairing.token, formBody: form }).then((r) => r.json());
  }
  const status = await pollJob(submitted.id, pairing.token, signal);
  if (status.status === "canceled") {
    throw named("Canceled", "Canceled");
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

/** Start a live preview on either engine. For `quarto` this produces exactly
 * the JSON `startQuartoPreview` always has; for `calepin`, `options` is the
 * `{ entrypoint, format }` pair validated by `calepinOptions`. */
export async function startLocalPreview({ engine = "quarto", job = {}, tree, options = {} } = {}) {
  const pairing = requirePairing();
  const form = engine === "calepin"
    ? await buildCalepinForm({ job, tree, options })
    : await buildQuartoForm({ job, tree, options });
  const request = JSON.parse(await form.get("job").text());
  const response = await send("POST", "previews", { token:pairing.token, jsonBody:request });
  return response.json();
}
export async function startQuartoPreview(input) {
  return startLocalPreview({ engine: "quarto", ...input });
}
export async function stopLocalPreview(id) {
  const pairing = requirePairing();
  await send("DELETE", `previews/${encodeURIComponent(id)}`, { token:pairing.token });
}
export async function stopQuartoPreview(id) {
  return stopLocalPreview(id);
}
export async function localPreviewStatus(id) {
  const pairing = requirePairing();
  const response = await send("GET", `previews/${encodeURIComponent(id)}`, { token:pairing.token });
  return response.json();
}
export async function quartoPreviewStatus(id) {
  return localPreviewStatus(id);
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
export async function localPreviewPage(id, { etag } = {}) {
  const pairing = requirePairing();
  const addr = address();
  const url = `${addr}librepaper/local/v1/previews/${encodeURIComponent(id)}/page`;
  const headers = { Accept: "text/html, application/pdf", Authorization: `Bearer ${pairing.token}` };
  if (etag) headers["If-None-Match"] = etag;
  let response;
  try {
    response = await deps.fetch(url, { method: "GET", mode: "cors", credentials: "omit", headers });
  } catch (error) {
    throw named("Unreachable", String(error?.message || error));
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
export async function quartoPreviewPage(id, opts) {
  return localPreviewPage(id, opts);
}

// -------------------------------------------------------------- workspace sync

async function buildWorkspaceForm(tree) {
  const files = collectTreeFiles(tree);
  const manifest = await manifestOf(files);
  const form = new FormData();
  form.append("manifest", new Blob([JSON.stringify(manifest)], { type: "application/json" }), "manifest.json");
  for (const [path, bytes] of files) form.append("file", new Blob([toArrayBuffer(bytes)]), path);
  return form;
}

// Pushes the shared tree into the local app's hosted workspace so a
// `quarto preview` running there sees the current files, without going
// through the job queue. Same file layout as a job's multipart body, but the
// JSON part is named `manifest` (a bare array) rather than `job`.
export async function syncWorkspace({ tree } = {}) {
  const pairing = requirePairing();
  const form = await buildWorkspaceForm(tree);
  const response = await send("PUT", "workspace", { token: pairing.token, formBody: form });
  return response.json();
}
