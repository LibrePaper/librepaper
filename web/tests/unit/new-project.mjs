// The New project page's pure logic: the template catalog and picker, the
// project name and format, and the staging of files for the Upload tab.
import assert from "node:assert/strict";
import {
  bytes, browse, catalog, finalPaths, formatSummary, guessMain, mainCandidates,
  pickFormat, settle, stage, suggestName, tree, uniqueName, unstage, wrapperOf,
} from "../../src/lib/new-project.js";

const ids = (list) => list.map((each) => each.id);
const names = (list) => list.map((each) => each.name);

// ---- catalog ----
{
  const builtIn = [
    { id: "blank", name: "Blank", description: "An empty page.", keywords: ["empty"], formats: ["markdown"] },
    { id: "letter", name: "Letter", description: "A formal letter.", keywords: ["reference"], formats: ["typst", "latex"] },
    { id: "cv", name: "CV", description: "A curriculum vitae.", keywords: ["resume"], formats: ["html"] },
  ];
  const rows = [{ slug: "lab-abc", title: "Lab report", source_format: "markdown", url: "/d/lab" }];
  const all = catalog(builtIn, rows);
  assert.deepEqual(ids(all), ["letter", "cv", "lab-abc"], "blank is left out, built-ins keep their order, custom rows come last");
  assert.deepEqual(all[0], {
    id: "letter", name: "Letter", description: "A formal letter.", keywords: ["reference"],
    formats: ["typst", "latex"], custom: false, url: "",
  });
  assert.deepEqual(all[2], {
    id: "lab-abc", name: "Lab report", description: "", keywords: [],
    formats: ["markdown"], custom: true, url: "/d/lab",
  }, "a custom row maps to a template with its title, source format and link");
  assert.deepEqual(ids(catalog(builtIn)), ["letter", "cv"], "custom rows are optional");
}

// ---- browse ----
{
  const templates = [
    { id: "article", name: "Article", description: "A paper.", keywords: ["journal"], formats: ["markdown", "latex"], custom: false },
    { id: "book", name: "Book", description: "Chapters.", keywords: ["thesis"], formats: ["latex", "typst", "quarto"], custom: false },
    { id: "letter", name: "Letter", description: "A letter.", keywords: ["reference"], formats: ["typst", "latex"], custom: false },
    { id: "cv", name: "CV", description: "A curriculum vitae.", keywords: ["resume"], formats: ["html", "typst"], custom: false },
    { id: "lab", name: "Lab report", description: "", keywords: [], formats: ["markdown"], custom: true },
  ];
  assert.deepEqual(ids(browse(templates)), ["article", "book", "cv", "lab", "letter"], "name A to Z by default");
  assert.deepEqual(ids(browse(templates, { sort: "name-asc" })), ["article", "book", "cv", "lab", "letter"]);
  assert.deepEqual(ids(browse(templates, { sort: "name-desc" })), ["letter", "lab", "cv", "book", "article"], "name Z to A");
  assert.equal(templates[0].id, "article", "the input is not reordered");
  assert.deepEqual(ids(browse(templates, { format: "latex" })), ["article", "book", "letter"]);
  assert.deepEqual(ids(browse(templates, { query: "thesis" })), ["book"]);
  assert.deepEqual(ids(browse(templates, { query: "letter", format: "typst" })), ["letter"]);
  assert.deepEqual(ids(browse(templates, { query: "thesis", format: "html" })), [], "filters that do not meet give nothing");
  assert.deepEqual(ids(browse(templates, { query: "chapters", format: "latex", sort: "name-desc" })), ["book"], "query and format intersect");
  assert.deepEqual(ids(browse(templates, { query: "", format: "", sort: "name-asc" })), ids(browse(templates)));
  assert.deepEqual(names(browse([])), []);
}

