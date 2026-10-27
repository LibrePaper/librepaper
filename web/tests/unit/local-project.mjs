// A folder opened as a project: the standalone session, the document built
// from a tree, and what an outside change to a file does to the editor.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import * as collab from "../../src/lib/collab.js";
import { createProjectSession } from "../../src/lib/project-session.js";
import { createFigureStore, createLocalProject, localIdOf, localSlug } from "../../src/lib/reader/local-project.js";

const tick = async () => {
  for (let turn = 0; turn < 20; turn++) await new Promise((resolve) => setImmediate(resolve));
};
// Waits for work that leaves the event loop, such as hashing a figure.
const until = async (done) => {
  for (let turn = 0; turn < 200 && !done(); turn++) await new Promise((resolve) => setTimeout(resolve, 5));
};
const enc = new TextEncoder();
const dec = new TextDecoder();
const bytesOf = (value) => (typeof value === "string" ? enc.encode(value) : value);
const digest = (value) => createHash("sha256").update(bytesOf(value)).digest("hex");

// -------------------------------------------------------------- routing

assert.equal(localIdOf("?local=abc123"), "abc123");
assert.equal(localIdOf("?x=1&local=a-b_c"), "a-b_c");
assert.equal(localIdOf(""), "", "an ordinary address is not a local project");
assert.equal(localIdOf("?local="), "");
assert.equal(localIdOf("?local=../etc"), "", "an id is a token, never a path");
assert.equal(localSlug("abc123"), "local-abc123", "the project id of a folder");

// -------------------------------------------------------- standalone session

{
  const states = [];
  const session = createProjectSession({
    send: () => assert.fail("a standalone session sends nothing"),
    onState: (state) => states.push(state),
    standalone: true,
  });
  assert.equal(session.joined, true, "with nothing to hydrate it is joined at once");
  assert.equal(states.at(-1).joined, true);
  assert.equal(states.at(-1).pending, 0);
  session.addText("a.md", "hello");
  assert.equal(session.tree().texts["a.md"], "hello");
  assert.equal(states.at(-1).pending, 0, "an edit waits on no server");
  session.leave();
}

{
  const states = [];
  let release;
  const hydration = new Promise((resolve) => { release = resolve; });
  let events;
  const session = createProjectSession({
    send: () => assert.fail("a standalone session sends nothing"),
    onState: (state) => states.push(state),
    standalone: true,
    persistence: { open(_doc, given) { events = given; return { hydration, flush: async () => {}, close() {} }; } },
  });
  assert.equal(session.joined, false, "joined waits for the persistence to hydrate");
  assert.equal(states.at(-1).joined, false);
  release();
  await tick();
  assert.equal(session.joined, true);
  assert.equal(states.at(-1).joined, true);
  events.writing(2);
  assert.equal(states.at(-1).pending, 2, "pending follows the persistence adapter");
  events.persisted();
  assert.equal(states.at(-1).pending, 0);
  session.leave();
}

// ------------------------------------------------------------ a fake folder

function fakeFolder(initial) {
  const files = new Map(Object.entries(initial));
  const queued = [];
  let waiter = null;
  const folder = {
    files,
    skipped: [],
    treeReads: 0,
    fetched: [],
    treeCursor: "t0",
    firstSince: undefined,
    async projectTree(_id, extensions) {
      folder.extensions = extensions;
      folder.treeReads += 1;
      return {
        cursor: folder.treeCursor,
        files: [...files].map(([path, value]) => ({ path, size: bytesOf(value).length, sha256: digest(value), mtime: 0 })),
        skipped: [],
      };
    },
    async projectFile(_id, path) {
      folder.fetched.push(path);
      if (!files.has(path)) throw Object.assign(new Error("gone"), { name: "Refused", status: 404 });
      return bytesOf(files.get(path));
    },
    async projectName() { return "thesis"; },
    projectChanges(_id, since, { signal } = {}) {
      folder.firstSince ??= since;
      folder.lastSince = since;
      if (queued.length) return Promise.resolve(queued.shift());
      return new Promise((resolve, reject) => {
        waiter = resolve;
        signal?.addEventListener("abort", () => reject(Object.assign(new Error("Canceled"), { name: "Canceled" })), { once: true });
      });
    },
    // The folder reports these changes the next time it is asked.
    report(...changes) {
      const answer = { cursor: "1", changes };
      if (waiter) {
        const resolve = waiter;
        waiter = null;
        resolve(answer);
      } else queued.push(answer);
    },
    reportReset() {
      const answer = { cursor: 0, reset: true, changes: [] };
      if (waiter) {
        const resolve = waiter;
        waiter = null;
        resolve(answer);
      } else queued.push(answer);
    },
  };
  return folder;
}

