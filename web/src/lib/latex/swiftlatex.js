// SwiftLaTeX, driven.
//
// Written by us against the engine's exported API. Nothing of TeXlyre's
// editor is imported; its worker code is the best available reading of how
// this engine wants to be driven and was read for that. SwiftLaTeX is
// AGPL-3.0 and is fetched at runtime as a separate work rather than linked
// into Komodoc's build.
//
// The engine is itself a worker script -- it installs its own `onmessage` and
// answers a handful of commands -- so it is loaded as a nested worker rather
// than with `importScripts`, which would take our own `onmessage` with it.
// It is created from a URL under the mirror, not from a blob, because
// Emscripten finds its `.wasm` beside its `.js`: that is why the mirror puts
// a whole release in one digested directory instead of a digest in each
// filename.
//
//   settexliveurl   where to fetch a TeX Live file from
//   setmainfile     the file to run on
//   mkdir/writefile the in-memory filesystem, one call at a time
//   compilelatex    one pass; answers with the PDF and the log
//
// Two things about this engine that the card and the measurements have to be
// honest about, because neither is obvious and both were found by driving it.
//
// It carries no TeX Live at all. Every `.sty`, `.tfm`, `.cls` and font -- and
// the LaTeX format itself, ten megabytes of it, before a single line is
// typeset -- is fetched one file at a time, over *synchronous* XHR, from
// inside the compile. That is why "small to start and chatty afterwards" is
// the trade, and why those fetches cannot go through Cache Storage: its API
// is asynchronous and the engine's call site is a C function that cannot
// await. The engine keeps what it fetched in its own filesystem for as long
// as the worker lives, so the chatter is a first-compile cost per session,
// not a per-compile one.
//
// And it has no SyncTeX. There is no synctex symbol in the module, so
// `synctex` is always null here and `05-SPEC-latex.md`'s last step cannot be
// built on this distribution.

const READY = "ok";

export async function create({ name, base, distribution }) {
  const xetex = name === "swiftlatex-xetex";
  const packages = new URL("packages/", base).href;

  const tex = await start(
    distribution.files[xetex ? "swiftlatexxetex.js" : "swiftlatexpdftex.js"],
    base,
    packages,
  );
  // XeTeX writes a DVI, not a PDF: the second half of that engine is a
  // separate Emscripten program with its own filesystem, so the distribution
  // is two workers and the `.xdv` is carried from one to the other by hand.
  const dvi = xetex ? await start(distribution.files["swiftlatexdvipdfm.js"], base, packages) : null;

  return {
    async compile(tree) {
      // A fresh working directory for every compile, so a file removed from
      // the tree is removed from the compile: a stale `.aux` or a chapter the
      // author deleted must not keep a document compiling.
      tex.reset();
      for (const [path, text] of Object.entries(tree.texts || {})) await tex.put(path, text);
      for (const [path, bytes] of Object.entries(tree.assets || {})) {
        await tex.put(path, bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes));
      }
      tex.send({ cmd: "setmainfile", url: tree.main });

      // The engine runs BibTeX itself at the end of a successful pass but
      // does not run LaTeX again afterwards, so the passes are driven here:
      // one to write the `.aux`, one to read the bibliography back, one to
      // settle the references. Three is what a paper with citations and
      // cross-references needs and is the most any document here needs.
      let last = null;
      for (let pass = 0; pass < 3; pass++) {
        last = await tex.ask({ cmd: "compilelatex" }, true);
        // A pass that produced nothing will not produce anything on the next
        // one either: the error is real and running twice more only makes
        // the reader wait for the same message.
        if (last.result !== READY) break;
        if (pass > 0 && !needsAnotherPass(last.log || "")) break;
      }

      let pdf = last?.pdf ? new Uint8Array(last.pdf) : null;
      let log = last?.log || "";
      if (pdf && dvi) {
        // What XeTeX produced is a `.xdv`; dvipdfmx turns it into the page.
        const stem = tree.main.replace(/\.tex$/, "");
        dvi.reset();
        await dvi.put(`${stem}.xdv`, pdf);
        dvi.send({ cmd: "setmainfile", url: `${stem}.xdv` });
        const made = await dvi.ask({ cmd: "compilepdf" }, true);
        log += "\n" + (made.log || "");
        pdf = made.result === READY && made.pdf ? new Uint8Array(made.pdf) : null;
      }

      return {
        pdf,
        // No synctex symbol in either module: see the note at the top.
        synctex: null,
        log,
      };
    },
    close() {
      tex.close();
      dvi?.close();
    },
  };
}

/// One Emscripten engine, started and pointed at the mirror.
async function start(loader, base, packages) {
  if (!loader) throw new Error("the mirror has no loader for this engine");
  const worker = new Worker(new URL(loader.url, base).href);
  const inbox = mailbox(worker);
  // The engine says nothing until Emscripten's postRun, and says `ok` then.
  const hello = await inbox.next();
  if (hello.result !== READY) throw new Error("a SwiftLaTeX engine did not start");
  // Every TeX Live file comes from the mirror and from nowhere else. The
  // engine appends `<engine>/<format>/<name>` to this itself. Four of the
  // engine's commands answer nothing at all, which is why `send` exists
  // beside `ask`: awaiting a reply that never comes would hang the compile.
  inbox.send({ cmd: "settexliveurl", url: packages });

  const made = new Set();
  return {
    ...inbox,
    reset() {
      inbox.send({ cmd: "flushcache" });
      made.clear();
    },
    async put(path, contents) {
      // The engine has no `mkdir -p`, so each directory of the path is made
      // in turn and an existing one is not an error worth stopping for.
      const parts = path.split("/");
      for (let i = 1; i < parts.length; i++) {
        const dir = parts.slice(0, i).join("/");
        if (made.has(dir)) continue;
        made.add(dir);
        await inbox.ask({ cmd: "mkdir", url: dir }, true);
      }
      await inbox.ask({ cmd: "writefile", url: path, src: contents });
    },
    close() {
      worker.terminate();
    },
  };
}

// `log.js` owns the reading of a log, but "run me again" is not a diagnostic
// and does not belong in a diagnostic list, so the question is asked here in
// the two forms TeX puts it in.
function needsAnotherPass(log) {
  return /Rerun to get|Please rerun|Label\(s\) may have changed|Citation .* undefined/.test(log);
}

/// The engine answers with a message per command and does not tag them, so
/// the replies are taken in order. A queue is enough because there is exactly
/// one caller and it awaits every command.
function mailbox(worker) {
  const waiting = [];
  const arrived = [];
  worker.onmessage = (event) => {
    const waiter = waiting.shift();
    if (waiter) waiter(event.data);
    else arrived.push(event.data);
  };
  const next = () =>
    arrived.length ? Promise.resolve(arrived.shift()) : new Promise((resolve) => waiting.push(resolve));
  return {
    next,
    /// A command the engine does not answer: `settexliveurl`, `setmainfile`,
    /// `flushcache`, `grace`. Awaiting one would hang the compile.
    send(message) {
      worker.postMessage(message);
    },
    /// A command the engine does answer. `tolerate` lets a refusal come back
    /// as a result rather than as a throw, because a document that does not
    /// compile is an ordinary state of an editor and not a failure of this
    /// module's.
    async ask(message, tolerate = false) {
      worker.postMessage(message);
      const reply = await next();
      if (!tolerate && reply.result !== READY) {
        throw new Error(`the engine refused ${message.cmd}`);
      }
      return reply;
    },
  };
}
