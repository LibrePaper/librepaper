// Blob ids and content hashes for forge sync. The expected ids were printed by
// `git hash-object --stdin`, so a change here that moves them has moved away
// from what GitHub computes.

import assert from "node:assert/strict";

import { sha256HexOfText } from "../../src/lib/digest.js";
import { decodeText, gitBlobId, isLfsPointer, textHash, toLf } from "../../src/lib/forge/hash.js";

const encode = (text) => new TextEncoder().encode(text);

assert.equal(await gitBlobId(encode("a\nb\n")), "422c2b7ab3b3c668038da977e4e93a5fc623169c");
assert.equal(await gitBlobId(encode("a\r\nb\r\n")), "c30dea8a3641ea99b125d04d599d843712292759");
assert.equal(
  await gitBlobId(new Uint8Array([0, 1, 2, 255])),
  "f971a5e28b6c4cb237ca3c7349e33bb600dbc907",
);
assert.equal(await gitBlobId(new Uint8Array()), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
assert.equal(await gitBlobId(encode("café\n")), "572eb43fe8e34fb87d01c69e01151ff696022924");

// A CRLF file and its LF twin have different blob ids and one content hash.
assert.equal(toLf("a\r\nb\r\n"), "a\nb\n");
assert.equal(toLf("a\rb\n"), "a\rb\n");
assert.equal(await textHash("a\r\nb\r\n"), await textHash("a\nb\n"));
assert.equal(await textHash("a\nb\n"), await sha256HexOfText("a\nb\n"));
assert.notEqual(await textHash("a\nb\n"), await textHash("a\nb"));

assert.deepEqual(decodeText(encode("café")), { text: "café" });
assert.equal(decodeText(new Uint8Array([0xff, 0xfe, 0x41])), null);
assert.deepEqual(decodeText(new Uint8Array()), { text: "" });

assert.equal(isLfsPointer(encode("version https://git-lfs.github.com/spec/v1\noid sha256:ab\nsize 3\n")), true);
assert.equal(isLfsPointer(encode("version 1")), false);
assert.equal(isLfsPointer(encode("# title\n")), false);

console.log("forge hash ok");
