// How a page opens a project, and what it knows once it has.
//
// Opening is one sequence: learn the role, decide whether the project can be
// drawn here at all, load the CRDT if the role needs it, join the room, and
// start editing if the person may. The facts that sequence produces -- may
// this browser edit, is the socket up, is the work safe, how many people are
// here, has the document said who this browser is -- used to be separate
// variables in Reader, written from half a dozen places. They live here, with
// the session and the collaboration, so that every write is in one file.
//
// What the page does when something happens is not decided here. Every effect
// is a callback the page hands in, so the sequence can be driven without one:
// the role is known, the format is known, an editor is ready, a session
// arrived, a connection went up or down. Each callback is called after the
// state it follows has been written, so it can read the new state.
//
// The reactive part is `state`. `session` and `collaboration` are reactive
// too, but raw: the session is a bag of Loro types and functions that the
// collaboration module hands back in its callbacks, compared to this one by
// identity, and a deep proxy would never be equal to it.

import { m } from "../paraglide/messages.js";
import { createReaderCollaboration } from "./collaboration.js";

/**
 * @typedef {{ pending: number, local: boolean, localPending: number, localError: string, joined: boolean }} PersistenceState
 * @typedef {{
 *   mayEdit: boolean,
 *   suggestOnly: boolean,
 *   connected: boolean,
 *   persistence: PersistenceState,
 *   peers: number,
 *   settled: boolean,
 *   openedLocally: boolean,
 * }} OpeningState
 */

/**
 * @param {{
 *   slug: string,
 *   key?: string,
 *   standalone?: boolean,
 *   getIdentity?: () => string,
 *   getAccountId?: () => string,
 *   loadCollab?: () => Promise<any>,
 *   createCollaboration?: typeof createReaderCollaboration,
 *   disposed?: () => boolean,
 *   seed?: ((session: any) => Promise<unknown>) | null,
 *   onMessage?: (message: any) => void,
 *   roleKnown?: (document_: any) => void,
 *   formatKnown?: (format: string) => void,
 *   canRender?: (format: string) => boolean,
 *   noRenderer?: (format: string) => void,
 *   refreshProjection?: () => Promise<unknown>,
 *   editorReady?: (format: string) => void,
 *   onConnected?: (up: boolean) => void,
 *   onSession?: (session: any) => void,
 *   onFiles?: (events: any, session: any) => void,
 *   onAwareness?: (session: any) => void,
 *   onDocumentChanged?: (reason: string) => void,
 *   localStorageFailed?: (error: string) => void,
 *   paint?: () => void,
 *   openFailed?: (error: any) => void,
 * }} options
 */
