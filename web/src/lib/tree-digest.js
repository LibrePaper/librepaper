// The digest the server gives a document tree.
//
// Rust hashes serde_json::to_vec(Tree), where Tree's files are a BTreeMap and
// TreeEntry omits an empty id. Keep the serialization here deliberately close
// to that shape: the rendering name must identify the exact source that was
// compiled, rather than the source that happened to be current a moment later.

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

async function digest(bytes) {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hash)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function bytesOf(value) {
  if (value == null) return null;
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  return null;
}

/// Returns the server's canonical digest for a renderer tree. `tree.files`,
/// when present, is the session's path-to-entry metadata and supplies the
/// stable Yjs id and asset size. The fallback shape remains useful for old
/// sessions, but a live text without its id cannot equal a server checkpoint.
export async function snapshotDigest(tree, assets = tree?.assets || {}) {
  const texts = tree?.texts || {};
  const digests = tree?.digests || {};
  const supplied = tree?.files || {};
  const paths = [...new Set([...Object.keys(texts), ...Object.keys(digests)])].sort(compareUtf8);
  const files = [];

  for (const path of paths) {
    const metadata = Object.prototype.hasOwnProperty.call(supplied, path) ? supplied[path] || {} : {};
    if (Object.prototype.hasOwnProperty.call(texts, path)) {
      const body = String(texts[path]);
      const entry = {
        kind: "text",
        ...(metadata.id ? { id: String(metadata.id) } : {}),
        sha: await digest(encoder.encode(body)),
        size: encoder.encode(body).byteLength,
      };
      files.push([path, entry]);
    } else {
      const body = bytesOf(assets[path]);
      files.push([path, {
        kind: "asset",
        sha: String(digests[path]),
        size: body ? body.byteLength : Number.isInteger(metadata.size) ? metadata.size : 0,
      }]);
    }
  }

  // JSON.stringify reorders integer-looking object keys regardless of
  // insertion order. Emit the BTreeMap entries explicitly so paths such as
  // `10` and `2`, or `__proto__`, retain their Rust order and meaning.
  const json = `{"main":${JSON.stringify(String(tree?.main || ""))},"files":{${files
    .map(([path, entry]) => `${JSON.stringify(path)}:${JSON.stringify(entry)}`)
    .join(",")}}}`;
  return digest(encoder.encode(json));
}

export { compareUtf8 };
