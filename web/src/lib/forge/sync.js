import { unsuggestedText } from "../track-changes.js";
import { MovedError } from "./github.js";
import { sha256Hex } from "../digest.js";
import { decodeText, isLfsPointer, textHash, toLf } from "./hash.js";
import { planPull, planPush } from "./plan.js";
import { syncRules } from "./rules.js";
import * as defaultStore from "./store.js";

// Pull, push and export, driven from the browser. Each call reads the link row
// from the server, talks to GitHub as the connected user and writes the result
// back with the row's revision, so two tabs cannot record baselines out of
// order.
//
// `ctx` is { slug, session, rules, github, api, me, store?, maxFileBytes?, web? }.
// `api` is { getLink, putLink, sourceChange, uploadAsset, assetBytes }; each
// returns the parsed JSON, or throws an error with `status` and `body` so a
// 409 can be read. `me` is { name, handle } of the person pressing the button.

const MAX_TRIES = 3;
const BLOB_PARALLEL = 6;
const DEFAULT_MAX_FILE_BYTES = 10 * 1024 * 1024;
const IGNORE_FILE = ".librepaperignore";
const encoder = new TextEncoder();

// The ignore files, by repository and blob id, so a pull fetches one only
// when the file changed. The cache lives as long as the page.
const ignoreCache = new Map();

const answered = (error) => typeof error?.status === "number";

function operationId() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `v2.${Date.now()}.${hex}`;
}

async function mapLimit(items, limit, action) {
  const out = new Array(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const index = next;
      next += 1;
      out[index] = await action(items[index], index);
    }
  });
  await Promise.all(workers);
  return out;
}

function changeFiles(change) {
  const paths = new Set();
  for (const list of [change?.patches, change?.create, change?.delete, change?.put_binary]) {
    for (const item of list || []) paths.add(item.path);
  }
  return paths.size;
}

function result(status, extra = {}) {
  return { status, files: 0, conflicts: [], refusals: [], skipped: [], link: null, ...extra };
}

async function requireLink(ctx) {
  const link = (await ctx.api.getLink())?.link ?? null;
  if (!link) throw new Error("This project is not connected to a repository.");
  return link;
}

async function ignoreText(ctx, link, head, byPath, path) {
  const entry = byPath.get(path);
  if (!entry) return null;
  const key = `${link.repository}@${entry.sha}`;
  if (!ignoreCache.has(key)) ignoreCache.set(key, await ctx.github.textFile(link.repository, head, path));
  return ignoreCache.get(key);
}

// The listing at a commit, run through the sync rules. Returns null in `rules`
// when GitHub truncated the listing.
async function inventory(ctx, link, commit) {
  const { entries, truncated, tree } = await ctx.github.treeOf(link.repository, commit);
  if (truncated) return { truncated, entries, tree, rules: null };
  const byPath = new Map(entries.filter((entry) => entry.type === "blob").map((entry) => [entry.path, entry]));
  const folderIgnore = link.folder ? `${link.folder}/${IGNORE_FILE}` : null;
  const rules = syncRules({
    rules: ctx.rules,
    folder: link.folder,
    maxFileBytes: ctx.maxFileBytes ?? DEFAULT_MAX_FILE_BYTES,
    ignoreRoot: await ignoreText(ctx, link, commit, byPath, IGNORE_FILE),
    ignoreFolder: folderIgnore ? await ignoreText(ctx, link, commit, byPath, folderIgnore) : null,
  });
  return { truncated, entries, tree, rules };
}

// The project as plain data: text without pending suggested insertions, and
// asset digests, for the synced paths only.
async function snapshot(ctx, isSynced) {
  const project = new Map();
  for (const file of ctx.session.list()) {
    if (!isSynced(file.path)) continue;
    if (file.kind === "text") {
      const text = unsuggestedText(ctx.session.doc, file.id);
      project.set(file.path, { kind: "text", text, hash: await textHash(text) });
    } else {
      project.set(file.path, { kind: "asset", sha: file.sha });
    }
  }
  return project;
}

