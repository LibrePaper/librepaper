// TeXlyre's BusyTeX, driven.
//
// Written by us against the pipeline the release ships, the way `busytex.js`
// is. The npm package `texlyre-busytex` is a TypeScript driver over the same
// pipeline and none of it is imported: it carries no wasm and no TeX Live, it
// would put a dependency in the lockfile the browser never sees, and its
// `BusyTexRunner` presents a shape that is not the one `worker.js` wants. The
// distribution is fetched at runtime as a separate work; it is AGPL-3.0, and
// the card has to say so.
//
// This is the same lineage as `busytex.js` -- the pipeline class has the same
// name and very nearly the same constructor -- and everything the two glues
// can share, they share: `run`, which is the one script-loading shape the
// shell's CSP allows, and `engineFor`, which reads the document. What is
// different is worth the second file:
//
//   * TeX Live 2026 rather than 2023, which is the whole reason this was
//     measured: a 2020 LaTeX kernel is what `siunitx` refuses.
//   * `-synctex=1` is in every one of the pipeline's argument vectors, so a
//     `.synctex.gz` comes back from a run that produced a page. BusyTeX's
//     pipeline does not ask for one, which is why `busytex.js` returns null
//     and why step 6 could not be built on it.
//   * A standalone biber wasm beside the engine, loaded when a `.bcf`
//     appears, so `biblatex` has a backend.
//   * A remote package endpoint. The engine builds `<base>/<format>/<name>`
//     itself, in C, over synchronous XHR -- SwiftLaTeX's protocol without the
//     engine segment -- so the package half of the mirror answers it
//     unchanged, and a deployment that mirrors no data bundle at all still
//     compiles. That is the trade this distribution can be either side of,
//     and which side it is on is decided by the manifest, not here: whatever
//     bundles the mirror carries are offered, and the endpoint catches the
//     rest.
//
// One thing this glue cannot reach from outside the pipeline, said here
// rather than discovered later: it builds its own argument vectors and uses
// `--halt-on-error` on every engine, so a document with two errors reports
// the first. That is BusyTeX's behaviour too, and the same caveat applies.

import { run, engineFor } from "./busytex.js";

/// Which of the pipeline's drivers each engine is. A document that needs
/// XeTeX is compiled through dvipdfmx by the pipeline itself; LuaTeX is the
/// HarfBuzz build, which is the one this release carries a format for.
const DRIVERS = {
  pdftex: "pdftex_bibtex8",
  xetex: "xetex_bibtex8_dvipdfmx",
  luatex: "luahbtex_bibtex8",
};

