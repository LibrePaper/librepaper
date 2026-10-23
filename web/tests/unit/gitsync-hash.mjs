// Content hashes and text helpers for Git sync.

import assert from "node:assert/strict";

import { sha256HexOfText } from "../../src/lib/digest.js";
import { decodeText, isLfsPointer, textHash, toLf } from "../../src/lib/gitsync/hash.js";

const encode = (text) => new TextEncoder().encode(text);

// A CRLF file and its LF twin have one content hash.
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

console.log("gitsync hash ok");
