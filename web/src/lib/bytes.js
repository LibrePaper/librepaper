// Coercing arbitrary values to bytes, in the three contracts this codebase
// actually uses.
//
// These were five separate copies -- in `tree-digest.js`, `latex.js`,
// `latex/vm.js`, `latex/local.js` and `latex/bibliography.js` -- and they had
// already drifted into three different answers for the same input: a string
// was `null` in one, encoded UTF-8 in another; an unrecognised value was
// `null` in one and a thrown `TypeError` in another. Collapsing them to a
// single lenient helper would have changed behaviour at call sites that
// depend on the difference, so the three contracts are kept and named, and
// each call site imports the one it already had.

const encoder = new TextEncoder();

/// The view-preserving part every contract shares: the typed-array cases,
/// which never fail. Returns `undefined` for anything that is not already
/// bytes, leaving the decision about that to the caller.
function viewOf(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  return undefined;
}

/// Strict: bytes or a UTF-8 encoding of a string, and a `TypeError` for
/// anything else. For callers that are sending the result somewhere it must
/// be real bytes -- a request body, a guest filesystem write -- where a
/// silent `null` would surface much later as a confusing failure.
export function bytesOf(value) {
  const view = viewOf(value);
  if (view) return view;
  if (typeof value === "string") return encoder.encode(value);
  throw new TypeError("expected bytes");
}

/// Nullable: bytes, or `null` for `null`/`undefined` and for anything that is
/// not already bytes -- a string included. For callers that are asking "is
/// there a byte payload here at all?" and have a branch for "no".
export function maybeBytes(value) {
  if (value == null) return null;
  return viewOf(value) ?? null;
}

/// Total: always returns a `Uint8Array`. `null`/`undefined` become empty, and
/// anything that is not bytes is stringified and encoded. For hashing inputs,
/// where every value has to reduce to something and an empty digest input is
/// a meaningful answer rather than an error.
export function bytesOrEmpty(value) {
  if (value == null) return new Uint8Array(0);
  return viewOf(value) ?? encoder.encode(String(value));
}

/// The backing buffer for exactly this view, copied when the view is a window
/// onto a larger buffer. Structured-clone targets (`postMessage`) and APIs
/// that want an `ArrayBuffer` must not be handed the neighbours' bytes.
export function toArrayBuffer(bytes) {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}
