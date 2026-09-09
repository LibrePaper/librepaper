import { needsBibliography } from "./bibliography-engine.js";
// The renderers, loaded into the editor.
//
// The modules come from the same pinned renderer releases the native command
// line links, so the preview is the document a save would
// store, byte for byte, and an edit made here renders exactly as one made from
// the terminal. The deployment renders nothing: it stores what this browser
// produced.
//
// They are plain WebAssembly with a handful of exports rather than
// wasm-bindgen, so this is the whole of the glue: reserve memory in the
// module, write the source into it as UTF-8, call compile, read the page back
// out. The URLs carry a digest of each module's bytes and are handed to the
// page by the server, so a module cached for a year cannot outlive the loader
// that speaks to it.

import * as latex from "./latex.js";

import { rendererRequest } from "./renderer-client.js";

/// Whether this deployment serves LaTeX distributions, which is the one
/// renderer that is a property of the deployment rather than of the build:
/// the compiler is not in the binary, it is behind `--latex`, and a
/// deployment without a mirror has nowhere to send a browser for one. Set
/// from what `/api/documents/<slug>` reports, which is the same flag
/// `/api/config` answers with.
let latexOffered = false;
export function offerLatex(on) {
  latexOffered = Boolean(on);
}

function urls() {
  return globalThis.LIBREPAPER_MODULES || {};
}

/// Whether this deployment advertises a renderer for this format. All four
/// browser modules are required build inputs; LaTeX depends on the deployment.
///
/// HTML is the exception, and always true: its renderer is the identity, so it
/// is the one format every deployment can edit whatever it was built with.
export function available(format) {
  if (format === "latex") return latexOffered;
  return format === "html" || Boolean(urls()[format]);
}

// The source format and the output painted by the reader are separate
// concerns. Both LaTeX and Typst produce a paged PDF; Markdown and authored
// HTML produce flow HTML. Keeping this mapping here prevents the reader from
// accidentally using LaTeX's compiler chooser as a proxy for PDF support.
export function outputKind(format) {
  if (format === "latex" || format === "typst") return "pdf";
  if (format === "markdown" || format === "html") return "html";
  return "";
}

export function producesPdf(format) {
  return outputKind(format) === "pdf";
}

// Whether this browser can compile a source document. Stored artifacts remain
// readable when this is false; in particular a reader does not need Typst WASM
// merely to open a PDF another editor already produced.
export function compilerAvailable(format) {
  return format === "latex" ? latexOffered : available(format);
}

function request(format, operation, args = {}) {
  const url = urls()[format];
  if (!url) return Promise.reject(new Error(`no renderer for ${format}`));
  return rendererRequest(new URL(url, globalThis.location.href).href, operation, args);
}

/// The word-level diff shared with `librepaper sync`. It does not depend on
/// the source format, but runs through the same engine module as the document
/// so the browser needs no second WASM bundle. HTML documents use Markdown's
/// small module when one is available.
export function diff(oldText, newText, format = "markdown") {
  // Diffing is format independent. Prefer Markdown even for a Typst reader so
  // opening the changes panel does not fetch a thirty-megabyte compiler just
  // to run a text algorithm.
  const chosen = urls().markdown ? "markdown" : urls().typst ? "typst" : "";
  if (!chosen) return Promise.reject(new Error("no renderer is available for word diff"));
  return request(chosen, "diff", { old: oldText || "", new: newText || "" });
}

/// What a document is written in, which follows from what its main file is
/// called. A document is a directory, so the format is a property of one file
/// in it rather than of the whole.
export function formatOf(path) {
  const lower = (path || "").toLowerCase();
  if (lower.endsWith(".typ")) return "typst";
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  if (lower.endsWith(".tex") || lower.endsWith(".ltx")) return "latex";
  return "";
}

