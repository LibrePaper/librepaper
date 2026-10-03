import { snapshotDigest } from "./projection-digest.js";
import { diagnosticContext } from "./assistant-review.js";

const owns = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
// Shared with agent-client.js, which validates the same candidate paths
// fetched over authenticated same-origin requests.
export const validPath = (path) => typeof path === "string" && path.length > 0 &&
  !path.startsWith("/") && !path.includes("\\") && !/[\u0000-\u001f]/.test(path) &&
  path.split("/").every((part) => part && part !== "." && part !== "..");

// Capture before the first await. The live session and renderer never share
// mutable text maps, file metadata or asset buffers with a candidate.
/** @typedef {{main?: string, texts?: Record<string, string>, digests?: Record<string, string>, files?: Record<string, {kind?: string, sha?: string, size?: number}>, settings?: Record<string, unknown>, assets?: Record<string, Uint8Array<ArrayBufferLike>>, urls?: Record<string, string>}} PreviewTree */
/** @typedef {Required<Pick<PreviewTree, "texts"|"digests"|"files"|"assets"|"urls">> & PreviewTree} CapturedPreviewTree */
/** @param {PreviewTree} tree @returns {CapturedPreviewTree} */
export function capturePreviewTree(tree) {
  return {
    main: tree.main,
    texts: { ...tree.texts },
    digests: { ...tree.digests },
    files: Object.fromEntries(Object.entries(tree.files || {}).map(([path, entry]) => [path, { ...entry }])),
    settings: tree.settings ? { ...tree.settings } : undefined,
    assets: Object.fromEntries(Object.entries(tree.assets || {}).map(([path, bytes]) => [path, new Uint8Array(bytes).slice()])),
    urls: { ...(tree.urls || {}) },
  };
}

// Turn the immutable manifest fetched by the browser client into the same
// renderer tree used by the live editor. Candidate source is deliberately
// supplied separately from the manifest: a small control frame can identify
// a large file without carrying its contents through the chat relay.
export function candidateTree(candidate) {
  if (!candidate || typeof candidate !== "object") throw new Error("Candidate metadata is required.");
  const manifest = candidate.files;
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    throw new Error("Candidate file manifest is required.");
  }
  const sources = candidate.texts;
  if (!sources || typeof sources !== "object" || Array.isArray(sources)) {
    throw new Error("Candidate source files are required.");
  }
  /** @type {Record<string, {kind: "asset"|"text", sha: string, size: number}>} */
  const files = {};
  /** @type {Record<string, string>} */
  const texts = {};
  /** @type {Record<string, Uint8Array>} */
  const assets = {};
  /** @type {Record<string, string>} */
  const digests = {};
  for (const [path, raw] of Object.entries(manifest)) {
    if (!validPath(path) || !raw || typeof raw !== "object") {
      throw new Error("Candidate manifest contains an invalid file.");
    }
    if (raw.kind !== "asset" && raw.kind !== "text") {
      throw new Error("Candidate manifest contains incomplete file metadata.");
    }
    const kind = raw.kind;
    const sha = raw.sha ? String(raw.sha) : "";
    const size = raw.size;
    if (!sha || !Number.isInteger(size) || size < 0) {
      throw new Error("Candidate manifest contains incomplete file metadata.");
    }
    const entry = { kind, sha, size };
    files[path] = entry;
    if (entry.kind === "text") {
      if (!Object.prototype.hasOwnProperty.call(sources, path) || typeof sources[path] !== "string") {
        throw new Error(`Candidate source for ${path} is unavailable.`);
      }
      texts[path] = sources[path];
    } else {
      digests[path] = entry.sha;
    }
  }
  if (typeof candidate.main !== "string" || !validPath(candidate.main) || !files[candidate.main]) {
    throw new Error("Candidate main file is unavailable.");
  }
  return {
    main: candidate.main,
    files,
    texts,
    digests,
    assets,
    settings: candidate.settings && typeof candidate.settings === "object" ? { ...candidate.settings } : undefined,
  };
}

