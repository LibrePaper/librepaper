// Only dependencies actually referenced by the rendered page become public.
// Inputs used by a compiler (bibliographies, data, maps, source files) are
// deliberately absent from this display-type allowlist.
import { sha256 } from "./publication.js";

const TYPES = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif",
  webp: "image/webp", avif: "image/avif", svg: "image/svg+xml", ico: "image/x-icon",
  css: "text/css", js: "application/javascript", mjs: "application/javascript",
  woff: "font/woff", woff2: "font/woff2", ttf: "font/ttf", otf: "font/otf",
  mp4: "video/mp4", webm: "video/webm", mp3: "audio/mpeg", ogg: "audio/ogg",
};
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const external = /^(?:https?:|data:|mailto:|tel:|#|\/\/)/i;

// CSS strings and comments can contain examples such as `url(secret.png)`.
// They are text, not dependencies. Keep the small scanner here instead of a
// broad regex so the publication closure follows only CSS tokens that load a
// resource.
function cssReferences(css) {
  const references = [];
  const name = (character) => /[a-z0-9_-]/i.test(character || "");
  const space = (character) => /\s/.test(character || "");
  function skipTrivia(at) {
    while (at < css.length) {
      if (space(css[at])) { at += 1; continue; }
      if (css.startsWith("/*", at)) {
        const end = css.indexOf("*/", at + 2);
        if (end < 0) return css.length;
        at = end + 2;
        continue;
      }
      break;
    }
    return at;
  }
  function quoted(at) {
    const quote = css[at];
    let end = at + 1;
    while (end < css.length) {
      if (css[end] === "\\") { end += 2; continue; }
      if (css[end] === quote) return { start: at + 1, end, next: end + 1 };
      end += 1;
    }
    return null;
  }
  function url(at) {
    let open = skipTrivia(at + 3);
    if (css[open] !== "(") return null;
    let value = skipTrivia(open + 1);
    if (css[value] === "'" || css[value] === '"') {
      const token = quoted(value);
      if (!token) return null;
      const close = skipTrivia(token.next);
      return css[close] === ")" ? { ...token, next: close + 1 } : null;
    }
    const close = css.indexOf(")", value);
    if (close < 0) return null;
    let end = close;
    while (end > value && space(css[end - 1])) end -= 1;
    return { start: value, end, next: close + 1 };
  }

  for (let at = 0; at < css.length;) {
    if (css.startsWith("/*", at)) {
      const end = css.indexOf("*/", at + 2);
      at = end < 0 ? css.length : end + 2;
      continue;
    }
    if (css[at] === "'" || css[at] === '"') {
      const token = quoted(at);
      at = token ? token.next : css.length;
      continue;
    }
    if (css[at] === "@" && css.slice(at + 1, at + 7).toLowerCase() === "import" && !name(css[at + 7])) {
      const value = skipTrivia(at + 7);
      if (css[value] === "'" || css[value] === '"') {
        const token = quoted(value);
        if (token) {
          references.push({ start: token.start, end: token.end, reference: css.slice(token.start, token.end) });
          at = token.next;
          continue;
        }
      }
    }
    if (css.slice(at, at + 3).toLowerCase() === "url" && !name(css[at - 1]) && !name(css[at + 3])) {
      const token = url(at);
      if (token) {
        references.push({ start: token.start, end: token.end, reference: css.slice(token.start, token.end) });
        at = token.next;
        continue;
      }
    }
    at += 1;
  }
  return references;
}

export async function buildDisplayBundle(html, tree, { fetcher = globalThis.fetch, parse = (value) => new DOMParser().parseFromString(value, "text/html") } = {}) {
  const document = parse(html);
  // A base element can make our relative manifest paths resolve outside the
  // publication. Its effects are resolved against the captured main instead.
  if (document.querySelector("base")) throw new Error("Remove the HTML base element before publishing; display assets must use project-relative paths.");
  const objects = new Map();
  const resolving = new Set();
  const assets = new Map();
  const urlPaths = new Map(Object.entries(tree.urls || {}).map(([path, url]) => [url, path]));

  function resolvePath(reference, from) {
    if (urlPaths.has(reference)) return urlPaths.get(reference);
    const digest = reference.match(/#librepaper-asset=([a-f0-9]{64})$/)?.[1];
    if (digest) return Object.keys(tree.digests || {}).find((path) => tree.digests[path] === digest);
    if (reference.startsWith("blob:")) return reference;
    if (reference.startsWith("/") || /^[a-z][a-z0-9+.-]*:/i.test(reference)) throw new Error(`Unsupported display dependency: ${reference}`);
    const root = new URL(from, "https://publication.invalid/");
    const path = new URL(reference, root).pathname.slice(1);
    return decodeURIComponent(path);
  }

  async function rewriteCss(css, from, nested = false) {
    for (const token of cssReferences(css).reverse()) {
      const after = await dependency(token.reference.trim(), from, nested);
      css = css.slice(0, token.start) + after + css.slice(token.end);
    }
    if (/sourceMappingURL\s*=/.test(css)) throw new Error("Remove source map references from display styles before publishing.");
    return css;
  }

  async function dependency(reference, from, nested = false) {
    if (!reference || external.test(reference)) return reference;
    const suffix = reference.startsWith("blob:") ? "" : (reference.match(/[?#].*$/)?.[0] || "");
    const path = resolvePath(reference, from);
    if (!path) throw new Error(`Missing display dependency: ${reference}`);
    if (resolving.has(path)) throw new Error(`Circular display dependency: ${path}`);
    if (!objects.has(path)) {
      resolving.add(path);
      try {
        let bytes = tree.assets?.[path];
        if (!bytes && Object.hasOwn(tree.texts || {}, path)) bytes = encoder.encode(tree.texts[path]);
        let extension = path.split(".").pop().toLowerCase();
        let mime = TYPES[extension];
        if (path.startsWith("blob:")) {
          const response = await fetcher(path);
          if (!response.ok) throw new Error(`Missing generated display asset: ${reference}`);
          const blob = await response.blob();
          mime = blob.type.split(";", 1)[0];
          extension = Object.keys(TYPES).find((type) => TYPES[type] === mime);
          bytes = new Uint8Array(await blob.arrayBuffer());
        }
        if (!mime || !extension) throw new Error(`Unsupported public asset type: ${path}`);
        if (!bytes) throw new Error(`Missing display dependency: ${path}`);
        bytes = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
        if (mime === "text/css") bytes = encoder.encode(await rewriteCss(decoder.decode(bytes), path, true));
        if (mime === "application/javascript") {
          const script = decoder.decode(bytes);
          if (/sourceMappingURL\s*=/.test(script)) throw new Error("Remove source map references from display scripts before publishing.");
          // Module dependencies must be explicitly packaged; silently copying
          // a module with private imports would produce an incomplete page.
          if (/(?:\bfrom\s*|\bimport\s*\(?\s*)["']\.{1,2}\//.test(script)) throw new Error(`Bundle relative module imports before publishing: ${path}`);
        }
        const name = `${await sha256(bytes)}.${extension}`;
        const publicPath = `assets/${name}`;
        objects.set(path, name);
        assets.set(publicPath, { path: publicPath, bytes, mime });
      } finally { resolving.delete(path); }
    }
    return `${nested ? "" : "assets/"}${objects.get(path)}${suffix}`;
  }

  for (const element of document.querySelectorAll("[src],link[href],[poster],object[data],image[href],use[href]")) {
    for (const attribute of ["src", "href", "poster", "data"]) {
      if (!element.hasAttribute(attribute)) continue;
      element.setAttribute(attribute, await dependency(element.getAttribute(attribute), tree.main));
    }
    if (element.tagName === "IMG" && !element.hasAttribute("loading")) element.setAttribute("loading", "lazy");
  }
  for (const element of document.querySelectorAll("[srcset]")) {
    const value = element.getAttribute("srcset");
    if (value.includes("data:")) continue;
    const entries = [];
    for (const entry of value.split(",")) {
      const [url, ...descriptor] = entry.trim().split(/\s+/);
      entries.push([await dependency(url, tree.main), ...descriptor].join(" "));
    }
    element.setAttribute("srcset", entries.join(", "));
  }
  for (const element of document.querySelectorAll("style")) element.textContent = await rewriteCss(element.textContent, tree.main);
  for (const element of document.querySelectorAll("[style]")) element.setAttribute("style", await rewriteCss(element.getAttribute("style"), tree.main));
  for (const script of document.querySelectorAll("script:not([src])")) {
    if (/sourceMappingURL\s*=/.test(script.textContent)) throw new Error("Remove embedded source map references before publishing.");
    // Inline modules are part of the page too.  A relative import here has
    // the same unpublished dependency problem as one in a copied .js file.
    if (/(?:\bfrom\s*|\bimport\s*\(?\s*)["']\.{1,2}\//.test(script.textContent)) {
      throw new Error("Bundle relative module imports before publishing: inline script");
    }
  }
  return { html: `<!doctype html>\n${document.documentElement.outerHTML}`, assets: [...assets.values()] };
}