/// Renders a document into the page a save would store, and says what the
/// compiler had to say about it.
///
/// `tree` is the whole directory -- `{ main, texts: {path: string}, assets:
/// {path: Uint8Array} }` -- because a paper is a main file, its chapters, its
/// bibliography and its figures, and a compiler given only one of them
/// produces the error a reader would otherwise be shown.
///
/// A document that does not compile is an ordinary state of an editor rather
/// than an error of this loader's, so it resolves with no page and a list of
/// diagnostics; it throws only for what it threw for before, a module that
/// could not be fetched.
///
/// A render carries `html` or `pdf`, never both, and the caller posts
/// whichever it has: flow documents are painted into the shell and paged
/// documents into the PDF frame, and that is the whole difference here.
export async function render(tree, title, { manual = false } = {}) {
  const source = tree.texts?.[tree.main] ?? "";
  const format = formatOf(tree.main);
  // HTML's renderer is the identity, so there is nothing to fetch and nothing
  // that can fail: the source is the page. A directory whose main file is
  // HTML is allowed, and nothing in it is rewritten -- an HTML document is
  // self-contained, as it always was.
  if (format === "html") return { html: source, diagnostics: [] };
  // LaTeX branches before `load`, and not into it. What follows below writes
  // files into a WebAssembly module's memory through `alloc`/`add_file`/
  // `set_main`, an ABI the engine crate exports and a TeX distribution has
  // never heard of; `latex.js` owns its own worker and takes the tree whole.
  // It reads `tree.assets` -- the figures' bytes -- and ignores `tree.urls`
  // entirely, because it writes files into an in-memory filesystem rather
  // than emitting an `<img src>`. That also keeps `/api/documents/<slug>/
  // assets/<sha>` out of anything rendered: on a private document that route
  // needs a credential, and a credential does not belong in a page.
  if (format === "latex") {
    const { pdf, synctex, diagnostics, seconds, log, attempts, provenance, failure, job, ok } = await latex.compile(tree, { manual });
    // Keep the output channels explicit. In particular, a failed LaTeX
    // compile has no HTML page; `undefined` would look like a page to callers
    // that use a null check and could replace a previously good preview.
    return {
      html: null,
      pdf: pdf || null,
      synctex: synctex || null,
      diagnostics: diagnostics || [],
      seconds,
      // The log travels too, for the one case the list is empty and the log
      // is the only account of why there is no PDF.
      log: log || "",
      // `latex.js` may have fallen back to a local or VM backend; the reader
      // shows that provenance and both attempts' logs (SPEC "Failure
      // presentation") rather than the browser-only shape this used to be.
      attempts,
      provenance,
      failure,
      job,
      ok,
    };
  }
  // Checkpoints may be Svelte proxies, which cannot cross a worker boundary.
  // Send only the compiler inputs, copied into ordinary maps.
  const module = format === "markdown" && needsBibliography({ source }) ? "citations" : format;
  return request(module, "render", {
    tree: { main: tree.main, texts: { ...tree.texts }, assets: { ...tree.assets }, urls: { ...tree.urls } },
    title,
  }).then((result) => {
    // The binary worker returns `{pdf, diagnostics}` for Typst. Normalize the
    // owned bytes here so Reader never decodes a PDF through TextDecoder.
    if (format === "typst") {
      const pdf = result?.pdf;
      return {
        ...result,
        html: null,
        pdf: pdf == null ? null : pdf instanceof Uint8Array ? pdf : new Uint8Array(pdf),
        diagnostics: result?.diagnostics || [],
      };
    }
    return result;
  });
}

/// The page to show where a document would be when there is nothing else to
/// show it: what the last compile said, dressed as a document rather than as a
/// crash. The engine builds it, so the command line and a reader can show the
/// same one. Only meaningful straight after a render that produced no page.
export async function failurePage(title, format) {
  // No TeX makes a page out of a log. A LaTeX document that will not compile
  // keeps the last one that did, and shows nothing before there was one --
  // which is what the badge and the pane are for.
  if (producesPdf(format)) return null;
  return request(format, "failure", { title });
}

/// The document's first heading, which names a document that was never given a
/// title of its own. For HTML that is its own `<title>`, or its first `<h1>`.
///
/// The main file's heading, and only its: a chapter's first heading names the
/// chapter, and a document is titled by the file that is the document.
export async function titleOf(tree) {
  const source = tree.texts?.[tree.main] ?? "";
  const format = formatOf(tree.main);
  if (format === "html") return htmlTitleOf(source);
  // The same \title rule the server scans with -- see `title_from_latex` in
  // `render.rs` -- so a document named here and one named by `publish` are
  // named the same. Written twice because there is no shared implementation
  // to reach for: the engine crate has no TeX in it.
  if (format === "latex") return latexTitleOf(source);
  if (!format) return "";
  return request(format, "title", { source });
}