const tooLarge = () => result("refused", {
  refusals: [{ reason: "the repository is too large to compare", paths: [] }],
});

/// Brings GitHub's changes into the project.
export async function pull(ctx) {
  const store = ctx.store || defaultStore;
  let replayed = 0;
  const pending = await store.pendingOperation(ctx.slug);
  if (pending) {
    // An earlier request was sent and never answered. The same operation id
    // gets the first receipt back if the server did apply it.
    try {
      await ctx.api.sourceChange(pending.body);
      replayed = changeFiles(pending.body.change);
    } catch (error) {
      if (!answered(error)) throw error;
    }
    await store.settleOperation(ctx.slug);
  }

  let changedTries = 0;
  let changedFiles = [];
  for (let staleTries = 0; staleTries < MAX_TRIES + 2; staleTries += 1) {
    const step = await pullOnce(ctx, store);
    if (!step.retry) {
      if (step.status === "nothing" && replayed) return { ...step, status: "pulled", files: replayed };
      return step;
    }
    if (step.retry === "changed") {
      changedTries += 1;
      changedFiles = step.files;
      // Someone kept typing in a touched file. Show the files after three tries.
      if (changedTries >= MAX_TRIES) {
        return result("refused", { refusals: [{ reason: "changed", paths: changedFiles }] });
      }
    }
  }
  throw new Error("The link kept changing during the pull. Try again.");
}

async function pullOnce(ctx, store) {
  const link = await requireLink(ctx);
  const repo = link.repository;
  const head = await ctx.github.head(repo, link.branch);
  if (head === link.synced_commit) return result("nothing", { link });

  const listing = await inventory(ctx, link, head);
  if (listing.truncated) return tooLarge();
  const classified = listing.rules.classify(listing.entries);
  if (classified.refusals.length) return result("refused", { refusals: classified.refusals, link });

  const project = await snapshot(ctx, listing.rules.isSynced);
  let exportMerged = false;
  if (link.export?.commit) {
    const status = await ctx.github.compare(repo, link.export.commit, head);
    exportMerged = status === "ahead" || status === "identical";
  }
  const plan = await planPull({
    link,
    synced: classified.synced,
    project,
    readBlob: (sha) => ctx.github.blob(repo, sha),
    exportMerged,
    isSynced: listing.rules.isSynced,
  });
  if (plan.refusals.length) {
    return result("refused", { refusals: plan.refusals, skipped: plan.skipped, link });
  }
  if (plan.conflicts.length) {
    return result("conflict", { conflicts: plan.conflicts, skipped: plan.skipped, link });
  }

  for (const upload of plan.uploads) {
    const stored = await ctx.api.uploadAsset(upload.bytes);
    const planned = plan.change.put_binary?.find((item) => item.path === upload.path);
    if (planned && stored?.sha && planned.hash !== stored.sha) {
      throw new Error(`The server stored ${upload.path} under a different hash than planned.`);
    }
  }

  const body = {
    operation: operationId(),
    provider: "github",
    change: plan.change,
    sync: {
      revision: link.revision,
      synced_commit: head,
      synced_map: plan.syncedMap,
      export_consumed: plan.exportConsumed,
    },
  };
  await store.rememberOperation(ctx.slug, { operation: body.operation, body });
  let answer;
  try {
    answer = await ctx.api.sourceChange(body);
  } catch (error) {
    if (!answered(error)) throw error;
    await store.settleOperation(ctx.slug);
    const reply = error.body || {};
    if (error.status === 409 && reply.reason === "stale") return { retry: "stale" };
    if (error.status === 409 && reply.reason === "changed") return { retry: "changed", files: reply.files || [] };
    if (error.status === 409 && reply.reason === "suggestion") {
      return result("refused", {
        refusals: [{ reason: "suggestion", paths: [reply.path], change: reply.change }],
        skipped: plan.skipped,
        link,
      });
    }
    throw error;
  }
  await store.settleOperation(ctx.slug);
  const arrived = await untilArrived(ctx, listing.rules.isSynced, plan);
  return result("pulled", { files: changeFiles(plan.change), skipped: plan.skipped, link: answer.link, arrived });
}

