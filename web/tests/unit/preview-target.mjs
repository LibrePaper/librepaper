// Which file the preview draws, and the changes that move, empty or lock it.
// The module is loaded as the page compiles it. The texts and the open file are
// plain values the checks change, the rule for the target is the real one from
// lib/opening.js, the frame's facts are a real createPainted(), and every effect
// is a counter.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createPainted } = await loadRunes(new URL("../../src/lib/reader/painted.svelte.js", import.meta.url));
const { createPreviewTarget } = await loadRunes(new URL("../../src/lib/reader/preview-target.svelte.js", import.meta.url));

const formatOf = (path) => (path.endsWith(".tex") ? "latex" : path.endsWith(".md") ? "markdown" : "");
const nextTurn = () => new Promise((resolve) => setImmediate(resolve));

// A page: the project's files and texts, the open file, the session and the
// effects. `world` is what a check moves; `calls` counts what the effects saw.
function page({ files = [], texts = {} } = {}) {
  const world = { files, texts, openFile: "", session: true, project: null, mayEdit: true, viewing: false, disposed: false };
  const calls = { configure: [], warm: [], invalidate: 0, clearFrame: 0, cancelDebounce: 0, resetStatus: 0, cleared: 0, navigateFrame: 0, paint: 0 };
  const painted = createPainted();
  const target = createPreviewTarget({
    texts: () => world.texts,
    formatOf,
    open: () => ({
      textPath: world.files.find((file) => file.id === world.openFile && file.kind === "text")?.path || "",
      hasFile: Boolean(world.openFile),
    }),
    lockedPath: () => world.files.find((file) => file.id === target.state.lockedFile)?.path || "",
    skip: () => (!world.session && !world.project) || (!world.mayEdit && world.viewing),
    painted: () => painted,
    configure: (format) => calls.configure.push(format),
    warm: (format, text) => calls.warm.push([format, text]),
    invalidate: () => calls.invalidate++,
    clearFrame: () => calls.clearFrame++,
    navigateFrame: () => calls.navigateFrame++,
    cancelDebounce: () => calls.cancelDebounce++,
    resetStatus: () => calls.resetStatus++,
    cleared: () => calls.cleared++,
    paint: () => calls.paint++,
    settle: () => Promise.resolve(),
    disposed: () => world.disposed,
  });
  const open = (id) => {
    world.openFile = id;
    target.update();
  };
  return { target, world, calls, painted, open };
}

// 1. Pointing the preview at a LaTeX file configures the already joined
// project, even though the initial prepare ran while it was Markdown.
{
  const { target, calls, open } = page({
    files: [{ id: "paper.tex", path: "paper.tex", kind: "text" }],
    texts: { "paper.tex": "\\documentclass{article}" },
  });
  target.state.format = "markdown";
  open("paper.tex");
  assert.deepEqual(calls.configure, ["latex"], "the format is configured once, for the file it now draws");
  assert.equal(target.state.path, "paper.tex");
  assert.equal(target.state.reason, "alone");
  assert.equal(target.state.format, "latex");
}

