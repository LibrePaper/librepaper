// DOI lookup: turns what a person typed into a DOI, fetches its CSL-JSON from
// doi.org and writes it as BibTeX. The Rust twin in
// crates/librepaper-companion/src/automation/doi.rs must produce byte-identical
// output; both are tested against tests/fixtures/doi-bibtex.json. This file
// imports no messages, so it works anywhere.

/** @param {string} text @returns {string | null} */
export function normaliseDoi(text) {
  let value = String(text ?? "").trim();
  const url = /^https?:\/\/(?:dx\.)?doi\.org\//i.exec(value);
  if (url) {
    value = value.slice(url[0].length).split(/[?#]/)[0];
    try { value = decodeURIComponent(value); } catch { return null; }
  } else {
    value = value.replace(/^doi:\s*/i, "");
  }
  return /^10\.\d{4,9}\/\S+$/.test(value) ? value : null;
}

/**
 * Fetches the CSL-JSON record of a DOI. Errors carry `.code`: "not_found" for
 * an HTTP 404, "unreachable" for anything else, a 10 s timeout included.
 * @param {string} doi
 * @param {typeof fetch} [fetchImpl]
 */
export async function fetchDoi(doi, fetchImpl = fetch) {
  const fail = (code) => Object.assign(new Error(code), { code });
  const path = doi.split("/").map(encodeURIComponent).join("/");
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10000);
  try {
    let response;
    try {
      response = await fetchImpl(`https://doi.org/${path}`, {
        headers: { Accept: "application/vnd.citationstyles.csl+json" },
        redirect: "follow",
        signal: controller.signal,
      });
    } catch { throw fail("unreachable"); }
    if (response.status === 404) throw fail("not_found");
    if (!response.ok) throw fail("unreachable");
    let body;
    try { body = await response.json(); } catch { throw fail("unreachable"); }
    if (!usableCsl(body)) throw fail("not_found");
    return body;
  } finally {
    clearTimeout(timer);
  }
}

/** Whether a parsed response is a CSL record: an object with a non-empty type and title. @param {any} csl */
export function usableCsl(csl) {
  if (!csl || typeof csl !== "object" || Array.isArray(csl)) return false;
  const filled = (value) => typeof value === "string" && value !== "";
  return filled(csl.type) && filled(Array.isArray(csl.title) ? csl.title[0] : csl.title);
}

const TYPES = {
  "article-journal": "article",
  "journal-article": "article",
  book: "book",
  monograph: "book",
  "edited-book": "book",
  "reference-book": "book",
  chapter: "incollection",
  "book-chapter": "incollection",
  "paper-conference": "inproceedings",
  "proceedings-article": "inproceedings",
  thesis: "phdthesis",
  dissertation: "phdthesis",
  report: "techreport",
  webpage: "online",
};
const STOP_WORDS = new Set("a an and as at by for from in of on or the to with".split(" "));

function first(value) {
  return Array.isArray(value) ? value[0] : value;
}

/** The cleaned text of a CSL value (tags stripped, entities decoded, braces removed), or "" when empty. */
export function cslText(value) {
  value = first(value);
  if (value === undefined || value === null || typeof value === "object") return "";
  return String(value)
    .replace(/<[^>]*>/g, "")
    .replace(/&(amp|lt|gt|quot|#39);/g, (_, name) => ({ amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'" })[name])
    .replace(/[{}]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function fragment(text) {
  const letters = String(text).normalize("NFKD").replace(/\p{M}/gu, "").replace(/[^A-Za-z]/g, "").slice(0, 3).toLowerCase();
  return letters.charAt(0).toUpperCase() + letters.slice(1);
}

// Escapes TeX specials for a written field value; the key uses the unescaped text.
function tex(text) {
  return text.replace(/\\/g, "").replace(/[&%#_$]/g, (char) => `\\${char}`).replace(/\^/g, "\\^{}");
}

function pagesOf(value) {
  return value.replace(/–/g, "--").replace(/(?<!-)-(?!-)/g, "--");
}

/** The issued year as a decimal string, or "" when the record has none. @param {any} csl */
export function cslYear(csl) {
  const value = csl?.issued?.["date-parts"]?.[0]?.[0];
  if (Number.isInteger(value) && value >= 0) return String(value);
  return typeof value === "string" && /^\d+$/.test(value) ? value : "";
}

/**
 * @param {any} csl
 * @param {string} doi the normalised DOI the person typed
 * @returns {{ key: string, bibtex: string }}
 */
export function cslToBibtex(csl, doi) {
  csl = csl || {};
  const type = TYPES[cslText(csl.type)] || "misc";
  const authors = (Array.isArray(csl.author) ? csl.author : []).map((author) => {
    const family = cslText(author?.family), given = cslText(author?.given), literal = cslText(author?.literal);
    return { family, given, literal };
  }).filter((author) => author.family || author.literal);
  const authorText = authors.map((author) => author.family
    ? (!author.given && /\s/.test(author.family) ? `{${tex(author.family)}}` : author.given ? `${tex(author.family)}, ${tex(author.given)}` : tex(author.family))
    : `{${tex(author.literal)}}`).join(" and ");
  const year = cslYear(csl);
  const title = cslText(csl.title);
  const container = cslText(csl["container-title"]);
  const page = cslText(csl.page);
  const url = cslText(csl.URL);
  const fields = [
    ["author", authorText],
    ["title", title],
    ["year", year],
    ["journal", type === "article" ? container : ""],
    ["booktitle", type === "incollection" || type === "inproceedings" ? container : ""],
    ["volume", cslText(csl.volume)],
    ["number", cslText(csl.issue)],
    ["pages", page ? pagesOf(page) : ""],
    ["publisher", cslText(csl.publisher)],
    ["address", cslText(csl["publisher-place"])],
    ["doi", cslText(csl.DOI) || doi],
    ["isbn", cslText(csl.ISBN)],
    ["issn", cslText(csl.ISSN)],
    ["url", /^[a-z][a-z0-9+.-]*:\/\/(?:dx\.)?doi\.org(?:[:/?#]|$)/i.test(url) ? "" : url],
  ].filter(([, value]) => value);

  let stem = authors.slice(0, 5).map((author) => fragment(author.family || author.literal)).filter(Boolean).join("");
  if (!stem) {
    stem = title.split(/[^\p{L}]+/u).filter((word) => word && !STOP_WORDS.has(word.toLowerCase())).map(fragment).filter(Boolean).slice(0, 3).join("");
  }
  const key = (stem || "Ref") + (year || "ND");
  const body = fields.map(([name, value]) => `  ${name} = {${name === "doi" || name === "url" ? value : tex(value)}}`).join(",\n");
  return { key, bibtex: `@${type}{${key},\n${body}${body ? "\n" : ""}}\n` };
}
