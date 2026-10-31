// Pure text and mapping helpers for compile provenance.
//
// `Diagnostics.svelte` needs to turn a `Provenance` into a
// sentence a reader can act on. None of that needs a DOM, a worker or
// `latex.js` itself, so it lives here, in plain functions a Node check can
// call directly (see tests/unit/latex-reader.mjs) -- the same reason
// `latex/status.js` keeps the store apart from `latex.js`.
//
// LaTeX is built in the browser, or on this computer by the command Settings
// holds, so there are two backends to name and no fallback to explain. Every
// wording below uses the controller messages verbatim; this file never
// invents a `status.message`.

import { m } from "../paraglide/messages.js";

const ENGINE_NAMES = { pdflatex: "pdfLaTeX", xelatex: "XeLaTeX", lualatex: "LuaLaTeX" };

function engineName(engine) {
  return ENGINE_NAMES[engine] || engine || "";
}

const BIBLIOGRAPHY_NAMES = {
  bibtex: "BibTeX",
  "browser-biber": m.status_text_bibliography_browser_biber(),
};

function bibliographyName(kind) {
  return BIBLIOGRAPHY_NAMES[kind] || kind;
}

/// The "Compiled with" sentence Diagnostics shows: engine, release, and the
/// bibliography backend when one ran. e.g. "Compiled in the browser with
/// pdfLaTeX (release r1); bibliography via browser Biber."
export function provenanceSentence(provenance) {
  if (!provenance) return "";
  if (provenance.backend === "local") return m.status_text_rendered_local();
  const engine = engineName(provenance.engine);
  const release = provenance.release;
  if (provenance.bibliography) {
    const bibliography = bibliographyName(provenance.bibliography);
    if (engine && release) return m.status_text_browser_engine_release_bibliography({ engine, release, bibliography });
    if (engine) return m.status_text_browser_engine_bibliography({ engine, bibliography });
    if (release) return m.status_text_browser_release_bibliography({ release, bibliography });
    return m.status_text_browser_bibliography({ bibliography });
  }
  if (engine && release) return m.status_text_browser_engine_release({ engine, release });
  if (engine) return m.status_text_browser_engine({ engine });
  if (release) return m.status_text_browser_release({ release });
  return m.status_text_browser();
}
