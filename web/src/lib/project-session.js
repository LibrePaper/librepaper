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
// **Relaying is not durability.** An update is relayed the moment it lands.
// Nonempty batches are acknowledged after writing. `doc-ack` is transport
// bookkeeping; `pending` is based on the server's durable version vector, so
// the toolbar does not say "saved" merely because a socket happens to be open.
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

async function sha256Hex(bytes) {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) throw new Error("this browser cannot verify referenced document state");
  const digest = new Uint8Array(await subtle.digest("SHA-256", bytes));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

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

// A person's presence lives under this prefix and their tab's id: the store
// is shared and last-write-wins per key, so peers need a key each.
export const PRESENCE_PREFIX = "user:";

/// Whether `key` holds somebody's presence rather than, say, a caret.
const isPresence = (key) => String(key).startsWith(PRESENCE_PREFIX);

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
  presenceColour = () => "",
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
  // One presence key per person, not one key everybody writes to. The store is
  // last-write-wins per key, so while every peer announced itself under
  // `user`, the last to speak replaced everyone before it: a file with three
  // people in it listed none of them, and a browser could find its own
  // presence -- name, colour and all -- overwritten by somebody else's.
  const localTab = presenceId();
  const localKey = `${PRESENCE_PREFIX}${localTab}`;

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

  // Transport sends this browser has made and the server has not yet retired
  // with a doc-ack, by the number they were sent under.
  const unacknowledged = new Map();
  // The newest local state this browser needs the server to durably cover.
  // Remote imports never move this target. Keep an owned copy of each vector:
  // Loro's WASM wrappers are mutable resources, and a later import must not
  // change a saved target under our feet.
  let saveTarget = null;
  let durableCoverage = null;
  let hydrationCaptured = !mayEdit;
  const remoteDuringHydration = [];
  let seq = 0;
  // Whether this browser has been given the server's copy since the socket
  // last came up. Until it has, what is here may be behind.
  let joined = false;
  let presenceTimer = null;
  const pendingPresence = new Set();
  let presenceConnection = 0;
  let announcedConnection = -1;
  let left = false;

  // A join can arrive as `doc-state` alone, `doc-rows` alone, or `doc-state`
  // naming a base by reference followed by `doc-rows` (§6.2 steps 3-5). Each
  // is its own WebSocket frame and its own turn on the event loop, so nothing
  // about receiving them stops `doc-rows` from running while `start`'s
  // hydration wait or referenced-base fetch is still in flight -- `start` is
  // asynchronous and its caller does not block on it before the next message
  // is dispatched. `sequenceJoin` gives every step that imports into `doc` on
  // the way in a strict order: whichever one is in flight is awaited before
  // the next one imports anything or exports a catch-up, so the exported
  // vector never describes a document this browser has not finished
  // assembling, and a `doc-rows` frame that turns out to be the first frame
  // of a join still gets hydration awaited, the same as `start` gets it.
  let joinGate = Promise.resolve();
  function sequenceJoin(step) {
    const ran = joinGate.then(step, step);
    // The gate itself must never reject, or every join step queued behind a
    // failed one would be skipped; the failure still reaches this step's own
    // caller through `ran`.
    joinGate = ran.then(() => {}, () => {});
    return ran;
  }

  // Whether the document is in this client's own storage, which is what makes
  // a reload safe while the socket is down.
  let local = !mayEdit;
  let localPending = 0;
  let localError = "";

  function copyVector(vector) {
    return vector ? VersionVector.decode(vector.encode()) : new VersionVector(undefined);
  }

  // Version vectors from the server normally compare as prefixes. The
  // element-wise maximum also handles an old join frame racing a newer
  // doc-durable frame (and the undefined comparison for concurrent vectors)
  // without ever moving the known durable coverage backwards.
  function mergeVectors(first, second) {
    if (!first) return copyVector(second);
    if (!second) return copyVector(first);
    const merged = new Map(first.toJSON());
    for (const [peer, counter] of second.toJSON()) {
      if (Number(counter) > Number(merged.get(peer) || 0)) merged.set(peer, counter);
    }
    return VersionVector.parseJSON(merged);
  }

  function rememberLocalTarget(vector = doc.oplogVersion()) {
    if (!mayEdit) return;
    saveTarget = mergeVectors(saveTarget, vector);
  }

  function captureHydratedTarget() {
    if (hydrationCaptured || !mayEdit) return;
    // This is the last point before server join frames are imported. Include
    // any local edits made while hydration was in flight without replacing an
    // already captured target from an edit made during that wait.
    rememberLocalTarget();
    hydrationCaptured = true;
  }

  function flushRemoteDuringHydration() {
    // A live update can arrive while the persistence adapter is still
    // restoring its local copy. Apply those only after the conservative local
    // target has been captured, so a peer's update cannot become this target.
    for (const update of remoteDuringHydration.splice(0)) doc.import(decode(update));
  }

  function coveredByDurable() {
    if (!saveTarget || saveTarget.length() === 0) return true;
    if (!durableCoverage) return false;
    const relation = durableCoverage.compare(saveTarget);
    return relation === 0 || relation === 1;
  }

  function mergeDurableCoverage(encoded) {
    if (!encoded) return;
    const next = VersionVector.decode(decode(encoded));
    durableCoverage = mergeVectors(durableCoverage, next);
  }

  // A read-only client keeps nothing: it has no local changes to lose. An
  // editor can still work when storage is unavailable, but `local` remains
  // false so its UI does not call those changes safe on this device.
  let persister = null;
  const report = () => onState?.({
    // Keep this numeric for the existing toolbar truthiness checks. It is a
    // save signal, not a count of transport batches.
    pending: Number(mayEdit && !coveredByDurable()),
    local,
    localPending,
    localError,
    joined,
  });
  try {
    persister = persistence?.open?.(doc, {
      hydrated() {
        local = true;
        captureHydratedTarget();
        report();
      },
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
      // §6.3: nothing is persisted per batch and there is no replay cursor,
      // so there is no coverage confirmation to record here any more.
    }) || null;
  } catch (error) {
    localError = error?.message || String(error || "local persistence failed");
  }
  // Sessions without an asynchronous persistence adapter are hydrated at
  // construction. Mark that baseline before a socket can import remote data.
  if (!persister?.hydration) captureHydratedTarget();
  report();

  // `color` is the wire spelling every peer already reads. It is the person's
  // colour rather than this session's: it comes from the name, and `rename`
  // asks for it again if the name arrives after the join.
  store.set(localKey, {
    name: name || "Anonymous",
    // Their name, or -- for somebody who has not said who they are -- their
    // tab, which is theirs alone for as long as it is open. One shared
    // "Anonymous" would paint every reader in a file the same colour.
    color: presenceColour(name || localTab, takenColours()),
    tab: localTab,
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
      // Capture the full vector, including dependencies visible at this edit.
      // Later remote imports do not call this subscription and therefore do
      // not move the save target.
      rememberLocalTarget();
      report();
      sendUpdate(update, mine);
    });
  }

  /// The colours the other people in this file are already drawn in.
  function takenColours() {
    const taken = [];
    for (const [key, state] of Object.entries(store.getAllStates())) {
      if (!isPresence(key) || key === localKey || !state?.color) continue;
      taken.push(state.color);
    }
    return taken;
  }

  /// Two people can still choose the same colour: they choose when they join,
  /// and neither can see somebody who has not arrived yet. So the choice is
  /// made again whenever presence arrives, and the one whose key sorts higher
  /// gives way. Both sides run the same comparison, so exactly one of them
  /// moves and it moves once -- the other stays where everybody already sees
  /// it, and somebody who was alone in the file keeps the colour they had.
  function settleColour() {
    const mine = store.get(localKey);
    if (!mine?.color) return;
    const taken = [];
    let giveWay = false;
    for (const [key, state] of Object.entries(store.getAllStates())) {
      if (!isPresence(key) || key === localKey || !state?.color) continue;
      taken.push(state.color);
      if (state.color === mine.color && key < localKey) giveWay = true;
    }
    if (!giveWay) return;
    const next = presenceColour(mine.name || localTab, taken);
    if (next && next !== mine.color) store.set(localKey, { ...mine, color: next });
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
    // EphemeralStore encodings are complete messages, not byte streams that
    // may be concatenated. Send one latest message per changed key.
    for (const key of keys) {
      const keyBytes = store.encode(key);
      if (keyBytes) sendPresence(keyBytes);
    }
  }

  if (mayEdit) {
    store.subscribe((event) => {
      // Presence changes from peers (and expiry of stale peers) are for local
      // rendering only. Relaying them would create a broadcast echo loop.
      // People, not keys: a caret and a file are keys of their own, and
      // counting those said four people were here when two were.
      onPeers?.(Object.keys(store.getAllStates()).filter(isPresence).length);
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
    if (store.get(localKey) && announcedConnection !== presenceConnection) {
      const bytes = store.encode(localKey) || new Uint8Array();
      sendPresence(bytes);
    }
  }

  /// Everything this browser has beyond `vector`, as one update. Sending it
  /// after a join (§6.2 step 6) is how the changes made while the socket was
  /// down reach the server: applying it is idempotent, so it costs nothing
  /// when there were none. The same export is what a `doc-gap` reply asks
  /// for (§5 step 4): the server refused a batch it could not place and
  /// dropped it rather than queueing it, so this is what makes that work
  /// reach the log.
  function catchUp(vector) {
    // What arrives on the wire is bytes, and what `from` wants is a version
    // vector, so it has to be decoded first. Handing `from` the bytes is not
    // an error, it just reads as an empty vector and resends the whole
    // history, which is correct but is the thing this decode exists to
    // avoid.
    //
    // Absent or empty bytes ARE the empty vector, and mean "send
    // everything": the server reads them the same way
    // (`sequencer::decode_vector`), and the two sides have to agree or a
    // document nobody has typed into yet cannot be joined at all.
    const from = vector ? VersionVector.decode(decode(vector)) : new VersionVector(undefined);
    const whole = doc.export({ mode: "update", from });
    const mine = ++seq;
    // Replace transmission bookkeeping with this export. The save target
    // survives independently, including work the head already covers.
    unacknowledged.clear();
    unacknowledged.set(mine, whole);
    report();
    sendUpdate(whole, mine);
  }

  /// What both `doc-state` and `doc-rows` do once the import they carried is
  /// applied (§6.2 step 6): the browser is caught up with the server, so it
  /// announces itself and, if it may edit, sends back whatever it has that
  /// the server's vector does not cover yet.
  function finishJoin(vector, durableVector) {
    joined = true;
    // `vector` is the head used for catch-up. Durable coverage is a separate
    // signal and may arrive out of order with this join's asynchronous work.
    mergeDurableCoverage(durableVector);
    announcePresence();
    if (mayEdit) catchUp(vector);
    else report();
  }

  return {
    doc,
    files,
    paths,
    meta,
    ephemeral: store,

    /// This browser's own presence: the name on its caret and the colour it
    /// is drawn in. The store is shared, so a caller cannot reach for a
    /// well-known key -- every person in the file has one of their own.
    localPresence() {
      return store.get(localKey) || null;
    },
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
      const removedMain = removed.some((file) => file.main);
      for (const file of removed) {
        if (file.kind === "asset") assets.delete(file.path);
        else { files.delete(file.id); paths.delete(file.id); }
      }
      for (const path of this.folders()) if (selected(path)) meta.delete(`folder:${path}`);
      for (const entry of roots) {
        const parent = parentPath(entry.path);
        if (parent) meta.set(`folder:${parent}`, true);
      }
      if (removedMain) {
        const remaining = [...paths.entries()]
          .filter(([id]) => files.get(id)?.kind?.() === "Text")
          .sort((a, b) => String(a[1]).localeCompare(String(b[1])) || String(a[0]).localeCompare(String(b[0])));
        if (remaining.length) meta.set("main", remaining[0][0]);
        else meta.delete("main");
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
      doc.commit({ origin: DIRECTORY_ORIGIN });
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
      if (!mayEdit) throw new Error("This project is read-only.");
      const id = mintId();
      const text = new LoroText();
      if (body) {
        text.insert(0, body);
      }
      files.setContainer(id, text);
      paths.set(id, path);
      if (!meta.get("main")) meta.set("main", id);
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

    /// Removes a file, its name with it. Empty projects are valid; when the
    /// selected main is removed, choose a remaining text deterministically or
    /// leave main unset rather than fabricating content.
    removeFile(id) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const wasMain = meta.get("main") === id;
      files.delete(id);
      paths.delete(id);
      if (wasMain) {
        const remaining = [...paths.entries()]
          .filter(([file]) => files.get(file)?.kind?.() === "Text")
          .sort((a, b) => String(a[1]).localeCompare(String(b[1])) || String(a[0]).localeCompare(String(b[0])));
        if (remaining.length) meta.set("main", remaining[0][0]);
        else meta.delete("main");
      }
      doc.commit({ origin: DIRECTORY_ORIGIN });
    },

    removeAsset(path) {
      assets.delete(path);
      doc.commit({ origin: DIRECTORY_ORIGIN });
    },

    putAsset(path, sha) {
      assets.set(path, sha);
      doc.commit({ origin: DIRECTORY_ORIGIN });
    },

    /// Names the main file: the one a renderer is run on, and the one the
    /// document's format is read from.
    setMain(id) {
      meta.set("main", id);
      doc.commit({ origin: DIRECTORY_ORIGIN });
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
      const user = store.get(localKey) || {};
      store.set(localKey, { ...user, file: id });
    },

    /// Who is in which file: an id to the initials of the people in it.
    whereEveryoneIs() {
      const by = new Map();
      const states = store.getAllStates();
      for (const [key, state] of Object.entries(states)) {
        // Everybody but this browser, and only those who have said which file
        // they are in.
        if (!isPresence(key) || key === localKey) continue;
        if (state?.file) {
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
      const states = store.getAllStates();
      const result = [];
      for (const [key, state] of Object.entries(states)) {
        if (!isPresence(key) || key === localKey) continue;
        result.push({
          key: state?.tab || key,
          name: state?.name || "Anonymous",
          // The colour they are drawing their own caret in, so the badge in
          // the presence strip and the caret in the text are the same person
          // in the same colour.
          colour: state?.color || "",
        });
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
      return {
        type: "doc-open",
        vector: encode(doc.oplogVersion().encode()),
        // The protocol string carries the version now; there is no separate
        // `version`/`schema_version` field to keep in step with it. A server
        // that cannot speak this protocol answers with upgrade-required
        // before this socket can send an update (§6.1).
        protocol: "librepaper.room.v3",
      };
    },

    /// The server's answer to `doc-open`: either everything (§6.2 step 3),
    /// or the base -- inline or by reference -- for a join `doc-rows` (below)
    /// will finish (§6.2 steps 4-5). Sequenced through `joinGate` so a
    /// `doc-rows` frame that lands mid-fetch waits for this to finish
    /// importing before it touches `doc` itself.
    start(state) {
      return sequenceJoin(async () => {
        // Disk recovery must win the race with the room handshake. Otherwise a
        // delayed import lands after catch-up and is never offered to the
        // server (hydration is intentionally not a local Loro update).
        await persister?.hydration;
        if (left) return;
        captureHydratedTarget();
        if (state.protocol !== "librepaper.room.v3") {
          throw new Error("This document requires a newer LibrePaper version.");
        }
        if (state.ref) {
          // Same origin, immutable, digest-bound, and short-lived. Whatever arrives during the
          // fetch is caught up by the state sent below, which is why the fetch
          // does not have to be atomic with anything.
          //
          // It carries what every other call to the API carries. The opaque
          // reference identifies bytes but does not authorize whoever holds
          // it, so the route asks again -- and asking means the
          // same-origin marker rule A turns on, and the link key a reader
          // arrived with. Without them a document too large to send inline was
          // refused, which is what the notebook examples were doing.
          if (!fetchReference) throw new Error("this client cannot fetch referenced document state");
          let referenced;
          try {
            referenced = await fetchReference(state.ref);
          } catch (error) {
            if (error?.restartBaseline && !left) {
              send(open());
              return;
            }
            throw error;
          }
          if (!state.digest || await sha256Hex(referenced) !== state.digest) {
            throw new Error("referenced document state failed its integrity check");
          }
          doc.import(referenced);
        } else if (state.base) {
          doc.import(decode(state.base));
        }
        // Two Loro updates concatenated are not one Loro update, so each row
        // is imported as its own entry in the batch rather than joined first.
        if (state.updates?.length) doc.importBatch(state.updates.map(decode));
        flushRemoteDuringHydration();
        finishJoin(state.vector, state.durableVector);
      });
    },

    /// Rows the join's `doc-open` vector did not cover, sent after (or
    /// instead of, when the base alone was enough) `doc-state` (§6.2 steps
    /// 4-5). Importing is the same batch operation as above; catching up is
    /// the same operation as `start`, hence `finishJoin`. `doc-rows` can also
    /// arrive as the WHOLE join reply (step 4), so this awaits hydration
    /// itself rather than assuming `start` already did -- and `sequenceJoin`
    /// still lines it up behind a `start` that is mid-fetch for a referenced
    /// base, so the rows this frame carries are never caught up ahead of the
    /// base they build on.
    rows(frame) {
      return sequenceJoin(async () => {
        await persister?.hydration;
        if (left) return;
        captureHydratedTarget();
        if (frame.protocol !== "librepaper.room.v3") {
          throw new Error("This document requires a newer LibrePaper version.");
        }
        if (frame.updates?.length) doc.importBatch(frame.updates.map(decode));
        flushRemoteDuringHydration();
        finishJoin(frame.vector, frame.durableVector);
      });
    },

    /// The server refused a batch because its start vector was not covered
    /// (§5 step 4): the batch was dropped, not queued, so this export from
    /// the vector the server actually holds is what makes that work reach
    /// the log. A reader never sends updates, so a `doc-gap` naming one
    /// should not reach it, but nothing here assumes that holds.
    gap(vector) {
      if (left || !mayEdit) return;
      catchUp(vector);
    },

    apply(update) {
      if (!hydrationCaptured) {
        remoteDuringHydration.push(update);
        return;
      }
      doc.import(decode(update));
    },

    applyPresence(update) {
      store.apply(decode(update));
      // Somebody new may have arrived wearing this browser's colour.
      settleColour();
    },

    /// Retires transport bookkeeping through this client sequence number.
    /// This does not change save pending: durability is reported separately
    /// by `doc-durable`.
    acknowledge(upTo) {
      for (const mine of [...unacknowledged.keys()]) {
        if (mine <= upTo) unacknowledged.delete(mine);
      }
      report();
    },

    /// The server's durable log coverage. This is intentionally separate from
    /// `acknowledge`: an empty catch-up can be acknowledged while earlier work
    /// is still buffered in the server.
    durable(vector) {
      if (left || !vector) return;
      mergeDurableCoverage(vector);
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
      const user = store.get(localKey) || {};
      // The colour follows the name: a reader who signs in part way through
      // stops being their tab's colour and becomes their own, which is the
      // one everybody else already knows them by.
      store.set(localKey, { ...user, name: who, color: presenceColour(who || localTab, takenColours()) });
      settleColour();
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
