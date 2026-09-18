import { newRequestKey } from "./request-key.js";
import { authHeaders } from "./api.js";

const encoder = new TextEncoder();

/** A bundle is the only document payload a reader is allowed to load. */
export const bundleUrl = (slug) =>
  `/api/documents/${encodeURIComponent(slug)}/bundle`;

export async function sha256(value) {
  const bytes = typeof value === "string" ? encoder.encode(value) : value;
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function digestBundle({ html, assets = [] }) {
  const htmlBytes = typeof html === "string" ? encoder.encode(html) : html;
  const htmlHash = await sha256(htmlBytes);
  const manifestAssets = [];
  for (const asset of assets) {
    const bytes = typeof asset.bytes === "string" ? encoder.encode(asset.bytes) : asset.bytes;
    if (!(bytes instanceof Uint8Array) && !(bytes instanceof ArrayBuffer)) {
      throw new TypeError(`asset ${asset.path || ""} has no bytes`);
    }
    const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    manifestAssets.push({
      path: asset.path,
      sha256: await sha256(view),
      bytes: view.byteLength,
      mime: asset.mime || "application/octet-stream",
      bytesValue: view,
    });
  }
  const ordered = manifestAssets.slice().sort((left, right) => {
    const a = encoder.encode(left.path), b = encoder.encode(right.path);
    for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
      if (a[index] !== b[index]) return a[index] - b[index];
    }
    return a.length - b.length;
  });
  const descriptor = JSON.stringify({ html: htmlHash, assets: ordered.map(({ bytesValue, ...item }) => item) });
  return {
    digest: await sha256(descriptor),
    html: { sha256: htmlHash, bytes: htmlBytes.byteLength, mime: "text/html", bytesValue: htmlBytes },
    assets: manifestAssets,
  };
}

async function json(response) {
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(body.error || `${response.status}`);
    error.status = response.status;
    throw error;
  }
  return body;
}

export function createBundlePublisher({ slug, key = "", fetcher = globalThis.fetch } = {}) {
  const request = (path, options = {}) => fetcher(path, {
    ...options,
    headers: { ...authHeaders(key, options.body ? "application/json" : ""), ...(options.headers || {}) },
  }).then(json);

  async function publish({ html, assets = [], sourceRevision, renderConfig, expectedBundleId = null, idempotencyKey = newRequestKey(), onProgress = () => {} }) {
    const digested = await digestBundle({ html, assets });
    const sourceSha = sourceRevision || await sha256("");
    const renderConfigSha = await sha256(JSON.stringify(renderConfig || {}));
    const manifest = {
      bundle_sha256: digested.digest,
      source_sha256: sourceSha,
      render_config_sha256: renderConfigSha,
      html: { sha256: digested.html.sha256, bytes: digested.html.bytes, mime: digested.html.mime },
      assets: digested.assets.map(({ bytesValue, ...asset }) => asset),
    };
    onProgress({ phase: "checking", manifest });
    const missing = await request(`${bundleUrl(slug)}/prepare`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ manifest, expected_bundle_id: expectedBundleId }),
    });
    const uniqueObjects = new Map([digested.html, ...digested.assets].map((object) => [object.sha256, object]));
    const objects = [...uniqueObjects.values()].filter((object) =>
      missing.missing.includes(object.sha256));
    for (const object of objects) {
      onProgress({ phase: "uploading", hash: object.sha256, completed: objects.indexOf(object), total: objects.length });
      let payload = object.bytesValue;
      let compressed = false;
      if (object.mime.startsWith("text/") || object.mime === "application/javascript" || object.mime === "application/json") {
        try {
          const original = payload;
          const stream = new Blob([payload]).stream().pipeThrough(new CompressionStream("gzip"));
          const zipped = new Uint8Array(await new Response(stream).arrayBuffer());
          if (zipped.byteLength < original.byteLength) {
            payload = zipped;
            compressed = true;
          }
        } catch { /* CompressionStream is optional; decoded uploads remain valid. */ }
      }
      await request(`${bundleUrl(slug)}/objects/${object.sha256}`, {
        method: "PUT",
        headers: {
          "content-type": object.mime,
          "Idempotency-Key": idempotencyKey,
          ...(compressed ? { "content-encoding": "gzip" } : {}),
        },
        body: payload,
      });
    }
    onProgress({ phase: "activating", manifest });
    const result = await request(`${bundleUrl(slug)}/activate`, {
      method: "POST",
      headers: {
        "Idempotency-Key": idempotencyKey,
      },
      body: JSON.stringify({ manifest, expected_bundle_id: expectedBundleId }),
    });
    const bundle = result.bundle;
    onProgress({ phase: "published", bundle });
    return bundle;
  }

  return { publish };
}

export function createBundleReader({ slug, key = "", fetcher = globalThis.fetch, onBundle = () => {}, onError = () => {} } = {}) {
  let disposed = false;
  let current = null;
  async function refresh() {
    try {
      const response = await fetcher(bundleUrl(slug), { headers: authHeaders(key) });
      const value = await json(response);
      if (disposed) return null;
      current = value.bundle;
      onBundle(current);
      return current;
    } catch (error) {
      if (!disposed) onError(error);
      return null;
    }
  }
  function announce(value) {
    const id = value?.id || value?.bundle_id;
    const currentId = current?.id;
    if (!id || id === currentId) return false;
    onBundle({ ...current, pending_id: id, update_available: true });
    return true;
  }
  function dispose() { disposed = true; }
  return { refresh, announce, dispose, current: () => current };
}
