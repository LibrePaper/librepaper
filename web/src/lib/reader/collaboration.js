// Owns one room and one Loro collaboration session. Reader keeps the domain
// callbacks and rendering state; this module owns sockets, retries, metadata
// checks, and all observers attached to the session.
//
// Who owns what, and in what order it is given up:
//
// - This module owns the room, the reconnect attempt, the metadata and
//   capability check that gates a rejoin, and the identity of the *current*
//   session. Nothing else decides which session Reader is looking at.
// - `project-session.js` owns the CRDT, the presence store, local persistence
//   status and durable coverage. It alone decides whether this browser's work
//   is safe; an acknowledgement or an empty send queue never establishes that.
// - `Editor.svelte` owns the editor instance and its subscriptions, and
//   `reader/render-coordinator.js` owns preview work. Both outlive individual
//   join frames and are torn down by their own mount/unmount.
//
// Teardown runs outwards. `disposeSession` cancels the session generation
// first, so every callback below stops being delivered before anything is
// actually released; then the retry timer, then the observers this module
// registered, then `session.leave()`. `close()` does the same and adds the
// room. A session that has been left still finishes an outstanding local
// persistence write -- cancelling delivery to a disposed UI is not the same
// as cancelling recoverable work -- but it reports nothing when it lands.
//
// `collab` is handed in rather than imported. It is the only door to
// loro-crdt, and loro-crdt is three megabytes of wasm: importing it here would
// put the CRDT on the critical path of every reader, including the ones who
// have a published document and no source room to join at all. The caller
// loads it -- with a dynamic `import()` -- on the one path that needs it, so
// everything below stays synchronous and the bytes stay off a reader's first
// paint.
import { openRoom as defaultOpenRoom } from "../room.js";
import { keyHeaders } from "../api.js";
import { assertSameProject, projectIdentity } from "../project-identity.js";
import { createGeneration } from "./generation.js";

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
  collab,
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
  sourceSync = true,
  retryMs = 1000,
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
}) {
  let room = null;
  let session = null;
  let document_ = null;
  let disposed = false;
  const reconnects = createGeneration();
  // Which session is the current one. A session that has been replaced or
  // left can still have work in flight -- a persistence write completing, a
  // join step that has not noticed yet -- and none of it may reach a UI that
  // has moved on.
  const sessions = createGeneration();
  let retryTimer = null;
  let filesCleanup = null;
  // EphemeralStore hands back an unsubscribe function rather than taking a
  // handler it can be asked to forget later, so what is kept is the way out.
  let presenceCleanup = null;

  function clearRetry() {
    if (retryTimer !== null) clearTimer(retryTimer);
    retryTimer = null;
  }

  function send(message) {
    // Updates made while a socket is reconnecting remain in the local
    // document. `start()` catches them up after the server identity check.
    if (message.type?.startsWith("doc-update") && !session?.joined) return undefined;
    return room?.send(message);
  }

  function disposeSession() {
    sessions.cancel();
    clearRetry();
    filesCleanup?.();
    filesCleanup = null;
    presenceCleanup?.();
    presenceCleanup = null;
    session?.leave();
    session = null;
  }

  function join(nextDocument) {
    disposeSession();
    document_ = nextDocument;
    if (!sourceSync) return null;
    const canEdit = Boolean(getCanEdit());
    // Begun before `collab.join`, because the session reports its initial
    // state from inside its own constructor.
    const replaced = sessions.begin();
    const live = () => !disposed && !replaced();
    session = collab.join({
      send,
      onPeers: (count) => { if (live()) onPeers(count); },
      onState: (state) => { if (live()) onState(state); },
      // Empty rather than "Anonymous": the session names them that itself,
      // and a literal here would make every unnamed reader the same person
      // as far as the colour is concerned.
      name: getIdentity() || nextDocument.commenting_as || "",
      slug,
      documentId: nextDocument.document_id,
      createdAt: nextDocument.created_at,
      key,
      mayEdit: canEdit,
    });
    const active = session;
    active.watchSource(() => { if (live()) onSource(active); });
    active.onSwap(() => { if (live()) onSwap(active); });
    filesCleanup = active.onFiles((events) => { if (live()) onFiles(events, active); });
    presenceCleanup = active.ephemeral.subscribe(() => { if (live()) onAwareness(active); });
    onSession(active);
    // Reader/commenter sessions use this socket only for rendered
    // annotations. They receive the initial comment hello, but never send a
    // document open request or receive source state.
    if (sourceSync) room?.send(active.open());
    return active;
  }

  function changed(reason) {
    clearRetry();
    onDocumentChanged(reason);
  }

  async function reconnect(up) {
    if (disposed) return;
    const stale = reconnects.begin();
    clearRetry();
    if (!up) {
      session?.disconnected();
      onConnected(false);
      return;
    }
    const active = session;
    if ((sourceSync && !active) || disposed) return;
    active?.disconnected();
    try {
      const response = await fetcher(`/api/documents/${slug}`, { headers: keyHeaders(key) });
      if (disposed || stale() || session !== active) return;
      if (!response.ok) return changed("document");
      const latest = await response.json();
      if (disposed || stale() || session !== active) return;
      if (document_.document_id || latest.document_id) {
        try {
          assertSameProject(
            projectIdentity({ server: globalThis.location?.origin, documentId: document_.document_id }),
            projectIdentity({ server: globalThis.location?.origin, documentId: latest.document_id }),
          );
        } catch {
          return changed("document");
        }
      } else if ((document_.created_at || latest.created_at)
          && document_.created_at !== latest.created_at) {
        return changed("document");
      }
      if (capability(latest) !== capability(document_)) return changed("capability");
      if (sourceSync) room?.send(active.open());
      onConnected(true);
    } catch (error) {
      if (disposed || stale() || session !== active) return;
      clearRetry();
      retryTimer = setTimer(() => {
        retryTimer = null;
        if (!disposed && !stale() && session === active) void reconnect(true);
      }, retryMs);
    }
  }

  function openLocal(nextDocument) {
    if (disposed) return null;
    if (session) return session;
    document_ = nextDocument;
    return join(nextDocument);
  }

  function connect() {
    if (disposed || room) return session;
    if (!document_) throw new Error("open the local project before connecting");
    room = openRoom(slug, {
      onMessage: (event) => { if (!disposed) onMessage(event); },
      onConnected: reconnect,
      key,
    });
    if (sourceSync) room.send(session.open());
    return session;
  }

  function start(nextDocument) {
    openLocal(nextDocument);
    connect();
    return session;
  }

  function close() {
    if (disposed) return;
    disposed = true;
    reconnects.cancel();
    sessions.cancel();
    clearRetry();
    filesCleanup?.();
    filesCleanup = null;
    presenceCleanup?.();
    presenceCleanup = null;
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
