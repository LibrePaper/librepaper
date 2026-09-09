// Drives the real `latex.js` -- the actual queue, job identity, bibliography
// caching and routing logic -- against a scripted worker and scripted
// local/VM backends, so this check exercises the same code the browser runs
// with none of a browser, a real WasmTex build, a real local app or a real
// VM anywhere in reach. `latex.js`'s `_testing.inject` hook is what makes
// that possible; see its doc comment in `src/lib/latex.js`.
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";

const enc = new TextEncoder();
const PDF = new Uint8Array([0x25, 0x50, 0x44, 0x46]); // "%PDF"

// --- A scripted worker, speaking the section 2.4 protocol ------------------
//
// Every scenario below sets `worker().texReplies`/`.bibtexReplies` to an
// explicit, ordered script of what the next `tex`/`bibtex` command should
// answer (the last entry repeats once the script is exhausted, which is
// convenient for "keep succeeding cleanly" tails). This is deliberately not
// a simulation of a real TeX engine's aux/bcf bookkeeping -- `bibliography.js`
// already has its own fixture-based check for that -- it is just enough of
// the protocol for `latex.js`'s own orchestration to be exercised honestly.
class FakeWorker {
  constructor() {
    this.messages = [];
    this.onmessage = null;
    this.onerror = null;
    this.dead = false;
    this.staged = [];
    // A scenario that needs its script in place before the worker's first
    // message (the queue-semantics gate, below) sets `FakeWorker.nextTex...`
    // ahead of calling `compile()`; everyone else just mutates `worker()
    // .texReplies` once the (already-created, reused) worker is in hand.
    this.texReplies = FakeWorker.nextTexReplies || [{ status: 0, pdf: PDF, synctex: null, log: "", outputs: {} }];
    this.bibtexReplies = FakeWorker.nextBibtexReplies || [{ status: 0, bbl: enc.encode("BBL"), blg: "" }];
    FakeWorker.nextTexReplies = null;
    FakeWorker.nextBibtexReplies = null;
    this.bibtexCalls = [];
    if (FakeWorker.onCreate) FakeWorker.onCreate(this);
  }
  postMessage(message) {
    if (this.dead) throw new Error("posted to a terminated worker");
    this.messages.push(message);
    Promise.resolve().then(async () => {
      if (this.dead) return;
      try {
        if (message.cmd === "configure") return this.reply(message.id, { engines: [] });
        if (message.cmd === "stage") {
          this.staged.push(message);
          return this.reply(message.id, {});
        }
        if (message.cmd === "write") return this.reply(message.id, {});
        if (message.cmd === "read") return this.reply(message.id, { bytes: null });
        if (message.cmd === "makeindex") return this.reply(message.id, { ind: null, ilg: "" });
        if (message.cmd === "tex") {
          const script = this.texReplies;
          const next = script.length > 1 ? script.shift() : script[0];
          const built = typeof next === "function" ? await next(message) : next;
          return this.reply(message.id, built);
        }
        if (message.cmd === "bibtex") {
          this.bibtexCalls.push(message);
          FakeWorker.totalBibtexCalls += 1; // survives a settings change recreating the worker instance
          const script = this.bibtexReplies;
          const next = script.length > 1 ? script.shift() : script[0];
          const built = typeof next === "function" ? await next(message) : next;
          return this.reply(message.id, built);
        }
        return this.reply(message.id, {});
      } catch (error) {
        this.onmessage?.({ data: { id: message.id, failed: String(error?.message || error) } });
      }
    });
  }
  reply(id, extra) {
    this.onmessage?.({ data: { id, ok: true, ...extra } });
  }
  terminate() {
    this.dead = true;
  }
  crash(message) {
    this.onerror?.({ message });
  }
}

FakeWorker.totalBibtexCalls = 0;

let liveWorker = null;
FakeWorker.onCreate = (instance) => {
  liveWorker = instance;
};
const worker = () => liveWorker;

const MANIFEST = {
  default_release: "r1",
  releases: { r1: { id: "r1", snapshot: "snap1", base: "wasmtex/r1/", texlive_base: "texlive/snap1/", engines: {} } },
  texlive: { snap1: {} },
};
const fakeFetch = async () => new Response(JSON.stringify(MANIFEST), { status: 200, headers: { "content-type": "application/json" } });