const ARRIVAL_MS = 15_000;

// Waits until this tab's document shows what the server applied: every
// written path holds the content the sync map records, and every deleted path
// is gone. The change travels back over the socket like a coauthor's edit, so
// it lands a moment after the server answers. Answers false after a bound,
// which happens when someone types in a pulled file meanwhile.
async function untilArrived(ctx, isSynced, plan) {
  const written = [...plan.change.create, ...plan.change.patches, ...plan.change.put_binary].map((item) => item.path);
  const deleted = plan.change.delete.map((item) => item.path);
  if (!written.length && !deleted.length) return true;
  const deadline = Date.now() + (ctx.arrivalMs ?? ARRIVAL_MS);
  for (;;) {
    const project = await snapshot(ctx, isSynced);
    const landed = written.every((path) => {
      const file = project.get(path);
      const hash = file ? (file.kind === "text" ? file.hash : file.sha) : null;
      return hash === plan.syncedMap[path]?.hash;
    }) && deleted.every((path) => !project.has(path));
    if (landed) return true;
    if (Date.now() > deadline) return false;
    await new Promise((done) => setTimeout(done, 100));
  }
}

function authorOf(ctx) {
  const handle = ctx.me?.handle || "librepaper";
  return { name: ctx.me?.name || handle, email: `${handle}@users.noreply.${location.hostname}` };
}

async function bytesOf(ctx, item) {
  if (item.kind === "text") return encoder.encode(toLf(item.text));
  return ctx.api.assetBytes(item.sha);
}

// Writes the blobs and the tree for a change set on top of a commit's tree.
// Returns the new root tree and, per changed path, the blob id GitHub made.
async function writeTree(ctx, link, listing, plan) {
  const repo = link.repository;
  const modes = new Map(listing.entries.map((entry) => [entry.path, entry.mode]));
  const blobs = await mapLimit(plan.changed, BLOB_PARALLEL, async (item) =>
    ctx.github.createBlob(repo, await bytesOf(ctx, item)));
  const entries = [];
  plan.changed.forEach((item, index) => {
    const path = listing.rules.repoPath(item.path);
    entries.push({ path, mode: modes.get(path) || "100644", type: "blob", sha: blobs[index] });
  });
  for (const path of plan.deleted) {
    const repoPath = listing.rules.repoPath(path);
    entries.push({ path: repoPath, mode: modes.get(repoPath) || "100644", type: "blob", sha: null });
  }
  const tree = await ctx.github.createTree(repo, listing.tree, entries);
  return { tree, blobs };
}

// The map after a change set: unchanged entries stay, deleted ones go, changed
// ones take GitHub's blob id and the hash the browser computed.
function nextMap(link, plan, blobs) {
  const map = { ...link.synced_map };
  for (const path of plan.deleted) delete map[path];
  plan.changed.forEach((item, index) => {
    map[item.path] = { blob: blobs[index], hash: item.hash };
  });
  return map;
}

function linkBody(link, extra) {
  return {
    revision: link.revision,
    provider: link.provider,
    repository_id: link.repository_id,
    repository: link.repository,
    branch: link.branch,
    folder: link.folder,
    synced_commit: link.synced_commit,
    synced_map: link.synced_map,
    export: link.export ?? null,
    pushed: false,
    ...extra,
  };
}

const namesOf = (plan) => [...plan.changed.map((item) => item.path), ...plan.deleted].join(", ");

