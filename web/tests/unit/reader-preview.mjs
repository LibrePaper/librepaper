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
const previewSource = await readFile(new URL("../../src/lib/reader/preview-render.js", import.meta.url), "utf8");
assert.match(readerSource, /\["markdown", "quarto"\]\.includes\(displayedFormat\) && buildPreferences\.output === "pdf"[\s\S]*?\? "pdf"/);
assert.match(readerSource, /const frameLoaded = \(\) => \{[\s\S]*?framePreview\.markReady\(\)[\s\S]*?replayPreview\(\)/);
assert.match(readerSource, /<Preview[\s\S]*?onload=\{frameLoaded\}/);
assert.doesNotMatch(readerSource, /toastDone\("(?:Quarto |Calepin )?Preview ready"/);
assert.doesNotMatch(readerSource, /say\(error\.message \|\| "could not render", true\)/);
assert.doesNotMatch(readerSource, /nothing to jump to here/);
// A compile that produced no page still says why, and says it through the
// diagnostics rather than by blanking the frame. The render itself is a
// module now, so this is asked of the module.
assert.match(previewSource, /const failed = hasError[\s\S]*?severity: "error"[\s\S]*?diagnostics\.rendered\(\{ page: null, diagnostics: failed \}\)/);
// Running a document's code is one session-scoped choice covering both local
// tools, off every time any document is opened -- including a shared one,
// whose remembered build preference must not turn it on.
assert.match(readerSource, /let localExecution = \$state\(false\)/);
assert.match(readerSource, /async function startLocalExecution\(\)[\s\S]*?localExecution = true;[\s\S]*?setQuartoPreviewMode\("quarto"\)[\s\S]*?setTypstPreviewMode\("calepin"\)/);
assert.match(readerSource, /async function stopLocalExecution\(\)\s*\{\s*localExecution = false;[\s\S]*?setQuartoPreviewMode\("markdown"\)[\s\S]*?setTypstPreviewMode\("typst"\)/);
// It is offered under its own heading in the View menu, with a check mark,
// and choosing it asks rather than acts.
assert.match(readerSource, /menu-section-label">Local execution<\/div>\s*<Menu\.Item value="local-execution"[\s\S]*?\{localExecution \? "✓" : ""\}/);
assert.match(readerSource, /const toggleLocalExecution = \(\) => \{\s*if \(localExecution\) void stopLocalExecution\(\);\s*else localExecutionConsent = true;/);
// The control asks before it runs: the warning is a dialog somebody has to
// answer, not a tooltip a mouse might hover over.
assert.match(readerSource, /\{#snippet previewStatusControl\(\)\}[\s\S]*?sourceFormat === "quarto" && mayEdit && !localExecution\}[\s\S]*?<button[\s\S]*?localExecutionConsent = true[\s\S]*?<\/button>/);
assert.match(readerSource, /<Modal bind:open=\{localExecutionConsent\}[\s\S]*?LOCAL_EXECUTION_WARNING[\s\S]*?>Cancel<\/button>[\s\S]*?startLocalExecution\(\)[\s\S]*?>OK<\/button>/);
assert.match(readerSource, /const LOCAL_EXECUTION_WARNING = "Quarto and Calepin execution can run arbitrary code/);
// A PDF from Markdown or Quarto source is a local build, and the preview
// header -- where the gesture that starts one lives -- is hidden for as long
// as nothing has been painted. So the card standing in for the missing page
// has to carry it itself, rather than claim a browser render is under way
// that can never finish.
assert.match(readerSource, /const pdfNeedsLocalTool = \$derived\(\s*pdfOutput && \["markdown", "quarto"\]\.includes\(sourceFormat\)/);
assert.match(readerSource, /\{#if pdfNeedsLocalTool\}[\s\S]*?<h2 class="h4">PDF needs[\s\S]*?\{#if pdfNeedsLocalExecution\}[\s\S]*?localExecutionConsent = true[\s\S]*?<\/button>[\s\S]*?Preview as HTML instead<\/button>[\s\S]*?\{:else\}\s*<h2 class="h4">Not yet rendered<\/h2>/);
// Choosing Quarto's own preview is a choice of tool, not of output: a PDF
// that was asked for survives enabling the thing that can produce it.
assert.match(readerSource, /async function setQuartoPreviewMode\(mode\)[\s\S]*?output: mode === "markdown" \? "html" : \(buildPreferences\.output \|\| "html"\)/);
// A document nobody approved for execution is drawn by the browser: Quarto
// as the Markdown it is, Typst by this browser's own renderer, whatever tool
// a remembered preference asks for. The condition used to carry a `typeof`
// guard, because the only way to test it was to slice it out of the component
// into a context that had never declared the name; it is an argument now.
assert.match(previewSource, /const executes = format === "quarto" \|\| \["quarto", "calepin"\]\.includes\(current\.buildPreferences\.tool\)/);
assert.match(previewSource, /executes && !current\.localExecution[\s\S]*?backend: "browser", tool: "typst"[\s\S]*?backend: "browser", tool: "markdown"/);
// The paged controls are the preview header's, and the frame is what says
// what they should read: a zoom the frame could not honour -- a pane too
// narrow for 150% -- must not leave the header claiming it did.
assert.match(readerSource, /case "viewer-state":[\s\S]*?viewerView = message\.drawn[\s\S]*?: null;/);
assert.match(readerSource, /tell\(\{ type: "viewer-scale", mode \}\)/);
assert.match(readerSource, /tell\(\{ type: "viewer-tool", tool: next \}\)/);
// No PDF in the frame, no paged controls in the header: that is what keeps
// the row honest for a flowing document, which has none of them.
assert.match(readerSource, /\{#snippet previewControls\(\)\}\s*\{#if viewerView\}/);
const viewerSource = await readFile(new URL("../../src/entries/viewer.js", import.meta.url), "utf8");
// Nothing of the frame's own is drawn over the page any more.
assert.doesNotMatch(viewerSource, /createToolbar/);
assert.match(viewerSource, /type: "viewer-state"[\s\S]*?readerOrigin/);
// The frame answers the window that spoke to it, never "*".
assert.doesNotMatch(viewerSource, /parent\.postMessage\([\s\S]*?"\*"\)/);

const agentSource = await readFile(new URL("../../src/agent/agent.js", import.meta.url), "utf8");
assert.match(agentSource, /librepaper-flow img \{[\s\S]*?max-width: 100%;[\s\S]*?height: auto;/);
