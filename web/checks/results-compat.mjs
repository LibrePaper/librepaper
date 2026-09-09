import assert from "node:assert/strict";
import { resultEngineOf, draftFormatOf, documentResultsIdentity, validateResultsManifest } from "../src/lib/engines/identity.js";
import { sha256 } from "../src/lib/results-hash.js";
import { resultItems as legacyItems, resultAnchor as legacyAnchor } from "../src/lib/quarto-comments.js";
import { resultItems, resultAnchor } from "../src/lib/results-comments.js";
import { createPreviewApi } from "../src/lib/reader/preview-api.js";

const api = createPreviewApi({ slug: "legacy", keyHeaders: () => ({}), request: async (url) => url });
const { quartoBundle } = api;
assert.equal(quartoBundle, api.resultBundle);
assert.equal(await quartoBundle("saved"), "/api/documents/legacy/quarto/bundles/saved");

// Legacy source metadata and old bundles retain their Quarto identity.
assert.deepEqual(documentResultsIdentity({ source_format: "quarto" }), { execution_engine: "quarto", draft_format: "markdown" });
assert.equal(resultEngineOf({ schema: "librepaper-quarto-bundle/v1", render_id: "old", cells: [] }), "quarto");
assert.equal(draftFormatOf({ source_format: "typst" }), "typst");
assert.deepEqual(documentResultsIdentity({ source_format: "" }), { execution_engine: "none", draft_format: "html" });
assert.deepEqual(documentResultsIdentity({}), { execution_engine: "none", draft_format: "html" });
assert.equal(resultEngineOf({ execution_engine: "none", draft_format: "markdown" }), "none");
assert.throws(() => resultEngineOf({ execution_engine: "calepin" }), /unsupported browser results engine/i);
assert.throws(() => documentResultsIdentity({ execution_engine: "quarto", draft_format: "typst" }), /Markdown draft format/i);
assert.throws(() => documentResultsIdentity({ source_format: "quarto", execution_engine: "none", draft_format: "markdown" }), /Legacy Quarto source/i);
assert.throws(() => validateResultsManifest({ engine: "quarto", assets: [{ path: "runtime.typ", role: "draft-dependency" }] }), /asset role/i);

const digest = await sha256("old output");
const manifest = { render_id: "legacy", cells: [{ id: "main.qmd#cell-1", outputs: [{ ordinal: 0, kind: "text", text: "old output", content_sha256: digest }] }], assets: [] };
const current = resultItems(manifest)[0];
assert.deepEqual(legacyItems(manifest), resultItems(manifest));
assert.deepEqual(legacyAnchor(manifest, current), resultAnchor(manifest, current));
console.log("results compatibility: engine defaults, legacy manifest identity, and comment anchors passed");
