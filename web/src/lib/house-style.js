// The house style: one stylesheet for every HTML document LibrePaper shows.
// Markdown, Quarto and Org pages carry it already (the wasm renderers embed
// it). LaTeXML and Typst HTML come from engines with their own stylesheets,
// which are swapped for the house one here, before the page is painted.

import { loadPaperRenderer } from "./paper/renderer.js";

const STYLE = /<style\b[^>]*>[\s\S]*?<\/style>/gi;
const LATEXML_STYLE = /<style\b[^>]*\bdata-latexml-css\b[^>]*>/i;
const MATH_RULES = /\bmath\b|\bm(?:row|i|n|o|frac|sup|sub|root|sqrt|table|tr|td)\b/i;

function houseTag(css) {
  return `<style data-lp-house>${css.replace(/<\/style/gi, "<\\/style")}</style>`;
}

// Removes the styles `drop` names, puts the house style where the first one
// stood (or at the end of the head, or the start of the page), and keeps the
// rest.
function swap(html, css, drop) {
  if (!css) return html;
  let placed = false;
  const tag = houseTag(css);
  const out = html.replace(STYLE, (style) => {
    if (!drop(style)) return style;
    if (placed) return "";
    placed = true;
    return tag;
  });
  if (placed) return out;
  if (/<\/head>/i.test(out)) return out.replace(/<\/head>/i, () => `${tag}</head>`);
  return tag + out;
}

/**
 * LaTeXML's page with the house stylesheet in place of its own
 * (`<style data-latexml-css>` elements).
 * @param {string} html
 * @param {string} css the house stylesheet
 * @returns {string}
 */
export function latexmlWithHouseStyle(html, css) {
  return swap(html, css, (style) => LATEXML_STYLE.test(style));
}

/**
 * Typst's HTML page with the house stylesheet in place of its own rules. The
 * styles that carry MathML rules are Typst's, and stay.
 * @param {string} html
 * @param {string} css the house stylesheet
 * @returns {string}
 */
export function typstWithHouseStyle(html, css) {
  return swap(html, css, (style) => !MATH_RULES.test(style));
}

/**
 * The house stylesheet as the Markdown module embeds it, or "" when this
 * deployment has no Markdown module (the page then keeps its own styles).
 * @returns {Promise<string>}
 */
export async function houseStyleCss() {
  try {
    return (await loadPaperRenderer("markdown")).pageCss || "";
  } catch {
    return "";
  }
}
