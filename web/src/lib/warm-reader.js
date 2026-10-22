// The document page is a separate navigation, so its scripts and wasm start
// cold. This fetches them into the HTTP cache while the user is on the
// project list, so opening a document skips the download. The wasm still
// compiles when the document page loads.
import loroWasm from "loro-crdt/web/loro_wasm_bg.wasm?url";

let started = false;

export function warmReader() {
  if (started) return;
  started = true;
  // Network Information is Chromium-only and absent from the DOM typings.
  const connection = /** @type {{ saveData?: boolean, effectiveType?: string } | undefined} */ (
    /** @type {any} */ (navigator).connection);
  if (connection?.saveData || connection?.effectiveType?.includes("2g")) return;
  const run = () => { void prefetch(); };
  if (typeof requestIdleCallback === "function") requestIdleCallback(run, { timeout: 5000 });
  else setTimeout(run, 2000);
}

async function prefetch() {
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
        try {
          const asset = await fetch(url, { priority: "low" });
          await asset.arrayBuffer();
        } catch { /* best effort: a failed asset is fetched again when the document opens */ }
      }
    };
    await Promise.all(Array.from({ length: Math.min(3, queue.length) }, drain));
  } catch { /* best effort: the document page works without the warm cache */ }
}