// ---- formatSummary and pickFormat ----
{
  assert.equal(formatSummary(["latex", "markdown"]), "Markdown, LaTeX", "names in FORMATS order");
  assert.equal(formatSummary(["typst"]), "Typst");
  assert.equal(formatSummary(["markdown", "quarto", "latex"]), "Markdown, Quarto, LaTeX");
  assert.equal(formatSummary(["markdown", "quarto", "latex", "typst", "html"]), "Markdown, Quarto, LaTeX +2", "more than three gives the first three and a count");
  assert.equal(formatSummary(["markdown", "quarto"], 1), "Markdown +1");
  assert.equal(formatSummary([]), "");

  const letter = { id: "letter", formats: ["typst", "latex"] };
  assert.equal(pickFormat(letter, { chosen: "latex", filter: "typst", preferred: "typst" }), "latex", "the chosen format wins");
  assert.equal(pickFormat(letter, { chosen: "html", filter: "typst" }), "typst", "a chosen format the template lacks is passed over");
  assert.equal(pickFormat(letter, { filter: "html", preferred: "latex" }), "latex", "then the preferred one");
  assert.equal(pickFormat(letter, { chosen: "html", filter: "html", preferred: "html" }), "typst", "then the first the template offers");
  assert.equal(pickFormat(letter), "typst");
  assert.equal(pickFormat(null, { chosen: "typst" }), "", "a missing template has no format");

  const formats = ["markdown", "quarto", "latex", "typst", "html", "bogus", ""];
  for (const chosen of formats) {
    for (const filter of formats) {
      for (const preferred of formats) {
        const picked = pickFormat(letter, { chosen, filter, preferred });
        assert.ok(letter.formats.includes(picked), `${picked} must be offered by the template`);
      }
    }
  }
}

// ---- uniqueName ----
{
  assert.equal(uniqueName([]), "Untitled project");
  assert.equal(uniqueName(["Thesis"]), "Untitled project");
  assert.equal(uniqueName(["untitled PROJECT  "]), "Untitled project 2", "case and surrounding spaces are ignored");
  assert.equal(uniqueName(["Untitled project", "Untitled project 2"]), "Untitled project 3");
  assert.equal(uniqueName(new Set(["Untitled project 2"])), "Untitled project", "an iterable works, and a free base is used");
  assert.equal(uniqueName(["Draft"], "Draft"), "Draft 2", "the base can be given");
}

// ---- stage ----
const rules = {
  extensions: [".tex", ".md", ".qmd"],
  text_extensions: [".tex", ".md", ".qmd", ".bib"],
  asset_extensions: [".png"],
  derived_extensions: [".aux"],
  max_path: 200,
};
const blob = (text) => new Blob([text]);
{
  const existing = [{ path: "main.tex", file: blob("old"), source: "" }];
  const incoming = [
    { path: "main.tex", file: blob("new!!"), source: "paper.zip" },
    { path: "Fig.png", file: blob("image"), source: "" },
    { path: "notes.docx", file: blob("word"), source: "" },
    { path: ".git/config", file: blob("private"), source: "" },
    { path: "__MACOSX/._main.tex", file: blob("metadata"), source: "" },
    { path: "../evil.tex", file: blob("climb"), source: "" },
    { path: "fig.png", file: blob("again"), source: "" },
    { path: "chapters/a.tex", file: blob("chapter"), source: "" },
  ];
  const result = stage(existing, incoming, rules);
  assert.equal(existing.length, 1, "the input list is not mutated");
  assert.deepEqual(result.staged.map((entry) => entry.path), ["main.tex", "Fig.png", "chapters/a.tex"], "accepted entries are added in order");
  assert.deepEqual(result.conflicts.map((entry) => entry.path), ["main.tex", "fig.png"], "a staged path and a repeat within incoming are conflicts");
  assert.equal(result.conflicts[0].file.size, 5, "a conflict keeps its incoming file");
  assert.equal(result.conflicts[0].source, "paper.zip", "a conflict keeps its source");
  assert.deepEqual(result.skipped, ["notes.docx"], "an unsupported extension is skipped and listed");
  assert.equal(result.errors.length, 1, "an invalid path is reported");
  assert.match(result.errors[0], /^\.\.\/evil\.tex: .*climb/);
  assert.equal(result.staged.find((entry) => entry.path === "Fig.png").source, "", "a missing source defaults to empty");
  assert.ok(!result.staged.some((entry) => entry.path.includes("git") || entry.path.includes("MACOSX")), "hidden files are dropped silently");
  assert.ok(!result.errors.some((error) => error.includes("MACOSX") || error.includes(".git")));
  assert.ok(!result.skipped.some((path) => path.includes(".git")));
}
{
  const result = stage([], [{ path: "a.md", file: blob("a") }, { path: "a.md", file: blob("again") }], rules);
  assert.equal(result.staged.length, 1);
  assert.equal(result.conflicts.length, 1, "a second copy within the same batch is a conflict");
}

