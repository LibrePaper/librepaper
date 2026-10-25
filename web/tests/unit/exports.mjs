import assert from "node:assert/strict";
import { EXPORT_TARGETS, exportAvailability, exportFile, unrunCells } from "../../src/lib/reader/exports.js";

const all = (...formats) => (format) => formats.includes(format);

// Availability: every format against every target.
{
  const rich = { pandoc: true, compiler: all("latex", "typst", "markdown", "quarto") };
  for (const format of ["latex", "typst", "markdown", "quarto", "html"]) {
    for (const target of EXPORT_TARGETS) {
      assert.deepEqual(exportAvailability(format, target, rich), { ok: true }, `${format} to ${target}`);
    }
  }
  for (const format of ["text", ""]) {
    for (const target of EXPORT_TARGETS) assert.equal(exportAvailability(format, target, rich).ok, false);
  }
  const noPandoc = { pandoc: false, compiler: all("latex", "typst", "markdown", "quarto") };
  assert.equal(exportAvailability("latex", "pdf", noPandoc).ok, true);
  assert.equal(exportAvailability("latex", "docx", noPandoc).ok, false);
  assert.equal(exportAvailability("typst", "html", noPandoc).ok, true);
  assert.equal(exportAvailability("typst", "docx", noPandoc).ok, false);
  assert.equal(exportAvailability("markdown", "html", noPandoc).ok, true);
  assert.equal(exportAvailability("markdown", "pdf", noPandoc).ok, false);
  assert.equal(exportAvailability("html", "html", noPandoc).ok, true);
  assert.equal(exportAvailability("html", "pdf", noPandoc).ok, false);
  assert.equal(exportAvailability("html", "docx", noPandoc).ok, false);
  const noLatex = { pandoc: true, compiler: all("typst", "markdown", "quarto") };
  for (const target of EXPORT_TARGETS) assert.equal(exportAvailability("latex", target, noLatex).ok, false);
  assert.equal(exportAvailability("typst", "pdf", noLatex).ok, true);
  // Markdown to PDF needs the Typst renderer, not the Markdown one.
  const onlyMarkdown = { pandoc: true, compiler: all("markdown") };
  assert.equal(exportAvailability("markdown", "pdf", onlyMarkdown).ok, false);
  assert.equal(exportAvailability("markdown", "docx", onlyMarkdown).ok, true);
  const reason = exportAvailability("typst", "pdf", { pandoc: true, compiler: all() }).reason;
  assert.equal(typeof reason, "string");
  assert.equal(exportAvailability("latex", "rtf", rich).ok, false);
}

const pdfBytes = Uint8Array.of(37, 80, 68, 70);
const docxBytes = Uint8Array.of(80, 75);
const makeRun = () => {
  const calls = [];
  const run = async (options, files) => {
    calls.push({ options, files });
    const output = options.to === "docx" ? docxBytes : new TextEncoder().encode("#set page()\n");
    return { output, warnings: [], stderr: "" };
  };
  run.calls = calls;
  return run;
};
const makeRender = (result) => {
  const calls = [];
  const render = async (tree, title, options) => {
    calls.push({ tree, title, options });
    return typeof result === "function" ? result(tree, options) : result;
  };
  render.calls = calls;
  return render;
};
const base = { title: "Paper", slug: "paper", bindingId: "hosted", preview: null, previewCurrent: false };
const mdTree = { main: "doc/main.md", texts: { "doc/main.md": "x" }, assets: { "doc/f.png": Uint8Array.of(1) } };

