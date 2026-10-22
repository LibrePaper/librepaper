// The document page is a separate navigation, so its scripts and wasm start
// cold. This fetches them into the HTTP cache while the user is on the
// project list, so opening a document skips the download. The wasm still
// compiles when the document page loads. Engines for the account's formats
// are prefetched too, after the reader's own assets and one at a time.
import loroWasm from "loro-crdt/web/loro_wasm_bg.wasm?url";

let started = false;

export function warmReader({ formats = [], latexOutput = "html" } = {}) {
  if (started) return;
  started = true;
  // Network Information is Chromium-only and absent from the DOM typings.
  const connection = /** @type {{ saveData?: boolean, effectiveType?: string } | undefined} */ (
    /** @type {any} */ (navigator).connection);
  if (connection?.saveData || connection?.effectiveType?.includes("2g")) return;
  const run = () => { void prefetch(formats, latexOutput); };
  if (typeof requestIdleCallback === "function") requestIdleCallback(run, { timeout: 5000 });
  else setTimeout(run, 2000);
}

async function prefetch(formats, latexOutput) {
  try {
    const response = await fetch("/reader.html");
    if (!response.ok) return;
    const markup = new DOMParser().parseFromString(await response.text(), "text/html");
    const urls = new Set();
    const add = (value) => {
      if (!value) return;
      const url = new URL(value, location.origin);
      if (url.origin === location.origin) urls.add(url.href);
    };
    for (const element of markup.querySelectorAll("script[src], link[rel=modulepreload][href], link[rel=stylesheet][href]")) {
      add(element.getAttribute("src") || element.getAttribute("href"));
    }
    add(loroWasm);
    const queue = [...urls];
    const drain = async () => {
      while (queue.length) {
        const url = queue.shift();
        await warm(url, true);
      }
    };
    await Promise.all(Array.from({ length: Math.min(3, queue.length) }, drain));

    // Engines are fetched after the reader's assets, one at a time. The
    // Typst engine is tens of megabytes, so it must not run beside them.
    const engines = [];
    const addEngine = (href) => {
      if (!href || urls.has(href)) return;
      urls.add(href);
      engines.push(href);
    };
    const wanted = new Set(formats);
    const modules = readModules(markup);
    if (wanted.has("typst") && typeof modules.typst === "string") addEngine(new URL(modules.typst, location.origin).href);
    if ((wanted.has("markdown") || wanted.has("quarto")) && typeof modules.markdown === "string") {
      addEngine(new URL(modules.markdown, location.origin).href);
    }
    if (wanted.has("latex")) await addLatexEngines(latexOutput, addEngine);
    for (const url of engines) await warm(url, false);
  } catch { /* best effort: the document page works without the warm cache */ }
}

// The reader page names its modules in a JSON script element, keyed by name.
function readModules(markup) {
  try {
    const element = markup.getElementById("librepaper-modules");
    return element ? JSON.parse(element.textContent) || {} : {};
  } catch { return {}; }
}

// The LaTeX engine files live on the mirror named by /api/config, listed in
// its release.json. Failures here are swallowed so the other engines still load.
async function addLatexEngines(latexOutput, addEngine) {
  try {
    const config = await fetch("/api/config").then((answer) => (answer.ok ? answer.json() : null));
    const latexMirror = config?.latexMirror;
    if (typeof latexMirror !== "string" || !latexMirror) return;
    const answer = await fetch(new URL("release.json", latexMirror));
    if (!answer.ok) return;
    const release = await answer.json();
    const engine = latexOutput === "pdf" ? "pdftex" : "latexml";
    for (const name of release.engines?.[engine]?.files || []) {
      const path = release.files?.[name]?.url;
      if (path) addEngine(new URL(path, latexMirror).href);
    }
    if (typeof release.bundles?.index === "string") addEngine(new URL(release.bundles.index, latexMirror).href);
  } catch { /* best effort: the LaTeX engines are fetched when a LaTeX document opens */ }
}

// Reads the body without keeping it, so a large engine never sits in memory.
async function warm(url, lowPriority) {
  try {
    const asset = await fetch(url, lowPriority ? { priority: "low" } : undefined);
    if (asset.ok) await asset.body?.pipeTo(new WritableStream());
  } catch { /* best effort: a failed asset is fetched again when the document opens */ }
}
