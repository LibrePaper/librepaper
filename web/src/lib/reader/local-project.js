// A folder on this computer, opened as a project (SPEC-local-folders.md).
//
// The Loro document is an editing buffer built from the folder's files when
// the project opens and thrown away when it closes. Nothing here has a socket:
// the object `createLocalProject` returns has the shape of the Reader's
// collaboration object (`start`, `close`, `send`, `syncStatus`, `sendLive`),
// so the Reader drives it the way it drives a room. Edits reach the folder
// through the folder sync, which tells this module what it wrote with `wrote`.
//
// This module owns what it learns from the folder: the text last read from or
// written to disk for each path, the figure bytes, and the conflicts waiting
// for an answer. Its callbacks only go outward.
//
// An outside change to a file is handled as follows.
// - The editor's text equals the text last read from or written to disk: the
//   new text replaces it through `textPatches`, so the cursor and the rest of
//   the document stay put.
// - The editor holds unsaved edits to that file: `onConflict` is told, and the
//   person chooses `keepMine` or `loadDisk`.
// - Files that were added join the project, files that were removed leave it,
//   and an asset is replaced by its new bytes.
import { checkPath } from "../paths.js";
import { toLf } from "../line-endings.js";
import { textPatches } from "../gitsync/patches.js";
import { formatOf } from "../formats.js";
import { openingFile } from "../opening.js";
import { toBase64, typeOf } from "../figures.js";
import * as folderRead from "../desktop/folder-read.js";

/** @typedef {{ path: string, kind: "text", text: string, sha: string | null, ending: string, raw?: Uint8Array }} TextItem */
/** @typedef {{ path: string, kind: "asset", sha: string }} AssetItem */
/** @typedef {TextItem | AssetItem} Item */
/** @typedef {{ kind: "text", text: string, sha: string | null, ending?: string, raw?: Uint8Array } | { kind: "asset", sha: string }} DiskRecord */

/// The origin of a commit that brings the disk's text into the editor. The
/// prefix is the directory's, which the editor's undo manager leaves alone:
/// undoing a reload would write the old text back over the file.
const DISK_ORIGIN = "directory:disk";
// Files read at once while the project opens.
const READ_BATCH = 8;
// How long the change loop waits after a failed request before asking again.
const RETRY_MS = 3000;

/// The `?local=` value of a reader address, or an empty string when the
/// address is not a local project's.
/** @param {string} search */
export function localIdOf(search) {
  const id = new URLSearchParams(search).get("local") || "";
  return /^[\w-]{1,64}$/.test(id) ? id : "";
}

/// The project id of a local project, which is what renders, build settings
/// and the preview are scoped by.
/** @param {string} id */
export const localSlug = (id) => `local-${id}`;