// A folder that numbers its changes the way the desktop app does: `since` is
// the newest change handled, every later one is answered in order (the same
// ones again until the cursor moves), and a `since` below `dropped` is a reset.
// A read of a path in `failures` fails that many times first.
function numberedFolder(initial) {
  const folder = fakeFolder(initial);
  const events = [];
  let dropped = 0;
  let waiter = null;
  folder.sinces = [];
  folder.reads = [];
  folder.failures = new Map();
  folder.projectTree = async function projectTree(_id, extensions) {
    folder.extensions = extensions;
    folder.treeReads += 1;
    return {
      cursor: events.length,
      files: [...folder.files].map(([path, value]) => ({ path, size: bytesOf(value).length, sha256: digest(value), mtime: 0 })),
      skipped: [],
    };
  };
  const readFile = folder.projectFile;
  folder.projectFile = async function projectFile(given, path) {
    folder.reads.push(path);
    const left = folder.failures.get(path) || 0;
    if (left > 0) {
      folder.failures.set(path, left - 1);
      throw Object.assign(new Error("busy"), { name: "Refused", status: 404 });
    }
    return readFile(given, path);
  };
  folder.projectChanges = function projectChanges(_id, since, { signal } = {}) {
    folder.sinces.push(since);
    const answer = () => {
      if (since < dropped) return { cursor: events.length, reset: true, changes: [] };
      const later = events.filter((event) => event.id > since).map((event) => event.change);
      return later.length ? { cursor: events.length, changes: later } : null;
    };
    const now = answer();
    if (now) return Promise.resolve(now);
    return new Promise((resolve, reject) => {
      waiter = () => { const next = answer(); if (next) { waiter = null; resolve(next); } };
      signal?.addEventListener("abort", () => reject(Object.assign(new Error("Canceled"), { name: "Canceled" })), { once: true });
    });
  };
  folder.emit = (...changes) => {
    for (const change of changes) events.push({ id: events.length + 1, change });
    waiter?.();
  };
  // The desktop app drops its queue: every cursor handed out before is stale.
  folder.lose = () => {
    events.push({ id: events.length + 1, change: null });
    dropped = events.length;
    waiter?.();
  };
  return folder;
}

// A wait the test lets go of itself, one retry at a time.
function gatedWait() {
  const gates = [];
  const wait = () => new Promise((resolve) => gates.push(resolve));
  wait.gates = gates;
  wait.release = () => gates.shift()?.();
  return wait;
}

const RULES = { text_extensions: [".tex", ".md", ".bib"], asset_extensions: [".png"], derived_extensions: [".log"] };
const MAIN = "\\documentclass{article}\n\\begin{document}\nHello world.\n\\input{chapters/one}\n\\end{document}\n";
const PNG = new Uint8Array([137, 80, 78, 71, 1, 2, 3]);

async function open(initial, extra = {}) {
  const folder = extra.folder ?? fakeFolder(initial);
  const conflicts = [];
  const resends = [];
  const problems = [];
  const figures = createFigureStore();
  const project = createLocalProject({
    id: "abc",
    rules: RULES,
    collab,
    figures,
    folder,
    onConflict: (conflict) => conflicts.push(conflict),
    onResend: (paths) => resends.push(paths),
    onProblem: (message) => problems.push(message),
    wait: () => new Promise(() => {}),
    ...extra,
  });
  const loaded = await project.load();
  const session = project.start({});
  await tick();
  return { folder, conflicts, resends, problems, figures, project, session, loaded };
}

