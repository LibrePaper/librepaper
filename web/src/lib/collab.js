// The document, as this browser holds it.
//
// The source is a Yjs document -- a CRDT -- so two people typing in the same
// sentence converge on the same text without either of them waiting for the
// other. What travels is a small binary update per change.
//
// The server is not a relay. It holds the same document, with `yrs`, and it is
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
// makes is held until its `y-ack` arrives, so `pending` is the honest answer
// to "is my work safe", and the toolbar says that rather than saying "saved"
// because a socket happens to be open.
//
// **Nothing is lost to a disconnection.** y-indexeddb keeps the document in
// this browser, so a reload while the socket is down comes back with the text;
// and on reconnect the whole local state is sent, which the server merges, so
// the changes made on either side of the gap reach the other.

import * as Y from "yjs";
import { IndexeddbPersistence } from "y-indexeddb";
import { cacheName, restoreLegacyCache } from "./collab-cache.js";
import { Awareness, encodeAwarenessUpdate, applyAwarenessUpdate } from "y-protocols/awareness.js";
import { SHELL_HEADERS, keyHeaders } from "./api.js";
import { keyFor } from "./storage.js";
import { checkPlacement, folderPaths, inside, parentPath, relocation, topEntries } from "./file-manager.js";

// Keep each JSON WebSocket frame comfortably below the server's one-megabyte
// receive limit. Base64 expands the binary update by a third, and the JSON
// envelope adds a little more, so the chunk is deliberately smaller than the
// apparent limit. The server reassembles these chunks before applying them as
// one Yjs update.
const UPDATE_CHUNK_BYTES = 600_000;

// Updates are binary and the room's socket carries JSON, so they travel
// base64-encoded. A keystroke is a few dozen bytes either way.
const encode = (bytes) => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};
const decode = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

// Someone else's caret is drawn in a colour of their own. Picked from a fixed
// set rather than at random so two people in a session rarely look alike, and
// so a person keeps the same colour for as long as they are in it.
const COLOURS = ["#2f5bd0", "#c2410c", "#15803d", "#7c3aed", "#be123c", "#0e7490"];

