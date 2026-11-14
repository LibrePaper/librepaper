import { authHeaders } from "../api.js";
import { sha256 } from "../results-hash.js";

// The shared results of a run document: one record the owner's storage
// holds, uploaded after a local run and fetched by everyone who opens the
// document. A Quarto record holds per-cell HTML, and a Typst record holds the
// files Calepin stored. Nothing here throws into the preview path.

const endpoint = (slug) => `/api/documents/${encodeURIComponent(slug)}/results`;

/**
 * The record, null when the server says there is none, or undefined when
 * the answer could not be had (a network or server error), so a caller can
 * keep what it already shows.
 * @param {string} slug
 * @param {Record<string, string>} [headers]
 */
export async function fetchResults(slug, headers = authHeaders("")) {
  try {
    const response = await fetch(endpoint(slug), { headers });
    if (response.status === 404) return null;
    if (!response.ok) return undefined;
    const body = await response.json();
    return body && (Array.isArray(body.cells) || Array.isArray(body.files)) ? body : null;
  } catch { return undefined; }
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
 * The state of one open document's shared results (Quarto cells or Typst
 * files, see the top of this module): the record everyone sees,
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
  let uploadedAt = "";

  let generation = 0;

  async function refresh() {
    const ticket = ++generation;
    const next = await fetchResults(slug, headers());
    // A slower, older answer never overwrites a newer one.
    if (ticket !== generation) return state.record;
    // A failed fetch keeps the outputs already shown; the next notification
    // or reconnect fetches again.
    if (next === undefined) return state.record;
    state.record = next;
    // Someone else's upload, or a delete, replaced what this tab sent: the
    // next run uploads even when its output equals this tab's last one.
    if ((next?.updatedAt ?? "") !== uploadedAt) uploadedHash = "";
    return next;
  }

  /**
   * Uploads `record` unless it is identical to the last one this tab sent.
   * `allowed` is asked again right before the request, so turning sharing
   * off while the record is hashed still stops it.
   * @param {unknown} record
   * @param {() => boolean} [allowed]
   */
  async function upload(record, allowed = () => true) {
    try {
      const hash = await sha256(JSON.stringify(record));
      if (hash === uploadedHash) return state.status;
      if (!allowed()) return state.status;
      const result = await putResults(slug, record, { ...headers(), "content-type": "application/json" });
      if (result.ok) {
        uploadedHash = hash;
        uploadedAt = result.updatedAt;
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
    reset() { uploadedHash = ""; uploadedAt = ""; state.status = { kind: "idle" }; },
  };
}
