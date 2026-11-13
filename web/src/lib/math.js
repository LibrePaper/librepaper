// Math in a markdown document, typeset where it is read.
//
// The markdown renderer keeps `$…$` and `$$…$$` as TeX, escaped inside a span
// tagged with its style, and does nothing more: it is a WebAssembly module
// that knows nothing about where a KaTeX might live. The agent inside the
// document frame does, and this is what it does with those spans. KaTeX is
// fetched from the document's own origin the first time a document with math
// arrives, and never for one without.
//
import { numberDisplays, parseReference } from "./math-numbering.js";

// Written against a `document` and a `window` passed in rather than the
// globals, so the checks can drive it with a stand-in for either.

/// The spans the renderer left, that no typesetting has touched yet: a span
/// that has been rendered holds KaTeX's markup rather than its TeX.
export function untypeset(root) {
  return [...root.querySelectorAll("span[data-math-style]")].filter(
    (span) => !span.firstElementChild,
  );
}

// Quarto's self-contained HTML normally asks MathJax to replace these spans
// at page load. Live preview HTML is inserted with innerHTML, deliberately
// without executing its scripts, so translate the inert TeX spans into the
// same marker the browser Markdown renderer uses and let KaTeX handle them.
export function prepareQuartoMath(root) {
  let prepared = 0;
  for (const span of root.querySelectorAll("span.math:not([data-math-style])")) {
    if (span.firstElementChild) continue;
    let tex = String(span.textContent || "").trim();
    const display = span.classList?.contains("display") || tex.startsWith("\\[");
    const delimiters = display ? ["\\[", "\\]"] : ["\\(", "\\)"];
    if (tex.startsWith(delimiters[0]) && tex.endsWith(delimiters[1])) {
      tex = tex.slice(delimiters[0].length, -delimiters[1].length).trim();
    }
    span.dataset.mathStyle = display ? "display" : "inline";
    span.textContent = tex;
    prepared += 1;
  }
  return prepared;
}

/// Renders every span in place. `throwOnError: false` because a formula with
/// a typo in it is the author's to see, in red, rather than a reason to show
/// nothing; `output: "html"` because the alternative writes MathML beside the
/// HTML, and the text of a passage is what comments are anchored into -- a
/// formula that appears twice in that text would be a passage that reads
/// differently from how it looks.
export function typeset(katex, spans, numbering) {
  for (const span of spans) {
    const source = span.textContent;
    span.setAttribute?.("data-equation-source", source);
    const display = span.dataset.mathStyle === "display";
    if (!display && numbering && linkReference(span, source, numbering)) continue;
    katex.render(display ? (numbering?.tex.get(span) ?? source) : source, span, {
      displayMode: display,
      throwOnError: false,
      output: "html",
    });
  }
}

// An inline \ref{k} or \eqref{k} of a known label becomes a link to the
// equation. The child element keeps `untypeset` from taking the span again.
function linkReference(span, source, { labels, document }) {
  const reference = parseReference(source);
  const number = reference && labels.get(reference.key);
  if (!number) return false;
  const link = document.createElement("a");
  link.setAttribute("href", `#${reference.key}`);
  link.textContent = reference.kind === "eqref" ? `(${number})` : String(number);
  span.textContent = "";
  span.appendChild(link);
  return true;
}

/// The TeX of every display formula in order, typeset or not: a rendered one
/// keeps its TeX in `data-equation-source`, so numbers stay put on a repeat.
function displaySpans(root) {
  return [...root.querySelectorAll("span[data-math-style]")].filter(
    (span) => span.dataset.mathStyle === "display",
  );
}

/// The TeX of every display formula under `root`, in order: what numbering
/// is computed from.
export function displaySources(root) {
  return displaySpans(root).map(
    (span) => (span.firstElementChild && span.getAttribute?.("data-equation-source")) || span.textContent,
  );
}

/// Gives the display formulas under `root` their numbers. `tex[i]` and `keys[i]`
/// belong to the i-th display span, as `numberDisplays(displaySources(root))`
/// gives them; `labels` is the document-wide map from key to number. Each
/// labelled formula gets an id to link to (unless something else has it), and
/// `<a href="key">` links, as Org emits for [[key]], point at them. With
/// `ids: false` no id is set: the editor's paper blocks follow no links, and a
/// block drawn again would find its own outgoing copy holding the id. Returns
/// what `typeset` takes.
export function applyNumbering(root, document, { tex, keys, labels }, { ids = true } = {}) {
  const spans = displaySpans(root);
  if (ids) spans.forEach((span, i) => {
    const key = keys[i]?.[0];
    if (!key || !span.setAttribute) return;
    const holder = document.getElementById?.(key);
    if (!holder) span.setAttribute("id", key);
  });
  for (const link of root.querySelectorAll("a[href]")) {
    const href = link.getAttribute("href");
    if (!labels.has(href)) continue;
    link.setAttribute("href", `#${href}`);
    if (link.textContent === href) link.textContent = String(labels.get(href));
  }
  return { tex: new Map(spans.map((span, i) => [span, tex[i]])), labels, document };
}

/// Numbers the document's display formulas and applies the numbers to `root`.
export function numberDocument(root, document) {
  return applyNumbering(root, document, numberDisplays(displaySources(root)));
}

/**
 * Loads KaTeX from `base`, ending in a slash, once. Returns a function that
 * resolves to `window.katex`, adding the stylesheet and the script the first
 * time they are needed. A failed script load is forgotten, so the next call
 * tries again.
 */
export function createKatexLoader({ base, document, window }) {
  let loading = null;
  let styled = false;

  return function load() {
    if (window.katex) return Promise.resolve(window.katex);
    if (loading) return loading;
    if (!styled) {
      styled = true;
      const link = document.createElement("link");
      link.rel = "stylesheet";
      link.href = `${base}katex.min.css`;
      document.head.appendChild(link);
    }
    loading = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = `${base}katex.min.js`;
      script.onload = () => resolve(window.katex);
      script.onerror = () => {
        // So that the next document with math tries again rather than
        // waiting on a load that already failed.
        loading = null;
        reject(new Error(`KaTeX did not load from ${script.src}`));
      };
      document.head.appendChild(script);
    });
    return loading;
  };
}

/**
 * The typesetter the agent calls after every repaint. `base` is where KaTeX
 * is served, ending in a slash. Returns whether the document was typeset
 * before returning: it is when KaTeX is already here, and otherwise the
 * rendering lands once the script has loaded, which is a mutation the
 * agent's observer republishes like any other.
 */
export function createMathTypesetter({ base, document, window, load = createKatexLoader({ base, document, window }) }) {
  return function typesetMath() {
    prepareQuartoMath(document.body);
    const spans = untypeset(document.body);
    if (spans.length === 0) return false;
    if (window.katex) {
      typeset(window.katex, spans, numberDocument(document.body, document));
      return true;
    }
    // The spans may have been replaced by the time the script arrives, so
    // they are found again rather than kept.
    load().then(
      (katex) => typeset(katex, untypeset(document.body), numberDocument(document.body, document)),
      () => {},
    );
    return false;
  };
}