async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// A text file's body with LF endings, or null when the bytes are not UTF-8,
/// in which case the file is not text this editor could write back.
/** @param {Uint8Array} bytes */
function decodeText(bytes) {
  try {
    return toLf(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    return null;
  }
}

const encoder = new TextEncoder();

/// The line-ending style of a file's bytes: CRLF when they hold CRLF and no
/// lone LF, otherwise LF.
/** @param {Uint8Array} bytes */
function endingOf(bytes) {
  const raw = new TextDecoder().decode(bytes);
  return raw.includes("\r\n") && !raw.replace(/\r\n/g, "").includes("\n") ? "crlf" : "lf";
}

/** @param {string} text @param {string | undefined} ending */
const styled = (text, ending) => encoder.encode(ending === "crlf" ? text.replace(/\n/g, "\r\n") : text);

function realWait(ms, signal) {
  return new Promise((resolve) => {
    if (signal?.aborted) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
  });
}

/// The figures of a local project. The bytes are held here, under their
/// digest, in place of the server's asset store: `upload` and `gather` have the
/// shape of the Reader's `uploadAsset` and `figures.gather`.
export function createFigureStore() {
  /** @type {Map<string, Uint8Array>} */
  const bytes = new Map();
  /** @type {Map<string, string>} */
  const urls = new Map();

  /** @param {Uint8Array} data */
  async function put(data) {
    const sha = await sha256Hex(data);
    bytes.set(sha, data);
    return sha;
  }

  /** @param {string} _slug @param {File} file */
  async function upload(_slug, file) {
    return { sha: await put(new Uint8Array(await file.arrayBuffer())) };
  }

  /** @param {string} _slug @param {Record<string, string>} digests */
  async function gather(_slug, digests) {
    /** @type {Record<string, Uint8Array>} */
    const assets = {};
    /** @type {Record<string, string>} */
    const where = {};
    /** @type {string[]} */
    const missing = [];
    for (const [path, sha] of Object.entries(digests || {})) {
      const body = bytes.get(sha);
      if (!body) {
        missing.push(path);
        continue;
      }
      assets[path] = body;
      const type = typeOf(path);
      const key = `${type}|${sha}`;
      if (!urls.has(key)) {
        const base = type === "application/pdf"
          ? URL.createObjectURL(new Blob([/** @type {Uint8Array<ArrayBuffer>} */ (body)], { type }))
          : `data:${type};base64,${toBase64(body)}`;
        urls.set(key, base);
      }
      where[path] = `${urls.get(key)}#librepaper-asset=${encodeURIComponent(sha)}`;
    }
    return { assets, urls: where, missing };
  }

  function release() {
    for (const url of urls.values()) if (url.startsWith("blob:")) URL.revokeObjectURL(url);
    urls.clear();
    bytes.clear();
  }

  return { put, upload, gather, release };
}

/** @param {{
 * id: string,
 * rules: { text_extensions?: string[], asset_extensions?: string[], [key: string]: unknown },
 * collab: { join: (options: any) => any },
 * figures?: ReturnType<typeof createFigureStore>,
 * folder?: Pick<typeof folderRead, "projectTree" | "projectFile" | "projectChanges" | "projectName">,
 * onPeers?: (count: number) => void,
 * onState?: (state: any) => void,
 * onSession?: (session: any) => void,
 * onFiles?: (events: any, session: any) => void,
 * onAwareness?: (session: any) => void,
 * onConflict?: (conflict: { path: string, keepMine: () => void, loadDisk: () => void }) => void,
 * onResend?: (paths: string[]) => void,
 * onProblem?: (message: string) => void,
 * wait?: (ms: number, signal?: AbortSignal) => Promise<void>,
 * }} options */
export function createLocalProject({
  id,
  rules,
  collab,
  figures = createFigureStore(),
  folder = folderRead,
  onPeers = () => {},
  onState = () => {},
  onSession = () => {},
  onFiles = () => {},
  onAwareness = () => {},
  onConflict = () => {},
  onResend = () => {},
  onProblem = () => {},
  wait = realWait,
}) {
  const slug = localSlug(id);
  const abort = new AbortController();
  let closed = false;
  let session = null;
  let filesCleanup = null;
  let presenceCleanup = null;
  /** The files read by `load`, added to the session when it starts.
   * @type {Item[]} */
  let initial = [];
  /** What the folder holds for each path as far as this module knows: the text
   * last read from or written to disk, and the digest of the bytes when it is
   * known. @type {Map<string, DiskRecord>} */
  const disk = new Map();
  /** The disk version of a file the person has not yet answered for.
   * @type {Map<string, { text: string, sha: string | null, item: TextItem }>} */
  const conflicts = new Map();
  let failing = false;
  /** Where the desktop app's changes resume: the cursor of the latest tree
   * read, so nothing between that read and the next request is skipped. */
  let cursor;
  /** @type {Record<string, string>} */
  let seeded = {};

  const complain = (message) => {
    if (closed) return;
    onProblem(message);
  };

  /// The files of a listing that belong to the project. Anything the
  /// deployment's rules do not name is left out, and never touched.
  /** @param {Array<{ path: string }>} listed */
  function managed(listed) {
    const kept = new Map();
    for (const file of listed || []) {
      const checked = checkPath(rules, file.path);
      if (!checked.error) kept.set(file.path, { ...file, kind: /** @type {"text" | "asset"} */ (checked.kind) });
    }
    return kept;
  }

  /// Reads each file of a listing, a few at a time.
  async function readAll(files) {
    const read = [];
    for (let at = 0; at < files.length; at += READ_BATCH) {
      const batch = files.slice(at, at + READ_BATCH);
      read.push(...await Promise.all(batch.map(async (file) => ({ file, bytes: await folder.projectFile(id, file.path) }))));
    }
    return read;
  }

  /// A file's bytes as the project holds them: a text with LF endings, or an
  /// asset's digest. Null for a text that is not UTF-8.
  /** @param {{ file: { path: string, kind: "text" | "asset", sha256?: string | null }, bytes: Uint8Array }} read
   * @returns {Promise<Item | null>} */
  async function interpret({ file, bytes }) {
    if (file.kind === "text") {
      const text = decodeText(bytes);
      if (text === null) return null;
      const ending = endingOf(bytes);
      // Bytes that the style cannot rebuild (a byte order mark, mixed endings)
      // are kept, so a file that is not edited is written back as it was.
      const exact = styled(text, ending);
      const same = exact.length === bytes.length && exact.every((byte, at) => byte === bytes[at]);
      return { path: file.path, kind: "text", text, sha: file.sha256 ?? null, ending, raw: same ? undefined : bytes };
    }
    return { path: file.path, kind: "asset", sha: await figures.put(bytes) };
  }

  /// Opens the folder: reads the tree and every file the rules name. Resolves
  /// with the folder's name and the format of the file the project opens on.
  async function load() {
    const [tree, name] = await Promise.all([
      folder.projectTree(id),
      folder.projectName(id).catch(() => ""),
    ]);
    cursor = tree.cursor;
    const read = await readAll([...managed(tree.files).values()]);
    initial = [];
    for (const one of read) {
      const item = await interpret(one);
      if (item) initial.push(item);
    }
    seeded = Object.fromEntries(initial.filter((item) => item.sha).map((item) => [item.path, item.sha]));
    const texts = Object.fromEntries(initial.filter((item) => item.kind === "text").map((item) => [item.path, item.text]));
    return { name: name || "Local project", sourceFormat: formatOf(openingFile(texts)) };
  }

  const entryOf = (path, kind) => session.list().find((file) => file.path === path && file.kind === kind);

  /// Replaces a text's words with the disk's, by the smallest edits.
  function replaceText(path, entry, next) {
    const text = session.textOf(entry.id);
    if (!text) return;
    const patches = textPatches(path, text.toString(), next).sort((a, b) => b.start - a.start);
    for (const patch of patches) {
      if (patch.end > patch.start) text.deleteUtf8(patch.start, patch.end - patch.start);
      if (patch.replacement) text.insertUtf8(patch.start, patch.replacement);
    }
    session.doc.commit({ origin: DISK_ORIGIN });
  }

  /** @param {TextItem} item @returns {DiskRecord} */
  const textRecord = (item) => ({ kind: "text", text: item.text, sha: item.sha, ending: item.ending, raw: item.raw });

  /** @param {TextItem} item */
  function askAboutText(item) {
    const { path, text, sha } = item;
    const waiting = conflicts.get(path);
    if (waiting) {
      // The disk moved on again while the question stood. The answer applies
      // to the newest text.
      waiting.text = text;
      waiting.sha = sha;
      waiting.item = item;
      return;
    }
    const pending = { text, sha, item };
    conflicts.set(path, pending);
    const current = () => !closed && conflicts.get(path) === pending;
    onConflict({
      path,
      keepMine() {
        if (!current()) return;
        conflicts.delete(path);
        onResend([path]);
      },
      loadDisk() {
        if (!current()) return;
        conflicts.delete(path);
        const entry = entryOf(path, "text");
        if (entry) replaceText(path, entry, pending.text);
        else session.addText(path, pending.text);
        disk.set(path, textRecord(pending.item));
      },
    });
  }

  /// A text that is new on disk or changed there.
  /** @param {TextItem} item */
  function ingestText(item) {
    const { path, text } = item;
    const entry = entryOf(path, "text");
    if (!entry) {
      session.addText(path, text);
      disk.set(path, textRecord(item));
      return;
    }
    const mine = session.textOf(entry.id)?.toString();
    if (mine === text) {
      conflicts.delete(path);
      disk.set(path, textRecord(item));
      return;
    }
    const known = disk.get(path);
    if (known?.kind === "text" && known.text === mine) {
      conflicts.delete(path);
      replaceText(path, entry, text);
      disk.set(path, textRecord(item));
      return;
    }
    askAboutText(item);
  }

  function ingestAsset(path, sha) {
    if (entryOf(path, "asset")?.sha !== sha) session.putAsset(path, sha);
    disk.set(path, { kind: "asset", sha });
  }

  /** @param {Item} item */
  function ingest(item) {
    if (item.kind === "text") ingestText(item);
    else ingestAsset(item.path, item.sha);
  }

  function drop(path) {
    conflicts.delete(path);
    disk.delete(path);
    const text = entryOf(path, "text");
    if (text) session.removeFile(text.id);
    if (entryOf(path, "asset")) session.removeAsset(path);
  }

  /// One change from the desktop app. A file that cannot be read now has
  /// changed again or gone; the next report says which.
  async function applyChange(change) {
    const checked = checkPath(rules, change.path);
    if (checked.error) return;
    if (change.kind === "removed") {
      drop(change.path);
      return;
    }
    let bytes;
    try {
      bytes = await folder.projectFile(id, change.path);
    } catch (error) {
      if (error?.name === "Canceled") throw error;
      return;
    }
    if (closed) return;
    const item = await interpret({ file: { path: change.path, kind: /** @type {"text" | "asset"} */ (checked.kind), sha256: change.sha256 ?? null }, bytes });
    if (item && !closed) ingest(item);
  }

  /// The desktop app lost the thread of its changes, so the tree is read
  /// again and compared with what this module knows.
  async function reread() {
    const tree = await folder.projectTree(id);
    if (closed) return;
    cursor = tree.cursor;
    const listed = managed(tree.files);
    for (const path of [...disk.keys()]) if (!listed.has(path)) drop(path);
    const changed = [...listed.values()].filter((file) => {
      const known = disk.get(file.path);
      return !known || known.sha === null || known.sha !== file.sha256;
    });
    for (const one of await readAll(changed)) {
      if (closed) return;
      const item = await interpret(one);
      if (item && !closed) ingest(item);
    }
  }

  async function follow() {
    while (!closed) {
      try {
        const answer = await folder.projectChanges(id, cursor, { signal: abort.signal });
        if (closed) return;
        failing = false;
        if (answer.reset) {
          // The thread is lost; the tree read gives the cursor to resume from.
          await reread();
          continue;
        }
        cursor = answer.cursor;
        for (const change of answer.changes || []) {
          if (closed) return;
          await applyChange(change);
        }
      } catch (error) {
        if (closed || error?.name === "Canceled") return;
        if (!failing) complain(`The folder could not be read: ${error?.message || "unknown error"}`);
        failing = true;
        await wait(RETRY_MS, abort.signal);
      }
    }
  }

  /// Builds the session from what `load` read and starts following the folder.
  /** @param {unknown} _document */
  function start(_document) {
    if (closed) return null;
    if (session) return session;
    const active = collab.join({
      send: () => {},
      onPeers: (count) => { if (!closed) onPeers(count); },
      onState: (state) => { if (!closed) onState(state); },
      name: "",
      slug,
      mayEdit: true,
      standalone: true,
    });
    session = active;
    active.setRules(rules);
    // The session is empty, so each file is simply added. Looking each path up
    // first would read the whole directory once per file.
    for (const item of initial) {
      if (item.kind === "text") active.addText(item.path, item.text);
      else active.putAsset(item.path, item.sha);
      disk.set(item.path, item.kind === "text" ? textRecord(item) : { kind: "asset", sha: item.sha });
    }
    initial = [];
    filesCleanup = active.onFiles((events) => { if (!closed) onFiles(events, active); });
    presenceCleanup = active.ephemeral.subscribe(() => { if (!closed) onAwareness(active); });
    onSession(active);
    void follow();
    return active;
  }

  /// Records what the folder sync wrote, which is what the folder now holds.
  /** @param {{ texts?: Record<string, string>, digests?: Record<string, string> }} tree */
  function wrote({ texts = {}, digests = {} }) {
    const written = new Set([...Object.keys(texts), ...Object.keys(digests)]);
    for (const path of [...disk.keys()]) {
      if (!written.has(path) && !session?.list().some((file) => file.path === path)) disk.delete(path);
    }
    for (const [path, text] of Object.entries(texts)) {
      const before = disk.get(path);
      disk.set(path, {
        kind: "text",
        text,
        sha: null,
        ending: before?.kind === "text" ? before.ending : undefined,
        raw: before?.kind === "text" && before.text === text ? before.raw : undefined,
      });
    }
    for (const [path, sha] of Object.entries(digests)) disk.set(path, { kind: "asset", sha });
  }

  /// The bytes of a text as the folder holds them: the style it was read in,
  /// or exactly the bytes it was read as while it has not been edited.
  /** @param {string} path @param {string} text */
  function encode(path, text) {
    const known = disk.get(path);
    if (known?.kind === "text" && known.raw && known.text === text) return known.raw;
    return styled(text, known?.kind === "text" ? known.ending : undefined);
  }

  /// What the folder holds as the project was read, by path and digest, for
  /// the first sync to find nothing to send.
  function seed() {
    return seeded;
  }

  function close() {
    if (closed) return;
    closed = true;
    abort.abort();
    filesCleanup?.();
    filesCleanup = null;
    presenceCleanup?.();
    presenceCleanup = null;
    session?.leave();
    session = null;
    figures.release();
  }

  return {
    load,
    start,
    close,
    wrote,
    encode,
    seed,
    // There is no room to write to.
    send: () => undefined,
    syncStatus: () => ({}),
    sendLive: () => ({ ok: false, error: new Error("a local project has no room") }),
  };
}
