// A document as one client holds it.
//
// The source is a Loro document -- a CRDT -- so two people typing in the same
// sentence converge on the same text without either of them waiting for the
// other. What travels is a small binary update per change.
//
// The server is not a relay. It holds the same document, with `loro`, and it is
// the durable copy: the session outlives every socket, so a tab closed by
// mistake, a laptop that dies or a browser that restarts loses nothing. Three
// things follow, and they are the whole of what is new here.
//
// **Reading and editing join the same session.** A reader receives updates and
// sends none; the server drops anything a reader sends, and so does this,
// which is a courtesy rather than the enforcement.
//
// **Relaying is not durability.** An update is relayed the moment it lands and
// acknowledged only once the server has written it. Every update this browser
// makes is held until its `doc-ack` arrives, so `pending` is the honest answer
// to "is my work safe", and the toolbar says that rather than saying "saved"
// because a socket happens to be open.
//
// **Nothing is lost to a disconnection.** A persistence adapter can retain the
// CRDT across client restarts; on reconnect the whole local state is sent and
// merged, so changes made on either side of the gap reach the other.

import { LoroDoc, LoroText, EphemeralStore, VersionVector } from "loro-crdt";
import { checkPlacement, folderPaths, inside, parentPath, relocation, topEntries } from "./file-manager.js";

// Keep each JSON WebSocket frame comfortably below the server's one-megabyte
// receive limit. Base64 expands the binary update by a third, and the JSON
// envelope adds a little more, so the chunk is deliberately smaller than the
// apparent limit. The server reassembles these chunks before applying them as
// one document update.
const UPDATE_CHUNK_BYTES = 600_000;

// The origin every change to the directory is committed under: a file added,
// renamed, moved or removed, and the folders that hold them.
//
// It exists so the editor's undo manager can be told to leave those alone.
// The manager is one per document -- it has to be, because a person's edits
// are spread across the files they have open -- and it merges everything
// within a second into a single step. Creating a file and typing the first
// word into it falls inside that second, so one Ctrl-Z took the file away
// along with the word: `textOf` returned null and the file left the list,
// which reads as data loss rather than as an undo.
//
// `Editor.svelte` names this prefix in `excludeOriginPrefixes`. The origin is
// not persisted (Loro keeps `message`, not `origin`), so this is a runtime
// label for the local undo manager and nothing that reaches the wire.
export const DIRECTORY_ORIGIN = "directory";

// Carets can change once per editor transaction. Awareness is ephemeral, so
// sending the latest state at this rate is enough for a smooth cursor while
// avoiding one room frame per keystroke.
const AWARENESS_THROTTLE_MS = 100;

// Updates are binary and the room's socket carries JSON, so they travel
// base64-encoded. A keystroke is a few dozen bytes either way.
const encode = (bytes) => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};
const decode = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));