// 2. The preview follows the open file. A file that cannot render alone is
// shown inside the one file that includes it, a file with nothing to draw
// empties the preview, and a lock keeps the preview on one file while the
// source moves on.
{
  const texts = {
    "thesis.tex": "\\documentclass{book}\n\\input{chapters/intro}",
    "chapters/intro.tex": "Introduction",
    "appendix.tex": "\\documentclass{article}",
    "refs.bib": "@book{a}",
  };
  const files = Object.keys(texts).map((path) => ({ id: path, path, kind: "text" }));
  files.push({ id: "logo.png", path: "logo.png", kind: "asset" });
  const { target, world, calls, painted, open } = page({ files, texts });

  open("thesis.tex");
  target.update();
  assert.equal(target.state.path, "thesis.tex", "the preview starts on the open file");
  assert.equal(target.state.reason, "alone");
  assert.deepEqual([...target.state.alone].sort(), ["appendix.tex", "thesis.tex"], "the files that render alone are the ones the eye offers");

  open("appendix.tex");
  assert.equal(target.state.path, "appendix.tex", "opening a file that renders alone moves the preview");
  await nextTurn();
  assert.equal(calls.paint, 1, "a moved preview is painted once it settles");

  open("chapters/intro.tex");
  assert.equal(target.state.path, "thesis.tex", "an included file is shown inside its includer");
  assert.equal(target.state.reason, "inside");

  open("refs.bib");
  assert.equal(target.state.path, "", "a file with nothing to draw empties the preview");
  assert.equal(target.state.reason, "none");
  assert.equal(target.state.format, "");
  assert.equal(calls.cleared, 1, "emptying the preview runs the clearing tail once");

  // The eye locks without moving the source.
  target.lock("appendix.tex");
  assert.equal(world.openFile, "refs.bib", "locking does not open the file");
  assert.equal(target.state.path, "appendix.tex");
  assert.equal(target.state.reason, "locked");
  open("chapters/intro.tex");
  assert.equal(target.state.path, "appendix.tex", "the lock holds while the source moves");

  // A figure open in the source leaves the preview where it is.
  open("logo.png");
  assert.equal(target.state.path, "appendix.tex");

  target.unlock();
  open("chapters/intro.tex");
  assert.equal(target.state.path, "thesis.tex", "unlocking returns to following the open file");
  assert.equal(target.state.lockedFile, "");

  // The lock is by identity: a rename keeps it, and a delete drops it.
  target.lock("appendix.tex");
  files.find((file) => file.id === "appendix.tex").path = "annex.tex";
  texts["annex.tex"] = texts["appendix.tex"];
  delete texts["appendix.tex"];
  open("chapters/intro.tex");
  assert.equal(target.state.path, "annex.tex");
  world.files = files.filter((file) => file.id !== "appendix.tex");
  delete texts["annex.tex"];
  open("chapters/intro.tex");
  assert.equal(target.state.lockedFile, "", "deleting the locked file drops the lock");
  assert.equal(target.state.path, "thesis.tex");
  assert.ok(calls.clearFrame >= 3, "each move of the preview clears the frame");
}

// 3. An update that is skipped does nothing: a link holder reading a file in
// place of the preview keeps the preview as it was underneath.
{
  const files = [
    { id: "a.tex", path: "a.tex", kind: "text" },
    { id: "b.tex", path: "b.tex", kind: "text" },
  ];
  const { target, world, calls, open } = page({ files, texts: { "a.tex": "\\documentclass{article}", "b.tex": "\\documentclass{article}" } });
  open("a.tex");
  world.mayEdit = false;
  world.viewing = true;
  const before = calls.configure.length;
  open("b.tex");
  assert.equal(target.state.path, "a.tex", "a skipped update leaves the preview where it was");
  assert.equal(calls.configure.length, before);
  world.viewing = false;
  target.update();
  assert.equal(target.state.path, "b.tex", "the next update that is not skipped moves it");
}

// 4. A target that has not changed costs a comparison and nothing else.
{
  const files = [{ id: "a.tex", path: "a.tex", kind: "text" }];
  const { target, calls, open } = page({ files, texts: { "a.tex": "A" } });
  open("a.tex");
  const before = { configure: calls.configure.length, warm: calls.warm.length, invalidate: calls.invalidate };
  target.update();
  assert.deepEqual(
    { configure: calls.configure.length, warm: calls.warm.length, invalidate: calls.invalidate },
    before,
    "an unchanged target configures, warms and invalidates nothing",
  );
}

// 5. A disposed page is never painted, even when its target moved.
{
  const files = [
    { id: "a.tex", path: "a.tex", kind: "text" },
    { id: "b.tex", path: "b.tex", kind: "text" },
  ];
  const { world, calls, open } = page({ files, texts: { "a.tex": "\\documentclass{article}", "b.tex": "\\documentclass{article}" } });
  open("a.tex");
  open("b.tex");
  world.disposed = true;
  await nextTurn();
  assert.equal(calls.paint, 0, "nothing is painted once the page is gone");
}

// 6. navigation counts the changes: bump() moves it, and a replacement moves it.
{
  const files = [
    { id: "a.tex", path: "a.tex", kind: "text" },
    { id: "b.tex", path: "b.tex", kind: "text" },
  ];
  const { target, open } = page({ files, texts: { "a.tex": "\\documentclass{article}", "b.tex": "\\documentclass{article}" } });
  const start = target.navigation;
  target.bump();
  assert.equal(target.navigation, start + 1);
  open("a.tex");
  const afterFirst = target.navigation;
  open("b.tex");
  assert.equal(target.navigation, afterFirst + 1, "replacing the target starts a new navigation");
}

console.log("preview-target: the preview follows the open file, shows includes inside, honours the lock, and skips a page it may not move");
