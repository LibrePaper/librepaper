// What a document can be exported as, and how each file is made.
//
// Every source format offers a PDF, an HTML page and a Word file, produced in
// this browser. LaTeX and Typst compile to PDF and render to HTML; Markdown
// and Quarto render to HTML and reach a PDF through Pandoc and Typst; authored
// HTML is its own page and reaches a PDF the same way. A Word file is Pandoc's
// reading of the HTML page. Nothing here touches the DOM, and nothing is
// fetched or uploaded: the Reader supplies the tree it already holds.
//
// An export follows the Run code switch, as the preview does: with it on in
// the desktop app and the format set to this computer, a file the format has a
// command for comes from that command, and otherwise it takes the browser
// route, which does not run code cells.

import { pandocAvailable } from "../pandoc/client.js";
import { dirOf, pandocPath, typstTree } from "../pandoc/typst.js";
import { compilerAvailable } from "../renderers.js";
import { LOCAL_OUTPUTS } from "../desktop/client.js";
import { parseQuarto } from "../engines/quarto.js";
import { docxFile, inlineBlobUrls } from "./downloads.js";

export const EXPORT_TARGETS = ["pdf", "html", "docx"];

const NAMES = { pdf: "PDF", html: "HTML page", docx: "Word file" };

const MARKDOWN_PDF = { backend: "browser", output: "pdf" };
const MARKDOWN_HTML = { backend: "browser", output: "html" };

/// How many executable code cells a Quarto main file holds, which a browser
/// export leaves unrun.
export function unrunCells(format, tree) {
  if (format !== "quarto") return 0;
  const source = tree.texts?.[tree.main];
  if (typeof source !== "string") return 0;
  return parseQuarto(source, { path: tree.main }).cells.length;
}

/// Whether `target` can be produced for source `format` in this browser.
///
/// `deps.pandoc` says whether this deployment offers Pandoc, and
/// `deps.compiler` answers whether a format has a renderer; both default to
/// the live answers. Returns `{ ok: true }` or `{ ok: false, reason }` with
/// one plain sentence.
export function exportAvailability(format, target, deps = {}) {
  const hasPandoc = deps.pandoc ?? pandocAvailable();
  const compiler = deps.compiler ?? compilerAvailable;
  if (!EXPORT_TARGETS.includes(target)) return { ok: false, reason: "That kind of file cannot be exported." };
  if (!["latex", "typst", "markdown", "quarto", "html"].includes(format)) {
    return { ok: false, reason: "Only LaTeX, Typst, Markdown, Quarto and HTML documents can be exported." };
  }
  const needs = (...requirements) => {
    for (const [ok, reason] of requirements) if (!ok) return { ok: false, reason };
    return { ok: true };
  };
  const pandocNeed = [hasPandoc, "This deployment does not offer Pandoc."];
  const noRenderer = (name) => [compiler(name), `This deployment cannot render ${name === "latex" ? "LaTeX" : name === "typst" ? "Typst" : "Markdown"} documents.`];
  if (format === "html") {
    if (target === "html") return { ok: true };
    return target === "pdf" ? needs(noRenderer("typst"), pandocNeed) : needs(pandocNeed);
  }
  if (format === "latex") return target === "docx" ? needs(noRenderer("latex"), pandocNeed) : needs(noRenderer("latex"));
  if (format === "typst") return target === "docx" ? needs(noRenderer("typst"), pandocNeed) : needs(noRenderer("typst"));
  // Markdown and Quarto.
  if (target === "html") return needs(noRenderer(format));
  if (target === "pdf") return needs(noRenderer("typst"), pandocNeed);
  return needs(noRenderer(format), pandocNeed);
}

// A render that produced nothing says why in plain words: the first thing
// the compiler had to say, when it said anything.
function failure(result, target) {
  const message = result?.diagnostics?.find((d) => d?.message)?.message || result?.failure?.message;
  return new Error(message ? `The ${NAMES[target]} could not be made: ${message}` : `The ${NAMES[target]} could not be made.`);
}

