// HTML previews have their own engine lifetime: switching the View menu must
// not change the PDF compiler's settings, bibliography state or saved output.
import { createEngine } from "./driver.js";
import { fetchVerified } from "./resources.js";

const DEADLINE_MS = 240_000;

function superseded() {
  return Object.assign(new Error("Preview superseded"), { name: "Superseded" });
}

function projectPath(path) {
  if (typeof path !== "string" || !path || path.startsWith("/") || path.includes("\\") ||
      path.includes("\0") || path.split("/").some((part) => part === ".." || part === "." || !part)) {
    throw new Error(`Invalid project file path: ${path}`);
  }
  return path;
}

export function createHtmlCompiler({
  makeEngine = createEngine,
  request = (...args) => fetch(...args),
  verified = fetchVerified,
  deadline = DEADLINE_MS,
} = {}) {
  let engine = null;
  let engineKey = null;
  let manifest = null;
  let manifestBase = null;
  let queued = null;
  let running = false;
  let cancelActive = null;
  let epoch = 0;

  function retire() {
    engine?.terminate();
    engine = null;
    engineKey = null;
  }

  function cancel() {
    epoch++;
    retire();
    cancelActive?.(superseded());
    if (queued) queued.reject(superseded());
    queued = null;
  }

  async function prepare(base, settings, current) {
    if (manifestBase !== base || !manifest) {
      const response = await request(new URL("manifest.json", base));
      if (!response.ok) throw new Error(`LaTeX mirror unavailable (${response.status})`);
      const data = await response.json();
      if (current !== epoch) throw superseded();
      if (data.format !== 1) throw new Error("Unsupported LaTeX mirror format");
      manifest = data;
      manifestBase = base;
    }
    // Existing documents pin the PDF toolchain. Older releases predate HTML
    // preview, so use the current preview engine without changing that pin.
    const pinned = settings?.release;
    const releaseId = manifest.releases?.[pinned]?.engines?.latexml
      ? pinned : manifest.default_release;
    const release = manifest.releases?.[releaseId];
    const spec = release?.engines?.latexml;
    if (!spec) throw new Error("This LaTeX release does not include the HTML preview renderer.");
    const key = `${base}\n${releaseId}`;
    if (engine && engineKey === key) return engine;
    retire();
    if (!release.bundles?.index || !release.bundles.sha256) throw new Error("Missing verified TeX bundle index");
    const indexUrl = new URL(release.bundles.index, base);
    const index = new Uint8Array(await (await verified(
      { ...release, digest: releaseId }, indexUrl.href, { sha256: release.bundles.sha256 },
    )).arrayBuffer());
    if (current !== epoch) throw superseded();
    const target = makeEngine({
      kind: "latexml",
      url: new URL(`${release.base}${spec.worker}`, base).href,
      texliveUrl: new URL(".", indexUrl).href,
      release,
    });
    engine = target;
    await target.init();
    if (current !== epoch) throw superseded();
    const loaded = await target.loadBundleIndex(index);
    if (current !== epoch) throw superseded();
    if (loaded?.result !== "ok") throw new Error("Could not load the TeX bundle index");
    engineKey = key;
    return target;
  }

  async function run({ tree, options }, current) {
    const started = Date.now();
    const base = new URL(options.base || "/latex/", globalThis.location?.href || "http://localhost/").href;
    const target = await prepare(base.endsWith("/") ? base : `${base}/`, options.settings, current);
    if (current !== epoch) throw superseded();
    projectPath(tree.main);
    await target.flushCache();
    for (const [path, content] of Object.entries({ ...tree.texts, ...tree.assets })) {
      projectPath(path);
      const slash = path.lastIndexOf("/");
      if (slash >= 0) await target.mkdir(path.slice(0, slash));
      await target.writeFile(path, content);
    }
    if (current !== epoch) throw superseded();
    target.setMainFile(tree.main);
    const result = await target.run("compilelatex");
    if (current !== epoch) throw superseded();
    // A WASM trap can leave Rust's thread-local state borrowed or incomplete.
    // Document errors are reusable; a failed runtime needs a fresh worker.
    if (result.status < 0) retire();
    const ok = result.ok && typeof result.html === "string" && result.html.length > 0;
    return {
      html: ok ? result.html : null,
      pdf: null,
      ok,
      seconds: (Date.now() - started) / 1000,
      log: result.log,
      diagnostics: result.diagnostics?.length ? result.diagnostics : ok ? [] : [{
        severity: "error", message: result.log || "LaTeX HTML conversion failed", hints: [],
        file: tree.main, line: 0, column: 0, end_line: 0, end_column: 0,
      }],
    };
  }

  async function drain() {
    if (running) return;
    running = true;
    while (queued) {
      const job = queued;
      queued = null;
      const current = epoch;
      let timer;
      try {
        const result = await Promise.race([
          run(job, current),
          new Promise((_, reject) => {
            cancelActive = reject;
            timer = setTimeout(() => {
              epoch++;
              retire();
              reject(new Error("LaTeX HTML preview timed out"));
            }, deadline);
          }),
        ]);
        job.resolve(result);
      } catch (error) {
        retire();
        job.reject(error);
      } finally {
        clearTimeout(timer);
        cancelActive = null;
      }
    }
    running = false;
  }

  function compile(tree, options = {}) {
    // Snapshot before waiting: a later edit must never mutate a queued job.
    const snapshot = {
      main: tree.main,
      texts: { ...tree.texts },
      assets: Object.fromEntries(Object.entries(tree.assets || {}).map(([path, bytes]) => [path, bytes.slice()])),
    };
    return new Promise((resolve, reject) => {
      if (queued) queued.reject(superseded());
      queued = { tree: snapshot, options: { ...options, settings: { ...options.settings } }, resolve, reject };
      void drain();
    });
  }

  return { compile, cancel };
}

const compiler = createHtmlCompiler();
export const compile = compiler.compile;
export const cancel = compiler.cancel;
