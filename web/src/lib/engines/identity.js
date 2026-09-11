// Browser-side identity for the shared results boundary. The server already
// validates these values; this helper keeps a stale or hand-written document
// from silently selecting an engine the browser cannot execute.

const ENGINES = new Set(["none", "quarto"]);
const DRAFT_FORMATS = new Set(["markdown", "html", "typst", "latex", "other"]);

function normalized(value) {
  return String(value || "").trim().toLowerCase();
}

export function resultEngineOf(value) {
  const explicit = typeof value === "string" ? value : value?.execution_engine ?? value?.executionEngine ?? value?.engine;
  if (explicit == null || explicit === "") throw new Error("Results engine is required.");
  const engine = normalized(explicit);
  if (!ENGINES.has(engine)) throw new Error(`Unsupported browser results engine: ${engine || "(empty)"}`);
  return engine;
}

export function bundleEngineOf(bundle) {
  const explicit = bundle?.engine ?? bundle?.execution_engine ?? bundle?.executionEngine;
  return resultEngineOf(explicit);
}

export function draftFormatOf(value) {
  const explicit = typeof value === "object" ? value?.draft_format ?? value?.draftFormat : null;
  if (explicit != null && explicit !== "") {
    const format = normalized(explicit);
    if (!DRAFT_FORMATS.has(format)) throw new Error(`Unsupported browser draft format: ${format || "(empty)"}`);
    return format;
  }
  throw new Error("Draft format is required.");
}

export function documentResultsIdentity(document) {
  const execution_engine = resultEngineOf(document);
  const draft_format = draftFormatOf(document);
  if (execution_engine === "quarto" && draft_format !== "markdown") {
    throw new Error("Quarto results require a Markdown draft format.");
  }
  return { execution_engine, draft_format };
}

export function validateResultsManifest(manifest, { requireQuarto = true } = {}) {
  const engine = bundleEngineOf(manifest);
  if (requireQuarto && engine !== "quarto") throw new Error(`Unsupported saved-results engine: ${engine}`);
  for (const asset of manifest?.assets || []) {
    const role = normalized(asset?.role);
    if (!role) throw new Error("Saved-results asset role is required.");
    if (role !== "display") throw new Error(`Unsupported saved-results asset role: ${role || "(empty)"}`);
  }
  return engine;
}

export { ENGINES as supportedResultEngines, DRAFT_FORMATS as supportedDraftFormats };