/// Pandoc's HTML reader takes equations only from MathML, and from a MathML
/// element it reads the TeX annotation. The Markdown renderer carries math as
/// TeX in data-attributed spans, so each becomes a MathML shell around its TeX,
/// which Pandoc writes into Word as a native equation.
export function pandocMath(html) {
  return html.replace(
    /<span\b[^>]*?\bdata-math-style="(inline|display)"[^>]*>([\s\S]*?)<\/span>/g,
    (_whole, style, tex) => `<math xmlns="http://www.w3.org/1998/Math/MathML"${style === "display" ? ' display="block"' : ""}>` +
      `<semantics><mrow></mrow><annotation encoding="application/x-tex">${tex}</annotation></semantics></math>`,
  );
}

/// Produce the file to save, from what this tab holds and nothing else.
///
/// - `tree`: the full render tree, assets and urls already gathered.
/// - `preview`: what the frame holds now, `{ kind: "pdf", bytes }`,
///   `{ kind: "html", html }` or null; reused when its kind is the one
///   wanted and `previewCurrent` says it is of the source on screen.
/// - `project`: the document's slug, which a local build is scoped to.
/// - `render`: `renderers.render`. `run`: Pandoc.
/// - `local`: whether the desktop app builds the file with the command for
///   its format, false for the browser route.
///
/// Resolves `{ blob, name }`; rejects with an Error whose message is a plain
/// sentence.
export async function exportFile({ target, format, tree, title, slug, project = "", preview, previewCurrent, render, run, local = false }) {
  const reused = previewCurrent && preview?.kind === target ? preview : null;

  // The desktop app builds the target itself, with no Pandoc step. The frame
  // is not reused: its page may predate Run code and lack the cells' results.
  async function localBuild() {
    const result = await render(tree, title, {
      format: target,
      project,
      buildPreferences: { backend: "local", output: target },
    });
    if (result?.ok === false) throw failure(result, target);
    return result;
  }

  async function localFile() {
    const result = await localBuild();
    if (target === "pdf") {
      if (!result?.pdf) throw failure(result, target);
      return { blob: new Blob([result.pdf], { type: "application/pdf" }), name: `${slug}.pdf` };
    }
    if (target === "html") {
      if (typeof result?.html !== "string") throw failure(result, target);
      return { blob: new Blob([await inlineBlobUrls(result.html)], { type: "text/html" }), name: `${slug}.html` };
    }
    if (result?.artifactKind !== "docx" || !result.artifact) throw failure(result, target);
    return docxFile(result.artifact, slug);
  }

  async function pdfBytes() {
    if (reused) return reused.bytes;
    let result;
    if (format === "html") {
      result = await render(await typstTree(tree, { from: "html", run }), title, { format: "pdf", project });
    } else if (format === "markdown" || format === "quarto") {
      result = await render(tree, title, { format: "pdf", project, buildPreferences: MARKDOWN_PDF });
    } else {
      result = await render(tree, title, { format: "pdf", project });
    }
    if (!result?.pdf) throw failure(result, "pdf");
    return result.pdf;
  }

  async function htmlText() {
    if (format === "html") {
      const source = tree.texts?.[tree.main];
      if (typeof source !== "string") throw new Error("The HTML page could not be read.");
      return source;
    }
    if (previewCurrent && preview?.kind === "html") return inlineBlobUrls(preview.html);
    const markdown = format === "markdown" || format === "quarto";
    const result = await render(tree, title, markdown
      ? { format: "html", project, buildPreferences: MARKDOWN_HTML }
      : { format: "html", project });
    if (typeof result?.html !== "string") throw failure(result, target);
    return inlineBlobUrls(result.html);
  }

  if (local && (LOCAL_OUTPUTS[format] || []).includes(target)) return localFile();
  if (target === "pdf") {
    return { blob: new Blob([await pdfBytes()], { type: "application/pdf" }), name: `${slug}.pdf` };
  }
  if (target === "html") {
    return { blob: new Blob([await htmlText()], { type: "text/html" }), name: `${slug}.html` };
  }
  if (target === "docx") {
    const html = pandocMath(await htmlText());
    const htmlPath = pandocPath(tree, "html");
    const { output } = await run(
      { from: "html", to: "docx", "input-files": [htmlPath], "resource-path": [dirOf(tree.main)] },
      { ...tree.texts, ...tree.assets, [htmlPath]: html },
    );
    return docxFile(output, slug);
  }
  throw new Error("That kind of file cannot be exported.");
}
