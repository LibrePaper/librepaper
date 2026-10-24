import { VersionVector } from "loro-crdt";

import { checkPath, collisionKey, normalisePath } from "../paths.js";
import { sha256Hex, sha256HexOfText } from "../digest.js";
import { unsuggestedText } from "../track-changes.js";
import { decodeText, isLfsPointer, textHash, toLf } from "./hash.js";
import { textPatches } from "./patches.js";
import { syncRules } from "./rules.js";

// Pull, push and the first connection, driven from the desktop page. The
// sidecar does the Git work; this module decides which paths sync, builds the
// three inputs of a merge, turns a tree difference into one checked source
// change and records the outcome on the server.
//
// `ctx` is { slug, session, rules, git, api, me, maxFileBytes?, arrivalMs? }.
// `git` is the bridge of bridge.js. `api` is { getLink, putLink, deleteLink,
// sourceChange, uploadAsset, assetBytes } from server.js; its errors carry
// `status` and `body`. `session` exposes `doc` and `list()`. `me` is
// { name, handle } of the person pressing the button.
//
// Paths in the three inputs (the baseline B', the incoming side R' and the
// local side L) are project paths, with the linked folder stripped.

const MAX_CHANGED_TRIES = 3;
const MAX_STALE_TRIES = 5;
const ARRIVAL_MS = 15_000;
const DEFAULT_MAX_FILE_BYTES = 10 * 1024 * 1024;
/// A sent request is kept in the journal only when it is small, because the
/// journal file is limited. A larger one is rebuilt by a fresh pull.
const JOURNAL_BODY_LIMIT = 256 * 1024;
/// An asset this small can be a Git LFS pointer, so it is read to find out.
const LFS_POINTER_MAX_BYTES = 1024;
const IGNORE_FILE = ".librepaperignore";
const GIT_KINDS = new Set(["auth", "network", "not-found"]);
const decoder = new TextDecoder();
/// File content keeps a leading byte order mark, as the project text does.
const contentDecoder = new TextDecoder("utf-8", { ignoreBOM: true });
const encoder = new TextEncoder();
const NOT_CAUGHT_UP = "This page has not received every change yet. Try again in a moment.";
const RECONNECT = "This computer is connected to a different repository for this project. Connect this computer again.";
const NOT_LINKED = "This project is not connected to a Git repository.";
const NOT_SYNCED = "This computer has not synced this project yet.";
const GONE_FROM_REMOTE = "The commit this project last synced with is no longer on the remote.";
const PULL_FIRST = "The remote has new commits. Pull first, then push.";
const COLLIDING = "These files would have the same name in LibrePaper, which ignores case and accents in names. Rename one of them, in LibrePaper or in the repository, then try again.";
/// Error kinds that say a commit is not in the private repository.
const MISSING_KINDS = new Set(["not-found", "missing"]);

const answered = (error) => typeof error?.status === "number";

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const operationId = () => `v2.${Date.now()}.${randomHex(16)}`;
const refId = () => `op-${randomHex(8)}`;

/**
 * @param {string} status
 * @param {Record<string, any>} [fields]
 * @returns {{ status: string } & Record<string, any>}
 */
function result(status, fields = {}) {
  return { status, ...fields };
}

function refused(reason, paths = [], fields = {}) {
  return result("refused", { refusals: [{ reason, paths }], ...fields });
}

/// Turns a failure of Git to reach the remote into a refusal the panel shows.
async function guarded(work) {
  try {
    return await work();
  } catch (error) {
    if (GIT_KINDS.has(error?.kind)) return refused(error.message, [], { kind: error.kind });
    throw error;
  }
}

// The manual Git panel and automatic sync share one journal, one project and
// one shared record. One operation at a time per project keeps recovery from
// settling a journal entry whose push or pull is still in flight. Where the Web
// Locks API exists the lock spans every window of the same origin, so two
// windows on one project take turns. Elsewhere it covers this window only. The
// lock is not reentrant: a function that runs under it must not call another one.
const tails = new Map();

/// Runs `work` once the earlier operations on the same project have finished,
/// whether they succeeded or not, and releases the lock even when `work` throws.
async function exclusive(ctx, work) {
  const locks = globalThis.navigator?.locks;
  if (locks?.request) return locks.request(`librepaper:git:${ctx.slug}`, work);
  const previous = tails.get(ctx.slug) || Promise.resolve();
  const turn = previous.catch(() => {}).then(work);
  const tail = turn.catch(() => {});
  tails.set(ctx.slug, tail);
  try {
    return await turn;
  } finally {
    if (tails.get(ctx.slug) === tail) tails.delete(ctx.slug);
  }
}

