// Randomized Git sync round trips. A seeded random walk drives the sync library
// from both sides at once: plain git on the other side, and a project session
// on this side, the way the browser holds one. Each seed starts with a
// repository that holds text files, binary assets and protected entries: a
// symlink, a submodule gitlink, a Git LFS pointer, an ignored file and, on
// Linux, a file whose name is not UTF-8. Each round makes a few edits on both
// sides (renames, folder moves, folder deletions, binary assets, empty files,
// nested folders, and the same path created or deleted on both sides), syncs,
// and then checks that history only moves forward, protected entries keep their
// exact records, the project and the repository agree, no conflict markers were
// left behind, every edit that neither side contested reached both sides, and
// the object store passes fsck.
//
// To replay one seed, run with LIBREPAPER_GIT_FUZZ_SEED=<n>. Without it the run
// covers seeds 1 to LIBREPAPER_GIT_FUZZ_SEEDS (default 2), and each seed runs
// LIBREPAPER_GIT_FUZZ_ROUNDS rounds (default 4), about a minute in all. Every failure names the seed,
// the round and the operations so far, which is enough to replay the walk.
//
// Requires the same things as git-sync.mjs, and skips the same way.
// `make git-test` runs both with a throwaway PostgreSQL.
import assert from "node:assert/strict";
import { mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { test } from "node:test";
import { applyResolution, checkRemote, connect, localChanges, outgoing, pull, push } from "../../src/lib/gitsync/sync.js";
import { git, limit, paths, replaceIn, settle, textAt, wait, withFixture } from "../helpers/git-sync-harness.mjs";

const LINUX = process.platform === "linux";
const CRLF_PATH = "crlf.md";
const LFS_PATH = "data/big.csv";
const IGNORED_PATH = "build/out.log";
// Project files that sit on or under an entry LibrePaper must not touch: under
// the symlink, under the submodule, in the ignored folder, on the LFS pointer,
// and on the lossy spelling of the name that is not UTF-8. They must stay in
// the project and never reach the repository.
const HOSTILE = ["fig/notes.md", "vendor/lib/readme.md", "build/new.md", LFS_PATH, ...(LINUX ? ["odd�.md"] : [])];
const RULES_PATH = ".librepaperignore";
const TEXT_BASES = ["intro", "methods", "refs", "results", "summary", "appendix", "discussion", "one", "two"];
const TEXT_EXTENSIONS = [".qmd", ".md", ".tex", ".bib"];
const TEXT_FOLDERS = ["", "notes/", "ch/", "ch/deep/"];
// The first eight bytes of every PNG file.
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// An 8-bit RGB PNG of `width` by `height` whose channel bytes come from `byte()`.
function pngOf(width, height, byte) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const rows = [];
  for (let y = 0; y < height; y += 1) {
    rows.push(Buffer.from([0, ...Array.from({ length: width * 3 }, byte)]));
  }
  return Buffer.concat([
    Buffer.from(PNG_SIGNATURE),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.concat(rows))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/// Reads a whole number from the environment, or the fallback when it is unset.
function wholeNumber(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return fallback;
  const value = Number(raw);
  assert.ok(Number.isInteger(value) && value >= 0, `${name} is a whole number, not ${JSON.stringify(raw)}`);
  return value;
}

const ROUNDS = wholeNumber("LIBREPAPER_GIT_FUZZ_ROUNDS", 4);
const SEEDS = process.env.LIBREPAPER_GIT_FUZZ_SEED
  ? [wholeNumber("LIBREPAPER_GIT_FUZZ_SEED", 0)]
  : Array.from({ length: wholeNumber("LIBREPAPER_GIT_FUZZ_SEEDS", 2) }, (_, index) => index + 1);

/// mulberry32, a small generator whose sequence depends only on its seed.
function mulberry32(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/// The entries of a tree keyed by path. Each holds what git prints before the
/// tab: the mode, the type and the id. Read as latin1, so every byte survives.
function lsTree(cwd, ref) {
  const entries = new Map();
  for (const record of git(cwd, "ls-tree", "-r", "-z", ref).toString("latin1").split("\0")) {
    if (!record) continue;
    const tab = record.indexOf("\t");
    entries.set(record.slice(tab + 1), record.slice(0, tab));
  }
  return entries;
}

/// A path for a message, with bytes outside printable ASCII written as hex.
const shown = (path) => path.replace(/[^\x20-\x7e]/g, (byte) => `\\x${byte.charCodeAt(0).toString(16)}`);
/// Text with CRLF line endings read as LF, which is how the project holds it.
const lf = (text) => (text === null ? null : text.replace(/\r\n/g, "\n"));
const hasMarker = (text) => text.includes("<<<<<<<") || text.includes(">>>>>>>");
/// The paths the sync leaves alone on purpose: the rules file and build output.
const ignored = (path) => path === RULES_PATH || path.startsWith("build/");
/// Binary assets are the PNG files; every other synced path is text.
const isBinary = (path) => path.endsWith(".png");
const isText = (path) => !isBinary(path);
/// The last segment of a path, and the folder part of a path with its slash.
const baseOf = (path) => path.slice(path.lastIndexOf("/") + 1);
const dirOf = (path) => path.slice(0, path.lastIndexOf("/") + 1);
/// The first letter of a name in capitals, for a case-only rename.
const cased = (name) => name.charAt(0).toUpperCase() + name.slice(1);
/// The folders above a path, nearest first.
function parentsOf(path) {
  const out = [];
  for (let at = path.lastIndexOf("/"); at > 0; at = path.lastIndexOf("/", at - 1)) out.push(path.slice(0, at));
  return out;
}
/// Whether two byte sequences are equal, where a missing expectation never is.
const sameBytes = (bytes, expected) => expected !== null && Buffer.from(bytes).equals(expected);
/// The lines of a text, without the empty string after its final newline.
function linesOf(text) {
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}
/// The lines of a seeded text file, each one naming its own path.
function textBody(path, count) {
  return Array.from({ length: count }, (_, index) => `line ${index + 1} of ${path}\n`).join("");
}
/// What one round recorded: the paths each side touched, the paths that were
/// deleted or contested, the tokens written, and the binary assets as bytes
/// (null for an asset that is gone).
function newTurn() {
  return {
    added: 0,
    remote: new Set(),
    local: new Set(),
    deleted: new Set(),
    conflicted: new Set(),
    refused: new Set(),
    caseRenamed: new Set(),
    writes: [],
    binaries: new Map(),
  };
}

/// A live edit at the end of a file in the browser, then waits until the server has it.
async function appendIn(client, path, line) {
  const file = client.session.list().find((item) => item.path === path);
  const body = client.session.textOf(file.id);
  const current = body.toString();
  const prefix = current === "" || current.endsWith("\n") ? "" : "\n";
  body.insert(current.length, `${prefix}${line}\n`);
  client.session.doc.commit();
  await settle(client);
}

/// One seed's walk over a fresh repository and a fresh project.
async function walk(seed, fx) {
  const { ctx, repo } = fx;
  const owner = fx.client;
  // The page that makes an edit is `client`. It is the owner page, except inside
  // `editAs`, which makes one edit on another page and then restores the owner.
  let client = owner;
  const rng = mulberry32(seed);
  const int = (n) => Math.floor(rng() * n);
  const pick = (list) => list[int(list.length)];
  const hex = (bytes) => Array.from({ length: bytes }, () => int(256).toString(16).padStart(2, "0")).join("");
  const log = [];
  const protectedPaths = [];
  const hostile = new Set();
  const synced = () => paths(client).filter((path) => !hostile.has(path));
  const protectedBefore = new Map();
  const remoteGone = new Set();
  const counts = {
    renames: 0,
    moves: 0,
    caseRenames: 0,
    binaries: 0,
    folderDeletes: 0,
    deleteEdits: 0,
    sameCreates: 0,
    recreates: 0,
    empties: 0,
    nested: 0,
    concurrentOps: 0,
    typedEdits: 0,
    secondEdits: 0,
    collisions: 0,
  };
  let round = 0;
  let turn = newTurn();
  const fail = (message) => `${message}\nseed=${seed} round=${round}\n${log.join("\n")}`;
  const isProtected = (path) => protectedPaths.includes(path);
  /// Whether a path is protected or hostile, so that no edit may touch it.
  const reserved = (path) => isProtected(path) || hostile.has(path);
  /// Whether a folder holds, or sits on, a protected or hostile path.
  const reservedFolder = (folder) => [...protectedPaths, ...hostile]
    .some((path) => path === folder || path.startsWith(`${folder}/`) || folder.startsWith(`${path}/`));
  /// The name a path had before the walk renamed it, or the same name.
  const taken = (path, target) => [...paths(client), ...syncedPaths()]
    .some((other) => other !== path && other.toLowerCase() === target.toLowerCase());

  // ------------------------------------------------------------ the repository

  /// The synced files on the branch: blobs that are not protected and not ignored.
  function syncedPaths() {
    return [...lsTree(repo.bare, "refs/heads/main")]
      .filter(([path, meta]) => meta.split(" ")[1] === "blob" && !isProtected(path) && !ignored(path))
      .map(([path]) => path)
      .sort();
  }

  /// The synced text files on the branch that may be edited on the repository side.
  function remoteTexts() {
    return syncedPaths().filter((path) => isText(path) && path !== CRLF_PATH);
  }

  /// The text files in the project.
  function textFiles() {
    return client.session.list().filter((file) => file.kind === "text").map((file) => file.path);
  }

  /// The project's text files that may be edited on this side.
  function localTexts() {
    return client.session.list()
      .filter((file) => file.kind === "text" && !reserved(file.path))
      .map((file) => file.path);
  }

  /// The folders on this side that hold synced files and nothing protected or hostile.
  function localFolders() {
    const all = client.session.list();
    const names = new Set([...client.session.folders(), ...all.flatMap((file) => parentsOf(file.path))]);
    return [...names].filter((folder) => !reservedFolder(folder) && all.some((file) => file.path.startsWith(`${folder}/`)));
  }

  /// Records a token written this round on one side.
  function wrote(side, path, token) {
    turn.writes.push({ side, path, paths: [path], token });
    turn[side].add(path);
  }

  /// Moves what the round recorded at `from` to `to`, after a rename or a move.
  function moved(from, to) {
    for (const write of turn.writes) {
      if (write.path === from) {
        write.path = to;
        write.paths.push(to);
      }
    }
    if (turn.binaries.has(from)) {
      const bytes = turn.binaries.get(from);
      turn.binaries.set(from, null);
      if (bytes) turn.binaries.set(to, bytes);
    }
  }

  /// A valid PNG of random size and random pixels, from the seeded generator.
  /// The server checks that an uploaded figure really is an image.
  function randomPng() {
    return pngOf(2 + int(15), 2 + int(15), () => int(256));
  }

  function isAncestor(older, newer) {
    try {
      git(repo.bare, "merge-base", "--is-ancestor", older, newer);
      return true;
    } catch {
      return false;
    }
  }

  /// Null when `git fsck --strict --no-dangling` passes, otherwise its output.
  function fsckProblem() {
    try {
      git(repo.bare, "fsck", "--strict", "--no-dangling");
      return null;
    } catch (error) {
      return String(error.stderr || error.message);
    }
  }

  // ------------------------------------------------------------ the checks

  /// The paths the checks compare: every synced path except those a refusal named this round.
  const compared = (list) => list.filter((path) => !turn.refused.has(path));

  /// Whether the project and the repository hold the same content at `path`.
  async function sameContent(path) {
    const file = client.session.list().find((item) => item.path === path);
    if (isBinary(path)) {
      if (!file || file.kind !== "asset") return false;
      return sameBytes(await fx.ctx.api.assetBytes(file.sha), repo.show(path));
    }
    if (!file || file.kind !== "text") return false;
    return lf(textAt(client, path)) === lf(repo.read(path));
  }

  async function converged() {
    const expected = compared(syncedPaths());
    if (JSON.stringify(compared(synced())) !== JSON.stringify(expected)) return false;
    for (const path of expected) {
      if (!(await sameContent(path))) return false;
    }
    return true;
  }

  /// Waits briefly for the project to catch up. The checks after it name any mismatch.
  async function eventually(check) {
    try {
      await wait("the project to catch up", check, 15000);
    } catch {
      // The checks that follow report what differs.
    }
  }

  async function checkConverged(where) {
    const expected = compared(syncedPaths());
    assert.deepEqual(compared(synced()), expected, fail(`${where}: the project holds the synced paths`));
    for (const path of hostile) {
      assert.ok(paths(client).includes(path), fail(`${where}: the project keeps ${shown(path)}, which never syncs`));
    }
    for (const path of expected) {
      if (isBinary(path)) {
        const file = client.session.list().find((item) => item.path === path);
        assert.equal(file?.kind, "asset", fail(`${where}: ${shown(path)} is an asset in the project`));
        assert.ok(sameBytes(await fx.ctx.api.assetBytes(file.sha), repo.show(path)), fail(`${where}: ${shown(path)} matches the repository bytes`));
      } else {
        assert.equal(lf(textAt(client, path)), lf(repo.read(path)), fail(`${where}: ${shown(path)} matches the repository`));
      }
    }
  }

  function checkMarkers(where) {
    for (const path of textFiles()) {
      assert.ok(!hasMarker(textAt(client, path) ?? ""), fail(`${where}: no conflict marker in the project copy of ${shown(path)}`));
    }
    for (const path of syncedPaths().filter(isText)) {
      assert.ok(!hasMarker(repo.read(path) ?? ""), fail(`${where}: no conflict marker in the repository copy of ${shown(path)}`));
    }
  }

  function checkProtected(where) {
    const now = lsTree(repo.bare, "refs/heads/main");
    for (const path of protectedPaths) {
      assert.equal(now.get(path), protectedBefore.get(path), fail(`${where}: the protected entry ${shown(path)} is unchanged`));
    }
  }

  /// Every token this round that neither side contested must be on both sides.
  /// A token counts as contested when the other side touched any path it has
  /// lived at this round, since a rename by either side can carry it along.
  function checkWrites() {
    for (const write of turn.writes) {
      const other = write.side === "remote" ? turn.local : turn.remote;
      const touches = (set) => write.paths.some((path) => set.has(path));
      if (touches(other) || touches(turn.conflicted) || touches(turn.refused) || turn.deleted.has(write.path)) continue;
      const onRepository = lf(repo.read(write.path)) ?? "";
      const inProject = lf(textAt(client, write.path)) ?? "";
      assert.ok(onRepository.includes(write.token), fail(`the ${write.side} token ${write.token} is on the repository in ${shown(write.path)}`));
      assert.ok(inProject.includes(write.token), fail(`the ${write.side} token ${write.token} is in the project copy of ${shown(write.path)}`));
    }
  }

  /// Every binary this round that neither side contested must hold the bytes
  /// written, on both sides; a null entry must be gone from both.
  async function checkBinaries(where) {
    for (const [path, bytes] of turn.binaries) {
      if (turn.conflicted.has(path) || turn.refused.has(path) || (turn.remote.has(path) && turn.local.has(path))) continue;
      const file = client.session.list().find((item) => item.path === path);
      if (bytes === null) {
        assert.equal(repo.show(path), null, fail(`${where}: the repository no longer holds ${shown(path)}`));
        assert.equal(file, undefined, fail(`${where}: the project no longer holds ${shown(path)}`));
        continue;
      }
      assert.ok(sameBytes(bytes, repo.show(path)), fail(`${where}: the repository holds the bytes written to ${shown(path)}`));
      assert.equal(file?.kind, "asset", fail(`${where}: the project holds ${shown(path)} as an asset`));
      assert.ok(sameBytes(await fx.ctx.api.assetBytes(file.sha), bytes), fail(`${where}: the project holds the bytes written to ${shown(path)}`));
    }
  }

  // ------------------------------------------------------------ seeding

  const wanted = 3 + int(4);
  const seeded = {};
  while (Object.keys(seeded).length < wanted) {
    const path = `${pick(TEXT_FOLDERS)}${pick(TEXT_BASES)}${pick(TEXT_EXTENSIONS)}`;
    if (!(path in seeded)) seeded[path] = textBody(path, 6 + int(7));
  }
  seeded[CRLF_PATH] = "# CRLF notes\r\n\r\nFirst line.\r\nSecond line.\r\n";
  seeded[LFS_PATH] = `version https://git-lfs.github.com/spec/v1\noid sha256:${hex(32)}\nsize 12345\n`;
  seeded[IGNORED_PATH] = "scratch output\n";
  seeded[RULES_PATH] = "build/\n";
  seeded["img/seed.png"] = randomPng();
  repo.commit(seeded, "Seed");
  log.push(`seed ${Object.keys(seeded).sort().join(", ")}`);

  // The symlink and the gitlink are made with git directly. The gitlink is
  // added to the index after `add --all`, so that the add never sees it
  // without a directory.
  symlinkSync("intro.qmd", join(repo.clone, "fig"));
  if (LINUX) writeFileSync(Buffer.concat([Buffer.from(`${repo.clone}/`), Buffer.from("odd"), Buffer.from([0xff]), Buffer.from(".md")]), "not UTF-8\n");
  git(repo.clone, "add", "--all");
  git(repo.clone, "update-index", "--add", "--cacheinfo", `160000,${repo.headOf()},vendor/lib`);
  git(repo.clone, "commit", "--quiet", "-m", "Protected entries");
  git(repo.clone, "push", "--quiet", "origin", "main");
  // An uninitialized submodule is an empty directory. Without it, the other
  // side's later `add --all` would stage the gitlink's removal itself.
  mkdirSync(join(repo.clone, "vendor/lib"), { recursive: true });
  log.push("seed the protected entries: fig, vendor/lib, data/big.csv, build/out.log, and the non-UTF-8 name");

  protectedPaths.push("fig", "vendor/lib", LFS_PATH, IGNORED_PATH);
  if (LINUX) {
    // The name is read back from the tree, as git lists it.
    const odd = [...lsTree(repo.clone, "HEAD").keys()].find((path) => path.includes("ÿ"));
    assert.ok(odd, fail("the file whose name is not UTF-8 is in the tree"));
    protectedPaths.push(odd);
  }
  const branch = lsTree(repo.bare, "refs/heads/main");
  for (const path of protectedPaths) {
    protectedBefore.set(path, branch.get(path));
    assert.ok(branch.get(path), fail(`the protected entry ${shown(path)} is on the branch`));
  }

  // A second page on the same project, which makes some of the local edits.
  const second = await fx.another();

  // ------------------------------------------------------------ connecting

  const recorded = await limit("connect", connect(ctx, fx.draft));
  assert.equal(recorded.status, "connected", fail(`connect ended as ${recorded.status}: ${JSON.stringify(recorded)}`));
  const connected = await limit("pull", pull(ctx));
  assert.ok(["pulled", "nothing"].includes(connected.status), fail(`the first pull ended as ${connected.status}: ${JSON.stringify(connected)}`));
  log.push(`connect, first pull ${connected.status}`);
  await eventually(converged);
  await checkConverged("the first connection");
  checkMarkers("the first connection");
  checkProtected("the first connection");
  await eventually(() => viewOf(second.client) === viewOf(owner));
  assert.equal(viewOf(second.client), viewOf(owner), fail("the second page holds the connected project"));

  // ------------------------------------------------------------ edits

  /// Runs one edit with `who` as the page that makes it. The owner page is
  /// `client` again afterwards, so the checks always read the owner.
  async function editAs(who, action) {
    const was = client;
    client = who;
    if (who !== owner) counts.secondEdits += 1;
    try {
      await action();
    } finally {
      client = was;
    }
  }

  /// The project as a page sees it: each path with its kind, and its text or its asset digest.
  function viewOf(who) {
    const rows = who.session.list().map((file) => [file.path, file.kind, file.kind === "text" ? who.session.textOf(file.id).toString() : file.sha]);
    return JSON.stringify(rows.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
  }

  /// One edit typed during a sync call, on a text file the typing page holds.
  async function typeOnce() {
    const candidates = localTexts();
    if (!candidates.length) return;
    const path = pick(candidates);
    const word = hex(3);
    await appendIn(client, path, `line typed during sync r${round} ${word}`);
    wrote("local", path, word);
    counts.typedEdits += 1;
  }

  /// Types one or two edits while a sync call runs, mostly in the second page.
  async function typeDuring() {
    const edits = 1 + int(2);
    for (let index = 0; index < edits; index += 1) {
      await editAs(rng() < 0.7 ? second.client : owner, typeOnce);
    }
  }

  /// The resolution for one conflicted path: a side that has the file, or a deletion.
  function choiceFor(item) {
    const options = [];
    if (item.ours) options.push({ keep: "ours" });
    if (item.theirs) options.push({ keep: "theirs" });
    if (!item.ours || !item.theirs) options.push({ delete: true });
    return pick(options);
  }

  /// One edit on the other side, committed and pushed through the repository helper.
  function remoteEdit() {
    const special = rng();
    if (special < 0.05) return remoteRename();
    if (special < 0.08) return remoteCase();
    if (special < 0.12) return remoteBinary();
    if (special < 0.14) return remoteFolderDelete();
    if (special < 0.18) return remoteRecreate();
    if (special < 0.22) return remoteEmpty();
    if (special < 0.25) return remoteNested();

    const editable = remoteTexts();
    const roll = rng();
    let kind = "add";
    if (editable.length && roll < 0.125) kind = "delete";
    else if (editable.length && roll < 0.45) kind = "change";
    else if (editable.length && roll < 0.75) kind = "append";

    if (kind === "add") {
      const path = `new-r${round}-${turn.added}.md`;
      turn.added += 1;
      const word = hex(3);
      repo.commit({ [path]: `line added remote r${round} ${word}\n` }, `Fuzz r${round} add`);
      log.push(`r${round} remote add ${path}`);
      turn.remote.add(path);
      turn.writes.push({ side: "remote", path, paths: [path], token: word });
      return;
    }
    const path = pick(editable);
    if (kind === "delete") {
      repo.commit({ [path]: null }, `Fuzz r${round} delete`);
      log.push(`r${round} remote delete ${path}`);
      turn.remote.add(path);
      turn.deleted.add(path);
      remoteGone.add(path);
      return;
    }
    // A path this round already wrote on this side takes an append, so that a
    // line change never overwrites a token written earlier in the round.
    const edit = turn.remote.has(path) ? "append" : kind;
    const word = hex(3);
    const lines = linesOf(repo.read(path));
    if (edit === "change") {
      const index = int(lines.length);
      lines[index] = `line ${index + 1} remote r${round} ${word}`;
      log.push(`r${round} remote edit ${path} line ${index + 1}`);
    } else {
      lines.push(`line appended remote r${round} ${word}`);
      log.push(`r${round} remote append ${path}`);
    }
    repo.commit({ [path]: `${lines.join("\n")}\n` }, `Fuzz r${round} edit`);
    turn.remote.add(path);
    turn.writes.push({ side: "remote", path, paths: [path], token: word });
  }

  /// A rename on the repository side, as `git mv` makes it: the old path goes and the new one takes its content.
  function remoteRename() {
    const candidates = remoteTexts().filter((path) => !turn.remote.has(path));
    if (!candidates.length) return;
    const path = pick(candidates);
    const k = turn.added;
    turn.added += 1;
    const target = rng() < 0.5
      ? `moved/remote-r${round}-${k}-${baseOf(path)}`
      : `${dirOf(path)}remote-r${round}-${k}-${baseOf(path)}`;
    repo.commit({ [path]: null, [target]: repo.read(path) }, `Fuzz r${round} rename`);
    log.push(`r${round} remote rename ${shown(path)} to ${shown(target)}`);
    turn.remote.add(path);
    turn.remote.add(target);
    turn.deleted.add(path);
    moved(path, target);
    remoteGone.add(path);
    counts.renames += 1;
  }

  /// A case-only rename on the repository side, such as `intro.md` to `Intro.md`.
  function remoteCase() {
    const candidates = remoteTexts().filter((path) => !turn.remote.has(path) && /^[a-z]/.test(baseOf(path)));
    if (!candidates.length) return;
    const path = pick(candidates);
    const target = `${dirOf(path)}${cased(baseOf(path))}`;
    if (taken(path, target)) return;
    repo.commit({ [path]: null, [target]: repo.read(path) }, `Fuzz r${round} case rename`);
    log.push(`r${round} remote case ${shown(path)} to ${shown(target)}`);
    turn.remote.add(path);
    turn.remote.add(target);
    turn.deleted.add(path);
    turn.caseRenamed.add(path);
    turn.caseRenamed.add(target);
    moved(path, target);
    remoteGone.add(path);
    counts.caseRenames += 1;
  }

  /// Adds, replaces or deletes a binary asset on the repository side.
  function remoteBinary() {
    const existing = syncedPaths().filter((path) => isBinary(path) && !turn.remote.has(path));
    const roll = rng();
    if (existing.length && roll < 0.33) {
      const path = pick(existing);
      repo.commit({ [path]: null }, `Fuzz r${round} binary delete`);
      log.push(`r${round} remote binary delete ${shown(path)}`);
      turn.remote.add(path);
      turn.binaries.set(path, null);
      counts.binaries += 1;
      return;
    }
    if (existing.length && roll < 0.66) {
      const path = pick(existing);
      const bytes = randomPng();
      repo.commit({ [path]: bytes }, `Fuzz r${round} binary replace`);
      log.push(`r${round} remote binary replace ${shown(path)}`);
      turn.remote.add(path);
      turn.binaries.set(path, bytes);
      counts.binaries += 1;
      return;
    }
    const path = `img/remote-r${round}-${turn.added}.png`;
    turn.added += 1;
    const bytes = randomPng();
    repo.commit({ [path]: bytes }, `Fuzz r${round} binary add`);
    log.push(`r${round} remote binary add ${shown(path)}`);
    turn.remote.add(path);
    turn.binaries.set(path, bytes);
    counts.binaries += 1;
  }

  /// Deletes every synced file of one folder on the repository side in one commit.
  function remoteFolderDelete() {
    const folders = new Set(syncedPaths().flatMap(parentsOf));
    const candidates = [...folders].filter((folder) => !reservedFolder(folder));
    if (!candidates.length) return;
    const folder = pick(candidates);
    const doomed = syncedPaths().filter((path) => path.startsWith(`${folder}/`));
    if (!doomed.length) return;
    repo.commit(Object.fromEntries(doomed.map((path) => [path, null])), `Fuzz r${round} folder delete`);
    log.push(`r${round} remote folder delete ${shown(folder)} (${doomed.length} files)`);
    for (const path of doomed) {
      turn.remote.add(path);
      if (isBinary(path)) {
        turn.binaries.set(path, null);
      } else {
        turn.deleted.add(path);
        remoteGone.add(path);
      }
    }
    counts.folderDeletes += 1;
  }

  /// Recreates a text file that the repository deleted in an earlier round, with new content and a token.
  function remoteRecreate() {
    const candidates = [...remoteGone].filter((path) => repo.show(path) === null
      && !reserved(path) && path !== CRLF_PATH && !turn.remote.has(path) && !turn.local.has(path));
    if (!candidates.length) return;
    const path = pick(candidates);
    const word = hex(3);
    repo.commit({ [path]: `recreated remote r${round} ${word}\n` }, `Fuzz r${round} recreate`);
    log.push(`r${round} remote recreate ${shown(path)}`);
    remoteGone.delete(path);
    wrote("remote", path, word);
    counts.recreates += 1;
  }

  /// Creates an empty text file, or empties an existing one, on the repository side.
  function remoteEmpty() {
    if (rng() < 0.5) {
      const path = `empty-r${round}-${turn.added}.md`;
      turn.added += 1;
      repo.commit({ [path]: "" }, `Fuzz r${round} empty add`);
      log.push(`r${round} remote empty add ${path}`);
      turn.remote.add(path);
    } else {
      const candidates = remoteTexts().filter((path) => !turn.remote.has(path));
      if (!candidates.length) return;
      const path = pick(candidates);
      repo.commit({ [path]: "" }, `Fuzz r${round} empty edit`);
      log.push(`r${round} remote empty edit ${shown(path)}`);
      turn.remote.add(path);
    }
    counts.empties += 1;
  }

  /// Creates a file in a folder that did not exist before, on the repository side.
  function remoteNested() {
    const path = `deep/a/b/remote-r${round}-${turn.added}.md`;
    turn.added += 1;
    const word = hex(3);
    repo.commit({ [path]: `nested remote r${round} ${word}\n` }, `Fuzz r${round} nested`);
    log.push(`r${round} remote nested ${path}`);
    wrote("remote", path, word);
    counts.nested += 1;
  }

  /// One live edit in the browser: a new file, a replaced line, an appended line, or a removed file.
  async function localEdit() {
    if (rng() < 0.15) {
      const free = HOSTILE.filter((path) => !paths(client).includes(path));
      if (free.length) {
        const path = pick(free);
        client.session.addText(path, `a project file that must not sync r${round}\n`);
        await settle(client);
        hostile.add(path);
        log.push(`r${round} local add ${shown(path)} over a protected entry`);
        return;
      }
    }
    const special = rng();
    if (special < 0.05) return renameLocal();
    if (special < 0.08) return caseLocal();
    if (special < 0.11) return moveFolderLocal();
    if (special < 0.14) return binaryLocal();
    if (special < 0.16) return deleteFolderLocal();
    if (special < 0.19) return recreateLocal();
    if (special < 0.22) return emptyLocal();
    if (special < 0.25) return nestedLocal();

    const names = localTexts();
    const roll = rng();
    let kind = "add";
    if (names.length && roll < 0.125) kind = "remove";
    else if (names.length && roll < 0.45) kind = "replace";
    else if (names.length && roll < 0.75) kind = "append";

    if (kind === "add") {
      const path = `new-l${round}-${turn.added}.md`;
      turn.added += 1;
      const word = hex(3);
      client.session.addText(path, `line added local r${round} ${word}\n`);
      await settle(client);
      log.push(`r${round} local add ${path}`);
      turn.local.add(path);
      turn.writes.push({ side: "local", path, paths: [path], token: word });
      return;
    }
    const path = pick(names);
    const file = client.session.list().find((item) => item.path === path);
    if (kind === "remove") {
      client.session.removeFile(file.id);
      await settle(client);
      log.push(`r${round} local remove ${path}`);
      turn.local.add(path);
      turn.deleted.add(path);
      return;
    }
    const word = hex(3);
    const current = client.session.textOf(file.id).toString();
    const lines = linesOf(current);
    // A line can be replaced only where its first occurrence is that line.
    const candidates = [];
    let offset = 0;
    lines.forEach((line, index) => {
      if (line !== "" && current.indexOf(line) === offset) candidates.push(index);
      offset += line.length + 1;
    });
    if (kind === "replace" && candidates.length) {
      const index = pick(candidates);
      const line = lines[index];
      await replaceIn(client, path, line, `${line} local r${round} ${word}`);
      log.push(`r${round} local edit ${path} line ${index + 1}`);
    } else {
      await appendIn(client, path, `line appended local r${round} ${word}`);
      log.push(`r${round} local append ${path}`);
    }
    turn.local.add(path);
    turn.writes.push({ side: "local", path, paths: [path], token: word });
  }

  /// A rename in the browser, to a new folder or to a new name beside the old one.
  async function renameLocal() {
    const candidates = localTexts().filter((path) => path !== CRLF_PATH && !turn.local.has(path));
    if (!candidates.length) return;
    const path = pick(candidates);
    const file = client.session.list().find((item) => item.path === path);
    const k = turn.added;
    turn.added += 1;
    const target = rng() < 0.5
      ? `moved/r${round}-${k}-${baseOf(path)}`
      : `${dirOf(path)}r${round}-${k}-${baseOf(path)}`;
    client.session.renameFile(file.id, target);
    await settle(client);
    log.push(`r${round} local rename ${shown(path)} to ${shown(target)}`);
    turn.local.add(path);
    turn.local.add(target);
    turn.deleted.add(path);
    moved(path, target);
    counts.renames += 1;
  }

  /// A case-only rename in the browser, such as `intro.md` to `Intro.md`.
  async function caseLocal() {
    const candidates = localTexts().filter((path) => path !== CRLF_PATH && !turn.local.has(path) && /^[a-z]/.test(baseOf(path)));
    if (!candidates.length) return;
    const path = pick(candidates);
    const target = `${dirOf(path)}${cased(baseOf(path))}`;
    if (taken(path, target)) return;
    const file = client.session.list().find((item) => item.path === path);
    client.session.renameFile(file.id, target);
    await settle(client);
    log.push(`r${round} local case ${shown(path)} to ${shown(target)}`);
    turn.local.add(path);
    turn.local.add(target);
    turn.deleted.add(path);
    turn.caseRenamed.add(path);
    turn.caseRenamed.add(target);
    moved(path, target);
    counts.caseRenames += 1;
  }

  /// Moves a whole folder that holds synced files into `moved/`, with a unique name.
  async function moveFolderLocal() {
    const folders = localFolders();
    if (!folders.length) return;
    const folder = pick(folders);
    const k = turn.added;
    turn.added += 1;
    const destination = `moved/r${round}-${k}-${baseOf(folder)}`;
    const plan = client.session.relocate([{ kind: "folder", id: folder, path: folder }], destination, ctx.rules, true);
    await settle(client);
    log.push(`r${round} local move folder ${shown(folder)} to ${shown(destination)} (${plan.files.length} files)`);
    for (const file of plan.files) {
      turn.local.add(file.previousPath);
      turn.local.add(file.path);
      turn.deleted.add(file.previousPath);
      moved(file.previousPath, file.path);
    }
    counts.moves += 1;
  }

  /// Removes a whole folder that holds synced files, with every file in it.
  async function deleteFolderLocal() {
    const folders = localFolders();
    if (!folders.length) return;
    const folder = pick(folders);
    const doomed = client.session.list().filter((file) => file.path.startsWith(`${folder}/`));
    client.session.removeEntries([{ kind: "folder", id: folder, path: folder }]);
    await settle(client);
    log.push(`r${round} local folder delete ${shown(folder)} (${doomed.length} files)`);
    for (const file of doomed) {
      turn.local.add(file.path);
      if (file.kind === "asset") {
        turn.binaries.set(file.path, null);
      } else {
        turn.deleted.add(file.path);
      }
    }
    counts.folderDeletes += 1;
  }

  /// Adds, replaces or removes a binary asset in the browser.
  async function binaryLocal() {
    const assets = client.session.list().filter((file) => file.kind === "asset" && !reserved(file.path) && !turn.local.has(file.path));
    const roll = rng();
    if (assets.length && roll < 0.33) {
      const file = pick(assets);
      client.session.removeAsset(file.path);
      await settle(client);
      log.push(`r${round} local binary remove ${shown(file.path)}`);
      turn.local.add(file.path);
      turn.binaries.set(file.path, null);
      counts.binaries += 1;
      return;
    }
    const bytes = randomPng();
    const { sha } = await fx.ctx.api.uploadAsset(bytes);
    let path;
    if (assets.length && roll < 0.66) {
      path = pick(assets).path;
      log.push(`r${round} local binary replace ${shown(path)}`);
    } else {
      path = `img/r${round}-${turn.added}.png`;
      turn.added += 1;
      log.push(`r${round} local binary add ${shown(path)}`);
    }
    client.session.putAsset(path, sha);
    await settle(client);
    turn.local.add(path);
    turn.binaries.set(path, bytes);
    counts.binaries += 1;
  }

  /// Deletes a text file in the browser and recreates it at once, with a token.
  async function recreateLocal() {
    const candidates = localTexts().filter((path) => path !== CRLF_PATH && !turn.local.has(path) && !turn.remote.has(path));
    if (!candidates.length) return;
    const path = pick(candidates);
    const file = client.session.list().find((item) => item.path === path);
    client.session.removeFile(file.id);
    await settle(client);
    const word = hex(3);
    client.session.addText(path, `recreated local r${round} ${word}\n`);
    await settle(client);
    log.push(`r${round} local recreate ${shown(path)}`);
    wrote("local", path, word);
    counts.recreates += 1;
  }

  /// Creates an empty text file in the browser, or empties an existing one.
  async function emptyLocal() {
    if (rng() < 0.5) {
      const path = `empty-l${round}-${turn.added}.md`;
      turn.added += 1;
      client.session.addText(path, "");
      await settle(client);
      log.push(`r${round} local empty add ${path}`);
      turn.local.add(path);
    } else {
      const candidates = localTexts().filter((path) => !turn.local.has(path));
      if (!candidates.length) return;
      const path = pick(candidates);
      const file = client.session.list().find((item) => item.path === path);
      const body = client.session.textOf(file.id);
      body.delete(0, body.toString().length);
      client.session.doc.commit();
      await settle(client);
      log.push(`r${round} local empty edit ${shown(path)}`);
      turn.local.add(path);
    }
    counts.empties += 1;
  }

  /// Creates a file in a folder that did not exist before, in the browser.
  async function nestedLocal() {
    const path = `deep/a/b/l-r${round}-${turn.added}.md`;
    turn.added += 1;
    const word = hex(3);
    client.session.addText(path, `nested local r${round} ${word}\n`);
    await settle(client);
    log.push(`r${round} local nested ${path}`);
    wrote("local", path, word);
    counts.nested += 1;
  }

  /// A delete on one side against an edit on the other, in the same round.
  /// Either side may be the one that deletes. The outcome is contested.
  async function deleteVsEdit() {
    const candidates = remoteTexts().filter((path) => !turn.remote.has(path) && !turn.local.has(path));
    if (!candidates.length) return;
    const path = pick(candidates);
    const word = hex(3);
    if (rng() < 0.5) {
      repo.commit({ [path]: null }, `Fuzz r${round} delete against an edit`);
      remoteGone.add(path);
      turn.remote.add(path);
      turn.deleted.add(path);
      log.push(`r${round} remote delete ${shown(path)} against a local edit`);
      await appendIn(client, path, `line appended local r${round} ${word}`);
      wrote("local", path, word);
    } else {
      const file = client.session.list().find((item) => item.path === path);
      client.session.removeFile(file.id);
      await settle(client);
      turn.local.add(path);
      turn.deleted.add(path);
      log.push(`r${round} local delete ${shown(path)} against a remote edit`);
      const current = repo.read(path);
      const body = current.endsWith("\n") || current === "" ? current : `${current}\n`;
      repo.commit({ [path]: `${body}line appended remote r${round} ${word}\n` }, `Fuzz r${round} edit against a delete`);
      wrote("remote", path, word);
    }
    counts.deleteEdits += 1;
  }

  /// The same new path created on both sides in one round, with the same or different content.
  async function sameCreate() {
    const path = `dup-r${round}-${turn.added}.md`;
    turn.added += 1;
    const word = hex(3);
    const shared = `same content r${round} ${word}\n`;
    const identical = rng() < 0.5;
    repo.commit({ [path]: shared }, `Fuzz r${round} same path`);
    client.session.addText(path, identical ? shared : `different content r${round} ${hex(3)}\n`);
    await settle(client);
    log.push(`r${round} same path ${path} on both sides, ${identical ? "identical" : "different"} content`);
    turn.remote.add(path);
    turn.local.add(path);
    counts.sameCreates += 1;
  }

  // ------------------------------------------------------------ syncing

  /// Answers each conflict with a random side, up to three times over.
  async function resolveAll(what, answer) {
    let current = answer;
    for (let attempt = 0; current.status === "conflict" && attempt < 3; attempt += 1) {
      log.push(`r${round} ${what} conflict [${current.conflicts.map((item) => item.path).join(", ")}]`);
      const choices = {};
      for (const item of current.conflicts) {
        choices[item.path] = choiceFor(item);
        turn.conflicted.add(item.path);
        log.push(`r${round} resolve ${item.path} ${choices[item.path].delete ? "delete" : choices[item.path].keep}`);
      }
      current = await limit("apply the resolution", applyResolution(ctx, current.session, choices));
      current = await recoverApplied(current, choices);
    }
    return current;
  }

  /// The refusal text for two project paths that would share a name.
  const SAME_NAME = "These files would have the same name in LibrePaper";
  /// Whether an answer refuses because two project paths would share a name.
  const isColliding = (answer) => answer.status === "refused"
    && (answer.refusals ?? []).some((item) => item.reason.startsWith(SAME_NAME));
  /// The paths that a colliding refusal names.
  const collidingPaths = (answer) => (answer.refusals ?? [])
    .filter((item) => item.reason.startsWith(SAME_NAME))
    .flatMap((item) => item.paths ?? []);

  /// Logs a colliding refusal and marks its paths as contested, so the token checks skip them.
  function noteCollision(answer) {
    const named = collidingPaths(answer);
    counts.collisions += 1;
    for (const path of named) turn.conflicted.add(path);
    log.push(`r${round} refused as colliding [${named.map((path) => shown(path)).join(", ")}]`);
  }

  /// Retries a resolution that collided. The first retry deletes a conflicted
  /// path that the repository side supplied, and a later one keeps the project side.
  async function recoverApplied(answer, choices) {
    let current = answer;
    let retry = choices;
    for (let tries = 0; isColliding(current) && tries < 2; tries += 1) {
      noteCollision(current);
      const named = collidingPaths(current);
      retry = { ...retry };
      for (const item of current.session.conflicts) {
        if (named.includes(item.path)) retry[item.path] = tries === 0 && item.theirs ? { delete: true } : { keep: "ours" };
      }
      current = await limit("apply the resolution", applyResolution(ctx, current.session, retry));
    }
    return current;
  }

  /// Removes the project's copy of each colliding path that the repository does
  /// not hold, then settles, so that the next pull brings the repository's copy in.
  async function dropLocalCopies(answer) {
    noteCollision(answer);
    const remote = new Set(syncedPaths());
    for (const path of collidingPaths(answer)) {
      const file = client.session.list().find((item) => item.path === path);
      if (!file || remote.has(path)) continue;
      client.session.removeFile(file.id);
      log.push(`r${round} local remove ${shown(path)}`);
      turn.local.add(path);
      turn.deleted.add(path);
    }
    await settle(client);
  }

  /// Whether an answer refuses because the files changed while the call ran.
  /// The sync library retries those itself, then names the files it gave up on.
  const changedRefusal = (answer) => answer.status === "refused"
    && (answer.refusals ?? []).some((item) => item.reason === "changed" || /changed in LibrePaper/.test(item.reason));

  /// Runs one sync call. When `concurrent` is set, the page types while the call
  /// runs, so the edits race it. A refusal because the files changed is retried
  /// up to three times over, and each retry is logged.
  async function runOp(what, start, concurrent = false) {
    for (let tries = 0; ; tries += 1) {
      const running = limit(what, start());
      if (concurrent) {
        counts.concurrentOps += 1;
        await typeDuring();
      }
      const answer = await running;
      if (!changedRefusal(answer) || tries >= 3) return answer;
      log.push(`r${round} ${what} refused as changed, retry ${tries + 1}`);
    }
  }

  /// Runs one sync call and resolves its conflicts.
  async function step(what, start, concurrent = false) {
    return resolveAll(what, await runOp(what, start, concurrent));
  }

  /// Whether a refusal names only paths this round renamed by case. The server
  /// may refuse those, since two names that differ only in case cannot both
  /// live in one project. Those paths are excluded from this round's checks,
  /// and a refusal of any other path is not tolerated.
  function tolerated(answer) {
    if (answer.status !== "refused") return false;
    const named = (answer.refusals ?? []).flatMap((item) => item.paths ?? []);
    if (!named.length || !named.every((path) => turn.caseRenamed.has(path))) return false;
    for (const path of named) turn.refused.add(path);
    log.push(`r${round} refused the case renames [${named.map((path) => shown(path)).join(", ")}]`);
    return true;
  }

  /// One pull. In a concurrent call the page types while it runs.
  async function pullStep(label, concurrent) {
    let answer = await step("pull", () => pull(ctx), concurrent);
    for (let tries = 0; isColliding(answer) && tries < 2; tries += 1) {
      await dropLocalCopies(answer);
      answer = await step("pull", () => pull(ctx));
    }
    assert.ok(!isColliding(answer) && (tolerated(answer) || ["pulled", "nothing"].includes(answer.status)), fail(`${label} ended as ${answer.status}: ${JSON.stringify(answer)}`));
  }

  /// A push that answers behind, because the remote has commits the checkpoint
  /// lacks, is a legitimate outcome: the step pulls, then pushes once more. A
  /// second behind answer is left to the settling steps of the round.
  async function behindStep(retried) {
    log.push(`r${round} behind the remote, pulled`);
    await pullStep("the pull after a behind answer");
    if (!retried) await pushStep(false, true);
  }

  /// One outgoing check and push. A remote that moved during the push is
  /// planned again once. A collision found by the pull inside either call is
  /// recovered once, by a fresh check, and then the step runs again.
  async function pushStep(concurrent, retried = false) {
    const planned = await step("outgoing", () => outgoing(ctx));
    if (isColliding(planned) && !retried) {
      await dropLocalCopies(planned);
      return pushStep(false, true);
    }
    if (planned.status === "behind") return behindStep(retried);
    if (planned.status !== "outgoing") {
      assert.ok(!isColliding(planned) && (tolerated(planned) || planned.status === "nothing"), fail(`the outgoing check ended as ${planned.status}: ${JSON.stringify(planned)}`));
      return;
    }
    log.push(`r${round} outgoing [${(planned.files || []).map((file) => file.path).join(", ")}]`);
    let pushed = await runOp("push", () => push(ctx, `Fuzz r${round}`, { confirmed: planned.outgoing }), concurrent);
    if (pushed.status === "moved") {
      log.push(`r${round} push moved, pulled again and retried once`);
      const replanned = await step("outgoing", () => outgoing(ctx));
      if (replanned.status === "outgoing") {
        pushed = await runOp("push", () => push(ctx, `Fuzz r${round}`, { confirmed: replanned.outgoing }), concurrent);
      } else {
        pushed = isColliding(replanned) || replanned.status === "behind" ? replanned : null;
      }
    }
    if (isColliding(pushed) && !retried) {
      await dropLocalCopies(pushed);
      return pushStep(false, true);
    }
    if (pushed?.status === "behind") return behindStep(retried);
    if (pushed) assert.ok(!isColliding(pushed) && (pushed.status === "pushed" || tolerated(pushed)), fail(`the push ended as ${pushed.status}: ${JSON.stringify(pushed)}`));
    log.push(`r${round} push done`);
  }

  /// A full sync: pull, push and pull. In a concurrent round the page types
  /// while some of those calls run, and a settling push and pull follow, so the
  /// edits typed during the calls reach the repository.
  async function syncRound(concurrent) {
    await pullStep("the pull", concurrent && rng() < 0.5);
    await pushStep(concurrent && rng() < 0.5);
    await pullStep("the final pull", concurrent && rng() < 0.5);
    if (concurrent) {
      await pushStep(false);
      await pullStep("the settling pull", false);
    }
  }

  // ------------------------------------------------------------ rounds

  // A thrown error, not only a failed assertion, carries the seed, the round
  // and the operations so far, so it can be replayed too.
  try {
  for (round = 1; round <= ROUNDS; round += 1) {
    turn = newTurn();
    const concurrent = rng() < 0.4;
    const before = repo.headOf();
    if (rng() < 0.2) await deleteVsEdit();
    if (rng() < 0.15) await sameCreate();
    const remoteEdits = int(3);
    for (let index = 0; index < remoteEdits; index += 1) remoteEdit();
    const localEdits = int(3);
    const bySecond = int(3);
    for (let index = 0; index < localEdits; index += 1) {
      await editAs(index < bySecond ? second.client : owner, localEdit);
    }

    await syncRound(concurrent);
    await eventually(converged);
    await checkConverged(`round ${round}`);
    checkMarkers(`round ${round}`);
    checkWrites();
    await checkBinaries(`round ${round}`);
    checkProtected(`round ${round}`);
    await eventually(() => viewOf(second.client) === viewOf(owner));
    assert.equal(viewOf(second.client), viewOf(owner), fail(`round ${round}: the second page holds the project the owner holds`));
    assert.equal((await checkRemote(second.ctx)).status, "current", fail(`round ${round}: the second page finds the repository current`));
    const leftover = await localChanges(second.ctx);
    assert.equal(leftover.status, "none", fail(`round ${round}: the second page finds no local changes, not ${JSON.stringify(leftover)}`));
    assert.ok(isAncestor(before, repo.headOf()), fail(`the remote head ${before} is an ancestor of ${repo.headOf()}`));
    assert.equal(fsckProblem(), null, fail("git fsck --strict --no-dangling passes on the repository"));
  }
  } catch (error) {
    if (!(error instanceof assert.AssertionError)) error.message = fail(error.message);
    throw error;
  }

  assert.equal((await fx.link()).checkpoint, repo.headOf(), fail("the link records the remote head"));
  // One line per seed says what the walk exercised, so a run that passes
  // without a single conflict or deletion is visible as such.
  const count = (pattern) => log.filter((line) => pattern.test(line)).length;
  console.log(`git-sync-fuzz seed=${seed}: ${count(/ push done$/)} pushes, ${count(/ conflict \[/)} conflicts, `
    + `${count(/ resolve /)} resolutions, ${count(/ (remote delete|local remove) /)} deletions, ${count(/ refused as changed/)} retries, ${counts.collisions} collisions, ${hostile.size} files over protected entries, `
    + `${counts.renames} renames, ${counts.moves} folder moves, ${counts.caseRenames} case renames, ${counts.binaries} binary operations, `
    + `${counts.folderDeletes} folder deletions, ${counts.deleteEdits} delete-edit pairs, ${counts.sameCreates} same-path creations, `
    + `${counts.recreates} recreations, ${counts.empties} empty files, ${counts.nested} nested creations, `
    + `${counts.concurrentOps} concurrent operations, ${counts.typedEdits} edits typed during them, ${counts.secondEdits} second-page edits`);
}

for (const seed of SEEDS) {
  test(`git sync fuzz, seed ${seed}`, { timeout: 600000 }, () => withFixture({}, (fx) => walk(seed, fx)));
}
