/** Create once per mutation intent; retries must reuse the returned key. */
export function newRequestKey(now = Date.now()) {
  if (!Number.isSafeInteger(now) || now < 0) throw new TypeError("Invalid request timestamp");
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return `v2.${now}.${Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("")}`;
}
