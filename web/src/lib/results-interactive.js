// Only mount this HTML in a sandbox="allow-scripts" iframe. It deliberately
// has no editor bridge and never receives pairing credentials or API URLs.
import { resultsArtifactReference } from "./results-artifact.js";

function encoded(bytes, mime) {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 8192) binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return `data:${mime};base64,${btoa(binary)}`;
}

export function artifactPages(manifest, artifact, resources) {
  const main = manifest.artifact.entrypoint;
  const files = new Map(resources);
  files.set(main, artifact);
  const descriptors = new Map(manifest.assets.map(asset => [asset.path, asset]));
  const pages = [main, ...manifest.assets.filter(asset => asset.mime === "text/html" && asset.path !== main).map(asset => asset.path)];
  function page(path, interactive = false) {
    if (!pages.includes(path)) throw new Error("This page is not in the saved artifact.");
    const active = new Set();
    const memo = new Map();
    function url(value, parent) {
      if (value.startsWith("#")) return value;
      const ref = resultsArtifactReference(value, parent);
      if (!ref || !files.has(ref.path) || pages.includes(ref.path)) return "about:blank";
      if (memo.has(ref.path)) return memo.get(ref.path) + ref.fragment;
      if (active.has(ref.path)) return "about:blank";
      active.add(ref.path);
      const mime = descriptors.get(ref.path)?.mime || "application/octet-stream";
      const content = mime === "text/css" ? new TextEncoder().encode(css(new TextDecoder().decode(files.get(ref.path)), ref.path)) : files.get(ref.path);
      const result = encoded(content, mime);
      active.delete(ref.path);
      memo.set(ref.path, result);
      return result + ref.fragment;
    }
    function css(text, parent) {
      return text.replace(/(@import\s+)(['"])([^'"]+)\2/gi, (_, p, q, value) => `${p}${q}${url(value, parent)}${q}`)
        .replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)'"\s][^)]*))\s*\)/gi, (_, a, b, c) => `url("${url(a ?? b ?? c.trim(), parent)}")`);
    }
    const doc = new DOMParser().parseFromString(new TextDecoder().decode(files.get(path)), "text/html");
    for (const node of doc.querySelectorAll("base, meta[http-equiv], iframe, frame, frameset, object, embed, portal, form, link[rel=import]")) node.remove();
    for (const node of doc.querySelectorAll("script")) {
      // The preview reload client belongs to the local watcher, never a saved artifact.
      if (!interactive || /quarto-preview|quarto-dev|livereload/i.test(node.getAttribute("src") || "")) node.remove();
    }
    for (const node of doc.querySelectorAll("*")) {
      for (const attr of [...node.attributes]) {
        if (/^on/i.test(attr.name) || ["srcdoc", "integrity", "crossorigin", "nonce"].includes(attr.name)) node.removeAttribute(attr.name);
      }
      for (const name of ["src", "href", "xlink:href", "poster", "action", "formaction"]) {
        if (node.hasAttribute(name)) node.setAttribute(name, url(node.getAttribute(name), path));
      }
      node.removeAttribute("srcset");
      if (node.hasAttribute("style")) node.setAttribute("style", css(node.getAttribute("style"), path));
      if (node.localName === "style") node.textContent = css(node.textContent, path);
    }
    const policy = doc.createElement("meta");
    policy.httpEquiv = "Content-Security-Policy";
    policy.content = `default-src 'none'; script-src ${interactive ? "'unsafe-inline' data:" : "'none'"}; style-src 'unsafe-inline' data:; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`;
    doc.head.prepend(policy);
    return `<!doctype html>\n${doc.documentElement.outerHTML}`;
  }
  return { pages, page };
}
