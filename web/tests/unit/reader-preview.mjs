import assert from "node:assert/strict";
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
