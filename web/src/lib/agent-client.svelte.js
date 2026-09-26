import { SHELL_HEADERS, keyHeaders } from "./api.js";
import { composeTaskMessage, checkContextSize, CONTEXT_LIMIT } from "./assistant.js";
import { validPath } from "./assistant-preview.js";

const REQUEST_TIMEOUT_MS = 15000;
const ACK_TIMEOUT_MS = 10000;
const STORAGE_PREFIX = "librepaper.agent.session.";
const MAX_MESSAGES = 256;
const MAX_TASKS = 128;
const MAX_CANDIDATE_SOURCE_BYTES = 32 * 1024 * 1024;
const MAX_CANDIDATE_TOTAL_BYTES = 32 * 1024 * 1024;
const MAX_CANDIDATE_FILES = 512;
const MAX_PREVIEW_RESPONSES = 8;

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

// The view's shape when no conversation is attached. Used both for the
// client's initial state and whenever a conversation ends, so the shape is
// defined once rather than written out twice.
function emptyView() {
  return { id: "", token: "", messages: [], tasks: {}, connected: false, runnerConnected: false,
    status: "idle", error: "", capabilities: null, assistantAgent: "", assistantAccess: "" };
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
 * session; this client owns socket lifetime, bounded local history, task
 * state, and dispatching the runner's preview requests to `onpreview`. */
export function createAgentClient({ origin = globalThis.location?.origin || "", project,
  slug = project, link: initialLink = "", fetcher = globalThis.fetch,
  WebSocketImpl = globalThis.WebSocket, storage = globalThis.localStorage,
  onpreview = null }) {
  if (!slug) throw new Error("A document slug is required.");
  let link = initialLink;
  let id = "";
  let token = "";
  let socket;
  let disposed = false;
  let reconnectTimer;
  let reconnectAttempt = 0;
  let manuallyClosed = false;
  let generation = 0;
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
  // A runner retries an unconfirmed preview with the same request ID after
  // reconnecting. Reuse the immutable browser result rather than compiling
  // the candidate twice, and never dispatch the same in-flight request twice.
  const previewResponses = new Map();
  const previewInFlight = new Set();
  // Replaced wholesale on every change, never edited in place, so `.raw`
  // avoids proxying the transcript and task map on every publish.
  let view = $state.raw({ ...emptyView(),
    id: persisted?.id || "", token: persisted?.token || "",
    messages: persisted?.messages || [], tasks: persisted?.tasks || {},
    status: persisted?.id ? "reconnecting" : "idle",
    assistantAgent: persisted?.assistantAgent || "", assistantAccess: persisted?.assistantAccess || "" });
  let runtimeSessionId = persisted?.runtimeSessionId || "";
  id = persisted?.id || "";
  token = persisted?.token || "";
  let saveQueued = false;

  function save() {
    if (!id || !token) return;
    writeSession(storage, sessionStorageKey, { id, token,
      messages: view.messages.slice(-MAX_MESSAGES),
      tasks: Object.fromEntries(Object.entries(view.tasks).slice(-MAX_TASKS)),
      deliveries: [...deliveries.values()].slice(-MAX_TASKS), runtimeSessionId,
      assistantAgent: view.assistantAgent, assistantAccess: view.assistantAccess });
  }
  function saveSoon() {
    if (saveQueued) return;
    saveQueued = true;
    queueMicrotask(() => { saveQueued = false; save(); });
  }
  function publish(patch) {
    if (disposed) return;
    view = { ...view, ...patch };
    // Socket presence/status is transient. Only transcript/task/session/
    // assistant changes rewrite the bounded durable record, coalesced per
    // microtask.
    if (["id", "token", "messages", "tasks", "assistantAgent", "assistantAccess"].some((key) => Object.prototype.hasOwnProperty.call(patch, key))) saveSoon();
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
  // Every browser->server request shares its abort controller, timeout and
  // network-error translation; `request` and `requestText` differ only in
  // what they do with the response.
  async function withTimeout(run) {
    const controller = new AbortController();
    requests.add(controller);
    const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
    try {
      return await run(controller.signal);
    } catch (error) {
      if (error.name === "AbortError") throw new Error("Assistant request timed out.");
      if (error instanceof TypeError) throw new Error("The server is unavailable. Your draft remains in the composer.");
      throw error;
    } finally { clearTimeout(timeout); requests.delete(controller); }
  }
  async function request(method, suffix = "", body, authorized = true, overrideLink = link, extraHeaders = {}) {
    return withTimeout(async (signal) => {
      const headers = { Accept: "application/json", ...SHELL_HEADERS, ...keyHeaders(documentKey(overrideLink)), ...extraHeaders };
      if (authorized && token) headers["X-LibrePaper-Chat-Token"] = token;
      if (body !== undefined) headers["Content-Type"] = "application/json";
      const response = await fetcher(`/api/documents/${encodeURIComponent(slug)}${suffix}`, {
        method, headers, body: body === undefined ? undefined : JSON.stringify(body),
        credentials: "same-origin", cache: "no-store", signal });
      const result = await response.json().catch(() => ({}));
      if (!response.ok) { const error = new Error(result.error || `Assistant request failed (${response.status}).`); error.status = response.status; throw error; }
      return result;
    });
  }
  async function requestText(suffix, overrideLink = link, extraHeaders = {}) {
    return withTimeout(async (signal) => {
      const response = await fetcher(`/api/documents/${encodeURIComponent(slug)}${suffix}`, {
        method: "GET", headers: { Accept: "text/plain", ...SHELL_HEADERS, ...keyHeaders(documentKey(overrideLink)), ...extraHeaders },
        credentials: "same-origin", cache: "no-store", signal,
      });
      if (!response.ok) {
        const body = await response.text().catch(() => "");
        throw new Error(body || `Assistant request failed (${response.status}).`);
      }
      const advertised = Number(response.headers.get("content-length"));
      if (Number.isFinite(advertised) && advertised > MAX_CANDIDATE_SOURCE_BYTES) {
        throw new Error("Candidate source exceeds the browser limit.");
      }
      const chunks = [];
      let total = 0;
      if (response.body?.getReader) {
        const reader = response.body.getReader();
        try {
          for (;;) {
            const part = await reader.read();
            if (part.done) break;
            total += part.value.byteLength;
            if (total > MAX_CANDIDATE_SOURCE_BYTES) {
              await reader.cancel();
              throw new Error("Candidate source exceeds the browser limit.");
            }
            chunks.push(part.value);
          }
        } finally { reader.releaseLock(); }
      } else {
        const body = new Uint8Array(await response.arrayBuffer());
        total = body.byteLength;
        if (total > MAX_CANDIDATE_SOURCE_BYTES) throw new Error("Candidate source exceeds the browser limit.");
        chunks.push(body);
      }
      const body = new Uint8Array(total);
      let offset = 0;
      for (const chunk of chunks) { body.set(chunk, offset); offset += chunk.byteLength; }
      try { return new TextDecoder("utf-8", { fatal: true }).decode(body); }
      catch { throw new Error("Candidate source is not valid UTF-8."); }
    });
  }
  async function fetchCandidate(candidateId, overrideLink = link, candidateToken = "") {
    if (typeof candidateId !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(candidateId)) {
      throw new Error("The preview candidate identifier is invalid.");
    }
    const prefix = `/agent/candidates/${encodeURIComponent(candidateId)}`;
    const candidateHeaders = candidateToken ? { "X-LibrePaper-Candidate-Token": candidateToken } : {};
    const metadata = await request("GET", prefix, undefined, false, overrideLink, candidateHeaders);
    const manifest = metadata?.files;
    if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
      throw new Error("The server returned no candidate manifest.");
    }
    if (Object.keys(manifest).length === 0 || Object.keys(manifest).length > MAX_CANDIDATE_FILES) {
      throw new Error("The candidate manifest exceeds the browser limit.");
    }
    const textEntries = Object.entries(manifest)
      .filter(([, entry]) => entry?.kind === "text")
      .map(([path, entry]) => [path, entry]);
    const advertisedTotal = textEntries.reduce((sum, [, entry]) => {
      const size = Number(entry?.size);
      return Number.isSafeInteger(size) && size >= 0 ? sum + size : Number.POSITIVE_INFINITY;
    }, 0);
    if (!Number.isFinite(advertisedTotal) || advertisedTotal > MAX_CANDIDATE_TOTAL_BYTES) {
      throw new Error("Candidate source exceeds the browser aggregate limit.");
    }
    const fetched = new Array(textEntries.length);
    let next = 0;
    let fetchedBytes = 0;
    const worker = async () => {
      for (;;) {
        const index = next++;
        if (index >= textEntries.length) return;
        const [path] = textEntries[index];
        if (!validPath(path)) throw new Error("The server returned an invalid candidate path.");
        const query = `?path=${encodeURIComponent(path)}`;
        const text = await requestText(`${prefix}/source${query}`, overrideLink, candidateHeaders);
        fetchedBytes += new TextEncoder().encode(text).byteLength;
        if (fetchedBytes > MAX_CANDIDATE_TOTAL_BYTES) throw new Error("Candidate source exceeds the browser aggregate limit.");
        fetched[index] = [path, text];
      }
    };
    await Promise.all(Array.from({ length: Math.min(4, textEntries.length) }, () => worker()));
    const texts = Object.fromEntries(fetched.filter(Boolean).map(([path, text]) => [path, text]));
    return {
      ...metadata,
      base_revision: metadata.base_revision || "",
      revision: metadata.revision || "",
      files: manifest,
      texts,
    };
  }
  function addMessage(message) {
    if (!message?.id) return;
    const taskId = message.role === "agent" ? message.context?.task_id : null;
    const existing = view.messages.findIndex((item) =>
      (taskId && message.context?.task_status !== true && item.role === "agent" && item.context?.task_id === taskId &&
        (message.context?.streamed_answer !== true || item.context?.streamed_answer === true)) ||
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
    if (["queued", "working", "completed", "failed", "cancelled", "interrupted"].includes(patch.status)) delete merged.input;
    if (patch.delivery === null) delete merged.delivery;
    const next = Object.fromEntries(Object.entries(merged).filter(([, value]) => value !== undefined));
    const tasks = { ...view.tasks, [taskId]: next };
    for (const [id, item] of Object.entries(tasks)) {
      if (Object.keys(tasks).length <= MAX_TASKS) break;
      if (["completed", "failed", "cancelled", "interrupted"].includes(item.status)) delete tasks[id];
    }
    publish({ tasks });
  }
  function canTransmit() {
    return Boolean(socket && socket.readyState === WebSocketImpl?.OPEN
      && view.connected && view.runnerConnected);
  }
  function markDeliveryUncertain(message, detail) {
    const delivery = deliveries.get(message.id);
    if (delivery) delivery.uncertain = true;
    updateTask(message.id, { delivery: "uncertain", error: detail });
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
  // Shared by `transmit` and `respond`: register a pending acknowledgement,
  // time it out, and (when the frame carries a delivery) mark it uncertain.
  function awaitAck(ackId, { message, timeoutMessage = "The assistant did not acknowledge the request." } = {}) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        sends.delete(ackId);
        if (message) markDeliveryUncertain(message, "Delivery was not confirmed. Retry when the runner is connected.");
        reject(new Error(timeoutMessage));
      }, ACK_TIMEOUT_MS);
      sends.set(ackId, { resolve, reject, timeout, message });
    });
  }
  function transmit(message) {
    if (!canTransmit()) throw new Error("Start the LibrePaper runner before sending a message.");
    const accepted = awaitAck(message.id, { message });
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
  // Closing a socket is intentional in three places (a replaced conversation,
  // ending one, and disposing the client): each time, its handlers must be
  // cleared first so its own close event cannot schedule a reconnect.
  function closeSocket(target) {
    if (!target) return;
    target.onopen = target.onmessage = target.onerror = target.onclose = null;
    target.close();
  }
  async function handlePreviewRequest(frame) {
    if (!frame?.id || !onpreview) return;
    const conversationAtArrival = id;
    const sendResult = (result) => id === conversationAtArrival && previewResult({
      ...result,
      id: `${frame.id}-result`, request_id: frame.id,
      task_id: frame.task_id,
      base_revision: frame.base_revision, revision: frame.revision,
    });
    const cached = previewResponses.get(frame.id);
    if (cached) { await sendResult(cached); return; }
    if (previewInFlight.has(frame.id)) return;
    previewInFlight.add(frame.id);
    let result;
    try {
      // The relay carries only an immutable candidate handle. Source files
      // travel over authenticated same-origin requests, so a large file
      // never consumes the chat frame's bounded context budget.
      const candidate = frame.candidate_id
        ? await fetchCandidate(frame.candidate_id, undefined, frame.candidate_token || "")
        : null;
      result = await onpreview(candidate ? { ...frame, candidate } : frame);
    } catch (error) {
      result = {
        ok: false,
        diagnostics: [{ severity: "error", message: error?.message || "Candidate verification failed." }],
        output: "none",
      };
    }
    previewResponses.set(frame.id, result);
    while (previewResponses.size > MAX_PREVIEW_RESPONSES) previewResponses.delete(previewResponses.keys().next().value);
    previewInFlight.delete(frame.id);
    await sendResult(result);
  }
  function frameReceived(frame, current) {
    if (disposed || socket !== current) return;
    if (frame.type === "capabilities" && typeof frame.session_id === "string" && frame.session_id.length <= 128 && frame.session_id && frame.session_id !== runtimeSessionId) {
      const hadSession = Boolean(runtimeSessionId);
      runtimeSessionId = frame.session_id;
      if (hadSession || view.messages.length) addMessage({ id: `session-${runtimeSessionId}`, role: "agent", text: "New agent session. The agent does not remember earlier messages in this conversation.", context: { session_boundary: true } });
      saveSoon();
    }
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
    if (frame.type === "answer") {
      const taskId = frame.task_id;
      const previous = view.tasks[taskId];
      if (typeof taskId !== "string" || !taskId || !Number.isSafeInteger(frame.seq) || frame.seq <= 0 ||
          typeof frame.text !== "string" || new TextEncoder().encode(frame.text).byteLength > 32 * 1024 ||
          typeof frame.truncated !== "boolean" || frame.seq <= (previous?.answer_seq || 0) ||
          frame.seq < (previous?.seq || 0)) return;
      updateTask(taskId, { answer_seq: frame.seq });
      addMessage({
        id: `task-answer-${taskId}`, role: "agent", text: frame.text,
        context: { task_id: taskId, truncated: frame.truncated, streamed_answer: true,
          ...(previous?.result ? { results: previous.result } : {}) },
      });
      return;
    }
    if (frame.type === "capabilities" && frame.capabilities) {
      publish({ connected: true, runnerConnected: true, capabilities: frame.capabilities, status: "ready", error: "" });
      return;
    }
    if (frame.type === "task") {
      const taskId = frame.task_id;
      const previous = view.tasks[taskId];
      if (frame.seq !== undefined && (!Number.isSafeInteger(frame.seq) || frame.seq <= 0)) return;
      if (frame.seq !== undefined && Number.isSafeInteger(previous?.seq) && frame.seq <= previous.seq) {
        // A reconnect can replay the last durable frame. It may still settle
        // the browser's delivery promise, but must not roll task state back.
        confirmDelivery(taskId, frame);
        return;
      }
      const context = frame.context && typeof frame.context === "object" ? frame.context : {};
      updateTask(taskId, { seq: frame.seq, status: frame.status, message: frame.text, input: context.input, result: context.results });
      const answerMessage = view.messages.find((item) => item.role === "agent" && item.context?.streamed_answer === true && item.context?.task_id === taskId);
      if (answerMessage && context.results) {
        addMessage({ ...answerMessage, context: { ...answerMessage.context, results: context.results } });
      }
      confirmDelivery(taskId, frame);
      if (["failed", "cancelled", "interrupted"].includes(frame.status)) {
        addMessage({
          id: `task-${taskId}`,
          role: "agent",
          text: frame.text || (frame.status === "cancelled" ? "Task cancelled." : frame.status === "interrupted" ? "Task interrupted; reconcile its document operations before retrying." : "Task failed."),
          context: { task_id: taskId, task_status: true, ...(context.results ? { results: context.results } : {}) },
        });
      }
      return;
    }
    if (frame.type === "preview_request") {
      void handlePreviewRequest(frame);
      return;
    }
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
    if (socket && (socket.readyState === WebSocketImpl.OPEN || socket.readyState === WebSocketImpl.CONNECTING)) return;
    const owner = generation;
    const current = new WebSocketImpl(socketUrl(origin, slug, id, documentKey(link)));
    let joined = false;
    socket = current;
    current.onopen = () => {
      if (disposed || generation !== owner || socket !== current) return;
      reconnectAttempt = 0;
      current.send(JSON.stringify({ type: "join", token, role: "user" }));
    };
    current.onmessage = (event) => {
      if (disposed || generation !== owner || socket !== current) return;
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
      if (disposed || generation !== owner || socket !== current) return;
      socket = undefined;
      rejectSends("The assistant connection closed before the message was accepted.");
      if (manuallyClosed) return;
      markDeliveriesUncertain("The connection closed before the runner confirmed the task.");
      publish({ connected: false, runnerConnected: false, status: "reconnecting" }); scheduleReconnect();
    };
    current.onerror = () => {
      if (disposed || generation !== owner || socket !== current) return;
      current.close();
    };
  }
  async function create() {
    // Closing a replaced socket is intentional; prevent its close callback
    // from scheduling a reconnect for the old channel.
    manuallyClosed = true;
    clearTimeout(reconnectTimer); reconnectTimer = undefined;
    generation++;
    closeSocket(socket);
    socket = undefined;
    deliveries.clear();
    previewResponses.clear();
    previewInFlight.clear();
    manuallyClosed = false;
    const result = await request("POST", "/chat", {}, false);
    if (!result.id || !result.token) throw new Error("The server returned an invalid assistant channel.");
    id = result.id; token = result.token;
    runtimeSessionId = "";
    publish({ ...emptyView(), id, token, status: "connecting" });
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
    deliveries.set(message.id, { message: wire, taskId: wire.id, uncertain: false, accepted: false });
    updateTask(wire.id, { delivery: "pending", error: null });
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
    const ackId = randomId();
    const accepted = awaitAck(ackId, { timeoutMessage: "The assistant did not acknowledge the response." });
    try {
      socket.send(JSON.stringify({ type: "input", id: ackId, task_id: taskId, request_id: requestId, response }));
    } catch (error) {
      const pending = sends.get(ackId); clearTimeout(pending?.timeout); sends.delete(ackId); pending?.reject(error);
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
    manuallyClosed = true; generation++; clearTimeout(reconnectTimer); reconnectTimer = undefined;
    const oldId = id;
    closeSocket(socket);
    socket = undefined;
    rejectSends("The assistant channel was closed.");
    if (oldId && token) { try { await request("DELETE", `/chat/${encodeURIComponent(oldId)}`); } catch (error) { if (error.status !== 404) throw error; } }
    deliveries.clear();
    previewResponses.clear();
    previewInFlight.clear();
    removeSession(storage, sessionStorageKey); id = ""; token = "";
    runtimeSessionId = "";
    publish(emptyView());
  }
  return {
    get current() { return view; }, setLink(next) { link = next || ""; },
    async resume() { if (id && token) { publish({ status: "reconnecting", error: "" }); connect(); } return view; },
    create, send, retry, cancel, respond, previewResult, end,
    rememberAssistant({ agent = "", access = "" } = {}) { publish({ assistantAgent: agent, assistantAccess: access }); },
    capabilities: async (nextLink = link) => request("GET", "/assistant/capabilities", undefined, false, nextLink),
    fetchCandidate,
    reconnect() { manuallyClosed = false; reconnectAttempt = 0; connect(); return Promise.resolve(view); },
    dispose() {
      if (saveQueued) { saveQueued = false; save(); }
      disposed = true; manuallyClosed = true; generation++; clearTimeout(reconnectTimer);
      closeSocket(socket);
      socket = undefined;
      rejectSends("The assistant panel was closed.");
      for (const controller of requests) controller.abort();
    },
  };
}
