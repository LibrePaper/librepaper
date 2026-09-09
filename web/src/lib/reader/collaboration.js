// Owns one room and one Yjs collaboration session. Reader keeps the domain
// callbacks and rendering state; this module owns sockets, retries, metadata
// checks, and all observers attached to the session.
import { openRoom as defaultOpenRoom } from "../room.js";
import * as defaultCollab from "../collab.js";
import { keyHeaders } from "../api.js";

const capability = (document_) => JSON.stringify([
  document_?.role ?? null,
  document_?.can_edit ?? null,
  document_?.can_moderate ?? null,
  document_?.can_see_sharing ?? null,
]);

export function createReaderCollaboration({
  slug,
  key = "",
  fetcher = globalThis.fetch,
  openRoom = defaultOpenRoom,
  collab = defaultCollab,
  getIdentity = () => "",
  getCanEdit = () => false,
  onMessage = () => {},
  onConnected = () => {},
  onPeers = () => {},
  onState = () => {},
  onSession = () => {},
  onDocumentChanged = () => {},
  onSource = () => {},
  onSwap = () => {},
  onFiles = () => {},
  onAwareness = () => {},
  retryMs = 1000,
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
}) {
  let room = null;
  let session = null;
  let document_ = null;
  let disposed = false;
  let reconnectGeneration = 0;
  let retryTimer = null;
  let filesCleanup = null;
  let awarenessHandler = null;

  function clearRetry() {
    if (retryTimer !== null) clearTimer(retryTimer);
    retryTimer = null;
  }

  function send(message) {
    // Y updates made while a socket is reconnecting remain in the local Yjs
    // document. `start()` catches them up after the server identity check.
    if (message.type?.startsWith("y-update") && !session?.joined) return undefined;
    return room?.send(message);
  }

  function disposeSession() {
    clearRetry();
    filesCleanup?.();
    filesCleanup = null;
    if (session && awarenessHandler) session.awareness.off("change", awarenessHandler);
    awarenessHandler = null;
    session?.leave();
    session = null;
  }

  function join(nextDocument) {
    disposeSession();
    document_ = nextDocument;
    const canEdit = Boolean(getCanEdit());
    session = collab.join({
      send,
      onPeers: (count) => onPeers(count),
      onState,
      name: getIdentity() || nextDocument.commenting_as || "Anonymous",
      slug,
      createdAt: nextDocument.created_at,
      key,
      mayEdit: canEdit,
    });
    const active = session;
    active.watchSource(() => onSource(active));
    active.onSwap(() => onSwap(active));
    filesCleanup = active.onFiles((events) => onFiles(events, active));
    awarenessHandler = () => onAwareness(active);
    active.awareness.on("change", awarenessHandler);
    onSession(active);
    room?.send(active.open());
    return active;
  }

  function changed(reason) {
    clearRetry();
    onDocumentChanged(reason);
  }

  async function reconnect(up) {
    const current = ++reconnectGeneration;
    if (!up) {
      clearRetry();
      session?.disconnected();
      onConnected(false);
      return;
    }
    const active = session;
    if (!active || disposed) return;
    active.disconnected();
    try {
      const response = await fetcher(`/api/documents/${slug}`, { headers: keyHeaders(key) });
      if (disposed || current !== reconnectGeneration || session !== active) return;
      if (!response.ok) return changed("document");
      const latest = await response.json();
      if (disposed || current !== reconnectGeneration || session !== active) return;
      if ((document_.created_at && latest.created_at !== document_.created_at)
          || capability(latest) !== capability(document_)) return changed("capability");
      room?.send(active.open());
      onConnected(true);
    } catch (error) {
      if (disposed || current !== reconnectGeneration || session !== active) return;
      clearRetry();
      retryTimer = setTimer(() => {
        retryTimer = null;
        void reconnect(true);
      }, retryMs);
    }
  }

  function start(nextDocument) {
    if (disposed) return null;
    if (room) return session;
    document_ = nextDocument;
    room = openRoom(slug, {
      onMessage: (event) => { if (!disposed) onMessage(event); },
      onConnected: reconnect,
      key,
    });
    return join(nextDocument);
  }

  function close() {
    if (disposed) return;
    disposed = true;
    reconnectGeneration += 1;
    clearRetry();
    filesCleanup?.();
    filesCleanup = null;
    if (session && awarenessHandler) session.awareness.off("change", awarenessHandler);
    awarenessHandler = null;
    session?.leave();
    session = null;
    room?.close();
    room = null;
  }

  return {
    start,
    close,
    send,
    sendLive(message) {
      return room?.sendLive?.(message) || { ok: false, error: new Error("room is closed") };
    },
  };
}