const textOf = (session, path) => session.textOf(session.idOf(path)).toString();

// ---------------------------------------------------- the document from a tree

{
  const { session, loaded, project, figures, folder } = await open({
    "main.tex": MAIN,
    "chapters/one.tex": "One.\n",
    "refs.bib": "@book{a}\n",
    "fig/a.png": PNG,
    // Left out: not in the rules, a dotfile, a derived name, not UTF-8 text.
    "notes.pdf": "%PDF",
    ".hidden.tex": "x",
    "build.log": "log",
    "bad.md": new Uint8Array([0xff, 0xfe, 0xfd]),
  });
  assert.equal(loaded.name, "thesis", "the title is the folder's name");
  assert.equal(loaded.sourceFormat, "latex", "the format follows the file the project opens on");
  assert.equal(session.joined, true);
  const tree = session.tree();
  assert.deepEqual(Object.keys(tree.texts).sort(), ["chapters/one.tex", "main.tex", "refs.bib"]);
  assert.equal(tree.texts["main.tex"], MAIN);
  assert.deepEqual(Object.keys(tree.digests), ["fig/a.png"]);
  assert.equal(tree.digests["fig/a.png"], digest(PNG));
  assert.deepEqual([...new Set(folder.extensions)].sort(), ["bib", "md", "png", "tex"], "the listing is asked for the rules' extensions, without dots");
  assert.ok(!folder.fetched.includes("fig/a.png"), "opening does not read a figure");
  assert.ok(folder.fetched.includes("main.tex"), "opening reads the texts");
  assert.equal(figures.size(tree.digests["fig/a.png"]), PNG.length, "the size comes from the listing");
  assert.equal(project.onDisk("fig/a.png"), digest(PNG));
  assert.equal(project.onDisk("main.tex"), digest(MAIN));
  assert.equal(project.onDisk("nothing.tex"), null);
  const held = await figures.gather("local-abc", tree.digests);
  assert.deepEqual(folder.fetched.filter((path) => path === "fig/a.png"), ["fig/a.png"], "gather reads the figure");
  assert.deepEqual([...held.assets["fig/a.png"]], [...PNG], "the figure bytes are held for the render and the folder sync");
  await figures.gather("local-abc", tree.digests);
  assert.deepEqual(folder.fetched.filter((path) => path === "fig/a.png"), ["fig/a.png"], "and keeps it by digest");
  assert.match(held.urls["fig/a.png"], /^data:image\/png;base64,/);
  assert.deepEqual(held.missing, []);
  assert.deepEqual(project.send({ type: "anything" }), undefined, "there is no room to write to");
  assert.deepEqual(project.syncStatus(), {});
  assert.equal(project.sendLive({}).ok, false);
  project.close();
}

// CRLF files come into the editor with LF endings.
{
  const { session, project } = await open({ "a.md": "one\r\ntwo\r\n" });
  assert.equal(textOf(session, "a.md"), "one\ntwo\n");
  project.close();
}

// A figure added in the editor is stored and read back through the same store.
{
  const figures = createFigureStore();
  const { sha } = await figures.upload("local-abc", new File([PNG], "b.png"));
  assert.equal(sha, digest(PNG));
  const held = await figures.gather("local-abc", { "b.png": sha, "gone.png": "0".repeat(64) });
  assert.deepEqual(held.missing, ["gone.png"]);
  assert.deepEqual([...held.assets["b.png"]], [...PNG]);
}

// ----------------------------------------------------------- outside changes