export async function create({ base, distribution }) {
  const url = (name) => new URL(distribution.files[name].url, base).href;
  const extra = (name) =>
    distribution.extra?.[name] ? new URL(distribution.extra[name].url, base).href : null;

  // Order matters: the pipeline's constructor tests `typeof BusytexBiber` and
  // decides biber does not exist if that name is not yet defined, silently,
  // falling back to bibtex8 for a document that asked for biber.
  await run(url("busytex_biber.js"));
  await run(url("busytex_pipeline.js"));
  if (!self.BusytexPipeline) throw new Error("the BusyTeX pipeline did not define itself");

  // Every bundle the mirror happens to carry. The pipeline reads the document,
  // works out which packages it needs, scans each catalogued bundle's loader
  // for the `\ProvidesPackage` lines in it, and loads only the bundles that
  // carry them -- so offering a three-hundred-megabyte bundle costs nothing
  // for a document that does not need it. A mirror with no bundles at all is
  // a supported configuration and leaves the endpoint below to answer for
  // everything.
  const bundles = Object.values(distribution.bundles || {}).map(
    (pair) => new URL(pair[".js"].url, base).href,
  );

  let printed = [];
  const pipeline = new self.BusytexPipeline(
    url("busytex.js"),
    url("busytex.wasm"),
    // The catalogue: what may be loaded when a document turns out to want it.
    bundles,
    // Preloaded, before any document is seen: the base TeX Live and nothing
    // else. `busytex.js` has to preload its font bundle too, because its
    // release has no way to fetch one file and `\usepackage[T1]{fontenc}`
    // would otherwise try to run `mktexpk` and fail to fork. This release has
    // the endpoint, so a missing font is one request rather than a bundle.
    [url("texlive-basic.js")],
    [],
    (line) => printed.push(line),
    () => {},
    true,
    run,
    extra("biber.js"),
    extra("biber.wasm"),
    extra("biber.data"),
  );

  // The engine builds this URL itself and appends `<format>/<name>`, so it
  // must be the package root and nothing else. It is a URL under the mirror
  // the deployment gave us; the browser never learns TeXlyre's own endpoint,
  // which is the rule the spec cares about and the reason a package name --
  // which is a description of the document -- goes nowhere new.
  const endpoint = distribution.packages
    ? new URL(`packages/${distribution.packages}/`, base).href
    : "";

  return {
    async compile(tree) {
      printed = [];
      const files = [
        ...Object.entries(tree.texts || {}).map(([path, contents]) => ({ path, contents })),
        ...Object.entries(tree.assets || {}).map(([path, bytes]) => ({
          path,
          contents: bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes),
        })),
      ];
      const driver = DRIVERS[engineFor(tree)] || DRIVERS.pdftex;
      let result;
      try {
        result = await pipeline.compile(
          files,
          tree.main,
          // `bibtex` null asks the pipeline to decide from the source, which
          // is the right answer: it looks for a `\bibliography` or an
          // `\addbibresource`, and a BibTeX pass over a document with no
          // citations costs a pass for nothing.
          null,
          // `biber` is the one it cannot decide as well as we can. Its own
          // resolver reads the backend out of `\usepackage[backend=...]`, and
          // a document that writes `\addbibresource` without naming a backend
          // gets biblatex's default, which is biber. Saying so here rather
          // than letting it fall through to bibtex8 is the difference between
          // a bibliography and an empty one.
          biberFor(tree),
          // makeindex: the pipeline runs it when an `.idx` appears, and
          // deciding that from the source would be guessing at `\makeindex`.
          null,
          // rerun: likewise decided from the log's "Rerun to get" lines.
          null,
          "silent",
          driver,
          [],
          endpoint,
          // Shell escape is a hook for JavaScript handlers we register none
          // of, and a document that asks for `\write18` should be told no.
          false,
        );
      } catch (error) {
        // A compile the pipeline threw on still has a log, and the log is what
        // the reader needs. A throw with nothing in it becomes a spanless
        // error by way of the log parser seeing an empty log.
        return { pdf: null, synctex: null, log: printed.join("\n") + "\n" + String(error) };
      }
      return {
        pdf: result.pdf ? new Uint8Array(result.pdf) : null,
        // Gzipped bytes, exactly as the engine wrote them: this is a
        // `.synctex.gz`, not a `.synctex`, and whoever reads it has to gunzip
        // it first. Passing it through compressed rather than expanding it
        // here keeps the decision -- and the megabyte -- with the reader.
        synctex: result.synctex ? new Uint8Array(result.synctex) : null,
        log: result.log || printed.join("\n"),
      };
    },
    close() {
      // The pipeline holds an Emscripten module with a TeX Live in its heap.
      try {
        pipeline.terminate?.();
      } catch {
        /* a pipeline that never initialised has nothing to release */
      }
    },
  };
}

/// Whether to ask for biber rather than bibtex8. `\addbibresource` is
/// biblatex's own way of naming a `.bib` and is the honest signal; an explicit
/// `backend=bibtex` overrides it, because a document that says so means it.
function biberFor(tree) {
  const all = Object.values(tree.texts || {}).join("\n");
  if (!/\\addbibresource/.test(all)) return null;
  if (/\\usepackage\[[^\]]*backend\s*=\s*bibtex8?[^\]]*\]\{biblatex\}/.test(all)) return false;
  return true;
}
