// What a pull or a push has to do, decided without touching the network.
//
// Both directions compare three things per path: what GitHub lists now, what
// the project holds now, and what the sync map says both held at the last
// sync. A side changed when it differs from the map. The planners are pure:
// the caller reads the link row, the listing and the project, hands blobs over
// through `readBlob`, and posts what comes back.
//
// A project snapshot is `Map<projectPath, { kind: "text", text, hash } |
// { kind: "asset", sha }>`. `text` is the text without pending suggested
// insertions, and `hash` is its `textHash`.

import { diff } from "@codemirror/merge";

import { sha256Hex, sha256HexOfText } from "../digest.js";
import { decodeText, isLfsPointer, textHash, toLf } from "./hash.js";

const encoder = new TextEncoder();

function contentHash(file) {
  return file.kind === "text" ? file.hash : file.sha;
}

function isHigh(code) {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLow(code) {
  return code >= 0xdc00 && code <= 0xdfff;
}

/// The edits that turn `from` into `to`, as patches on UTF-8 byte offsets of
/// `from`. The character diff works in UTF-16 units, so a boundary that falls
/// inside a surrogate pair moves outward to keep every patch whole characters.
export function textPatches(path, from, to) {
  const patches = [];
  let units = 0;
  let bytes = 0;
  const at = (index) => {
    bytes += encoder.encode(from.slice(units, index)).length;
    units = index;
    return bytes;
  };
  for (const change of diff(from, to)) {
    let { fromA, toA, fromB, toB } = change;
    if (
      fromA > 0
      && isHigh(from.charCodeAt(fromA - 1))
      && (isLow(from.charCodeAt(fromA)) || isLow(to.charCodeAt(fromB)))
    ) {
      fromA -= 1;
      fromB -= 1;
    }
    if (
      toA > 0
      && isHigh(from.charCodeAt(toA - 1))
      && (isLow(from.charCodeAt(toA)) || isLow(to.charCodeAt(toB)))
    ) {
      toA += 1;
      toB += 1;
    }
    if (fromA < units) continue;
    const start = at(fromA);
    const end = at(toA);
    patches.push({
      path,
      start,
      end,
      exact: from.slice(fromA, toA),
      replacement: to.slice(fromB, toB),
    });
  }
  return patches;
}

/// The dependency that guards a patched or deleted text file: the digest of
/// the text exactly as projected, so that typing between the plan and the
/// apply is noticed.
async function dependency(path, text) {
  return { path, file_hash: await sha256HexOfText(text), start: 0, end: 0, exact: "" };
}

function emptyChange() {
  return { dependencies: [], patches: [], create: [], delete: [], put_binary: [] };
}

/// The plan for a pull. `link` is the link row, `synced` the listing's synced
/// files from `classify`, `project` the snapshot, `readBlob(blobSha)` resolves
/// to the blob's bytes, and `exportMerged` says whether GitHub's head contains
/// the outstanding export. `isSynced`, when given, tells a path that left the
/// listing because it is no longer a project file (a new ignore line) from a
/// path that was deleted, so the first is forgotten and not deleted here.
///
/// Returns `{ conflicts, refusals, skipped, change, syncedMap, exportConsumed,
/// uploads }`. With any conflict, `change` is empty and nothing is to be sent.
export async function planPull({ link, synced, project, readBlob, exportMerged, isSynced }) {
  const map = link.synced_map || {};
  const exported = link.export?.map || {};
  const change = emptyChange();
  const syncedMap = {};
  const conflicts = [];
  const skipped = [];
  const uploads = [];
  const paths = [...new Set([...synced.keys(), ...Object.keys(map)])].sort();

  for (const path of paths) {
    const listed = synced.get(path);
    const base = map[path];
    const mine = project.get(path);

    if (!listed && isSynced && !isSynced(path)) continue;

    const github = listed ? !base || base.blob !== listed.blob : !!base;
    if (!github) {
      if (base) syncedMap[path] = base;
      continue;
    }
    const local = mine ? !base || base.hash !== contentHash(mine) : !!base;

    // What GitHub holds now, read once.
    let fetched = null;
    if (listed) {
      const bytes = await readBlob(listed.blob);
      const kind = mine?.kind || listed.kind;
      if (isLfsPointer(bytes)) {
        skipped.push({ path, reason: "a Git LFS pointer" });
      } else if (kind === "text") {
        const decoded = decodeText(bytes);
        if (decoded) {
          const text = toLf(decoded.text);
          fetched = { kind, bytes, text, hash: await textHash(text) };
        } else {
          skipped.push({ path, reason: "not valid UTF-8" });
        }
      } else {
        fetched = { kind, bytes, hash: await sha256Hex(bytes) };
      }
      if (!fetched) {
        if (base) syncedMap[path] = base;
        continue;
      }
    }

    if (local) {
      const same = listed ? !!mine && fetched.hash === contentHash(mine) : !mine;
      if (same) {
        if (listed) syncedMap[path] = { blob: listed.blob, hash: fetched.hash };
        continue;
      }
      const mineHash = mine ? contentHash(mine) : null;
      const exportedHash = exported[path]?.hash ?? null;
      const merged = exportMerged && !!link.export && mineHash === exportedHash;
      if (!merged) {
        conflicts.push(path);
        continue;
      }
    }

    // GitHub's side wins here: it changed and the project either did not or
    // already agrees through the export.
    if (!listed) {
      if (mine.kind === "text") {
        change.dependencies.push(await dependency(path, mine.text));
        change.delete.push({ path });
      } else {
        change.delete.push({ path, expect: mine.sha });
      }
      continue;
    }
    if (fetched.kind === "text") {
      if (!mine) {
        change.create.push({ path, text: fetched.text });
      } else {
        const patches = textPatches(path, mine.text, fetched.text);
        if (patches.length) {
          change.dependencies.push(await dependency(path, mine.text));
          change.patches.push(...patches);
        }
      }
    } else if (!mine || mine.sha !== fetched.hash) {
      change.put_binary.push({ path, hash: fetched.hash, expect: mine ? mine.sha : null });
      uploads.push({ path, bytes: fetched.bytes });
    }
    syncedMap[path] = { blob: listed.blob, hash: fetched.hash };
  }

  if (conflicts.length) {
    return {
      conflicts,
      refusals: [],
      skipped,
      change: emptyChange(),
      syncedMap: { ...map },
      exportConsumed: false,
      uploads: [],
    };
  }
  return {
    conflicts,
    refusals: [],
    skipped,
    change,
    syncedMap,
    exportConsumed: !!link.export && !!exportMerged,
    uploads,
  };
}

/// What a push or an export has to send: the synced paths whose content hash
/// differs from the sync map or that the map lacks, and the mapped paths the
/// project no longer holds. `isSynced(path)` is the rules' filter. A changed
/// entry is `{ path, kind, text, hash }` for text or `{ path, kind, sha, hash }`
/// for an asset.
export function planPush({ link, project, isSynced }) {
  const map = link.synced_map || {};
  const changed = [];
  const deleted = [];
  for (const [path, file] of [...project.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (!isSynced(path)) continue;
    const hash = contentHash(file);
    if (map[path]?.hash === hash) continue;
    if (file.kind === "text") changed.push({ path, kind: "text", text: file.text, hash });
    else changed.push({ path, kind: "asset", sha: file.sha, hash });
  }
  for (const path of Object.keys(map).sort()) {
    if (!project.has(path) && isSynced(path)) deleted.push(path);
  }
  return { changed, deleted };
}
