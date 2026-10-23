// Content identities for synced files.
//
// Git computes blob ids. LibrePaper keeps project text as LF, so it names text
// by the SHA-256 of the LF form and compares that with the digests the server
// projects.

import { sha256HexOfText } from "../digest.js";

const encoder = new TextEncoder();

const LFS_PREFIX = encoder.encode("version https://git-lfs.github.com/spec/v1");

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