/// Sends the project's changes to the connected branch. `message` defaults to
/// the changed file names.
export async function push(ctx, message) {
  let moved = false;
  for (let round = 0; round < MAX_TRIES + 2; round += 1) {
    const link = await requireLink(ctx);
    const repo = link.repository;
    const head = await ctx.github.head(repo, link.branch);
    if (head !== link.synced_commit) {
      const pulled = await pull(ctx);
      if (pulled.status === "refused" || pulled.status === "conflict") return pulled;
      // A push planned before the pull reached this tab would read the pulled
      // files as deleted here and delete them on GitHub.
      if (pulled.status === "pulled" && pulled.arrived === false) {
        return result("refused", {
          refusals: [{ reason: "the pulled changes have not reached this tab yet; push again", paths: [] }],
          link: pulled.link,
        });
      }
      continue;
    }

    const listing = await inventory(ctx, link, head);
    if (listing.truncated) return tooLarge();
    const plan = planPush({ link, project: await snapshot(ctx, listing.rules.isSynced), isSynced: listing.rules.isSynced });
    if (!plan.changed.length && !plan.deleted.length) return result("nothing", { link });

    const { tree, blobs } = await writeTree(ctx, link, listing, plan);
    const commit = await ctx.github.createCommit(repo, {
      message: message || namesOf(plan),
      tree,
      parents: [head],
      author: authorOf(ctx),
    });
    try {
      await ctx.github.moveBranch(repo, link.branch, commit);
    } catch (error) {
      // The branch moved after it was read. Start over once from the read.
      if (error instanceof MovedError && !moved) {
        moved = true;
        continue;
      }
      throw error;
    }

    try {
      const answer = await ctx.api.putLink(linkBody(link, {
        synced_commit: commit,
        synced_map: nextMap(link, plan, blobs),
        pushed: true,
      }));
      return result("pushed", { files: plan.changed.length + plan.deleted.length, commit, link: answer.link });
    } catch (error) {
      if (error?.status !== 409) throw error;
      const current = error.body?.link;
      // Another tab recorded the same push.
      if (current?.synced_commit === commit) {
        return result("pushed", { files: plan.changed.length + plan.deleted.length, commit, link: current });
      }
    }
  }
  throw new Error("The link kept changing while recording the push. Try again.");
}

const two = (n) => String(n).padStart(2, "0");

