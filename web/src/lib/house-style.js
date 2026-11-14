// The house style: one stylesheet for every HTML document LibrePaper shows.
// The app holds the only copy (house-css.js). Markdown, Quarto, Org and
// bibliography pages from the wasm renderers get it put in here, as the first
// stylesheet of the head. LaTeXML and Typst HTML come from engines with their
// own stylesheets, which are swapped for the house one, before the page is
// painted.

import HOUSE_CSS from "./house-css.js";

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

const HOUSE_MARK = /<style\b[^>]*\bdata-lp-house\b[^>]*>/i;
const FIRST_SHEET = /<style\b|<link\b[^>]*\brel\s*=\s*["']?stylesheet/i;

/**
 * A page from one of the wasm renderers with the house stylesheet as the first
 * stylesheet of its head, so the renderer's own styles come after it. A page
 * that has it is returned as it is; an old module's embedded copy (a style
 * that defines `--lp-ink`) is removed first.
 * @param {string} html
 * @param {string} [css] the house stylesheet; the bundled one by default
 * @returns {string}
 */
export function withHouseStyle(html, css = HOUSE_CSS) {
  if (!css || typeof html !== "string" || HOUSE_MARK.test(html)) return html;
  const page = html.replace(STYLE, (style) => (style.includes("--lp-ink") ? "" : style));
  const tag = houseTag(css);
  const open = /<head\b[^>]*>/i.exec(page);
  if (!open) {
    const root = /<html\b[^>]*>/i.exec(page);
    if (!root) return `<head>${tag}</head>${page}`;
    const at = root.index + root[0].length;
    return `${page.slice(0, at)}<head>${tag}</head>${page.slice(at)}`;
  }
  const from = open.index + open[0].length;
  const close = /<\/head>/i.exec(page.slice(from));
  const end = close ? from + close.index : page.length;
  const sheet = FIRST_SHEET.exec(page.slice(from, end));
  const at = sheet ? from + sheet.index : end;
  return page.slice(0, at) + tag + page.slice(at);
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

// Quarto's page put in the house order and wording: the contents follow the
// title block (Quarto lays them out as a sidebar, which the house page has
// not), a caption's "Figure 1: " becomes the house caption label, and the
// contents' default title becomes the offline renderer's. Appendix headings
// need no rewrite: Quarto numbers them with the letter alone (`A`), as the
// offline renderer does.
const HOUSE_TOC_TITLE = "Contents";
function quartoMarkup(html) {
  let out = html;
  const toc = /<nav\b[^>]*\bid="TOC"[^>]*>[\s\S]*?<\/nav>/i.exec(out);
  const header = /<header\b[^>]*\bid="title-block-header"[^>]*>[\s\S]*?<\/header>/i.exec(out);
  if (toc && header && toc.index < header.index) {
    out = out.slice(0, toc.index) + out.slice(toc.index + toc[0].length);
    const at = out.indexOf(header[0]) + header[0].length;
    out = out.slice(0, at) + toc[0] + out.slice(at);
  }
  // Only Quarto's own English default: a title the author set is kept.
  out = out.replace(
    /(<h2\b[^>]*\bid="toc-title"[^>]*>)\s*Table of contents\s*(<\/h2>)/i,
    (whole, open, close) => `${open}${HOUSE_TOC_TITLE}${close}`,
  );
  return out.replace(
    /(<figcaption\b[^>]*\bquarto-float-caption\b[^>]*>)\s*([\p{L}][\p{L} .]*?[\u00a0 ]\d+(?:\.\d+)*)\s*[:.]?\s*/gu,
    (whole, open, label) => `${open}<span class="caption-label">${label.replace(/\u00a0/g, " ")}.</span> `,
  );
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
  const out = quartoMarkup(swap(stripped, css, (style) => QUARTO_THEME_CSS.test(style)));
  return out.replace(/<html\b([^>]*)>/i, (tag, attrs) => {
    const cls = /\bclass\s*=\s*(["'])([\s\S]*?)\1/i.exec(attrs);
    if (!cls) return `<html${attrs} class="lp-quarto">`;
    return `<html${attrs.replace(cls[0], `class=${cls[1]}${cls[2]} lp-quarto${cls[1]}`)}>`;
  });
}

/**
 * The house stylesheet, as bundled with the app.
 * @returns {Promise<string>}
 */
export async function houseStyleCss() {
  return HOUSE_CSS;
}
