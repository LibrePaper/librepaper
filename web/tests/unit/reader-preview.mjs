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
