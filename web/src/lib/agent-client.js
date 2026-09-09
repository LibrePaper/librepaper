import { SHELL_HEADERS, keyHeaders } from "./api.js";
import { composeTaskMessage, checkContextSize, CONTEXT_LIMIT } from "./assistant.js";

const REQUEST_TIMEOUT_MS = 15000;
const ACK_TIMEOUT_MS = 10000;
const STORAGE_PREFIX = "librepaper.agent.session.";
const MAX_MESSAGES = 256;
const MAX_TASKS = 128;

function randomId() {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  return `browser-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function documentKey(link) {
  try {
    const url = new URL(link, globalThis.location?.href || "https://invalid.example/");
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return "";
    return new URLSearchParams(url.hash.slice(1)).get("k") || "";
  } catch { return ""; }
}

function storageKey(origin, slug) { return `${STORAGE_PREFIX}${origin}:${slug}`; }

function socketUrl(origin, slug, id, key) {
  const url = new URL(`/api/documents/${encodeURIComponent(slug)}/chat/${encodeURIComponent(id)}/socket`, origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  if (key) url.searchParams.set("k", key);
  return url.href;
}

function readSession(storage, key) {
  try {
    const value = JSON.parse(storage?.getItem(key) || "null");
    return value && typeof value === "object" && value.id && value.token ? value : null;
  } catch { return null; }
}

function writeSession(storage, key, value) {
  try { storage?.setItem(key, JSON.stringify(value)); } catch { /* private browsing */ }
}

function removeSession(storage, key) {
  try { storage?.removeItem(key); } catch { /* private browsing */ }
}

function compactDiagnostic(item) {
  if (!item || typeof item !== "object") return { severity: "error", message: String(item || "Candidate verification failed.") };
  const compact = {};
  for (const key of ["severity", "file", "line", "column", "message", "source"]) {
    if (item[key] === undefined || item[key] === null) continue;
    compact[key] = typeof item[key] === "string" ? item[key].slice(0, key === "source" ? 2048 : 1024) : item[key];
  }
  return compact;
}

function boundedDiagnostics(value) {
  const all = Array.isArray(value) ? value.map(compactDiagnostic) : [];
  const ordered = [...all.filter((item) => item.severity === "error"), ...all.filter((item) => item.severity !== "error")];
  const selected = [];
  let skipped = 0;
  const encoded = (items) => new TextEncoder().encode(JSON.stringify(items)).byteLength;
  for (const item of ordered) {
    if (encoded([...selected, item]) <= CONTEXT_LIMIT) selected.push(item);
    else skipped++;
  }
  if (!skipped) return selected;
  const truncation = { severity: "warning", message: `${skipped} additional preview diagnostics were omitted.` };
  while (selected.length && encoded([...selected, truncation]) > CONTEXT_LIMIT) selected.pop();
  return [...selected, truncation];
}

/** Browser side of the persistent runner channel. The runner owns the model
 * session; this client owns socket lifetime, bounded local history, and task
 * state. */
export function createAgentClient({ origin = globalThis.location?.origin || "", project,
  slug = project, link: initialLink = "", fetcher = globalThis.fetch,
  WebSocketImpl = globalThis.WebSocket, storage = globalThis.localStorage,
  onChange = () => {} }) {
  if (!slug) throw new Error("A document slug is required.");
  let link = initialLink;
  let id = "";
  let token = "";
  let socket;
  let disposed = false;
  let reconnectTimer;
  let reconnectAttempt = 0;
  let manuallyClosed = false;
  const sessionStorageKey = storageKey(origin, slug);
  const persisted = readSession(storage, sessionStorageKey);
  const requests = new Set();
  const sends = new Map();
  const deliveries = new Map((persisted?.deliveries || [])
    .filter((item) => item?.message?.id)
    .map((item) => [item.message.id, {
      message: item.message, taskId: item.taskId || item.message.id,
      // A page can crash after relay ACK and before the runner's task event;
      // every record restored from storage therefore needs reconciliation.
      uncertain: true, accepted: item.accepted === true,
    }]));
  let view = {
    id: persisted?.id || "", token: persisted?.token || "", messages: persisted?.messages || [],
    tasks: persisted?.tasks || {}, connected: false, runnerConnected: false,
    status: persisted?.id ? "reconnecting" : "idle",
    error: "", capabilities: null,
  };
  id = view.id;
  token = view.token;

  function save() {
    if (!id || !token) return;
    writeSession(storage, sessionStorageKey, { id, token,
      messages: view.messages.slice(-MAX_MESSAGES),
      tasks: Object.fromEntries(Object.entries(view.tasks).slice(-MAX_TASKS)),
      deliveries: [...deliveries.values()].slice(-MAX_TASKS) });
  }
  function publish(patch) {
    if (disposed) return;
    view = { ...view, ...patch };
    save();
    onChange(view);
  }
  function rejectSends(message) {
    for (const [eventId, pending] of sends) {
      clearTimeout(pending.timeout);
      const delivery = deliveries.get(eventId);
      if (delivery) delivery.uncertain = true;
      if (pending.message) markDeliveryUncertain(pending.message, message);
      pending.reject(new Error(message));
    }
    sends.clear();
    save();
  }
  function markDeliveriesUncertain(detail) {
    for (const delivery of deliveries.values()) {
      delivery.uncertain = true;
      updateTask(delivery.taskId, { delivery: "uncertain", error: detail });
    }
    save();
  }
  async function request(method, suffix = "", body, authorized = true, overrideLink = link) {
    const controller = new AbortController();
    requests.add(controller);
    const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
    const headers = { Accept: "application/json", ...SHELL_HEADERS, ...keyHeaders(documentKey(overrideLink)) };
    if (authorized && token) headers["X-LibrePaper-Chat-Token"] = token;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    try {
      const response = await fetcher(`/api/documents/${encodeURIComponent(slug)}${suffix}`, {
        method, headers, body: body === undefined ? undefined : JSON.stringify(body),
        credentials: "same-origin", cache: "no-store", signal: controller.signal });
      const result = await response.json().catch(() => ({}));
      if (!response.ok) { const error = new Error(result.error || `Assistant request failed (${response.status}).`); error.status = response.status; throw error; }
      return result;
    } catch (error) {
      if (error.name === "AbortError") throw new Error("Assistant request timed out.");
      if (error instanceof TypeError) throw new Error("The server is unavailable. Your draft remains in the composer.");
      throw error;
    } finally { clearTimeout(timeout); requests.delete(controller); }
  }
  function addMessage(message) {
    if (!message?.id) return;
    const taskId = message.role === "agent" ? message.context?.task_id : null;
    const existing = view.messages.findIndex((item) =>
      (taskId && item.role === "agent" && item.context?.task_id === taskId) ||
      (item.id === message.id && item.role === message.role));
    if (existing >= 0) {
      const messages = [...view.messages];
      const previous = messages[existing];
      // The relay's echo intentionally carries only wire fields. Preserve
      // the local task identity so a retry can resend the exact request.
      messages[existing] = {
        ...previous, ...message,
        ...(message.task_id || !previous.task_id ? {} : { task_id: previous.task_id }),
        ...(message.task || !previous.task ? {} : { task: previous.task }),
      };
      publish({ messages, error: "" });
    } else publish({ messages: [...view.messages, message].slice(-MAX_MESSAGES), error: "" });
  }
  function updateTask(taskId, patch) {
    if (!taskId) return;
    const previous = view.tasks[taskId] || { id: taskId, status: "queued" };
    const merged = { ...previous, ...patch, id: taskId };
    if (["queued", "working", "completed", "failed", "cancelled"].includes(patch.status)) delete merged.input;
    if (patch.delivery === null) delete merged.delivery;
    const next = Object.fromEntries(Object.entries(merged).filter(([, value]) => value !== undefined));
    const tasks = { ...view.tasks, [taskId]: next };
    for (const [id, item] of Object.entries(tasks)) {
      if (Object.keys(tasks).length <= MAX_TASKS) break;
      if (["completed", "failed", "cancelled"].includes(item.status)) delete tasks[id];
    }
    publish({ tasks });
  }
  function canTransmit() {
    return Boolean(socket && socket.readyState === WebSocketImpl?.OPEN
      && view.connected && view.runnerConnected);
  }
  function deliveryTaskId(message) { return message.id; }
  function markDeliveryUncertain(message, detail) {
    const delivery = deliveries.get(message.id);
    if (delivery) delivery.uncertain = true;
    const taskId = deliveryTaskId(message);
    updateTask(taskId, { delivery: "uncertain", error: detail });
  }
  function confirmDelivery(taskId, result) {
    for (const [eventId, delivery] of deliveries) {
      if (delivery.taskId !== taskId) continue;
      deliveries.delete(eventId);
      const pending = sends.get(eventId);
      if (pending) {
        clearTimeout(pending.timeout);
        pending.resolve(result);
        sends.delete(eventId);
      }
    }
    updateTask(taskId, { delivery: null });
    save();
  }
  function retryDeliveries() {
    if (!canTransmit()) return;
    for (const delivery of deliveries.values()) {
      if (!delivery.uncertain || sends.has(delivery.message.id)) continue;
      delivery.uncertain = false;
      delivery.accepted = false;
      void transmit(delivery.message).catch(() => {});
    }
  }
  function transmit(message) {
    if (!canTransmit()) throw new Error("Start the LibrePaper runner before sending a message.");
    const accepted = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        sends.delete(message.id);
        markDeliveryUncertain(message, "Delivery was not confirmed. Retry when the runner is connected.");
        reject(new Error("The assistant did not acknowledge the request."));
      }, ACK_TIMEOUT_MS);
      sends.set(message.id, { resolve, reject, timeout, message });
    });
    try {
      socket.send(JSON.stringify(message));
    } catch (error) {
      const pending = sends.get(message.id);
      clearTimeout(pending?.timeout);
      sends.delete(message.id);
      markDeliveryUncertain(message, error?.message || "Delivery failed before confirmation.");
      pending?.reject(error);
    }
    return accepted;
  }
  function storedMessage(message) {
    return {
      type: "message", id: message.id, text: message.text,
      ...(message.task ? { task: message.task } : {}),
      context: message.context || {},
    };
  }
  function frameReceived(frame, current) {
    if (disposed || socket !== current) return;
    if (frame.type === "ready" || frame.type === "presence") {
      const runnerConnected = Boolean(frame.agent);
      publish({ connected: true, runnerConnected, status: runnerConnected ? "ready" : "waiting", capabilities: frame.capabilities || view.capabilities, error: "" });
      if (runnerConnected) retryDeliveries();
      else markDeliveriesUncertain("The runner is disconnected; the request will be reconciled when it returns.");
      return;
    }
    if (frame.type === "message" && frame.message) {
      addMessage(frame.message);
      return;
    }
    if (frame.type === "capabilities" && frame.capabilities) {
      publish({ connected: true, runnerConnected: true, capabilities: frame.capabilities, status: "ready", error: "" });
      return;
    }
    if (frame.type === "task") {
      const taskId = frame.task_id;
      const context = frame.context && typeof frame.context === "object" ? frame.context : {};
      updateTask(taskId, { status: frame.status, message: frame.text, input: context.input, result: context.results });
      confirmDelivery(taskId, frame);
      if (["failed", "cancelled"].includes(frame.status)) {
        addMessage({
          id: frame.id || `task-${taskId}`,
          role: "agent",
          text: frame.text || (frame.status === "cancelled" ? "Task cancelled." : "Task failed."),
          context: { task_id: taskId },
        });
      }
      return;
    }
    if (frame.type === "preview_request") { publish({ previewRequest: frame }); return; }
    if (frame.type === "preview_result") { updateTask(frame.task_id, { preview: frame }); publish({ previewResult: frame }); return; }
    if (frame.type === "ack" && sends.has(frame.id)) {
      const pending = sends.get(frame.id); clearTimeout(pending.timeout); pending.resolve(frame); sends.delete(frame.id);
      const delivery = deliveries.get(frame.id);
      if (delivery) {
        // Relay acknowledgement only proves that the frame reached the
        // server. Keep it for runner reconciliation until a task event proves
        // that the runner admitted it durably.
        delivery.accepted = true;
        updateTask(delivery.taskId, { delivery: "waiting", error: null });
      }
      save();
      if (frame.task_id) updateTask(frame.task_id, { status: frame.status || "queued" });
      return;
    }
    if (frame.type === "error") {
      const message = frame.message || "The assistant rejected this request.";
      if (frame.id && sends.has(frame.id)) {
        const pending = sends.get(frame.id);
        clearTimeout(pending.timeout);
        if (pending.message) markDeliveryUncertain(pending.message, message);
        pending.reject(new Error(message));
        sends.delete(frame.id);
      }
      if (frame.status === 404) {
        manuallyClosed = true;
        clearTimeout(reconnectTimer); reconnectTimer = undefined;
        removeSession(storage, sessionStorageKey);
        id = ""; token = "";
        publish({ id: "", token: "", connected: false, runnerConnected: false, status: "idle", error: message });
      } else publish({ error: message });
    }
  }
  function scheduleReconnect() {
    if (disposed || manuallyClosed || reconnectTimer || !id || !token) return;
    const delay = Math.min(30000, 500 * (2 ** reconnectAttempt++));
    reconnectTimer = setTimeout(() => { reconnectTimer = undefined; connect(); }, delay);
  }
  function connect() {
    if (!id || !token || disposed || !WebSocketImpl) return;
    if (socket && socket.readyState === WebSocketImpl.OPEN) return;
    const current = new WebSocketImpl(socketUrl(origin, slug, id, documentKey(link)));
    let joined = false;
    socket = current;
    current.onopen = () => { reconnectAttempt = 0; current.send(JSON.stringify({ type: "join", token, role: "user" })); };
    current.onmessage = (event) => {
      let frame; try { frame = JSON.parse(event.data); } catch { return; }
      if (frame.type === "ready" || frame.type === "presence") joined = true;
      if (frame.type === "error" && frame.status === 409 && !joined) {
        manuallyClosed = true;
        clearTimeout(reconnectTimer); reconnectTimer = undefined;
        publish({ connected: false, runnerConnected: false, status: "idle", error: "This conversation is already open in another browser tab." });
        current.close();
        return;
      }
      frameReceived(frame, current);
    };
    current.onclose = () => {
      if (socket !== current) return;
      rejectSends("The assistant connection closed before the message was accepted.");
      if (manuallyClosed) return;
      markDeliveriesUncertain("The connection closed before the runner confirmed the task.");
      publish({ connected: false, runnerConnected: false, status: "reconnecting" }); scheduleReconnect();
    };
    current.onerror = () => current.close();
  }
  async function create() {
    // Closing a replaced socket is intentional; prevent its close callback
    // from scheduling a reconnect for the old channel.
    manuallyClosed = true;
    clearTimeout(reconnectTimer); reconnectTimer = undefined;
    if (socket) socket.close();
    deliveries.clear();
    manuallyClosed = false;
    const result = await request("POST", "/chat", {}, false);
    if (!result.id || !result.token) throw new Error("The server returned an invalid assistant channel.");
    id = result.id; token = result.token;
    publish({ id, token, messages: [], tasks: {}, connected: false, runnerConnected: false, status: "connecting", error: "" });
    connect();
    return result;
  }
  async function send(body, context = {}) {
    const value = String(body || "").trim();
    if (!value) return null;
    if (!id || !token) await create();
    if (!canTransmit()) throw new Error("Start the LibrePaper runner before sending a message.");
    const message = composeTaskMessage({ id: randomId(), text: value, ...context });
    const size = checkContextSize({ task: message.task, context: message.context });
    if (!size.ok) throw new Error(`The attached context is too large (${size.bytes} bytes; limit ${size.limit}). Remove or replace the passage.`);
    // Every request is a task, including free-form chat. The runner can then
    // report status, cancellation and completion against one stable id.
    const taskId = message.id;
    deliveries.set(message.id, { message: storedMessage(message), taskId, uncertain: false, accepted: false });
    updateTask(taskId, { status: "queued", task: message.task, request: message.text });
    addMessage({ id: message.id, role: "user", text: message.text, task_id: taskId, context: message.context,
      ...(message.task ? { task: message.task } : {}) });
    await transmit(message);
    return message;
  }
  async function retry(messageId) {
    const message = view.messages.find((item) => item?.role === "user" && item.id === messageId);
    if (!message) return false;
    const wire = storedMessage(message);
    deliveries.set(message.id, { message: wire, taskId: deliveryTaskId(wire), uncertain: false, accepted: false });
    updateTask(deliveryTaskId(wire), { delivery: "pending", error: null });
    await transmit(wire);
    return true;
  }
  async function cancel(taskId) {
    if (!taskId || !socket || socket.readyState !== WebSocketImpl.OPEN) return false;
    socket.send(JSON.stringify({ type: "cancel", task_id: taskId, id: randomId() }));
    updateTask(taskId, { cancelRequested: true });
    return true;
  }
  async function respond(taskId, requestId, response) {
    if (!taskId || !requestId || !socket || socket.readyState !== WebSocketImpl.OPEN) return false;
    const id = randomId();
    const accepted = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { sends.delete(id); reject(new Error("The assistant did not acknowledge the response.")); }, ACK_TIMEOUT_MS);
      sends.set(id, { resolve, reject, timeout });
    });
    try {
      socket.send(JSON.stringify({ type: "input", id, task_id: taskId, request_id: requestId, response }));
    } catch (error) {
      const pending = sends.get(id); clearTimeout(pending?.timeout); sends.delete(id); pending?.reject(error);
    }
    await accepted;
    return true;
  }
  async function previewResult(result) {
    if (!socket || socket.readyState !== WebSocketImpl.OPEN) return false;
    try {
      socket.send(JSON.stringify({ type: "preview_result", ...result, diagnostics: boundedDiagnostics(result?.diagnostics) }));
      return true;
    } catch (error) {
      publish({ error: error?.message || "The preview result could not be delivered." });
      return false;
    }
  }
  async function end() {
    manuallyClosed = true; clearTimeout(reconnectTimer); reconnectTimer = undefined;
    const oldId = id;
    if (socket) socket.close(); socket = undefined; rejectSends("The assistant channel was closed.");
    if (oldId && token) { try { await request("DELETE", `/chat/${encodeURIComponent(oldId)}`); } catch (error) { if (error.status !== 404) throw error; } }
    deliveries.clear(); removeSession(storage, sessionStorageKey); id = ""; token = "";
    publish({ id: "", token: "", messages: [], tasks: {}, connected: false, runnerConnected: false, status: "idle", error: "" });
  }
  return {
    get current() { return view; }, setLink(next) { link = next || ""; },
    async resume() { if (id && token) { publish({ status: "reconnecting", error: "" }); connect(); } return view; },
    create, send, retry, cancel, respond, previewResult, end,
    capabilities: async (nextLink = link) => request("GET", "/assistant/capabilities", undefined, false, nextLink),
    reconnect() { manuallyClosed = false; reconnectAttempt = 0; connect(); return Promise.resolve(view); },
    dispose() { disposed = true; manuallyClosed = true; clearTimeout(reconnectTimer); if (socket) socket.close(); rejectSends("The assistant panel was closed."); for (const controller of requests) controller.abort(); },
  };
}
