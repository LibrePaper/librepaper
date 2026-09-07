import { SHELL_HEADERS, keyHeaders } from "./api.js";

const POLL_MS = 700;
const REQUEST_TIMEOUT_MS = 15000;

function tabStorage() {
  try { return globalThis.sessionStorage; } catch { return null; }
}

function randomId() {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  return `browser-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function documentKey(link) {
  try {
    const url = new URL(link, globalThis.location?.href || "https://invalid.example/");
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return "";
    return new URLSearchParams(url.hash.slice(1)).get("k") || "";
  } catch { return ""; }
}

function safeMessages(messages) {
  return messages.filter((message) => message && typeof message.id === "string")
    .map((message) => ({
      id: message.id,
      cursor: Number.isFinite(message.cursor) ? message.cursor : 0,
      role: message.role === "user" ? "user" : "agent",
      text: typeof message.text === "string" ? message.text : "",
      ...(message.context && typeof message.context === "object" ? { context: message.context } : {}),
    }));
}

export function createAgentClient({ origin = globalThis.location?.origin || "", project, slug = project,
  link = "", fetcher = globalThis.fetch, storage = tabStorage(), onChange = () => {} }) {
  if (!slug) throw new Error("A document slug is required.");
  const storageKey = `komodoc-chat:${origin}:${slug}`;
  let saved = {};
  try { saved = JSON.parse(storage?.getItem(storageKey) || "{}"); } catch { /* unavailable storage */ }
  if (!saved || typeof saved !== "object") saved = {};

  let id = typeof saved.id === "string" ? saved.id : "";
  let token = typeof saved.token === "string" ? saved.token : "";
  let cursor = 0;
  let timer;
  let disposed = false;
  let generation = 0;
  let polling = false;
  let unconfirmed;
  const pending = new Set();
  let view = {
    id, token, cursor, messages: [], connected: false,
    listening: false, error: "",
  };

  function publish(patch) {
    if (disposed) return;
    view = { ...view, ...patch };
    onChange(view);
  }

  function remember() {
    try {
      if (id && token) storage?.setItem(storageKey, JSON.stringify({ id, token }));
      else storage?.removeItem(storageKey);
    } catch { /* storage is only a remount hint */ }
  }

  async function request(method, suffix = "", body, authorized = true) {
    const controller = new AbortController();
    pending.add(controller);
    const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
    const headers = { Accept: "application/json", ...SHELL_HEADERS };
    Object.assign(headers, keyHeaders(documentKey(link)));
    if (authorized && token) headers["X-Komodoc-Chat-Token"] = token;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const path = `/api/documents/${encodeURIComponent(slug)}/chat${suffix}`;
    try {
      const response = await fetcher(path, {
        method, headers, body: body === undefined ? undefined : JSON.stringify(body),
        credentials: "same-origin", cache: "no-store", signal: controller.signal,
      });
      const result = await response.json().catch(() => ({}));
      if (!response.ok) {
        const error = new Error(result.error || `Chat request failed (${response.status}).`);
        error.status = response.status;
        throw error;
      }
      return result;
    } catch (error) {
      if (error.name === "AbortError") throw new Error("Chat request timed out.");
      if (error instanceof TypeError) throw new Error("The server is unavailable. Your message remains in the composer.");
      throw error;
    } finally {
      clearTimeout(timeout);
      pending.delete(controller);
    }
  }

  function stopPolling() {
    clearTimeout(timer);
    timer = undefined;
    generation += 1;
    polling = false;
  }

  function merge(messages) {
    const merged = [...view.messages];
    for (const message of safeMessages(messages)) {
      const index = merged.findIndex((item) => item.id === message.id);
      if (index < 0) merged.push(message);
      else merged[index] = { ...merged[index], ...message };
    }
    return merged.slice(-256);
  }

  function clearConversation(error = "") {
    stopPolling();
    id = ""; token = ""; cursor = 0; remember();
    unconfirmed = undefined;
    publish({ id: "", token: "", cursor: 0, messages: [], connected: false, listening: false, error });
  }

  async function poll(epoch) {
    if (disposed || !id || !token || epoch !== generation || polling) return;
    polling = true;
    try {
      if (!id || !token || epoch !== generation) return;
      const result = await request("GET", `/${encodeURIComponent(id)}?after=${cursor}`);
      if (disposed || epoch !== generation) return;
      const next = Number.isInteger(result.next_cursor) ? result.next_cursor : cursor;
      cursor = Math.max(cursor, next);
      remember();
      publish({
        messages: merge(Array.isArray(result.messages) ? result.messages : []),
        cursor, listening: Boolean(result.listening), connected: true,
        error: "",
      });
    } catch (error) {
      if (epoch === generation && error.status === 404) {
        clearConversation("This conversation has expired or was deleted. Start a new one.");
      } else if (epoch === generation) publish({ connected: false, error: error.message });
    } finally {
      polling = false;
      if (!disposed && id && token && epoch === generation) timer = setTimeout(() => poll(epoch), POLL_MS);
    }
  }

  async function create() {
    stopPolling();
    const epoch = generation;
    const result = await request("POST", "", {}, false);
    if (disposed || epoch !== generation) throw new Error("The chat panel was closed.");
    if (!result.id || !result.token) throw new Error("The server returned an invalid chat capability.");
    id = result.id;
    token = result.token;
    cursor = 0;
    unconfirmed = undefined;
    remember();
    publish({ id, token, cursor, messages: [], connected: true, listening: false, error: "" });
    void poll(generation);
    return result;
  }

  async function send(text, context = {}) {
    const value = String(text || "").trim();
    if (!value) return;
    if (!id || !token) await create();
    const messageContext = {
      ...(context.file || context.path ? { file: context.file || context.path } : {}),
      ...(context.selection ? { selection: context.selection.exact || context.selection } : {}),
    };
    if (unconfirmed && unconfirmed.text !== value) {
      throw new Error("Retry the current message before composing another one.");
    }
    const message = unconfirmed || { id: randomId(), role: "user", text: value, context: messageContext };
    unconfirmed = message;
    const conversation = id;
    let result;
    try { result = await request("POST", `/${encodeURIComponent(conversation)}`, message); }
    catch (error) {
      if ([400, 409, 413].includes(error.status)) unconfirmed = undefined;
      if (error.status === 404) clearConversation("This conversation has expired or was deleted. Start a new one.");
      throw error;
    }
    if (disposed || conversation !== id) throw new Error("The chat panel was closed.");
    unconfirmed = undefined;
    publish({ messages: merge([result.message || message]), error: "" });
  }

  async function end() {
    if (!id || !token) return;
    stopPolling();
    try { await request("DELETE", `/${encodeURIComponent(id)}`); }
    catch (error) {
      if (error.status !== 404) {
        void poll(generation);
        throw error;
      }
    }
    clearConversation();
  }

  return {
    get current() { return view; },
    async resume() {
      publish({ id, token, cursor, connected: false, error: "" });
      if (id && token) { stopPolling(); void poll(generation); }
      return view;
    },
    create,
    send,
    end,
    dispose() {
      disposed = true;
      stopPolling();
      for (const controller of pending) controller.abort();
    },
  };
}
