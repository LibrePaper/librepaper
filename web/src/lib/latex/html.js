// HTML previews have their own engine lifetime: switching the View menu must
// not change the PDF compiler's settings or bibliography state.
import { createEngine } from "./driver.js";
import { fetchVerified } from "./resources.js";

const DEADLINE_MS = 240_000;
const DEFAULT_BASE = "https://latex.librepaper.workers.dev/";

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
  let manifestRequest = null;
  let queued = null;
  let running = false;
  let cancelActive = null;
  let epoch = 0;

  function retire() {
    engine?.terminate();
    engine = null;
    engineKey = null;
  }

  function cancel({ keepWarm = false } = {}) {
    epoch++;
    // A view switch can reuse an idle runtime and its loaded TeX resources.
    // Active jobs still need termination: their worker may be mutating state.
    if (!keepWarm || running) retire();
    cancelActive?.(superseded());
    if (queued) queued.reject(superseded());
    queued = null;
  }

  async function configuration(base, settings = {}) {
    base = new URL(base || DEFAULT_BASE, globalThis.location?.href || "http://localhost/").href;
    if (!base.endsWith("/")) base += "/";
    if (manifestBase !== base || !manifest) {
      if (!manifestRequest || manifestRequest.base !== base) {
        const pending = (async () => {
          const response = await request(new URL("manifest.json", base));
          if (!response.ok) throw new Error(`LaTeX mirror unavailable (${response.status})`);
          const data = await response.json();
          if (data.format !== 1) throw new Error("Unsupported LaTeX mirror format");
          return data;
        })();
        manifestRequest = { base, pending };
      }
      const captured = manifestRequest;
      try { manifest = await captured.pending; manifestBase = base; }
      finally { if (manifestRequest === captured) manifestRequest = null; }
    }
    // HTML previews always use the mirror's current default release. Legacy
    // per-document release values are deliberately ignored.
    const capturedSettings = { ...settings };
    delete capturedSettings.release;
    const releaseId = manifest.default_release;
    const release = manifest.releases?.[releaseId];
    const spec = release?.engines?.latexml;
    if (!spec) throw new Error("This LaTeX release does not include the HTML preview renderer.");
    const capturedRelease = structuredClone(release);
    return {
      base, releaseId, release: capturedRelease, settings: capturedSettings,
      fallback: false,
      identity: JSON.stringify({ base, releaseId, release: capturedRelease, settings: capturedSettings, metadataVersion: 1 }),
    };
  }

  async function prepare(base, settings, current, captured) {
    const resolved = captured || await configuration(base, settings);
    if (current !== epoch) throw superseded();
    const { releaseId, release } = resolved;
    base = resolved.base;
    const spec = release.engines.latexml;
    const key = resolved.identity;
    if (engine && engineKey === key) return engine;
    retire();
    const bundleFile = Object.values(release.files || {}).find((file) => file?.url === release.bundles?.index);
    if (!bundleFile || !/^[a-f0-9]{64}$/.test(bundleFile.sha256) || !Number.isInteger(bundleFile.size) || bundleFile.size < 0 ||
        bundleFile.sha256 !== release.bundles?.sha256) throw new Error("Missing verified TeX bundle index");
    const indexUrl = new URL(release.bundles.index, base);
    const index = new Uint8Array(await (await verified(
      { ...release, digest: releaseId }, indexUrl.href, { sha256: bundleFile.sha256, size: bundleFile.size },
    )).arrayBuffer());
    if (current !== epoch) throw superseded();
    const target = makeEngine({
      kind: "latexml",
      url: new URL(`${release.base}${spec.worker}`, base).href,
      base,
      texliveUrl: new URL(".", indexUrl).href,
      release,
      assets: Object.fromEntries((() => {
        const names = [...new Set(spec.files || [spec.worker])];
        if (!names.includes(spec.worker)) throw new Error(`Incomplete latexml asset inventory: ${spec.worker}`);
        return names;
      })().map((name) => {
        const file = release.files?.[name];
        if (!file || !/^[a-f0-9]{64}$/.test(file.sha256) || !Number.isInteger(file.size) || file.size < 0) {
          throw new Error(`Missing or invalid manifest metadata for latexml asset ${name}`);
        }
        return [name, file];
      })),
      workerName: spec.worker,
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
    // A queued historical render carries the manifest identity resolved by
    // the caller. Its base is authoritative; consulting the live chooser
    // here could fetch a different release than the cache key names.
    const base = new URL(options.configuration?.base || options.base || DEFAULT_BASE, globalThis.location?.href || "http://localhost/").href;
    const target = await prepare(base.endsWith("/") ? base : `${base}/`, options.settings, current, options.configuration);
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
    const capturedConfiguration = options.configuration == null
      ? null : structuredClone(options.configuration);
    const snapshot = {
      main: tree.main,
      texts: { ...tree.texts },
      assets: Object.fromEntries(Object.entries(tree.assets || {}).map(([path, bytes]) => [path, bytes.slice()])),
    };
    return new Promise((resolve, reject) => {
      if (queued) queued.reject(superseded());
      queued = {
        tree: snapshot,
        options: {
          ...options,
          settings: { ...options.settings },
          configuration: capturedConfiguration,
        },
        resolve,
        reject,
      };
      void drain();
    });
  }

  return { compile, cancel, configuration };
}

const compiler = createHtmlCompiler();
export const compile = compiler.compile;
export const cancel = compiler.cancel;
export const configuration = compiler.configuration;