// A file the editor has not touched is replaced by a small edit, so the rest of
// the document, other files and the file's identity stay where they are.
{
  const { folder, session, conflicts, project } = await open({ "main.tex": MAIN, "chapters/one.tex": "One.\n" });
  const id = session.idOf("main.tex");
  const cursor = session.textOf(id).getCursor(MAIN.indexOf("Hello"));
  const next = MAIN.replace("Hello world.", "Hello there, world.");
  folder.files.set("main.tex", next);
  folder.report({ path: "main.tex", kind: "changed" });
  await tick();
  assert.equal(textOf(session, "main.tex"), next);
  assert.equal(session.idOf("main.tex"), id, "the file keeps its identity");
  assert.equal(textOf(session, "chapters/one.tex"), "One.\n", "other files stay as they were");
  assert.equal(conflicts.length, 0, "a clean file is not asked about");
  const resolved = session.doc.getCursorPos(cursor).offset;
  assert.equal(next.slice(resolved, resolved + 5), "Hello", "a cursor on unchanged words stays on them");
  project.close();
}

// Changes made on disk with the same text as the editor change nothing.
{
  const { folder, session, conflicts, project } = await open({ "a.md": "same\n" });
  folder.report({ path: "a.md", kind: "changed" });
  await tick();
  assert.equal(textOf(session, "a.md"), "same\n");
  assert.equal(conflicts.length, 0);
  project.close();
}

// An unsaved edit makes the person choose. "Load from disk" replaces the editor's
// text, and "Keep mine" leaves it and asks for a write.
{
  const { folder, session, conflicts, resends, project } = await open({ "a.md": "base\n" });
  const edit = (text) => {
    session.textOf(session.idOf("a.md")).insert(0, text);
    session.doc.commit();
  };
  edit("mine ");
  folder.files.set("a.md", "base from disk\n");
  folder.report({ path: "a.md", kind: "changed" });
  await tick();
  assert.equal(conflicts.length, 1);
  assert.equal(conflicts[0].path, "a.md");
  assert.equal(textOf(session, "a.md"), "mine base\n", "nothing changes before the person answers");

  // The disk moves on again while the question stands: the answer is for the newest text.
  folder.files.set("a.md", "base from disk again\n");
  folder.report({ path: "a.md", kind: "changed" });
  await tick();
  assert.equal(conflicts.length, 1, "one question per file");
  conflicts[0].loadDisk();
  assert.equal(textOf(session, "a.md"), "base from disk again\n");
  conflicts[0].loadDisk();
  assert.equal(textOf(session, "a.md"), "base from disk again\n", "a question is answered once");

  edit("mine ");
  folder.files.set("a.md", "third\n");
  folder.report({ path: "a.md", kind: "changed" });
  await tick();
  assert.equal(conflicts.length, 2);
  conflicts[1].keepMine();
  assert.equal(textOf(session, "a.md"), "mine base from disk again\n");
  assert.deepEqual(resends, [["a.md"]], "keeping mine asks for the editor's text to be written");
  project.close();
}

// Once the folder sync has written the editor's text, the file is clean again.
{
  const { folder, session, conflicts, project } = await open({ "a.md": "base\n" });
  session.textOf(session.idOf("a.md")).insert(0, "mine ");
  session.doc.commit();
  project.wrote({ texts: session.tree().texts, digests: {} });
  folder.files.set("a.md", "mine base and more\n");
  folder.report({ path: "a.md", kind: "changed" });
  await tick();
  assert.equal(conflicts.length, 0, "the text last written to disk is the baseline");
  assert.equal(textOf(session, "a.md"), "mine base and more\n");
  project.close();
}

// A file the editor made and the folder has never held is unsaved by definition.
{
  const { folder, session, conflicts, project } = await open({ "a.md": "a\n" });
  session.addText("new.md", "mine");
  folder.files.set("new.md", "theirs");
  folder.report({ path: "new.md", kind: "added" });
  await tick();
  assert.equal(conflicts.length, 1);
  assert.equal(textOf(session, "new.md"), "mine");
  project.close();
}

