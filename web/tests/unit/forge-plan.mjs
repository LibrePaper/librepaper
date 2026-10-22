// The pull and push planners, on fixtures. Nothing here touches a network: the
// listing, the blobs, the sync map and the project are built in memory.

import assert from "node:assert/strict";

import { sha256Hex, sha256HexOfText } from "../../src/lib/digest.js";
import { gitBlobId, textHash } from "../../src/lib/forge/hash.js";
import { planPull, planPush } from "../../src/lib/forge/plan.js";
import { syncRules } from "../../src/lib/forge/rules.js";

const rules = {
  text_extensions: [".md", ".qmd", ".csv"],
  asset_extensions: [".png"],
  derived_extensions: [],
  max_path: 40,
  max_files: 5,
};

const enc = new TextEncoder();
const dec = new TextDecoder();
const bytesOf = (content) => (typeof content === "string" ? enc.encode(content) : content);
const isAsset = (path) => path.endsWith(".png");

// A listing, as `classify` returns it, plus the blobs behind it.
async function github(files) {
  const synced = new Map();
  const blobs = new Map();
  for (const [path, content] of Object.entries(files)) {
    const bytes = bytesOf(content);
    const blob = await gitBlobId(bytes);
    blobs.set(blob, bytes);
    synced.set(path, {
      repoPath: path,
      blob,
      size: bytes.length,
      kind: isAsset(path) ? "asset" : "text",
    });
  }
  const readBlob = async (sha) => {
    assert.ok(blobs.has(sha), `unexpected blob read ${sha}`);
    return blobs.get(sha);
  };
  return { synced, readBlob };
}

async function projectOf(files) {
  const project = new Map();
  for (const [path, content] of Object.entries(files)) {
    if (isAsset(path)) project.set(path, { kind: "asset", sha: await sha256Hex(bytesOf(content)) });
    else project.set(path, { kind: "text", text: content, hash: await textHash(content) });
  }
  return project;
}

// The sync map after both sides held `files`.
async function mapOf(files) {
  const map = {};
  for (const [path, content] of Object.entries(files)) {
    const bytes = bytesOf(content);
    map[path] = {
      blob: await gitBlobId(bytes),
      hash: isAsset(path) ? await sha256Hex(bytes) : await textHash(content),
    };
  }
  return map;
}

// Runs a plan for a world where the last sync held `base`, GitHub holds `gh`
// and the project holds `lp`.
async function plan({ base, gh, lp, exportMerged = false, exported = null, isSynced }) {
  const listing = await github(gh);
  const link = {
    synced_map: await mapOf(base),
    export: exported
      ? { branch: "librepaper/2026-10-22", commit: "e1", map: await mapOf(exported) }
      : null,
  };
  return planPull({
    link,
    synced: listing.synced,
    project: await projectOf(lp),
    readBlob: listing.readBlob,
    exportMerged,
    isSynced,
  });
}

// The text after a plan's patches, applied at their UTF-8 offsets, checking
// each patch's `exact` on the way.
function applied(text, patches) {
  let bytes = enc.encode(text);
  for (const patch of [...patches].sort((a, b) => b.start - a.start)) {
    const exact = dec.decode(bytes.slice(patch.start, patch.end));
    assert.equal(exact, patch.exact);
    const joined = new Uint8Array([
      ...bytes.slice(0, patch.start),
      ...enc.encode(patch.replacement),
      ...bytes.slice(patch.end),
    ]);
    bytes = joined;
  }
  return dec.decode(bytes);
}

const emptyChange = { dependencies: [], patches: [], create: [], delete: [], put_binary: [] };

// Changed on GitHub only: patches, one dependency on the exact project text,
// and the map moves to the new blob.
{
  const base = "# Title\n\nSome text.\n";
  const next = "# Title\n\nSome longer text.\nMore.\n";
  const result = await plan({ base: { "a.md": base }, gh: { "a.md": next }, lp: { "a.md": base } });
  assert.deepEqual(result.conflicts, []);
  assert.ok(result.change.patches.length >= 1);
  assert.equal(applied(base, result.change.patches), next);
  assert.deepEqual(result.change.dependencies, [
    { path: "a.md", file_hash: await sha256HexOfText(base), start: 0, end: 0, exact: "" },
  ]);
  assert.deepEqual(result.syncedMap, await mapOf({ "a.md": next }));
  assert.equal(result.exportConsumed, false);
  assert.deepEqual(result.uploads, []);
}