// The server always sends a candidate reference (`request.candidate`): a
// manifest plus the source files the agent client fetched over authenticated
// same-origin requests, never inline edits carried in the chat frame itself.
/** @param {{request: {candidate?: {main?: string, files?: Record<string, {kind: "asset"|"text", sha: string, size: number}>, texts?: Record<string, string>, settings?: Record<string, unknown>, base_revision?: string, revision?: string, dependency_hash?: string, settings_hash?: string}, base_revision?: string, revision?: string}, tree?: {main?: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>, digests?: Record<string, string>, files?: Record<string, {sha?: string}>}, render: (tree: object, title: string) => Promise<{html?: string|null, pdf?: Uint8Array|null, diagnostics?: {severity: string, message: string, file?: string, line?: number, column?: number}[], provenance?: {engine?: string}, ok?: boolean}>, title?: (tree: object) => Promise<string>, digest?: (tree: object) => Promise<string>}} options */
export async function previewCandidate({ request, tree: sourceTree, render, title = async (_tree) => "", digest = snapshotDigest }) {
  const referenced = request?.candidate;
  if (!referenced) throw new Error("Candidate reference is required.");
  const tree = capturePreviewTree(candidateTree(referenced));
  if (sourceTree) {
    // Reader gathers candidate assets after constructing the metadata tree.
    // Preserve only bytes whose path and digest still match the immutable
    // candidate manifest; rebuilding from metadata must not discard them.
    for (const [path, sha] of Object.entries(tree.digests || {})) {
      const sourceSha = sourceTree.digests?.[path] || sourceTree.files?.[path]?.sha;
      if (sourceSha !== sha || !owns(sourceTree.assets || {}, path)) continue;
      tree.assets[path] = new Uint8Array(sourceTree.assets[path]).slice();
      if (sourceTree.urls?.[path]) tree.urls[path] = sourceTree.urls[path];
    }
  }
  const baseRevision = request.base_revision || referenced.base_revision;
  const revision = request.revision || referenced.revision;
  if (!baseRevision || !revision) throw new Error("Candidate revisions are required.");
  if (!owns(tree.texts, tree.main)) throw new Error("The main source file is unavailable.");
  if (Object.keys(tree.digests).some((path) => !owns(tree.assets, path))) {
    throw new Error("Document assets are unavailable for verification.");
  }
  const candidateRevision = await digest(tree);
  if (candidateRevision !== revision) throw new Error("The candidate revision does not match its source files.");
  const rendered = await render(tree, await title(tree));
  const diagnostics = (rendered.diagnostics || []).map((item) => diagnosticContext(item, tree, candidateRevision));
  const hasOutput = typeof rendered.html === "string" || Boolean(rendered.pdf?.byteLength);
  const main = tree.main.toLowerCase();
  const format = main.endsWith(".qmd") ? "quarto" : main.endsWith(".typ") ? "typst"
    : main.endsWith(".tex") || main.endsWith(".ltx") ? "latex"
      : main.endsWith(".html") || main.endsWith(".htm") ? "html" : "markdown";
  const engine = rendered.provenance?.engine || format;
  return {
    revision: candidateRevision,
    base_revision: baseRevision,
    ok: rendered.ok !== false && hasOutput && !diagnostics.some((item) => !["warning", "info", "hint"].includes(item.severity)),
    diagnostics,
    provenance: {
      ...(rendered.provenance && typeof rendered.provenance === "object" ? rendered.provenance : {}),
      renderer: "browser",
      engine,
      implementation: globalThis.__LIBREPAPER_RENDERER_BUILD__ || "web-renderer",
      dependency_hash: referenced.dependency_hash || null,
      settings_hash: referenced.settings_hash || null,
      environment_reproducible: format !== "quarto",
    },
    output: typeof rendered.html === "string" ? "html" : rendered.pdf?.byteLength ? "pdf" : "none",
  };
}
