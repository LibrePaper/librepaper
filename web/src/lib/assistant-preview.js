import { snapshotDigest } from "./tree-digest.js";
import { diagnosticContext } from "./assistant-review.js";

const owns = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
const validPath = (path) => typeof path === "string" && path.length > 0 &&
  !path.startsWith("/") && !path.includes("\\") && !/[\u0000-\u001f]/.test(path) &&
  path.split("/").every((part) => part && part !== "." && part !== "..");

// Capture before the first await. The live session and renderer never share
// mutable text maps, file metadata or asset buffers with a candidate.
export function capturePreviewTree(tree) {
  return {
    main: tree.main,
    texts: { ...tree.texts },
    digests: { ...tree.digests },
    files: Object.fromEntries(Object.entries(tree.files || {}).map(([path, entry]) => [path, { ...entry }])),
    settings: tree.settings ? { ...tree.settings } : undefined,
    assets: Object.fromEntries(Object.entries(tree.assets || {}).map(([path, bytes]) => [path, new Uint8Array(bytes).slice()])),
    urls: { ...tree.urls },
  };
}

// Turn the immutable manifest fetched by the browser client into the same
// renderer tree used by the live editor. Candidate source is deliberately
// supplied separately from the manifest: a small control frame can identify
// a large file without carrying its contents through the chat relay.
export function candidateTree(candidate) {
  if (!candidate || typeof candidate !== "object") throw new Error("Candidate metadata is required.");
  const manifest = candidate.files || candidate.manifest;
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    throw new Error("Candidate file manifest is required.");
  }
  const sources = candidate.texts || candidate.sources;
  if (!sources || typeof sources !== "object" || Array.isArray(sources)) {
    throw new Error("Candidate source files are required.");
  }
  const files = {};
  const texts = {};
  const assets = {};
  const digests = {};
  for (const [path, raw] of Object.entries(manifest)) {
    if (!validPath(path) || !raw || typeof raw !== "object") {
      throw new Error("Candidate manifest contains an invalid file.");
    }
    const entry = {
      kind: raw.kind === "asset" ? "asset" : raw.kind === "text" ? "text" : "",
      ...((raw.id || raw.file_id) ? { id: String(raw.id || raw.file_id) } : {}),
      ...((raw.sha || raw.digest) ? { sha: String(raw.sha || raw.digest) } : {}),
      ...(Number.isInteger(raw.size) ? { size: raw.size } : {}),
    };
    if (!entry.kind || !entry.sha || !Number.isInteger(entry.size) || entry.size < 0) {
      throw new Error("Candidate manifest contains incomplete file metadata.");
    }
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

export async function previewCandidate({ request, tree: sourceTree, render, title = async () => "", digest = snapshotDigest }) {
  const referenced = request?.candidate;
  const tree = referenced ? capturePreviewTree(candidateTree(referenced)) : capturePreviewTree(sourceTree);
  if (referenced && sourceTree) {
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
  if (!request || typeof request.files !== "object" || Array.isArray(request.files) || !request.files) {
    if (!referenced) throw new Error("Candidate files are required.");
  }
  const baseRevision = request.base_revision || referenced?.base_revision;
  const revision = request.revision || referenced?.revision;
  if (!baseRevision || !revision) throw new Error("Candidate revisions are required.");
  if (!referenced && await digest(tree) !== baseRevision) {
    throw new Error("The document changed before verification. Read a fresh snapshot and prepare the candidate again.");
  }
  if (!referenced && request.main && request.main !== tree.main) {
    throw new Error("Candidate verification cannot change the document's main file.");
  }
  if (!referenced) {
    for (const [path, text] of Object.entries(request.files)) {
      if (!validPath(path) || typeof text !== "string" || !owns(tree.texts, path)) {
        throw new Error("Candidate changes must name existing text files in this document.");
      }
      Object.defineProperty(tree.texts, path, { value: text, enumerable: true, writable: true, configurable: true });
    }
  }
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
      dependency_hash: referenced?.dependency_hash || referenced?.dependencies_hash || null,
      settings_hash: referenced?.settings_hash || null,
      environment_reproducible: format !== "quarto",
    },
    output: typeof rendered.html === "string" ? "html" : rendered.pdf?.byteLength ? "pdf" : "none",
  };
}