// UTF-8 byte offsets, past two-byte and four-byte characters.
{
  const base = "café 😀 end\n";
  const next = "café 😀 fin\n";
  const result = await plan({ base: { "a.md": base }, gh: { "a.md": next }, lp: { "a.md": base } });
  assert.equal(applied(base, result.change.patches), next);
  assert.ok(result.change.patches.every((patch) => patch.start >= 4 + 1 + 4 + 1 - 1));
  const swapped = await plan({
    base: { "a.md": "x😀y" },
    gh: { "a.md": "x😁y" },
    lp: { "a.md": "x😀y" },
  });
  assert.equal(applied("x😀y", swapped.change.patches), "x😁y");
}

// Changed in LibrePaper only: nothing to pull, the map entry stays.
{
  const result = await plan({
    base: { "a.md": "one\n" },
    gh: { "a.md": "one\n" },
    lp: { "a.md": "one\ntwo\n" },
  });
  assert.deepEqual(result.conflicts, []);
  assert.deepEqual(result.change, emptyChange);
  assert.deepEqual(result.syncedMap, await mapOf({ "a.md": "one\n" }));
}

// Both changed to the same content: nothing to apply, the map moves.
{
  const result = await plan({
    base: { "a.md": "one\n" },
    gh: { "a.md": "two\n" },
    lp: { "a.md": "two\n" },
  });
  assert.deepEqual(result.conflicts, []);
  assert.deepEqual(result.change, emptyChange);
  assert.deepEqual(result.syncedMap, await mapOf({ "a.md": "two\n" }));
}

// Both changed to different content: a conflict, and nothing to send.
{
  const result = await plan({
    base: { "a.md": "one\n", "b.md": "b\n" },
    gh: { "a.md": "two\n", "b.md": "b2\n" },
    lp: { "a.md": "three\n", "b.md": "b\n" },
  });
  assert.deepEqual(result.conflicts, ["a.md"]);
  assert.deepEqual(result.change, emptyChange);
  assert.deepEqual(result.uploads, []);
  assert.equal(result.exportConsumed, false);
}

// Added on GitHub: created. Added in LibrePaper: left for push. Added on both
// sides with the same text: recorded, with different text: a conflict.
{
  const result = await plan({
    base: {},
    gh: { "new.md": "fresh\n", "same.md": "s\n" },
    lp: { "mine.md": "mine\n", "same.md": "s\n" },
  });
  assert.deepEqual(result.conflicts, []);
  assert.deepEqual(result.change.create, [{ path: "new.md", text: "fresh\n" }]);
  assert.deepEqual(Object.keys(result.syncedMap).sort(), ["new.md", "same.md"]);
  const clash = await plan({ base: {}, gh: { "x.md": "a\n" }, lp: { "x.md": "b\n" } });
  assert.deepEqual(clash.conflicts, ["x.md"]);
}

// Deleted on one side.
{
  const base = { "a.md": "a\n", "b.md": "b\n" };
  const onGitHub = await plan({ base, gh: { "b.md": "b\n" }, lp: base });
  assert.deepEqual(onGitHub.change.delete, [{ path: "a.md" }]);
  assert.deepEqual(onGitHub.change.dependencies, [
    { path: "a.md", file_hash: await sha256HexOfText("a\n"), start: 0, end: 0, exact: "" },
  ]);
  assert.deepEqual(Object.keys(onGitHub.syncedMap), ["b.md"]);

  const inProject = await plan({ base, gh: base, lp: { "b.md": "b\n" } });
  assert.deepEqual(inProject.change, emptyChange);
  assert.deepEqual(Object.keys(inProject.syncedMap).sort(), ["a.md", "b.md"]);

  const both = await plan({ base, gh: { "b.md": "b\n" }, lp: { "b.md": "b\n" } });
  assert.deepEqual(both.conflicts, []);
  assert.deepEqual(both.change, emptyChange);
  assert.deepEqual(Object.keys(both.syncedMap), ["b.md"]);
}

// Deleted on one side and edited on the other.
{
  const base = { "a.md": "a\n" };
  const goneOnGitHub = await plan({ base, gh: {}, lp: { "a.md": "edited\n" } });
  assert.deepEqual(goneOnGitHub.conflicts, ["a.md"]);
  const goneHere = await plan({ base, gh: { "a.md": "edited\n" }, lp: {} });
  assert.deepEqual(goneHere.conflicts, ["a.md"]);
}