export function createOpening({
  slug,
  key = "",
  // The demo sandbox: no room, no storage, and a project to seed.
  standalone = false,
  getIdentity = () => "",
  getAccountId = () => "",
  // The CRDT is loaded here and nowhere else. A reader with a link to a
  // published document never joins the source room, so it never asks for the
  // three megabytes of loro wasm behind it; an editor pays for them once, on
  // the way in, rather than before the page has drawn.
  loadCollab = () => import("../collab.js"),
  createCollaboration = createReaderCollaboration,
  // True once the page has been torn down.
  disposed = () => false,
  // Fills a standalone session with its starting project.
  seed = null,
  // Every frame the room delivers, for the page to route.
  onMessage = (_message) => {},
  // The role is known: offer renderers, tell the workspace, forget a project
  // this browser may no longer edit, move off a panel the role is not offered.
  roleKnown = (_document) => {},
  // The source format is known (empty for an empty project).
  formatKnown = (_format) => {},
  // Whether this browser can draw a format.
  canRender = (_format) => true,
  // Says that a format cannot be drawn here.
  noRenderer = (_format) => {},
  // A reader's projection, fetched once the room is open.
  refreshProjection = async () => {},
  // An editor's session is in: warm the engine, configure LaTeX, start editing.
  editorReady = (_format) => {},
  // The socket went up or down, after `state.connected` followed it.
  onConnected = (_up) => {},
  // A session arrived (or was dropped), after `session` followed it.
  onSession = (_session) => {},
  onFiles = (_events, _session) => {},
  onAwareness = (_session) => {},
  onDocumentChanged = (_reason) => {},
  // A local write failed that had not failed before.
  localStorageFailed = (_error) => {},
  // Paints the preview.
  paint = () => {},
  // Says that the session could not open.
  openFailed = (_error) => {},
}) {
  const state = $state({
    // What this browser may do with the document: the role endpoint's answer.
    mayEdit: false,
    // Whether this browser may only suggest: its edits are tracked changes.
    suggestOnly: false,
    // Whether the socket is up.
    connected: false,
    // Whether this browser's work is safe, which is a different question from
    // whether the socket is up. `pending` counts the updates the server has
    // not yet said it has written; `local` says the document is in this
    // browser's own storage, which is what makes a reload safe while the
    // socket is down.
    persistence: { pending: 0, local: false, localPending: 0, localError: "", joined: false },
    peers: 1,
    // Whether the document has said who this browser is. Until it has, the
    // column is drawn empty rather than as one audience's and then the other's.
    settled: false,
    // Set when the page was opened from this browser's copy because the server
    // could not be reached, so the page says it may be missing newer changes.
    openedLocally: false,
  });
  // Raw on purpose, as the header says.
  let session = $state.raw(null);
  let collaboration = $state.raw(null);

  /// What the page hears from a session joined to a room.
  function sessionEvents() {
    return {
      onPeers: (count) => { if (!disposed()) state.peers = Math.max(state.peers, count); },
      onState: (next) => {
        // Teardown sets this before it closes the collaboration, so a local
        // write landing in that window has a UI to miss.
        if (disposed()) return;
        const newlyFailed = next.localError && next.localError !== state.persistence.localError;
        state.persistence = next;
        if (newlyFailed) localStorageFailed(next.localError);
      },
      onSession: (active) => {
        session = active;
        onSession(active);
      },
      onFiles,
      onAwareness,
    };
  }

  const opening = {
    state,
    get session() {
      return session;
    },
    get collaboration() {
      return collaboration;
    },
    // A close is only worth interrupting when the work has reached neither this
    // browser's storage nor the server.
    get atRisk() {
      return Boolean(state.mayEdit && (state.persistence.localError || (state.persistence.pending && !state.persistence.local)));
    },
    /// The document has answered for this browser, or has been found unable to.
    settle() {
      state.settled = true;
    },
    /// Leave the room.
    close() {
      collaboration?.close();
    },

    /// Join the room. The CRDT is loaded only when the role syncs the source.
    async start(document_, { sourceSync = true } = {}) {
      collaboration?.close();
      const collab = sourceSync ? await loadCollab() : null;
      // Awaiting the module gave the reader time to be torn down or replaced.
      if (disposed()) return;
      collaboration = createCollaboration({
        slug,
        key,
        collab,
        getIdentity,
        // Only a signed-in person has an account; the id is empty for anybody else.
        getAccountId,
        getCanEdit: () => state.mayEdit,
        getSuggestOnly: () => state.suggestOnly,
        sourceSync,
        standalone,
        onMessage,
        onConnected: (up) => {
          state.connected = up;
          onConnected(up);
        },
        ...sessionEvents(),
        onDocumentChanged,
      });
      session = collaboration.start(document_);
      if (seed) await seed(session);
    },

    /// What this browser may do with the document, and whether it can render it
    /// at all.
    async prepare(document_) {
      // The role endpoint answer is authoritative for every affordance.
      const ROLES = ["reader", "commenter", "suggester", "editor", "owner"];
      const allowed = ROLES.indexOf(document_.role) >= ROLES.indexOf("suggester");
      // The desktop app must see the resolved permission on its first
      // configuration. In particular, account examples arrive as editable
      // Quarto documents; configuring before this assignment leaves the
      // desktop app inactive and the pane stuck on its Markdown fallback.
      state.mayEdit = Boolean(allowed);
      state.suggestOnly = document_.role === "suggester";
      state.openedLocally = Boolean(document_.local_copy);
      roleKnown(document_);
      // Reader and commenter links receive an allowlisted projection of the
      // current project. They share the renderer but never join CRDT sync.
      if (!state.mayEdit) {
        await opening.start(document_, { sourceSync: false });
        await refreshProjection();
        return;
      }
      // Rendering follows the durable source format. Generated-result identity
      // is deliberately absent: a reader compiles this source on demand.
      // An empty project has no format; the first file added gives it one.
      const format = document_.source_format ||
        (document_.execution_engine === "quarto" ? "quarto" : "");
      formatKnown(format);
      // Every reader compiles from source when the browser has the engine. A
      // missing engine produces an actionable local-tool message.
      if (format && !canRender(format)) {
        noRenderer(format);
        state.settled = true;
        return;
      }
      state.settled = true;
      await opening.start(document_);
      editorReady(format);
    },

    /// The session frames of the room. Returns true when the event was one of
    /// them and has been handled.
    ///
    /// The shared document: the state of the session as it stands, one more
    /// change to it, what the server has written, who else is in it, or where
    /// their carets are. A reader receives all of this too -- that is how they
    /// see the current text -- and sends none of it.
    receive(event) {
      if (event.type === "doc-state") {
        const active = session;
        active
          ?.start(event)
          .then(() => {
            if (active !== session) return;
            paint();
          })
          .catch((error) => {
            openFailed(error);
          });
        state.peers = event.count || 1;
        return true;
      }
      if (event.type === "doc-rows") {
        // The rows this browser's vector did not cover, plus whatever is still
        // in the server's buffer (§6.2). It is a join like `doc-state`, and it
        // is awaited the same way: `doc-rows` can arrive as the whole join
        // reply on its own, so `session.rows` awaits hydration itself, and it
        // is sequenced behind any `start` that is still mid-fetch for a
        // referenced base (project-session.js `joinGate`), so it can no longer
        // be assumed synchronous.
        const active = session;
        active
          ?.rows(event)
          .then(() => {
            if (active !== session) return;
            paint();
          })
          .catch((error) => {
            openFailed(error);
          });
        return true;
      }
      if (event.type === "doc-gap") {
        // The server refused a batch because it did not cover what that batch
        // started from, and dropped it rather than queueing it (§5 step 4).
        // Nothing is lost: exporting from the vector the server named resends
        // the refused operations along with anything else it is missing.
        session?.gap(event.vector);
        return true;
      }
      if (event.type === "doc-update") {
        session?.apply(event.update);
        return true;
      }
      if (event.type === "doc-presence") {
        session?.applyPresence(event.update);
        return true;
      }
      if (event.type === "doc-ack") {
        // Retire transmission bookkeeping. An empty catch-up can be acknowledged
        // while earlier work is still buffered, so this does not prove saving.
        session?.acknowledge(event.upTo || 0);
        return true;
      }
      if (event.type === "doc-durable") {
        session?.durable(event.vector);
        return true;
      }
      if (event.type === "doc-peers") {
        state.peers = event.count || 1;
        return true;
      }
      return false;
    },

    /// What Ctrl-S should say about where the work is, or "" for nothing.
    ///
    /// "Saved" is the word to be careful with. This browser's own storage is
    /// not a save: it is evictable, it is one device, and it holds nothing the
    /// server has not also been sent only while the socket is down. So an
    /// offline Ctrl-S says where the work currently is and what will happen to
    /// it, and leaves the word alone. Only the server copy is called saved.
    report() {
      const { persistence, connected } = state;
      // A local write that failed or is still in flight is not worth
      // reassuring anybody about; the failure says so on its own.
      if (persistence.localError || persistence.localPending) return "";
      if (!connected) {
        if (!persistence.local) return "";
        return persistence.pending
          ? m.readerc_offline_pending()
          : m.readerc_offline_saved();
      }
      if (!persistence.joined || persistence.pending) return "";
      return m.readerc_saved_on_server();
    },
  };
  return opening;
}