// Files that appear join the project; files outside the rules do not.
{
  const { folder, session, project } = await open({ "a.md": "a\n" });
  folder.files.set("b.md", "# b\n");
  folder.files.set("fig.png", PNG);
  folder.files.set("notes.pdf", "%PDF");
  folder.report(
    { path: "b.md", kind: "added" },
    { path: "fig.png", kind: "added" },
    { path: "notes.pdf", kind: "added" },
  );
  await until(() => session.list().length === 3);
  assert.deepEqual(session.list().map((file) => file.path), ["a.md", "b.md", "fig.png"]);
  assert.equal(textOf(session, "b.md"), "# b\n");
  assert.equal(session.tree().digests["fig.png"], digest(PNG));
  project.close();
}

// A removed file leaves the project, text and asset alike.
{
  const { folder, session, project } = await open({ "a.md": "a\n", "b.md": "b\n", "fig.png": PNG });
  folder.files.delete("b.md");
  folder.files.delete("fig.png");
  folder.report({ path: "b.md", kind: "removed" }, { path: "fig.png", kind: "removed" });
  await tick();
  assert.deepEqual(session.list().map((file) => file.path), ["a.md"]);
  project.close();
}

// An asset that changed is replaced by its new bytes.
{
  const { folder, session, figures, project } = await open({ "a.md": "a\n", "fig.png": PNG });
  const next = new Uint8Array([9, 9, 9]);
  folder.files.set("fig.png", next);
  folder.report({ path: "fig.png", kind: "changed" });
  await until(() => session.tree().digests["fig.png"] === digest(next));
  assert.equal(session.tree().digests["fig.png"], digest(next));
  const held = await figures.gather("local-abc", session.tree().digests);
  assert.deepEqual([...held.assets["fig.png"]], [...next]);
  project.close();
}

// A change that carries its digest and size does not read the figure: the
// figure store reads it when its bytes are asked for.
{
  const { folder, session, figures, project } = await open({ "a.md": "a\n", "fig.png": PNG });
  const next = new Uint8Array([9, 9, 9]);
  folder.files.set("fig.png", next);
  folder.report({ path: "fig.png", kind: "changed", sha256: digest(next), size: next.length });
  await until(() => session.tree().digests["fig.png"] === digest(next));
  assert.equal(session.tree().digests["fig.png"], digest(next));
  assert.ok(!folder.fetched.includes("fig.png"), "the poller's change is not read");
  assert.equal(project.onDisk("fig.png"), digest(next));
  assert.equal(figures.size(digest(next)), next.length);
  const held = await figures.gather("local-abc", session.tree().digests);
  assert.deepEqual([...held.assets["fig.png"]], [...next]);
  assert.deepEqual(folder.fetched, ["a.md", "fig.png"], "gather reads it once");
  project.close();
}

// A figure whose bytes do not match the digest, or that cannot be read, is
// reported missing and is not cached.
{
  const { folder, figures, session, project } = await open({ "a.md": "a\n", "fig.png": PNG });
  const digests = { ...session.tree().digests };
  folder.files.set("fig.png", new Uint8Array([7, 7]));
  const stale = await figures.gather("local-abc", digests);
  assert.deepEqual(stale.missing, ["fig.png"], "the file on disk is not the one the digest names");
  assert.deepEqual(Object.keys(stale.assets), []);
  folder.files.delete("fig.png");
  const gone = await figures.gather("local-abc", digests);
  assert.deepEqual(gone.missing, ["fig.png"]);
  folder.files.set("fig.png", PNG);
  assert.deepEqual((await figures.gather("local-abc", digests)).missing, [], "a later read that matches succeeds");
  project.close();
}

// What the folder sync wrote is what the folder now holds.
{
  const { project, session } = await open({ "a.md": "a\n", "fig.png": PNG });
  const sha = "b".repeat(64);
  project.wrote({ texts: { "a.md": "a\n" }, digests: { "fig.png": sha } });
  assert.equal(project.onDisk("fig.png"), sha);
  assert.equal(project.onDisk("a.md"), null, "a text written by the sync has no digest recorded");
  assert.ok(session);
  project.close();
}

