// The project directory, driven directly.
//
// The workspace owns the file list, so it can be built here and exercised
// without a component, a VM realm, or a browser. What is checked is what used
// to be checked by slicing Reader.svelte's `refreshFiles` out of its source:
// what happens to the open file when the directory changes underneath it.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createWorkspace } = await loadRunes(new URL("../../src/lib/reader/workspace.svelte.js", import.meta.url));

const RULES = { text_extensions: [".md", ".tex"], asset_extensions: [".png"], derived_extensions: [".log"] };

function fakeSession(entries = [], folders = []) {
  const session = {
    files: entries,
    folderList: folders,
    added: [],
    assets: [],
    removed: [],
    mained: [],
    list: () => session.files,
    folders: () => session.folderList,
    mainId: () => session.files[0]?.id || "",
    mainPath: () => session.files[0]?.path || "",
    addText(path, text) {
      const file = { id: `id:${path}`, path, kind: "text" };
      session.files = [...session.files, file];
      session.added.push({ path, text });
      return file.id;
    },
    putAsset(path, sha) {
      session.files = [...session.files, { id: path, path, kind: "asset", sha }];
      session.assets.push({ path, sha });
    },
    removeEntries: (entries_) => session.removed.push(entries_),
    setMain: (id) => session.mained.push(id),
    addFolder: (path, rules) => session.folderList.push({ path, rules }),
    duplicateEntry: (entry, path, rules) => session.folderList.push({ entry, path, rules }),
    relocate: () => ({ files: [{ path: "b.md", previousPath: "a.md" }] }),
    whereEveryoneIs: () => new Map([["id:main.md", ["ada"]]]),
    participants: () => ["ada"],
    tree: () => ({ digests: { "fig.png": "sha-fig" } }),
  };
  return session;
}

function build(extra = {}) {
  const said = [];
  const events = { paints: 0, retargets: 0, arrived: [] };
  const workspace = createWorkspace({
    slug: "paper",
    key: "",
    say: (message) => said.push(message),
    paint: () => events.paints++,
    retarget: () => events.retargets++,
    onarrived: (file) => events.arrived.push(file.path),
    upload: async () => ({ sha: "sha-new" }),
    gather: async (_slug, digests) => ({
      assets: {},
      urls: Object.fromEntries(Object.keys(digests).map((path) => [path, `blob:${path}`])),
    }),
    ...extra,
  });
  workspace.state.rules = RULES;
  workspace.state.canEdit = true;
  return { workspace, said, events };
}

// The directory read: the list and the folders come from the session, and
// re-pointing the preview happens after, once, on every read.
{
  const { workspace, events } = build();
  const session = fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }]);
  workspace.attach(session);
  // Attaching does not read: the first read is the caller's to schedule,
  // because it is the one that re-points the preview.
  assert.deepEqual(workspace.state.files, []);
  assert.equal(events.retargets, 0);
  workspace.refresh();
  assert.deepEqual(workspace.state.files.map((file) => file.path), ["main.md"]);
  assert.equal(workspace.state.openFile, "id:main.md", "with nothing open, the main file is");
  assert.equal(events.retargets, 1);
}

// A file that went away under this browser leaves the editor showing
// something that is not there any more, so it falls back to the document.
{
  const { workspace } = build();
  const session = fakeSession([
    { id: "id:main.md", path: "main.md", kind: "text" },
    { id: "id:other.md", path: "other.md", kind: "text" },
  ]);
  workspace.attach(session);
  workspace.refresh();
  workspace.state.openFile = "id:other.md";
  session.files = session.files.filter((file) => file.id !== "id:other.md");
  workspace.refresh();
  assert.equal(workspace.state.openFile, "id:main.md", "a deleted open file falls back to the main one");
}

// A figure renamed by somebody else is still the figure being looked at, and
// stays open if it was open.
{
  const { workspace } = build();
  const figure = { id: "fig.png", path: "fig.png", kind: "asset", sha: "sha-fig" };
  const session = fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }, figure]);
  workspace.attach(session);
  workspace.refresh();
  workspace.show(figure);
  assert.equal(workspace.state.figure.path, "fig.png");
  session.files = [session.files[0], { ...figure, id: "moved.png", path: "moved.png" }];
  workspace.refresh();
  assert.equal(workspace.state.figure.path, "moved.png", "the renamed figure is followed by its digest");
  assert.equal(workspace.state.openFile, "moved.png", "and stays the open file");
}

// The file a link named is opened once, and only for somebody who has a
// source pane to open it in.
{
  const entries = [{ id: "id:main.md", path: "main.md", kind: "text" }, { id: "id:ch.md", path: "ch.md", kind: "text" }];
  const reader = build({ arrivedFile: "ch.md" });
  reader.workspace.state.canEdit = false;
  reader.workspace.attach(fakeSession([...entries]));
  reader.workspace.refresh();
  assert.deepEqual(reader.events.arrived, [], "a reader has no pane to open the named file in");

  const editor = build({ arrivedFile: "ch.md" });
  editor.workspace.attach(fakeSession([...entries]));
  editor.workspace.refresh();
  editor.workspace.refresh();
  assert.deepEqual(editor.events.arrived, ["ch.md"], "an editor opens it, once");
}

