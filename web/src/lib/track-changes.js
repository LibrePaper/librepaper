/* Live tracked edits. Revision records share the Y.Doc with source text. */
import * as Y from "yjs";

const STORAGE_PREFIX = "librepaper.track-changes";

function id() {
  if (typeof globalThis.crypto?.randomUUID === "function") return globalThis.crypto.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function toBase64(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  if (typeof globalThis.btoa === "function") return globalThis.btoa(binary);
  if (typeof Buffer !== "undefined") return Buffer.from(bytes).toString("base64");
  return binary;
}

function fromBase64(value) {
  if (typeof value !== "string" || !value) return null;
  if (typeof globalThis.atob === "function") {
    const binary = globalThis.atob(value);
    return Uint8Array.from(binary, (char) => char.charCodeAt(0));
  }
  if (typeof Buffer !== "undefined") return Uint8Array.from(Buffer.from(value, "base64"));
  return Uint8Array.from(value, (char) => char.charCodeAt(0));
}

function encodedPosition(text, index, assoc = 0) {
  try {
    return toBase64(Y.encodeRelativePosition(
      Y.createRelativePositionFromTypeIndex(text, Math.max(0, index), assoc),
    ));
  } catch {
    return "";
  }
}

function decodePosition(text, value) {
  try {
    if (!(text instanceof Y.Text) || !text.doc) return null;
    const bytes = fromBase64(value);
    if (!bytes) return null;
    const relative = Y.decodeRelativePosition(bytes);
    return Y.createAbsolutePositionFromRelativePosition(relative, text.doc)?.index ?? null;
  } catch {
    return null;
  }
}

export function preferenceKey(documentId, userId) {
  return `${STORAGE_PREFIX}.${documentId || "unknown"}.${userId || "anonymous"}`;
}

function eventMode(userEvent, changes) {
  const event = String(userEvent || "input");
  if (event.includes("paste") || event.includes("drop") || event.includes("deleteByCut")) return "paste";
  if (changes.some((change) => change.to > change.from && !change.insert)) return "delete";
  if (changes.some((change) => change.to > change.from && change.insert)) return "replace";
  return "insert";
}

// Map one boundary through one old-coordinate edit. End boundaries use the
// right side of inserted text; start boundaries use the left side.
function mapBoundary(value, from, to, insertLength, end = false) {
  if (value < from) return value;
  if (value > to) return value + insertLength - (to - from);
  return from + (end ? insertLength : 0);
}

function recordRange(record, text, fallback = {}) {
  const start = decodePosition(text, record.start);
  const end = decodePosition(text, record.end);
  const hintedStart = Number(record.start_offset ?? fallback.start ?? 0);
  const hintedEnd = Number(record.end_offset ?? fallback.end ?? hintedStart);
  // A deletion can make both relative endpoints resolve at the same live
  // location. Retain the record's last known span as a fallback long enough
  // to recognize deletion of the author's own pending insertion.
  if ((record.kind === "insert" || record.kind === "replace") && Number.isInteger(start)
      && Number.isInteger(end) && end - start < String(record.after || "").length
      && hintedEnd - hintedStart >= String(record.after || "").length) {
    return { start: hintedStart, end: hintedEnd };
  }
  return {
    start: Number.isInteger(start) ? start : hintedStart,
    end: Number.isInteger(end) ? end : Number(record.end_offset ?? fallback.end ?? start ?? 0),
  };
}

export function createRevisionController({
  doc,
  revisions = doc?.getMap?.("revisions"),
  author = "anonymous",
  documentId = "",
  send = null,
  fileOf,
  textOf,
  mayEdit = true,
  now = () => new Date().toISOString(),
} = {}) {
  if (!doc || !revisions) throw new Error("a Y.Doc and revisions map are required");
  const listeners = new Set();
  const observed = new Map();
  const snapshots = new Map();
  const undoManagers = new Map();
  const pendingDecisions = new Map();
  const user = String(author || "anonymous");
  const sessionKey = preferenceKey(documentId, user);
  let session = "";
  let tracking = false;
  let showMarkup = true;
  let disposed = false;
  let group = null;
  let captureContext = null;
  let processingCapture = false;
  let filesMap = null;
  let filesObserver = null;

  try {
    const saved = JSON.parse(globalThis.localStorage?.getItem(sessionKey) || "null");
    tracking = saved?.tracking === true;
    session = typeof saved?.session === "string" ? saved.session : "";
  } catch { /* storage is optional (private windows and SSR) */ }
  try {
    showMarkup = JSON.parse(globalThis.localStorage?.getItem(`${sessionKey}.markup`) || "true") !== false;
  } catch { /* storage is optional */ }

  const save = () => {
    try { globalThis.localStorage?.setItem(sessionKey, JSON.stringify({ tracking, session })); } catch {}
  };
  const emit = () => listeners.forEach((listener) => listener());
  const values = () => [...revisions.entries()].map(([key, value]) => {
    try {
      const record = typeof value === "string" ? JSON.parse(value) : value;
      return record && typeof record === "object" ? { ...record, id: record.id || key } : null;
    } catch { return null; }
  }).filter(Boolean);
  const pending = () => values().filter((record) => record.status === "pending");

  function breakGroup() { group = null; captureContext = null; }

  function setTracking(value) {
    if (!mayEdit) return false;
    const next = Boolean(value);
    if (next && !tracking) session = id();
    if (next !== tracking) breakGroup();
    tracking = next;
    save();
    emit();
    return tracking;
  }

  function registerUndoManager(manager, fileId = "") {
    if (!manager) return () => {};
    undoManagers.set(manager, fileId);
    return () => undoManagers.delete(manager);
  }

  function isUndoOrigin(origin) {
    if (!origin) return false;
    if (origin === "revision-decision" || origin === "remote") return true;
    if (undoManagers.has(origin)) return true;
    return String(origin?.constructor?.name || "") === "UndoManager";
  }

  /* Editor calls this immediately before applying a CodeMirror transaction.
   * The context gives the observer old-coordinate edits and old anchors. */
  function capture(fileId, changes, apply, metadata = {}) {
    if (typeof apply !== "function") return undefined;
    const edits = (changes || []).filter((change) => Number.isInteger(change?.from)
      && Number.isInteger(change?.to) && typeof change?.insert === "string")
      .sort((a, b) => a.from - b.from || a.to - b.to);
    if (!edits.length) return apply();
    const mode = eventMode(metadata.userEvent, edits);
    const previous = group;
    const first = edits[0];
    const contiguous = previous && previous.fileId === fileId && previous.session === session
      && previous.mode === mode && mode !== "replace" && mode !== "paste"
      && (mode === "insert" ? previous.end === first.from
        : (previous.start === first.to || previous.start === first.from || previous.end === first.from));
    if (!contiguous) {
      for (const [manager, managerFile] of undoManagers) {
        if (!managerFile || managerFile === fileId) manager.stopCapturing?.();
      }
    }
    const text = textOf?.(fileId);
    const beforeRecords = text instanceof Y.Text
      ? pending().map((record) => ({ ...record, __range: recordRange(record, text) }))
      : pending().map((record) => ({ ...record }));
    captureContext = { fileId, edits, mode, beforeRecords, text, allowMerge: Boolean(contiguous) };
    try {
      // The editor's Y.Text mutation and every revision-map mutation made by
      // its synchronous observer must be emitted as one Yjs update. Otherwise
      // a collaborator can briefly—or after a dropped frame, permanently—see
      // proposed text without the metadata required to review it.
      let result;
      processingCapture = true;
      doc.transact(() => {
        result = apply();
        // Type observers run during transaction cleanup, after this callback.
        // Materialize the revision now so it shares the text transaction;
        // the observer recognizes processingCapture and does not duplicate it.
        process(fileId, text, { delta: [] });
        snapshots.set(text, text.toString());
      }, metadata.origin || "tracked-edit");
      return result;
    } finally {
      processingCapture = false;
      captureContext = null;
    }
  }

  function deltaEdits(event, previous) {
    let index = 0;
    const edits = [];
    for (const part of event.delta || []) {
      if (part.retain) index += part.retain;
      if (part.delete) {
        edits.push({ from: index, to: index + part.delete, insert: "" });
        index += part.delete;
      }
      if (part.insert != null) {
        const insert = String(part.insert);
        const adjacent = edits[edits.length - 1];
        if (adjacent && adjacent.to === index && !adjacent.insert) adjacent.insert = insert;
        else edits.push({ from: index, to: index, insert });
        index += insert.length;
      }
    }
    return edits.map((edit) => ({ ...edit, before: previous.slice(edit.from, edit.to) }));
  }

  function revisionFor(fileId, edit, mode, state, allowMerge) {
    const before = edit.before ?? "";
    const after = edit.insert ?? "";
    const kind = before && after ? "replace" : before ? "delete" : "insert";
    const foreignProposal = state.some((item) => item.status === "pending" && item.file_id === fileId
      && item.author !== user && (item.kind === "insert" || item.kind === "replace")
      && edit.from < item.__range.end && edit.to > item.__range.start);
    // Tracking off only suppresses ordinary new revisions. An edit that
    // touches somebody else's proposal is still attributed as a dependent
    // revision so it cannot silently rewrite their pending work.
    const eligible = mayEdit && (tracking || foreignProposal) && (kind !== "insert" || after.length > 0);
    let proposal = null;
    for (const candidate of state) {
      if (candidate.status !== "pending" || candidate.file_id !== fileId) continue;
      const range = candidate.__range;
      const own = candidate.author === user && candidate.session === session;
      const proposed = candidate.kind === "insert" || candidate.kind === "replace";
      const overlaps = proposed ? edit.from < range.end && edit.to > range.start
        : edit.from === range.start && edit.to === range.start;
      const adjacent = proposed && edit.from === range.end && edit.to === range.end;
      if (own && proposed && (overlaps || (adjacent && mode === "insert" && (allowMerge || !tracking)))) {
        proposal = { candidate, range, adjacent };
        break;
      }
      if (own && candidate.kind === "delete" && allowMerge && mode === "delete"
          && (edit.from === range.start || edit.to === range.start)) {
        candidate.before = edit.from < range.start ? `${before}${candidate.before}` : `${candidate.before}${before}`;
        candidate.__dirty = true;
        candidate.__range = { start: Math.min(range.start, edit.from), end: Math.min(range.start, edit.from) };
        return;
      }
    }
    if (proposal) {
      const { candidate, range, adjacent } = proposal;
      const currentProposal = candidate.after || "";
      const relativeFrom = Math.max(0, edit.from - range.start);
      const relativeTo = Math.max(relativeFrom, Math.min(currentProposal.length, edit.to - range.start));
      if (adjacent || currentProposal.slice(relativeFrom, relativeTo) === before) {
        candidate.after = adjacent ? currentProposal + after
          : `${currentProposal.slice(0, relativeFrom)}${after}${currentProposal.slice(relativeTo)}`;
        candidate.__dirty = true;
        if (!candidate.after) state.splice(state.indexOf(candidate), 1);
        return;
      }
    }
    if (!eligible) return;
    const timestamp = now();
    const revision = {
      id: id(), file_id: fileId, path: fileOf?.(fileId) || "", author: user, session,
      kind, before, after, start: "", end: "", start_offset: edit.from,
      end_offset: edit.from + after.length, status: "pending", dependencies: [],
      created_at: timestamp, updated_at: timestamp, history: [], __dirty: true,
      // Coordinates are still pre-edit here; process() maps every record
      // through this edit immediately after revisionFor returns.
      __range: { start: edit.from, end: edit.to },
    };
    revision.dependencies = state.filter((item) => item.status === "pending"
      && item.file_id === fileId && item.author !== user
      && edit.from < item.__range.end && edit.to > item.__range.start).map((item) => item.id);
    state.push(revision);
  }

  function process(fileId, text, event) {
    const previousText = snapshots.get(text) || "";
    const context = captureContext && captureContext.fileId === fileId && captureContext.text === text
      ? captureContext : null;
    const edits = context ? context.edits.map((edit) => ({ ...edit, before: previousText.slice(edit.from, edit.to) }))
      : deltaEdits(event, previousText);
    if (!edits.length) return;
    const base = (context?.beforeRecords || pending()).map((record) => ({
      ...record, __range: record.__range || recordRange(record, text),
    }));
    const originalIds = new Set(base.map((record) => record.id));
    const state = base;
    const mode = context?.mode || eventMode("input", edits);
    const allowMerge = context?.allowMerge !== false && mode !== "replace" && mode !== "paste";
    let shift = 0;
    let working = previousText;
    for (const edit of edits) {
      const current = {
        from: edit.from + shift, to: edit.to + shift, insert: edit.insert,
        before: working.slice(edit.from + shift, edit.to + shift),
      };
      revisionFor(fileId, current, mode, state, allowMerge);
      for (const record of state) {
        record.__range.start = mapBoundary(record.__range.start, current.from, current.to, current.insert.length, false);
        record.__range.end = mapBoundary(record.__range.end, current.from, current.to, current.insert.length, true);
      }
      working = `${working.slice(0, current.from)}${current.insert}${working.slice(current.to)}`;
      shift += current.insert.length - (current.to - current.from);
    }
    const currentIds = new Set(state.map((record) => record.id));
    for (const oldId of originalIds) if (!currentIds.has(oldId) && revisions.has(oldId)) revisions.delete(oldId);
    for (const record of state) {
      if (!record.__dirty && !record.__new) continue;
      const range = record.__range;
      const clean = { ...record };
      delete clean.__range; delete clean.__dirty; delete clean.__new;
      clean.start_offset = Math.max(0, range.start);
      clean.end_offset = Math.max(clean.start_offset, range.end);
      clean.start = encodedPosition(text, clean.start_offset, 0);
      clean.end = encodedPosition(text, clean.end_offset, 0);
      clean.updated_at = now();
      revisions.set(clean.id, JSON.stringify(clean));
    }
    const last = edits[edits.length - 1];
    group = {
      fileId, session, mode,
      start: last.from + shift - (last.insert.length || 0), end: last.from + shift,
    };
    emit();
  }

  function observe(fileId, text) {
    if (!(text instanceof Y.Text) || observed.has(text)) return;
    snapshots.set(text, text.toString());
    const handler = (event, transaction) => {
      if (processingCapture) {
        snapshots.set(text, text.toString());
        return;
      }
      if (!transaction?.local || !mayEdit || isUndoOrigin(transaction.origin)) {
        snapshots.set(text, text.toString());
        breakGroup();
        return;
      }
      process(fileId, text, event, transaction);
      snapshots.set(text, text.toString());
    };
    text.observe(handler);
    observed.set(text, handler);
  }

  function attach() {
    const files = doc.getMap("files");
    filesMap = files;
    filesObserver = (event) => {
      for (const [fileId, text] of event.target.entries()) observe(fileId, text);
    };
    for (const [fileId, text] of files) observe(fileId, text);
    files.observe(filesObserver);
  }
  attach();
  const revisionObserver = () => emit();
  revisions.observe(revisionObserver);

  function decide(revisionId, action, requestId = id()) {
    if (!revisionId || !["accept", "reject", "undo"].includes(action)) return Promise.reject(new Error("invalid revision decision"));
    if (pendingDecisions.has(requestId)) return pendingDecisions.get(requestId).promise;
    let resolve;
    let reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    pendingDecisions.set(requestId, { revisionId, action, resolve, reject, promise });
    const request = { type: "revision-decide", revision_id: revisionId, action, request_id: requestId };
    if (!send) {
      pendingDecisions.delete(requestId); reject(new Error("review transport is unavailable")); return promise;
    }
    let result;
    try { result = send(request); } catch (error) {
      pendingDecisions.delete(requestId); reject(error); return promise;
    }
    Promise.resolve(result).then((reply) => {
      if (reply && reply.ok === false) {
        const pendingRequest = pendingDecisions.get(requestId);
        pendingDecisions.delete(requestId);
        pendingRequest?.reject(reply.error instanceof Error ? reply.error : new Error(reply.error || "revision decision failed"));
      }
    }).catch((error) => {
      const pendingRequest = pendingDecisions.get(requestId);
      pendingDecisions.delete(requestId); pendingRequest?.reject(error);
    });
    return promise;
  }

  function receive(message) {
    if (!message || !["revision-decision", "revision-error", "error"].includes(message.type)) return false;
    const requestId = message.request_id;
    const pendingRequest = pendingDecisions.get(requestId);
    if (!pendingRequest || (message.type === "error" && !message.revision_id)) return false;
    pendingDecisions.delete(requestId);
    if (message.type === "revision-decision") pendingRequest.resolve(message);
    else pendingRequest.reject(new Error(message.error || message.message || "revision decision failed"));
    breakGroup(); emit(); return true;
  }

  return {
    revisions, get tracking() { return tracking; }, get enabled() { return tracking; }, get session() { return session; },
    records: values, pending,
    snapshot: () => ({ revisions: values(), enabled: tracking, tracking, showMarkup, session }),
    setTracking, setEnabled: setTracking, breakGroup, capture, registerUndoManager,
    setShowMarkup(value) {
      showMarkup = Boolean(value);
      try { globalThis.localStorage?.setItem(`${sessionKey}.markup`, JSON.stringify(showMarkup)); } catch {}
      emit();
    },
    onChange(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    decide, undo: (revisionId, requestId = id()) => decide(revisionId, "undo", requestId), receive,
    locate(record) { return { file_id: record?.file_id, offset: decodePosition(textOf?.(record?.file_id), record?.start) }; },
    position(record) { return decodePosition(textOf?.(record?.file_id), record?.start); },
    range(record) {
      const text = textOf?.(record?.file_id);
      return { start: decodePosition(text, record?.start), end: decodePosition(text, record?.end) };
    },
    dispose() {
      if (disposed) return;
      disposed = true; revisions.unobserve(revisionObserver);
      filesMap?.unobserve(filesObserver);
      filesMap = null; filesObserver = null;
      for (const [text, handler] of observed) text.unobserve(handler);
      observed.clear(); snapshots.clear(); undoManagers.clear(); listeners.clear(); pendingDecisions.clear();
    },
  };
}
