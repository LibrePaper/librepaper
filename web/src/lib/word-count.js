// Words are counted from the source. The spellcheck scanners blank markup, so
// commands, math, comments and code are skipped while headings, captions and
// footnotes still count. Counting runs over the blanked text from proseText.
import { proseText } from "./harper/prose.js";
import { formatOf } from "./formats.js";
import { includesOf } from "./opening.js";

// A word is a run of letters, marks or digits, joined by one apostrophe
// (ASCII or typographic) or hyphen to another run.
const WORD = /[\p{L}\p{M}\p{N}]+(?:['\u2019-][\p{L}\p{M}\p{N}]+)*/gu;
// Han, Hiragana and Katakana have no spaces, so each character is a word.
const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]/gu;

// The number of words in text that has already been blanked.
function countTokens(prose) {
  const cjk = prose.match(CJK);
  const words = prose.replace(CJK, " ").match(WORD);
  return (cjk ? cjk.length : 0) + (words ? words.length : 0);
}

/**
 * The number of words in the whole text.
 * @param {string} text
 * @param {string} format
 * @returns {number}
 */
export function countWords(text, format) {
  return countTokens(proseText(text, format).text);
}

/**
 * The number of words in the prose between two offsets. The whole text is
 * blanked first so the markup context is right, and a word cut by an edge
 * counts once.
 * @param {string} text
 * @param {string} format
 * @param {number} from
 * @param {number} to
 * @returns {number}
 */
export function countWordsIn(text, format, from, to) {
  return countTokens(proseText(text, format).text.slice(from, to));
}

/**
 * The words in each section. Heading i covers [headings[i].from,
 * headings[i + 1].from), and the last heading runs to the end of the text.
 * @param {string} text
 * @param {string} format
 * @param {Array<{ from: number }>} headings  sorted by from
 * @returns {number[]}  parallel to headings
 */
export function sectionWords(text, format, headings) {
  // Blanking once keeps the cost linear in the text, not in the headings.
  const prose = proseText(text, format).text;
  return headings.map((heading, i) => {
    const to = i + 1 < headings.length ? headings[i + 1].from : text.length;
    return countTokens(prose.slice(heading.from, to));
  });
}

/**
 * The root file, then every file it reaches through includes, depth first in
 * source order, each once. Empty when the root is not a text of the project.
 * @param {string} root
 * @param {{ [path: string]: string }} texts
 * @returns {string[]}
 */
export function documentFiles(root, texts) {
  if (!Object.hasOwn(texts, root)) return [];
  const seen = new Set();
  const files = [];
  const visit = (path) => {
    seen.add(path);
    files.push(path);
    for (const next of includesOf(path, texts)) {
      if (!seen.has(next)) visit(next);
    }
  };
  visit(root);
  return files;
}

/**
 * The words in a document: the root file and every file it includes.
 * @param {string} root
 * @param {{ [path: string]: string }} texts
 * @returns {{ words: number, files: string[] }}
 */
export function documentWords(root, texts) {
  const files = documentFiles(root, texts);
  let words = 0;
  for (const path of files) words += countWords(texts[path], formatOf(path) || "");
  return { words, files };
}
