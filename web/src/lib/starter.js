// What a new project is made of. A project is a directory and a main file, so
// a project with no main file is not a project; the files themselves are
// templates, kept as plain files under src/templates and read by templates.js
// (the half that needs Vite). This half is pure, so node can test it: it
// knows the formats, and how a name and an author become text in each one.
//
// Keep the extensions in step with `main_path_for` in
// crates/librepaper/src/document/render.rs.

export const FORMATS = [
  { id: "markdown", name: "Markdown", extension: "md" },
  { id: "quarto", name: "Quarto", extension: "qmd" },
  { id: "latex", name: "LaTeX", extension: "tex" },
  { id: "typst", name: "Typst", extension: "typ" },
  { id: "html", name: "HTML", extension: "html" },
];

export const formatNamed = (id) => FORMATS.find((format) => format.id === id) ?? FORMATS[0];

// Names are written into source, so the characters each format reads as
// syntax have to stop being syntax: a paper called "A & B" is a title in all
// five and a broken file in two of them. A template puts each placeholder in
// one kind of place per format (see below), and the escape is for that place.
const texEscape = (title) => title.replace(/[\\{}$&#^_%~]/g, (char) => ({
  "\\": "\\textbackslash{}", "^": "\\textasciicircum{}", "~": "\\textasciitilde{}",
}[char] ?? `\\${char}`));

const htmlEscape = (title) => title.replace(/[&<>"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[char]));

// Typst strings and YAML double-quoted scalars agree on what to escape.
const quoteEscape = (title) => title.replace(/[\\"]/g, (char) => `\\${char}`);

// CommonMark lets any ASCII punctuation be backslash-escaped, which is the
// one way to say "this is text" without knowing which characters matter.
const markdownEscape = (title) => title.replace(/[!-/:-@[-`{-~]/g, (char) => `\\${char}`);

const ESCAPES = {
  latex: texEscape,
  typst: quoteEscape,
  quarto: quoteEscape,
  markdown: markdownEscape,
  html: htmlEscape,
};

/// A template's files with `{{title}}` and `{{author}}` filled in: [{ path, text }].
/// Each format keeps a placeholder to one kind of context (LaTeX text, a Typst
/// or YAML string, Markdown text, HTML text or attribute), so the value is
/// escaped once, for that context. Replacement is by function, so a `$` in a
/// name is never read as a replacement pattern.
export function fillTemplate(files, formatId, { title = "", author = "" } = {}) {
  const escape = ESCAPES[formatNamed(formatId).id];
  const values = {
    title: escape(title.trim() || "Untitled"),
    author: escape(author.trim() || "Your Name"),
  };
  return files.map(({ path, text }) => ({
    path,
    text: text.replace(/\{\{(title|author)\}\}/g, (_, key) => values[key]),
  }));
}

/// The templates worth showing for a format and a search. `formatId` of null
/// or "" means any. Every word of the query has to match the name, the
/// description or a keyword, in any order and any case, so "cover letter"
/// finds a letter and "thesis book" finds the one that is both.
export function matchTemplates(templates, query, formatId) {
  const words = (query ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  return templates.filter((template) => {
    if (formatId && !template.formats.includes(formatId)) return false;
    const haystack = [template.name, template.description, ...(template.keywords ?? [])].join("\n").toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}