// ---- settle ----
{
  const staged = [
    { path: "main.tex", file: blob("old"), source: "" },
    { path: "Fig.png", file: blob("image"), source: "" },
  ];
  const conflicts = [
    { path: "main.tex", file: blob("newer!"), source: "paper.zip" },
    { path: "fig.png", file: blob("picture!"), source: "" },
  ];
  const replaced = settle(staged, conflicts, "replace");
  assert.deepEqual(replaced.map((entry) => entry.path), ["main.tex", "fig.png"], "replace takes the incoming spelling in the staged place");
  assert.equal(replaced[0].file.size, 6);
  assert.equal(replaced[0].source, "paper.zip");
  assert.equal(staged[0].file.size, 3, "the staged list is not mutated");

  const both = settle(staged, [conflicts[0]], "keep-both");
  assert.deepEqual(both.map((entry) => entry.path), ["main.tex", "Fig.png", "main (2).tex"], "keep-both adds the copy under a new name");
  assert.equal(both[2].file.size, 6);

  const skipped = settle(staged, conflicts, "skip");
  assert.deepEqual(skipped, staged, "skip drops the incoming files");
}

// ---- unstage ----
{
  const staged = [
    { path: "a/x.md" }, { path: "a/b/y.md" }, { path: "a.md" }, { path: "ab.md" },
  ];
  assert.deepEqual(unstage(staged, "a").map((entry) => entry.path), ["a.md", "ab.md"], "a folder takes everything under it, and nothing beside it");
  assert.deepEqual(unstage(staged, "a.md").map((entry) => entry.path), ["a/x.md", "a/b/y.md", "ab.md"], "a file takes only itself");
  assert.equal(staged.length, 4, "unstage does not mutate");
}

// ---- wrapperOf and finalPaths ----
{
  const wrapped = [{ path: "paper/main.tex" }, { path: "paper/fig/a.png" }];
  assert.equal(wrapperOf(wrapped), "paper");
  assert.equal(wrapperOf([...wrapped, { path: "other.md" }]), "", "a root-level file means no wrapper");
  assert.equal(wrapperOf([{ path: "main.tex" }]), "");
  assert.equal(wrapperOf([{ path: "a/x.tex" }, { path: "b/y.tex" }]), "", "two top folders means no wrapper");
  assert.equal(wrapperOf([]), "");
  assert.deepEqual(finalPaths(wrapped, true), ["main.tex", "fig/a.png"]);
  assert.deepEqual(finalPaths(wrapped, false), ["paper/main.tex", "paper/fig/a.png"]);
  assert.deepEqual(finalPaths([{ path: "a.md" }], true), ["a.md"], "nothing is stripped without a wrapper");
}