// A reset rereads the tree and applies the difference.
{
  const { folder, session, conflicts, project } = await open({
    "a.md": "a\n",
    "gone.md": "gone\n",
    "same.md": "same\n",
    "fig.png": PNG,
  });
  folder.files.set("a.md", "a changed\n");
  folder.files.delete("gone.md");
  folder.files.set("fresh.md", "fresh\n");
  folder.files.set("fig.png", new Uint8Array([4, 5]));
  assert.equal(folder.firstSince, "t0", "the first request resumes from the tree's cursor");
  folder.treeCursor = "t1";
  folder.reportReset();
  // The reread hashes a figure, which leaves the event loop.
  await until(() => folder.lastSince === "t1");
  assert.equal(folder.treeReads, 2, "a reset rereads the tree once and no more");
  assert.equal(folder.lastSince, "t1", "and continues from the new tree cursor, not the reset's");
  assert.deepEqual(session.list().map((file) => file.path), ["a.md", "fig.png", "fresh.md", "same.md"]);
  assert.equal(textOf(session, "a.md"), "a changed\n");
  assert.equal(textOf(session, "same.md"), "same\n");
  assert.equal(textOf(session, "fresh.md"), "fresh\n");
  assert.equal(session.tree().digests["fig.png"], digest(new Uint8Array([4, 5])));
  assert.equal(conflicts.length, 0);
  project.close();
}

// A file that cannot be read keeps the cursor where it was. The batch comes
// again on the next poll, and what was applied before the failure is not applied
// a second time.
{
  const wait = gatedWait();
  const folder = numberedFolder({ "a.md": "a\n", "b.md": "b\n", "c.md": "c\n" });
  const { session, conflicts, problems, project } = await open({}, { folder, wait });
  folder.reads.length = 0;
  const edit = (path) => {
    session.textOf(session.idOf(path)).insert(0, "mine ");
    session.doc.commit();
  };
  edit("a.md");
  folder.files.delete("a.md");
  folder.files.set("b.md", "b from disk\n");
  folder.files.set("c.md", "c from disk\n");
  folder.failures.set("b.md", 1);
  folder.emit(
    { path: "a.md", kind: "removed" },
    { path: "b.md", kind: "changed" },
    { path: "c.md", kind: "changed" },
  );
  await tick();
  assert.deepEqual(folder.sinces, [0], "the first poll resumes from the tree's cursor");
  assert.equal(textOf(session, "b.md"), "b\n", "the failed read changed nothing");
  assert.equal(textOf(session, "c.md"), "c\n", "the batch stops at the file it could not read");
  assert.equal(problems.length, 1);
  assert.match(problems[0], /Could not read b\.md/);
  assert.equal(conflicts.length, 1, "the removal was applied and the person was asked");
  assert.equal(conflicts[0].deleted, true);
  assert.equal(wait.gates.length, 1, "the loop waits before asking again");

  // The person answers while the batch is outstanding. Applying the removal a
  // second time would now drop the file they chose to keep.
  conflicts[0].keepMine();
  wait.release();
  await tick();
  assert.equal(folder.sinces[1], 0, "the cursor did not move, so the batch came again");
  assert.deepEqual(folder.reads, ["b.md", "b.md", "c.md"], "only the unread files were read again");
  assert.equal(textOf(session, "b.md"), "b from disk\n", "the retried read was applied");
  assert.equal(textOf(session, "c.md"), "c from disk\n");
  assert.equal(conflicts.length, 1, "the removal was applied once, not twice");
  assert.equal(textOf(session, "a.md"), "mine a\n", "the kept file is still there");
  assert.equal(problems.length, 1, "the same failure is said once");
  assert.deepEqual(folder.sinces, [0, 0, 3], "once the batch is applied in full the cursor is the batch's");
  folder.files.set("d.md", "d\n");
  folder.emit({ path: "d.md", kind: "added" });
  await tick();
  assert.equal(textOf(session, "d.md"), "d\n");
  assert.equal(folder.treeReads, 1, "a retry that works never rereads the tree");
  project.close();
}

