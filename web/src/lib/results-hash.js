// Shared content hashing for browser result identities. This module deliberately
// has no source-format imports: saved-result comments and storage can be used by
// any engine adapter.

function stableStringify(value) {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export async function sha256Bytes(bytes) {
  if (!globalThis.crypto?.subtle) throw new Error("Web Crypto SHA-256 is required for result identity");
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((x) => x.toString(16).padStart(2, "0")).join("");
}

export async function sha256(value) {
  const bytes = new TextEncoder().encode(typeof value === "string" ? value : stableStringify(value));
  return sha256Bytes(bytes);
}

export { stableStringify };
