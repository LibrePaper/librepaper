// The projection corpus, from the JavaScript side.
//
// `crates/librepaper-document-core/tests/projection_fixtures.rs` reads the
// same file and asserts the same expectations. Two implementations of
// the projection held equal by trees and digests: a reader's etag,
// a rendered comment's staleness check and an export all name a document by
// this digest, so the two sides disagreeing is not a cosmetic difference.
//
// Expectations are generated from the Rust side. To move them, change the
// algorithm on both sides and regenerate:
//
//     LIBREPAPER_REGENERATE_FIXTURES=1 cargo test -p librepaper-document-core

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { LoroDoc, LoroText } from "loro-crdt";

import { project, canonicalBytes, compareUtf8 } from "../../src/lib/projection.js";
import { sha256Hex } from "../../src/lib/digest.js";

const corpus = JSON.parse(
  readFileSync(fileURLToPath(new URL("../fixtures/projection.json", import.meta.url)), "utf8"),
);
const rules = corpus.rules;

function insert(map, key, value) {
  map.set(key, value);
}

function build(testCase) {
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  const assets = doc.getMap("assets");
  const meta = doc.getMap("meta");
  for (const entry of testCase.files || []) {
    if (typeof entry.text === "string") {
      const text = files.setContainer(entry.id, new LoroText());
      if (entry.text) text.insert(0, entry.text);
    } else {
      insert(files, entry.id, entry.value);
    }
  }
  for (const entry of testCase.paths || []) {
    insert(paths, entry.id, entry.path !== undefined ? entry.path : entry.value);
  }
  for (const entry of testCase.assets || []) insert(assets, entry.path, entry.value);
  for (const entry of testCase.meta || []) insert(meta, entry.key, entry.value);
  return doc;
}

/// The same shape the Rust side writes: diagnostics without their sentence,
/// which each implementation words for its own callers.
function actualOf(projection, digest) {
  return {
    files: [...projection.files].map(([path, entry]) => [
      path,
      { bytes: entry.bytes, digest: entry.digest, id: entry.id, kind: entry.kind },
    ]),
    diagnostics: projection.diagnostics.map((diagnostic) => {
      const { why, ...rest } = diagnostic;
      void why;
      return rest;
    }),
    digest,
  };
}

/// Key order differs between the two serializers, so compare values rather
/// than text.
function normalise(value) {
  return JSON.parse(JSON.stringify(value));
}

for (const testCase of corpus.cases) {
  const doc = build(testCase);
  const projection = await project(doc, rules);
  const digest = await sha256Hex(canonicalBytes(projection));
  const actual = normalise(actualOf(projection, digest));
  const expected = normalise(testCase.expect);
  assert.deepEqual(
    normalise(projection.retained),
    normalise(testCase.expect.retained || {}),
    `${testCase.name}: the browser retains different texts from the server`,
  );
  delete expected.retained;
  assert.deepEqual(
    actual,
    expected,
    `${testCase.name}: the browser projects this differently from the server`,
  );

  // Every placed text is readable at its emitted path, which is what a render
  // and an export depend on.
  for (const [path, entry] of projection.files) {
    if (entry.kind === "text") {
      assert.ok(
        projection.texts.has(path),
        `${testCase.name}: ${path} is a text in the tree with no body beside it`,
      );
    }
  }
}

// Retained texts that collide are suffixed in id order.
{
  const retainRules = { ...rules, text_extensions: [".md"], asset_extensions: [], derived_extensions: [] };
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  const add = (id, path, body) => {
    const text = files.setContainer(id, new LoroText());
    text.insert(0, body);
    paths.set(id, path);
  };
  add("aaa", "a.R", "first");
  add("bbb", "a.R", "second");
  add("ccc", "Notes.py", "upper");
  add("ddd", "notes.py", "lower");
  const result = await project(doc, retainRules);
  assert.deepEqual(result.retained, {
    "a.R": "first",
    "a (2).R": "second",
    "Notes.py": "upper",
    "notes (2).py": "lower",
  });
}

// A retained text never takes a path a placed file needs as a directory.
{
  const retainRules = { ...rules, text_extensions: [".md"], asset_extensions: [], derived_extensions: [] };
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  const add = (id, path, body) => {
    const text = files.setContainer(id, new LoroText());
    text.insert(0, body);
    paths.set(id, path);
  };
  add("aaa", "scripts.R/readme.md", "placed");
  add("bbb", "scripts.R", "retained");
  const result = await project(doc, retainRules);
  assert.ok(result.files.has("scripts.R/readme.md"));
  assert.deepEqual(result.retained, { "scripts (2).R": "retained" });
}