const latex = await import("../src/lib/latex.js?latex-controller-check");

function tick(times = 1) {
  let p = Promise.resolve();
  for (let i = 0; i < times; i++) p = p.then(() => new Promise((resolve) => setImmediate(resolve)));
  return p;
}

/// Polls `done` once per turn of the event loop until it holds or `ms` have
/// passed. A turn is microseconds, so a budget counted in turns is a budget
/// of a few milliseconds: enough on a warm laptop, not on a two-core runner
/// where `crypto.subtle.digest` waits its turn on the threadpool. Time is
/// the bound, and the loop keeps spinning so I/O completions are seen.
async function until(done, ms = 10_000) {
  const deadline = Date.now() + ms;
  while (!done() && Date.now() < deadline) await tick();
  return done();
}

/// Waits until the worker created for the current compile actually exists.
/// The path from `compile()` to the first `new Worker(...)` crosses several
/// real awaits (the manifest fetch, `crypto.subtle.digest` for the job
/// identity), so a fixed tick count is a race; polling is not.
async function untilWorker(previous) {
  await until(() => worker() !== previous);
  assert.notEqual(worker(), previous, "a worker was created");
  return worker();
}

async function untilMessage(target, pred) {
  if (await until(() => target.messages.some(pred))) return;
  throw new Error("expected message was not observed in time");
}

function tree(main, text, assets = {}) {
  return { main, texts: { [main]: text }, assets };
}

// Explicit "checked and unavailable" overrides -- see `_testing.inject`'s
// doc comment in `src/lib/latex.js` for why `null` here differs from leaving
// the field out.
const noLocal = null;
const noVm = null;

let projectCounter = 0;
function nextProject() {
  projectCounter += 1;
  return `project-${projectCounter}`;
}

// ============================================================================
// 1 & 2. Queue semantics and job identity.
// ============================================================================
{
  const project = nextProject();
  let gateResolve;
  const gate = new Promise((resolve) => {
    gateResolve = resolve;
  });
  let pass = 0;

  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  const treeA = tree("main.tex", "A");
  const treeB = tree("main.tex", "B");
  const treeC = tree("main.tex", "C");

  FakeWorker.nextTexReplies = [
    async (message) => {
      pass += 1;
      const staged = worker().staged.at(-1).tree.texts[message.main];
      if (pass === 1) await gate;
      return { status: 0, pdf: PDF, synctex: null, log: `ok:${staged}`, outputs: {} };
    },
  ];

  const first = latex.compile(treeA);
  const w = await untilWorker(null);
  await untilMessage(w, (m) => m.cmd === "tex");

  const second = latex.compile(treeB);
  const third = latex.compile(treeC); // replaces `second` in the queue before it ever runs

  const stagesBeforeRelease = worker().staged.length;
  gateResolve();
  const [resultFirst, resultSecond, resultThird] = await Promise.all([first, second, third]);

  assert.equal(stagesBeforeRelease, 1, "the queued job did not stage until the running one finished");
  assert.equal(resultSecond, resultThird, "a replaced request receives the replacing result");
  assert.match(resultThird.log, /ok:C$/, "the queue always ran the newest tree, not the replaced one");
  assert.equal(worker().staged.length, 2, "exactly two jobs ran: A, then the winner of the queue (C)");
  assert.ok(!worker().staged.some((m) => m.tree.texts["main.tex"] === "B"), "B was discarded, never staged");

  // Job identity: id, snapshot and every echoed field.
  assert.equal(resultFirst.job.id, `${project}:${resultFirst.job.generation}`);
  assert.equal(resultFirst.job.project, project);
  assert.equal(resultFirst.job.engine, "pdflatex");
  assert.equal(resultFirst.job.release, "r1");
  assert.equal(resultFirst.job.main, "main.tex");
  assert.match(resultFirst.job.snapshot, /^[0-9a-f]{64}$/, "snapshot is a sha256 hex digest");
  assert.ok(resultThird.job.generation > resultFirst.job.generation, "generation increases per job that actually runs");
}

