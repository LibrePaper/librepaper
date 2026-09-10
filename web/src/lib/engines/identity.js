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
  // A bundle predating the discriminator is a Quarto bundle by contract. An
  // absent document discriminator has a different meaning: ordinary authored
  // documents have no computation engine.
  if (explicit == null || explicit === "") {
    if (value && typeof value === "object" && (value.schema === "librepaper-quarto-bundle/v1" || value.render_id && value.cells || normalized(value.source_format ?? value.sourceFormat) === "quarto")) return "quarto";
    return "none";
  }
  const engine = normalized(explicit);
  if (!ENGINES.has(engine)) throw new Error(`Unsupported browser results engine: ${engine || "(empty)"}`);
  return engine;
}

export function bundleEngineOf(bundle) {
  const explicit = bundle?.engine ?? bundle?.execution_engine ?? bundle?.executionEngine;
  return explicit == null || explicit === "" ? "quarto" : resultEngineOf(explicit);
}

export function draftFormatOf(value) {
  const explicit = typeof value === "object" ? value?.draft_format ?? value?.draftFormat : null;
  if (explicit != null && explicit !== "") {
    const format = normalized(explicit);
    if (!DRAFT_FORMATS.has(format)) throw new Error(`Unsupported browser draft format: ${format || "(empty)"}`);
    return format;
  }
  const source = normalized(typeof value === "string" ? value : value?.source_format ?? value?.sourceFormat);
  if (!source) return "html";
  if (source === "quarto" || source === "markdown") return "markdown";
  if (["html", "typst", "latex"].includes(source)) return source;
  return "other";
}

export function documentResultsIdentity(document) {
  const execution_engine = resultEngineOf(document);
  const draft_format = draftFormatOf(document);
  const source = normalized(document?.source_format ?? document?.sourceFormat);
  if (execution_engine === "quarto" && draft_format !== "markdown") {
    throw new Error("Quarto results require a Markdown draft format.");
  }
  if (execution_engine === "none" && source === "quarto") {
    throw new Error("Legacy Quarto source cannot use the none execution engine.");
  }
  return { execution_engine, draft_format };
}

export function validateResultsManifest(manifest, { requireQuarto = true } = {}) {
  const engine = bundleEngineOf(manifest);
  if (requireQuarto && engine !== "quarto") throw new Error(`Unsupported saved-results engine: ${engine}`);
  for (const asset of manifest?.assets || []) {
    const role = normalized(asset?.role || "display");
    if (role !== "display") throw new Error(`Unsupported saved-results asset role: ${role || "(empty)"}`);
  }
  return engine;
}

export function browserCanExecute(engine) {
  return resultEngineOf(engine) === "quarto";
}

export { ENGINES as supportedResultEngines, DRAFT_FORMATS as supportedDraftFormats };
