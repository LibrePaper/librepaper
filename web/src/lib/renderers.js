import { needsBibliography } from "./bibliography-engine.js";
import { formatOf, previewFormatOf } from "./formats.js";
import { textPage } from "./text-page.js";
import { typstTree } from "./pandoc/typst.js";
import { expandOrgIncludes } from "./org-include.js";
import { orgTreeReady } from "./source-trees.js";
import { parse as parseLatexLog } from "./latex/log.js";
import { parse as parseTypstLog } from "./typst-log.js";
// The renderers, loaded into the editor.
//
// The modules come from the same pinned renderer releases as the native
// command line tools, so the preview follows the document source. Rendering
// stays in this browser or the desktop app and is never stored by
// the deployment. The one exception: when the author turns on Share results,
// the output of a local Quarto HTML run is stored per document and shown in
// the draft of collaborators.
//
// They are plain WebAssembly with a handful of exports rather than
// wasm-bindgen, so this is the whole of the glue: reserve memory in the
// module, write the source into it as UTF-8, call compile, read the page back
// out. The URLs carry a digest of each module's bytes and are handed to the
// page by the server, so a module cached for a year cannot outlive the loader
// that speaks to it.

import * as latex from "./latex.js";
import * as latexHtml from "./latex/html.js";
import * as quarto from "./engines/quarto.js";
import * as localBridge from "./desktop/client.js";
import { logMissesAFile, runDirectoryDiagnostics } from "./reader/run-directory.js";
import { snapshotDigest } from "./projection-digest.js";

import { rendererRequest } from "./renderer-client.js";
import { m } from "./paraglide/messages.js";
import { houseStyleCss, latexmlWithHouseStyle, typstWithHouseStyle } from "./house-style.js";

/// This browser's identity for a render tree -- `{ main, texts, digests }`
/// -- used only as a transient cache key: whether a local build's job
/// matches the source it was started from, and whether a page kept from a
/// previous visit still matches the document open now. It is never sent to
/// the server.
///
/// Re-exported from the leaf that owns the canonical form of §4.4, so this
/// browser has one digest algorithm rather than two. A renderer tree is not
/// a projection -- it has no Loro ids -- but the question is the same one.
export { snapshotDigest };

/// Whether this deployment serves LaTeX distributions, which is the one
/// renderer that is a property of the deployment rather than of the build:
/// the compiler is not in the binary, it is behind `--asset-mirror`, and a
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
  if (format === "quarto") return Boolean(urls().markdown);
  return format === "html" || format === "text" || Boolean(urls()[format]);
}

// The source format and the output painted by the reader are separate
// concerns. Both LaTeX and Typst produce a paged PDF; Markdown, Org and authored
// HTML produce flow HTML. Keeping this mapping here prevents the reader from
// accidentally using LaTeX's compiler chooser as a proxy for PDF support.
export function outputKind(format) {
  if (format === "latex" || format === "typst") return "pdf";
  if (format === "markdown" || format === "quarto" || format === "org" || format === "html" || format === "text") return "html";
  return "";
}

export function producesPdf(format) {
  return outputKind(format) === "pdf";
}

