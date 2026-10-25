// The language a source declares for itself. Harper only knows English, so the
// editor asks this before running it and stays quiet on a document written in
// French or German.
const SCAN = 20000;

// Babel and polyglossia names for English, besides anything that starts with "en".
const ENGLISH = new Set([
  "american", "british", "usenglish", "ukenglish", "canadian", "australian", "newzealand",
  "usenglishmax",
]);

function latex(text) {
  const source = text.split("\n").filter((line) => !line.trimStart().startsWith("%")).join("\n");
  const babel = /\\usepackage\s*\[([^\]]*)\]\s*\{[^}]*\bbabel\b[^}]*\}/.exec(source);
  if (babel) {
    const options = babel[1].split(",").map((o) => o.trim()).filter(Boolean);
    const main = options.find((o) => /^main\s*=/.test(o));
    if (main) return main.replace(/^main\s*=\s*/, "").trim();
    // Babel makes the last language the main one.
    if (options.length) return options[options.length - 1];
  }
  const poly = /\\setmainlanguage\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}/.exec(source);
  return poly ? poly[1].trim() : null;
}

function typst(text) {
  const set = /#set\s+text\s*\(([^)]*)\)/g;
  let match;
  while ((match = set.exec(text))) {
    const lang = /\blang\s*:\s*"([^"]*)"/.exec(match[1]);
    if (lang) return lang[1];
  }
  return null;
}

function markdown(text) {
  const lines = text.split(/\r?\n/);
  if (lines[0]?.trimEnd() !== "---") return null;
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i].trimEnd();
    if (line === "---" || line === "...") break;
    const lang = /^lang\s*:\s*(?:"([^"]*)"|'([^']*)'|([^\s#]+))/.exec(line);
    if (lang) return (lang[1] ?? lang[2] ?? lang[3]).trim();
  }
  return null;
}

function html(text) {
  const tag = /<html\b[^>]*>/i.exec(text);
  if (!tag) return null;
  const lang = /\blang\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(tag[0]);
  return lang ? (lang[1] ?? lang[2] ?? lang[3]).trim() : null;
}

/**
 * The language the source declares, as written, or null when it declares none.
 * @param {string} text
 * @param {string} format
 * @returns {string | null}
 */
export function declaredLanguage(text, format) {
  const head = text.slice(0, SCAN);
  if (format === "latex") return latex(head) || null;
  if (format === "typst") return typst(head) || null;
  if (format === "markdown" || format === "quarto") return markdown(head) || null;
  if (format === "html") return html(head) || null;
  return null;
}

/** @param {string | null | undefined} language */
export function isEnglish(language) {
  if (!language) return true;
  const name = language.trim().toLowerCase();
  return !name || name.startsWith("en") || ENGLISH.has(name);
}

/**
 * @param {string} text
 * @param {string} format
 */
export function isEnglishDocument(text, format) {
  return isEnglish(declaredLanguage(text, format));
}