/// What a LaTeX document calls itself: the argument of the first `\title`.
///
/// A scan and not a parse, deliberately, and the same one `render.rs` does:
/// an optional `[short title]` skipped, braces matched rather than counted
/// to the first `}`, `\thanks` and its kind taken with their argument,
/// `\\` a line break, and every other macro dropped with its argument kept.
/// Nothing found is the empty string and the caller falls back to the
/// filename, as it does for every format.
export function latexTitleOf(source) {
  const body = argumentAfter(source || "", "\\title");
  if (body === null) return "";
  let text = body;
  for (const name of ["\\thanks", "\\footnote", "\\footnotemark", "\\label"]) {
    for (;;) {
      const at = text.indexOf(name);
      if (at < 0) break;
      const group = balanced(text.slice(at + name.length));
      text = text.slice(0, at) + text.slice(at + name.length + (group ? group.used : 0));
    }
  }
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") {
      if (text[i + 1] === "\\") i += 1;
      else while (i + 1 < text.length && /[A-Za-z]/.test(text[i + 1])) i += 1;
      out += " ";
    } else if (c === "{" || c === "}" || c === "~") out += " ";
    else if (c === "%") break;
    else out += c;
  }
  return out.split(/\s+/).filter(Boolean).join(" ");
}

/// The braced argument of the first `name`, with an optional `[...]`
/// between the two skipped. `\titlepage` is not `\title`, and a `%`
/// earlier on the line is the only comment this scan notices.
function argumentAfter(source, name) {
  let from = 0;
  for (;;) {
    const start = source.indexOf(name, from);
    if (start < 0) return null;
    let rest = source.slice(start + name.length);
    const line = source.slice(0, start).split("\n").pop();
    if (/^[A-Za-z]/.test(rest) || line.includes("%")) {
      from = start + name.length;
      continue;
    }
    rest = rest.replace(/^\s+/, "");
    if (rest.startsWith("[")) {
      const end = rest.indexOf("]");
      if (end < 0) return null;
      rest = rest.slice(end + 1).replace(/^\s+/, "");
    }
    const group = balanced(rest);
    return group ? group.inner : null;
  }
}

/// The contents of the `{...}` group a string starts with, and how many of
/// its characters the whole group took. Braces nest, so they are matched; a
/// brace an author escaped as `\{` is not one.
function balanced(rest) {
  const trimmed = rest.replace(/^\s+/, "");
  const skipped = rest.length - trimmed.length;
  if (trimmed[0] !== "{") return null;
  let depth = 1;
  for (let i = 1; i < trimmed.length; i++) {
    const c = trimmed[i];
    if (c === "\\") i += 1;
    else if (c === "{") depth += 1;
    else if (c === "}" && --depth === 0) {
      return { inner: trimmed.slice(1, i), used: skipped + i + 1 };
    }
  }
  return null;
}

/// The identity renderer's title scan, which matches the engine's: the
/// `<title>`, or failing that the first `<h1>`, so the landing page, the
/// command line and this browser all name an uploaded file the same way.
export function htmlTitleOf(source) {
  const inside = (open, close) => {
    const lower = source.toLowerCase();
    const start = lower.indexOf(open);
    if (start < 0) return "";
    const after = source.indexOf(">", start);
    if (after < 0) return "";
    const end = lower.indexOf(close, after);
    if (end < 0) return "";
    const parsed = new DOMParser().parseFromString(source.slice(after + 1, end), "text/html");
    return (parsed.body.textContent || "").split(/\s+/).filter(Boolean).join(" ");
  };
  return inside("<title", "</title>") || inside("<h1", "</h1>");
}

/// Starts the download before anyone asks to render, so the wait for the
/// renderer overlaps with reading the document rather than following it.
export function warm(format) {
  if (format === "html") return; // the identity has nothing to fetch
  // Nothing is fetched for LaTeX until a person chooses a distribution. That
  // is the whole of the first rule in `docs/specs/latex.md`, and warming here
  // would break it silently.
  if (format === "latex") return;
  request(format, "warm").catch(() => {
    /* reported when something is actually rendered */
  });
}
