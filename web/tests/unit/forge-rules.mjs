// Which repository paths are project files.

import assert from "node:assert/strict";

import { syncRules } from "../../src/lib/forge/rules.js";

const rules = {
  text_extensions: [".tex", ".md", ".qmd", ".bib", ".csv"],
  asset_extensions: [".png"],
  derived_extensions: [".aux"],
  max_path: 40,
  max_files: 5,
};

const blob = (path, size = 10, extra = {}) => ({
  path,
  mode: "100644",
  type: "blob",
  sha: `sha-${path}`,
  size,
  ...extra,
});

function classify(entries, options = {}) {
  return syncRules({
    rules,
    folder: "",
    maxFileBytes: 1000,
    ignoreRoot: null,
    ignoreFolder: null,
    ...options,
  }).classify(entries);
}

// The built-in rules.
{
  const result = classify([
    blob("paper.qmd"),
    blob("fig/a.png"),
    blob("Makefile"),
    blob(".github/workflows/x.md"),
    blob(".gitignore"),
    blob("main.aux"),
    blob("link.md", 3, { mode: "120000" }),
    blob("huge.md", 5000),
    { path: "fig", mode: "040000", type: "tree", sha: "t" },
    { path: "sub", mode: "160000", type: "commit", sha: "c" },
  ]);
  assert.deepEqual([...result.synced.keys()], ["paper.qmd", "fig/a.png"]);
  assert.equal(result.synced.get("paper.qmd").kind, "text");
  assert.equal(result.synced.get("fig/a.png").kind, "asset");
  assert.equal(result.synced.get("paper.qmd").blob, "sha-paper.qmd");
  const reasons = new Map(result.unsynced.map((item) => [item.path, item.reason]));
  assert.equal(reasons.size, 6);
  assert.equal(reasons.get("link.md"), "a symbolic link");
  assert.equal(reasons.get("huge.md"), "over the file size limit");
  assert.deepEqual(result.refusals, []);
  assert.deepEqual([...result.folders], [[".", 1], ["fig", 1]]);
}

// A linked folder: paths outside it are unsynced, paths inside lose the prefix.
{
  const set = syncRules({ rules, folder: "paper", maxFileBytes: 1000, ignoreRoot: null, ignoreFolder: null });
  const result = set.classify([blob("paper/main.qmd"), blob("README.md"), blob("papers/x.md")]);
  assert.deepEqual([...result.synced.keys()], ["main.qmd"]);
  assert.equal(result.synced.get("main.qmd").repoPath, "paper/main.qmd");
  assert.deepEqual(
    result.unsynced.map((item) => [item.path, item.reason]),
    [["README.md", "outside the linked folder"], ["papers/x.md", "outside the linked folder"]],
  );
  assert.equal(set.projectPath("paper/main.qmd"), "main.qmd");
  assert.equal(set.repoPath("main.qmd"), "paper/main.qmd");
  assert.equal(set.isSynced("main.qmd"), true);
  assert.equal(set.isSynced(".hidden.md"), false);
}

// The ignore file, with `!` re-inclusion, folders, and the root file's paths.
{
  const entries = [
    blob("a.qmd"),
    blob("data/big.csv"),
    blob("data/keep.csv"),
    blob("notes/x.md"),
    blob("notes/deep/y.md"),
  ];
  const ignoreRoot = "data/*\n!data/keep.csv\nnotes/\n";
  const result = classify(entries, { ignoreRoot });
  assert.deepEqual([...result.synced.keys()], ["a.qmd", "data/keep.csv"]);

  // A file inside an ignored folder cannot be re-included by `!`.
  const folderRule = classify(entries, { ignoreRoot: "notes/\n!notes/x.md\n" });
  assert.equal(folderRule.synced.has("notes/x.md"), false);

  // The ignore file cannot bring back what the built-in rules refuse.
  const reinclude = classify([blob("Makefile"), blob("a.qmd")], { ignoreRoot: "!Makefile\n" });
  assert.deepEqual([...reinclude.synced.keys()], ["a.qmd"]);

  // A file added later inside an ignored folder stays out.
  const added = classify([...entries, blob("notes/new.md")], { ignoreRoot });
  assert.equal(added.synced.has("notes/new.md"), false);
}

// Root rules see repository paths and folder rules see paths in the folder.
{
  const set = syncRules({
    rules,
    folder: "paper",
    maxFileBytes: 1000,
    ignoreRoot: "paper/skip.md\n",
    ignoreFolder: "draft.md\n",
  });
  const result = set.classify([blob("paper/skip.md"), blob("paper/draft.md"), blob("paper/ok.md")]);
  assert.deepEqual([...result.synced.keys()], ["ok.md"]);
  assert.equal(set.isSynced("skip.md"), false);
  assert.equal(set.isSynced("draft.md"), false);
  assert.equal(set.isSynced("ok.md"), true);
}

// A collision pair refuses and names both paths.
{
  const result = classify([blob("A.qmd"), blob("a.qmd"), blob("b.qmd")]);
  assert.deepEqual([...result.synced.keys()], ["b.qmd"]);
  assert.equal(result.refusals.length, 1);
  assert.deepEqual(result.refusals[0].paths.sort(), ["A.qmd", "a.qmd"]);
  const nfc = classify([blob("café.md"), blob("café.md")]);
  assert.equal(nfc.refusals.length, 1);
  assert.equal(nfc.refusals[0].paths.length, 2);
}

// An overlong path refuses, but only when the file would be synced.
{
  const long = `${"d".repeat(45)}.md`;
  const result = classify([blob(long), blob(`${"d".repeat(45)}.zip`), blob("ok.md")]);
  assert.deepEqual([...result.synced.keys()], ["ok.md"]);
  assert.equal(result.refusals.length, 1);
  assert.deepEqual(result.refusals[0].paths, [long]);
}

// More files than max_files refuses.
{
  const names = ["a", "b", "c", "d", "e", "f"].map((name) => blob(`${name}.md`));
  assert.equal(classify(names.slice(0, 5)).refusals.length, 0);
  const result = classify(names);
  assert.equal(result.refusals.length, 1);
  assert.deepEqual(result.refusals[0].paths, []);
  assert.match(result.refusals[0].reason, /6 syncable files/);
}

// An renv folder is an environment, whatever its files' extensions.
{
  const set = syncRules({ rules, folder: "", maxFileBytes: 1000, ignoreRoot: null, ignoreFolder: null });
  assert.equal(set.isSynced("renv/notes.qmd"), false);
  assert.equal(set.isSynced("code/renv/notes.qmd"), false);
  assert.equal(set.isSynced("analysis.qmd"), true);
}

console.log("forge rules ok");