// Writing is guarded here as well as at the server, so a stale menu cannot
// reach past a read-only session.
{
  const { workspace } = build();
  workspace.attach(fakeSession());
  workspace.state.canEdit = false;
  assert.throws(() => workspace.addText("new.md"), /read-only/);
  await assert.rejects(workspace.addFigure({ name: "fig.png" }), /read-only/);
  await assert.rejects(workspace.addDroppedText({ name: "new.md", text: async () => "x" }), /read-only/);
}

// Adding: the placement rules are applied before anything is written, and
// what was added becomes the open file.
{
  const { workspace, events } = build();
  const session = fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }]);
  workspace.attach(session);
  workspace.addText("chapter.md");
  assert.equal(workspace.state.openFile, "id:chapter.md");
  assert.deepEqual(session.added, [{ path: "chapter.md", text: "" }]);
  assert.equal(events.paints, 1, "a new file changes what a compiler would produce");
  assert.throws(() => workspace.addText("notes.exe"), /texts and figures/);
  assert.throws(() => workspace.addText("main.md"), /already a file or folder/);
  assert.throws(() => workspace.addText("build.log"), /a compiler writes/);
}

// A figure: the bytes go to the store and the name goes into the shared
// document, in that order, and an upload that lands in a session that has
// since changed installs nothing.
{
  const { workspace } = build();
  const session = fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }]);
  workspace.attach(session);
  const path = await workspace.addFigure({ name: "figure.png" });
  assert.equal(path, "figure.png");
  assert.deepEqual(session.assets, [{ path: "figure.png", sha: "sha-new" }]);
}
{
  const swapped = build({
    upload: async () => {
      swapped.workspace.attach(fakeSession());
      return { sha: "sha-new" };
    },
  });
  swapped.workspace.attach(fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }]));
  await assert.rejects(swapped.workspace.addFigure({ name: "figure.png" }), /editing session changed/);
}

// A text dropped is read before it is placed, and the same session check
// applies: reading a file yields to other editors.
{
  const { workspace, events } = build();
  const session = fakeSession([{ id: "id:main.md", path: "main.md", kind: "text" }]);
  workspace.attach(session);
  await workspace.addDroppedText({ name: "notes.md", text: async () => "dropped" });
  assert.deepEqual(session.added, [{ path: "notes.md", text: "dropped" }]);
  assert.equal(workspace.state.openFile, "id:notes.md");
  assert.equal(events.paints, 1);
}

// Moving files says so: a reference in a source file is not rewritten, and
// somebody has to know that.
{
  const { workspace, said } = build();
  workspace.attach(fakeSession());
  workspace.relocate([{ path: "a.md" }], "folder", false);
  assert.match(said[0], /References in source files are not changed automatically/);
}

// Removing and re-maining both change what a compiler would produce.
{
  const { workspace, events } = build();
  const session = fakeSession();
  workspace.attach(session);
  workspace.remove([{ path: "a.md" }]);
  workspace.setMain({ id: "id:b.md", kind: "text" });
  workspace.setMain({ id: "fig.png", kind: "asset" });
  assert.equal(session.removed.length, 1);
  assert.deepEqual(session.mained, ["id:b.md"], "a figure cannot be the main file");
  assert.equal(events.paints, 2);
}

// Where everyone is, and who they are, are read together.
{
  const { workspace } = build();
  workspace.attach(fakeSession());
  workspace.refreshPeers();
  assert.deepEqual([...workspace.state.peersByFile.keys()], ["id:main.md"]);
  assert.deepEqual(workspace.state.participants, ["ada"]);
}

// The figure being looked at has its bytes fetched, and a fetch that comes
// back for a figure nobody is looking at any more is dropped.
{
  let settle;
  const { workspace } = build({
    gather: () => new Promise((resolve) => { settle = () => resolve({ assets: {}, urls: { "fig.png": "blob:fig" } }); }),
  });
  workspace.attach(fakeSession());
  workspace.state.figure = { path: "fig.png", sha: "sha-fig" };
  workspace.watchFigure();
  workspace.state.figure = null;
  settle();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(workspace.state.figureUrl, "", "a figure nobody is looking at gets no URL");
}

// What the toolbar names, and whether this browser could preview it.
{
  const { workspace } = build();
  const session = fakeSession([
    { id: "id:main.md", path: "main.md", kind: "text" },
    { id: "fig.png", path: "fig.png", kind: "asset" },
  ]);
  workspace.attach(session);
  workspace.refresh();
  assert.equal(workspace.openPath(), "main.md");
  assert.equal(workspace.canPreview(), true);
  workspace.state.openFile = "fig.png";
  assert.equal(workspace.openPath(), "fig.png");
  assert.equal(workspace.canPreview(), false, "a figure is not a document to preview");
}

console.log("workspace: the directory, its rules, and what is open in it");
