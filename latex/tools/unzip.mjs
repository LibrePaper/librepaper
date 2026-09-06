// Enough of the zip format to take four files out of one release.
//
// SwiftLaTeX ships its engines in a single zip, and Node has no unzip. Adding
// a dependency to read four members of one archive would put a package in the
// lockfile that the browser never sees and the build never uses, so the
// central directory is walked here instead: it is ninety lines, it is the
// part of the format that has not changed since 1989, and it keeps
// `mirror.mjs` free of anything a `bun install` has to provide.
//
// Stored and deflated members only. A release that used anything else would
// fail loudly here rather than quietly downstream.

import { inflateRawSync } from "node:zlib";

const END_OF_CENTRAL_DIRECTORY = 0x06054b50;
const CENTRAL_FILE_HEADER = 0x02014b50;

/// Every member of the archive, by name, as bytes.
export function unzip(buffer) {
  const end = findEndOfCentralDirectory(buffer);
  const count = buffer.readUInt16LE(end + 10);
  let at = buffer.readUInt32LE(end + 16);
  const files = new Map();
  for (let i = 0; i < count; i++) {
    if (buffer.readUInt32LE(at) !== CENTRAL_FILE_HEADER) {
      throw new Error("zip: the central directory does not start where it says it does");
    }
    const method = buffer.readUInt16LE(at + 10);
    const compressedSize = buffer.readUInt32LE(at + 20);
    const nameLength = buffer.readUInt16LE(at + 28);
    const extraLength = buffer.readUInt16LE(at + 30);
    const commentLength = buffer.readUInt16LE(at + 32);
    const offset = buffer.readUInt32LE(at + 42);
    const name = buffer.toString("utf8", at + 46, at + 46 + nameLength);
    at += 46 + nameLength + extraLength + commentLength;
    if (name.endsWith("/")) continue;

    // The local header repeats the name and carries its own extra field,
    // whose length is the one that counts: the central directory's may differ.
    const localNameLength = buffer.readUInt16LE(offset + 26);
    const localExtraLength = buffer.readUInt16LE(offset + 28);
    const start = offset + 30 + localNameLength + localExtraLength;
    const raw = buffer.subarray(start, start + compressedSize);
    if (method === 0) files.set(name, Buffer.from(raw));
    else if (method === 8) files.set(name, inflateRawSync(raw));
    else throw new Error(`zip: ${name} uses compression method ${method}, which is not one of ours`);
  }
  return files;
}

// The end record is at the tail, after a comment of unknown length, so it is
// found by searching backwards for its signature.
function findEndOfCentralDirectory(buffer) {
  for (let at = buffer.length - 22; at >= 0; at--) {
    if (buffer.readUInt32LE(at) === END_OF_CENTRAL_DIRECTORY) return at;
  }
  throw new Error("zip: no end-of-central-directory record; not a zip");
}