// ============================================================================
// 3, 4, 5. BibTeX: first run + reruns until settled; prose reuse; citation
// invalidation -- all within one project, so the bibliography cache and the
// staged-generated-state reuse actually apply across compiles.
// ============================================================================
{
  const project = nextProject();
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  const auxWithA = () => enc.encode("\\bibdata{refs}\n\\citation{a}\n");
  const auxWithAB = () => enc.encode("\\bibdata{refs}\n\\citation{a}\n\\citation{b}\n");

  // --- 3. First BibTeX run, then two reruns until `rerun()` is false.
  worker().texReplies = [
    { status: 0, pdf: PDF, synctex: null, log: "LaTeX Warning: Citation `a' on page 1 undefined on input line 1.", outputs: { "main.aux": auxWithA() } },
    { status: 0, pdf: PDF, synctex: null, log: "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.", outputs: { "main.aux": auxWithA() } },
    { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": auxWithA() } },
  ];
  worker().bibtexReplies = [{ status: 0, bbl: enc.encode("BBL-a"), blg: "" }];

  const bibtexBefore0 = worker().bibtexCalls.length;
  const texBefore0 = worker().messages.filter((m) => m.cmd === "tex").length;
  const t1 = tree("main.tex", "citing a", { "refs.bib": enc.encode("@book{a,}") });
  const r1 = await latex.compile(t1);
  assert.equal(r1.ok, true);
  assert.equal(worker().bibtexCalls.length - bibtexBefore0, 1, "exactly one BibTeX run");
  assert.equal(worker().messages.filter((m) => m.cmd === "tex").length - texBefore0, 3, "the first pass plus two reruns");
  assert.equal(r1.provenance.bibliography, "bibtex");

  // --- 4. A prose-only edit: same citations, so the aux content -- and
  // therefore the bibliography identity -- is unchanged. The worker reports
  // the bbl it already has (staged from the previous job) and no undefined
  // citation, so no BibTeX command is sent at all.
  const bibtexBefore = worker().bibtexCalls.length;
  worker().texReplies = [
    { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": auxWithA(), "main.bbl": enc.encode("BBL-a") } },
  ];
  const t2 = tree("main.tex", "citing a, now with a longer sentence around it", { "refs.bib": enc.encode("@book{a,}") });
  const r2 = await latex.compile(t2);
  assert.equal(r2.ok, true);
  assert.equal(worker().bibtexCalls.length, bibtexBefore, "a prose edit reused the cached bibliography; no BibTeX run");

  // --- 5. A citation edit: the aux gains `\citation{b}`, which changes the
  // bibliography identity and must trigger a fresh BibTeX run.
  worker().texReplies = [
    { status: 0, pdf: PDF, synctex: null, log: "LaTeX Warning: Citation `b' on page 1 undefined on input line 1.", outputs: { "main.aux": auxWithAB() } },
    { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": auxWithAB() } },
  ];
  worker().bibtexReplies = [{ status: 0, bbl: enc.encode("BBL-ab"), blg: "" }];
  const t3 = tree("main.tex", "citing a and b", { "refs.bib": enc.encode("@book{a,}@book{b,}") });
  const r3 = await latex.compile(t3);
  assert.equal(r3.ok, true);
  assert.equal(worker().bibtexCalls.length, bibtexBefore + 1, "a citation edit reran BibTeX");
}

// ============================================================================
// 6. Biber -> local (reachable) -> browser continuation.
// ============================================================================
{
  const project = nextProject();
  const bcf = () => enc.encode('<bcf:controlfile><bcf:datasource type="file">refs.bib</bcf:datasource></bcf:controlfile>');
  const localReachable = {
    async runBiber(request) {
      assert.equal(request.job.project, project);
      assert.ok(request.bcf, "the bcf bytes were forwarded to local Biber");
      return { ok: true, bbl: enc.encode("BBL-local"), blg: "biber ran locally", exit: 0, tool: { name: "biber", version: "2.21", backend: "local" } };
    },
    async capabilities() {
      return { tools: {} };
    },
  };
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: localReachable, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  worker().texReplies = [
    { status: 0, pdf: PDF, synctex: null, log: "Package biblatex Warning: Please (re)run Biber on the file: main\n", outputs: { "main.aux": enc.encode("\\relax\n"), "main.bcf": bcf() } },
    { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": enc.encode("\\relax\n") } },
  ];

  const result = await latex.compile(tree("main.tex", "\\cite{a}", { "refs.bib": enc.encode("@book{a,}") }));
  assert.equal(result.ok, true);
  assert.equal(result.provenance.bibliography, "local-biber");
  assert.equal(result.provenance.backend, "browser");
  assert.ok(result.attempts.some((a) => a.stage === "local-biber" && a.backend === "local" && a.ok));
}

// ============================================================================
// 7. Biber -> local unreachable -> VM -> browser continuation.
// ============================================================================
{
  const project = nextProject();
  const bcf = () => enc.encode('<bcf:controlfile><bcf:datasource type="file">refs.bib</bcf:datasource></bcf:controlfile>');
  const localUnreachable = {
    async runBiber() {
      const error = new Error("could not reach local LibrePaper");
      error.name = "Unreachable";
      throw error;
    },
    async capabilities() {
      throw new Error("unreachable");
    },
  };
  const vmModule = {
    supported() {
      return { ok: true };
    },
    async prepare() {
      /* no image to fetch in this fake */
    },
    async runBiber(request) {
      return { ok: true, bbl: enc.encode("BBL-vm"), blg: "biber ran in the VM", exit: 0, tool: { name: "biber", version: "2.21", backend: "vm" } };
    },
  };
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: localUnreachable, vm: vmModule });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  worker().texReplies = [
    { status: 0, pdf: PDF, synctex: null, log: "Package biblatex Warning: Please (re)run Biber on the file: main\n", outputs: { "main.aux": enc.encode("\\relax\n"), "main.bcf": bcf() } },
    { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": enc.encode("\\relax\n") } },
  ];

  const result = await latex.compile(tree("main.tex", "\\cite{a}", { "refs.bib": enc.encode("@book{a,}") }));
  assert.equal(result.ok, true);
  assert.equal(result.provenance.bibliography, "vm-biber");
  assert.equal(result.provenance.backend, "browser");
  assert.ok(result.attempts.some((a) => a.stage === "vm-biber" && a.backend === "vm" && a.ok));
}

// ============================================================================
// 8. Browser TeX failure -> native once; a second failure of the same
// snapshot does not retry native.
// ============================================================================
{
  const project = nextProject();
  let nativeCalls = 0;
  const localWithFailingNative = {
    async capabilities() {
      return { tools: { pdflatex: { available: true, version: "1.40.27", note: "" } } };
    },
    async runTex() {
      nativeCalls += 1;
      return { ok: false, error: "pdflatex exited 1", log: "! Emergency stop.\n", diagnostics: [] };
    },
  };
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: localWithFailingNative, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  const failing = tree("main.tex", "\\bogus");
  worker().texReplies = [{ status: 1, pdf: null, synctex: null, log: "! Undefined control sequence.\nl.1 \\bogus\n", outputs: {} }];
  const first = await latex.compile(failing);
  assert.equal(first.ok, false);
  assert.equal(first.failure.kind, "native");
  assert.equal(nativeCalls, 1);
  assert.ok(first.attempts.some((a) => a.stage === "native" && a.ok === false));

  // Same tree, same engine, same release -> same snapshot. Native must not
  // be retried automatically a second time.
  worker().texReplies = [{ status: 1, pdf: null, synctex: null, log: "! Undefined control sequence.\nl.1 \\bogus\n", outputs: {} }];
  const second = await latex.compile(failing);
  assert.equal(second.ok, false);
  assert.equal(nativeCalls, 1, "no second automatic native attempt for the same snapshot");
  assert.equal(second.failure.kind, "tex");
}

// ============================================================================
// 9. A successful native fallback switches the session to the native route
// until `tryBrowser()`.
// ============================================================================
{
  const project = nextProject();
  let nativeCalls = 0;
  const localWithNative = {
    async capabilities() {
      return { tools: { pdflatex: { available: true, version: "1.40.27", note: "" } } };
    },
    async runTex({ engine }) {
      nativeCalls += 1;
      return {
        ok: true,
        pdf: enc.encode("NATIVE-PDF"),
        synctex: null,
        log: "native ok",
        diagnostics: [],
        exit: 0,
        provenance: { backend: "local", bibliography: null, engine, release: null, tools: { tex: "pdfTeX 1.40.27" } },
      };
    },
  };
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: localWithNative, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  worker().texReplies = [{ status: 1, pdf: null, synctex: null, log: "! Emergency stop.\n", outputs: {} }];
  const first = await latex.compile(tree("main.tex", "\\bogus"));
  assert.equal(first.ok, true);
  assert.equal(first.provenance.backend, "local");
  assert.equal(latex.status().route, "native");

  const texCallsBefore = worker().messages.filter((m) => m.cmd === "tex").length;
  const second = await latex.compile(tree("main.tex", "anything at all"));
  assert.equal(second.provenance.backend, "local");
  assert.equal(nativeCalls, 2);
  assert.equal(
    worker().messages.filter((m) => m.cmd === "tex").length,
    texCallsBefore,
    "the session-native route skipped the browser worker entirely",
  );

  latex.tryBrowser();
  assert.equal(latex.status().route, "browser");
  worker().texReplies = [{ status: 0, pdf: PDF, synctex: null, log: "", outputs: {} }];
  const third = await latex.compile(tree("main.tex", "back to browser"));
  assert.equal(third.provenance.backend, "browser");
  assert.ok(
    worker().messages.filter((m) => m.cmd === "tex").length > texCallsBefore,
    "tryBrowser() sent the next compile back through the worker",
  );
}

// ============================================================================
// 10. Cancel/supersede.
// ============================================================================
{
  const project = nextProject();
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  worker().texReplies = [
    async () => {
      await gate;
      return { status: 0, pdf: PDF, synctex: null, log: "", outputs: {} };
    },
  ];

  const running = latex.compile(tree("main.tex", "running"));
  await tick(3);
  const queued = latex.compile(tree("main.tex", "queued"));

  latex.cancel();
  await assert.rejects(running, (error) => error.name === "Superseded");
  await assert.rejects(queued, (error) => error.name === "Superseded");

  release(); // let the abandoned background run settle; nothing should observe it
  await tick(3);
}

// ============================================================================
// 11. Deadline.
// ============================================================================
{
  const project = nextProject();
  // The first call becomes `startedAt`; the second is the deadline check
  // ahead of the first pass (must still say "on time" or no pass would ever
  // run); every call after that -- the deadline check ahead of the second
  // pass -- reports the budget already spent.
  let calls = 0;
  const fakeNow = () => (calls++ < 2 ? 0 : latex.DEADLINE_MS + 1000);
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm, now: fakeNow });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  // Always asks for a rerun, so the loop's next-iteration deadline check --
  // not `MAX_PASSES` -- is what has to stop it.
  const texBefore = worker().messages.filter((m) => m.cmd === "tex").length;
  worker().texReplies = [{ status: 0, pdf: PDF, synctex: null, log: "Rerun to get cross-references right.", outputs: {} }];
  const result = await latex.compile(tree("main.tex", "never converges"));
  assert.equal(result.ok, false);
  assert.equal(result.failure.kind, "timeout");
  assert.equal(
    worker().messages.filter((m) => m.cmd === "tex").length - texBefore,
    1,
    "stopped after the deadline, not after 8 passes",
  );

  latex._testing.inject({ now: () => Date.now() });
}

// ============================================================================
// 12. A discarded (canceled) job's result, however late it arrives, never
// replaces a newer job's `lastResult` -- the practical form this guard takes
// in a controller that runs at most one job at a time: generations always
// resolve in order, so the only way an "older" result could ever reach the
// store is a canceled job's background work settling after its successor's.
// ============================================================================
{
  const project = nextProject();
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  let releaseStale;
  const staleGate = new Promise((resolve) => {
    releaseStale = resolve;
  });
  worker().texReplies = [
    async () => {
      await staleGate;
      return { status: 0, pdf: PDF, synctex: null, log: "stale", outputs: {} };
    },
  ];
  const stale = latex.compile(tree("main.tex", "stale"));
  await tick(3);
  latex.cancel(); // `stale` rejects now; its worker call keeps running in the background
  await assert.rejects(stale, (error) => error.name === "Superseded");

  worker().texReplies = [{ status: 0, pdf: PDF, synctex: null, log: "fresh", outputs: {} }];
  const fresh = await latex.compile(tree("main.tex", "fresh"));
  assert.equal(fresh.ok, true);
  assert.equal(latex.status().lastResult.job.generation, fresh.job.generation);

  releaseStale();
  await tick(5);
  assert.equal(
    latex.status().lastResult.job.generation,
    fresh.job.generation,
    "the late, canceled, older-generation result never became lastResult",
  );
}

// ============================================================================
// 13. A settings change invalidates cached bibliography reuse.
// ============================================================================
{
  const project = nextProject();
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  const aux = enc.encode("\\bibdata{refs}\n\\citation{a}\n");
  const undefinedCitation = { status: 0, pdf: PDF, synctex: null, log: "LaTeX Warning: Citation `a' on page 1 undefined on input line 1.", outputs: { "main.aux": aux } };
  const settled = { status: 0, pdf: PDF, synctex: null, log: "", outputs: { "main.aux": aux, "main.bbl": enc.encode("BBL-a") } };
  worker().texReplies = [undefinedCitation, settled];
  worker().bibtexReplies = [{ status: 0, bbl: enc.encode("BBL-a"), blg: "" }];
  const t = tree("main.tex", "citing a", { "refs.bib": enc.encode("@book{a,}") });
  const before = await latex.compile(t);
  assert.equal(before.ok, true);
  const bibtexBefore = FakeWorker.totalBibtexCalls;

  // `setSettings` drops the configured release, so the *next* compile that
  // actually reaches `ensureWorker` may end up on a fresh worker instance --
  // track BibTeX calls at the class level rather than per-instance so this
  // assertion survives that either way.
  latex.setSettings({ release: "r1" }); // a settings call is itself the change event; it invalidates reuse

  FakeWorker.nextTexReplies = [undefinedCitation, settled];
  FakeWorker.nextBibtexReplies = [{ status: 0, bbl: enc.encode("BBL-a"), blg: "" }];
  worker().texReplies = [undefinedCitation, settled]; // in case the existing worker is reused instead
  worker().bibtexReplies = [{ status: 0, bbl: enc.encode("BBL-a"), blg: "" }];
  const after = await latex.compile(t);
  assert.equal(after.ok, true);
  assert.ok(FakeWorker.totalBibtexCalls > bibtexBefore, "the identical aux was not served from the pre-settings-change cache");
}

// ============================================================================
// Bonus: worker death rejects the active job with an ordinary `failure.kind
// === "init"` result (design requirement 1), then retires and recreates the
// worker lazily rather than leaving the queue stuck behind a dead one.
// ============================================================================
{
  const project = nextProject();
  latex._testing.inject({ worker: FakeWorker, fetch: fakeFetch, local: noLocal, vm: noVm });
  latex.configure({ project, settings: { engine: "pdflatex", release: "r1" } });

  // The project's release is unchanged from the previous scenario, so
  // `ensureWorker` reuses the existing worker instance rather than making a
  // new one -- set the hang directly on it and wait for *this* compile's own
  // `tex` message by counting from where we are now, since its `.messages`
  // history already carries earlier scenarios' traffic.
  const w0 = worker();
  const messagesBefore = w0.messages.length;
  w0.texReplies = [() => new Promise(() => {})]; // never resolves; the crash is what ends it
  const crashing = latex.compile(tree("main.tex", "about to crash"));
  const posted = () => w0.messages.length > messagesBefore && w0.messages.at(-1).cmd === "tex";
  assert.ok(await until(posted), "this compile's own tex command was posted");
  w0.crash("the compiler worker died");
  const crashed = await crashing;
  assert.equal(crashed.ok, false);
  assert.equal(crashed.failure.kind, "init");

  // The dead worker was retired (design requirement 1): the next compile
  // must construct a genuinely new instance, which is exactly what consumes
  // `nextTexReplies`.
  FakeWorker.nextTexReplies = [{ status: 0, pdf: PDF, synctex: null, log: "", outputs: {} }];
  const recovered = await latex.compile(tree("main.tex", "after recovery"));
  assert.equal(recovered.ok, true);
  assert.notEqual(worker(), w0, "a fresh worker was created lazily rather than reusing the dead one");
}

// A legacy mirror is an operator error, not a request for release "undefined".
{
  latex.at("/legacy-mirror/");
  latex._testing.inject({ worker: FakeWorker, fetch: async () => new Response(JSON.stringify({ version: 1, distributions: {} })), local: noLocal, vm: noVm });
  latex.configure({ project: nextProject(), settings: { engine: "pdflatex", release: null } });
  const result = await latex.compile(tree("main.tex", "source"));
  assert.equal(result.ok, false);
  assert.equal(result.failure.kind, "resources");
  assert.match(result.failure.message, /no default WasmTex release/);
  assert.match(result.failure.message, /make latex-mirror/);
  assert.doesNotMatch(result.failure.message, /undefined/);
}

// ============================================================================
// 15. `worker.js`'s `ensureEngine`, driven directly (not through the
// `FakeWorker` above, which stands in for the whole of `worker.js` and so
// never exercises it): a fake nested engine `Worker` stands in for a real
// WasmTex engine controller, and this drives worker.js's own section 2.4
// protocol by id, the same way `latex-wasmtex-browser.mjs`'s in-page driver
// does against the real thing. Checks SPEC-latex.md's bundle-mode fan-out --
// every bundle-capable kind receives `loadbundleindex`, and XeTeX alone also
// receives `loadicudata` with the release's ICU table inflated from the
// gzip it ships.
// ============================================================================
{
  const icuPlain = enc.encode("ICU-DATA-FIXTURE");
  const icuGz = new Uint8Array(gzipSync(Buffer.from(icuPlain)));
  const fmtPlain = enc.encode("XETEX-FORMAT-FIXTURE");
  const fmtGz = new Uint8Array(gzipSync(Buffer.from(fmtPlain)));

  async function sha256Hex(bytes) {
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
  }

  const BASE = "https://mirror.example/mirror/";
  const bundlesIndexBytes = enc.encode(JSON.stringify({ bundles: {}, files: {} }));

  const files = {
    "wasmtex-xetex.fmt.gz": { url: "wasmtex/rel1/wasmtex-xetex.fmt.gz", sha256: await sha256Hex(fmtGz), size: fmtGz.length },
    "icudt68l.dat.gz": { url: "wasmtex/rel1/icudt68l.dat.gz", sha256: await sha256Hex(icuGz), size: icuGz.length },
  };
  const release = {
    id: "rel1",
    digest: "c".repeat(64),
    base: "wasmtex/rel1/",
    texlive_base: "texlive/snap1/",
    bundles: { index: "wasmtex/rel1/bundles/bundles.json" }, // no `sha256`: digest check is skipped, exercised elsewhere
    engines: {
      xetex: { worker: "wasmtex-xetex.worker.js", format: "wasmtex-xetex.fmt.gz", icu: "icudt68l.dat.gz" },
      dvipdfm: { worker: "wasmtex-dvipdfm.worker.js" },
      bibtex: { worker: "wasmtex-bibtex.worker.js" },
      pdftex: { worker: "wasmtex-pdftex.worker.js" },
    },
    files,
  };

  const engineWorkers = [];
  class FakeEngineWorker {
    constructor(url) {
      this.url = String(url);
      this.messages = [];
      this.onmessage = null;
      this.onerror = null;
      this.dead = false;
      engineWorkers.push(this);
      // Emscripten's postRun: the one reply every real controller sends with
      // no `cmd`, meaning the engine finished starting (wasmtex.js's own
      // header note). Queued so it fires only after `worker.onmessage` is
      // assigned, matching a real Worker's genuine asynchrony.
      queueMicrotask(() => this.onmessage?.({ data: { result: "ok" } }));
    }
    postMessage(message) {
      if (this.dead) throw new Error("posted to a terminated fake engine worker");
      this.messages.push(message);
      queueMicrotask(() => {
        if (this.dead) return;
        const reply = (extra) => this.onmessage?.({ data: extra });
        if (typeof message.cmd === "string" && message.cmd.startsWith("compile")) {
          return reply({ cmd: "compile", result: "ok" });
        }
        switch (message.cmd) {
          case "settexliveurl":
            return; // no controller replies to this one
          case "readfile":
            return reply({
              cmd: "readfile",
              result: "ok",
              data: message.url && message.url.endsWith(".aux") ? "\\relax\n" : null,
            });
          default:
            return reply({ cmd: message.cmd, result: "ok" });
        }
      });
    }
    terminate() {
      this.dead = true;
    }
  }

  const previousSelf = globalThis.self;
  const previousWorker = globalThis.Worker;
  const previousFetch = globalThis.fetch;
  globalThis.self = globalThis;
  globalThis.Worker = FakeEngineWorker;
  globalThis.fetch = async (url) => {
    const key = typeof url === "string" ? url : url.toString();
    if (key === new URL(release.bundles.index, BASE).href) return new Response(bundlesIndexBytes, { status: 200 });
    if (key === new URL(files["wasmtex-xetex.fmt.gz"].url, BASE).href) return new Response(fmtGz, { status: 200 });
    if (key === new URL(files["icudt68l.dat.gz"].url, BASE).href) return new Response(icuGz, { status: 200 });
    return new Response(null, { status: 404 });
  };

  await import("../src/lib/latex/worker.js?ensure-engine-check"); // registers self.onmessage as a side effect

  let seq = 0;
  const pending = new Map();
  const outerOnMessage = globalThis.self.onmessage;
  globalThis.self.postMessage = (msg) => {
    if (!msg || msg.id === undefined) return; // unsolicited progress/downloading, not awaited here
    const waiter = pending.get(msg.id);
    if (!waiter) return;
    pending.delete(msg.id);
    msg.failed ? waiter.reject(new Error(msg.failed)) : waiter.resolve(msg);
  };
  function send(cmd, extra) {
    const id = ++seq;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      outerOnMessage({ data: Object.assign({ id, cmd }, extra || {}) });
    });
  }

  try {
    await send("configure", { base: BASE, release, texlive: null });
    await send("stage", { engine: "xelatex", tree: { main: "main.tex", texts: { "main.tex": "x" } }, generated: {} });

    const xetexWorker = engineWorkers.find((w) => w.url === new URL("wasmtex/rel1/wasmtex-xetex.worker.js", BASE).href);
    const dvipdfmWorker = engineWorkers.find((w) => w.url === new URL("wasmtex/rel1/wasmtex-dvipdfm.worker.js", BASE).href);
    assert.ok(xetexWorker, "a fake xetex engine worker was created");
    assert.ok(dvipdfmWorker, "a fake dvipdfm engine worker was created");

    const xetexCmds = xetexWorker.messages.map((m) => m.cmd);
    assert.ok(xetexCmds.includes("loadbundleindex"), "the bundled xetex worker received loadbundleindex");
    assert.ok(xetexCmds.includes("loadicudata"), "the bundled xetex worker also received loadicudata");
    const icuMessage = xetexWorker.messages.find((m) => m.cmd === "loadicudata");
    assert.deepEqual(
      new Uint8Array(icuMessage.data),
      icuPlain,
      "the ICU bytes sent to the worker were inflated from the release's icudt68l.dat.gz fixture",
    );

    const dvipdfmCmds = dvipdfmWorker.messages.map((m) => m.cmd);
    assert.ok(dvipdfmCmds.includes("loadbundleindex"), "the bundled dvipdfm worker received loadbundleindex too");
    assert.ok(!dvipdfmCmds.includes("loadicudata"), "loadicudata is XeTeX-only");

    // BibTeX, through worker.js's own `bibtex` command: reads the primary
    // (xetex) engine's staged .aux, then lazily creates the bibtex engine.
    await send("bibtex", { stem: "main", eight: false });
    const bibtexWorker = engineWorkers.find((w) => w.url === new URL("wasmtex/rel1/wasmtex-bibtex.worker.js", BASE).href);
    assert.ok(bibtexWorker, "a fake bibtex engine worker was created");
    assert.ok(
      bibtexWorker.messages.map((m) => m.cmd).includes("loadbundleindex"),
      "the bundled bibtex worker received loadbundleindex",
    );

    await send("retire", {});
  } finally {
    globalThis.self = previousSelf;
    globalThis.Worker = previousWorker;
    globalThis.fetch = previousFetch;
  }
  console.log("latex worker/ensureEngine: bundle-mode fan-out and XeTeX ICU data checked");
}

latex._testing.reset();
console.log("latex controller: queue, bibliography reuse, routing and lifecycle checks passed");
