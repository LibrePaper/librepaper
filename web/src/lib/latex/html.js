// HTML previews have their own engine lifetime: switching the View menu must
// not change the PDF compiler's settings or bibliography state.
import { createEngine } from "./driver.js";
import { fetchVerified } from "./resources.js";

const DEADLINE_MS = 240_000;

/** @typedef {{url: string, sha256: string, size: number}} LatexAsset */
/** @typedef {{worker: string, files?: string[]}} LatexEngineSpec */
/** @typedef {{format: number, id: string, digest?: string, files: Record<string, LatexAsset>, bundles?: {index: string, sha256: string}, engines: {latexml?: LatexEngineSpec}}} LatexRelease */

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

const ERROR_LINE = /^(?:! )?((?:LaTeX|Package \S+|Class \S+) Error: .*)$/;
const TEX_ERROR_LINE = /^! (.*)$/;
const CONTINUATION_LINE = /^\([^)]+\)\s*(.*)$/;
const MAX_DIAGNOSTICS = 10;
const ANSI = /\x1b\[[0-9;]*m/g;
const LATEXML_HEADER = /^(Error|Warning|Fatal):([^:\s]*):(\S*)\s*(.*)$/;
const LATEXML_AT = /^\tat (.+?); line (\d+) col (\d+)(?: - line (\d+) col (\d+))?/;

/** @param {{file?: string, line?: number, column?: number, end_line?: number, end_column?: number}} [at] */
function diagnostic(message, hints, main, severity = "error", at = {}) {
  const line = at.line ?? 0;
  const column = at.column ?? 0;
  return {
    severity, message, hints, file: at.file ?? main, line, column,
    end_line: at.end_line ?? line, end_column: at.end_column ?? column,
  };
}

/** Finds the project file a LaTeXML source name refers to, if any. */
function resolvePath(name, paths) {
  if (paths.includes(name)) return name;
  if (paths.includes(`${name}.tex`)) return `${name}.tex`;
  return paths.find((path) => path.split("/").pop().replace(/\.[^.]*$/, "") === name) ?? null;
}

function hintsFor(message) {
  if (message.includes("Cannot run piped system commands")) {
    return ["The HTML converter cannot run external programs, so packages that shell out (minted, some tcolorbox listings) fail here. The PDF view may still work."];
  }
  if (message.includes("Mismatched LaTeX support files")) {
    return ["The HTML converter's copy of LaTeX is older than the packages this document loads. This is a limitation of LibrePaper's HTML preview, not an error in your document. The PDF view may still work."];
  }
  return [];
}

/**
 * Turns a LaTeX or LaTeXML log into the diagnostics worth showing: errors
 * first, then warnings, each capped. Returns [] when nothing was recognised.
 * @param {string|null|undefined} log
 * @param {{main: string, paths: string[]}} project
 */
export function logDiagnostics(log, { main, paths }) {
  const lines = String(log ?? "").split("\n").map((line) => line.replace(ANSI, ""));
  const seen = new Set();
  const errors = [];
  const warnings = [];
  const add = (severity, message, at) => {
    if (!message) return;
    const item = diagnostic(message, hintsFor(message), main, severity, at);
    const key = [item.severity, item.file, item.line, item.message].join("\0");
    if (seen.has(key)) return;
    seen.add(key);
    (severity === "warning" ? warnings : errors).push(item);
  };
  for (let i = 0; i < lines.length; i++) {
    const record = lines[i].match(LATEXML_HEADER);
    if (record) {
      const [, kind, category, object, rest] = record;
      const parts = [rest.trim() || `${category} ${object}`.trim()];
      let location = null;
      for (let j = i + 1; j < lines.length && lines[j].startsWith("\t"); j++) {
        i = j;
        const found = lines[j].match(LATEXML_AT);
        if (found) location = found;
        else if (lines[j].startsWith("\tIn /")) continue;
        else if (lines[j].trim()) parts.push(lines[j].trim());
      }
      const file = location ? resolvePath(location[1], paths) : null;
      const at = file ? {
        file,
        line: Number(location[2]),
        column: Number(location[3]),
        end_line: location[4] && Number(location[4]),
        end_column: location[5] && Number(location[5]),
      } : {};
      add(kind === "Warning" ? "warning" : "error", parts.join(" "), at);
      continue;
    }
    const match = lines[i].match(ERROR_LINE) ?? lines[i].match(TEX_ERROR_LINE);
    if (!match) continue;
    const parts = [match[1].trim()];
    for (let j = i + 1; j < lines.length; j++) {
      const next = lines[j].match(CONTINUATION_LINE);
      if (!next) break;
      if (next[1].trim()) parts.push(next[1].trim());
      i = j;
    }
    add("error", parts.join(" "), {});
  }
  return [...errors.slice(0, MAX_DIAGNOSTICS), ...warnings.slice(0, MAX_DIAGNOSTICS)];
}

/** @param {{makeEngine?: typeof createEngine, request?: (input: string|URL, init?: RequestInit) => Promise<Response>, verified?: typeof fetchVerified, deadline?: number}} [options] */
export function createHtmlCompiler({
  makeEngine = createEngine,
  request = (input, init) => fetch(input, init),
  verified = fetchVerified,
  deadline = DEADLINE_MS,
} = {}) {
  let engine = null;
  let engineKey = null;
  /** @type {LatexRelease|null} */
  let pinned = null;
  let pinnedBase = null;
  /** @type {{base: string, pending: Promise<LatexRelease>}|null} */
  let pinnedRequest = null;
  let queued = null;
  let running = false;
  let cancelActive = null;
  let epoch = 0;
  // Engines whose start-up timings an earlier result already carried.
  const reported = new WeakSet();

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

  /** @param {string} base @param {Record<string, unknown>} [settings] */
  async function configuration(base, settings = {}) {
    base = new URL(base, globalThis.location?.href || "http://localhost/").href;
    if (!base.endsWith("/")) base += "/";
    if (pinnedBase !== base || !pinned) {
      if (!pinnedRequest || pinnedRequest.base !== base) {
        const pending = (async () => {
          const response = await request(new URL("release.json", base));
          if (!response.ok) throw new Error(`LaTeX release unavailable (${response.status})`);
          const data = await response.json();
          if (data.format !== 2) throw new Error("Unsupported LaTeX release format");
          return /** @type {LatexRelease} */ (data);
        })();
        pinnedRequest = { base, pending };
      }
      const captured = pinnedRequest;
      try { pinned = await captured.pending; pinnedBase = base; }
      finally { if (pinnedRequest === captured) pinnedRequest = null; }
    }
    // HTML previews always use the release the server pinned (`base` is its
    // directory). Legacy per-document release values are deliberately ignored.
    const capturedSettings = { ...settings };
    delete capturedSettings.release;
    const release = pinned;
    const releaseId = release.id;
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
      url: new URL(spec.worker, base).href,
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
    // A queued historical render carries the release identity resolved by
    // the caller. Its base is authoritative; consulting the live chooser
    // here could fetch a different release than the cache key names.
    const base = new URL(options.configuration?.base || options.base, globalThis.location?.href || "http://localhost/").href;
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
    const startup = {};
    if (target.timings && !reported.has(target)) {
      reported.add(target);
      startup.latexml = target.timings;
    }
    const ok = result.ok && typeof result.html === "string" && result.html.length > 0;
    const diagnostics = logDiagnostics(result.log, { main: tree.main, paths: Object.keys({ ...tree.texts, ...tree.assets }) });
    if (!ok && !diagnostics.some((item) => item.severity === "error")) {
      // A log the parser found nothing in still ends with the engine's last word.
      const tail = String(result.log ?? "").split("\n").filter((line) => line.trim()).slice(-12).join("\n");
      diagnostics.push(diagnostic(tail || "LaTeX HTML conversion failed", [], tree.main));
    }
    return {
      html: ok ? result.html : null,
      pdf: null,
      ok,
      seconds: (Date.now() - started) / 1000,
      log: result.log,
      diagnostics,
      timings: {
        startup,
        passes: [{ kind: "latexml", phaseTimings: result.phaseTimings ?? null, bundleStats: result.bundleStats ?? null }],
      },
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
