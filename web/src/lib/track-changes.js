/* Live tracked edits.
 *
 * Revision records intentionally live in the same Y.Doc as source text.  A
 * Y.Text observer runs while its originating transaction is still open, so
 * writing the record here makes text and metadata one Yjs update.
 */
import * as Y from "yjs";

const STORAGE_PREFIX = "librepaper.track-changes";

function id() {
  if (typeof crypto?.randomUUID === "function") return crypto.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function encodedPosition(text, index, assoc = 0) {
  try {
    const bytes = Y.encodeRelativePosition(Y.createRelativePositionFromTypeIndex(text, Math.max(0, index), assoc));
    let binary = "";
    bytes.forEach((byte) => { binary += String.fromCharCode(byte); });
    return typeof btoa === "function" ? btoa(binary) : binary;
  } catch {
    // Anchors are persisted as base64 strings.  Returning an array here makes
    // malformed records look superficially valid and causes the server to
    // reject the whole update later.
    return "";
  }
}

function decodePosition(text, value) {
  try {
    const binary = typeof value === "string" && typeof atob === "function" ? atob(value) : String(value || "");
    const relative = Y.decodeRelativePosition(Uint8Array.from(binary, (char) => char.charCodeAt(0)));
    return Y.createAbsolutePositionFromRelativePosition(relative, text.doc)?.index ?? null;
  } catch {
    return null;
  }
}

export function preferenceKey(documentId, userId) {
  return `${STORAGE_PREFIX}.${documentId || "unknown"}.${userId || "anonymous"}`;
}

export function createRevisionController({
  doc,
  revisions = doc.getMap("revisions"),
  author = "anonymous",
  documentId = "",
  send = null,
  fileOf,
  textOf,
  mayEdit = true,
  now = () => new Date().toISOString(),
} = {}) {
  const listeners = new Set();
  const observed = new Map();
  const snapshots = new Map();
  const pendingDecisions = new Map();
  const user = String(author || "anonymous");
  const sessionKey = preferenceKey(documentId, user);
  let session = "";
  let tracking = false;
  try {
    const saved = JSON.parse(localStorage.getItem(sessionKey) || "null");
    tracking = saved?.tracking === true;
    session = saved?.session || "";
  } catch { /* storage is optional (private windows and SSR) */ }
  let showMarkup = true;
  try { showMarkup = JSON.parse(localStorage.getItem(`${sessionKey}.markup`) || "true") !== false; } catch {}

  const save = () => {
    try { localStorage.setItem(sessionKey, JSON.stringify({ tracking, session })); } catch {}
  };
  const emit = () => listeners.forEach((listener) => listener());
  // Revision metadata is shared through Y.Map. Text observers cover local
  // captures, but remote captures and review decisions arrive through this
  // map without touching a Y.Text, so forward those changes to the sidebar.
  const revisionObserver = () => emit();
  revisions.observe(revisionObserver);
  const records = () => [...revisions.values()].map((value) => {
    try { return typeof value === "string" ? JSON.parse(value) : value; } catch { return null; }
  }).filter(Boolean);

  function setTracking(value) {
    if (!mayEdit) return false;
    value = Boolean(value);
    if (value && !tracking) session = id();
    tracking = value;
    save();
    emit();
    return tracking;
  }

  function addRecord(fileId, text, from, to, before, after, kind, transaction) {
    if (!tracking || !transaction?.local || !mayEdit) return;
    const timestamp = now();
    const existing = records().find((record) => record.status === "pending" && record.file_id === fileId && record.author === user && record.session === session && (record.end_offset ?? record.end) === from && record.kind === kind);
    const revision = existing && (kind === "insert" || kind === "delete") ? {
      ...existing,
      // Deletions are represented by the same zero-width live anchor while
      // retaining their old text for reject.  Extend that retained text in
      // document order when adjacent delete operations are coalesced.
      before: kind === "delete" ? existing.before + before : existing.before,
      after: kind === "insert" ? existing.after + after : existing.after,
      end_offset: to,
      updated_at: timestamp,
    } : {
      id: id(), file_id: fileId, path: fileOf?.(fileId) || "", author: user, session,
      kind, before, after, start: encodedPosition(text, from, 0), end: encodedPosition(text, to, 0), start_offset: from, end_offset: to, status: "pending", dependencies: [],
      created_at: timestamp, updated_at: timestamp, history: [],
    };
    const overlaps = records().filter((item) => item.status === "pending" && item.file_id === fileId && item.id !== revision.id && (item.start_offset ?? item.start) < to && (item.end_offset ?? item.end) > from);
    revision.dependencies = [...new Set([...(revision.dependencies || []), ...overlaps.filter((item) => item.author !== user).map((item) => item.id)])];
    revisions.set(revision.id, JSON.stringify(revision));
  }

  function observe(fileId, text) {
    if (!(text instanceof Y.Text) || observed.has(text)) return;
    snapshots.set(text, text.toString());
    const handler = (event, transaction) => {
      if (!transaction?.local || transaction.origin === "revision-decision") { snapshots.set(text, text.toString()); return; }
      if (!tracking) { snapshots.set(text, text.toString()); return; }
      let index = 0;
      const previous = snapshots.get(text) || "";
      const operations = [];
      for (const part of event.delta || []) {
        if (part.retain) index += part.retain;
        if (part.delete) {
          operations.push({ from: index, to: index, before: previous.slice(index, index + part.delete), after: "" });
          index += part.delete;
        }
        if (part.insert != null) {
          const value = String(part.insert);
          const last = operations[operations.length - 1];
          if (last && last.from === index - (last.before?.length || 0) && !last.after) last.after = value;
          else operations.push({ from: index, to: index + value.length, before: "", after: value });
          index += value.length;
        }
      }
      snapshots.set(text, text.toString());
      for (const operation of operations) {
        const { from, to, before, after } = operation;
        if (!before && !after) continue;
        addRecord(fileId, text, from, to, before, after, before && after ? "replace" : before ? "delete" : "insert", transaction);
      }
      emit();
    };
    text.observe(handler);
    observed.set(text, handler);
  }

  function attach() {
    for (const [fileId, text] of doc.getMap("files")) observe(fileId, text);
    doc.getMap("files").observe((event) => {
      for (const [fileId, text] of event.target.entries()) observe(fileId, text);
    });
  }
  attach();

  function decide(revisionId, action, requestId = id()) {
    const request = { type: "revision-decide", revision_id: revisionId, action, request_id: requestId };
    const promise = new Promise((resolve, reject) => pendingDecisions.set(requestId, { revisionId, action, resolve, reject }));
    if (!send) return Promise.reject(new Error("review transport is unavailable"));
    let result;
    try { result = send(request); } catch (error) {
      pendingDecisions.delete(requestId);
      return Promise.reject(error);
    }
    return Promise.resolve(result).then((reply) => {
      if (reply && reply.ok === false) {
        const pending = pendingDecisions.get(requestId);
        pendingDecisions.delete(requestId);
        pending?.reject(reply.error || new Error("revision decision failed"));
      }
      return promise;
    });
  }

  function receive(message) {
    if (message?.type !== "revision-decision" && message?.type !== "revision-error") return false;
    const pending = pendingDecisions.get(message.request_id);
    if (!pending) return false;
    pendingDecisions.delete(message.request_id);
    if (message.type === "revision-error") { pending.reject?.(new Error(message.error || "revision decision failed")); emit(); return true; }
    pending.resolve?.(message);
    emit();
    return true;
  }

  return {
    revisions, get tracking() { return tracking; }, get enabled() { return tracking; }, get session() { return session; }, records,
    snapshot: () => ({ revisions: records(), enabled: tracking, showMarkup, session }),
    pending: () => records().filter((record) => record.status === "pending"),
    setTracking, setEnabled: setTracking, setShowMarkup(value) { showMarkup = Boolean(value); try { localStorage.setItem(`${sessionKey}.markup`, JSON.stringify(showMarkup)); } catch {} emit(); },
    onChange(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    decide, undo: (revisionId, requestId = id()) => decide(revisionId, "undo", requestId), receive,
    locate: (record) => ({ file_id: record.file_id, offset: decodePosition(textOf?.(record.file_id), record.start) }),
    position: (record) => decodePosition(textOf?.(record.file_id), record.start),
    dispose() { revisions.unobserve(revisionObserver); for (const [text, handler] of observed) text.unobserve(handler); observed.clear(); snapshots.clear(); listeners.clear(); },
  };
}