// Retained texts: structurally sound, extension refused. Downloads only.
{
  const retainRules = { ...rules, text_extensions: [".md"], asset_extensions: [], derived_extensions: [] };
  const doc = new LoroDoc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  const add = (id, path, body) => {
    const text = files.setContainer(id, new LoroText());
    if (body) text.insert(0, body);
    paths.set(id, path);
  };
  add("a", "main.md", "# hi");
  add("b", "analysis.py", "print(1)");
  add("c", "../escape.py", "no");
  add("d", "MAIN.MD.py", "x");
  add("e", "Notes.PY", "n");
  add("f", "notes.py", "m");
  const withRule = await project(doc, retainRules);
  assert.deepEqual([...withRule.files.keys()], ["main.md"]);
  assert.equal(withRule.retained["analysis.py"], "print(1)");
  assert.ok(!("../escape.py" in withRule.retained), "a structural refusal is not retained");
  assert.deepEqual(Object.keys(withRule.retained), Object.keys(withRule.retained).slice().sort(compareUtf8));
  assert.equal(withRule.diagnostics.filter((d) => d.kind === "invalid-path").length, 5);

  // Same digest as a document without the retained texts.
  const plain = new LoroDoc();
  const plainText = plain.getMap("files").setContainer("a", new LoroText());
  plainText.insert(0, "# hi");
  plain.getMap("paths").set("a", "main.md");
  const bare = await project(plain, retainRules);
  assert.equal(
    await sha256Hex(canonicalBytes(withRule)),
    await sha256Hex(canonicalBytes(bare)),
    "retained texts do not move the digest",
  );
}

// Directory and file collisions, both orders: a taken file blocks a directory
// of the same name, and a taken directory blocks a file under it.
{
  const retainRules = { ...rules, text_extensions: [".md"], asset_extensions: [], derived_extensions: [] };
  const add = (files, paths, id, path, body) => {
    const text = files.setContainer(id, new LoroText());
    text.insert(0, body);
    paths.set(id, path);
  };
  const fileFirst = new LoroDoc();
  add(fileFirst.getMap("files"), fileFirst.getMap("paths"), "aaa", "a.md", "file");
  add(fileFirst.getMap("files"), fileFirst.getMap("paths"), "bbb", "a.md/b.md", "under");
  const one = await project(fileFirst, retainRules);
  const twice = await project(fileFirst, retainRules);
  assert.deepEqual(normalise(actualOf(one, "")), normalise(actualOf(twice, "")), "collisions are deterministic");
  assert.ok(one.diagnostics.some((d) => d.kind === "moved" || d.kind === "dropped"), "a file and a directory of the same name collide");
  for (const path of one.files.keys()) {
    for (const other of one.files.keys()) {
      assert.ok(
        path === other || !other.startsWith(`${path}/`),
        `${path} and ${other} are both placed, one under the other`,
      );
    }
  }

  const dirFirst = new LoroDoc();
  add(dirFirst.getMap("files"), dirFirst.getMap("paths"), "aaa", "x.md/y.md", "under");
  add(dirFirst.getMap("files"), dirFirst.getMap("paths"), "bbb", "x.md", "file");
  const dirResult = await project(dirFirst, retainRules);
  assert.ok(dirResult.diagnostics.some((d) => d.kind === "moved" || d.kind === "dropped"), "x taken then x/y.md collides");
  for (const path of dirResult.files.keys()) {
    for (const other of dirResult.files.keys()) {
      assert.ok(path === other || !other.startsWith(`${path}/`), `${path} and ${other} are both placed, one under the other`);
    }
  }
}

// A larger project is placed deterministically and without collisions.
{
  const retainRules = { ...rules, text_extensions: [".md"], asset_extensions: [], derived_extensions: [] };
  const build300 = () => {
    const doc = new LoroDoc();
    const files = doc.getMap("files");
    const paths = doc.getMap("paths");
    for (let i = 0; i < 300; i += 1) {
      const id = `f${String(i).padStart(3, "0")}`;
      const text = files.setContainer(id, new LoroText());
      text.insert(0, `body ${i}`);
      const dir = `d${i % 7}`;
      paths.set(id, i % 3 === 0 ? `${dir}/n${i}.md` : `${dir}/sub/n${i}.md`);
    }
    return doc;
  };
  const first = await project(build300(), retainRules);
  const second = await project(build300(), retainRules);
  assert.deepEqual(normalise(actualOf(first, "")), normalise(actualOf(second, "")), "300 files project deterministically");
  assert.equal(first.files.size, 300);
  assert.deepEqual(first.diagnostics, []);
}
