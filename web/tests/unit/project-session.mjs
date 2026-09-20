import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";
import { assertSameProject, projectIdentity, sameProject } from "../../src/lib/project-identity.js";

const states = [];
let persistenceEvents;
let closed = false;
const sent = [];
const session = createProjectSession({
  send: (message) => sent.push(message),
  onState: (state) => states.push(state),
  presenceId: () => "test-presence",
  persistence: {
    open(_doc, events) {
      persistenceEvents = events;
      return { close: () => { closed = true; } };
    },
  },
});

try {
  assert.equal(states.at(-1).local, false);
  persistenceEvents.hydrated();
  assert.equal(states.at(-1).local, true);
  persistenceEvents.writing(2);
  assert.equal(states.at(-1).localPending, 2);
  persistenceEvents.persisted();
  assert.equal(states.at(-1).localPending, 0);

  session.addText("paper.md", "offline");
  assert.equal(states.at(-1).pending, 1);
  assert.equal(sent.at(-1).type, "doc-update");
  session.acknowledge(sent.at(-1).seq, "AA==");
  assert.equal(states.at(-1).pending, 0);

  persistenceEvents.failed(new Error("quota full"));
  assert.equal(states.at(-1).localError, "quota full");
  assert.equal(states.at(-1).local, false);
} finally {
  session.leave();
}
assert.equal(closed, true);

// A swap is the main file becoming a different file. Editing the main file is
// not one: everything hanging off `onSwap` rebuilds itself, so a swap per
// keystroke used to throw the editor -- and the caret with it -- back to the
// top of the file on every commit.
const swapping = createProjectSession({ send: () => {}, onState: () => {} });
try {
  const main = swapping.addText("paper.md", "hello");
  swapping.setMain(main);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  let swaps = 0;
  swapping.onSwap(() => { swaps += 1; });
  swapping.textOf(main).insert(5, " world");
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 0);
  assert.equal(swapping.text.toString(), "hello world");

  const other = swapping.addText("other.md", "x");
  swapping.setMain(other);
  swapping.doc.commit();
  await new Promise((resume) => setTimeout(resume, 20));
  assert.equal(swaps, 1);
  assert.equal(swapping.text.toString(), "x");
} finally {
  swapping.leave();
}

const first = projectIdentity({ server: "https://paper.example/path", slug: "paper", createdAt: "one" });
const same = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "one" });
const recreated = projectIdentity({ server: "https://paper.example", slug: "paper", createdAt: "two" });
assert.equal(sameProject(first, same), true);
assert.throws(() => assertSameProject(first, recreated), { code: "project-identity-changed" });

// What counts as a change to the directory. The file list moves with four
// maps -- the texts, their paths, the figures and the metadata -- and only the
// session knows which containers those are. A reader that decided for itself,
// by watching the texts alone, drew every rename and every move as no change
// at all: the name moved in `paths`, the list was never told, and the file
// kept its old name on screen until the page was reloaded.
{
  const rules = { extensions: [".md"], text_extensions: [".md"], asset_extensions: [], derived_extensions: [],
    max_path: 200, max_files: 200, max_document: 1000000, max_assets: 1000000, max_asset: 500000 };
  const directory = createProjectSession({ send: () => {}, onState: () => {} });
  try {
    const main = directory.addText("paper.md", "hello");
    directory.setMain(main);
    directory.doc.commit();
    await new Promise((resume) => setTimeout(resume, 20));
    const batches = [];
    const stop = directory.onFiles((events) => batches.push(directory.directoryChanged(events)));

    directory.relocate([{ id: main, kind: "text", path: "paper.md" }], "renamed.md", rules, true);
    await new Promise((resume) => setTimeout(resume, 20));
    assert.deepEqual(directory.list().map((file) => file.path), ["renamed.md"]);
    assert.equal(batches.at(-1), true, "a rename is a change to the directory");

    directory.addFolder("chapters", rules);
    await new Promise((resume) => setTimeout(resume, 20));
    assert.equal(batches.at(-1), true, "a new folder is a change to the directory");

    const seen = batches.length;
    directory.textOf(main).insert(5, " there");
    directory.doc.commit();
    await new Promise((resume) => setTimeout(resume, 20));
    assert.equal(batches.length, seen + 1, "typing still reaches the watcher, for the preview");
    assert.equal(batches.at(-1), false, "but typing inside a file is not a change to the directory");
    stop();
  } finally {
    directory.leave();
  }
}

console.log("project-session: persistence, identity, swap, directory changes and acknowledgement boundaries passed");