// A current preview of the right kind is reused without rendering.
{
  const render = makeRender({});
  const file = await exportFile({ ...base, target: "pdf", format: "typst", tree: mdTree, preview: { kind: "pdf", bytes: pdfBytes }, previewCurrent: true, render, run: makeRun() });
  assert.equal(file.name, "paper.pdf");
  assert.equal(file.blob.type, "application/pdf");
  assert.equal(render.calls.length, 0);
  const html = await exportFile({ ...base, target: "html", format: "markdown", tree: mdTree, preview: { kind: "html", html: "<p>x</p>" }, previewCurrent: true, render, run: makeRun() });
  assert.equal(html.name, "paper.html");
  assert.equal(await html.blob.text(), "<p>x</p>");
  assert.equal(render.calls.length, 0);
  // A stale preview is not reused.
  const stale = makeRender({ pdf: pdfBytes, diagnostics: [] });
  await exportFile({ ...base, target: "pdf", format: "latex", tree: mdTree, preview: { kind: "pdf", bytes: Uint8Array.of(9) }, previewCurrent: false, render: stale, run: makeRun() });
  assert.equal(stale.calls.length, 1);
}

// PDF routes.
{
  const render = makeRender({ pdf: pdfBytes, diagnostics: [] });
  await exportFile({ ...base, target: "pdf", format: "latex", tree: mdTree, render, run: makeRun() });
  assert.deepEqual(render.calls[0].options, { format: "pdf", project: "", bindingId: "hosted" });
  await exportFile({ ...base, target: "pdf", format: "quarto", tree: mdTree, render, run: makeRun() });
  assert.deepEqual(render.calls[1].options, { format: "pdf", project: "", bindingId: "hosted", buildPreferences: { backend: "browser", output: "pdf" } });
  // Authored HTML goes through Pandoc to Typst, and the Typst tree is rendered.
  const run = makeRun();
  const htmlTree = { main: "site/index.html", texts: { "site/index.html": "<p>x</p>" }, assets: {} };
  const file = await exportFile({ ...base, target: "pdf", format: "html", tree: htmlTree, render, run });
  assert.equal(run.calls[0].options.from, "html");
  assert.equal(render.calls[2].tree.main, "site/index.pandoc.typ");
  assert.equal(file.name, "paper.pdf");
}

// HTML routes.
{
  const render = makeRender({ html: "<p>made</p>", diagnostics: [] });
  const typst = await exportFile({ ...base, target: "html", format: "typst", tree: mdTree, render, run: makeRun() });
  assert.deepEqual(render.calls[0].options, { format: "html", project: "", bindingId: "hosted" });
  assert.equal(await typst.blob.text(), "<p>made</p>");
  await exportFile({ ...base, target: "html", format: "markdown", tree: mdTree, render, run: makeRun() });
  assert.equal(render.calls[1].options.buildPreferences.output, "html");
  // Authored HTML is exported verbatim, with nothing rendered.
  const untouched = makeRender({});
  const authored = await exportFile({
    ...base, target: "html", format: "html",
    tree: { main: "a.html", texts: { "a.html": "<h1>source</h1>" }, assets: {} },
    preview: { kind: "html", html: "<p>painted</p>" }, previewCurrent: true, render: untouched, run: makeRun(),
  });
  assert.equal(await authored.blob.text(), "<h1>source</h1>");
  assert.equal(untouched.calls.length, 0);
}

// DOCX: math spans are rewritten and the HTML is written beside main.
{
  const html = '<p>a <span data-math-style="inline">x &lt; y</span></p><span data-math-style="display">\\frac{1}{2}</span>';
  const render = makeRender({ html, diagnostics: [] });
  const run = makeRun();
  const file = await exportFile({ ...base, target: "docx", format: "markdown", tree: mdTree, render, run });
  assert.equal(file.name, "paper.docx");
  assert.match(file.blob.type, /wordprocessingml/);
  const { options, files } = run.calls[0];
  assert.equal(options.from, "html");
  assert.equal(options.to, "docx");
  assert.deepEqual(options["input-files"], ["doc/main.pandoc.html"]);
  assert.deepEqual(options["resource-path"], ["doc"]);
  assert.equal(files["doc/main.pandoc.html"],
    '<p>a <math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow></mrow><annotation encoding="application/x-tex">x &lt; y</annotation></semantics></math></p>' +
    '<math xmlns="http://www.w3.org/1998/Math/MathML" display="block"><semantics><mrow></mrow><annotation encoding="application/x-tex">\\frac{1}{2}</annotation></semantics></math>');
  assert.equal(files["doc/main.md"], "x");
  assert.deepEqual(files["doc/f.png"], Uint8Array.of(1));
  // Authored HTML is converted from its own source.
  const authoredRun = makeRun();
  await exportFile({ ...base, target: "docx", format: "html", tree: { main: "a.html", texts: { "a.html": "<p>hi</p>" }, assets: {} }, render: makeRender({}), run: authoredRun });
  assert.equal(authoredRun.calls[0].files["a.pandoc.html"], "<p>hi</p>");
}

