// The three distributions Komodoc knows how to drive, as data.
//
// A distribution is a set of static files a browser fetches and runs: one or
// more WebAssembly modules, the JavaScript Emscripten generated to host them,
// and the TeX Live files the engine reads. This file is the only place the
// upstream releases are named, so mirroring a new release is an edit here and
// a re-run of `mirror.mjs`, and the browser never learns any of these URLs:
// it reads `manifest.json` from whatever base URL the deployment gave it.
//
// The sizes are deliberately not written here. They are measured, and
// `examples/latex/MEASUREMENTS.md` and the card carry the numbers.

/// The SwiftLaTeX release the mirror is pinned to. One zip holds every engine.
export const SWIFTLATEX_RELEASE = {
  tag: "v20022022",
  url: "https://github.com/SwiftLaTeX/SwiftLaTeX/releases/download/v20022022/20-02-2022.zip",
};

/// The BusyTeX WebAssembly build the mirror is pinned to. Every file is its
/// own release asset, so nothing is unpacked.
export const BUSYTEX_RELEASE = {
  tag: "build_wasm_4499aa69fd3cf77ad86a47287d9a5193cf5ad993_7936974349_1",
  base:
    "https://github.com/busytex/busytex/releases/download/" +
    "build_wasm_4499aa69fd3cf77ad86a47287d9a5193cf5ad993_7936974349_1",
};

export const DISTRIBUTIONS = [
  {
    name: "swiftlatex-pdftex",
    label: "SwiftLaTeX pdfTeX",
    engines: ["pdfTeX"],
    bibliography: "BibTeX, inside the engine",
    licence: "AGPL-3.0",
    // The words the card says, from the spec: which is small-and-chatty and
    // which is large-and-quiet.
    trade: "small to start and chatty afterwards: a compile that meets a new package stops, fetches it, and resumes.",
    // Fetched before a first compile can begin.
    upfront: [
      { from: "zip", entry: "swiftlatexpdftex.js", as: "swiftlatexpdftex.js" },
      { from: "zip", entry: "swiftlatexpdftex.wasm", as: "swiftlatexpdftex.wasm" },
    ],
    // Fetched one file at a time, by name, while a compile runs.
    packages: "pdftex",
  },
  {
    name: "swiftlatex-xetex",
    label: "SwiftLaTeX XeTeX",
    engines: ["XeTeX", "dvipdfmx"],
    bibliography: "BibTeX, inside the engine",
    licence: "AGPL-3.0",
    trade: "small to start and chatty afterwards; system fonts are not available, only fetched ones.",
    upfront: [
      { from: "zip", entry: "swiftlatexxetex.js", as: "swiftlatexxetex.js" },
      { from: "zip", entry: "swiftlatexxetex.wasm", as: "swiftlatexxetex.wasm" },
      { from: "zip", entry: "swiftlatexdvipdfm.js", as: "swiftlatexdvipdfm.js" },
      { from: "zip", entry: "swiftlatexdvipdfm.wasm", as: "swiftlatexdvipdfm.wasm" },
    ],
    packages: "xetex",
  },
  {
    name: "busytex",
    label: "BusyTeX, TeX Live 2023",
    engines: ["pdfTeX", "XeTeX", "LuaTeX", "BibTeX", "dvipdfmx"],
    bibliography: "BibTeX",
    licence: "MIT",
    trade: "large to start and quiet afterwards: the whole distribution arrives before the first compile and nothing is fetched during one.",
    upfront: [
      { from: "busytex", entry: "busytex.js" },
      { from: "busytex", entry: "busytex.wasm" },
      { from: "busytex", entry: "busytex_pipeline.js" },
      { from: "busytex", entry: "texlive-basic.js" },
      { from: "busytex", entry: "texlive-basic.data" },
    ],
    // Bundles a document can ask for, fetched whole when a package inside one
    // is missing. Each is a pair: the Emscripten loader and its data blob.
    bundles: [
      "ubuntu-texlive-latex-recommended",
      "ubuntu-texlive-latex-extra",
      "ubuntu-texlive-fonts-recommended",
      "ubuntu-texlive-science",
    ],
    packages: null,
  },
];

export function distribution(name) {
  const found = DISTRIBUTIONS.find((one) => one.name === name);
  if (!found) throw new Error(`no distribution named ${name}`);
  return found;
}
