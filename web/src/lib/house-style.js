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

// Quarto's HTML page, as the paired app renders it, carries Bootstrap and the
// scripts that serve it: Bootstrap, Popper, Tippy, AnchorJS, clipboard.js, the
// tabset persistence and the polyfills. MathJax (its loader and typesetMath)
// is the page's own and stays, because the math is shown by it.
const QUARTO_SCRIPT = /<script\b([^>]*)>([\s\S]*?)<\/script>/gi;
const QUARTO_THEME_JS = /clipboard\.js v|@popperjs\/core|Bootstrap v\d|AnchorJS - v|Polyfill service|\btippy\b|quarto-html-after-body/;
const QUARTO_STYLE_LINK = /<link\b[^>]*\brel\s*=\s*["']?stylesheet[^>]*>/gi;
const QUARTO_THEME_CSS = /\.tippy-box|bootstrap-icons|--bs-|\.quarto-/;

function quartoThemeScript(attrs, body) {
  const data = /\bsrc\s*=\s*["']data:[^,]*;base64,([^"']{0,400})/i.exec(attrs);
  if (data) {
    try {
      return /tabsets/.test(atob(data[1].slice(0, data[1].length - (data[1].length % 4))));
    } catch {
      return false;
    }
  }
  return QUARTO_THEME_JS.test(attrs) || QUARTO_THEME_JS.test(body.slice(0, 600));
}

/**
 * Quarto's page, as the paired app renders it, shown in the house style: the
 * Bootstrap theme and the scripts that only serve it are removed, the house
 * stylesheet takes the place of the first of them, and the root takes the
 * class `lp-quarto` for the Quarto adapter rules. Only the preview takes this
 * copy; the stored page and exports keep the render as it came.
 * @param {string} html
 * @param {string} css the house stylesheet
 * @returns {string}
 */
export function quartoWithHouseStyle(html, css) {
  if (!css) return html;
  const stripped = html
    .replace(QUARTO_SCRIPT, (script, attrs, body) => (quartoThemeScript(attrs, body) ? "" : script))
    .replace(QUARTO_STYLE_LINK, (link) => (/data:text\/css|bootstrap|site_libs/i.test(link) ? "" : link));
  const out = swap(stripped, css, (style) => QUARTO_THEME_CSS.test(style));
  return out.replace(/<html\b([^>]*)>/i, (tag, attrs) => {
    const cls = /\bclass\s*=\s*(["'])([\s\S]*?)\1/i.exec(attrs);
    if (!cls) return `<html${attrs} class="lp-quarto">`;
    return `<html${attrs.replace(cls[0], `class=${cls[1]}${cls[2]} lp-quarto${cls[1]}`)}>`;
  });
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
