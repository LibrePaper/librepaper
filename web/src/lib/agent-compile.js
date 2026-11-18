// Answering an agent's compile request from the owner's open editor tab.
// The server relays a `compile_request` frame; this tab compiles the current
// tree and posts the outcome back. The messages here go to the agent, not to
// the interface, so they are plain English and not translated.

import { boundedDiagnostics } from "./diagnostic-bounds.js";

export const LOG_LIMIT = 16 * 1024;
const ID = /^compile_[0-9a-f]{32}$/;

/** @param {unknown} frame @param {number} [now] seconds since the epoch */
export function parseCompileRequest(frame, now = Date.now() / 1000) {
  if (!frame || typeof frame !== "object" || frame.type !== "compile_request") return null;
  const { id, path, output, run, expires_at: expires } = /** @type {Record<string, unknown>} */ (frame);
  if (typeof id !== "string" || !ID.test(id)) return null;
  if (typeof path !== "string" || !path || path.length > 1024) return null;
  if (output !== null && output !== undefined && output !== "pdf" && output !== "html") return null;
  if (typeof run !== "boolean") return null;
  if (typeof expires !== "number" || !Number.isFinite(expires) || expires <= now) return null;
  return { id, path, output: output ?? null, run, expires_at: expires };
}

/** The last `limit` bytes of `text`, never starting inside a character. */
export function tailBytes(text, limit = LOG_LIMIT) {
  const bytes = new TextEncoder().encode(String(text ?? ""));
  if (bytes.byteLength <= limit) return String(text ?? "");
  let start = bytes.byteLength - limit;
  // Skip continuation bytes (10xxxxxx) so the cut lands on a character.
  while (start < bytes.byteLength && (bytes[start] & 0xc0) === 0x80) start++;
  return new TextDecoder().decode(bytes.subarray(start));
}

/** @param {{ok?: boolean, html?: unknown, pdf?: {byteLength?: number}|null, diagnostics?: unknown[], log?: string, provenance?: unknown, failure?: {message?: string}}} result @param {string} revision */
export function compileReply(result, revision) {
  const rendered = result || {};
  const output = rendered.pdf?.byteLength ? "pdf" : typeof rendered.html === "string" ? "html" : "none";
  const diagnostics = boundedDiagnostics(rendered.diagnostics);
  const log = tailBytes(rendered.log || rendered.failure?.message || "");
  const provenance = rendered.provenance && typeof rendered.provenance === "object" ? rendered.provenance : {};
  return { revision, ok: rendered.ok !== false && output !== "none", output, diagnostics, log, provenance };
}

/** @param {"run_unavailable"|"run_disabled"|"busy"|"no_renderer"|"failed"} code @param {string} message */
export function errorReply(code, message) {
  return { error: code, message };
}

/** One request at a time. `compile(request)` resolves to a reply body;
 * `post(id, body)` sends it. A request that arrives while one runs is
 * answered busy, and an id seen before is ignored. */
export function createCompileResponder({ compile, post }) {
  const seen = new Set();
  let running = false;
  return {
    async handle(request) {
      if (!request || seen.has(request.id)) return;
      seen.add(request.id);
      if (seen.size > 256) seen.delete(seen.values().next().value);
      if (running) {
        await post(request.id, errorReply("busy", "This tab is already compiling another request."));
        return;
      }
      running = true;
      let body;
      try {
        body = await compile(request);
      } catch (error) {
        body = errorReply("failed", error?.message || "The compile failed.");
      } finally {
        running = false;
      }
      await post(request.id, body);
    },
  };
}
