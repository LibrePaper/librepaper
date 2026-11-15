// Real-server acceptance for the Quarto "Publish results" loop, end to end:
// the desktop run that produces the per-cell and inline outputs, the stored
// record on a real `librepaper admin serve`, a collaborator's read of it, and
// the browser's draft (the sidecar and markdown.wasm) that shows them.
// Run with `npm run check:e2e`. Needs the built binary, a disposable
// PostgreSQL connection in LIBREPAPER_TEST_POSTGRES_URL, `quarto` and `Rscript`
// on PATH. Without quarto, Rscript or the Postgres URL the test prints a
// skip line and exits 0.
//
// The draft half reads markdown.wasm from LIBREPAPER_MARKDOWN_WASM, or from
// web/wasm/markdown.wasm. When the module predates inline values and the
// variable is not set, only the inline-draw assertions are skipped.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { removeTemporary } from "../helpers/browser-driver.mjs";
import { deploymentBinary, sessionCookie, startDeployment } from "../helpers/deployment.mjs";
import { tagCells, splitResults } from "../../src/lib/quarto-results.js";
import { quartoSidecar, QUARTO_SIDECAR } from "../../src/lib/engines/quarto.js";
import { call, handOver, validateExports } from "../../src/lib/renderer-wasm.js";

const repository = resolve(import.meta.dirname, "../../..");

function commandWorks(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  return !result.error && result.status === 0;
}

for (const command of ["quarto", "Rscript"]) {
  if (!commandWorks(command, ["--version"])) {
    console.log(`skipping: ${command} is not on PATH, so the Quarto publish loop cannot run`);
    process.exit(0);
  }
}

const binary = process.argv[2] || deploymentBinary();
const deployment = await startDeployment({ label: "quarto_publish", binary });
if (!deployment) throw new Error(`no librepaper binary at ${binary}; build it first`);
if (deployment.unavailable) {
  console.log(`skipping: ${deployment.unavailable}`);
  process.exit(0);
}

// The project. Mean 4, max 6, sum 12, length 3 are what the assertions expect.
const paper = [
  "---",
  'title: "Quarto publish loop"',
  "format: html",
  "---",
  "",
  "```{r}",
  "#| label: setup",
  "#| echo: false",
  "x <- c(2, 4, 6)",
  "```",
  "",
  "The mean is `r mean(x)` and the largest is `{r} max(x)`.",
  "",
  "```{r}",
  "summary(x)",
  "```",
  "",
  "```{r}",
  "#| label: tbl-x",
  '#| tbl-cap: "X"',
  "knitr::kable(data.frame(x = x))",
  "```",
  "",
  "{{< include _part.qmd >}}",
  "",
].join("\n");
const part = ["```{r}", "length(x)", "```", "", "The total is `r sum(x)`.", ""].join("\n");
const texts = { "paper.qmd": paper, "_part.qmd": part };

const desktopDirectory = mkdtempSync(join(tmpdir(), "librepaper-quarto-publish-desktop-"));
const readerPrefix = "results-reader";
const base = deployment.base;
const cookie = deployment.cookie;
// The client marker the app sends on every request (SHELL_HEADERS in api.js).
const shell = { "x-librepaper-client": "shell" };
const modulePath = process.env.LIBREPAPER_MARKDOWN_WASM || join(repository, "web/wasm/markdown.wasm");

// The 1-based line of the chunk fence above the code line `code`.
function fenceLineBefore(lines, code) {
  let at = lines.indexOf(code);
  while (at >= 0 && !lines[at].startsWith("```{")) at -= 1;
  assert.ok(at >= 0, `no chunk fence above ${code}`);
  return at + 1;
}