// A CRLF file edited only on GitHub is not a conflict, and the pulled text is LF.
{
  const lf = "a\nb\n";
  const crlf = "a\r\nb\r\n";
  const base = { "a.md": crlf };
  const project = { "a.md": lf };
  const edited = await plan({ base, gh: { "a.md": "a\r\nb\r\nc\r\n" }, lp: project });
  assert.deepEqual(edited.conflicts, []);
  assert.equal(applied(lf, edited.change.patches), "a\nb\nc\n");
  assert.equal(edited.syncedMap["a.md"].hash, await textHash("a\nb\nc\n"));

  // A new CRLF file arrives as LF text.
  const added = await plan({ base: {}, gh: { "n.md": crlf }, lp: {} });
  assert.deepEqual(added.change.create, [{ path: "n.md", text: lf }]);

  // Only the line endings changed on GitHub: the blob moves, the content does not.
  const converted = await plan({ base: { "a.md": lf }, gh: { "a.md": crlf }, lp: project });
  assert.deepEqual(converted.conflicts, []);
  assert.deepEqual(converted.change, emptyChange);
  assert.equal(converted.syncedMap["a.md"].blob, await gitBlobId(enc.encode(crlf)));

  // A CRLF file edited in both places to the same LF content is identical.
  const same = await plan({ base, gh: { "a.md": "a\r\nZ\r\n" }, lp: { "a.md": "a\nZ\n" } });
  assert.deepEqual(same.conflicts, []);
  assert.deepEqual(same.change, emptyChange);
}

// An LFS pointer and a non-UTF-8 text file are skipped and listed, and their
// map entries stay as they were.
{
  const pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:ab\nsize 3\n";
  const bad = new Uint8Array([0x61, 0xff, 0xfe]);
  const result = await plan({
    base: { "a.md": "a\n" },
    gh: { "a.md": pointer, "b.md": bad, "c.png": pointer },
    lp: { "a.md": "a\n" },
  });
  assert.deepEqual(result.conflicts, []);
  assert.deepEqual(result.skipped, [
    { path: "a.md", reason: "a Git LFS pointer" },
    { path: "b.md", reason: "not valid UTF-8" },
    { path: "c.png", reason: "a Git LFS pointer" },
  ]);
  assert.deepEqual(result.change, emptyChange);
  assert.deepEqual(result.syncedMap, await mapOf({ "a.md": "a\n" }));
}

// Binaries: new, replaced with expect, conflicting, deleted with expect.
{
  const v1 = new Uint8Array([1, 2, 3]);
  const v2 = new Uint8Array([4, 5, 6, 7]);
  const added = await plan({ base: {}, gh: { "f.png": v1 }, lp: {} });
  assert.deepEqual(added.change.put_binary, [{ path: "f.png", hash: await sha256Hex(v1), expect: null }]);
  assert.equal(added.uploads.length, 1);
  assert.equal(added.uploads[0].path, "f.png");
  assert.deepEqual([...added.uploads[0].bytes], [1, 2, 3]);
  assert.deepEqual(added.syncedMap["f.png"].hash, await sha256Hex(v1));

  const replaced = await plan({ base: { "f.png": v1 }, gh: { "f.png": v2 }, lp: { "f.png": v1 } });
  assert.deepEqual(replaced.change.put_binary, [
    { path: "f.png", hash: await sha256Hex(v2), expect: await sha256Hex(v1) },
  ]);

  const clash = await plan({
    base: { "f.png": v1 },
    gh: { "f.png": v2 },
    lp: { "f.png": new Uint8Array([9]) },
  });
  assert.deepEqual(clash.conflicts, ["f.png"]);

  const gone = await plan({ base: { "f.png": v1 }, gh: {}, lp: { "f.png": v1 } });
  assert.deepEqual(gone.change.delete, [{ path: "f.png", expect: await sha256Hex(v1) }]);
  assert.deepEqual(gone.change.dependencies, []);
}

