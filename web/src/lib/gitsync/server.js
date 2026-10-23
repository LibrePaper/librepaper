import { authHeaders } from "../api.js";

// The server half of sync: the link record, the source-change route and the
// asset store, for one document. Every call carries the link key when the
// reader arrived with one, so a share-link editor can sync.

async function parse(response) {
  const body = await response.json().catch(() => null);
  if (!response.ok) {
    const error = /** @type {Error & { status?: number, body?: any }} */ (
      new Error(body?.error || `The server answered ${response.status}.`)
    );
    error.status = response.status;
    error.body = body || {};
    throw error;
  }
  return body;
}

/// The calls sync.js expects as `ctx.api`. Errors carry `status` and `body`,
/// so a 409 can be read.
export function syncApi(slug, key) {
  const base = `/api/documents/${slug}`;
  const send = (method, path, body) =>
    fetch(`${base}${path}`, {
      method,
      headers: authHeaders(key, "application/json"),
      body: JSON.stringify(body),
    }).then(parse);

  return {
    getLink: () => fetch(`${base}/sync`, { headers: authHeaders(key) }).then(parse),
    putLink: (body) => send("PUT", "/sync", body),
    async deleteLink() {
      const response = await fetch(`${base}/sync`, { method: "DELETE", headers: authHeaders(key) });
      if (!response.ok) await parse(response);
    },
    sourceChange: (body) => send("POST", "/source-change", body),
    uploadAsset: (bytes) =>
      fetch(`${base}/assets`, { method: "PUT", headers: authHeaders(key), body: bytes }).then(parse),
    // The asset route answers HEAD for a present asset with 200 and a missing
    // one with 404, without sending the body.
    async hasAsset(sha) {
      const response = await fetch(`${base}/assets/${sha}`, { method: "HEAD", headers: authHeaders(key) });
      if (response.ok) return true;
      if (response.status === 404) return false;
      await parse(response);
      return false;
    },
    async assetBytes(sha) {
      const response = await fetch(`${base}/assets/${sha}`, { headers: authHeaders(key) });
      if (!response.ok) await parse(response);
      return new Uint8Array(await response.arrayBuffer());
    },
  };
}
