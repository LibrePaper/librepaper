// The projection: what a Loro document says, read as a directory.
//
// This is the browser's half of `librepaper-document-core::projection`, and
// the two are held equal by the corpus in `web/tests/fixtures/projection.json`
// -- the same file the Rust test reads. Every step below is numbered after
// SPEC-server-is-a-log §4.4, and the numbers are the contract: an
// implementation that reorders them produces a different tree for the same
// document, and the digest of that tree is what a reader's etag and a
// rendered comment's staleness check are both decided on.
//
// Nothing here writes to the document. There is no repair: a shape the schema
// does not use is absent from the projection and says so in a diagnostic.

import { LoroText } from "loro-crdt";
import { collisionKey, checkPath, normalisePath, suffixed } from "./paths.js";
import { sha256Hex, sha256HexOfText } from "./digest.js";
import { canonicalBytes, compareUtf8 } from "./projection-digest.js";

// Re-exported so a caller that already has a projection does not have to
// know that the canonical form lives one module down. It lives there because
// the LaTeX worker and the assistant preview need the name without needing
// `loro-crdt`, which this module imports.
export { canonicalBytes, compareUtf8 };

// A text's `bytes` is its UTF-8 length, which is what Rust's `String::len`
// counts. JavaScript's `.length` counts UTF-16 units and would disagree on
// any document with an accent or an emoji in it.
const encoder = new TextEncoder();

/// How long an asset digest is, spelled in hex.
export const ASSET_DIGEST_HEX = 64;

/// The bound on step 5's search. Larger than any document this deployment
/// holds, so reaching it means something pathological rather than crowded.
const MOST = 10_000;

const isHex = (value) => /^[0-9a-fA-F]+$/.test(value);

function diagnosticSortKey(diagnostic) {
  const { kind } = diagnostic;
  const first = diagnostic.id ?? diagnostic.path ?? "";
  let second = "";
  if (kind === "invalid-path" || kind === "dropped") second = diagnostic.path ?? "";
  if (kind === "moved") second = diagnostic.from ?? "";
  return [kind, first, second];
}

function compareKeys(left, right) {
  for (let at = 0; at < left.length; at += 1) {
    if (left[at] < right[at]) return -1;
    if (left[at] > right[at]) return 1;
  }
  return 0;
}

/// Whether `path` may be taken: not already a file, not a directory prefix of
/// a file already taken, and not under one. `taken` is a Set of collision
/// keys.
function free(taken, path) {
  const key = collisionKey(path);
  if (taken.has(key)) return false;
  const under = `${key}/`;
  for (const existing of taken) {
    if (existing.startsWith(under)) return false;
    if (key.startsWith(`${existing}/`)) return false;
  }
  return true;
}

