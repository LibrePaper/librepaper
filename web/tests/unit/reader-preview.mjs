import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createFramePreview } from "../../src/lib/reader/frame-preview.js";

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
const viteSource = await readFile(new URL("../../vite.config.js", import.meta.url), "utf8");
const offlineSource = await readFile(new URL("../../src/lib/offline-projects.js", import.meta.url), "utf8");
const persistenceSource = await readFile(new URL("../../src/lib/offline-persistence.js", import.meta.url), "utf8");
const editorSource = await readFile(new URL("../../src/components/Editor.svelte", import.meta.url), "utf8");
// Reader links receive renderer capability before their early prepare return,
// and bind LaTeX to a stable owner across source refreshes.
assert.match(readerSource, /renderers\.offerLatex\([\s\S]*?if \(!mayEdit\) \{/);
assert.match(readerSource, /configureLatex\(sourceFormat, readerProjectOwner\)/);
// The source identity stamped on a finished page is the one captured when
// that render began, never a refreshed project's live identity.
assert.match(previewSource, /const capturedProjectDigest = start\.projectDigest \|\| ""/);
assert.match(previewSource, /onRendered\(identity, capturedProjectDigest\)/);
assert.match(readerSource, /const renderedProjectCurrent = \$derived\.by\(\(\) =>\s*mayEdit \|\|/);
assert.match(readerSource, /shownVerbs = \$derived\.by\(\(\) => pending && renderedProjectCurrent/);
assert.match(readerSource, /current page is still rendering/);
assert.match(viteSource, /let watchingBuild = false;[\s\S]*?configResolved\(config\) \{ watchingBuild = Boolean\(config\.build\.watch\); \}[\s\S]*?if \(watchingBuild\)/);
assert.match(offlineSource, /request\.onblocked = \(\) => \{[\s\S]*?blocked = true;[\s\S]*?reject\(/);
assert.match(persistenceSource, /pendingFlush\.catch\(\(\) => \{\}\)\.finally/);
assert.match(editorSource, /states = new WeakMap\(\);\s*undoManager = null;\s*undoManagerDoc = null/);
assert.match(readerSource, /\["markdown", "quarto"\]\.includes\(displayedFormat\) && buildPreferences\.output === "pdf"[\s\S]*?\? "pdf"/);
assert.match(readerSource, /const frameLoaded = \(\) => \{[\s\S]*?framePreview\.markReady\(\)[\s\S]*?replayPreview\(\)/);
assert.match(readerSource, /<Preview[\s\S]*?onload=\{frameLoaded\}/);
assert.doesNotMatch(readerSource, /toastDone\("(?:Quarto |Calepin )?Preview ready"/);
assert.doesNotMatch(readerSource, /say\(error\.message \|\| "could not render", true\)/);
assert.doesNotMatch(readerSource, /nothing to jump to here/);
// A compile that produced no page still says why, and says it through the
// diagnostics rather than by blanking the frame. The render itself is a
// module now, so this is asked of the module.
assert.match(previewSource, /const failed = hasError[\s\S]*?severity: "error"[\s\S]*?diagnostics\.rendered\(\{ page: false, diagnostics: failed \}\)/);
// Running a document's code is off whenever a document opens and is never
// stored. The switch is the consent, and it is given again each time.
assert.match(readerSource, /let localExecution = \$state\(false\)/);
assert.doesNotMatch(readerSource, /granted\(|local-execution\.js|localExecutionConsent/);
assert.match(readerSource, /async function stopLocalExecution\(\)\s*\{\s*localExecution = false;[\s\S]*?setQuartoPreviewMode\("markdown"\)[\s\S]*?setTypstPreviewMode\("typst"\)/);
// The open-reset effect turns the code off when a document opens, and puts a
// stored build preference back on the browser's renderer.
assert.match(readerSource, /SLUG;[\s\S]*?localExecution = false;[\s\S]*?setQuartoPreviewMode\("markdown"\)[\s\S]*?setTypstPreviewMode\("typst"\)/);
// The switch sits in the top bar and the View menu, and it is the consent.
assert.match(readerSource, /\{#snippet localExecutionControl\(\)\}[\s\S]*?\{#if runCodeOffered\}[\s\S]*?role="switch"[\s\S]*?aria-checked=\{localExecution\}[\s\S]*?onclick=\{toggleLocalExecution\}/);
// The switch is offered for every editable Quarto document, and for an
// editable Typst document only when the desktop app found Calepin.
assert.match(readerSource, /const runCodeOffered = \$derived\([\s\S]*?sourceFormat === "quarto"[\s\S]*?sourceFormat === "typst"[\s\S]*?calepinFound/);
assert.match(readerSource, /const toggleLocalExecution = \(\) => \{\s*if \(localExecution\) void stopLocalExecution\(\);\s*else void startLocalExecution\(\);/);
// There is no confirmation dialog any more.
assert.doesNotMatch(readerSource, /Run code on this computer/);
// A PDF from Markdown or Quarto source is a local build, and the preview
// header -- where the gesture that starts one lives -- is hidden for as long
// as nothing has been painted. So the card standing in for the missing page
// has to carry it itself, rather than claim a browser render is under way
// that can never finish.
assert.match(readerSource, /const pdfNeedsLocalTool = \$derived\(\s*pdfOutput && \["markdown", "quarto"\]\.includes\(sourceFormat\)/);
assert.match(readerSource, /\{#if pdfNeedsLocalTool\}[\s\S]*?<h2 class="h4">PDF needs[\s\S]*?\{#if pdfNeedsLocalExecution\}[\s\S]*?startLocalExecution[\s\S]*?<\/button>[\s\S]*?Preview as HTML instead<\/button>[\s\S]*?\{:else\}\s*<h2 class="h4">(?:\{#if previewBusy\}<span class="spinner"><\/span>\{\/if\})?Not yet rendered<\/h2>/);
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

const frameSource = await readFile(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
assert.match(frameSource, /librepaper-flow img \{[\s\S]*?max-width: 100%;[\s\S]*?height: auto;/);

// A known empty main file is distinct from the pre-hydration placeholder: it
// clears the old payload/export and paints an explicit empty state.
assert.match(previewSource, /if \(!String\(source\.texts\?\.\[source\.main\] \?\? ""\)\.trim\(\)\) \{/);
assert.match(previewSource, /framePreview\.clear\?\.\(\);[\s\S]*?onEmpty\(\);/);
// Project folder setting is shown in the File menu for typst, markdown, and quarto.
assert.match(readerSource, /\{#if mayEdit && \["typst", "markdown", "quarto"\]\.includes\(sourceFormat\)\}[\s\S]*?<Menu\.Item value="project-folder"[\s\S]*?Project folder/);
// HTML/PDF output format choice for markdown and quarto in View menu.
assert.match(readerSource, /\{@const selectableFormat = \["latex", "typst", "markdown", "quarto"\]\.includes\(displayedFormat\)\}/);
assert.match(readerSource, /async function setMarkdownOutput\(format\)/);
