import { authHeaders } from "../api.js";
import { sha256 } from "../results-hash.js";

// The shared results of a Quarto document: one record the owner's storage
// holds, uploaded after a local run and fetched by everyone who opens the
// document. Nothing here throws into the preview path.

const endpoint = (slug) => `/api/documents/${encodeURIComponent(slug)}/results`;

/**
 * The record, or null when there is none (or it cannot be read).
 * @param {string} slug
 * @param {Record<string, string>} [headers]
 */
export async function fetchResults(slug, headers = authHeaders("")) {
  try {
    const response = await fetch(endpoint(slug), { headers });
    if (response.status === 404 || !response.ok) return null;
    const body = await response.json();
    return body && Array.isArray(body.cells) ? body : null;
  } catch { return null; }
}

/**
 * @param {string} slug
 * @param {unknown} record
 * @param {Record<string, string>} [headers]
 * @returns {Promise<{ok: true, bytes: number, updatedAt: string} | {ok: false, status: number, error: string}>}
 */
export async function putResults(slug, record, headers = authHeaders("", "application/json")) {
  try {
    const response = await fetch(endpoint(slug), { method: "PUT", headers, body: JSON.stringify(record) });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) return { ok: false, status: response.status, error: String(body.error || response.status) };
    return { ok: true, bytes: Number(body.bytes) || 0, updatedAt: String(body.updatedAt || "") };
  } catch (error) {
    return { ok: false, status: 0, error: String(error?.message || error) };
  }
}

/**
 * The state of one open document's shared results: the record everyone sees,
 * the hash of the last record this tab uploaded, and how that upload went.
 * @param {string} slug
 * @param {() => Record<string, string>} headers The request headers, read per call.
 */
export function createSharedResults(slug, headers = () => authHeaders("")) {
  const state = $state({
    /** @type {any} */ record: null,
    /** @type {{kind: "idle" | "shared" | "failed" | "skipped", bytes?: number, reason?: "storage" | "unmatched" | "network"}} */
    status: { kind: "idle" },
  });
  let uploadedHash = "";

  async function refresh() {
    const next = await fetchResults(slug, headers());
    state.record = next;
    return next;
  }

  /** Uploads `record` unless it is identical to the last one this tab sent. */
  async function upload(record) {
    try {
      const hash = await sha256(JSON.stringify(record));
      if (hash === uploadedHash) return state.status;
      const result = await putResults(slug, record, { ...headers(), "content-type": "application/json" });
      if (result.ok) {
        uploadedHash = hash;
        state.status = { kind: "shared", bytes: result.bytes };
      } else {
        state.status = { kind: "failed", reason: "status" in result && result.status === 413 ? "storage" : "network" };
      }
    } catch {
      state.status = { kind: "failed", reason: "network" };
    }
    return state.status;
  }

  /** The run produced nothing to share, or sharing does not apply to it. */
  function skip(reason) {
    state.status = reason === "unmatched" ? { kind: "failed", reason } : { kind: "skipped", reason };
  }

  return {
    state,
    refresh,
    upload,
    skip,
    reset() { uploadedHash = ""; state.status = { kind: "idle" }; },
  };
}
