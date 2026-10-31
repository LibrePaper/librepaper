// The pure half of the /try demo: which files the sandbox starts with, and
// how each format's copy of the article is renamed and annotated. Nothing
// here needs Vite, so node can test it. try-project.js loads the template
// files and hands them to buildTryFiles.

export const TRY_SLUG = "try-librepaper";
export const TRY_DOCUMENT_URL = "/api/try";

const WELCOME = "This is the LibrePaper demo sandbox. Nothing you type is saved, and reloading the page starts over. The same article is here in Typst, LaTeX, Markdown and HTML: open another file in Files to preview it.";

// The main file of each format, as the template names it, and the name it
// takes in the sandbox. Typst keeps its name, so it is the main file.
const MAINS = {
  typst: { from: "main.typ", to: "main.typ" },
  latex: { from: "main.tex", to: "latex.tex" },
  markdown: { from: "main.md", to: "markdown.md" },
  html: { from: "main.html", to: "page.html" },
};

const FORMAT_ORDER = ["typst", "latex", "markdown", "html"];

// A comment in the format's own syntax, ready to sit above the file.
const note = {
  typst: `// ${WELCOME}\n\n`,
  latex: `% ${WELCOME}\n\n`,
  markdown: `<!-- ${WELCOME} -->\n\n`,
  html: `<!-- ${WELCOME} -->\n\n`,
};

// Markdown may open with a YAML front matter block, which has to stay first.
const FRONT_MATTER = /^---\r?\n[\s\S]*?\r?\n---[ \t]*(?:\r?\n|$)/;

function withNote(format, text) {
  if (format !== "markdown") return note[format] + text;
  const match = FRONT_MATTER.exec(text);
  const at = match ? match[0].length : 0;
  return text.slice(0, at) + note.markdown + text.slice(at);
}

/**
 * The sandbox's files from each format's filled template files.
 * `byFormat` is { typst, latex, markdown, html }, each a list of { path, text }.
 * Typst goes first, so its references.bib is the one kept. Any other file
 * whose path is already taken is dropped, which removes the other formats'
 * copies of references.bib.
 * @param {Record<string, {path: string, text: string}[]>} byFormat
 * @returns {{path: string, text: string}[]}
 */
export function buildTryFiles(byFormat) {
  const taken = new Set();
  const files = [];
  for (const format of FORMAT_ORDER) {
    const { from, to } = MAINS[format];
    const main = (byFormat[format] ?? []).find((file) => file.path === from);
    if (!main) throw new Error(`the ${format} template has no ${from}`);
    files.push({ path: to, text: withNote(format, main.text) });
    taken.add(to);
    for (const file of byFormat[format] ?? []) {
      if (file.path === from || taken.has(file.path)) continue;
      files.push({ path: file.path, text: file.text });
      taken.add(file.path);
    }
  }
  return files;
}
