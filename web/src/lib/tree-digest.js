// The digest the server gives a document tree.
//
// Rust hashes `Tree::to_bytes`, which is deliberately not the whole stored
// object: a text's `id` is minted afresh whenever a file is created, and an
// entry's `size` follows from its bytes. Neither says anything about what the
// document says, so the canonical form is the main path, every path with the
// kind and digest of what is at it, and the compile settings. Keep the
// serialization here exactly that shape: the rendering name must identify the
// exact source that was compiled, and a publication names the checkpoint it
// was rendered from by this digest.

import { sha256Hex as digest } from "./digest.js";

const encoder = new TextEncoder();

function compareUtf8(left, right) {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  const length = Math.min(a.length, b.length);
  for (let at = 0; at < length; at += 1) {
    if (a[at] !== b[at]) return a[at] - b[at];
  }
  return a.length - b.length;
}

/// Returns the server's canonical digest for a renderer tree.
export async function snapshotDigest(tree) {
  const texts = tree?.texts || {};
  const digests = tree?.digests || {};
  const paths = [...new Set([...Object.keys(texts), ...Object.keys(digests)])].sort(compareUtf8);
  const files = [];

  for (const path of paths) {
    if (Object.prototype.hasOwnProperty.call(texts, path)) {
      files.push([path, ["text", await digest(encoder.encode(String(texts[path])))]]);
    } else {
      files.push([path, ["asset", String(digests[path])]]);
    }
  }

  // JSON.stringify reorders integer-looking object keys regardless of
  // insertion order. Emit the BTreeMap entries explicitly so paths such as
  // `10` and `2`, or `__proto__`, retain their Rust order and meaning.
  //
  // `settings` mirrors the engine-only compile setting. Legacy release pins
  // are intentionally omitted and therefore have no effect on compilation.
  const engine = tree?.settings?.engine || "";
  const settingsJson = engine ? `,"settings":{"engine":${JSON.stringify(engine)}}` : "";
  const json = `{"main":${JSON.stringify(String(tree?.main || ""))},"files":{${files
    .map(([path, entry]) => `${JSON.stringify(path)}:${JSON.stringify(entry)}`)
    .join(",")}}${settingsJson}}`;
  return digest(encoder.encode(json));
}

export { compareUtf8 };
