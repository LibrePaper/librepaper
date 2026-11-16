/**
 * Mermaid diagrams in the preview. Mermaid is loaded from `base` the first
 * time a document has one, and each diagram is drawn once per theme and
 * source, the SVG kept in a small cache so repaints do not draw it again.
 */

/**
 * Loads mermaid.min.js from `base`, ending in a slash, once. Returns a
 * function that resolves to `window.mermaid`. A failed script load is
 * forgotten, so the next call tries again.
 */
export function createMermaidLoader({ base, document, window }) {
  let loading = null;

  return function load() {
    if (window.mermaid) return Promise.resolve(window.mermaid);
    if (loading) return loading;
    loading = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = `${base}mermaid.min.js`;
      script.onload = () => resolve(window.mermaid);
      script.onerror = () => {
        // So that the next document with a diagram tries again.
        loading = null;
        reject(new Error(`Mermaid did not load from ${script.src}`));
      };
      document.head.appendChild(script);
    });
    return loading;
  };
}

/**
 * The `<pre>` elements to draw, in document order: `pre.mermaid` (Quarto Run
 * output) and the `<pre>` around each `code.language-mermaid` (the Markdown
 * and Quarto draft).
 */
export function findDiagrams(root) {
  const found = new Set();
  for (const node of root.querySelectorAll("pre.mermaid, code.language-mermaid")) {
    if (node.tagName === "PRE") found.add(node);
    else if (node.parentElement?.tagName === "PRE") found.add(node.parentElement);
  }
  return [...found];
}

/** Whether `pre` is still a diagram block: `pre.mermaid`, or a `<pre>` around `code.language-mermaid`. */
function isDiagram(pre) {
  return (pre.className || "").split(/\s+/).includes("mermaid") || Boolean(pre.querySelector("code.language-mermaid"));
}

/** The diagram's source: the code listing inside the `<pre>`, trailing space trimmed. */
export function diagramSource(pre) {
  const code = pre.querySelector("code.language-mermaid");
  return (code ? code.textContent : pre.textContent).trimEnd();
}

/** The value of a `%%| fig-cap:` option line, without surrounding quotes, or "". */
export function figureCaption(source) {
  const line = /^[ \t]*%%\|[ \t]*fig-cap:[ \t]*(.*?)[ \t\r]*$/m.exec(source);
  if (!line) return "";
  const quoted = /^(["'])(.*)\1$/.exec(line[1]);
  return quoted ? quoted[2] : line[1];
}

/**
 * A small least-recently-used cache of SVG strings. A read counts as a use;
 * past `limit` entries the oldest one is dropped.
 */
export function createDiagramCache(limit = 64) {
  const entries = new Map();
  return {
    get(key) {
      if (!entries.has(key)) return undefined;
      const svg = entries.get(key);
      entries.delete(key);
      entries.set(key, svg);
      return svg;
    },
    set(key, svg) {
      entries.delete(key);
      entries.set(key, svg);
      while (entries.size > limit) entries.delete(entries.keys().next().value);
    },
  };
}

/**
 * Draws the diagrams of a page. `applyCached(root)` swaps in the diagrams
 * already drawn and returns how many are still undrawn. `render()` swaps in
 * what it can synchronously, and when some are left it loads Mermaid and
 * draws them one at a time in the background, returning false.
 * `theme()` returns "dark" or "default".
 */
export function createDiagramRenderer({ document, window, load, cache, theme }) {
  let counter = 0;
  let initialized = null;
  let chain = Promise.resolve();

  const keyFor = (source, palette = theme()) => `${palette}\u0000${source}`;

  // The source and theme are kept on the figure so retheme() can turn it back
  // into a listing when the paper changes.
  function holder(svg, source, pre, palette = theme()) {
    const owner = pre.ownerDocument || document;
    const figure = owner.createElement("figure");
    figure.setAttribute("class", "lp-mermaid");
    figure.setAttribute("data-lp-mermaid", "");
    figure.setAttribute("data-lp-mermaid-source", source);
    figure.setAttribute("data-lp-mermaid-theme", palette);
    figure.innerHTML = svg;
    const caption = figureCaption(source);
    if (caption) {
      const figcaption = owner.createElement("figcaption");
      // Text, never markup: the caption comes from the document's source.
      figcaption.textContent = caption;
      figure.appendChild(figcaption);
    }
    return figure;
  }

  function applyCached(root) {
    let undrawn = 0;
    for (const pre of findDiagrams(root)) {
      const source = diagramSource(pre);
      const svg = cache.get(keyFor(source));
      if (!svg) {
        undrawn += 1;
        continue;
      }
      pre.replaceWith(holder(svg, source, pre));
    }
    return undrawn;
  }

  async function drawUndrawn() {
    const mermaid = window.mermaid;
    if (!mermaid) return;
    const wanted = theme();
    if (initialized !== wanted) {
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: wanted });
      initialized = wanted;
    }
    // Sources tried in this call, so an invalid one is not tried again and again.
    const attempted = new Set();
    // Each pass finds the diagrams again, since the page may have changed while
    // the script loaded or a render ran. Another pass follows only a swap; a
    // listing whose source moved on meanwhile is picked up by that pass or by
    // the next render().
    let replaced = true;
    while (replaced) {
      replaced = false;
      for (const pre of findDiagrams(document.body)) {
        // The paper changed: this pass's palette is stale, and retheme() has
        // queued a pass in the new one.
        if (theme() !== wanted) return;
        const source = diagramSource(pre);
        const key = keyFor(source, wanted);
        const cached = cache.get(key);
        if (cached) {
          pre.replaceWith(holder(cached, source, pre, wanted));
          continue;
        }
        if (attempted.has(key)) continue;
        attempted.add(key);
        // A half-typed diagram is not an error: its listing stays as it is.
        let parsed = false;
        try {
          parsed = await mermaid.parse(source, { suppressErrors: true });
        } catch {
          parsed = false;
        }
        if (!parsed) continue;
        const id = `lp-mermaid-${counter++}`;
        try {
          const { svg } = await mermaid.render(id, source);
          cache.set(key, svg);
          // The same <pre> may have been morphed to a newer source or another
          // language, or the paper changed, while this was drawing; a stale
          // drawing is not swapped in.
          if (pre.isConnected && isDiagram(pre) && diagramSource(pre) === source && theme() === wanted) {
            pre.replaceWith(holder(svg, source, pre, wanted));
            replaced = true;
          }
        } catch {
          // A failed render can leave its scratch elements behind.
          document.getElementById(id)?.remove();
          document.getElementById(`d${id}`)?.remove();
        }
      }
    }
  }

  // Turns drawn diagrams whose theme is no longer the current one back into
  // listings, and draws them again in the current palette.
  function retheme() {
    const wanted = theme();
    for (const figure of document.body.querySelectorAll("figure.lp-mermaid")) {
      const source = figure.getAttribute("data-lp-mermaid-source");
      if (source === null || figure.getAttribute("data-lp-mermaid-theme") === wanted) continue;
      const pre = document.createElement("pre");
      pre.setAttribute("class", "mermaid");
      pre.textContent = source;
      figure.replaceWith(pre);
    }
    return render();
  }

  function render() {
    if (applyCached(document.body) === 0) return true;
    let ready;
    try {
      ready = Promise.resolve(load());
    } catch (error) {
      ready = Promise.reject(error);
    }
    // One chain, so only one mermaid.render runs at a time.
    chain = chain
      .then(() => ready)
      .catch(() => {})
      .then(drawUndrawn)
      .catch(() => {});
    return false;
  }

  return { applyCached, render, retheme };
}