/// The seven steps of §4.4, in order. `doc` is a `LoroDoc`; `rules` is the
/// deployment's path rules as `/api/config` serves them.
///
/// Returns `{ main, mainId, files, diagnostics, texts }`, where `files` maps
/// a path to `{ kind, id, digest, bytes }` in UTF-8 path order and `texts`
/// maps a path to its body.
export function projectDirectory(doc, rules) {
  const diagnostics = [];
  const candidates = [];

  // 1. Collect candidates.
  const filesMap = doc.getMap("files");
  const pathsMap = doc.getMap("paths");
  const assetsMap = doc.getMap("assets");
  const metaMap = doc.getMap("meta");

  const named = new Map();
  for (const id of pathsMap.keys()) {
    const value = pathsMap.get(id);
    if (typeof value === "string") named.set(id, value);
    else diagnostics.push({ kind: "unnamed", id });
  }

  const fileIds = [...filesMap.keys()].sort(compareUtf8);
  const textIds = new Set();
  for (const id of fileIds) {
    const value = filesMap.get(id);
    if (!(value instanceof LoroText)) {
      diagnostics.push({ kind: "not-a-text", id });
      continue;
    }
    textIds.add(id);
    if (!named.has(id)) {
      diagnostics.push({ kind: "unnamed", id });
      continue;
    }
    const body = value.toString();
    candidates.push({
      asset: false,
      key: id,
      id,
      requested: named.get(id),
      digest: "",
      bytes: encoder.encode(body).length,
      text: body,
    });
  }
  for (const id of named.keys()) {
    if (!textIds.has(id)) diagnostics.push({ kind: "dangling", id });
  }

  const assetPaths = [...assetsMap.keys()].sort(compareUtf8);
  for (const path of assetPaths) {
    const value = assetsMap.get(path);
    if (typeof value !== "string" || value.length !== ASSET_DIGEST_HEX || !isHex(value)) {
      diagnostics.push({ kind: "not-a-digest", path });
      continue;
    }
    candidates.push({
      asset: true,
      key: `asset:${path}`,
      id: "",
      requested: path,
      digest: value,
      bytes: 0,
      text: null,
    });
  }

  // 2. Validate paths.
  const valid = [];
  for (const candidate of candidates) {
    const wanted = candidate.asset ? "asset" : "text";
    // A session can receive its first state before `/api/config` resolves.
    // It still needs deterministic collision handling in that interval; the
    // configured projection applies the deployment's extension rules as soon
    // as they arrive.
    const outcome = rules === null ? { kind: wanted } : checkPath(rules, candidate.requested);
    if (outcome.error) {
      diagnostics.push({
        kind: "invalid-path",
        id: candidate.key,
        path: candidate.requested,
        why: outcome.error,
      });
      continue;
    }
    if (outcome.kind !== wanted) {
      diagnostics.push({
        kind: "invalid-path",
        id: candidate.key,
        path: candidate.requested,
        why: `${candidate.requested}: the extension names a ${outcome.kind} and this is a ${wanted}`,
      });
      continue;
    }
    valid.push(candidate);
  }

  // 3. Order: kind first, text before asset, then the key as a byte string.
  valid.sort((left, right) => {
    if (left.asset !== right.asset) return left.asset ? 1 : -1;
    return compareUtf8(left.key, right.key);
  });

  // 4. Reserve.
  const taken = new Set();
  const placed = [];
  const deferred = [];
  for (const candidate of valid) {
    const normalised = normalisePath(candidate.requested);
    if (free(taken, normalised)) {
      taken.add(collisionKey(normalised));
      placed.push([candidate, normalised]);
    } else {
      deferred.push(candidate);
    }
  }

  // 5. Suffix.
  for (const candidate of deferred) {
    const requested = normalisePath(candidate.requested);
    let chosen = null;
    for (let nth = 2; nth < MOST; nth += 1) {
      const attempt = suffixed(requested, nth);
      if (free(taken, attempt)) {
        chosen = attempt;
        break;
      }
    }
    if (chosen === null) {
      diagnostics.push({ kind: "dropped", id: candidate.key, path: requested });
      continue;
    }
    taken.add(collisionKey(chosen));
    diagnostics.push({ kind: "moved", id: candidate.key, from: requested, to: chosen });
    placed.push([candidate, chosen]);
  }

  const files = new Map();
  const texts = new Map();
  const pathOfId = new Map();
  placed.sort((left, right) => compareUtf8(left[1], right[1]));
  for (const [candidate, path] of placed) {
    if (candidate.text !== null) {
      texts.set(path, candidate.text);
      pathOfId.set(candidate.id, path);
    }
    const entry = {
      kind: candidate.asset ? "asset" : "text",
      id: candidate.id,
      digest: candidate.digest,
      bytes: candidate.bytes,
    };
    // Kept outside the enumerable wire shape: the live editor needs the CRDT
    // key when a canonical asset path was normalised or suffixed.
    Object.defineProperty(entry, "source", { value: candidate.asset ? candidate.requested : candidate.id });
    files.set(path, entry);
  }

  // 6. Main.
  const declared = typeof metaMap.get("main") === "string" ? metaMap.get("main") : "";
  let mainId = "";
  let main = "";
  if (pathOfId.has(declared)) {
    mainId = declared;
    main = pathOfId.get(declared);
  } else {
    if (declared) diagnostics.push({ kind: "main-missing", id: declared });
    for (const candidate of valid) {
      if (candidate.asset) continue;
      if (pathOfId.has(candidate.id)) {
        mainId = candidate.id;
        main = pathOfId.get(candidate.id);
        break;
      }
    }
  }

  diagnostics.sort((left, right) => compareKeys(diagnosticSortKey(left), diagnosticSortKey(right)));

  return { main, mainId, files, diagnostics, texts };
}

/// The canonical projection, including the content digests used by snapshots.
/// Directory and render-tree callers that do not need hashes use
/// `projectDirectory`; both paths therefore share all validation, collision,
/// ordering, and main-file decisions.
export async function project(doc, rules) {
  const projection = projectDirectory(doc, rules);
  await Promise.all([...projection.texts.entries()].map(async ([path, body]) => {
    projection.files.get(path).digest = await sha256HexOfText(body);
  }));
  return projection;
}

// 7. Digest. The canonical form lives in `projection-digest.js`; this is
// the convenience that takes it over a projection this module just built.
export async function projectionDigest(projection) {
  return sha256Hex(canonicalBytes(projection));
}