// Authored HTML is displayed from the isolated preview origin, not from the
// directory that contains its source. Point relative display assets at the
// authenticated object URLs gathered for this exact tree. Keep the rewrite
// deliberately narrow: links and nested documents retain their authored
// meaning, while the attributes that load binary display content use the
// same URLs as Markdown's renderer.
export function resolveHtmlAssetUrls(source, tree) {
  const urls = tree.urls || {};
  const base = new URL(tree.main || "index.html", "https://librepaper.invalid/");
  const projectPath = (reference) => {
    if (!reference || reference.startsWith("#") || /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(reference)) return null;
    try {
      const target = new URL(reference, base);
      return target.origin === base.origin ? decodeURIComponent(target.pathname.slice(1)) : null;
    } catch {
      return null;
    }
  };
  const escapedAttribute = (value) => value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  // Project HTML files cannot be navigated to below the preview's /raw/
  // endpoint. Embed them as an iframe document instead, preserving the
  // authored sandbox boundary and avoiding a request that can only be a 404.
  source = source.replace(/<iframe\b([^>]*?)\bsrc\s*=\s*(["'])(.*?)\2([^>]*)>/gi,
    (whole, before, quote, reference, after) => {
      const path = projectPath(reference);
      const nested = path && tree.texts?.[path];
      return typeof nested === "string"
        ? `<iframe${before}srcdoc="${escapedAttribute(nested)}"${after}>`
        : whole;
    });
  const resolve = (reference) => {
    const path = projectPath(reference);
    return path && urls[path] ? urls[path] : reference;
  };
  return source.replace(/\b(src|poster|data)\s*=\s*(["'])(.*?)\2/gi,
    (whole, attribute, quote, reference) => `${attribute}=${quote}${resolve(reference)}${quote}`);
}

/// Seconds the last completed LaTeX HTML compile took, or null before one has.
export function latexHtmlLastSeconds() {
  return latexHtml.lastCompileSeconds();
}

export function cancelPreview(options) {
  latexHtml.cancel(options);
  activeLocalAbort?.abort();
  activeLocalAbort = null;
}

// Whether this browser can compile a source document. Readers without a
// compiler are shown the source and a tool requirement instead of a retained
// result.
export function compilerAvailable(format) {
  return format === "latex" ? latexOffered : available(format);
}

function request(format, operation, args = {}, moduleUrls = urls()) {
  const url = moduleUrls?.[format];
  if (!url) return Promise.reject(new Error(`no renderer for ${format}`));
  return rendererRequest(new URL(url, globalThis.location.href).href, operation, args);
}

let activeLocalAbort = null;

/// The word-level diff used for source projections. It does not depend on
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

export { formatOf, previewFormatOf } from "./formats.js";

// The output a local render of this document produces, or "" when the format
// has no command for what is asked: the commands for LaTeX and Typst make a
// PDF only, so an HTML preview of either stays in the browser.
function localOutput(format, buildPreferences, requestedFormat) {
  if (format === "latex" || format === "typst") return requestedFormat === "html" ? "" : "pdf";
  if (format !== "markdown" && format !== "quarto") return "";
  const chosen = buildPreferences?.output;
  return chosen === "pdf" || chosen === "docx" ? chosen : "html";
}

// A command on this computer returns a log and nothing else about errors, so
// the page reads the diagnostics out of it. Only LaTeX and Typst have a log
// format worth reading; the others show the log itself.
function localDiagnostics(format, result, tree) {
  const paths = [...Object.keys(tree.texts || {}), ...Object.keys(tree.assets || {})];
  if (format === "latex") return parseLatexLog(result.log, { main: tree.main, paths });
  if (format === "typst" && !result.ok) return parseTypstLog(result.log, { paths });
  return [];
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
export async function render(tree, title, { manual = false, format: requestedFormat = "pdf", configuration = null, buildPreferences = null, project = "" } = {}) {
  const source = tree.texts?.[tree.main] ?? "";
  const format = previewFormatOf(tree.main);
  // A format set to this computer renders there, through the command Settings
  // holds for its output. The caller has already turned that off while Run
  // code is off, so a document never runs without consent.
  const output = localOutput(format, buildPreferences, requestedFormat);
  if (output && buildPreferences?.backend === "local") {
    localBridge.configure({ project, origin: globalThis.location?.origin || "" });
    const digest = await snapshotDigest(tree);
    const job = { snapshot: digest, generation: Date.now() };
    activeLocalAbort?.abort();
    const abort = new AbortController();
    activeLocalAbort = abort;
    const result = await localBridge.runRender({ job, tree, format, output, entrypoint: tree.main }, { signal: abort.signal });
    // A Quarto run that could not find a file is told where its code looks.
    const missesAFile = format === "quarto" && !result.ok && logMissesAFile(result.log);
    const runDirectoryPath = missesAFile
      ? await localBridge.runDirectory(project).then((status) => status?.path || null, () => null)
      : null;
    return {
      notices: result.notices || [],
      artifact: result.artifact,
      artifactKind: output,
      html: output === "html" && result.artifact ? new TextDecoder().decode(result.artifact) : null,
      pdf: output === "pdf" ? result.artifact : null,
      synctex: result.synctex,
      diagnostics: [
        ...localDiagnostics(format, result, tree),
        ...runDirectoryDiagnostics({ format, ok: result.ok, log: result.log, notices: result.notices }, runDirectoryPath, [...Object.keys(tree.texts || {}), ...Object.keys(tree.assets || {})]),
      ],
      log: result.log,
      provenance: result.provenance,
      ok: result.ok,
      failure: result.ok ? null : { kind: "local", message: result.error || m.renderers_local_failed({ format }) },
    };
  }
  // A historical render receives a configuration snapshot resolved before
  // its cache lookup. Use its module URLs, not mutable shell globals, so the
  // cache identity names the renderer that actually runs.
  const capturedModules = configuration && Object.prototype.hasOwnProperty.call(configuration, "modules")
    ? configuration.modules : urls();
  // HTML needs no compiler: the source is the page. Resolve its relative
  // binary dependencies because the page itself is displayed from the
  // isolated documents origin rather than from the source directory.
  if (format === "html") return { html: resolveHtmlAssetUrls(source, tree), diagnostics: [] };
  // A text file needs no compiler either. Its source is shown as text.
  if (format === "text") return { html: await textPage(source, tree.main, title || tree.main), diagnostics: [] };
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
    if (requestedFormat === "html") {
      // Historical endpoints carry the settings captured with their tree.
      // Falling back to the live chooser is only for the ordinary preview
      // path, never for a label comparison.
      const settings = configuration?.latex && Object.prototype.hasOwnProperty.call(configuration.latex, "settings")
        ? configuration.latex.settings
        : Object.prototype.hasOwnProperty.call(tree, "settings") ? tree.settings || {} : latex.settings();
      const [result, css] = await Promise.all([
        latexHtml.compile(tree, {
          base: configuration?.latex?.base || latex.at(),
          settings,
          configuration: configuration?.latex,
        }),
        houseStyleCss(),
      ]);
      return result?.html ? { ...result, html: latexmlWithHouseStyle(result.html, css) } : result;
    }
    latex.ensureProject(project);
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
      // Keep provenance and the browser attempt log explicit for diagnostics.
      attempts,
      provenance,
      failure,
      job,
      ok,
    };
  }
  const capturedSettings = configuration && Object.prototype.hasOwnProperty.call(configuration, "settings")
    ? configuration.settings : tree.settings;
  // Markdown and Quarto reach a PDF through Pandoc and Typst: Pandoc writes
  // a Typst source beside the document and the Typst renderer compiles it.
  // Quarto's own diagnostics describe the HTML path and are not mapped here.
  if ((format === "markdown" || format === "quarto") && buildPreferences?.output === "pdf") {
    let pandocInput = tree;
    if (format === "quarto") {
      const drafted = await quartoDraft(tree, 1, capturedModules, capturedSettings).catch((error) => ({ error }));
      if (drafted.error) return quartoFailure(drafted.error, tree, title);
      pandocInput = draftTree(tree, drafted.markdown);
    }
    const typstSource = await typstTree(pandocInput, { from: "markdown" });
    return renderTypst(typstSource, title, "pdf", capturedModules, capturedSettings);
  }
  if (format === "typst") return renderTypst(tree, title, requestedFormat, capturedModules, capturedSettings);
  // The Org renderer does not follow #+INCLUDE, so the included text goes in first.
  if (format === "org") await orgTreeReady(tree.texts?.[tree.main] ?? "");
  if (format === "quarto") {
    try {
      return await renderQuarto(tree, title, capturedModules, capturedSettings, source);
    } catch (error) {
      return quartoFailure(error, tree, title);
    }
  }
  const module = format === "markdown" && needsBibliography({ source }) ? "citations" : format;
  const renderable = format === "org" ? expandOrgIncludes(tree) : tree;
  return request(module, "render", {
    tree: renderTree(renderable, capturedSettings),
    title,
  }, capturedModules);
}

// The Markdown module renders a Quarto document itself: the .qmd is the main
// file and the saved outputs reach it in a file beside the sources. Only a
// document with a bibliography still goes through citations.wasm, which does
// not know Quarto, so that one is given the module's draft as plain Markdown.
async function renderQuarto(tree, title, moduleUrls, settings, source) {
  const sidecar = await quarto.quartoSidecar(source, { path: tree.main, results: tree.quartoResults ?? null });
  const withSidecar = { ...tree, texts: { ...tree.texts, [quarto.QUARTO_SIDECAR]: JSON.stringify(sidecar) } };
  if (needsBibliography({ source })) {
    const drafted = await quartoDraft(withSidecar, 0, moduleUrls, settings);
    const result = await request("citations", "render", {
      tree: renderTree(draftTree(tree, drafted.markdown), settings),
      title,
    }, moduleUrls);
    const mapped = quarto.mapDraftDiagnostics(result?.diagnostics || [], drafted.line_map, tree.main);
    return { ...result, diagnostics: [...quarto.translateQuartoDiagnostics(drafted.diagnostics), ...mapped] };
  }
  const result = await request("markdown", "render", { tree: renderTree(withSidecar, settings), title }, moduleUrls);
  return result ? { ...result, diagnostics: quarto.translateQuartoDiagnostics(result.diagnostics) } : result;
}

// The module's Quarto draft of a tree, as Markdown with its line map.
function quartoDraft(tree, forPandoc, moduleUrls, settings) {
  return request("markdown", "draft", { tree: renderTree(tree, settings), forPandoc }, moduleUrls).then((drafted) => ({
    ...drafted,
    diagnostics: drafted.diagnostics || [],
  }));
}

// The draft stands in for the .qmd, as a Markdown file beside it.
function draftTree(tree, markdown) {
  let main = tree.main.replace(/\.qmd$/i, ".md");
  while (Object.hasOwn(tree.texts || {}, main)) main = main.replace(/\.md$/, "-draft.md");
  const texts = { ...tree.texts, [main]: markdown };
  delete texts[tree.main];
  return { ...tree, main, texts };
}

// A Quarto document the module cannot draft (an older module, or a failure
// inside it) is shown as its source, with the reason beside it.
async function quartoFailure(error, tree, title) {
  const reason = error?.message || String(error);
  return {
    html: await textPage(tree.texts?.[tree.main] ?? "", tree.main, title || tree.main),
    pdf: null,
    diagnostics: [{ severity: "error", message: m.renderers_quarto_failed(), hints: [reason], file: tree.main, line: 0, column: 0, end_line: 0, end_column: 0 }],
  };
}

// Labels may be Svelte proxies, which cannot cross a worker boundary.
// Send only the compiler inputs, copied into ordinary maps.
function renderTree(tree, settings) {
  return {
    main: tree.main,
    texts: { ...tree.texts },
    assets: { ...tree.assets },
    urls: { ...tree.urls },
    // Captured settings travel with historical trees. Current WASM modules
    // may ignore this field, while release-aware adapters can resolve it
    // without changing the worker ABI.
    settings: settings ? { ...settings } : {},
  };
}

// Typst has two compiler entry points. Keep the requested output explicit
// all the way through the worker so dependency retries use the same one.
// The binary worker returns `{pdf, diagnostics}`; normalize the owned bytes
// here so Reader never decodes a PDF through TextDecoder.
async function renderTypst(tree, title, requestedFormat, moduleUrls, settings) {
  const result = await request("typst", "render", {
    tree: renderTree(tree, settings),
    title,
    format: requestedFormat,
  }, moduleUrls);
  if (requestedFormat === "html") {
    const html = result?.html;
    return {
      ...result,
      html: html == null ? null : typstWithHouseStyle(String(html), await houseStyleCss()),
      pdf: null,
      diagnostics: result?.diagnostics || [],
    };
  }
  const pdf = result?.pdf;
  return {
    ...result,
    html: null,
    pdf: pdf == null ? null : pdf instanceof Uint8Array ? pdf : new Uint8Array(pdf),
    diagnostics: result?.diagnostics || [],
  };
}

/// The page to show where a flow document would be when compilation fails.
/// Paged formats leave the frame empty so the Reader can show source and
/// diagnostics without falling back to an older result.
export async function failurePage(title, format) {
  // No TeX makes a page out of a log.
  if (producesPdf(format)) return null;
  // A data file never fails to preview.
  if (format === "text") return null;
  return request(format === "quarto" ? "markdown" : format, "failure", { title });
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
  if (format === "quarto") {
    const parsed = quarto.parseQuarto(source, { path: tree.main });
    const title = parsed.metadata?.title;
    if (typeof title === "string" && title.trim()) return title.trim();
    const heading = source.match(/^ {0,3}#\s+(.+?)\s*#*\s*$/m);
    return heading ? heading[1].replace(/[*_`]/g, "").trim() : "";
  }
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
/// The promise settles once the engine is loaded, which is what the preview's
/// waiting display reports as the engine stage. It never rejects: a failure
/// is reported when something is actually rendered.
export function warm(format, source = "") {
  if (format === "html" || format === "text") return Promise.resolve(); // the identity has nothing to fetch
  // LaTeX is not a renderer-worker format: `latex.js` loads the pinned
  // engine release itself, on the editor's side, and reports its own phase.
  if (format === "latex") {
    // The downloads start now rather than at the first paint. Without a
    // mirror yet, the reader asks again once /api/config has answered.
    if (!latex.at()) return Promise.resolve();
    const settings = latex.settings();
    return settings.output === "pdf" ? latex.warm() : latexHtml.warm(latex.at(), settings);
  }
  const loading = [request(format === "quarto" ? "markdown" : format, "warm")];
  // A document with citations renders in citations.wasm, so it starts at the
  // same time as the Markdown module rather than at the first render.
  if ((format === "markdown" || format === "quarto") && needsBibliography({ source })) {
    loading.push(request("citations", "warm"));
  }
  return Promise.all(loading.map((load) => load.then(() => {}, () => {}))).then(() => {});
}
