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
    value = value.slice(url[0].length);
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
    try { return await response.json(); } catch { throw fail("unreachable"); }
  } finally {
    clearTimeout(timer);
  }
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

// The cleaned text of a CSL value, or "" when it is empty.
function clean(value) {
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

function pagesOf(value) {
  return value.replace(/–/g, "--").replace(/(?<!-)-(?!-)/g, "--");
}

/** The issued year as a decimal string, or "" when the record has none. @param {any} csl */
export function cslYear(csl) {
  const value = csl?.issued?.["date-parts"]?.[0]?.[0];
  if (Number.isInteger(value)) return String(value);
  return typeof value === "string" && /^\d+$/.test(value) ? value : "";
}

/**
 * @param {any} csl
 * @param {string} doi the normalised DOI the person typed
 * @returns {{ key: string, bibtex: string }}
 */
export function cslToBibtex(csl, doi) {
  csl = csl || {};
  const type = TYPES[clean(csl.type)] || "misc";
  const authors = (Array.isArray(csl.author) ? csl.author : []).map((author) => {
    const family = clean(author?.family), given = clean(author?.given), literal = clean(author?.literal);
    return { family, given, literal };
  }).filter((author) => author.family || author.literal);
  const authorText = authors.map((author) => author.family
    ? (author.given ? `${author.family}, ${author.given}` : author.family)
    : `{${author.literal}}`).join(" and ");
  const year = cslYear(csl);
  const title = clean(csl.title);
  const container = clean(csl["container-title"]);
  const page = clean(csl.page);
  const url = clean(csl.URL);
  const fields = [
    ["author", authorText],
    ["title", title],
    ["year", year],
    ["journal", type === "article" ? container : ""],
    ["booktitle", type === "incollection" || type === "inproceedings" ? container : ""],
    ["volume", clean(csl.volume)],
    ["number", clean(csl.issue)],
    ["pages", page ? pagesOf(page) : ""],
    ["publisher", clean(csl.publisher)],
    ["address", clean(csl["publisher-place"])],
    ["doi", clean(csl.DOI) || doi],
    ["isbn", clean(csl.ISBN)],
    ["issn", clean(csl.ISSN)],
    ["url", /^[a-z][a-z0-9+.-]*:\/\/(?:dx\.)?doi\.org(?:[:/?#]|$)/i.test(url) ? "" : url],
  ].filter(([, value]) => value);

  let stem = authors.slice(0, 5).map((author) => fragment(author.family || author.literal)).filter(Boolean).join("");
  if (!stem) {
    stem = title.split(/[^\p{L}]+/u).filter((word) => word && !STOP_WORDS.has(word.toLowerCase())).slice(0, 3).map(fragment).join("");
  }
  const key = (stem || "Ref") + (year || "ND");
  const body = fields.map(([name, value]) => `  ${name} = {${value}}`).join(",\n");
  return { key, bibtex: `@${type}{${key},\n${body}${body ? "\n" : ""}}\n` };
}
