// What a project contains the moment it is made. A project is a directory and
// a main file, so a project with no main file is not a project; naming one is
// the whole of what "New project" asks for, and this is what that name buys.
//
// Each starter is the smallest file that renders and says its own title, so
// the author opens on something the preview can already draw rather than on a
// blank page and an error. Keep the extensions in step with `main_path_for` in
// crates/librepaper/src/document/render.rs.

export const FORMATS = [
  { id: "markdown", name: "Markdown", extension: "md" },
  { id: "quarto", name: "Quarto", extension: "qmd" },
  { id: "latex", name: "LaTeX", extension: "tex" },
  { id: "typst", name: "Typst", extension: "typ" },
  { id: "html", name: "HTML", extension: "html" },
];

// Titles are written into source, so the characters each format reads as
// syntax have to stop being syntax: a paper called "A & B" is a title in all
// five and a broken file in two of them.
const texEscape = (title) => title.replace(/[\\{}$&#^_%~]/g, (char) => ({
  "\\": "\\textbackslash{}", "^": "\\textasciicircum{}", "~": "\\textasciitilde{}",
}[char] ?? `\\${char}`));

const htmlEscape = (title) => title.replace(/[&<>]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[char]));

const yamlQuote = (title) => `"${title.replace(/[\\"]/g, (char) => `\\${char}`)}"`;

const typstQuote = (title) => `"${title.replace(/[\\"]/g, (char) => `\\${char}`)}"`;

const STARTERS = {
  markdown: (title) => `# ${title}\n`,
  quarto: (title) => `---\ntitle: ${yamlQuote(title)}\n---\n\n`,
  latex: (title) => `\\documentclass{article}\n\n\\title{${texEscape(title)}}\n\n\\begin{document}\n\\maketitle\n\n\\end{document}\n`,
  typst: (title) => `#set document(title: ${typstQuote(title)})\n\n= ${title}\n`,
  html: (title) => `<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<title>${htmlEscape(title)}</title>\n</head>\n<body>\n<h1>${htmlEscape(title)}</h1>\n</body>\n</html>\n`,
};

export const formatNamed = (id) => FORMATS.find((format) => format.id === id) ?? FORMATS[0];

/// The main file a new project begins with: its path, and its text.
export function starterDocument(title, formatId) {
  const format = formatNamed(formatId);
  const named = title.trim() || "Untitled";
  return { path: `main.${format.extension}`, text: STARTERS[format.id](named) };
}