// A failed render rejects with a sentence.
{
  const render = makeRender({ html: null, pdf: null, diagnostics: [{ severity: "error", message: "unknown variable" }, { message: "later" }] });
  await assert.rejects(
    exportFile({ ...base, target: "pdf", format: "typst", tree: mdTree, render, run: makeRun() }),
    { message: "The PDF could not be made: unknown variable" },
  );
  await assert.rejects(
    exportFile({ ...base, target: "html", format: "latex", tree: mdTree, render, run: makeRun() }),
    /^Error: The HTML page could not be made: unknown variable$/,
  );
  await assert.rejects(
    exportFile({ ...base, target: "pdf", format: "typst", tree: mdTree, render: makeRender({ pdf: null }), run: makeRun() }),
    { message: "The PDF could not be made." },
  );
}

// An export with the desktop build on takes each file the format has a command for from that command.
{
  const qmdTree = { main: "a.qmd", texts: { "a.qmd": "# t" }, assets: {} };
  const local = async (target, result) => {
    const render = makeRender(result);
    const run = makeRun();
    const file = await exportFile({ ...base, slug: "t", target, format: "quarto", tree: qmdTree, project: "slug-x", render, run, local: true });
    assert.equal(run.calls.length, 0);
    assert.deepEqual(render.calls[0].options, {
      format: target,
      project: "slug-x",
      bindingId: "hosted",
      buildPreferences: { backend: "local", output: target },
    });
    return file;
  };
  const pdf = await local("pdf", { ok: true, pdf: pdfBytes });
  assert.equal(pdf.name, "t.pdf");
  assert.deepEqual(new Uint8Array(await pdf.blob.arrayBuffer()), pdfBytes);
  const html = await local("html", { ok: true, html: "<p>hi</p>" });
  assert.equal(html.name, "t.html");
  assert.equal(await html.blob.text(), "<p>hi</p>");
  const docx = await local("docx", { ok: true, artifact: docxBytes, artifactKind: "docx" });
  assert.equal(docx.name, "t.docx");
  assert.deepEqual(new Uint8Array(await docx.blob.arrayBuffer()), docxBytes);
  await assert.rejects(
    local("pdf", { ok: false, failure: { message: "quarto: boom" } }),
    /quarto: boom/,
  );
  // Without the desktop build the browser route is taken.
  const render = makeRender({ pdf: pdfBytes });
  await exportFile({ ...base, target: "pdf", format: "quarto", tree: qmdTree, render, run: makeRun() });
  assert.deepEqual(render.calls[0].options, {
    format: "pdf",
    project: "",
    bindingId: "hosted",
    buildPreferences: { backend: "browser", output: "pdf" },
  });
}

// A folder project's encoder reaches every render the export makes, and a web project passes none.
{
  const qmdTree = { main: "a.qmd", texts: { "a.qmd": "# t" }, assets: {} };
  const encode = (_path, text) => new TextEncoder().encode(text);
  const render = makeRender({ ok: true, pdf: pdfBytes });
  await exportFile({ ...base, bindingId: "local-1", slug: "t", target: "pdf", format: "quarto", tree: qmdTree, project: "slug-x", render, run: makeRun(), local: true, encode });
  assert.equal(render.calls[0].options.encode, encode, "the local build takes the folder's bytes");
  assert.equal(render.calls[0].options.bindingId, "local-1");
  const web = makeRender({ ok: true, pdf: pdfBytes });
  await exportFile({ ...base, slug: "t", target: "pdf", format: "quarto", tree: qmdTree, project: "slug-x", render: web, run: makeRun(), local: true });
  assert.equal("encode" in web.calls[0].options, false, "a web project passes no encoder");
}

