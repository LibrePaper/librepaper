// Canonical browser/local builder vocabulary. Capability responses only overlay
// this catalog; they never create executable choices by themselves.
export const BUILDERS = Object.freeze([
  { id: "tex", label: "pdfLaTeX / XeLaTeX", formats: ["latex"], backend: ["browser", "local"], engines: ["pdflatex", "xelatex", "lualatex"], minProtocol: 1 },
  { id: "latexmk", label: "latexmk", formats: ["latex"], backend: ["local"], engines: ["pdflatex", "xelatex", "lualatex"], minProtocol: 2 },
  { id: "tectonic", label: "Tectonic", formats: ["latex"], backend: ["local"], engines: [], minProtocol: 2 },
  { id: "typst", label: "Typst", formats: ["typst"], backend: ["browser", "local"], engines: [], minProtocol: 2 },
  { id: "calepin", label: "Calepin", formats: ["typst"], backend: ["local"], engines: [], minProtocol: 2 },
  { id: "markdown", label: "Markdown", formats: ["markdown", "quarto"], backend: ["browser"], engines: [], minProtocol: 1 },
  { id: "pandoc", label: "Pandoc", formats: ["markdown"], backend: ["local"], engines: [], minProtocol: 2 },
  { id: "quarto", label: "Quarto", formats: ["markdown", "quarto"], backend: ["local"], engines: [], minProtocol: 1 },
]);

export function buildersFor(format) { return BUILDERS.filter((builder) => builder.formats.includes(format)); }
export function builder(id) { return BUILDERS.find((entry) => entry.id === id) || null; }

export function capabilityFor(capabilities, id) {
  const list = capabilities?.builders;
  if (Array.isArray(list)) return list.find((entry) => entry.id === id) || null;
  // Protocol 1 used tool-specific capability fields. Map only the fields
  // that prove the corresponding adapter exists.
  if (id === "tex") {
    const tools = capabilities?.tools || {};
    const engines = ["pdflatex", "xelatex", "lualatex"].filter((engine) => tools[engine]?.available);
    return engines.length ? { id, available: true, engines, outputs: ["pdf"], operations: [{ kind: "build", workspace_modes: ["snapshot"] }] } : null;
  }
  if (id === "quarto") {
    const tool = capabilities?.quarto?.tool;
    if (!tool?.available) return null;
    const operations = [{ kind: "preview", workspace_modes: ["bound"] }];
    if (capabilities?.quarto?.build || capabilities?.quarto?.tool?.build) operations.push({ kind: "build", workspace_modes: ["snapshot"] });
    return { id, available: true, outputs: ["html"], operations };
  }
  if (id === "calepin") {
    const tool = capabilities?.calepin?.tool || capabilities?.calepin;
    return tool?.available || tool?.found ? { id, available: true, outputs: ["html", "pdf"], operations: [{ kind: "preview", workspace_modes: ["bound"] }] } : null;
  }
  return null;
}

// Older companions expose tool-specific fields. Map only operations they
// actually advertise; preview capability never implies one-shot builds.
export function operationsFor(capabilities, id) {
  const entry = capabilityFor(capabilities, id);
  if (Array.isArray(entry?.operations)) return entry.operations;
  if (!entry) return [];
  if (id === "quarto" || id === "calepin") return entry.preview ? [{ kind: "preview", workspace_modes: ["bound"] }] : [];
  return entry.available ? [{ kind: "build", workspace_modes: ["snapshot"] }] : [];
}

export function available(entry, capabilities) {
  if (entry.backend.includes("browser") && !entry.backend.includes("local")) return true;
  const cap = capabilityFor(capabilities, entry.id);
  return cap?.available === true && (!entry.minProtocol || (capabilities.protocol || 2) >= entry.minProtocol);
}

export function supportsOperation(capabilities, id, kind = "build", mode = "snapshot") {
  return operationsFor(capabilities, id).some((operation) => operation.kind === kind && operation.workspace_modes?.includes(mode));
}
