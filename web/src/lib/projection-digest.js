// The canonical form a projection is named by, and nothing else.
//
// This is a leaf on purpose. `projection.js` needs `loro-crdt` to walk a
// document, and `renderers.js` needs the whole renderer stack; neither can be
// imported by the LaTeX worker or by the assistant preview, which want the
// digest and nothing around it. So the canonical serialization lives here,
// importing only `digest.js`, and the two heavier modules re-export it.
// One implementation of the name, reachable from every layer that has to say
// it.
//
// The form is SPEC-server-is-a-log §4.4 step 7, and it is held byte-identical
// to `librepaper_document_core::Projection::canonical_bytes` by the corpus in
// `web/tests/fixtures/projection.json`.

import { sha256Hex, sha256HexOfText } from "./digest.js";

const encoder = new TextEncoder();

// Rust compares `&str` and sorts `BTreeMap<String, _>` by UTF-8 bytes;
// JavaScript's `<` compares UTF-16 code units. They disagree exactly once, on
// a supplementary character against a character in U+E000..U+FFFF, which is
// reachable with an emoji in a filename. Compare the encoded bytes so the two
// orders are the same one.
export function compareUtf8(left, right) {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  const length = Math.min(a.length, b.length);
  for (let at = 0; at < length; at += 1) {
    if (a[at] !== b[at]) return a[at] - b[at];
  }
  return a.length - b.length;
}

/// The bytes the digest is taken over: a version marker, the main path, then
/// one line per file in path order carrying its path, kind and content
/// digest.
///
/// A line-oriented form rather than JSON, because JSON leaves the
/// implementation free to reorder integer-looking keys and to spell escapes
/// differently, and the two implementations have to produce the same bytes.
/// No field can contain a newline: a path with a control character in it is
/// refused when the projection validates paths, and a kind and a digest are
/// drawn from fixed alphabets.
///
/// The digest is over each file's CONTENT rather than its Loro id. An id is
/// minted afresh whenever a file is created and never changes when the text
/// in it does, so a name built from ids would not move when the document
/// moved, and every use this name has -- a reader's etag, the
/// stale-selection check on a rendered comment -- needs it to move exactly
/// then.
///
/// `files` is an iterable of `[path, {kind, digest}]` already in path order.
export function canonicalBytes(projection) {
  let out = "librepaper.projection.v1\n";
  out += `${projection.main}\n`;
  for (const [path, entry] of projection.files) {
    out += `${path}\t${entry.kind}\t${entry.digest}\n`;
  }
  return encoder.encode(out);
}

/// The name of a renderer tree: `{main, texts, digests}` as the render path
/// carries it, hashed through the one canonical form above.
///
/// A renderer tree is not a projection -- it has no Loro ids and no
/// diagnostics -- but the question it answers is the same one, so it is
/// worth the same answer rather than a second hash of a differently shaped
/// object.
///
/// The compile settings deliberately do not participate. The projection
/// names what the document SAYS; an engine is a choice about how to render
/// it, and folding it in would mean two documents with identical text but
/// different engines failing to recognise each other as the same content.
export async function snapshotDigest(tree) {
  const texts = tree?.texts || {};
  const digests = tree?.digests || {};
  const paths = [...new Set([...Object.keys(texts), ...Object.keys(digests)])].sort(compareUtf8);
  const files = [];
  for (const path of paths) {
    if (Object.prototype.hasOwnProperty.call(texts, path)) {
      files.push([path, { kind: "text", digest: await sha256HexOfText(String(texts[path])) }]);
    } else {
      files.push([path, { kind: "asset", digest: String(digests[path]) }]);
    }
  }
  return sha256Hex(canonicalBytes({ main: String(tree?.main || ""), files }));
}
