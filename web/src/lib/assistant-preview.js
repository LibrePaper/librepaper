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

export async function previewCandidate({ request, tree: sourceTree, render, title = async () => "", digest = snapshotDigest }) {
  const tree = capturePreviewTree(sourceTree);
  if (!request || typeof request.files !== "object" || Array.isArray(request.files) || !request.files) {
    throw new Error("Candidate files are required.");
  }
  if (!request.base_revision || await digest(tree) !== request.base_revision) {
    throw new Error("The document changed before verification. Read a fresh snapshot and prepare the candidate again.");
  }
  for (const [path, text] of Object.entries(request.files)) {
    if (!validPath(path) || typeof text !== "string" || !owns(tree.texts, path)) {
      throw new Error("Candidate changes must name existing text files in this document.");
    }
    Object.defineProperty(tree.texts, path, { value: text, enumerable: true, writable: true, configurable: true });
  }
  if (request.main && request.main !== tree.main) throw new Error("Candidate verification cannot change the document's main file.");
  if (!owns(tree.texts, tree.main)) throw new Error("The main source file is unavailable.");
  if (Object.keys(tree.digests).some((path) => !owns(tree.assets, path))) {
    throw new Error("Document assets are unavailable for verification.");
  }
  const revision = await digest(tree);
  if (revision !== request.revision) throw new Error("The candidate revision does not match its source files.");
  const rendered = await render(tree, await title(tree));
  const diagnostics = (rendered.diagnostics || []).map((item) => diagnosticContext(item, tree, revision));
  const hasOutput = typeof rendered.html === "string" || Boolean(rendered.pdf?.byteLength);
  return {
    revision,
    base_revision: request.base_revision,
    ok: rendered.ok !== false && hasOutput && !diagnostics.some((item) => !["warning", "info", "hint"].includes(item.severity)),
    diagnostics,
    output: typeof rendered.html === "string" ? "html" : rendered.pdf?.byteLength ? "pdf" : "none",
  };
}
