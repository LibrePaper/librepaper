// The projection corpus, from the JavaScript side.
//
// `crates/librepaper-document-core/tests/projection_fixtures.rs` reads the
// same file and asserts the same expectations. Two implementations of
// SPEC-server-is-a-log §4.4 held equal by trees and digests: a reader's etag,
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

import { project, canonicalBytes } from "../../src/lib/projection.js";
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
    main: projection.main,
    mainId: projection.mainId,
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