// The rules decide what reaches the planner: oversize, outside the folder,
// ignored, re-included with `!`, added inside an ignored folder, a collision
// pair, an overlong path and too many files.
{
  const entry = (path, size = 5) => ({ path, mode: "100644", type: "blob", sha: `s-${path}`, size });
  const set = syncRules({
    rules,
    folder: "paper",
    maxFileBytes: 100,
    ignoreRoot: "paper/data/*\n!paper/data/keep.csv\npaper/scratch/\n",
    ignoreFolder: null,
  });
  const listing = set.classify([
    entry("paper/a.md"),
    entry("paper/big.md", 500),
    entry("README.md"),
    entry("paper/data/x.csv"),
    entry("paper/data/keep.csv"),
    entry("paper/scratch/new.md"),
  ]);
  assert.deepEqual([...listing.synced.keys()], ["a.md", "data/keep.csv"]);
  assert.deepEqual(listing.refusals, []);

  const gone = await planPull({
    link: { synced_map: {}, export: null },
    synced: listing.synced,
    project: new Map(),
    readBlob: async () => enc.encode("text\n"),
    exportMerged: false,
  });
  assert.deepEqual(gone.change.create.map((item) => item.path), ["a.md", "data/keep.csv"]);

  // A mapped file that a new ignore line now names is forgotten, not deleted.
  const forgotten = await planPull({
    link: { synced_map: { "data/x.csv": { blob: "b", hash: "h" } }, export: null },
    synced: listing.synced,
    project: new Map([["data/x.csv", { kind: "text", text: "x", hash: "h" }]]),
    readBlob: async () => enc.encode("text\n"),
    exportMerged: false,
    isSynced: set.isSynced,
  });
  assert.deepEqual(forgotten.change.delete, []);
  assert.equal("data/x.csv" in forgotten.syncedMap, false);

  const clash = set.classify([entry("paper/A.md"), entry("paper/a.md")]);
  assert.equal(clash.refusals.length, 1);
  assert.deepEqual(clash.refusals[0].paths.sort(), ["paper/A.md", "paper/a.md"]);
  assert.equal(clash.synced.size, 0);

  const long = set.classify([entry(`paper/${"x".repeat(45)}.md`)]);
  assert.equal(long.refusals.length, 1);

  const many = set.classify(["a", "b", "c", "d", "e", "f"].map((n) => entry(`paper/${n}.md`)));
  assert.equal(many.refusals.length, 1);
}

// An export consumed: the project still holds what was exported and GitHub
// holds the merge, so GitHub's side applies and the export clears.
{
  const base = { "a.md": "one\n" };
  const exportedText = { "a.md": "one\nmine\n" };
  const merged = { "a.md": "one\nmine\ntheirs\n" };
  const result = await plan({
    base,
    gh: merged,
    lp: exportedText,
    exportMerged: true,
    exported: exportedText,
  });
  assert.deepEqual(result.conflicts, []);
  assert.equal(applied("one\nmine\n", result.change.patches), "one\nmine\ntheirs\n");
  assert.equal(result.exportConsumed, true);
  assert.deepEqual(result.syncedMap, await mapOf(merged));

  // Typing after the export invalidates it.
  const typed = await plan({
    base,
    gh: merged,
    lp: { "a.md": "one\nmine\nmore\n" },
    exportMerged: true,
    exported: exportedText,
  });
  assert.deepEqual(typed.conflicts, ["a.md"]);
  assert.equal(typed.exportConsumed, false);

  // A head that does not contain the export is a conflict.
  const unmerged = await plan({
    base,
    gh: merged,
    lp: exportedText,
    exportMerged: false,
    exported: exportedText,
  });
  assert.deepEqual(unmerged.conflicts, ["a.md"]);

  // With no conflict at all, an outstanding export that GitHub contains is consumed.
  const quiet = await plan({
    base,
    gh: { "a.md": "one\nx\n" },
    lp: base,
    exportMerged: true,
    exported: exportedText,
  });
  assert.equal(quiet.exportConsumed, true);
  const none = await plan({ base, gh: { "a.md": "one\nx\n" }, lp: base, exportMerged: true });
  assert.equal(none.exportConsumed, false);
}

// The push change set: changed, added and deleted synced paths only.
{
  const link = { synced_map: await mapOf({ "a.md": "a\n", "b.md": "b\n", "gone.md": "g\n", "f.png": new Uint8Array([1]) }) };
  const project = await projectOf({
    "a.md": "a\n",
    "b.md": "b2\n",
    "new.md": "n\n",
    "f.png": new Uint8Array([2]),
    "ignored.md": "i\n",
  });
  const result = planPush({ link, project, isSynced: (path) => path !== "ignored.md" });
  assert.deepEqual(result, {
    changed: [
      { path: "b.md", kind: "text", text: "b2\n", hash: await textHash("b2\n") },
      { path: "f.png", kind: "asset", sha: await sha256Hex(new Uint8Array([2])), hash: await sha256Hex(new Uint8Array([2])) },
      { path: "new.md", kind: "text", text: "n\n", hash: await textHash("n\n") },
    ],
    deleted: ["gone.md"],
  });
  const quiet = planPush({ link, project: await projectOf({ "a.md": "a\n", "b.md": "b\n", "gone.md": "g\n", "f.png": new Uint8Array([1]) }), isSynced: () => true });
  assert.deepEqual(quiet, { changed: [], deleted: [] });
  const empty = planPush({ link: { synced_map: {} }, project: new Map(), isSynced: () => true });
  assert.deepEqual(empty, { changed: [], deleted: [] });
}

console.log("forge plan ok");
