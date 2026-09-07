// A bloom filter over TeX Live package keys, in the exact binary layout and
// hash the WasmTex workers parse (`loadbloom` / `bloomCheck` in
// `wasm-build/pdftex-worker.js`, shared by xetex/luatex/etc, which all fetch
// package files through the same "pdftex/<format>/<name>" URL space).
//
// Binary layout: `BF01` magic, one byte `k` (hash count), four bytes `m`
// (bit count, big-endian), then `ceil(m/8)` bytes of bits.
//
// Hash: FNV-1a based double hashing, `h_i = (h1 + i*h2) mod m`, exactly as
// `fnv1a`/`bloomCheck` compute it in the worker. The key a worker hashes is
// `"<kpathsea format code>/<bare name>"` -- no engine prefix, because the
// bloom filter lives in the URL space every engine's package fetch shares
// (`pdftex/<format>/<name>`), not in the manifest's `pdftex/<format>/<name>`
// key space (whose leading `pdftex` is a fixed label, not the engine that
// asked). Callers pass manifest-shaped keys (`pdftex/26/amsmath.sty`) and
// this strips the label before hashing.
//
// This file has no dependency on the worker source; it reimplements the tiny
// amount of arithmetic that binary format needs, so it can be required by
// both the mirror builder and its own self-test without reaching into
// `latex/benchmark`.

function fnv1a(str) {
  let h1 = 0x811c9dc5 | 0;
  let h2 = 0x01000193 | 0;
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    h1 = h1 ^ c;
    h1 = Math.imul(h1, 0x01000193);
    h2 = h2 ^ c;
    h2 = Math.imul(h2, 0x01000193);
  }
  return [h1 >>> 0, h2 >>> 0];
}

/// A manifest key `pdftex/<format>/<name>` to the bare key the worker hashes,
/// `<format>/<name>`. Manifest keys always start with the literal `pdftex/`
/// label (see `docs/specs/wasmtex-interfaces.md` section 1); anything else is
/// not a key this bloom filter can represent and is rejected loudly rather
/// than silently mis-hashed.
export function bloomKey(manifestKey) {
  const at = manifestKey.indexOf("/");
  const label = manifestKey.slice(0, at);
  if (label !== "pdftex") {
    throw new Error(`bloom: key ${manifestKey} does not start with the "pdftex/" label`);
  }
  return manifestKey.slice(at + 1);
}

/// Bits-per-key and hash count chosen for a false-positive rate around 1%,
/// the standard `m = -n*ln(p)/(ln2)^2`, `k = (m/n)*ln2` formulas, rounded to
/// whole numbers a byte array can hold.
function sizeFor(n) {
  const p = 0.01;
  const m = Math.max(64, Math.ceil((-n * Math.log(p)) / Math.LN2 ** 2));
  const k = Math.max(1, Math.min(255, Math.round((m / Math.max(n, 1)) * Math.LN2)));
  return { m, k };
}

/// Builds the filter's bytes over a set of manifest keys
/// (`pdftex/<format>/<name>`). Empty input still produces a well-formed,
/// tiny filter -- every `bloomCheck` on it answers false, which is correct:
/// nothing is recorded present yet.
export function buildBloom(manifestKeys) {
  const keys = [...manifestKeys].map(bloomKey);
  const { m, k } = sizeFor(keys.length);
  const bytes = new Uint8Array(Math.ceil(m / 8));
  for (const key of keys) {
    const [h1, h2] = fnv1a(key);
    for (let i = 0; i < k; i++) {
      const bit = ((h1 + Math.imul(i, h2)) >>> 0) % m;
      bytes[bit >>> 3] |= 1 << (bit & 7);
    }
  }
  const header = new Uint8Array(9);
  header[0] = 0x42; // 'B'
  header[1] = 0x46; // 'F'
  header[2] = 0x30; // '0'
  header[3] = 0x31; // '1'
  header[4] = k;
  header[5] = (m >>> 24) & 0xff;
  header[6] = (m >>> 16) & 0xff;
  header[7] = (m >>> 8) & 0xff;
  header[8] = m & 0xff;
  return Buffer.concat([Buffer.from(header), Buffer.from(bytes)]);
}

/// Re-parses a filter's bytes and re-implements `bloomCheck` exactly as the
/// worker does, so a caller can ask "does this filter answer positive for
/// every key I built it from" without loading the worker itself. Used both
/// as `wasmtex.mjs`'s post-build self-test and by anyone auditing a mirror.
export function bloomCheck(bytes, manifestKey) {
  if (bytes.length < 9 || bytes[0] !== 0x42 || bytes[1] !== 0x46 || bytes[2] !== 0x30 || bytes[3] !== 0x31) {
    throw new Error("bloom: bad magic bytes");
  }
  const k = bytes[4];
  const m = (bytes[5] << 24) | (bytes[6] << 16) | (bytes[7] << 8) | bytes[8];
  const bits = bytes.subarray(9);
  const [h1, h2] = fnv1a(bloomKey(manifestKey));
  for (let i = 0; i < k; i++) {
    const bit = ((h1 + Math.imul(i, h2)) >>> 0) % m;
    if ((bits[bit >>> 3] & (1 << (bit & 7))) === 0) return false;
  }
  return true;
}

/// Self-test: every key the filter was built from must test positive. A
/// bloom filter's whole contract is "no false negatives", so this is not a
/// probabilistic check -- any failure here means the build or the hash is
/// wrong, not that the filter got unlucky.
export function verifyBloom(bytes, manifestKeys) {
  const missing = [];
  for (const key of manifestKeys) {
    if (!bloomCheck(bytes, key)) missing.push(key);
  }
  return { ok: missing.length === 0, missing };
}
