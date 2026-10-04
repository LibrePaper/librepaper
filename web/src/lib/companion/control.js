// A per-instance control credential delivered by the running local companion.
// This is deliberately separate from document pairing: it can manage the
// companion from the main app without granting a website access to a project.

const PREFIX = "librepaper:companion-control:v1:";
const TOKEN = /^[A-Za-z0-9_-]{32,128}$/;
const INSTANCE = /^[a-f0-9]{16}$/;

function loopbackAddress(value) {
  if (typeof value !== "string" || value.length > 512) return "";
  try {
    const url = new URL(value);
    const host = url.hostname.toLowerCase();
    const loopback = host === "localhost" || host === "127.0.0.1" || host === "[::1]";
    if (!loopback || url.protocol !== "http:" || !url.port || url.pathname !== "/" ||
        url.search || url.hash || url.username || url.password) return "";
    return url.href;
  } catch {
    return "";
  }
}

function validInstance(value) {
  return typeof value === "string" && INSTANCE.test(value);
}

function activeKey(origin) {
  return `${PREFIX}active:${origin}`;
}

function credentialKey(origin, address, instance) {
  return `${PREFIX}${encodeURIComponent(origin)}:${encodeURIComponent(address)}:${instance}`;
}

/** Build an isolated control client; exported for focused security tests. */
export function createControlClient(deps = {}) {
  const getLocation = deps.location || (() => globalThis.location);
  const getStorage = deps.storage || (() => globalThis.sessionStorage);
  const fetcher = deps.fetch || ((input, init) => globalThis.fetch(input, init));
  const replaceUrl = deps.replaceUrl || ((url) => globalThis.history.replaceState(null, "", url));
  const listeners = new Set();
  const rejectedCredentials = new Set();
  let active = null;
  let connectGeneration = 0;
  const settingsListeners = new Set();

  function identity(credential) {
    return `${credential?.key || ""}\n${credential?.token || ""}`;
  }

  function wasRejected(credential) {
    return rejectedCredentials.has(identity(credential));
  }

  function notify() {
    const value = { available: available(), scope: scope() };
    for (const listener of listeners) {
      try { listener(value); } catch { /* a view cannot break the client */ }
    }
  }

  function origin() {
    try { return getLocation()?.origin || ""; } catch { return ""; }
  }

  function storage() {
    try { return getStorage() || null; } catch { return null; }
  }

  function inMemory(appOrigin) {
    return active?.origin === appOrigin && loopbackAddress(active.address) === active.address &&
      validInstance(active.instance) && TOKEN.test(active.token) && !wasRejected(active) ? active : null;
  }

  function readActive() {
    const appOrigin = origin();
    const store = storage();
    if (!appOrigin) return null;
    if (!store) {
      return inMemory(appOrigin);
    }
    try {
      const raw = store.getItem(activeKey(appOrigin));
      if (!raw) return inMemory(appOrigin);
      const value = JSON.parse(raw);
      const address = loopbackAddress(value?.address);
      if (!address || !validInstance(value?.instance)) return null;
      const key = credentialKey(appOrigin, address, value.instance);
      const token = store.getItem(key) || "";
      if (!TOKEN.test(token)) return inMemory(appOrigin);
      const credential = { address, instance: value.instance, token, key, activeKey: activeKey(appOrigin), origin: appOrigin };
      if (wasRejected(credential)) {
        const memory = inMemory(appOrigin);
        return memory && identity(memory) !== identity(credential) ? memory : null;
      }
      return credential;
    } catch {
      return inMemory(appOrigin);
    }
  }

  function available() {
    active = readActive();
    return Boolean(active);
  }

  // This identifier is safe to use for UI race guards. It contains no bearer.
  function scope() {
    active = readActive();
    return active ? `${active.address}|${active.instance}` : "";
  }

  function intake() {
    connectGeneration += 1;
    let loc;
    try { loc = getLocation(); } catch { return false; }
    const raw = String(loc?.hash || "").replace(/^#/, "");
    if (!raw) return false;
    const params = new URLSearchParams(raw);
    const requested = params.get("settings") === "local";
    const addressValue = params.get("companion_address");
    const token = params.get("companion_control");
    const instance = params.get("companion_instance");
    const carriedControl = addressValue !== null || token !== null || instance !== null;
    if (!requested && !carriedControl) return false;

    // Strip every control field, including invalid ones, before any component
    // intake reads this same fragment (which may also contain a document key).
    if (requested) params.delete("settings");
    params.delete("companion_address");
    params.delete("companion_control");
    params.delete("companion_instance");
    const url = new URL(loc.href);
    const remaining = params.toString();
    url.hash = remaining ? `#${remaining}` : "";
    try { replaceUrl(url.pathname + url.search + url.hash); } catch { /* sanitized storage still matters */ }

    if (requested) {
      const address = loopbackAddress(addressValue);
      const appOrigin = origin();
      const store = storage();
      active = address && TOKEN.test(token || "") && validInstance(instance)
        ? { address, instance, token, key: credentialKey(appOrigin, address, instance), activeKey: activeKey(appOrigin), origin: appOrigin }
        : null;
      if (store && appOrigin) {
        try {
          const previous = JSON.parse(store.getItem(activeKey(appOrigin)) || "null");
          const previousAddress = loopbackAddress(previous?.address);
          if (previousAddress && validInstance(previous?.instance)) {
            store.removeItem(credentialKey(appOrigin, previousAddress, previous.instance));
          }
          store.removeItem(activeKey(appOrigin));
          if (active) {
            store.setItem(active.key, token);
            store.setItem(active.activeKey, JSON.stringify({ address, instance }));
          }
        } catch { /* private browsing can deny storage */ }
      }
      active = readActive();
      notify();
    }
    return requested;
  }

  /** Open a trusted session only after the user explicitly asks to manage this companion. */
  async function connect(addressValue) {
    const generation = ++connectGeneration;
    const address = loopbackAddress(addressValue);
    if (!address) throw new TypeError("Enter a local companion address such as http://127.0.0.1:8763/.");
    const appOrigin = origin();
    let response;
    try {
      response = await fetcher(new URL("companion/api/session", address).href, {
        method: "POST", mode: "cors", credentials: "omit", cache: "no-store",
        headers: new Headers({ "Content-Type": "application/json" }), body: "{}",
        targetAddressSpace: "loopback",
      });
    } catch (error) {
      throw Object.assign(new Error("Could not establish a local companion session."), { name: "Unreachable", cause: error });
    }
    if (!response.ok) {
      let detail = "";
      try { detail = (await response.json())?.error || ""; } catch { /* no JSON detail */ }
      throw Object.assign(new Error(detail || `The companion session request failed (${response.status}).`), { name: "RequestFailed", status: response.status });
    }
    const session = await response.json();
    if (generation !== connectGeneration || appOrigin !== origin()) throw Object.assign(new Error("A newer companion connection was requested."), { name: "Canceled" });
    if (loopbackAddress(session?.address) !== address || !TOKEN.test(session?.token || "") || !validInstance(session?.instance)) {
      throw Object.assign(new Error("The companion returned an invalid control session."), { name: "InvalidSession" });
    }
    const credential = { address, instance: session.instance, token: session.token,
      key: credentialKey(appOrigin, address, session.instance), activeKey: activeKey(appOrigin), origin: appOrigin };
    active = credential;
    const store = storage();
    if (store && appOrigin) {
      try {
        store.setItem(credential.key, credential.token);
        store.setItem(credential.activeKey, JSON.stringify({ address, instance: session.instance }));
      } catch { /* keep the session in memory when storage is unavailable */ }
    }
    notify();
    return { address, instance: session.instance };
  }

  function showSettings() {
    for (const listener of settingsListeners) {
      try { listener(); } catch { /* a view cannot break navigation */ }
    }
  }

  function onSettingsRequested(listener) {
    if (typeof listener !== "function") return () => {};
    settingsListeners.add(listener);
    return () => settingsListeners.delete(listener);
  }

  async function openSettings(address) {
    if (address) await connect(address);
    showSettings();
  }

  /** @param {string} path @param {{ method?: string, body?: unknown }} [options] */
  async function request(path, { method = "GET", body } = {}) {
    const credential = readActive();
    if (!credential) throw Object.assign(new Error("Open the companion from its tray menu to manage it."), { name: "Unavailable" });
    if (typeof path !== "string" || !path.startsWith("/") || path.startsWith("//") || path.includes("\\")) {
      throw new TypeError("Invalid companion control path");
    }
    const url = new URL(`companion/api${path}`, credential.address);
    // Recheck before attaching the secret, including after loading from storage.
    if (loopbackAddress(url.origin + "/") !== credential.address || !url.pathname.startsWith("/companion/api/")) {
      throw new TypeError("Companion control requests must use the loopback API");
    }
    const headers = new Headers({ Authorization: `Bearer ${credential.token}` });
    let payload;
    if (body !== undefined) {
      headers.set("Content-Type", "application/json");
      payload = JSON.stringify(body);
    }
    /** @type {RequestInit & { targetAddressSpace?: "loopback" }} */
    const init = {
      method,
      mode: "cors",
      credentials: "omit",
      cache: "no-store",
      headers,
      body: payload,
      targetAddressSpace: "loopback",
    };
    let response;
    try {
      response = await fetcher(url.href, init);
    } catch (error) {
      throw Object.assign(new Error("Could not reach the local companion."), { name: "Unreachable", cause: error });
    }
    if (response.status === 401 || response.status === 403) {
      const store = storage();
      const current = readActive();
      const stillCurrent = current?.key === credential.key && current?.token === credential.token;
      rejectedCredentials.add(identity(credential));
      if (active?.key === credential.key && active?.token === credential.token) active = null;
      try {
        if (stillCurrent) {
          store?.removeItem(credential.key);
          store?.removeItem(credential.activeKey);
        }
      } catch { /* nothing else to clear */ }
      if (stillCurrent) notify();
      throw Object.assign(new Error("The companion control session expired. Open the companion again from its tray menu."), { name: "Unauthorized" });
    }
    if (!response.ok) {
      let detail = "";
      try {
        const failure = await response.json();
        if (typeof failure?.error === "string") detail = failure.error;
      } catch { /* an empty or non-JSON error response has no detail */ }
      throw Object.assign(new Error(detail || `The companion request failed (${response.status}).`), { name: "RequestFailed", status: response.status });
    }
    if (response.status === 204) return null;
    const type = response.headers?.get?.("content-type") || "";
    return type.includes("json") ? response.json() : response.text();
  }

  function subscribe(listener) {
    if (typeof listener !== "function") return () => {};
    listeners.add(listener);
    listener({ available: available(), scope: scope() });
    return () => listeners.delete(listener);
  }

  return { intake, available, scope, request, connect, showSettings, onSettingsRequested, openSettings, subscribe };
}

const client = createControlClient();
export const intake = client.intake;
export const available = client.available;
export const scope = client.scope;
export const request = client.request;
export const connect = client.connect;
export const showSettings = client.showSettings;
export const onSettingsRequested = client.onSettingsRequested;
export const openSettings = client.openSettings;
export const subscribe = client.subscribe;
