// BusyTeX, driven.
//
// Written by us against the pipeline's exported API. BusyTeX is MIT and is
// fetched at runtime as a separate work; nothing of it is linked into
// Komodoc's build.
//
// The shape of this distribution is the opposite of SwiftLaTeX's. One module
// holds pdfTeX, XeTeX, LuaTeX, BibTeX and dvipdfmx, and a TeX Live comes with
// it as an Emscripten data package -- a hundred and thirty-seven megabytes
// before the first compile, and then nothing during one. That is "large to
// start and quiet afterwards", and it is why the card must carry both numbers
// and not one.
//
// The pipeline runs the passes itself: it looks at the source for a
// bibliography, and if it finds one it runs LaTeX, BibTeX, LaTeX, LaTeX. So
// unlike SwiftLaTeX there is no pass loop here; there is a tree flattened
// into the list of `{ path, contents }` it wants, and a driver name.
//
// Two things this glue cannot reach from outside the pipeline, said here
// rather than discovered later. The pipeline builds its own argument vectors
// and does not pass `-synctex=1`, so no `.synctex` is written even though the
// module has SyncTeX compiled in; getting one means a patched pipeline, which
// is a step of its own and not this one. And it uses `--halt-on-error`, so a
// document with two errors reports the first, where SwiftLaTeX reports both.

import { cached } from "../cache.js";

/// Which of the pipeline's drivers each engine is. A document that needs
/// XeTeX is compiled through dvipdfmx by the pipeline itself.
const DRIVERS = {
  pdftex: "pdftex_bibtex8",
  xetex: "xetex_bibtex8_dvipdfmx",
  luatex: "luahbtex_bibtex8",
};

/// Runs a classic script in this worker's global scope.
///
/// The pipeline and its data packages are scripts from before modules: they
/// define globals and expect `importScripts`, which a module worker does not
/// have. Fetching the text and evaluating it through `Function` puts it in
/// the global scope, which is the scope those scripts were written for, and
/// costs one copy of the text -- of which the largest is two megabytes, the
/// hundred-megabyte payload being a `.data` file the script fetches itself.
///
/// The bytes come from the mirror by way of Cache Storage, so a distribution
/// is fetched once per browser and not once per session.
/// The names these scripts mean to leave in the global scope: the pipeline's
/// own class, and the factory Emscripten names after the module. A blob
/// module has a scope of its own, so each is copied out by an epilogue,
/// guarded because most scripts define neither.
/// `BusytexBiber` is named here for `texlyre.js`, whose release ships such a
/// script and whose pipeline decides biber exists at all by whether that name
/// is defined when its constructor runs. BusyTeX's own release has no such
/// script, and the epilogue guards every name it copies, so naming it here
/// costs this glue nothing.
// Candidate-only fix: biber.js defines its Emscripten factory as a module
// local `biber`; expose it on self so BusytexBiber can construct it later.
const GLOBALS = ["BusytexPipeline", "BusytexBiber", "busytex", "biber"];

/// Exported because `texlyre.js` drives a pipeline of the same lineage and
/// needs exactly this: the one script-loading shape the shell's CSP allows.
/// Two copies of it would be two things to get wrong.
export async function run(url) {
  const response = await cached(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const text = await response.text();
  // A blob module rather than `eval` or `new Function`: the shell's CSP
  // allows `blob:` in `script-src` and allows neither of the others, so this
  // is the one form that will still work when the worker runs under it. A
  // module has its own scope, so the few names the pipeline means to be
  // global are put there by an epilogue rather than by hoping.
  const epilogue = GLOBALS.map(
    (name) => `try { self[${JSON.stringify(name)}] = ${name}; } catch (e) {}`,
  ).join("\n");
  const blob = new Blob([text, "\n", epilogue, `\n//# sourceURL=${url}\n`], {
    type: "text/javascript",
  });
  const href = URL.createObjectURL(blob);
  try {
    await import(/* @vite-ignore */ href);
  } finally {
    URL.revokeObjectURL(href);
  }
}

export async function create({ base, distribution }) {
  const url = (name) => new URL(distribution.files[name].url, base).href;
  // Emscripten's loaders find their payloads by name beside themselves --
  // `busytex.js` wants `busytex.wasm` and `texlive-basic.js` wants
  // `texlive-basic.data` -- which is why the mirror keeps a whole release in
  // one digested directory rather than a digest in each filename.
  await run(url("busytex_pipeline.js"));
  if (!self.BusytexPipeline) throw new Error("the BusyTeX pipeline did not define itself");

  const bundles = Object.entries(distribution.bundles || {}).map(
    ([name, pair]) => new URL(pair[".js"].url, base).href,
  );

  let printed = [];
  const pipeline = new self.BusytexPipeline(
    url("busytex.js"),
    url("busytex.wasm"),
    // Every bundle the mirror has is offered; the pipeline reads the
    // document, works out which packages it needs, and loads only the
    // bundles that carry them.
    [url("texlive-basic.js"), ...bundles],
    // Preloaded, before any document is seen: the base TeX Live, and the
    // recommended fonts with it. The fonts are not optional in practice. A
    // document with `\usepackage[T1]{fontenc}` -- which is most of them --
    // wants the EC fonts, `texlive-basic` does not carry them, and the
    // resolver cannot know that from the source because a font is not a
    // package name. Without them the engine tries to run `mktexpk`, which in
    // a worker means `fork(): Function not implemented` and no page at all.
    // Ten megabytes up front is the price of not failing on `fontenc`.
    [url("texlive-basic.js"), ...bundles],
    [],
    (line) => printed.push(line),
    () => {},
    true,
    // The pipeline's own loader is `importScripts`; ours is the same thing
    // spelled for a module worker.
    run,
  );

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
        // `bibtex` null asks the pipeline to decide from the source, which is
        // the right answer: it looks for a `\bibliography` or a
        // `\addbibresource`, and running BibTeX on a document with no
        // citations costs a pass for nothing.
        result = await pipeline.compile(files, tree.main, null, "silent", driver, []);
      } catch (error) {
        // A compile the pipeline threw on still has a log, and the log is
        // what the reader needs. A throw with nothing in it becomes a
        // spanless error by way of the log parser seeing an empty log.
        return { pdf: null, synctex: null, log: printed.join("\n") + "\n" + String(error) };
      }
      return {
        pdf: result.pdf ? new Uint8Array(result.pdf) : null,
        // The module has SyncTeX; the pipeline does not ask for it. See above.
        synctex: null,
        log: result.log || printed.join("\n"),
      };
    },
    close() {},
  };
}

/// Which engine a document wants, from the document. `fontspec` is the honest
/// signal -- it is the package that refuses to load under pdfTeX -- and
/// `luacode` the same for LuaTeX. Everything else is pdfTeX, which is what
/// almost every paper is.
export function engineFor(tree) {
  const main = (tree.texts || {})[tree.main] || "";
  const all = Object.values(tree.texts || {}).join("\n");
  const wants = (pattern) => pattern.test(main) || pattern.test(all);
  if (wants(/\\usepackage(\[[^\]]*\])?\{[^}]*\b(fontspec|unicode-math|polyglossia)\b/)) return "xetex";
  if (wants(/\\usepackage(\[[^\]]*\])?\{[^}]*\bluacode\b/)) return "luatex";
  return "pdftex";
}
