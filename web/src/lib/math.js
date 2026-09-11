// Math in a markdown document, typeset where it is read.
//
// The markdown renderer keeps `$…$` and `$$…$$` as TeX, escaped inside a span
// tagged with its style, and does nothing more: it is a WebAssembly module
// that knows nothing about where a KaTeX might live. The agent inside the
// document frame does, and this is what it does with those spans. KaTeX is
// fetched from the document's own origin the first time a document with math
// arrives, and never for one without.
//
// Written against a `document` and a `window` passed in rather than the
// globals, so the checks can drive it with a stand-in for either.

/// The spans the renderer left, that no typesetting has touched yet: a span
/// that has been rendered holds KaTeX's markup rather than its TeX.
export function untypeset(root) {
  return [...root.querySelectorAll("span[data-math-style]")].filter(
    (span) => !span.firstElementChild,
  );
}

/// Renders every span in place. `throwOnError: false` because a formula with
/// a typo in it is the author's to see, in red, rather than a reason to show
/// nothing; `output: "html"` because the alternative writes MathML beside the
/// HTML, and the text of a passage is what comments are anchored into -- a
/// formula that appears twice in that text would be a passage that reads
/// differently from how it looks.
export function typeset(katex, spans) {
  for (const span of spans) {
    span.setAttribute?.("data-equation-source", span.textContent);
    katex.render(span.textContent, span, {
      displayMode: span.dataset.mathStyle === "display",
      throwOnError: false,
      output: "html",
    });
  }
}

/**
 * The typesetter the agent calls after every repaint. `base` is where KaTeX
 * is served, ending in a slash. Returns whether the document was typeset
 * before returning: it is when KaTeX is already here, and otherwise the
 * rendering lands once the script has loaded, which is a mutation the
 * agent's observer republishes like any other.
 */
export function createMathTypesetter({ base, document, window }) {
  let loading = null;
  let styled = false;

  function load() {
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
  }

  return function typesetMath() {
    const spans = untypeset(document.body);
    if (spans.length === 0) return false;
    if (window.katex) {
      typeset(window.katex, spans);
      return true;
    }
    // The spans may have been replaced by the time the script arrives, so
    // they are found again rather than kept.
    load().then(
      (katex) => typeset(katex, untypeset(document.body)),
      () => {},
    );
    return false;
  };
}