// An id for a file: twelve hex characters, which is what `session.rs` mints
// and enough randomness that two people creating a file at the same instant
// do not collide.
function mintId() {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// Joins the document's session. `send` puts a message on the room's socket;
/// `onPeers` is told how many people are in it; `onState` is told whenever the
/// answer to "is this browser's work safe" changes.
///
/// `mayEdit` is false for a reader, who joins to receive the text and never to
/// change it.
export function join({ send, onPeers, onState, name, slug, createdAt = "", key = "", mayEdit = true }) {
  const doc = new Y.Doc();
  // A document is a directory: `files` holds one Y.Text per file under an id
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
  // The text every document was before it was a directory. It is read here and
  // never written: a session the server has not migrated yet arrives with the
  // maps empty and this full, and the editor has to show something. The server
  // migrates it the first time it loads it, and what this browser then sees is
  // the same words under a name.
  const legacy = doc.getText("source");
  const awareness = new Awareness(doc);

  // The text the editor and the preview are following, and who is following
  // it. Which text that is can change under them -- when the maps arrive from
  // the server, or when somebody names a different main file -- so the
  // watchers are held here and moved across rather than re-registered by every
  // caller.
  let bound = null;
  const watchers = new Set();
  const swaps = new Set();

  function mainText() {
    const id = meta.get("main");
    const text = id ? files.get(id) : null;
    return text instanceof Y.Text ? text : legacy;
  }

  function rebind() {
    const next = mainText();
    if (next === bound) return;
    for (const watcher of watchers) {
      bound?.unobserve(watcher);
      next.observe(watcher);
    }
    bound = next;
    for (const swap of swaps) swap();
  }

  files.observe(rebind);
  meta.observe(rebind);
  rebind();

  // Updates this browser has made and the server has not yet said are durable,
  // by the number they were sent under.
  const unacknowledged = new Map();
  let seq = 0;
  // Whether this browser has been given the server's copy since the socket
  // last came up. Until it has, what is here may be behind.
  let joined = false;
  // Whether the document is in this browser's own storage, which is what makes
  // a reload safe while the socket is down.
  let local = false;

  // Kept per document, so two tabs on the same document share it and a tab on
  // another document is unaffected. A reader keeps nothing: they have nothing
  // of their own to lose, and a copy of somebody else's document in their
  // browser is not theirs to hold.
  // A browser that will not give us storage -- a private window, site data
  // blocked -- is not a browser that cannot edit. It simply has nowhere to
  // keep the document, which is exactly what `local` is for saying.
  let store = null;
  let legacyRestore = null;
  try {
    if (slug && mayEdit && typeof indexedDB !== "undefined") {
      store = new IndexeddbPersistence(cacheName(slug, createdAt), doc);
      store.whenSynced.then(() => {
        local = true;
        report();
      });
    }
  } catch {
    store = null;
  }

  const report = () => onState?.({ pending: unacknowledged.size, local, joined });

  awareness.setLocalStateField("user", {
    name: name || "Anonymous",
    color: COLOURS[Math.floor(Math.random() * COLOURS.length)],
  });

  // Anything this browser changes is sent on and held until it is
  // acknowledged; anything that arrives is applied with an origin that stops
  // it being sent straight back out. An update out of indexeddb is this
  // browser's own past work, and is sent on for the same reason a keystroke
  // is: the server may never have seen it.
  function sendUpdate(update, mine) {
    if (update.byteLength <= UPDATE_CHUNK_BYTES) {
      send({ type: "y-update", update: encode(update), seq: mine });
      return;
    }
    const chunks = Math.ceil(update.byteLength / UPDATE_CHUNK_BYTES);
    send({ type: "y-update-start", seq: mine, size: update.byteLength, chunks });
    for (let index = 0; index < chunks; index++) {
      const from = index * UPDATE_CHUNK_BYTES;
      send({
        type: "y-update-chunk",
        seq: mine,
        index,
        update: encode(update.subarray(from, Math.min(update.byteLength, from + UPDATE_CHUNK_BYTES))),
      });
    }
    send({ type: "y-update-end", seq: mine });
  }

  doc.on("update", (update, origin) => {
    if (origin === "remote" || !mayEdit) return;
    const mine = ++seq;
    unacknowledged.set(mine, update);
    report();
    sendUpdate(update, mine);
  });

  awareness.on("update", ({ added, updated, removed }) => {
    if (!mayEdit) return;
    const changed = added.concat(updated, removed);
    send({ type: "y-awareness", update: encode(encodeAwarenessUpdate(awareness, changed)) });
    onPeers?.(awareness.getStates().size);
  });

  /// Everything this browser has, as one update. Sending it after a join is
  /// how the changes made while the socket was down reach the server: applying
  /// it is idempotent, so it costs nothing when there were none.
  function catchUp(vector) {
    // A server can include the state vector used for its y-state response. In
    // that case only this browser's missing structs are sent back; older
    // servers omit it, so the bounded full-state path remains compatible.
    const whole = vector ? Y.encodeStateAsUpdate(doc, decode(vector)) : Y.encodeStateAsUpdate(doc);
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
    awareness,
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
      bound?.observe(watcher);
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
      return path;
    },

    relocate(entries, destination, rules, rename = false) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const plan = relocation(this.list(), this.folders(), entries, destination, rules, rename);
      doc.transact(() => {
        // Remove asset keys first, so a batch move never overwrites a source.
        for (const file of plan.files) if (file.kind === "asset") assets.delete(file.previousPath);
        for (const file of plan.files) {
          if (file.kind === "asset") assets.set(file.path, file.sha);
          else paths.set(file.id, file.path);
        }
        for (const path of plan.oldFolders) meta.delete(`folder:${path}`);
        for (const path of [...plan.folders, ...plan.parents]) meta.set(`folder:${path}`, true);
      });
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
      doc.transact(() => {
        for (const file of removed) {
          if (file.kind === "asset") assets.delete(file.path);
          else { files.delete(file.id); paths.delete(file.id); }
        }
        for (const path of this.folders()) if (selected(path)) meta.delete(`folder:${path}`);
        for (const entry of roots) {
          const parent = parentPath(entry.path);
          if (parent) meta.set(`folder:${parent}`, true);
        }
      });
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
        .filter(([file]) => files.get(file) instanceof Y.Text)
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
        if (path && text instanceof Y.Text) {
          texts[path] = text.toString();
          entries[path] = { kind: "text", id: file };
        }
      }
      const digests = Object.create(null);
      for (const [path, sha] of assets.entries()) {
        digests[path] = sha;
        entries[path] = { kind: "asset", sha };
      }
      // Compile settings ride in `meta` beside `main`, so they travel with
      // the project rather than with this browser. Only written when an
      // editor's browser has actually pinned one: a tree with neither key
      // set omits `settings` entirely, which is what lets
      // `tree-digest.js`/`history.rs` keep every existing checkpoint's sha.
      const engine = meta.get("latex.engine") || "";
      const release = meta.get("latex.release") || "";
      const settings = engine || release ? { engine, release } : undefined;
      return { main: this.mainPath(), texts, digests, files: entries, ...(settings ? { settings } : {}) };
    },

    /// This project's LaTeX compile settings: the engine an editor picked
    /// (or "auto", the default), and the browser release pinned for it, or
    /// null before any editor's browser has compiled it. Shared with every
    /// collaborator through `meta`, the same map `main` lives in.
    latexSettings() {
      return {
        engine: meta.get("latex.engine") || "auto",
        release: meta.get("latex.release") || null,
      };
    },

    /// Changes the project's engine and/or pinned release. Only the keys
    /// that actually change are written, so an editor picking the same
    /// engine again does not touch `release`'s history. A reader never calls
    /// this -- `mayEdit` refuses it the way every other write here does.
    setLatexSettings(next) {
      if (!mayEdit) throw new Error("This project is read-only.");
      const current = this.latexSettings();
      if (next.engine !== undefined && next.engine !== current.engine) {
        meta.set("latex.engine", next.engine);
      }
      if (next.release !== undefined && next.release !== current.release) {
        if (next.release) meta.set("latex.release", next.release);
        else meta.delete("latex.release");
      }
    },

    /// The text at a path, for the caller that has a path and not an id --
    /// which is what a diagnostic carries.
    idOf(path) {
      for (const [file, at] of paths.entries()) if (at === path) return file;
      return "";
    },

    textOf(id) {
      const text = files.get(id);
      return text instanceof Y.Text ? text : null;
    },

    /// Makes a file. One transaction, so no peer ever sees a text without the
    /// name it is known by.
    addText(path, body = "") {
      const id = mintId();
      doc.transact(() => {
        files.set(id, new Y.Text(body));
        paths.set(id, path);
      });
      return id;
    },

    /// Renames a file, which moves a string and leaves the words where they
    /// are. This is why the texts are keyed by an id: somebody typing into
    /// this file at this moment keeps what they typed.
    renameFile(id, path, kind) {
      if (kind === "asset" || (kind === undefined && assets.has(id))) {
        if (!assets.has(id)) return;
        const sha = assets.get(id);
        doc.transact(() => {
          assets.delete(id);
          assets.set(path, sha);
        });
        return;
      }
      if (kind === "text" || (kind === undefined && paths.has(id))) paths.set(id, path);
    },

    /// Removes a file, its name with it. The main file is never removed here;
    /// the file list refuses it, because a document has to be something.
    removeFile(id) {
      doc.transact(() => {
        files.delete(id);
        paths.delete(id);
      });
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
      // A Y.Text is nested below the files map. A shallow observer sees a new
      // file but not edits to an existing one, leaving readers of another file
      // with a stale preview. Deep observation covers both cases.
      files.observeDeep(watcher);
      paths.observe(watcher);
      assets.observe(watcher);
      meta.observe(watcher);
      return () => {
        files.unobserveDeep(watcher);
        paths.unobserve(watcher);
        assets.unobserve(watcher);
        meta.unobserve(watcher);
      };
    },

    /// Says which file this browser's caret is in, so the file list can show
    /// who is where. The caret's own position is published by y-codemirror
    /// against the text it was made in, so it already paints in the right
    /// file; this is for the list.
    inFile(id) {
      awareness.setLocalStateField("file", id);
    },

    /// Who is in which file: an id to the initials of the people in it.
    whereEveryoneIs() {
      const by = new Map();
      for (const [client, state] of awareness.getStates()) {
        if (client === doc.clientID || !state?.file) continue;
        const name = state?.user?.name || "?";
        const initials = name
          .split(/\s+/)
          .filter(Boolean)
          .slice(0, 2)
          .map((word) => word[0].toUpperCase())
          .join("");
        by.set(state.file, [...(by.get(state.file) || []), initials]);
      }
      return by;
    },

    /// Called when the text being followed is a different text, so whoever is
    /// bound to it can bind again.
    onSwap(swap) {
      swaps.add(swap);
    },

    /// What to send to join, or to rejoin: what this browser already has, so
    /// the server answers with the rest and nothing more.
    open() {
      return { type: "y-open", vector: encode(Y.encodeStateVector(doc)) };
    },

    /// The server's answer to `y-open`: the document, or -- when it is too
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
        const response = await fetch(state.ref, {
          credentials: "same-origin",
          headers: { ...SHELL_HEADERS, ...keyHeaders(key || keyFor(slug)) },
        });
        if (!response.ok) throw new Error("could not fetch the document");
        Y.applyUpdate(doc, new Uint8Array(await response.arrayBuffer()), "remote");
      } else if (state.update) {
        Y.applyUpdate(doc, decode(state.update), "remote");
      }
      if (store && createdAt) {
        legacyRestore ??= restoreLegacyCache(slug, doc);
        await legacyRestore;
      }
      joined = true;
      // Whatever this browser has that the server may not: its own unsent
      // work, and -- after a reference fetch -- anything that landed while it
      // was in flight.
      if (mayEdit) catchUp(state.vector);
      else report();
    },

    apply(update) {
      Y.applyUpdate(doc, decode(update), "remote");
    },

    applyAwareness(update) {
      applyAwarenessUpdate(awareness, decode(update), "remote");
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
      report();
    },

    text_() {
      return bound ? bound.toString() : "";
    },

    /// Says who this is, for the label on their caret.
    rename(who) {
      awareness.setLocalStateField("user", { ...awareness.getLocalState()?.user, name: who });
    },

    leave() {
      awareness.destroy();
      store?.destroy();
      doc.destroy();
    },
  };
}
