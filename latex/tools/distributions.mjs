// The four distributions Komodoc knows how to drive, as data.
//
// A distribution is a set of static files a browser fetches and runs: one or
// more WebAssembly modules, the JavaScript Emscripten generated to host them,
// and the TeX Live files the engine reads. This file is the only place the
// upstream releases are named, so mirroring a new release is an edit here and
// a re-run of `mirror.mjs`, and the browser never learns any of these URLs:
// it reads `manifest.json` from whatever base URL the deployment gave it.
//
// The sizes are deliberately not written here. They are measured, and
// `latex/corpus/MEASUREMENTS.md` and the card carry the numbers.

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

/// TeXlyre's own BusyTeX build, pinned by release tag. Unlike BusyTeX's
/// release, which is one asset per file, this one is a single tar.gz holding
/// every file under a `busytex/` directory, so `mirror.mjs` unpacks it once
/// into its cache and takes what it names out of there. The npm package of the
/// same name carries only a TypeScript driver -- no wasm, no data -- and
/// nothing of it is used here: the glue is written against the pipeline the
/// tarball ships, exactly as `busytex.js` is written against BusyTeX's.
export const TEXLYRE_RELEASE = {
  tag: "assets-v1.4.0",
  url:
    "https://github.com/TeXlyre/texlyre-busytex/releases/download/" +
    "assets-v1.4.0/busytex-assets.tar.gz",
  /// Everything in the archive lives under this one directory.
  prefix: "busytex/",
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
    // Measured, not estimated: `latex/corpus/MEASUREMENTS.md`. `upfront` in
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
    // is a measurement rather than an opinion: `latex/corpus/MEASUREMENTS.md`
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
    // Measured, not estimated: `latex/corpus/MEASUREMENTS.md`. `upfront` in
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
    // Measured, not estimated: `latex/corpus/MEASUREMENTS.md`. `upfront` in
    // the manifest is what `choose` fetches; these two are what a compile
    // fetches afterwards, which is most of the weight and is invisible until
    // the card says it. `first` is a first document on a cold cache,
    // `next` a second one. Bytes, from that file's tables.
    measured: { first: 182730000, next: 0 },
  },
  {
    name: "texlyre-busytex",
    label: "TeXlyre BusyTeX, TeX Live 2026",
    engines: ["pdfTeX", "XeTeX", "LuaHBTeX", "BibTeX", "biber", "makeindex", "dvipdfmx"],
    bibliography: "BibTeX, biber",
    // BusyTeX is MIT; this build is a derivative TeXlyre licenses AGPL-3.0,
    // which is the licence the card has to name for it.
    licence: "AGPL-3.0",
    // Measured but not offered. Nothing reads this entry except the harness
    // until somebody decides, with `latex/corpus/MEASUREMENTS.md` in hand,
    // that it belongs beside or instead of the one that is shown -- and that
    // decision is this flag and a re-run of `mirror.mjs`, nothing else.
    shown: false,
    trade:
      "the largest to start and the quietest afterwards: a whole TeX Live 2026 before the first page, and then nothing -- or, pointed at the package endpoint instead, the smallest of the four to start and the chattiest.",
    upfront: [
      { from: "texlyre", entry: "busytex.js" },
      { from: "texlyre", entry: "busytex.wasm" },
      { from: "texlyre", entry: "busytex_pipeline.js" },
      // Four kilobytes, and the pipeline's constructor decides whether biber
      // exists at all by whether this script has defined `BusytexBiber` by
      // then -- so it is up front even though biber itself is not.
      { from: "texlyre", entry: "busytex_biber.js" },
      { from: "texlyre", entry: "texlive-basic.js" },
      { from: "texlyre", entry: "texlive-basic.data" },
    ],
    // The two further TeX Live data packages, and biber. Biber is a data
    // package here only because the mirror has one shape for a pair of files
    // and biber is a pair of files plus a `.data`; the pipeline loads it when
    // a `.bcf` appears, not when a document names a package.
    bundles: ["texlive-recommended", "texlive-extra"],
    // `biber.js`/`biber.wasm`/`biber.data` are a triple rather than the pair
    // `bundles` describes, so they are named here and placed beside the rest.
    extra: ["biber.js", "biber.wasm", "biber.data"],
    // It can do both: whole bundles, or one file at a time from an endpoint
    // whose URL shape -- `<base>/<kpathsea format>/<name>` -- is SwiftLaTeX's
    // protocol minus the engine segment, so the package half of the mirror
    // already answers it. Which of the two a deployment wants is the whole
    // question this distribution was measured to settle.
    packages: "pdftex",
  },
];

export function distribution(name) {
  const found = DISTRIBUTIONS.find((one) => one.name === name);
  if (!found) throw new Error(`no distribution named ${name}`);
  return found;
}