// Failures that go on end in a reread of the whole tree, which finds the
// content the poller will not report again.
{
  const wait = gatedWait();
  const folder = numberedFolder({ "a.md": "a\n", "b.md": "b\n" });
  const { session, problems, project } = await open({}, { folder, wait });
  folder.files.set("a.md", "a from disk\n");
  folder.files.set("b.md", "b from disk\n");
  folder.failures.set("a.md", 3);
  folder.emit({ path: "a.md", kind: "changed" }, { path: "b.md", kind: "changed" });
  await tick();
  assert.equal(folder.treeReads, 1);
  wait.release();
  await tick();
  assert.equal(folder.treeReads, 1, "two failures in a row are still retried");
  wait.release();
  await tick();
  assert.equal(folder.treeReads, 2, "the third failure rereads the tree");
  assert.equal(textOf(session, "a.md"), "a from disk\n");
  assert.equal(textOf(session, "b.md"), "b from disk\n");
  assert.deepEqual(folder.sinces, [0, 0, 0, 2], "the loop goes on from the tree's cursor");
  folder.files.set("z.md", "z\n");
  folder.emit({ path: "z.md", kind: "added" });
  await tick();
  assert.equal(folder.sinces.at(-1), 3);
  assert.equal(textOf(session, "z.md"), "z\n");
  assert.equal(problems.length, 1, "and said so once");
  assert.equal(wait.gates.length, 0);
  project.close();
}

// A reset whose reread cannot read a file leaves the cursor and tries again.
{
  const wait = gatedWait();
  const folder = numberedFolder({ "a.md": "a\n", "b.md": "b\n" });
  const { session, problems, project } = await open({}, { folder, wait });
  folder.files.set("a.md", "a from disk\n");
  folder.files.delete("b.md");
  folder.failures.set("a.md", 1);
  folder.lose();
  await tick();
  assert.deepEqual(folder.sinces, [0]);
  assert.equal(folder.treeReads, 2, "the reset read the tree");
  assert.equal(textOf(session, "a.md"), "a\n", "the failed read changed no text");
  assert.equal(problems.length, 1);
  assert.equal(wait.gates.length, 1);
  wait.release();
  await tick();
  assert.deepEqual(folder.sinces, [0, 0, 1], "the cursor stayed, the reset came again, and the second tree read moved it");
  assert.equal(folder.treeReads, 3);
  assert.equal(textOf(session, "a.md"), "a from disk\n");
  assert.deepEqual(session.list().map((file) => file.path), ["a.md"]);
  assert.equal(problems.length, 1, "the same failure is said once");
  project.close();
}

// A failing folder is reported once, and closing stops the loop.
{
  let calls = 0;
  const waits = [];
  const { problems, project } = await open({ "a.md": "a\n" }, {
    folder: Object.assign(fakeFolder({ "a.md": "a\n" }), {
      projectChanges(_id, since) {
        calls += 1;
        return Promise.reject(Object.assign(new Error("unreachable"), { name: "Unreachable" }));
      },
    }),
    wait: async () => { waits.push(1); if (waits.length >= 3) await new Promise(() => {}); },
  });
  await tick();
  assert.equal(problems.length, 1, "the same failure is said once");
  assert.match(problems[0], /could not be read/);
  assert.equal(calls, 3, "the loop asks again after each wait");
  project.close();
}