function byPath(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function entryMap(entries) {
  return new Map(entries.filter((entry) => entry.type === "blob").map((entry) => [entry.path, entry]));
}

function linkFields(link) {
  return {
    remote_kind: link.remote_kind,
    remote_label: link.remote_label,
    connection_id: link.connection_id,
    branch: link.branch,
    folder: link.folder,
  };
}

function authorOf(ctx) {
  const handle = ctx.me?.handle || "librepaper";
  return { name: ctx.me?.name || handle, email: `${handle}@users.noreply.${location.hostname}` };
}

/// Whether this computer's sidecar is connected to the repository the shared
/// record names. Answers a refusal when it is not, else null.
async function connectionRefusal(ctx, link) {
  const status = await ctx.git.status();
  if (status?.connection_id && status.connection_id === link.connection_id) return null;
  return refused(RECONNECT, [], { reconnect: true });
}

/// Whether an error is a clear answer that a request was not applied: a 400,
/// or a 409 whose reason or kind says the record, the files, the suggestions,
/// the branch or the connection moved. Anything else (a 5xx, a 502, a lost
/// connection) leaves the outcome unknown, so the journal entry and the refs
/// stay for recovery.
function definitive(error) {
  if (!answered(error)) return false;
  if (error.status === 400) return true;
  if (error.status !== 409) return false;
  const reason = error.body?.reason;
  const kind = error.kind ?? error.body?.kind;
  return ["stale", "changed", "suggestion"].includes(reason) || ["moved", "stale-connection"].includes(kind);
}

const kindOf = (error) => error?.kind ?? error?.body?.kind;

// ---------------------------------------------------------------- version check

/// Pages that timed out waiting for a pull to arrive, until a version check
/// passes for them.
const timedOut = new WeakSet();

function decodeVersion(text) {
  if (!text) return null;
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return VersionVector.decode(bytes);
}

/// Waits, bounded, until this page holds every change the server holds (the
/// `version` the server answered) and has none left to send. A page that has
/// not received a change would otherwise send back content that was already
/// pulled. Answers false after the bound.
async function caughtUp(ctx, version) {
  const wanted = decodeVersion(version);
  const deadline = Date.now() + (ctx.arrivalMs ?? ARRIVAL_MS);
  for (;;) {
    const relation = wanted ? ctx.session.doc.oplogVersion().compare(wanted) : 0;
    const covered = relation === 0 || relation === 1;
    const unsent = ctx.session.unacknowledgedCount?.() ?? 0;
    if (covered && !unsent) {
      timedOut.delete(ctx.session);
      return true;
    }
    if (Date.now() > deadline) return false;
    await new Promise((done) => setTimeout(done, 100));
  }
}

/// The refusal for a page that is not caught up, or null once it is.
async function catchUpRefusal(ctx, version) {
  return (await caughtUp(ctx, version)) ? null : refused(NOT_CAUGHT_UP);
}

// ---------------------------------------------------------------- journal

async function journalRead(ctx) {
  return (await ctx.git.journal()) || [];
}

async function journalSet(ctx, entry) {
  await ctx.git.journalUpsert(entry);
}

async function journalDrop(ctx, id) {
  await ctx.git.journalRemove(id);
}

/// Ends an operation: its commits are no longer kept and its entry is gone.
async function finish(ctx, id) {
  await ctx.git.release(id);
  await journalDrop(ctx, id);
}

// ---------------------------------------------------------------- filter

/// `F(commit)`: the classified tree of a commit under the link's folder and the
/// ignore files found in that commit. Returns what `classify` returns plus
/// `rules` (for path spelling and `isSynced`), `tree`, `commit` and `entries`,
/// the blob entries by repository path, `occupied`, the normalized project path
/// of every entry the listing holds under the folder, whatever its type, and
/// `held`, the same for every such entry that is not a tree and is not synced.
/// A `commit` of null is the empty remote of a branch without commits.
export async function filterAt(ctx, link, commit) {
  const listing = commit ? await ctx.git.tree(commit) : { tree: null, entries: [] };
  const entries = entryMap(listing.entries);
  const folder = link.folder || "";
  const rootEntry = entries.get(IGNORE_FILE);
  const folderEntry = folder ? entries.get(`${folder}/${IGNORE_FILE}`) : null;
  const wanted = [rootEntry, folderEntry].filter(Boolean).map((entry) => entry.id);
  const blobs = wanted.length ? await ctx.git.blobs(wanted) : new Map();
  const read = (entry) => (entry ? decoder.decode(blobs.get(entry.id)) : null);
  const rules = syncRules({
    rules: ctx.rules,
    folder,
    maxFileBytes: ctx.maxFileBytes ?? DEFAULT_MAX_FILE_BYTES,
    ignoreRoot: read(rootEntry),
    ignoreFolder: read(folderEntry),
  });
  const classified = rules.classify(listing.entries);
  const prefix = rules.repoPath("");
  const under = listing.entries.filter((entry) => entry.path.startsWith(prefix));
  const occupied = new Set(under.map((entry) => normalisePath(rules.projectPath(entry.path))));
  const held = new Set(
    under
      .filter((entry) => entry.type !== "tree")
      .map((entry) => normalisePath(rules.projectPath(entry.path)))
      .filter((key) => !classified.synced.has(key)),
  );
  return { ...classified, rules, tree: listing.tree, commit, entries, occupied, held };
}

// ---------------------------------------------------------------- snapshots

/// What a commit holds at some project paths, read for normalization. Text is
/// decoded and made LF; an asset is only named by its blob id. A text file
/// that is not valid UTF-8, and a text file or an asset that is a Git LFS
/// pointer, is left out and listed in `skipped`. A text file whose bytes are
/// already LF keeps its blob id. `items` maps a project path to
/// `{ repoPath, kind }`.
async function loadFiles(ctx, items, entries, paths) {
  const files = new Map();
  const skipped = [];
  const texts = [];
  const probes = [];
  const sizes = new Map();
  for (const path of paths) {
    const item = items.get(path);
    const entry = item && entries.get(item.repoPath);
    if (!entry) continue;
    sizes.set(entry.id, entry.size);
    if (item.kind !== "asset") texts.push({ path, entry });
    else if (entry.size <= LFS_POINTER_MAX_BYTES) probes.push({ path, entry });
    else files.set(path, { kind: "asset", id: entry.id });
  }
  const wanted = [...texts, ...probes].map(({ entry }) => entry.id);
  const blobs = wanted.length ? await ctx.git.blobs(wanted, sizes) : new Map();
  for (const { path, entry } of probes) {
    if (isLfsPointer(blobs.get(entry.id))) skipped.push({ path, reason: "a Git LFS pointer" });
    else files.set(path, { kind: "asset", id: entry.id });
  }
  for (const { path, entry } of texts) {
    const bytes = blobs.get(entry.id);
    if (isLfsPointer(bytes)) {
      skipped.push({ path, reason: "a Git LFS pointer" });
      continue;
    }
    const decoded = decodeText(bytes);
    if (!decoded) {
      skipped.push({ path, reason: "not valid UTF-8" });
      continue;
    }
    const text = toLf(decoded.text);
    files.set(path, { kind: "text", text, id: text === decoded.text ? entry.id : null });
  }
  return { files, skipped };
}

/// Removes skipped files from the filter, so that they are in no input and in
/// no synced path list.
function pruneSkipped(filter, skipped) {
  for (const { path, reason } of skipped) {
    const item = filter.synced.get(path);
    if (!item) continue;
    filter.synced.delete(path);
    filter.unsynced.push({ path: item.repoPath, reason });
  }
}

/// A commit whose tree holds `files` (a map from project path to
/// `{ kind, text?, id? }`) and whose author is fixed, so equal inputs give
/// equal ids on every machine.
async function commitFiles(ctx, files, parent) {
  const paths = [...files.keys()].sort(byPath);
  const missing = paths.filter((path) => !files.get(path).id);
  const made = missing.length ? await ctx.git.putBlobs(missing.map((path) => ({ text: files.get(path).text }))) : [];
  const created = new Map(missing.map((path, index) => [path, made[index]]));
  const edits = paths.map((path) => ({ path, id: files.get(path).id || created.get(path) }));
  const tree = await ctx.git.treeEdit(null, edits);
  const commit = await ctx.git.commitTree(tree, parent ? [parent] : [], "LibrePaper sync snapshot", null);
  return { commit, tree };
}

/// `N(X, P)`: the normalized commit of the files at project paths `paths` of
/// commit `X`, whose blob entries are `entries`. `items` maps each project path
/// to its `{ repoPath, kind }`.
export async function normalized(ctx, items, entries, paths, parent) {
  const loaded = await loadFiles(ctx, items, entries, paths);
  return { ...(await commitFiles(ctx, loaded.files, parent)), skipped: loaded.skipped };
}

/// Whether writing a file at a project path would replace something the remote
/// holds and does not sync: an entry at the same path (a link, a submodule, an
/// oversize or LFS file, invalid UTF-8, or a folder), or an entry other than a
/// folder above it, which the file's folder would replace. Paths are compared
/// by their normalized project spelling, since the repository may spell a name
/// differently from the project.
function blocked(filter, path) {
  const key = normalisePath(path);
  if (filter.occupied.has(key) && !filter.synced.has(key)) return true;
  const parts = key.split("/");
  for (let i = 1; i < parts.length; i += 1) {
    if (filter.held.has(parts.slice(0, i).join("/"))) return true;
  }
  return false;
}

/// `L`: the project's files that pass the filter, as a commit with `parent`.
/// A project path whose repository path the remote holds but does not sync (a
/// link, a submodule, an oversize or LFS file, invalid UTF-8) is left out, so
/// that it is never sent over that entry. A file over the size limit is left
/// out and listed in `filter.unsynced`, and its path is returned in `oversize`
/// so callers leave it out of every other input too. Also returns the texts
/// and asset digests it was built from, and the kind of each file.
export async function snapshot(ctx, filter, parent) {
  const files = new Map();
  const texts = new Map();
  const assets = new Map();
  const kinds = new Map();
  const limit = ctx.maxFileBytes ?? DEFAULT_MAX_FILE_BYTES;
  const oversize = [];
  const listed = ctx.session.list()
    .filter((file) => filter.rules.isSynced(file.path) && !blocked(filter, file.path))
    .sort((a, b) => byPath(a.path, b.path));
  const shas = [...new Set(listed.filter((file) => file.kind === "asset").map((file) => file.sha))];
  const known = shas.length ? await ctx.git.knownAssets(shas) : new Map();
  // An asset over the limit is never uploaded: it is left out of every input.
  const tooBig = new Set();
  for (const sha of shas) {
    if (known.has(sha)) continue;
    const bytes = await ctx.api.assetBytes(sha);
    if (bytes.length > limit) {
      tooBig.add(sha);
      continue;
    }
    const [id] = await ctx.git.putBlobs([{ bytes, sha256: sha }]);
    known.set(sha, id);
  }
  for (const file of listed) {
    if (file.kind === "asset") {
      if (tooBig.has(file.sha)) {
        oversize.push(file.path);
        continue;
      }
      files.set(file.path, { kind: "asset", id: known.get(file.sha) });
      assets.set(file.path, file.sha);
      kinds.set(file.path, "asset");
      continue;
    }
    const text = unsuggestedText(ctx.session.doc, file.id);
    if (text == null) continue;
    if (encoder.encode(text).length > limit) {
      oversize.push(file.path);
      continue;
    }
    files.set(file.path, { kind: "text", text, id: null });
    texts.set(file.path, text);
    kinds.set(file.path, "text");
  }
  let built = await commitFiles(ctx, files, parent);
  if (assets.size) {
    // An asset's size is known only from the tree, so the commit is listed and
    // rebuilt without any file over the limit.
    const big = (await ctx.git.tree(built.commit)).entries
      .filter((entry) => entry.type === "blob" && entry.size > limit)
      .map((entry) => entry.path);
    if (big.length) {
      for (const path of big) {
        files.delete(path);
        texts.delete(path);
        assets.delete(path);
        kinds.delete(path);
        oversize.push(path);
      }
      built = await commitFiles(ctx, files, parent);
    }
  }
  for (const path of oversize) {
    const repoPath = filter.rules.repoPath(path);
    if (!filter.unsynced.some((item) => item.path === repoPath)) {
      filter.unsynced.push({ path: repoPath, reason: "over the file size limit" });
    }
  }
  return { ...built, texts, assets, kinds, paths: [...files.keys()], oversize };
}

/// The refusal for a remote that holds more project files than a project may,
/// counted after files left out for their content are pruned, or null.
function tooManyFiles(filter) {
  const max = filter.rules.maxFiles;
  if (filter.synced.size <= max) return null;
  return {
    reason: `the repository has ${filter.synced.size} syncable files and a project may hold ${max}`,
    paths: [],
  };
}

// ---------------------------------------------------------------- source change

/// Reads `ids` into a map, one call per group so that a large asset is asked
/// for alone.
async function readBlobs(ctx, ids) {
  const found = new Map();
  for (const id of new Set(ids)) {
    if (id) found.set(id, (await ctx.git.blobs([id])).get(id));
  }
  return found;
}

/// The paths of the blobs in `commit`, which for L are project paths.
async function blobPaths(ctx, commit) {
  return (await ctx.git.tree(commit)).entries.filter((entry) => entry.type === "blob").map((entry) => entry.path);
}

/// The refusal for a change that would leave two project paths with the same
/// collision key, or null. `held` lists the project paths of L and `changes`
/// is `diff(L, merged)`. Both sides are compared by collision key, so a
/// spelling that differs only in case or accents is caught too.
function colliding(held, changes) {
  const after = new Set(held);
  for (const item of changes) {
    if (item.status === "D") after.delete(item.path);
    else after.add(item.path);
  }
  const groups = new Map();
  for (const path of after) {
    const key = collisionKey(path);
    groups.set(key, [...(groups.get(key) || []), path]);
  }
  const paths = [...groups.values()].filter((group) => group.length > 1).flat().sort(byPath);
  return paths.length ? { reason: COLLIDING, paths } : null;
}

/// The source change that turns L into the merged tree, from `changes`
/// (`diff(L, merged)`). `known` may hold `texts` and `assets` of L by path, so
/// that they are not read back. Uploads assets as it goes. Returns
/// `{ change, files, expect }` or `{ refusal }`.
async function buildChange(ctx, changes, kinds, known) {
  const clash = colliding(known?.paths ?? [], changes);
  if (clash) return { refusal: clash };
  const change = { dependencies: [], patches: [], create: [], delete: [], put_binary: [] };
  const expect = { texts: new Map(), assets: new Map(), deleted: new Set() };
  const files = [];
  const needed = [];
  for (const item of changes) {
    const kind = kinds.get(item.path);
    if (item.status !== "D") needed.push(item.to);
    if (item.status !== "A") {
      const have = kind === "text" ? known?.texts?.has(item.path) : known?.assets?.has(item.path);
      if (!have) needed.push(item.from);
    }
  }
  const blobs = await readBlobs(ctx, needed);
  const lText = (item) => known?.texts?.get(item.path) ?? contentDecoder.decode(blobs.get(item.from));
  const lAsset = async (item) => known?.assets?.get(item.path) ?? sha256Hex(blobs.get(item.from));

  for (const item of [...changes].sort((a, b) => byPath(a.path, b.path))) {
    const { path, status } = item;
    const kind = kinds.get(path);
    if (!kind) throw new Error(`The merge produced ${path}, which is neither a text file nor an asset.`);
    if (kind === "text") {
      if (status === "D") {
        const text = lText(item);
        change.dependencies.push({ path, file_hash: await sha256HexOfText(text), start: 0, end: 0, exact: "" });
        change.delete.push({ path });
        expect.deleted.add(path);
      } else {
        const decoded = decodeText(blobs.get(item.to));
        if (!decoded) return { refusal: { reason: "the merged file is not valid UTF-8", paths: [path] } };
        const merged = decoded.text;
        if (status === "A") {
          change.create.push({ path, text: merged });
        } else {
          const from = lText(item);
          const patches = textPatches(path, from, merged);
          if (!patches.length) continue;
          change.dependencies.push({ path, file_hash: await sha256HexOfText(from), start: 0, end: 0, exact: "" });
          change.patches.push(...patches);
        }
        expect.texts.set(path, await sha256HexOfText(merged));
      }
    } else if (status === "D") {
      change.delete.push({ path, expect: await lAsset(item) });
      expect.deleted.add(path);
    } else {
      const bytes = blobs.get(item.to);
      const hash = await sha256Hex(bytes);
      const current = status === "A" ? null : await lAsset(item);
      if (current === hash) continue;
      // A retried pull builds the change again, and uploads are rate limited,
      // so an asset the server already holds is not sent twice.
      if (!(await ctx.api.hasAsset(hash))) await ctx.api.uploadAsset(bytes);
      change.put_binary.push({ path, hash, expect: current });
      expect.assets.set(path, hash);
    }
    files.push({ path, status });
  }
  return { change, files, expect };
}

/// Waits until this page's document shows what the server applied: every
/// written file holds the merged content and every deleted file is gone. The
/// change returns over the socket like a coauthor's edit. Answers false after
/// a bound, which happens when someone types in a pulled file meanwhile.
async function untilArrived(ctx, expect) {
  if (!expect.texts.size && !expect.assets.size && !expect.deleted.size) return true;
  const deadline = Date.now() + (ctx.arrivalMs ?? ARRIVAL_MS);
  for (;;) {
    const present = new Map(ctx.session.list().map((file) => [file.path, file]));
    let landed = true;
    for (const [path, hash] of expect.texts) {
      const file = present.get(path);
      const text = file?.kind === "text" ? unsuggestedText(ctx.session.doc, file.id) : null;
      if (text == null || (await textHash(text)) !== hash) landed = false;
    }
    for (const [path, sha] of expect.assets) {
      const file = present.get(path);
      if (file?.kind !== "asset" || file.sha !== sha) landed = false;
    }
    for (const path of expect.deleted) if (present.has(path)) landed = false;
    if (landed) return true;
    if (Date.now() > deadline) return false;
    await new Promise((done) => setTimeout(done, 100));
  }
}

// ---------------------------------------------------------------- pull

/// Git merges every file as text, which can join two edits of a binary into
/// bytes nobody wrote. So the merge is never trusted for assets: each asset
/// that differs from the baseline on both sides, and differs between the sides
/// (an absent file counts as a value), is a conflict whether or not Git
/// reported it. Answers the merge with those added, and not clean if any was.
async function assetConflicts(ctx, merge, kinds, [base, ours, theirs]) {
  const assets = [...kinds].filter(([, kind]) => kind === "asset").map(([path]) => path);
  if (!assets.length) return merge;
  const [b, o, t] = await Promise.all([base, ours, theirs].map(async (commit) => entryMap((await ctx.git.tree(commit)).entries)));
  const conflicts = [...merge.conflicts || []];
  const listed = new Set(conflicts.map((item) => item.path));
  for (const path of assets.sort(byPath)) {
    if (listed.has(path)) continue;
    const ids = [b, o, t].map((entries) => entries.get(path)?.id ?? null);
    if (ids[0] !== ids[1] && ids[0] !== ids[2] && ids[1] !== ids[2]) {
      conflicts.push({ path, base: ids[0], ours: ids[1], theirs: ids[2] });
    }
  }
  if (conflicts.length === (merge.conflicts || []).length) return merge;
  return { ...merge, clean: false, conflicts };
}

/// The base commit of a merge, B' = N(C, S intersect F(R)): the paths last
/// synced that the rules still select, whether or not the remote still holds
/// them, so a file the remote deleted is a deletion and not an addition on the
/// local side. A link without a checkpoint has an empty base.
async function lastSynced(ctx, link, filter, oversize) {
  if (!link.checkpoint) return commitFiles(ctx, new Map(), null);
  const listing = await ctx.git.tree(link.checkpoint);
  // The checkpoint may spell a name differently from the project (a
  // decomposed accent), so its own entries are looked up by normalized
  // project path.
  const prefix = filter.rules.repoPath("");
  const spelled = new Map();
  for (const entry of listing.entries) {
    if (entry.type !== "blob" || !entry.path.startsWith(prefix)) continue;
    spelled.set(normalisePath(filter.rules.projectPath(entry.path)), entry.path);
  }
  const items = new Map();
  for (const path of link.synced_paths || []) {
    if (!filter.rules.isSynced(path) || oversize.has(path) || blocked(filter, path)) continue;
    const kind = checkPath({ ...ctx.rules, max_path: Infinity }, path).kind;
    if (kind) items.set(path, { repoPath: spelled.get(normalisePath(path)) ?? filter.rules.repoPath(path), kind });
  }
  return normalized(ctx, items, entryMap(listing.entries), [...items.keys()].sort(byPath), null);
}

/// Everything a pull needs after fetching: the filter, the three inputs and
/// the merge. Returns `{ refusal }` when the pull cannot start, or when
/// `options.current` is set and the remote has commits the checkpoint lacks.
async function prepare(ctx, link, op, options) {
  const fetched = await ctx.git.fetch(link.checkpoint ? [link.checkpoint] : []);
  if (link.checkpoint && fetched.missing?.includes(link.checkpoint)) {
    return {
      refusal: refused(
        "The commit this project last synced with is no longer on the remote. Connect again to compare the files.",
        [],
        { rebaseline: true },
      ),
    };
  }
  if (!fetched.head && link.checkpoint) {
    return { refusal: refused("The branch no longer has any commits.") };
  }
  // A branch without commits has nothing to bring in and nothing to record.
  const remote = fetched.head || null;
  if (options.current && remote !== (link.checkpoint || null)) {
    return { refusal: result("behind", { head: remote, reason: PULL_FIRST }) };
  }
  const filter = await filterAt(ctx, link, remote);
  if (filter.refusals.length) return { refusal: result("refused", { refusals: filter.refusals }) };
  if (!remote) return { empty: true, link, filter, theirs: await commitFiles(ctx, new Map(), null) };

  const incoming = await loadFiles(ctx, filter.synced, filter.entries, [...filter.synced.keys()]);
  pruneSkipped(filter, incoming.skipped);
  const excess = tooManyFiles(filter);
  if (excess) return { refusal: result("refused", { refusals: [excess] }) };
  // A project file over the size limit is unsynced in both directions. Left
  // out of L alone, the merge would read it as a local deletion and push that.
  const oversize = new Set((await snapshot(ctx, filter, null)).oversize);
  for (const path of oversize) {
    filter.synced.delete(path);
    incoming.files.delete(path);
  }
  const paths = [...filter.synced.keys()].sort(byPath);

  const base = await lastSynced(ctx, link, filter, oversize);
  const theirs = await commitFiles(ctx, incoming.files, base.commit);
  const local = await snapshot(ctx, filter, base.commit);

  const kinds = new Map(paths.map((path) => [path, filter.synced.get(path).kind]));
  for (const [path, kind] of local.kinds) kinds.set(path, kind);

  await ctx.git.keep(op, { base: base.commit, local: local.commit, remote: theirs.commit });
  const merge = await assetConflicts(
    ctx,
    await ctx.git.merge(local.commit, theirs.commit),
    kinds,
    [base.commit, local.commit, theirs.commit],
  );
  return { link, remote, filter, paths, kinds, base, theirs, local, merge, skipped: incoming.skipped };
}

/// Applies the merged tree: one source-change request, then the wait for it to
/// reach the page. Returns `{ retry }` for a stale record or a refused
/// dependency, else `{ result, state }`.
async function complete(ctx, prepared, tree, op, resolves = null) {
  const { link, remote, paths, kinds, local, theirs, base } = prepared;
  const changes = await ctx.git.diff(local.commit, tree);
  const built = await buildChange(ctx, changes, kinds, local);
  if (built.refusal) {
    await finish(ctx, op);
    return { result: result("refused", { refusals: [built.refusal] }) };
  }
  const body = {
    operation: operationId(),
    change: built.change,
    sync: { revision: link.revision, connection_id: link.connection_id, checkpoint: remote, synced_paths: paths },
  };
  const merged = await ctx.git.commitTree(tree, [local.commit, theirs.commit], "LibrePaper sync merge", null);
  await ctx.git.keep(op, { base: base.commit, local: local.commit, remote: theirs.commit, merged });
  prepared.sent = true;
  const keepBody = JSON.stringify(body).length < JOURNAL_BODY_LIMIT;
  await journalSet(ctx, {
    id: op,
    kind: "pull",
    phase: "sent",
    revision: link.revision,
    connection_id: link.connection_id,
    checkpoint: remote,
    body: keepBody ? body : null,
    // The conflict session this request resolves, so recovery can finish it.
    ...(resolves ? { resolves } : {}),
  });

  let answer;
  try {
    answer = await ctx.api.sourceChange(body);
  } catch (error) {
    if (!definitive(error)) throw error;
    await finish(ctx, op);
    const reason = error.body?.reason;
    if (error.status === 409 && reason === "stale") return { retry: "stale" };
    if (error.status === 409 && reason === "changed") return { retry: "changed", files: error.body.files || [] };
    if (error.status === 409 && reason === "suggestion") {
      return {
        result: refused(
          `${error.body.path} has a pending suggestion that the change would touch.`,
          [error.body.path],
          { suggestion: { path: error.body.path, change: error.body.change } },
        ),
      };
    }
    throw error;
  }
  const arrived = await untilArrived(ctx, built.expect);
  await finish(ctx, op);
  return {
    result: result(built.files.length ? "pulled" : "nothing", {
      files: built.files,
      skipped: prepared.skipped || [],
      link: answer.link,
      arrived,
    }),
    state: { filter: prepared.filter, remote, theirs, link: answer.link, version: answer.version },
  };
}

/// Saves the inputs of a conflicted merge so that the resolver can work on
/// them and a restart can restore them.
async function openConflict(ctx, prepared, op, carry) {
  const { link, remote, paths, kinds, local, theirs, base, merge } = prepared;
  const conflicts = merge.conflicts.map((item) => ({
    path: item.path,
    base: item.base,
    ours: item.ours,
    theirs: item.theirs,
    kind: kinds.get(item.path) || "text",
  }));
  // The merged tree is only an id until a commit holds it, and an unreferenced
  // tree and its blobs could be collected before the session is applied.
  const merged = await ctx.git.commitTree(merge.tree, [local.commit, theirs.commit], "LibrePaper sync conflict", null);
  await ctx.git.keep(op, { merged });
  const session = {
    id: op,
    revision: link.revision,
    connection_id: link.connection_id,
    checkpoint: link.checkpoint,
    remote,
    base: base.commit,
    local: local.commit,
    theirs: theirs.commit,
    tree: merge.tree,
    merged,
    paths,
    kinds: Object.fromEntries(kinds),
    conflicts,
    carried: {},
  };
  if (carry) {
    for (const item of conflicts) {
      const was = carry.session.conflicts.find((old) => old.path === item.path);
      const resolution = carry.resolutions[item.path];
      if (was && resolution && was.base === item.base && was.ours === item.ours && was.theirs === item.theirs) {
        session.carried[item.path] = resolution;
      }
    }
  }
  // Resolutions saved on the newest older session of this connection carry
  // over too, so pulling again does not lose them.
  const older = (await journalRead(ctx)).filter((entry) => entry.kind === "conflict" && entry.id !== op);
  const same = older.filter((entry) => entry.session?.connection_id === link.connection_id);
  const saved = same.length ? await restoreSession(ctx, same.at(-1).session) : null;
  if (saved) {
    const resolutions = { ...saved.carried, ...saved.resolutions };
    for (const item of conflicts) {
      const was = saved.conflicts.find((old) => old.path === item.path);
      const resolution = resolutions[item.path];
      if (!session.carried[item.path] && was && resolution && was.base === item.base && was.ours === item.ours && was.theirs === item.theirs) {
        session.carried[item.path] = resolution;
      }
    }
  }
  const packed = await packResolutions(ctx, session.carried);
  if (packed.commit) await ctx.git.keep(op, { carried: packed.commit });
  await journalSet(ctx, { id: op, kind: "conflict", phase: "conflict", session: { ...session, carried: packed.stored } });
  // Sessions of another connection are kept for when that connection returns.
  for (const entry of same) await finish(ctx, entry.id);
  return { result: result("conflict", { session, conflicts }) };
}

/// The pull loop. Returns `{ result, state? }`; `state` is present when the
/// pull completed, and holds what a push continues from.
async function runPull(ctx, options = {}) {
  let changedTries = 0;
  let staleTries = 0;
  for (;;) {
    const { link, version } = await ctx.api.getLink();
    if (!link) return { result: refused("This project is not connected to a Git repository.") };
    const mismatch = await connectionRefusal(ctx, link);
    if (mismatch) return { result: mismatch };
    const op = refId();
    await journalSet(ctx, { id: op, kind: "pull", phase: "planned", revision: link.revision, checkpoint: link.checkpoint });
    let step;
    let prepared = null;
    try {
      prepared = await prepare(ctx, link, op, options);
      if (prepared.refusal) {
        await finish(ctx, op);
        return { result: prepared.refusal };
      }
      if (prepared.empty) {
        await finish(ctx, op);
        step = {
          result: result("nothing", { files: [], skipped: [], link, arrived: true }),
          state: { filter: prepared.filter, remote: null, theirs: prepared.theirs, link, version },
        };
      } else if (prepared.merge.clean) {
        step = await complete(ctx, prepared, prepared.merge.tree, op);
      } else {
        step = await openConflict(ctx, prepared, op, options.carry);
      }
    } catch (error) {
      // Once the request is sent its outcome may be unknown: only a clear
      // rejection ends the entry. Before that nothing is in flight.
      if (definitive(error) || !prepared?.sent) await finish(ctx, op);
      throw error;
    }
    if (step.retry === "stale") {
      staleTries += 1;
      if (staleTries >= MAX_STALE_TRIES) throw new Error("The connection record kept changing. Try again.");
      continue;
    }
    if (step.retry === "changed") {
      changedTries += 1;
      if (changedTries >= MAX_CHANGED_TRIES) {
        return {
          result: refused("These files changed in LibrePaper while they were being updated.", step.files, {
            files: step.files,
          }),
        };
      }
      continue;
    }
    if (step.result.status === "pulled" || step.result.status === "nothing") await clearStale(ctx, link);
    return step;
  }
}

/// After a pull succeeded for a connection, an earlier pull request whose body
/// was too large to journal is no longer in doubt (a pull is idempotent: if it
/// applied, this merge found nothing to do), and a saved conflict session of
/// the same connection has been consumed.
async function clearStale(ctx, link) {
  for (const entry of await journalRead(ctx)) {
    const stale =
      (entry.kind === "pull" && entry.phase === "sent" && !entry.body && entry.connection_id === link.connection_id) ||
      (entry.kind === "conflict" && entry.session?.connection_id === link.connection_id);
    if (stale) await finish(ctx, entry.id);
  }
}

/// Brings the remote branch's changes into the project. Resolves with
/// `{ status: "pulled" | "nothing" }` (and `files`), `{ status: "conflict",
/// session }` or `{ status: "refused", refusals }`.
export async function pull(ctx) {
  return exclusive(ctx, () => guarded(async () => (await runPull(ctx)).result));
}

/// Stores the typed texts of `resolutions` as the blobs of one unparented
/// commit, so that the journal, which is small, holds only their ids. Answers
/// `{ stored, commit }`: the resolutions with each text replaced by
/// `{ blob: id }`, and the commit to keep (null when there is no text).
async function packResolutions(ctx, resolutions) {
  const stored = {};
  const edits = [];
  const texts = [];
  for (const [path, choice] of Object.entries(resolutions || {})) {
    if (typeof choice?.text === "string") texts.push([path, choice.text]);
    else if (choice?.blob) edits.push({ path, id: choice.blob });
    else stored[path] = choice;
  }
  const ids = texts.length ? await ctx.git.putBlobs(texts.map(([, text]) => ({ text }))) : [];
  texts.forEach(([path], i) => edits.push({ path, id: ids[i] }));
  for (const edit of edits) stored[edit.path] = { blob: edit.id };
  if (!edits.length) return { stored, commit: null };
  const tree = await ctx.git.treeEdit(null, edits);
  return { stored, commit: await ctx.git.commitTree(tree, [], "LibrePaper conflict resolutions", null) };
}

/// The inverse of `packResolutions`: each `{ blob }` read back as `{ text }`,
/// with a leading byte order mark kept.
async function unpackResolutions(ctx, stored) {
  const entries = Object.entries(stored || {});
  const blobs = await readBlobs(ctx, entries.map(([, choice]) => choice?.blob));
  const resolutions = {};
  for (const [path, choice] of entries) {
    if (!choice?.blob) {
      resolutions[path] = choice;
      continue;
    }
    const bytes = blobs.get(choice.blob);
    if (!bytes) throw new Error(`The saved resolution of ${path} is missing.`);
    resolutions[path] = { text: contentDecoder.decode(bytes) };
  }
  return resolutions;
}

/// A saved conflict session with its carried and chosen resolutions read back.
async function restoreSession(ctx, session) {
  if (!session) return null;
  const restored = { ...session, carried: await unpackResolutions(ctx, session.carried) };
  if (session.resolutions) restored.resolutions = await unpackResolutions(ctx, session.resolutions);
  return restored;
}

/// Saves the resolutions chosen so far on the conflict session in the journal,
/// so that a restart restores them. Answers the saved session. Throws when
/// they could not be saved.
export async function saveResolutions(ctx, session, resolutions) {
  return exclusive(ctx, async () => {
    const chosen = await packResolutions(ctx, resolutions);
    const carried = await packResolutions(ctx, session.carried);
    const refs = {};
    if (chosen.commit) refs.resolutions = chosen.commit;
    if (carried.commit) refs.carried = carried.commit;
    if (Object.keys(refs).length) await ctx.git.keep(session.id, refs);
    await journalSet(ctx, {
      id: session.id,
      kind: "conflict",
      phase: "conflict",
      session: { ...session, carried: carried.stored, resolutions: chosen.stored },
    });
    return { ...session, resolutions };
  });
}

/// Applies the user's resolutions to a saved conflict session. `resolutions`
/// maps a path to `{ text }`, `{ keep: "ours" | "theirs" }` or `{ delete: true }`.
/// If the checkpoint or the remote head moved, the merge is made again and the
/// resolutions that still apply come back as `session.carried`. A refusal
/// leaves the session and its resolutions saved and comes back with `session`.
export async function applyResolution(ctx, session, resolutions = undefined) {
  return exclusive(ctx, () => guarded(async () => {
    const link = (await ctx.api.getLink()).link;
    if (!link) return refused("This project is not connected to a Git repository.");
    const mismatch = await connectionRefusal(ctx, link);
    if (mismatch) return mismatch;
    // A session of another connection is not merged into this one, and stays
    // in the journal as it is.
    if (!session) return refused("There is no saved conflict to resolve.");
    if (session.connection_id !== link.connection_id) return refused(RECONNECT, [], { reconnect: true });
    const restored = await restoreSession(ctx, session);
    const chosen = resolutions ? await unpackResolutions(ctx, resolutions) : restored.resolutions || {};
    const kept = { ...restored, resolutions: chosen };
    const again = async () => {
      const answer = (await runPull(ctx, { carry: { session: restored, resolutions: chosen } })).result;
      return answer.status === "refused" ? { ...answer, session: kept } : answer;
    };
    const fetched = await ctx.git.fetch(link.checkpoint ? [link.checkpoint] : []);
    if (link.checkpoint !== session.checkpoint || fetched.head !== session.remote) return again();
    const unresolved = session.conflicts.filter((item) => !chosen[item.path]).map((item) => item.path);
    if (unresolved.length) return result("conflict", { session: kept, conflicts: session.conflicts, unresolved });

    const edits = [];
    for (const item of session.conflicts) {
      const choice = chosen[item.path];
      let id = null;
      if (typeof choice.text === "string") [id] = await ctx.git.putBlobs([{ text: toLf(choice.text) }]);
      else if (choice.keep === "ours") id = item.ours;
      else if (choice.keep === "theirs") id = item.theirs;
      edits.push(id ? { path: item.path, id } : { path: item.path, delete: true });
    }
    const tree = await ctx.git.treeEdit(session.tree, edits);
    const prepared = {
      link,
      remote: session.remote,
      paths: session.paths,
      kinds: new Map(Object.entries(session.kinds)),
      // The project paths of L, read from its commit, for the name collision check.
      local: { commit: session.local, paths: await blobPaths(ctx, session.local) },
      theirs: { commit: session.theirs },
      base: { commit: session.base },
      filter: null,
    };
    // The request gets its own journal entry: the conflict session, with the
    // saved resolutions, stays until the request is known to have applied.
    const step = await complete(ctx, prepared, tree, refId(), session.id);
    if (step.retry) return again();
    if (step.result.status !== "pulled" && step.result.status !== "nothing") {
      return { ...step.result, session: kept };
    }
    await finish(ctx, session.id);
    await clearStale(ctx, link);
    return step.result;
  }));
}

/// Drops a saved conflict session and its chosen resolutions. The pull it came
/// from is abandoned: the project and the link stay as they were before it.
export async function discardConflict(ctx, session) {
  return exclusive(ctx, () => guarded(async () => {
    if (session) await finish(ctx, session.id);
    return result("nothing");
  }));
}

// ---------------------------------------------------------------- push

export function defaultMessage(files) {
  const names = files.slice(0, 3).map((file) => file.path.split("/").pop());
  const more = files.length > 3 ? ` and ${files.length - 3} more` : "";
  return `Update ${names.join(", ")}${more}`;
}

/// Works out what a push would send, without pulling. Resolves with
/// `{ status: "outgoing", files, outgoing }`, where `outgoing` is the prepared
/// push to hand back to `push` as `confirmed`; with `{ status: "nothing" }`;
/// or with `{ status: "behind", head, reason }` when the remote has commits
/// the link's checkpoint does not have, which the person clears by pulling.
/// It writes the journal as a pull does, so it runs under the lock.
export async function outgoing(ctx) {
  return exclusive(ctx, () => planOutgoing(ctx));
}

/// The body of `outgoing`, for the pushes that already hold the lock.
async function planOutgoing(ctx) {
  return guarded(async () => {
    const pulled = await runPull(ctx, { current: true });
    if (!pulled.state) return pulled.result;
    // An arrival timeout is remembered and refuses until the page has caught
    // up with the server, which the version check decides.
    if (pulled.result.arrived === false) timedOut.add(ctx.session);
    const behind = await catchUpRefusal(ctx, pulled.state.version);
    if (behind) return behind;
    const { filter, remote, theirs, link } = pulled.state;
    const local = await snapshot(ctx, filter, theirs.commit);
    // A file over the size limit is in neither direction, so its absence from
    // L is not a deletion to push.
    const skip = new Set(local.oversize);
    const changes = (await ctx.git.diff(theirs.commit, local.commit)).filter((item) => !skip.has(item.path));
    if (!changes.length) return result("nothing", { files: [], link });
    const files = changes.map(({ path, status }) => ({ path, status }));
    const edits = changes.map((item) => {
      const path = filter.synced.get(item.path)?.repoPath ?? filter.rules.repoPath(item.path);
      return item.status === "D" ? { path, delete: true } : { path, id: item.to };
    });
    return result("outgoing", {
      files,
      outgoing: {
        files,
        signature: changes.map((item) => `${item.status}:${item.path}:${item.to || ""}`).join("\n"),
        remote,
        remoteTree: filter.tree,
        edits,
        local: local.commit,
        paths: local.paths,
        link,
        connection_id: link.connection_id,
      },
    });
  });
}

/// Records a push on the server, safely after a crash or a lost reply. Does
/// nothing when the record already names the commit or a descendant, or when
/// it names neither the commit nor its parent: the next pull adopts the push.
/// Throws, leaving the entry for the person, when the record belongs to another
/// connection than the push.
async function recordPush(ctx, entry) {
  const link = (await ctx.api.getLink()).link;
  if (!link) return null;
  if (link.connection_id !== entry.connection_id) {
    throw Object.assign(new Error("The push belongs to another connection."), { otherConnection: true });
  }
  if (link.checkpoint === entry.commit) return link;
  if (link.checkpoint !== entry.parent) {
    let descendant = false;
    if (link.checkpoint) {
      try {
        descendant = await ctx.git.ancestor(entry.commit, link.checkpoint);
      } catch {
        descendant = false;
      }
    }
    return descendant ? link : null;
  }
  try {
    return (await ctx.api.putLink({
      ...linkFields(link),
      revision: link.revision,
      checkpoint: entry.commit,
      synced_paths: entry.paths,
      pushed: true,
    })).link;
  } catch (error) {
    if (error?.status === 409) return null;
    throw error;
  }
}

/// Writes one commit on top of the remote head and pushes it, naming the head
/// that was reviewed and the connection it was reviewed against.
async function publish(ctx, plan, message) {
  const tree = await ctx.git.treeEdit(plan.remoteTree, plan.edits);
  const commit = await ctx.git.commitTree(tree, plan.remote ? [plan.remote] : [], message?.trim() || defaultMessage(plan.files), authorOf(ctx));
  const op = refId();
  const entry = {
    id: op,
    kind: "push",
    phase: "prepared",
    commit,
    parent: plan.remote,
    paths: plan.paths,
    connection_id: plan.connection_id,
  };
  await journalSet(ctx, entry);
  await ctx.git.keep(op, { published: commit });
  try {
    await ctx.git.push(commit, plan.remote ?? null, plan.connection_id);
  } catch (error) {
    // Only a clear rejection ends the entry; a 502 or a lost connection may
    // have pushed the commit, which recovery finds out.
    if (definitive(error)) await finish(ctx, op);
    if (kindOf(error) === "moved") return { moved: true };
    if (kindOf(error) === "stale-connection") return { result: refused(RECONNECT, [], { reconnect: true }) };
    throw error;
  }
  await journalSet(ctx, { ...entry, phase: "pushed" });
  let link;
  try {
    link = await recordPush(ctx, entry);
  } catch (error) {
    if (error?.otherConnection) return { result: result("pushed", { files: plan.files, commit, link: null }) };
    throw error;
  }
  await finish(ctx, op);
  return { result: result("pushed", { files: plan.files, commit, link }) };
}

/// Pushes one commit on top of the remote head the project last synced with.
/// It never pulls: when the remote has commits the checkpoint lacks, the answer
/// is `{ status: "behind" }` and the person pulls first. With `confirmed`, the
/// `outgoing` value the person reviewed, exactly that set is pushed. Without
/// it, the outgoing set is worked out here. When the remote moved during the
/// push, the plan is made again once; if it moved again, or the person reviewed
/// a set that is no longer the outgoing one, the answer is
/// `{ status: "moved", outgoing }` and the panel asks again.
export async function push(ctx, message = "", { confirmed = null } = {}) {
  return exclusive(ctx, () => guarded(async () => {
    let plan = confirmed;
    if (confirmed) {
      const { link, version } = await ctx.api.getLink();
      if (!link) return refused("This project is not connected to a Git repository.");
      const mismatch = await connectionRefusal(ctx, link);
      if (mismatch) return mismatch;
      const behind = await catchUpRefusal(ctx, version);
      if (behind) return behind;
      if (confirmed.connection_id !== link.connection_id) {
        // The review was made against another connection: ask for a new one.
        const again = await planOutgoing(ctx);
        if (again.status !== "outgoing") return again;
        return result("moved", { files: again.files, outgoing: again.outgoing });
      }
    }
    for (let attempt = 0; ; attempt += 1) {
      if (!plan) {
        const planned = await planOutgoing(ctx);
        if (planned.status !== "outgoing") return planned;
        plan = planned.outgoing;
      }
      const sent = await publish(ctx, plan, message);
      if (!sent.moved) return sent.result;
      const again = await planOutgoing(ctx);
      if (again.status !== "outgoing") return again;
      if (attempt >= 1 || (confirmed && again.outgoing.signature !== plan.signature)) {
        return result("moved", { files: again.files, outgoing: again.outgoing });
      }
      plan = again.outgoing;
    }
  }));
}

/// Pushes the outgoing changes as one commit to a new branch of the connected
/// remote, for a pull request. The commit sits on the remote head the project
/// last synced with, as an ordinary push would. Nothing else changes: not the
/// connection, its checkpoint, the journal or the shared record, so the same
/// files stay local changes. With `confirmed`, the reviewed `outgoing` value,
/// exactly that set is pushed. Answers `{ status: "branched", branch, commit,
/// files }`, a refusal when the branch exists, or whatever outgoing answers
/// when there is nothing to push or the remote is ahead.
export async function pushToBranch(ctx, branch, message = "", { confirmed = null } = {}) {
  return exclusive(ctx, () => guarded(async () => {
    const name = String(branch || "").trim();
    if (!name) return refused("Name the branch to push to.");
    let plan = confirmed;
    if (!plan) {
      const planned = await planOutgoing(ctx);
      if (planned.status !== "outgoing") return planned;
      plan = planned.outgoing;
    } else {
      const { link } = await ctx.api.getLink();
      if (!link || link.connection_id !== plan.connection_id) {
        const again = await planOutgoing(ctx);
        if (again.status !== "outgoing") return again;
        return result("moved", { files: again.files, outgoing: again.outgoing });
      }
    }
    const tree = await ctx.git.treeEdit(plan.remoteTree, plan.edits);
    const commit = await ctx.git.commitTree(tree, plan.remote ? [plan.remote] : [], message?.trim() || defaultMessage(plan.files), authorOf(ctx));
    try {
      await ctx.git.pushBranch(commit, name, plan.connection_id);
    } catch (error) {
      if (kindOf(error) === "exists") return refused(`A branch named ${name} already exists on the remote. Choose another name.`, [], { exists: true });
      if (kindOf(error) === "stale-connection") return refused(RECONNECT, [], { reconnect: true });
      // A name the service cannot use: invalid, or the connected branch.
      if (error?.status === 400) return refused(error.body?.error || error.message || "That branch name cannot be used.");
      throw error;
    }
    return result("branched", { branch: name, commit, files: plan.files });
  }));
}

// ---------------------------------------------------------------- connecting

function draftLink(draft) {
  return {
    remote_kind: draft.remote?.grant ? "local" : "network",
    remote_label: draft.label || "",
    connection_id: draft.connection_id,
    branch: draft.branch,
    folder: draft.folder || "",
  };
}

const ALREADY_LINKED = "This project is already connected to a Git repository. Connect this computer from the Git panel instead.";

/// Gives the draft a connection id if it lacks one and tells the sidecar about
/// the remote, but only when the server holds no link yet. The label comes from
/// the sidecar, which has removed any user info from the address. Answers a
/// refusal when a link exists, else null.
async function openDraft(ctx, draft) {
  if ((await ctx.api.getLink()).link) return refused(ALREADY_LINKED);
  if (!draft.connection_id) draft.connection_id = crypto.randomUUID();
  const answer = await ctx.git.connect({ connection_id: draft.connection_id, remote: draft.remote, branch: draft.branch });
  if (answer?.label) draft.label = answer.label;
  else if (!draft.label) draft.label = "";
  return null;
}

/// Runs `work` after the draft is open. If it fails before the shared record
/// exists, the sidecar forgets the draft, so it never keeps pointing at a
/// remote that was not connected. `work` reports success of the record through
/// `mark.recorded`.
async function withDraft(ctx, draft, work) {
  const refusal = await openDraft(ctx, draft);
  if (refusal) return refusal;
  const mark = { recorded: false };
  try {
    return await work(mark);
  } catch (error) {
    if (!mark.recorded) {
      try {
        await ctx.git.forget();
      } catch {
        // The first failure is the one to report.
      }
    }
    throw error;
  }
}

/// Records the connection and nothing else: no pull, no push, no change to the
/// project. The link has no checkpoint, so the first pull merges against an
/// empty base: identical files merge cleanly, one-sided files are kept and
/// files that differ go to the resolver. Refuses when the branch cannot be
/// synced, or when the project is already connected.
export async function connect(ctx, draft) {
  return exclusive(ctx, () => guarded(() => withDraft(ctx, draft, async (mark) => {
    const fetched = await ctx.git.fetch([]);
    const spec = draftLink(draft);
    const filter = await filterAt(ctx, spec, fetched.head || null);
    if (filter.refusals.length) {
      await ctx.git.forget();
      return result("refused", { refusals: filter.refusals });
    }
    const incoming = await loadFiles(ctx, filter.synced, filter.entries, [...filter.synced.keys()]);
    pruneSkipped(filter, incoming.skipped);
    const excess = tooManyFiles(filter);
    if (excess) {
      await ctx.git.forget();
      return result("refused", { refusals: [excess] });
    }
    try {
      await ctx.api.putLink({ ...spec, revision: null, checkpoint: null, synced_paths: [], pushed: false });
    } catch (error) {
      if (error?.status === 409) {
        await ctx.git.forget();
        return refused(ALREADY_LINKED);
      }
      throw error;
    }
    mark.recorded = true;
    return result("connected", { link: (await ctx.api.getLink()).link ?? { ...spec, checkpoint: null } });
  })));
}

/// The project's synced files compared with the remote branch head, whatever
/// was synced before: `{ status: "compared", head, branch, files, same,
/// unsynced }`. Each file is { path, change, ours, theirs } with change
/// "different", "local" (only in LibrePaper) or "remote" (only on the remote),
/// and `ours` and `theirs` the blob ids of LibrePaper's and the remote's copy,
/// null where absent. `same` counts the synced files equal on both sides. It
/// changes nothing in the project, the journal or the shared record.
export async function compareRemote(ctx) {
  return guarded(async () => {
    const { link, answer } = await connectedLink(ctx);
    if (answer) return answer;
    const fetched = await ctx.git.fetch(link.checkpoint ? [link.checkpoint] : []);
    const head = fetched.head || null;
    const filter = await filterAt(ctx, link, head);
    if (filter.refusals.length) return result("refused", { refusals: filter.refusals });
    const incoming = await loadFiles(ctx, filter.synced, filter.entries, [...filter.synced.keys()]);
    pruneSkipped(filter, incoming.skipped);
    const excess = tooManyFiles(filter);
    if (excess) return result("refused", { refusals: [excess] });
    // A file over the size limit is unsynced in both directions.
    const local = await snapshot(ctx, filter, null);
    const oversize = new Set(local.oversize);
    for (const path of oversize) incoming.files.delete(path);
    const theirs = await commitFiles(ctx, incoming.files, null);
    const changes = (await ctx.git.diff(theirs.commit, local.commit)).filter((item) => !oversize.has(item.path));
    const files = changes
      .map((item) => {
        if (item.status === "A") return { path: item.path, change: "local", ours: item.to, theirs: null };
        if (item.status === "D") return { path: item.path, change: "remote", ours: null, theirs: item.from };
        return { path: item.path, change: "different", ours: item.to, theirs: item.from };
      })
      .sort((a, b) => byPath(a.path, b.path));
    const same = incoming.files.size - files.filter((file) => file.change !== "local").length;
    return result("compared", { head, branch: link.branch, files, same, unsynced: filter.unsynced });
  });
}

/// Forgets the shared record and the sidecar's connection. The private
/// repository, its commits and the journal stay.
export async function disconnect(ctx) {
  return exclusive(ctx, async () => {
    await ctx.api.deleteLink();
    await ctx.git.forget();
  });
}

/// Forgets the shared record but keeps this computer's connection, so the same
/// remote can be connected again on another branch. A local folder's grant stays.
export async function leaveBranch(ctx) {
  return exclusive(ctx, async () => {
    await ctx.api.deleteLink();
  });
}

// ---------------------------------------------------------------- state

/// The sidecar's status, the shared record and a summary of what is saved on
/// this computer: `{ git, link, conflict, pending }`.
export async function status(ctx) {
  const [git, answer, entries] = await Promise.all([
    ctx.git.status(),
    ctx.api.getLink(),
    safeJournal(ctx),
  ]);
  // The saved conflict shown is this connection's; another connection's stays
  // in the journal untouched.
  const id = answer.link?.connection_id;
  const mine = entries.filter((entry) => entry.kind === "conflict" && entry.session?.connection_id === id);
  const conflict = await restoreSession(ctx, mine.at(-1)?.session);
  const pending = entries.filter((entry) => entry.kind !== "conflict" && entry.phase !== "planned");
  return { git, link: answer.link, conflict, pending };
}

/// The synced entries of the checkpoint that only their content rules out (an
/// LFS pointer, bytes that are not UTF-8), as a pull finds them, so that a
/// project file over one is not listed as a change. They depend only on the
/// checkpoint, so they are read once per connection and checkpoint.
const skippedCache = new Map();
async function skippedAt(ctx, link, filter) {
  const key = `${link.connection_id}:${link.checkpoint || ""}`;
  if (!skippedCache.has(key)) {
    const loaded = await loadFiles(ctx, filter.synced, filter.entries, [...filter.synced.keys()]);
    skippedCache.set(key, loaded.skipped);
  }
  return skippedCache.get(key);
}

/// The link to work from, or `{ answer }` when there is none this computer is
/// connected to.
async function connectedLink(ctx) {
  const { link } = await ctx.api.getLink();
  if (!link) return { answer: result("unknown", { reason: NOT_LINKED }) };
  const refusal = await connectionRefusal(ctx, link);
  if (refusal) return { answer: result("unknown", { reason: refusal.refusals[0].reason, reconnect: true }) };
  return { link };
}

/// Fetches the branch and says whether the remote moved since the last sync:
/// `{ status: "current" | "behind", head }`, or `"unknown"` with a reason. It
/// changes nothing in the project, the journal or the shared record.
export async function checkRemote(ctx) {
  return guarded(async () => {
    const { link, answer } = await connectedLink(ctx);
    if (answer) return answer;
    const fetched = await ctx.git.fetch(link.checkpoint ? [link.checkpoint] : []);
    if (link.checkpoint && fetched.missing?.includes(link.checkpoint)) {
      return result("unknown", { reason: GONE_FROM_REMOTE });
    }
    const head = fetched.head || null;
    return result(head === (link.checkpoint || null) ? "current" : "behind", { head });
  });
}

/// The project's synced files compared with the last synced state:
/// `{ status: "changes" | "none", files }`, with each file's status "A", "M"
/// or "D". It makes no call to the remote and changes nothing.
export async function localChanges(ctx) {
  return guarded(async () => {
    const { link, answer } = await connectedLink(ctx);
    if (answer) return answer;
    try {
      const filter = await filterAt(ctx, link, link.checkpoint || null);
      if (filter.refusals.length) return result("unknown", { reason: filter.refusals[0].reason });
      pruneSkipped(filter, await skippedAt(ctx, link, filter));
      const oversize = new Set((await snapshot(ctx, filter, null)).oversize);
      const base = await lastSynced(ctx, link, filter, oversize);
      const local = await snapshot(ctx, filter, base.commit);
      const changes = (await ctx.git.diff(base.commit, local.commit)).filter((item) => !oversize.has(item.path));
      return result(changes.length ? "changes" : "none", {
        files: changes.map(({ path, status }) => ({ path, status })),
      });
    } catch (error) {
      if (link.checkpoint && MISSING_KINDS.has(kindOf(error) ?? error?.code)) return result("unknown", { reason: NOT_SYNCED });
      throw error;
    }
  });
}

/// The files the remote branch changed since the last sync:
/// `{ status: "changes" | "none", head, files }`, with each file's status "A",
/// "M" or "D". It changes nothing in the project, the journal or the shared
/// record.
export async function remoteChanges(ctx) {
  return guarded(async () => {
    const { link, answer } = await connectedLink(ctx);
    if (answer) return answer;
    const fetched = await ctx.git.fetch(link.checkpoint ? [link.checkpoint] : []);
    if (link.checkpoint && fetched.missing?.includes(link.checkpoint)) {
      return result("unknown", { reason: GONE_FROM_REMOTE });
    }
    const head = fetched.head || null;
    if (head === (link.checkpoint || null)) return result("none", { head, files: [] });
    const filter = await filterAt(ctx, link, head);
    if (filter.refusals.length) return result("unknown", { reason: filter.refusals[0].reason });
    const incoming = await loadFiles(ctx, filter.synced, filter.entries, [...filter.synced.keys()]);
    pruneSkipped(filter, incoming.skipped);
    // A file over the size limit is unsynced in both directions, as in a pull.
    const oversize = new Set((await snapshot(ctx, filter, null)).oversize);
    for (const path of oversize) incoming.files.delete(path);
    const base = await lastSynced(ctx, link, filter, oversize);
    const theirs = await commitFiles(ctx, incoming.files, base.commit);
    const changes = (await ctx.git.diff(base.commit, theirs.commit)).filter((item) => !oversize.has(item.path));
    return result(changes.length ? "changes" : "none", {
      head,
      files: changes.map(({ path, status }) => ({ path, status })).sort((a, b) => byPath(a.path, b.path)),
    });
  });
}

async function safeJournal(ctx) {
  try {
    return await journalRead(ctx);
  } catch {
    return [];
  }
}

/// Reconciles the journal after a restart. A pull that was sent is sent again
/// with the same operation id; a push that was prepared is checked against the
/// remote. Returns `{ recovered, pending, reasons, conflict, needsPull }`: the
/// ids settled, the entries that could not be settled yet, why, a saved
/// conflict session, and whether only a fresh pull can settle what is left
/// (every pending entry is an interrupted pull too large to send again).
export async function recover(ctx) {
  return exclusive(ctx, () => guarded(async () => {
    const recovered = [];
    const pending = [];
    const reasons = {};
    const oversized = new Set();
    let conflict = null;
    // Only this connection's conflict is offered; another connection's stays
    // in the journal untouched.
    let connection = null;
    try {
      connection = (await ctx.api.getLink()).link?.connection_id ?? null;
    } catch {
      connection = null;
    }
    const entries = await journalRead(ctx);
    // Requests come first: one that applied finishes the conflict session it
    // resolved, and that session is then not offered.
    const ordered = [...entries.filter((entry) => entry.kind !== "conflict"), ...entries.filter((entry) => entry.kind === "conflict")];
    const resolved = new Set();
    for (const entry of ordered) {
      if (entry.kind === "conflict") {
        if (!resolved.has(entry.id) && connection != null && entry.session?.connection_id === connection) {
          conflict = await restoreSession(ctx, entry.session);
        }
        continue;
      }
      try {
        if (entry.kind === "pull" && entry.phase === "sent" && !entry.body) {
          // The request was too large to journal, so it cannot be sent again
          // and nobody knows whether it applied. The next pull settles it.
          oversized.add(entry.id);
          reasons[entry.id] = "run Pull again to finish an interrupted pull";
          throw new Error(reasons[entry.id]);
        } else if (entry.phase === "planned" || entry.phase === "done") {
          await finish(ctx, entry.id);
        } else if (entry.kind === "pull") {
          // Only an answer that says the request was not applied (or applied it)
          // settles the entry. A server error or a lost connection leaves it.
          try {
            await ctx.api.sourceChange(entry.body);
            // The request applied, so the conflict it resolved is over.
            if (entry.resolves) {
              await finish(ctx, entry.resolves);
              resolved.add(entry.resolves);
            }
          } catch (error) {
            if (!definitive(error)) throw error;
            // Not applied: the conflict session stays and can be resumed.
          }
          await finish(ctx, entry.id);
        } else if (entry.phase === "prepared") {
          // An entry of another connection stays as it is: the connected
          // remote says nothing about its push.
          const link = (await ctx.api.getLink()).link;
          const connected = (await ctx.git.status())?.connection_id;
          if ((link && link.connection_id !== entry.connection_id) || connected !== entry.connection_id) {
            throw Object.assign(new Error("The push belongs to another connection."), { otherConnection: true });
          }
          const fetched = await ctx.git.fetch([entry.commit]);
          const published = fetched.head && (fetched.head === entry.commit || await ctx.git.ancestor(entry.commit, fetched.head));
          if (published) await recordPush(ctx, entry);
          await finish(ctx, entry.id);
        } else {
          await recordPush(ctx, entry);
          await finish(ctx, entry.id);
        }
        recovered.push(entry.id);
      } catch {
        pending.push(entry.id);
      }
    }
    const needsPull = pending.length > 0 && pending.every((id) => oversized.has(id));
    return { recovered, pending, reasons, conflict, needsPull };
  }));
}
