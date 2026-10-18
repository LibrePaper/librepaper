// KaTeX for the application's own pages: the Symbols and Math palettes draw
// their tiles and previews with it. It is the copy tools/vendor-katex.mjs
// puts under /assets/katex-<version>/ for the document frame, fetched the
// way the frame fetches it, so the shell carries one KaTeX and one set of
// fonts rather than a second of each bundled from node_modules.
import { createKatexLoader } from "./math.js";

let load = null;

// Lazy, so importing this module where there is no document does nothing.
export function loadKatex() {
  load ??= createKatexLoader({ base: __KATEX__, document, window });
  return load();
}