// A file another program deleted: clean files leave, files with unsaved edits
// are held out of the folder sync and the person is asked.
{
  const { folder, session, conflicts, resends, project } = await open({ "a.md": "a\n", "b.md": "b\n", "c.md": "c\n" });
  const edit = (path) => {
    session.textOf(session.idOf(path)).insert(0, "mine ");
    session.doc.commit();
  };
  const sent = () => Object.keys(project.withoutHeld(session.tree()).texts).sort();

  folder.files.delete("a.md");
  folder.report({ path: "a.md", kind: "removed" });
  await tick();
  assert.deepEqual(session.list().map((file) => file.path), ["b.md", "c.md"], "a clean file leaves the project");
  assert.equal(conflicts.length, 0);

  edit("b.md");
  folder.files.delete("b.md");
  folder.report({ path: "b.md", kind: "removed" });
  await tick();
  assert.equal(textOf(session, "b.md"), "mine b\n", "an edited file stays");
  assert.equal(conflicts.length, 1);
  assert.equal(conflicts[0].deleted, true);
  assert.equal(conflicts[0].path, "b.md");
  assert.deepEqual(sent(), ["c.md"], "and is left out of what the folder sync sends");

  conflicts[0].keepMine();
  assert.deepEqual(resends, [["b.md"]]);
  assert.deepEqual(sent(), ["b.md", "c.md"], "keeping mine puts it back in the manifest");

  edit("c.md");
  folder.files.delete("c.md");
  folder.report({ path: "c.md", kind: "removed" });
  await tick();
  assert.equal(conflicts.length, 2);
  conflicts[1].loadDisk();
  assert.deepEqual(session.list().map((file) => file.path), ["b.md"], "Delete drops the file");
  assert.deepEqual(sent(), ["b.md"]);
  project.close();
}

// A held file that comes back on disk is the changed-on-disk question.
{
  const { folder, session, conflicts, project } = await open({ "a.md": "a\n" });
  session.textOf(session.idOf("a.md")).insert(0, "mine ");
  session.doc.commit();
  folder.files.delete("a.md");
  folder.report({ path: "a.md", kind: "removed" });
  await tick();
  assert.equal(conflicts[0].deleted, true);
  folder.files.set("a.md", "back\n");
  folder.report({ path: "a.md", kind: "added" });
  await tick();
  assert.equal(conflicts.length, 2);
  assert.equal(conflicts[1].deleted, false);
  assert.deepEqual(Object.keys(project.withoutHeld(session.tree()).texts), ["a.md"], "it is no longer held");
  conflicts[0].loadDisk();
  assert.equal(textOf(session, "a.md"), "mine a\n", "the old deletion question is void");
  project.close();
}

// Line endings: a CRLF file is written back as CRLF, byte for byte when it was
// not edited, and an outside CRLF change reaches the editor as LF text.
{
  const bom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61, 0x0a, 0x62, 0x0d, 0x0a]);
  const { folder, session, project } = await open({ "w.md": "one\r\ntwo\r\n", "u.md": "x\ny\n", "m.md": bom });
  const seeded = project.seed();
  assert.equal(seeded["w.md"], digest("one\r\ntwo\r\n"), "the opening sync can find the folder's own digests");
  assert.equal(seeded["m.md"], digest(bom));
  const bytes = (path) => project.encode(path, textOf(session, path));
  assert.equal(textOf(session, "w.md"), "one\ntwo\n");
  assert.equal(dec.decode(bytes("w.md")), "one\r\ntwo\r\n", "an unedited CRLF file round-trips");
  assert.deepEqual([...bytes("m.md")], [...bom], "so does a file the style cannot rebuild");
  assert.equal(dec.decode(bytes("u.md")), "x\ny\n");
  session.textOf(session.idOf("w.md")).insert(0, "zero\n");
  session.doc.commit();
  assert.equal(dec.decode(bytes("w.md")), "zero\r\none\r\ntwo\r\n", "an edit is written with CRLF");
  assert.equal(dec.decode(project.encode("new.md", "a\nb")), "a\nb", "a new file is LF");

  folder.files.set("u.md", "p\r\nq\r\n");
  folder.report({ path: "u.md", kind: "changed" });
  await tick();
  assert.equal(textOf(session, "u.md"), "p\nq\n", "disk text is applied as LF");
  assert.equal(dec.decode(bytes("u.md")), "p\r\nq\r\n", "and the file is now known as CRLF");
  project.close();
}

console.log("local-project: routing, standalone session, tree build, outside-change rules");
