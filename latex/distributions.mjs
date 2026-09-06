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
    // On the card. See the comment above `shown` on the next entry for what
    // the flag decides and who decides it.
    shown: true,
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
    // Measured, not estimated: `examples/latex/MEASUREMENTS.md`. `upfront` in
    // the manifest is what `choose` fetches; these two are what a compile
    // fetches afterwards, which is most of the weight and is invisible until
    // the card says it. `first` is a first document on a cold cache,
    // `next` a second one. Bytes, from that file's tables.
    measured: { first: 17380000, next: 260000 },
  },
  {
    name: "swiftlatex-xetex",
    // Not on the card, and driven all the same.
    //
    // `shown` is the one place a distribution is offered to a person, and it
    // is a measurement rather than an opinion: `examples/latex/MEASUREMENTS.md`
    // records what each one could and could not compile. This engine's
    // dvipdfmx has no font to embed, and BusyTeX below cannot compile
    // `\\usepackage[T1]{fontenc}` -- no Type 1 EC fonts in any of its bundles,
    // and `mktexpk` cannot fork inside a worker. Both stay in the worker and
    // in the mirror, because the flag travels in the manifest: a self-hoster
    // who fixes a bundle turns one on by flipping this and rebuilding the
    // mirror, with no build of Komodoc involved. The card lists what the
    // manifest marks shown and names none of them itself.
    shown: false,
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
    // Measured, not estimated: `examples/latex/MEASUREMENTS.md`. `upfront` in
    // the manifest is what `choose` fetches; these two are what a compile
    // fetches afterwards, which is most of the weight and is invisible until
    // the card says it. `first` is a first document on a cold cache,
    // `next` a second one. Bytes, from that file's tables.
    measured: { first: 29540000, next: 100000 },
  },
  {
    name: "busytex",
    shown: false,
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
    // Measured, not estimated: `examples/latex/MEASUREMENTS.md`. `upfront` in
    // the manifest is what `choose` fetches; these two are what a compile
    // fetches afterwards, which is most of the weight and is invisible until
    // the card says it. `first` is a first document on a cold cache,
    // `next` a second one. Bytes, from that file's tables.
    measured: { first: 182730000, next: 0 },
  },
];

export function distribution(name) {
  const found = DISTRIBUTIONS.find((one) => one.name === name);
  if (!found) throw new Error(`no distribution named ${name}`);
  return found;
}