function branchStamp(date = new Date()) {
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())}-${two(date.getHours())}${two(date.getMinutes())}`;
}

/// The page on GitHub that opens the pull-request form for an export branch.
export function compareLink(link, exportBranch, web = "https://github.com") {
  return `${String(web).replace(/\/+$/, "")}/${link.repository}/compare/${link.branch}...${exportBranch}`;
}

/// Writes a branch `librepaper/<date>` holding the project's changes since the
/// synced commit, and records it as the outstanding export.
export async function exportChanges(ctx) {
  for (let round = 0; round < MAX_TRIES; round += 1) {
    const link = await requireLink(ctx);
    if (!link.synced_commit) throw new Error("There is no synced commit to export from.");
    const repo = link.repository;
    const listing = await inventory(ctx, link, link.synced_commit);
    if (listing.truncated) return tooLarge();
    const plan = planPush({ link, project: await snapshot(ctx, listing.rules.isSynced), isSynced: listing.rules.isSynced });
    if (!plan.changed.length && !plan.deleted.length) return result("nothing", { link });

    const { tree, blobs } = await writeTree(ctx, link, listing, plan);
    const commit = await ctx.github.createCommit(repo, {
      message: "LibrePaper changes",
      tree,
      parents: [link.synced_commit],
      author: authorOf(ctx),
    });
    const branch = `librepaper/${branchStamp()}`;
    await ctx.github.createBranch(repo, branch, commit);

    try {
      const answer = await ctx.api.putLink(linkBody(link, {
        export: { branch, commit, map: nextMap(link, plan, blobs) },
      }));
      return result("exported", {
        files: plan.changed.length + plan.deleted.length,
        branch,
        commit,
        url: compareLink(link, branch, ctx.web),
        link: answer.link,
      });
    } catch (error) {
      if (error?.status !== 409) throw error;
    }
  }
  throw new Error("The link kept changing while recording the export. Try again.");
}

/// Connects the project to a repository, with nothing recorded as synced.
export async function connectLink(ctx, { repositoryId, repository, branch, folder }) {
  const answer = await ctx.api.putLink({
    revision: null,
    provider: "github",
    repository_id: repositoryId,
    repository,
    branch,
    folder: folder || "",
    synced_commit: null,
    synced_map: {},
    export: null,
    pushed: false,
  });
  return answer.link;
}

/// The files GitHub has that the repository rules leave out, for the panel's
/// list. `link` defaults to the stored link.
export async function unsyncedFiles(ctx, link = null) {
  const current = link || await requireLink(ctx);
  const head = await ctx.github.head(current.repository, current.branch);
  const listing = await inventory(ctx, current, head);
  if (listing.truncated) throw new Error("The repository is too large to list.");
  return listing.rules.classify(listing.entries).unsynced;
}

/// The first comparison, before anything is recorded. `link` is a link-like
/// object { repository, branch, folder }; it defaults to `ctx.link`. Files on
/// both sides are compared by content, which needs the GitHub blob.
export async function compareFirst(ctx, link = ctx.link) {
  const head = await ctx.github.head(link.repository, link.branch);
  const listing = await inventory(ctx, link, head);
  if (listing.truncated) throw new Error("The repository is too large to compare.");
  const classified = listing.rules.classify(listing.entries);
  const project = await snapshot(ctx, listing.rules.isSynced);
  const unsynced = [...classified.unsynced];

  const incoming = [];
  const outgoing = [];
  const both = [];
  for (const path of classified.synced.keys()) {
    if (project.has(path)) both.push(path);
    else incoming.push(path);
  }
  for (const path of project.keys()) {
    if (!classified.synced.has(path)) outgoing.push(path);
  }

  const differing = [];
  const verdicts = await mapLimit(both, BLOB_PARALLEL, async (path) => {
    const listed = classified.synced.get(path);
    const mine = project.get(path);
    const bytes = await ctx.github.blob(link.repository, listed.blob);
    if (mine.kind === "asset") {
      return listed.kind === "asset" && (await sha256Hex(bytes)) === mine.sha ? "same" : "differs";
    }
    if (listed.kind !== "text") return "differs";
    if (isLfsPointer(bytes)) return "skip";
    const decoded = decodeText(bytes);
    if (!decoded) return "skip";
    return (await textHash(decoded.text)) === mine.hash ? "same" : "differs";
  });
  both.forEach((path, index) => {
    if (verdicts[index] === "differs") differing.push(path);
    if (verdicts[index] === "skip") unsynced.push({ path, reason: "not readable as text" });
  });

  return {
    incoming: incoming.sort(),
    outgoing: outgoing.sort(),
    differing: differing.sort(),
    folders: [...classified.folders.entries()].sort(([a], [b]) => (a < b ? -1 : 1)),
    unsynced,
    refusals: classified.refusals,
  };
}

/// The first push (or any push). Before the first sync the files on both
/// sides are marked with a hash no content has, so they are overwritten,
/// and files only on GitHub are left alone.
export async function firstPush(ctx, message) {
  const link = await requireLink(ctx);
  if (link.synced_commit) return push(ctx, message);
  const head = await ctx.github.head(link.repository, link.branch);
  const listing = await inventory(ctx, link, head);
  if (listing.truncated) return tooLarge();
  const classified = listing.rules.classify(listing.entries);
  if (classified.refusals.length) return result("refused", { refusals: classified.refusals, link });
  const project = await snapshot(ctx, listing.rules.isSynced);
  const map = {};
  for (const [path, listed] of classified.synced) {
    if (project.has(path)) map[path] = { blob: listed.blob, hash: "0".repeat(64) };
  }
  await ctx.api.putLink(linkBody(link, { synced_commit: head, synced_map: map }));
  return push(ctx, message);
}
