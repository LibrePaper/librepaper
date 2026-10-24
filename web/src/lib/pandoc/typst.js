// Markdown and HTML documents reach a PDF by way of Typst: Pandoc, running
// in the browser as WebAssembly, writes a Typst source and the Typst renderer
// compiles it. This turns one render tree into the other.

import { pandoc } from "./client.js";
import { parseQuarto } from "../engines/quarto.js";

/// The folder a project path sits in, or "." for a path at the root.
export function dirOf(path) {
  const at = path.lastIndexOf("/");
  return at < 0 ? "." : path.slice(0, at);
}

/// A path beside `main` that no file of the tree uses: `<dir>/<stem>.pandoc.<ext>`,
/// then `<stem>.pandoc-2.<ext>` and on. Beside the main file, so the relative
/// image and bibliography paths of the document resolve from it unchanged.
export function pandocPath(tree, ext) {
  const at = tree.main.lastIndexOf("/");
  const dir = at < 0 ? "" : tree.main.slice(0, at + 1);
  const name = tree.main.slice(at + 1);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const taken = (path) => path in (tree.texts || {}) || path in (tree.assets || {});
  let path = `${dir}${stem}.pandoc.${ext}`;
  for (let n = 2; taken(path); n += 1) path = `${dir}${stem}.pandoc-${n}.${ext}`;
  return path;
}

// The citation style a Markdown document names. The citations engine reads
// `csl` and then `bibliography-style`, so Pandoc is told the same.
function citationStyle(source, path) {
  const metadata = parseQuarto(source, { path }).metadata || {};
  for (const key of ["csl", "bibliography-style"]) {
    const value = metadata[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

/// Turn a Markdown or HTML document into a tree the Typst renderer compiles to PDF.
///
/// `tree` is `{ main, texts, assets, ... }` with `main` a Markdown, Quarto or
/// HTML path; a Quarto caller passes the tree already expanded. `from` is
/// "markdown" or "html". `run` is Pandoc, injected by tests.
///
/// Citations are not resolved by Pandoc: it writes `@key` and a
/// `#bibliography(...)` call, and Typst formats them.
export async function typstTree(tree, { from, run = pandoc }) {
  const typPath = pandocPath(tree, "typ");
  const variables = { "header-includes": "#set bibliography(title: none)" };
  if (from === "markdown") {
    const style = citationStyle(tree.texts?.[tree.main] ?? "", tree.main);
    if (style) variables.bibliographystyle = style;
  }
  const { output } = await run(
    {
      from,
      to: "typst",
      standalone: true,
      "input-files": [tree.main],
      "resource-path": [dirOf(tree.main)],
      variables,
    },
    { ...tree.texts, ...tree.assets },
  );
  return { ...tree, main: typPath, texts: { ...tree.texts, [typPath]: new TextDecoder().decode(output) } };
}
