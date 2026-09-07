import { SHELL_HEADERS, keyHeaders } from "./api.js";

const REQUEST_TIMEOUT_MS = 15000;
const ACK_TIMEOUT_MS = 10000;

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

function socketUrl(origin, slug, id, key) {
  const url = new URL(`/api/documents/${encodeURIComponent(slug)}/chat/${encodeURIComponent(id)}/socket`, origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  if (key) url.searchParams.set("k", key);
  return url.href;
}

export function createAgentClient({ origin = globalThis.location?.origin || "", project, slug = project,
  link = "", fetcher = globalThis.fetch, WebSocketImpl = globalThis.WebSocket, onChange = () => {} }) {
  if (!slug) throw new Error("A document slug is required.");
  let id = "";
  let token = "";
  let socket;
  let disposed = false;
  const requests = new Set();
  const sends = new Map();
  let view = { id: "", token: "", messages: [], connected: false, listening: false, error: "" };

  function publish(patch) {
    if (disposed) return;
    view = { ...view, ...patch };
    onChange(view);
  }

  function rejectSends(message) {
    for (const pending of sends.values()) {
      clearTimeout(pending.timeout);
      pending.reject(new Error(message));
    }
    sends.clear();
  }

  async function request(method, suffix = "", body, authorized = true) {
    const controller = new AbortController();
    requests.add(controller);
    const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
    const headers = { Accept: "application/json", ...SHELL_HEADERS, ...keyHeaders(documentKey(link)) };
    if (authorized && token) headers["X-Komodoc-Chat-Token"] = token;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    try {
      const response = await fetcher(`/api/documents/${encodeURIComponent(slug)}/chat${suffix}`, {
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
      requests.delete(controller);
    }
  }

  function connect() {
    if (!id || !token || disposed) return;
    if (!WebSocketImpl) throw new Error("This browser does not support live chat.");
    const current = new WebSocketImpl(socketUrl(origin, slug, id, documentKey(link)));
    socket = current;
    current.onopen = () => current.send(JSON.stringify({ type: "join", token, role: "user" }));
    current.onmessage = (event) => {
      if (disposed || socket !== current) return;
      let frame;
      try { frame = JSON.parse(event.data); } catch { return; }
      if (frame.type === "ready" || frame.type === "presence") {
        publish({ connected: true, listening: Boolean(frame.listening), error: "" });
      } else if (frame.type === "message" && frame.message) {
        if (!view.messages.some((message) => message.id === frame.message.id && message.role === frame.message.role)) {
          publish({ messages: [...view.messages, frame.message].slice(-256), error: "" });
        }
      } else if (frame.type === "ack" && sends.has(frame.id)) {
        clearTimeout(sends.get(frame.id).timeout);
        sends.get(frame.id).resolve();
        sends.delete(frame.id);
      } else if (frame.type === "error") {
        const message = frame.message || "Chat message was rejected.";
        if (frame.id && sends.has(frame.id)) {
          clearTimeout(sends.get(frame.id).timeout);
          sends.get(frame.id).reject(new Error(message));
          sends.delete(frame.id);
        }
        publish({ error: message });
      }
    };
    current.onclose = () => {
      if (socket !== current) return;
      rejectSends("The agent disconnected before the message was accepted.");
      publish({ connected: false, listening: false });
    };
    current.onerror = () => current.close();
  }

  async function create() {
    if (socket) socket.close();
    const result = await request("POST", "", {}, false);
    if (!result.id || !result.token) throw new Error("The server returned an invalid chat capability.");
    id = result.id;
    token = result.token;
    publish({ id, token, messages: [], connected: false, listening: false, error: "" });
    connect();
    return result;
  }

  async function send(text, context = {}) {
    const value = String(text || "").trim();
    if (!value) return;
    if (!id || !token) await create();
    if (!view.connected || !view.listening || socket?.readyState !== WebSocketImpl.OPEN) {
      throw new Error("Connect your agent before sending a message.");
    }
    const message = {
      type: "message", id: randomId(), text: value,
      context: {
        ...(context.file || context.path ? { file: context.file || context.path } : {}),
        ...(context.selection ? { selection: context.selection.exact || context.selection } : {}),
      },
    };
    const accepted = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        sends.delete(message.id);
        reject(new Error("The server did not acknowledge the message."));
      }, ACK_TIMEOUT_MS);
      sends.set(message.id, { resolve, reject, timeout });
    });
    try {
      socket.send(JSON.stringify(message));
    } catch (error) {
      const pending = sends.get(message.id);
      clearTimeout(pending?.timeout);
      sends.delete(message.id);
      pending?.reject(error);
    }
    await accepted;
  }

  async function end() {
    const oldId = id;
    if (socket) socket.close();
    socket = undefined;
    rejectSends("The chat panel was closed.");
    if (oldId && token) {
      try { await request("DELETE", `/${encodeURIComponent(oldId)}`); }
      catch (error) { if (error.status !== 404) throw error; }
    }
    id = ""; token = "";
    publish({ id: "", token: "", messages: [], connected: false, listening: false, error: "" });
  }

  return {
    get current() { return view; },
    async resume() { publish({ connected: false, error: "" }); return view; },
    create, send, end,
    dispose() {
      disposed = true;
      if (socket) socket.close();
      rejectSends("The chat panel was closed.");
      for (const controller of requests) controller.abort();
    },
  };
}