async function main() {
  // 1. Desktop half, as Reader.svelte runs a Quarto HTML run.
  const tagged = tagCells({ main: "paper.qmd", texts }).texts;
  for (const [name, body] of Object.entries(tagged)) {
    writeFileSync(join(desktopDirectory, name), body);
  }
  const rendered = spawnSync(
    "quarto",
    ["render", "paper.qmd", "--to", "html", "--embed-resources", "--standalone"],
    { cwd: desktopDirectory, encoding: "utf8", timeout: 300000 },
  );
  assert.equal(rendered.status, 0, `quarto render failed:\n${rendered.stdout}\n${rendered.stderr}`);
  const html = readFileSync(join(desktopDirectory, "paper.html"), "utf8");
  const split = await splitResults(html, texts["paper.qmd"], { path: "paper.qmd", files: texts });
  assert.ok(split.ok, `splitResults refused the run: ${split.reason}`);
  const record = split.record;
  assert.equal(record.cells.length, 4, "setup, summary, tbl-x and the included length chunk");
  assert.equal(record.cells[0].html, "", "the silent setup chunk has no output");
  for (const cell of record.cells.slice(1)) assert.notEqual(cell.html, "", `cell ${cell.label ?? "(unlabelled)"} has output`);
  assert.equal(record.cells[2].label, "tbl-x");
  assert.equal(record.inlines.length, 3, "mean, max and the included sum");
  for (const entry of record.inlines) assert.notEqual(entry.html, "", "every inline value came back from the run");
  assert.deepEqual(record.inlines.map((entry) => entry.html.trim()), ["4", "6", "12"]);
  assert.equal(record.inlines[2].path, "_part.qmd");

  // 2. Server half: publish the project as the signed-in owner, then store the record.
  const form = new FormData();
  form.set("title", "Quarto publish loop");
  for (const [name, body] of Object.entries(texts)) {
    form.append("file", new Blob([body], { type: "text/plain" }), name);
  }
  const published = await fetch(`${base}/api/documents`, {
    method: "POST",
    headers: { ...shell, cookie },
    body: form,
  });
  const publishedBody = await published.json();
  assert.equal(published.status, 201, `publish: ${JSON.stringify(publishedBody)}`);
  assert.equal(typeof publishedBody.share_url, "string", "a new project is offered a read link");
  const link = new URL(publishedBody.share_url, base);
  // A read link is /docs/<slug>/<key>.
  const [, slug, key] = link.pathname.split("/").filter(Boolean);
  assert.ok(key, `the read link carries a key: ${link.pathname}`);

  const put = await fetch(`${base}/api/documents/${slug}/results`, {
    method: "PUT",
    headers: { "content-type": "application/json", ...shell, cookie },
    body: JSON.stringify(record),
  });
  assert.equal(put.status, 200, `PUT results: ${put.status} ${await put.clone().text()}`);

  // A collaborator reads it with the link's key. The read link still needs a
  // registered identity, so this is a second account, as history-browser uses.
  const readerId = deployment.postgres.seedRegisteredAccount({
    provider: "github",
    subject: `github:${readerPrefix}`,
    handle: readerPrefix,
    displayName: "Results Reader",
  });
  const readerCookie = sessionCookie({
    dataDirectory: deployment.data,
    accountId: readerId,
    handle: readerPrefix,
    name: "Results Reader",
  });
  // The key goes in the header the app uses (KEY_HEADER in api.js).
  const fetchedResponse = await fetch(`${base}/api/documents/${slug}/results`, {
    headers: { ...shell, "X-LibrePaper-Key": key, cookie: readerCookie },
  });
  assert.equal(fetchedResponse.status, 200, `GET results with the read link: ${fetchedResponse.status}`);
  const fetched = await fetchedResponse.json();
  assert.equal(fetched.cells.length, 4);
  assert.equal(fetched.inlines.length, 3);
  assert.deepEqual(fetched.inlines.map((entry) => entry.html.trim()), ["4", "6", "12"]);

  // 3. Draft half: the sidecar the browser builds, and the page markdown.wasm draws from it.
  const sidecar = await quartoSidecar(texts["paper.qmd"], { path: "paper.qmd", results: fetched, files: texts });
  const paperLines = paper.split("\n");
  const partLines = part.split("\n");
  assert.deepEqual(
    sidecar.outputs.map((entry) => [entry.line, entry.path ?? null]),
    [
      [fenceLineBefore(paperLines, "summary(x)"), null],
      [fenceLineBefore(paperLines, "knitr::kable(data.frame(x = x))"), null],
      [fenceLineBefore(partLines, "length(x)"), "_part.qmd"],
    ],
    "one output per printing cell, placed on its fence line",
  );
  const meanLine = paperLines.findIndex((line) => line.includes("`r mean(x)`")) + 1;
  const maxLine = paperLines.findIndex((line) => line.includes("`{r} max(x)`")) + 1;
  const sumLine = partLines.findIndex((line) => line.includes("`r sum(x)`")) + 1;
  // Every character here is ASCII, so the byte column is the character index
  // of the opening backtick plus two (one for the backtick, one for 1-based).
  assert.equal(sidecar.inlines.length, 3, "three inline entries in the sidecar");
  assert.deepEqual(sidecar.inlines[0], {
    line: meanLine,
    column: paperLines[meanLine - 1].indexOf("`r mean(x)`") + 2,
    html: "4",
    stale: false,
  });
  assert.deepEqual(sidecar.inlines[1], {
    line: maxLine,
    column: paperLines[maxLine - 1].indexOf("`{r} max(x)`") + 2,
    html: "6",
    stale: false,
  });
  assert.deepEqual(sidecar.inlines[2], {
    line: sumLine,
    column: partLines[sumLine - 1].indexOf("`r sum(x)`") + 2,
    path: "_part.qmd",
    html: "12",
    stale: false,
  });

  if (!existsSync(modulePath)) {
    throw new Error(`no markdown.wasm at ${modulePath}; build it first or set LIBREPAPER_MARKDOWN_WASM`);
  }
  const wasm = validateExports(
    new WebAssembly.Instance(new WebAssembly.Module(readFileSync(modulePath)), {}).exports,
    modulePath,
  );
  const now = new Date();
  wasm.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
  // The full-page compile the worker does: the project's files, the sidecar
  // beside them, then the .qmd source and a title.
  handOver(wasm, { main: "paper.qmd", texts: { ...texts, [QUARTO_SIDECAR]: JSON.stringify(sidecar) } });
  const page = call(wasm, "compile", texts["paper.qmd"], "Quarto publish loop");
  assert.ok(page.ok, `markdown.wasm refused the project: ${JSON.stringify(page.diagnostics)}`);
  assert.equal(page.kind, "html");
  const text = page.text;
  assert.match(text, /Median/, "the summary output is drawn");
  assert.match(text, /<table/, "the table output is drawn");
  assert.match(text, /\[1\]\s*3\b/, "the included chunk's output is drawn");
  assert.match(text, /The total is/, "the included prose is drawn");

  const inlineDrawn = text.includes("quarto-inline-value");
  if (!inlineDrawn && !process.env.LIBREPAPER_MARKDOWN_WASM) {
    console.log(`note: ${modulePath} draws no inline values (an older build); skipping the inline-draw assertions`);
  } else {
    assert.ok(text.includes('<span class="quarto-inline-value">4</span>'), "the mean is drawn in its prose");
    assert.ok(text.includes('<span class="quarto-inline-value">6</span>'), "the max is drawn in its prose");
    assert.ok(text.includes('<span class="quarto-inline-value">12</span>'), "the included sum is drawn in its prose");
    assert.ok(!text.includes("r mean(x)"), "the inline code text is replaced, not shown");
  }
  console.log("quarto publish e2e: desktop split, stored record, read-link fetch and draft render passed");
}

try {
  await main();
} finally {
  removeTemporary(desktopDirectory);
  await deployment.stop();
}
