// Shared HTML handling for saved result content and the Quarto renderer.

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[char]));
}

function safeFragment(fragment) {
  const text = String(fragment ?? "");
  const plain = () => `<pre class="quarto-output-text"><code>${escapeHtml(text)}</code></pre>`;
  // Parse HTML before applying the allowlist. If no inert HTML parser exists
  // (for example a CLI-side check), preserve the fragment as literal text.
  if (typeof globalThis.DOMParser !== "function") return plain();
  const parsed = new DOMParser().parseFromString(text, "text/html");
  const allowed = new Set(["div", "span", "aside", "figure", "figcaption", "table", "thead", "tbody", "tfoot", "tr", "th", "td", "caption", "p", "pre", "code", "a", "img", "details", "summary", "br", "em", "strong", "b", "i", "small", "section"]);
  const blocked = new Set(["script", "style", "iframe", "object", "embed", "svg", "math", "template", "form", "input", "button", "meta", "link", "base"]);
  const attrs = new Set(["class", "id", "title", "alt", "width", "height", "colspan", "rowspan", "open", "aria-label", "role"]);
  const safeUrl = (value, image) => {
    const url = String(value).trim();
    if (/[\u0000-\u0020\u007f\\]/.test(url) || url.startsWith("//")) return "";
    if (/^data:/i.test(url)) return image && /^data:image\/(?:png|gif|jpe?g|webp);base64,/i.test(url) ? url : "";
    return /^(?:https?:|mailto:|#|\/|blob:)/i.test(url) ? url : "";
  };
  const copy = (node, target) => {
    if (node.nodeType === 3) { target.append(parsed.createTextNode(node.textContent)); return; }
    if (node.nodeType !== 1 || node.namespaceURI !== "http://www.w3.org/1999/xhtml") return;
    const name = node.localName;
    if (blocked.has(name)) return;
    if (!allowed.has(name)) { for (const child of node.childNodes) copy(child, target); return; }
    const clean = parsed.createElement(name);
    for (const attr of node.attributes) {
      if (attrs.has(attr.name)) clean.setAttribute(attr.name, attr.value);
      else if ((name === "a" && attr.name === "href") || (name === "img" && attr.name === "src")) {
        const url = safeUrl(attr.value, name === "img");
        if (url) clean.setAttribute(attr.name, url);
      }
    }
    for (const child of node.childNodes) copy(child, clean);
    target.append(clean);
  };
  const result = parsed.createElement("div");
  for (const node of parsed.body.childNodes) copy(node, result);
  return result.innerHTML;
}

export { escapeHtml, safeFragment };