// A format with a command only for some outputs takes the browser route for the rest.
{
  const texTree = { main: "a.tex", texts: { "a.tex": "x" }, assets: {} };
  const render = makeRender({ ok: true, pdf: pdfBytes, html: "<p>hi</p>" });
  const run = makeRun();
  const pdf = await exportFile({ ...base, slug: "t", target: "pdf", format: "latex", tree: texTree, project: "slug-x", render, run, local: true });
  assert.equal(pdf.name, "t.pdf");
  assert.deepEqual(render.calls[0].options, { format: "pdf", project: "slug-x", bindingId: "hosted", buildPreferences: { backend: "local", output: "pdf" } });
  await exportFile({ ...base, slug: "t", target: "html", format: "latex", tree: texTree, project: "slug-x", render, run, local: true });
  assert.deepEqual(render.calls[1].options, { format: "html", project: "slug-x", bindingId: "hosted" });
  assert.equal(run.calls.length, 0);
}

// An export builds where the preview does. A folder project's binding reaches every render, so
// its export runs in the folder and not in the hosted workspace.
{
  const bindingId = "folder-1";
  const bound = { ...base, bindingId, project: "local-folder-1" };
  const rendered = (render) => render.calls.map((call) => call.options.bindingId);
  const render = makeRender({ ok: true, pdf: pdfBytes, html: "<p>made</p>", artifact: docxBytes, artifactKind: "docx", diagnostics: [] });
  const qmdTree = { main: "a.qmd", texts: { "a.qmd": "# t" }, assets: {} };
  const texTree = { main: "a.tex", texts: { "a.tex": "x" }, assets: {} };
  const htmlTree = { main: "site/index.html", texts: { "site/index.html": "<p>x</p>" }, assets: {} };
  // The desktop build.
  for (const target of EXPORT_TARGETS) await exportFile({ ...bound, target, format: "quarto", tree: qmdTree, render, run: makeRun(), local: true });
  assert.deepEqual(rendered(render), [bindingId, bindingId, bindingId], "the local build");
  // The browser route, for each way it renders.
  render.calls.length = 0;
  await exportFile({ ...bound, target: "pdf", format: "latex", tree: texTree, render, run: makeRun() });
  await exportFile({ ...bound, target: "html", format: "latex", tree: texTree, render, run: makeRun() });
  await exportFile({ ...bound, target: "pdf", format: "quarto", tree: qmdTree, render, run: makeRun() });
  await exportFile({ ...bound, target: "html", format: "quarto", tree: qmdTree, render, run: makeRun() });
  await exportFile({ ...bound, target: "pdf", format: "html", tree: htmlTree, render, run: makeRun() });
  await exportFile({ ...bound, target: "docx", format: "quarto", tree: qmdTree, render, run: makeRun() });
  assert.deepEqual(rendered(render), Array(6).fill(bindingId), "the browser route");
  // The caller's own options are kept beside it.
  assert.equal(render.calls[0].options.project, "local-folder-1");
  assert.equal(render.calls[0].options.format, "pdf");
}

// Cells that a browser export leaves unrun.
{
  const source = "# t\n\n```{r}\n1\n```\n\n```{python}\n2\n```\n\n```\nplain\n```\n";
  assert.equal(unrunCells("quarto", { main: "a.qmd", texts: { "a.qmd": source } }), 2);
  assert.equal(unrunCells("latex", { main: "a.tex", texts: { "a.tex": source } }), 0);
}
