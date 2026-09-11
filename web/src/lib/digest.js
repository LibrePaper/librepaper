// SHA-256, as hex, in one place.
//
// Nine call sites derived this independently -- `tree-digest.js`,
// `latex/local.js`, `latex/bibliography.js`, `latex/jobs.js`,
// `latex/resources.js`, `latex/worker.js`, `results-hash.js`,
// `results-artifact.js` and `agent/agent.js` -- each re-writing the same
// `digest -> Uint8Array -> map to padded hex` reduction. They all agreed;
// there was simply no reason for nine of them.
//
// No imports: this is pulled in by the module worker, by the separately
// bundled in-frame agent, and by the shell, and it must stay cheap for all
// three.

const encoder = new TextEncoder();

// Hex for a digest buffer, kept separate only because `sha256Hex` reads
// better split in two.
function toHex(buffer) {
  return Array.from(new Uint8Array(buffer), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// The digest of some bytes, lower-case hex, full 64 characters.
export async function sha256Hex(bytes) {
  if (!globalThis.crypto?.subtle) throw new Error("Web Crypto SHA-256 is required");
  return toHex(await crypto.subtle.digest("SHA-256", bytes));
}

/// The digest of a string, UTF-8 encoded first. The common case for identity
/// keys built by joining already-canonical values.
export async function sha256HexOfText(text) {
  return sha256Hex(encoder.encode(text));
}
