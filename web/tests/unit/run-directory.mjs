// The Run directory notices become warnings, and a missing file gets a hint.
import assert from "node:assert/strict";
import { test } from "node:test";
import { belongsInRunDirectory, homeOf, leaf, logMissesAFile, noticeDiagnostics, pageVariant, runDirectoryDiagnostics, shorten } from "../../src/lib/reader/run-directory.js";

test("a clash and an unlinked entry become warnings", () => {
  const items = noticeDiagnostics([{ kind: "clash", path: "data/x.csv" }, { kind: "not-linked", path: "big" }, { kind: "later", path: "z" }, { kind: "clash", path: "" }]);
  assert.equal(items.length, 2);
  assert.ok(items.every((item) => item.severity === "warning" && item.source === "local-app"));
  assert.match(items[0].message, /data\/x\.csv is in LibrePaper and in your Run directory/);
  assert.match(items[1].message, /big in your Run directory could not be made available to Quarto/);
  assert.deepEqual(noticeDiagnostics(undefined), []);
});

test("the missing-file hint reads R, Python and shell messages", () => {
  for (const log of ["cannot open file 'data.csv': No such file", "FileNotFoundError: [Errno 2]", "Cannot find the file x", "x: No such file or directory"]) {
    assert.equal(logMissesAFile(log), true, log);
  }
  assert.equal(logMissesAFile("Error: object 'x' not found"), false);
  assert.equal(logMissesAFile(undefined), false);
});

test("only a failed Quarto render with a known folder gets the hint", () => {
  const log = "cannot open file 'data/x.csv'";
  const hinted = runDirectoryDiagnostics({ format: "quarto", ok: false, log, notices: [] }, "/home/me/study");
  assert.equal(hinted.length, 1);
  assert.match(hinted[0].message, /Code looks for files in your Run directory: \/home\/me\/study\./);
  assert.deepEqual(runDirectoryDiagnostics({ format: "quarto", ok: false, log, notices: [] }, null), []);
  assert.deepEqual(runDirectoryDiagnostics({ format: "quarto", ok: true, log, notices: [] }, "/p"), []);
  assert.deepEqual(runDirectoryDiagnostics({ format: "typst", ok: false, log, notices: [{ kind: "clash", path: "a" }] }, "/p"), []);
  assert.equal(runDirectoryDiagnostics({ format: "quarto", ok: true, notices: [{ kind: "clash", path: "a" }] }).length, 1);
});

test("shorten replaces the home directory with ~", () => {
  const home = homeOf("/home/ana/LibrePaper/thesis");
  assert.equal(home, "/home/ana");
  assert.equal(shorten("/home/ana/LibrePaper/thesis", home), "~/LibrePaper/thesis");
  assert.equal(shorten("/home/ana", home), "~");
  assert.equal(shorten("/home/anabel/x", home), "/home/anabel/x");
  assert.equal(shorten("/data/x", ""), "/data/x");
  assert.equal(homeOf("C:\\Users\\ana\\LibrePaper\\thesis"), "C:\\Users\\ana");
  assert.equal(homeOf("/tmp/other"), "");
});

test("leaf is the last path segment", () => {
  assert.equal(leaf("/a/b/c/"), "c");
  assert.equal(leaf("C:\\a\\b"), "b");
});

test("pageVariant picks the version of the page to show", () => {
  assert.equal(pageVariant({ path: "/h/a", exists: true, suggested: "/h/s" }, true), "current");
  assert.equal(pageVariant({ path: "/h/a", exists: false, suggested: "/h/s" }, false), "missing");
  assert.equal(pageVariant({ path: "", suggested: "/h/s" }, false), "suggested");
  assert.equal(pageVariant({ path: "", suggested: "" }, false), "none");
  assert.equal(pageVariant(null, false), "none");
});

test("scripts and data files belong in the Run directory, documents do not", () => {
  for (const path of ["analysis.R", "code/clean.py", "data/survey.CSV", "model.rds"]) assert.equal(belongsInRunDirectory(path), true, path);
  for (const path of ["paper.qmd", "refs.bib", "img/plot.png", "README", ".R/notes.md", "", undefined]) assert.equal(belongsInRunDirectory(path), false, String(path));
});