// An id for a file: twelve hex characters, which is what `session.rs` mints
// and enough randomness that two people creating a file at the same instant
// do not collide.
function mintId() {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function randomPresenceId() {
  return crypto.randomUUID?.() || `${mintId()}${mintId()}`;
}

export function uniquePresences(states, { localClient = null, localTab = "" } = {}) {
  const unique = new Map();
  for (const [client, state] of states || []) {
    if (client === localClient || !state?.user) continue;
    const tab = String(state.user.tab || "");
    if (localTab && tab === localTab) continue;
    unique.set(tab || `client:${client}`, { client, state });
  }
  return [...unique.entries()].map(([key, value]) => ({ key, ...value }));
}

/// Creates one client project session. `send` writes to its room transport;
/// `onPeers` is told how many people are in it; `onState` is told whenever the
/// answer to "is this browser's work safe" changes.
///
/// `mayEdit` is false for a reader, who joins to receive the text and never to
/// change it.
export function createProjectSession({
  send,
  onPeers,
  onState,
  name,
  mayEdit = true,
  persistence = null,
  fetchReference,
  presenceId = randomPresenceId,
  presenceColor = "",
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
}) {
  const doc = new LoroDoc();
  // A document is a directory: `files` holds one LoroText per file under an id
  // of its own, `paths` says what each of them is called, and `meta.main`
  // names the one the renderer is run on. Keying the texts by id rather than
  // by path is what makes a rename free -- the name moves and the text stays
  // where it is, so a keystroke made during a rename lands where it was always
  // going to land.
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  // Path to digest. An asset's bytes are never in the shared document -- they
  // are in the store, under their own digest -- so what travels here is the
  // name somebody gave it and what is at that name.
  const assets = doc.getMap("assets");
  const meta = doc.getMap("meta");
  // The four maps that are the directory itself. A change to any of them is a
  // change to the file list; a change under one of them is not, because a
  // file's text sits inside `files`.
  const directory = new Set([files.id, paths.id, assets.id, meta.id]);
  const store = new EphemeralStore(30000);

  // The text the editor and the preview are following, and who is following
  // it. Which text that is can change under them -- when the maps arrive from
  // the server, or when somebody names a different main file -- so the
  // watchers are held here and moved across rather than re-registered by every
  // caller.
  let bound = null;
  const watchers = new Set();
  const swaps = new Set();
  // Track subscriptions: Map from text.id -> Map from watcher -> unsub function
  const textSubs = new Map();

  function mainText() {
    const id = meta.get("main");
    const text = id ? files.get(id) : null;
    // Check if the value is a LoroText by checking its kind
    return text?.kind?.() === "Text" ? text : null;
  }

  function rebind() {
    const next = mainText();
    // By container id, not by handle: `files.get(id)` mints a fresh LoroText
    // wrapper on every call, so comparing the objects reads as "the main file
    // was swapped" once per commit -- once per keystroke -- and everything
    // hanging off `onSwap` tears itself down and builds again, the editor and
    // its caret among it. A handle stays live across edits, so the one already
    // bound is kept rather than replaced by an equivalent.
    if (next?.id === bound?.id) return;
    // Unsubscribe all watchers from the old bound text
    if (bound && textSubs.has(bound.id)) {
      const subs = textSubs.get(bound.id);
      for (const unsub of subs.values()) {
        unsub();
      }
      textSubs.delete(bound.id);
    }
    bound = next;
    // Subscribe all current watchers to the new bound text
    if (bound) {
      const subs = new Map();
      for (const watcher of watchers) {
        const unsub = bound.subscribe((event) => watcher(event));
        subs.set(watcher, unsub);
      }
      textSubs.set(bound.id, subs);
    }
    for (const swap of swaps) swap();
  }

  // Subscribe to document changes to detect when main file or files/paths change.
  // Each subscription will trigger rebind to update the bound text if needed.
  doc.subscribe(() => {
    rebind();
  });
  rebind();

  // Updates this browser has made and the server has not yet said are durable,
  // by the number they were sent under.
  const unacknowledged = new Map();
  let seq = 0;
  // Whether this browser has been given the server's copy since the socket
  // last came up. Until it has, what is here may be behind.
  let joined = false;
  let presenceTimer = null;
  const pendingPresence = new Set();
  let presenceConnection = 0;
  let announcedConnection = -1;
  let left = false;
  // Whether the document is in this client's own storage, which is what makes
  // a reload safe while the socket is down.
  let local = !mayEdit;
  let localPending = 0;
  let localError = "";

  // A read-only client keeps nothing: it has no local changes to lose. An
  // editor can still work when storage is unavailable, but `local` remains
  // false so its UI does not call those changes safe on this device.
  let persister = null;
  const report = () => onState?.({ pending: unacknowledged.size, local, localPending, localError, joined });
  try {
    persister = persistence?.open?.(doc, {
      hydrated() { local = true; report(); },
      writing(count = 1) {
        localPending = Math.max(0, Number(count) || 0);
        if (localPending) localError = "";
        report();
      },
      persisted() {
        local = true;
        localPending = 0;
        localError = "";
        report();
      },
      failed(error) {
        local = false;
        localPending = 0;
        localError = error?.message || String(error || "local persistence failed");
        report();
      },
    }) || null;
  } catch (error) {
    localError = error?.message || String(error || "local persistence failed");
  }
  report();

  store.set("user", {
    name: name || "Anonymous",
    color: presenceColor,
    tab: presenceId(),
  });

  // Anything this browser changes is sent on and held until it is
  // acknowledged; subscribeLocalUpdates only fires for local changes, so the
  // origin check is not needed. An update out of indexeddb is this
  // browser's own past work, and is sent on for the same reason a keystroke
  // is: the server may never have seen it.
  function sendUpdate(update, mine) {
    if (update.byteLength <= UPDATE_CHUNK_BYTES) {
      send({ type: "doc-update", update: encode(update), seq: mine });
      return;
    }
    const chunks = Math.ceil(update.byteLength / UPDATE_CHUNK_BYTES);
    send({ type: "doc-update-start", seq: mine, size: update.byteLength, chunks });
    for (let index = 0; index < chunks; index++) {
      const from = index * UPDATE_CHUNK_BYTES;
      send({
        type: "doc-update-chunk",
        seq: mine,
        index,
        update: encode(update.subarray(from, Math.min(update.byteLength, from + UPDATE_CHUNK_BYTES))),
      });
    }
    send({ type: "doc-update-end", seq: mine });
  }

  if (mayEdit) {
    doc.subscribeLocalUpdates((update) => {
      const mine = ++seq;
      unacknowledged.set(mine, update);
      report();
      sendUpdate(update, mine);
    });
  }

  function clearPresenceTimer() {
    if (presenceTimer !== null) clearTimer(presenceTimer);
    presenceTimer = null;
  }

  function sendPresence(bytes) {
    if (!bytes.byteLength || left) return;
    send({ type: "doc-presence", update: encode(bytes) });
    announcedConnection = presenceConnection;
  }

  function flushPresence() {
    presenceTimer = null;
    if (!pendingPresence.size || left) return;
    const keys = [...pendingPresence];
    pendingPresence.clear();
    // Encode only changed keys, not the full state. A burst may have advanced
    // the presence state several times, and peers only need the latest state
    // for each key. Encoding per key avoids sending redundant full-state updates.
    let bytes = new Uint8Array();
    for (const key of keys) {
      const keyBytes = store.encode(key);
      if (keyBytes) {
        bytes = new Uint8Array([...bytes, ...keyBytes]);
      }
    }
    sendPresence(bytes);
  }

  if (mayEdit) {
    store.subscribe((event) => {
      // Presence changes from peers (and expiry of stale peers) are for local
      // rendering only. Relaying them would create a broadcast echo loop.
      onPeers?.(Object.keys(store.getAllStates()).length);
      if (left || event.by !== "local") return;

      const changed = event.added.concat(event.updated, event.removed);
      for (const key of changed) pendingPresence.add(key);

      // A departure must reach peers before a coalescing delay can hide it.
      if (event.removed.length) {
        clearPresenceTimer();
        flushPresence();
        return;
      }

      // Keep local changes made while disconnected for the next join, where the
      // current state is announced once the socket is usable.
      if (!joined || presenceTimer !== null) return;
      presenceTimer = setTimer(flushPresence, AWARENESS_THROTTLE_MS);
    });
  }

  function announcePresence() {
    if (!mayEdit || left) return;
    clearPresenceTimer();
    flushPresence();
    if (store.get("user") && announcedConnection !== presenceConnection) {
      const bytes = store.encode("user") || new Uint8Array();
      sendPresence(bytes);
    }
  }

  /// Everything this browser has, as one update. Sending it after a join is
  /// how the changes made while the socket was down reach the server: applying
  /// it is idempotent, so it costs nothing when there were none.
  function catchUp(vector) {
    // A server can include the state vector used for its state response. In
    // that case only this browser's missing operations are sent back; older
    // servers omit it, so the bounded full-history path remains compatible.
    let whole;
    if (vector) {
      // And the mirror of the above: what arrives is bytes, and what `from`
      // wants is a version vector. Handing it the bytes is not an error --
      // it reads as an empty vector and resends the whole history, which is
      // correct but is the thing this branch exists to avoid.
      whole = doc.export({ mode: "update", from: VersionVector.decode(decode(vector)) });
    } else {
      // Export the full history from an empty version vector
      whole = doc.export({ mode: "update" });
    }
    const mine = ++seq;
    // One entry stands for every update it contains: the acknowledgment of
    // this send is the acknowledgment of all of them.
    unacknowledged.clear();
    unacknowledged.set(mine, whole);
    report();
    sendUpdate(whole, mine);
  }

  return {
    doc,
    files,
    paths,
    meta,
    ephemeral: store,
    /// The main file's text as it stands. A getter rather than a field,
    /// because which text that is is not known until the session arrives.
    get text() {
      return bound;
    },
    get joined() {
      return joined;
    },

    /// The path the main file is known by, which is what its format is read
    /// from. Empty until the maps arrive.
    mainPath() {
      const id = meta.get("main");
      return (id && paths.get(id)) || "";
    },

    /// The main file's id, which is what the editor opens on and what a
    /// diagnostic with no file of its own belongs to.
    mainId() {
      return meta.get("main") || "";
    },

    /// Follows the main file's text, across the text itself being swapped for
    /// another -- which happens once on every document migrated from before
    /// there were directories, and again whenever somebody names a different
    /// main file.
    watchSource(watcher) {
      watchers.add(watcher);
      if (bound) {
        // Subscribe this new watcher to the currently bound text
        const unsub = bound.subscribe((event) => watcher(event));
        if (!textSubs.has(bound.id)) textSubs.set(bound.id, new Map());
        textSubs.get(bound.id).set(watcher, unsub);
      }
    },

    /* --------------------------------------------------------- the directory */

    // Empty directories are metadata, never fake source files. Using one key
    // per path lets unrelated folder creations merge, and the existing server
    // metadata byte ceiling accounts for the names. Older clients ignore them.
    folders() {
      return [...meta.keys()].filter((key) => key.startsWith("folder:")).map((key) => key.slice(7));
    },

    addFolder(path, rules) {
      if (!mayEdit) throw new Error("This project is read-only.");
      path = checkPlacement(rules, { kind: "folder", path }, this.list(), this.folders());
      meta.set(`folder:${path}`, true);
      doc.commit({ origin: DIRECTORY_ORIGIN });
      return path;
    },

    relocate(entries, destination, rules, rename = false) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const plan = relocation(this.list(), this.folders(), entries, destination, rules, rename);
      // Remove asset keys first, so a batch move never overwrites a source.
      for (const file of plan.files) if (file.kind === "asset") assets.delete(file.previousPath);
      for (const file of plan.files) {
        if (file.kind === "asset") assets.set(file.path, file.sha);
        else paths.set(file.id, file.path);
      }
      for (const path of plan.oldFolders) meta.delete(`folder:${path}`);
      for (const path of [...plan.folders, ...plan.parents]) meta.set(`folder:${path}`, true);
      doc.commit({ origin: DIRECTORY_ORIGIN });
      return plan;
    },

    removeEntries(entries) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const roots = topEntries(entries);
      const current = this.list();
      const folders = folderPaths(current, this.folders());
      for (const entry of roots) {
        const exists = entry.kind === "folder" ? folders.includes(entry.path) : current.some((file) => file.id === entry.id && file.kind === entry.kind && file.path === entry.path);
        if (!exists) throw new Error(`${entry.path}: this item changed or was removed. Select it again.`);
      }
      const selected = (path) => roots.some((entry) => path === entry.path || (entry.kind === "folder" && inside(path, entry.path)));
      const removed = current.filter((file) => selected(file.path));
      if (removed.some((file) => file.main)) throw new Error("Choose another main file before deleting this file or its folder.");
      for (const file of removed) {
        if (file.kind === "asset") assets.delete(file.path);
        else { files.delete(file.id); paths.delete(file.id); }
      }
      for (const path of this.folders()) if (selected(path)) meta.delete(`folder:${path}`);
      for (const entry of roots) {
        const parent = parentPath(entry.path);
        if (parent) meta.set(`folder:${parent}`, true);
      }
      doc.commit({ origin: DIRECTORY_ORIGIN });
    },

    duplicateEntry(entry, path, rules) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const file = this.list().find((file) => file.kind === entry.kind && file.id === entry.id && file.path === entry.path);
      if (!file) throw new Error("This file changed or was removed. Select it again.");
      path = checkPlacement(rules, { ...file, path }, this.list(), this.folders());
      if (file.kind === "text") return this.addText(path, this.textOf(file.id).toString());
      assets.set(path, file.sha);
      return path;
    },

    /// Every file in the document: its id, its path, and whether it is the
    /// main one. Sorted with the main file first and the rest by path, which
    /// is the order the file list shows and the order a person reads a paper
    /// in.
    list() {
      const id = meta.get("main");
      const texts = [...paths.entries()]
        .filter(([file]) => {
          const val = files.get(file);
          return val?.kind?.() === "Text";
        })
        .map(([file, path]) => ({ id: file, path, kind: "text", main: file === id }));
      const figures = [...assets.entries()].map(([path, sha]) => ({
        id: path,
        path,
        sha,
        kind: "asset",
        main: false,
      }));
      return [...texts, ...figures].sort((a, b) => {
        if (a.main !== b.main) return a.main ? -1 : 1;
        return a.path.localeCompare(b.path);
      });
    },

    /// The whole directory as a renderer takes it. Assets are the digests
    /// only: their bytes are not in the shared document, and whoever renders
    /// fetches them.
    tree() {
      const texts = Object.create(null);
      const entries = Object.create(null);
      for (const [file, text] of files.entries()) {
        const path = paths.get(file);
        if (path && text?.kind?.() === "Text") {
          texts[path] = text.toString();
          entries[path] = { kind: "text", id: file };
        }
      }
      const digests = Object.create(null);
      for (const [path, sha] of assets.entries()) {
        digests[path] = sha;
        entries[path] = { kind: "asset", sha };
      }
      return { main: this.mainPath(), texts, digests, files: entries };
    },

    /// The text at a path, for the caller that has a path and not an id --
    /// which is what a diagnostic carries.
    idOf(path) {
      for (const [file, at] of paths.entries()) if (at === path) return file;
      return "";
    },

    textOf(id) {
      const text = files.get(id);
      return text?.kind?.() === "Text" ? text : null;
    },

    /// Makes a file. One transaction, so no peer ever sees a text without the
    /// name it is known by.
    addText(path, body = "") {
      const id = mintId();
      const text = new LoroText();
      if (body) {
        text.insert(0, body);
      }
      files.setContainer(id, text);
      paths.set(id, path);
      doc.commit({ origin: DIRECTORY_ORIGIN });
      return id;
    },

    /// Renames a file, which moves a string and leaves the words where they
    /// are. This is why the texts are keyed by an id: somebody typing into
    /// this file at this moment keeps what they typed.
    renameFile(id, path, kind) {
      if (kind === "asset" || (kind === undefined && assets.has(id))) {
        if (!assets.has(id)) return;
        const sha = assets.get(id);
        assets.delete(id);
        assets.set(path, sha);
        doc.commit({ origin: DIRECTORY_ORIGIN });
        return;
      }
      if (kind === "text" || (kind === undefined && paths.has(id))) {
        paths.set(id, path);
        doc.commit({ origin: DIRECTORY_ORIGIN });
      }
    },

    /// Removes a file, its name with it. The main file is never removed here;
    /// the file list refuses it, because a document has to be something.
    removeFile(id) {
      files.delete(id);
      paths.delete(id);
      doc.commit({ origin: DIRECTORY_ORIGIN });
    },

    removeAsset(path) {
      assets.delete(path);
    },

    putAsset(path, sha) {
      assets.set(path, sha);
    },

    /// Names the main file: the one a renderer is run on, and the one the
    /// document's format is read from.
    setMain(id) {
      meta.set("main", id);
    },

    /// Called whenever the directory changes -- a file added, renamed,
    /// removed, or made the main one -- so the list can be redrawn.
    onFiles(watcher) {
      // The watcher hears once per batch, not once per event. A rename moves a
      // path and a delete clears a digest in the same commit, and a file list
      // that redrew itself for each of those separately would show a state that
      // never existed.
      //
      // An event carries its absolute path from the root, so the first segment
      // says which of the four maps it happened under. Matching on that rather
      // than on the maps' own container ids is what makes this reach INSIDE a
      // file: typing into an included text is an event on that text, whose
      // path begins at `files`, and a reader that watched only the maps would
      // never repaint for it.
      const roots = new Set(["files", "paths", "assets", "meta"]);
      const unsub = doc.subscribe((eventBatch) => {
        const relevantEvents = eventBatch.events.filter(
          (event) => roots.has(event.path?.[0]) || directory.has(event.target),
        );
        if (relevantEvents.length > 0) {
          watcher(relevantEvents);
        }
      });
      return unsub;
    },

    /// Whether a batch from `onFiles` moved the directory -- a file added,
    /// renamed, moved, a folder made, a figure stored -- rather than only the
    /// words inside a file, which arrive on the same subscription because a
    /// file's text is a container under `files`. Asked here rather than worked
    /// out by the caller: which containers are the directory is this module's
    /// to know, and a caller that guessed at one of them -- the reader watched
    /// `files` alone -- silently stopped redrawing for every rename and move,
    /// which are changes to `paths`.
    directoryChanged(events) {
      return !Array.isArray(events) || events.some((event) => directory.has(event.target));
    },

    /// Says which file this browser's caret is in, so the file list can show
    /// who is where. The caret's own position is published by editor binding
    /// against the text it was made in, so it already paints in the right
    /// file; this is for the list.
    inFile(id) {
      const user = store.get("user") || {};
      store.set("user", { ...user, file: id });
    },

    /// Who is in which file: an id to the initials of the people in it.
    whereEveryoneIs() {
      const by = new Map();
      const localUser = store.get("user");
      const localTab = localUser?.tab || "";
      const states = store.getAllStates();
      for (const [key, state] of Object.entries(states)) {
        // Skip if this is the local user's own state or if it's the same tab
        if (key === "user" && state.tab === localTab) continue;
        // Only process user states that have file information
        if (key === "user" && state.file) {
          const name = state?.name || "?";
          const initials = name
            .split(/\s+/)
            .filter(Boolean)
            .slice(0, 2)
            .map((word) => word[0].toUpperCase())
            .join("");
          by.set(state.file, [...(by.get(state.file) || []), initials]);
        }
      }
      return by;
    },

    participants() {
      const localUser = store.get("user");
      const localTab = localUser?.tab || "";
      const states = store.getAllStates();
      const result = [];
      for (const [key, state] of Object.entries(states)) {
        if (key === "user") {
          // Skip the local user's own presence
          if (state.tab === localTab) continue;
          result.push({
            key: state.tab || key,
            name: state.name || "Anonymous",
          });
        }
      }
      return result;
    },

    /// Called when the text being followed is a different text, so whoever is
    /// bound to it can bind again.
    onSwap(swap) {
      swaps.add(swap);
    },

    /// What to send to join, or to rejoin: what this browser already has, so
    /// the server answers with the rest and nothing more.
    open() {
      // A version vector is an object, not bytes. It has to be encoded before
      // it can be base64'd for the wire -- spreading it straight into the
      // encoder threw "not iterable", which is what made opening a document
      // fail before it had sent anything.
      return { type: "doc-open", vector: encode(doc.oplogVersion().encode()) };
    },

    /// The server's answer to `doc-open`: the document, or -- when it is too
    /// large for a text frame -- somewhere to fetch it from.
    async start(state) {
      if (state.ref) {
        // Same origin, signed, and short-lived. Whatever arrives during the
        // fetch is caught up by the state sent below, which is why the fetch
        // does not have to be atomic with anything.
        //
        // It carries what every other call to the API carries. The signature
        // on the URL says the link was minted here; it does not say who is
        // holding it, so the route asks again -- and asking means the
        // same-origin marker rule A turns on, and the link key a reader
        // arrived with. Without them a document too large to send inline was
        // refused, which is what the notebook examples were doing.
        if (!fetchReference) throw new Error("this client cannot fetch referenced document state");
        doc.import(await fetchReference(state.ref));
      } else if (state.update) {
        doc.import(decode(state.update));
      }
      joined = true;
      announcePresence();
      // Whatever this browser has that the server may not: its own unsent
      // work, and -- after a reference fetch -- anything that landed while it
      // was in flight.
      if (mayEdit) catchUp(state.vector);
      else report();
    },

    apply(update) {
      doc.import(decode(update));
    },

    applyPresence(update) {
      store.apply(decode(update));
    },

    /// The server has written everything up to this number. Relaying was never
    /// this; storage is.
    acknowledge(upTo) {
      for (const mine of [...unacknowledged.keys()]) {
        if (mine <= upTo) unacknowledged.delete(mine);
      }
      report();
    },

    /// The socket dropped. Nothing is thrown away -- what was not acknowledged
    /// is still held, and goes again on the next join.
    disconnected() {
      joined = false;
      presenceConnection++;
      // Keep a pending local state for the next join, but do not leave a
      // timer running while the socket is unavailable.
      clearPresenceTimer();
      report();
    },

    text_() {
      return bound ? bound.toString() : "";
    },

    /// Says who this is, for the label on their caret.
    rename(who) {
      const user = store.get("user") || {};
      store.set("user", { ...user, name: who });
    },

    /// Resolves after the persistence adapter has committed the latest local
    /// state. A client without durable storage resolves immediately.
    persist() {
      return persister?.flush?.() || Promise.resolve();
    },

    leave() {
      clearPresenceTimer();
      pendingPresence.clear();
      left = true;
      persister?.close?.();
      doc.destroy?.();
    },
  };
}
