import { rendererRequest } from "./renderer-client.js";

/// Parses the document's bibliography into the library completion matches
/// against. Independent of the renderer, because an author writing typst or
/// LaTeX gets completions too and neither of those compilers can read a `.bib`
/// for the editor: typst.wasm does not export `bibliography`, and the TeX
/// engines are another toolchain entirely.
///
/// Markdown with citations is the exception, and worth taking. That document is
/// rendered by citations.wasm, which carries the same parser and exports the
/// same `bibliography` -- see renderers.js, which chooses it on this very
/// condition. Asking the module already loaded for the preview saves fetching,
/// compiling and holding a second one.
export function analyzeBibliography(request) {
  const modules = globalThis.LIBREPAPER_MODULES || {};
  const alreadyLoaded =
    request.format === "markdown" && needsBibliography({ source: request.source || "" });
  const url = (alreadyLoaded && modules.citations) || modules.bibliography;
  if (!url) return Promise.reject(new Error("Bibliography support is unavailable in this build."));
  return rendererRequest(new URL(url, globalThis.location.href).href, "bibliography", {
    main: request.main || "", format: request.format || "", source: request.source || "", texts: { ...request.texts },
  });
}

export function needsBibliography({ source = "" } = {}) {
  return /^bibliography\s*:/m.test(source) || /(^|[^\p{L}\p{N}_/:.@-])-?@[\p{L}\p{N}_]/u.test(source);
}