// ---- mainCandidates and guessMain ----
{
  const rulesForMain = { extensions: [".tex", ".md", ".qmd"] };
  assert.deepEqual(
    mainCandidates(["sub/Ch.qmd", "main.tex", "fig.png", "notes.MD"], rulesForMain),
    ["main.tex", "notes.MD", "sub/Ch.qmd"],
    "renderable files only, A to Z",
  );
  assert.equal(guessMain(["chapters/main.tex", "main.md", "index.qmd"]), "main.md", "a root main file beats an index");
  assert.equal(guessMain(["main.tex", "main.md"]), "main.tex", "tex is preferred among main files");
  assert.equal(guessMain(["index.md", "index.qmd", "notes.md"]), "index.qmd", "index.qmd beats index.md");
  assert.equal(guessMain(["index.md", "notes.md"]), "index.md");
  assert.equal(guessMain(["a.tex"]), "a.tex", "the only root-level candidate");
  assert.equal(guessMain(["notes.md", "sub/main.tex"]), "notes.md", "a nested main is not a root main");
  assert.equal(guessMain(["chapters/one.tex"]), "chapters/one.tex", "the only candidate at all");
  assert.equal(guessMain(["one.md", "two.md"]), "", "ambiguous gives nothing");
  assert.equal(guessMain(["ch/a.tex", "ch/b.tex"]), "");
  assert.equal(guessMain([]), "");
}

// ---- tree ----
{
  const nodes = tree([
    { path: "main.tex", size: 10 },
    { path: "fig/b.png", size: 2048 },
    { path: "fig/a.png", size: 5 },
    { path: "chapters/one.qmd", size: 7 },
    { path: "chapters/deep/x.md", size: 1 },
    { path: "zeta.md", size: 0 },
  ]);
  assert.deepEqual(names(nodes), ["chapters", "fig", "main.tex", "zeta.md"], "folders first, then files, each A to Z");
  const [chapters, fig, main] = nodes;
  assert.equal(chapters.path, "chapters");
  assert.equal(chapters.count, 2, "a folder counts the files below it");
  assert.deepEqual(names(chapters.children), ["deep", "one.qmd"]);
  assert.equal(chapters.children[0].count, 1);
  assert.equal(fig.count, 2);
  assert.deepEqual(names(fig.children), ["a.png", "b.png"]);
  assert.equal(fig.children[1].path, "fig/b.png");
  assert.equal(fig.children[1].size, 2048, "a file carries its size");
  assert.equal(main.size, 10);
  assert.equal(main.children, undefined, "a file has no children");
  assert.deepEqual(tree([]), []);
}

// ---- suggestName ----
{
  assert.equal(suggestName([{ path: "paper/main.tex" }], { archives: ["Paper.zip"] }), "Paper", "the ZIP name comes first");
  assert.equal(suggestName([{ path: "paper/main.tex" }], { archives: ["Draft.ZIP"] }), "Draft");
  assert.equal(suggestName([{ path: "paper/main.tex" }]), "paper", "then the wrapper folder");
  assert.equal(suggestName([{ path: "main.tex" }, { path: "fig.png" }]), "main", "then the main document's stem");
  assert.equal(suggestName([{ path: "a/x.tex" }, { path: "b/y.tex" }]), "", "nothing to suggest");
  assert.equal(suggestName([], { archives: ["x.zip"] }), "x");
  assert.equal(suggestName([]), "");
}

// ---- bytes ----
{
  assert.equal(bytes(0), "0 B");
  assert.equal(bytes(512), "512 B");
  assert.equal(bytes(1023), "1023 B");
  assert.equal(bytes(1024), "1.0 KB");
  assert.equal(bytes(1536), "1.5 KB");
  assert.equal(bytes(1023 * 1024), "1023 KB");
  assert.equal(bytes(10 * 1024), "10 KB", "no decimal from ten up");
  assert.equal(bytes(1024 * 1024), "1.0 MB");
  assert.equal(bytes(1024 * 1024 - 1), "1.0 MB", "a value that rounds up to the next unit moves up");
  assert.equal(bytes(Math.round(3.4 * 1024 * 1024)), "3.4 MB");
  assert.equal(bytes(Math.round(1.1 * 1024 ** 3)), "1.1 GB");
}

console.log("new-project: catalog, browse, format and name choice, staging, settling, wrappers, main file, tree, names and sizes passed");
