// The live project index: which files are re-indexed on each refresh, and when a
// refresh reports a change.
import assert from "node:assert/strict";
import test from "node:test";
import { createLiveIndex } from "../../src/lib/document-index/live.js";

test("refresh indexes new files and reports the change", () => {
  const live = createLiveIndex();
  const changed = live.refresh({
    texts: { "a.tex": "\\label{fig:a}\n", "b.tex": "\\label{fig:b}\n" },
    files: ["a.tex", "b.tex"],
    bibliography: null,
  });
  assert.equal(changed, true);
  assert.deepEqual(live.project.paths(), ["a.tex", "b.tex"]);
  assert.deepEqual(live.project.names("label"), ["fig:a", "fig:b"]);
  assert.deepEqual(live.project.knownFiles(), ["a.tex", "b.tex"]);
});

test("refresh re-indexes only the files whose text changed", () => {
  const live = createLiveIndex();
  const files = ["a.tex", "b.tex"];
  live.refresh({ texts: { "a.tex": "\\label{fig:a}\n", "b.tex": "\\label{fig:b}\n" }, files, bibliography: null });
  const unchanged = live.project.file("a.tex");

  const changed = live.refresh({
    texts: { "a.tex": "\\label{fig:a}\n", "b.tex": "\\label{fig:c}\n" },
    files,
    bibliography: null,
  });

  assert.equal(changed, true);
  assert.equal(live.project.file("a.tex"), unchanged, "unchanged file keeps its index object");
  assert.notEqual(live.project.file("b.tex"), undefined);
  assert.deepEqual(live.project.names("label"), ["fig:a", "fig:c"]);
});

test("refresh removes files that vanished", () => {
  const live = createLiveIndex();
  live.refresh({ texts: { "a.tex": "\\label{fig:a}\n", "b.tex": "\\label{fig:b}\n" }, files: ["a.tex", "b.tex"], bibliography: null });

  const changed = live.refresh({ texts: { "a.tex": "\\label{fig:a}\n" }, files: ["a.tex"], bibliography: null });

  assert.equal(changed, true);
  assert.equal(live.project.file("b.tex"), undefined);
  assert.deepEqual(live.project.paths(), ["a.tex"]);
  assert.deepEqual(live.project.names("label"), ["fig:a"]);
});

test("refresh ignores files that have no adapter", () => {
  const live = createLiveIndex();
  live.refresh({ texts: { "a.tex": "\\label{fig:a}\n", "data.csv": "x,y\n" }, files: ["a.tex", "data.csv"], bibliography: null });
  assert.equal(live.project.file("data.csv"), undefined);
  assert.deepEqual(live.project.paths(), ["a.tex"]);
});

test("refresh returns false when nothing changed", () => {
  const live = createLiveIndex();
  const snapshot = {
    texts: { "a.tex": "\\label{fig:a}\n" },
    files: ["a.tex"],
    bibliography: ["smith"],
  };
  live.refresh(snapshot);
  assert.equal(live.refresh({ ...snapshot, texts: { "a.tex": "\\label{fig:a}\n" } }), false);
});

test("refresh returns true when the bibliography changes", () => {
  const live = createLiveIndex();
  const files = ["a.tex"];
  const texts = { "a.tex": "\\label{fig:a}\n" };
  live.refresh({ texts, files, bibliography: ["smith"] });

  assert.equal(live.refresh({ texts, files, bibliography: ["smith", "jones"] }), true);
  assert.deepEqual(live.project.names("cite"), ["jones", "smith"]);
  assert.equal(live.refresh({ texts, files, bibliography: ["jones", "smith"] }), false);
  assert.equal(live.refresh({ texts, files, bibliography: null }), true);
  assert.equal(live.project.hasCitation("smith"), null);
});

test("refresh returns true when the file list changes", () => {
  const live = createLiveIndex();
  const texts = { "a.tex": "\\label{fig:a}\n" };
  live.refresh({ texts, files: ["a.tex"], bibliography: null });

  assert.equal(live.refresh({ texts, files: ["a.tex", "figures/plot.png"], bibliography: null }), true);
  assert.deepEqual(live.project.knownFiles(), ["a.tex", "figures/plot.png"]);
  assert.equal(live.refresh({ texts, files: ["figures/plot.png", "a.tex"], bibliography: null }), false);
});

test("invalidate makes the next refresh index unchanged files again", () => {
  const live = createLiveIndex();
  const snapshot = {
    texts: { "a.tex": "\\label{fig:a}\n", "b.tex": "\\label{fig:b}\n" },
    files: ["a.tex", "b.tex"],
    bibliography: null,
  };
  live.refresh(snapshot);
  const a = live.project.file("a.tex");
  const b = live.project.file("b.tex");
  assert.equal(live.refresh(snapshot), false);

  live.invalidate((path) => path === "a.tex");
  assert.equal(live.refresh(snapshot), true);
  assert.notEqual(live.project.file("a.tex"), a, "the invalidated file is indexed again");
  assert.equal(live.project.file("b.tex"), b, "other files are left alone");

  live.invalidate();
  live.refresh(snapshot);
  assert.notEqual(live.project.file("b.tex"), b, "without a filter every file is indexed again");
});
