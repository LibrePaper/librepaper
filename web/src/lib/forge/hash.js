// The two identities a synced file has.
//
// GitHub names a file by its git blob id, the SHA-1 of a header and the raw
// bytes, so a CRLF file and its LF twin have different ids. LibrePaper keeps
// project text as LF, so it names text by the SHA-256 of the LF form. The sync
// map holds both: the blob id notices a change on GitHub, the content hash
// notices a change here, and they differ for every CRLF file.

import { sha256HexOfText } from "../digest.js";

const encoder = new TextEncoder();

const LFS_PREFIX = encoder.encode("version https://git-lfs.github.com/spec/v1");

function toHex(buffer) {
  return Array.from(new Uint8Array(buffer), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// The git blob id of some bytes, 40 hex characters: what `git hash-object`
/// prints for a file with this content.
export async function gitBlobId(bytes) {
  if (!globalThis.crypto?.subtle) throw new Error("Web Crypto SHA-1 is required");
  const header = encoder.encode(`blob ${bytes.length}\0`);
  const whole = new Uint8Array(header.length + bytes.length);
  whole.set(header, 0);
  whole.set(bytes, header.length);
  return toHex(await crypto.subtle.digest("SHA-1", whole));
}

/// The text with every CRLF turned into LF. A lone CR stays, because it is
/// content and not a line ending.
export function toLf(text) {
  return text.replaceAll("\r\n", "\n");
}

/// The content hash of a text file: SHA-256 of its LF form, as hex.
export async function textHash(text) {
  return sha256HexOfText(toLf(text));
}

/// `{ text }` for bytes that are valid UTF-8, otherwise `null`. A byte order
/// mark is kept, so that the text encodes back to the same bytes.
export function decodeText(bytes) {
  try {
    return { text: new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes) };
  } catch {
    return null;
  }
}

/// Whether the bytes are a Git LFS pointer, which stands for a file this
/// browser cannot fetch from the ordinary blob endpoint.
export function isLfsPointer(bytes) {
  if (bytes.length < LFS_PREFIX.length) return false;
  for (let i = 0; i < LFS_PREFIX.length; i += 1) {
    if (bytes[i] !== LFS_PREFIX[i]) return false;
  }
  return true;
}
