import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const HERE = dirname(fileURLToPath(import.meta.url));
export const ROOT = join(HERE, "../..");
export const CACHE = join(HERE, ".cache");
export const SOURCES = JSON.parse(readFileSync(join(HERE, "sources.json"), "utf8"));

// Official examples exercise workflows; the small local cases isolate failures.
// Engine directives are added only to the in-memory input. The benchmark
// records whether LibrePaper's adapter actually honors the requested engine.
export const CASES = [
  { id: "acm-conference", source: "acmart", main: "sigconf.tex", engine: "pdflatex", features: "ACM conference, BibTeX, figures, tables" },
  { id: "acm-journal", source: "acmart", main: "acmsmall.tex", engine: "pdflatex", features: "ACM journal, BibTeX, figures, tables" },
  { id: "thesis", source: "thesis", main: "main-english.tex", engine: "lualatex", features: "KOMA-Script thesis, Biber, TikZ, glossary, fonts" },
  { id: "biber-related", source: "biblatex", main: "90-related-entries.tex", engine: "xelatex", features: "Biber related entries, translations, Unicode" },
  { id: "biber-sorting", source: "biblatex", main: "91-sorting-schemes.tex", engine: "xelatex", features: "Biber citation and bibliography sorting contexts" },
  { id: "multifile", local: "paper", main: "main.tex", engine: "pdflatex", features: "Chapters, custom style, PNG/PDF assets, BibTeX" },
  { id: "packages", local: "packages", main: "main.tex", engine: "pdflatex", features: "siunitx, TikZ, booktabs, biblatex/BibTeX" },
  { id: "unicode-fonts", local: "../benchmark/fixtures/unicode-fonts", main: "main.tex", engine: "xelatex", features: "Named fonts, Greek/Cyrillic, Unicode math" },
];

export function filesIn(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name)).flatMap((entry) => {
    const path = prefix + entry.name;
    if (entry.isSymbolicLink()) throw new Error(`Unexpected symlink: ${directory}/${entry.name}`);
    return entry.isDirectory() ? filesIn(join(directory, entry.name), `${path}/`) : [path];
  });
}

export function treeOf(example) {
  const directory = example.local ? join(ROOT, "latex/corpus", example.local) : join(CACHE, "projects", example.id);
  const texts = {}, assets = {};
  for (const path of filesIn(directory)) {
    if (path.startsWith("logs/") || /(^|\/)(main\.pdf|expected\.json)$/.test(path)) continue;
    if (!/\.(tex|bib|sty|cls|bst|cfg|def|bbx|cbx|dbx|lbx|csv|dat|png|pdf|jpe?g|otf|ttf)$/.test(path)) continue;
    const bytes = readFileSync(join(directory, path));
    if (/\.(png|pdf|jpe?g|otf|ttf)$/.test(path)) assets[path] = [...bytes];
    else texts[path] = bytes.toString("utf8");
  }
  if (!texts[example.main]) throw new Error(`Missing ${example.id}/${example.main}; run prepare.mjs`);
  texts[example.main] = `% !TeX program = ${example.engine}\n` + texts[example.main];
  return { main: example.main, texts, assets };
}

export const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
export const treeDigest = (tree) => digest(JSON.stringify(tree));

export function selectedCases(argv) {
  const at = argv.indexOf("--case");
  const wanted = at < 0 ? null : argv[at + 1]?.split(",");
  if (wanted && wanted.some((id) => !CASES.some((c) => c.id === id))) throw new Error(`Unknown case: ${wanted}`);
  return CASES.filter((c) => !wanted || wanted.includes(c.id));
}
