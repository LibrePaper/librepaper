// Canonical browser/local builder vocabulary. Capability responses only overlay
// this catalog; they never create executable choices by themselves.
//
// LaTeX is a browser format. The companion's TeX adapters exist only for the
// automatic Biber and native-fallback path in `latex.js`, which routes on its
// own and never consults this catalog, so no local LaTeX builder is offered
// here and `engines` is the browser engine choice.
export const BUILDERS = Object.freeze([
  { id: "tex", label: "pdfLaTeX / XeLaTeX", formats: ["latex"], backend: ["browser"], engines: ["pdflatex", "xelatex"] },
  { id: "typst", label: "Typst", formats: ["typst"], backend: ["browser", "local"], engines: [] },
  { id: "calepin", label: "Calepin", formats: ["typst"], backend: ["local"], engines: [] },
  { id: "markdown", label: "Markdown", formats: ["markdown", "quarto"], backend: ["browser"], engines: [] },
  { id: "pandoc", label: "Pandoc", formats: ["markdown"], backend: ["local"], engines: [] },
  { id: "quarto", label: "Quarto", formats: ["markdown", "quarto"], backend: ["local"], engines: [] },
]);

export function buildersFor(format) { return BUILDERS.filter((builder) => builder.formats.includes(format)); }

// A companion that does not speak protocol 2 never reaches "connected"
// (`companion/client.js`), so a capability response always carries `builders`,
// and every builder record carries its own explicit operations.
export function capabilityFor(capabilities, id) {
  const list = capabilities?.builders;
  return Array.isArray(list) ? list.find((entry) => entry.id === id) || null : null;
}

export function operationsFor(capabilities, id) {
  const operations = capabilityFor(capabilities, id)?.operations;
  return Array.isArray(operations) ? operations : [];
}

export function supportsOperation(capabilities, id, kind = "build", mode = "snapshot") {
  return operationsFor(capabilities, id).some((operation) => operation.kind === kind && operation.workspace_modes?.includes(mode));
}
