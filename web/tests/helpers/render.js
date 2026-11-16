// The examples, rendered the way the deployment renders them, so the check
// beside this one measures against the document a reader actually sees rather
// than against an approximation of it.
//
// The renderers are the engine's, compiled to WebAssembly -- the same modules
// the editor loads -- so this needs no toolchain of its own beyond what the
// build already produces.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { call, handOver, validateExports } from "../../src/lib/renderer-wasm.js";
import * as quarto from "../../src/lib/engines/quarto.js";

function load(name) {
  const path = new URL(`../../wasm/${name}.wasm`, import.meta.url);
  try {
    const module = new WebAssembly.Module(readFileSync(path));
    return validateExports(new WebAssembly.Instance(module, {}).exports, `${name}.wasm`);
  } catch (error) {
    if (error.code === "ENOENT") return null; // not built; the caller says so rather than failing
    throw error;
  }
}

// Typst's browser output is a PDF, so the sync check uses the same extracted
// text a PDF reader anchors against. Poppler is already a required dependency
// of `check:typst-pdf`; keeping this independent parser here avoids decoding a
// binary PDF as HTML and lets this check remain a Node-only source navigation
// check.
function pdfText(bytes, label) {
  const directory = mkdtempSync(join(tmpdir(), "librepaper-sync-pdf-"));
  const file = join(directory, "render.pdf");
  try {
    writeFileSync(file, bytes);
    return execFileSync("pdftotext", ["-layout", file, "-"], {
      encoding: "utf8",
      env: { ...process.env, LC_ALL: "C" },
    }).replace(/\s+/g, " ").trim();
  } catch (error) {
    throw new Error(`${label}: PDF text extraction needs pdftotext (${error.message})`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

/// What the agent publishes from a rendered document: its visible text, with
/// whitespace collapsed the way a browser collapses it.
export function visibleText(html) {
  const body = (html.match(/<body[^>]*>([\s\S]*)<\/body>/) || [null, html])[1];
  return body
    .replace(/<(script|style)\b[^>]*>[\s\S]*?<\/\1>/gi, " ")
    .replace(/<[^>]*>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

// A document is a directory, so what is rendered is a tree: `{ main, texts,
// assets }`, the same object `renderers.js` takes in the browser. The files go
// into the module before the compile and the main file is named, which is what
// lets an example import a chapter or cite a .bib here exactly as it would
// there.
//
// A bare string is still accepted and read as a document of one file, because
// most examples are one file and writing `{ texts: { "x.typ": source } }` at
// every call site would say nothing the string does not.
function treeOf(source, file) {
  if (typeof source === "string") return { main: file, texts: { [file]: source }, assets: {} };
  return { main: source.main || file, texts: source.texts || {}, assets: source.assets || {} };
}

const render = (name) => (source, file) => {
  const wasm = load(name);
  if (!wasm) return null;
  const tree = treeOf(source, file);
  handOver(wasm, tree);
  const result = call(wasm, "compile", tree.texts[tree.main] ?? "", tree.main);
  if (!result.ok) throw new Error(`${tree.main}: ${result.text}`);
  return result.kind === "pdf" ? pdfText(result.bytes, tree.main) : visibleText(result.text);
};

export const renderMarkdown = render("markdown");
export const renderTypst = render("typst");
/// Markdown or quarto that cites a bibliography, which the deployment renders
/// with this second module rather than the plain markdown one -- see the
/// choice `renderers.js` makes on `needsBibliography`.
export const renderCitations = render("citations");

/// HTML's renderer is the identity, so what a reader sees is the source's own
/// visible text and no module is needed to work it out.
export const renderHtml = (source) => visibleText(source);

/// A Quarto document as `renderers.js` renders one with a bibliography: the
/// markdown module drafts the .qmd (with its outputs sidecar), and the
/// citations module renders that draft, with the .qmd replaced by a .md file.
/// Async because the sidecar is built by the engine's fingerprinting.
export async function renderQuarto(source, file) {
  const drafter = load("markdown");
  const citations = load("citations");
  if (!drafter || !citations) return null;
  const tree = treeOf(source, file);
  const sidecar = await quarto.quartoSidecar(tree.texts[tree.main] ?? "", { path: tree.main, results: null, files: tree.texts, strings: {} });
  const withSidecar = { ...tree, texts: { ...tree.texts, [quarto.QUARTO_SIDECAR]: JSON.stringify(sidecar) } };
  handOver(drafter, withSidecar);
  const drafted = call(drafter, "draft", withSidecar.texts[tree.main] ?? "", 0);
  const parsed = JSON.parse(new TextDecoder().decode(drafted.bytes) || "{}");
  const main = tree.main.replace(/\.qmd$/i, ".md");
  const texts = { ...tree.texts, [main]: parsed.markdown ?? "" };
  delete texts[tree.main];
  const draftedTree = { ...tree, main, texts };
  handOver(citations, draftedTree);
  const result = call(citations, "compile", draftedTree.texts[main], main);
  if (!result.ok) throw new Error(`${main}: ${result.text}`);
  return result.kind === "pdf" ? pdfText(result.bytes, main) : visibleText(result.text);
}

/// What the compiler said about a tree, rather than what it produced. The
/// second result channel, read the way `renderers.js` reads it, so a check can
/// assert which file an error is in.
export function diagnose(name, source, file) {
  const wasm = load(name);
  if (!wasm) return null;
  const tree = treeOf(source, file);
  handOver(wasm, tree);
  const { ok, diagnostics } = call(wasm, "compile", tree.texts[tree.main] ?? "", tree.main);
  return { ok, diagnostics };
}
