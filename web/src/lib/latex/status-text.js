// Pure text and mapping helpers for the compile status line.
//
// `LatexStatus.svelte`, `Diagnostics.svelte` and `Reader.svelte`'s
// `renderedNote` all need to turn a `Status` or a `Provenance` into a
// sentence a reader can act on. None of that needs a DOM, a worker or
// `latex.js` itself, so it lives here, in plain functions a Node check can
// call directly (see tests/unit/latex-reader.mjs) -- the same reason
// `latex/status.js` keeps the store apart from `latex.js`.
//
// LaTeX is built in the browser and nowhere else, so there is one backend to
// name and no fallback to explain. Every wording below uses the controller
// messages verbatim; this file never invents a `status.message`.

/// The chip beside the status message: which backend is producing pages.
export function backendChip(status) {
  if (!status) return "";
  if (status.phase === "browser-biber") return "browser Biber";
  if (status.backend === "browser") return "browser";
  return "";
}

const ENGINE_NAMES = { pdflatex: "pdfLaTeX", xelatex: "XeLaTeX", lualatex: "LuaLaTeX" };

function engineName(engine) {
  return ENGINE_NAMES[engine] || engine || "";
}

const BIBLIOGRAPHY_NAMES = {
  bibtex: "BibTeX",
  "browser-biber": "browser Biber",
};

function bibliographyName(kind) {
  return BIBLIOGRAPHY_NAMES[kind] || kind;
}

/// The "Compiled with" sentence Diagnostics shows: engine, release, and the
/// bibliography backend when one ran. e.g. "Compiled in the browser with
/// pdfLaTeX (release r1); bibliography via browser Biber."
export function provenanceSentence(provenance) {
  if (!provenance) return "";
  const engine = engineName(provenance.engine);
  const detail = provenance.release ? ` (release ${provenance.release})` : "";
  const bibliography = provenance.bibliography
    ? `; bibliography via ${bibliographyName(provenance.bibliography)}`
    : "";
  return `Compiled in the browser${engine ? ` with ${engine}` : ""}${detail}${bibliography}.`;
}
