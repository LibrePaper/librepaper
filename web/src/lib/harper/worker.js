// Harper runs in this worker so a grammar pass never blocks the editor and its
// 280 MB goes away with the worker if it fails.
//
// This is our own worker because harper.js's WorkerLinter starts its worker
// from a blob: URL, which the app's CSP (worker-src 'self') forbids. The
// digest is checked here because Harper fetches a module by URL itself and
// would never see the digest in it. The checked bytes are handed to Harper as
// an object URL.
//
// harper.js is Apache-2.0. The wasm is fetched from the asset mirror, unmodified.
import { LocalLinter, createBinaryModuleFromUrl, Dialect } from "harper.js";
import { verifiedModuleBytes } from "../renderer-wasm.js";

const DIALECTS = {
  american: Dialect.American,
  british: Dialect.British,
  australian: Dialect.Australian,
  canadian: Dialect.Canadian,
  indian: Dialect.Indian,
};
const SUGGESTION_KINDS = ["replace", "remove", "insert"];

const imported = new Set();
let ready = null;
let dialectNow = null;

function dialectOf(name) {
  return DIALECTS[name] ?? Dialect.American;
}

async function create(url, dialect, words) {
  const bytes = await verifiedModuleBytes(url);
  const objectUrl = URL.createObjectURL(new Blob([bytes], { type: "application/wasm" }));
  try {
    const binary = createBinaryModuleFromUrl(objectUrl, "slim");
    const linter = new LocalLinter({ binary, dialect: dialectOf(dialect) });
    await linter.setup();
    dialectNow = dialect;
    const fresh = words.filter((word) => !imported.has(word));
    if (fresh.length) await linter.importWords(fresh);
    for (const word of fresh) imported.add(word);
    return linter;
  } finally {
    URL.revokeObjectURL(objectUrl);
  }
}

async function prepare({ url, dialect, words = [] }) {
  if (!ready) {
    ready = create(url, dialect, words).catch((error) => {
      ready = null;
      throw error;
    });
  }
  const linter = await ready;
  const fresh = words.filter((word) => !imported.has(word));
  if (fresh.length) {
    await linter.importWords(fresh);
    for (const word of fresh) imported.add(word);
  }
  if (dialect !== dialectNow) {
    await linter.setDialect(dialectOf(dialect));
    dialectNow = dialect;
  }
  return linter;
}

function plain(lint) {
  const span = lint.span();
  return {
    from: span.start,
    to: span.end,
    kind: lint.lint_kind(),
    message: lint.message(),
    problem: lint.get_problem_text(),
    suggestions: lint.suggestions().map((suggestion) => ({
      kind: SUGGESTION_KINDS[suggestion.kind()] ?? "replace",
      text: suggestion.get_replacement_text(),
    })),
  };
}

async function handle(data) {
  const linter = await prepare(data);
  if (data.op === "addWords") return null;
  if (data.op === "lint") {
    const lints = await linter.lint(data.text, { language: "plaintext" });
    return lints.map(plain);
  }
  throw new Error(`unknown Harper operation ${data.op}`);
}

// Imported by tests in Node, where there is no worker scope to listen on.
/** @type {any} */
const WorkerScope = Reflect.get(globalThis, "WorkerGlobalScope");
/** @type {{ onmessage: ((event: MessageEvent) => void) | null, postMessage: (message: unknown) => void } | null} */
const scope = WorkerScope && self instanceof WorkerScope ? /** @type {any} */ (self) : null;
if (scope) {
  scope.onmessage = async ({ data }) => {
    const wasReady = ready !== null;
    try {
      scope.postMessage({ id: data.id, result: await handle(data) });
    } catch (error) {
      // A failure while loading leaves nothing usable, so the client discards
      // this worker. Once loaded, a failed lint is only that request's failure.
      scope.postMessage({ id: data.id, error: error?.message || String(error), fatal: !wasReady || ready === null });
    }
  };
}
