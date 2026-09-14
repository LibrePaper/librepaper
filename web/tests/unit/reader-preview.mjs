import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { availableDownloads } from "../../src/lib/reader/downloads.js";
import { createFramePreview } from "../../src/lib/reader/frame-preview.js";

assert.deepEqual(availableDownloads({ outputKind: "pdf" }), { pdf: false, html: false });
assert.deepEqual(availableDownloads({ outputKind: "pdf", deliveredKind: "pdf" }), { pdf: true, html: false });
assert.deepEqual(availableDownloads({ outputKind: "pdf", rendering: { sha: "old" } }), { pdf: false, html: false });
assert.deepEqual(availableDownloads({ outputKind: "html", displayedFormat: "html" }), { pdf: false, html: true });
assert.deepEqual(availableDownloads({ outputKind: "html", rendering: { sha: "old" } }), { pdf: false, html: false });

const sources = [];
const preview = createFramePreview({slug:"paper",getDocsOrigin:()=>"https://docs.example",framePath:()=>"raw",setSource:source=>sources.push(source),send:()=>{}});
assert.equal(preview.navigate(), true);
assert.equal(sources[0], "https://docs.example/raw/paper/?v=1");
assert.equal(preview.navigate(), false);
preview.dispose();

// Reader must choose the PDF frame for a one-shot Quarto PDF build; the frame
// controller deliberately refuses to deliver a PDF to the raw HTML frame.
let path = "pdf";
const messages = [];
const quartoPdf = createFramePreview({
  slug: "paper",
  getDocsOrigin: () => "https://docs.example",
  framePath: () => path,
  setSource: source => sources.push(source),
  send: message => messages.push(message),
});
assert.equal(quartoPdf.navigate(), true);
assert.match(sources.at(-1), /\/pdf\/paper\//);
quartoPdf.markReady();
assert.equal(quartoPdf.publish({ kind: "pdf", bytes: new Uint8Array([1, 2, 3]) }), true);
assert.equal(messages.at(-1).type, "preview");
quartoPdf.dispose();

const readerSource = await readFile(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
assert.match(readerSource, /\["markdown", "quarto"\]\.includes\(displayedFormat\) && buildPreferences\.output === "pdf"[\s\S]*?\? "pdf"/);
assert.match(readerSource, /const frameLoaded = \(\) => \{[\s\S]*?framePreview\.markReady\(\)[\s\S]*?replayPreview\(\)/);
assert.match(readerSource, /<Preview[\s\S]*?onload=\{frameLoaded\}/);
assert.doesNotMatch(readerSource, /toastDone\("(?:Quarto |Calepin )?Preview ready"/);
assert.doesNotMatch(readerSource, /say\(error\.message \|\| "could not render", true\)/);
assert.doesNotMatch(readerSource, /nothing to jump to here/);
assert.match(readerSource, /failureDiagnostics[\s\S]*?severity: "error"[\s\S]*?diagnosticPainter\.rendered\(\{ page: null, diagnostics: failureDiagnostics \}\)/);
assert.match(readerSource, /let quartoExecutionApproved = \$state\(false\)/);
assert.match(readerSource, /async function runQuartoLocally\(\)[\s\S]*?await setQuartoPreviewMode\("quarto"\)[\s\S]*?quartoExecutionApproved = true/);
assert.match(readerSource, /quartoExecutionApproved = true;\n  \}/);
assert.match(readerSource, /Quarto can execute arbitrary code[\s\S]*?Run Quarto locally/);
assert.match(readerSource, /\{#snippet previewStatusControl\(\)\}[\s\S]*?sourceFormat === "quarto" && mayEdit && !quartoExecutionApproved\}[\s\S]*?<button[\s\S]*?runQuartoLocally\(\)[\s\S]*?>Run Quarto locally<\/button>/);
assert.match(readerSource, /format === "quarto" && \(typeof quartoExecutionApproved === "undefined" \|\| !quartoExecutionApproved\)[\s\S]*?backend: "browser", tool: "markdown"/);
const agentSource = await readFile(new URL("../../src/agent/agent.js", import.meta.url), "utf8");
assert.match(agentSource, /librepaper-flow img \{[\s\S]*?max-width: 100%;[\s\S]*?height: auto;/);
