import assert from "node:assert/strict";
import { resultEngineOf, draftFormatOf, documentResultsIdentity, validateResultsManifest } from "../src/lib/engines/identity.js";
import { sha256 } from "../src/lib/results-hash.js";
import { resultItems, resultAnchor } from "../src/lib/results-comments.js";
import { createPreviewApi } from "../src/lib/reader/preview-api.js";

const api = createPreviewApi({ slug: "paper", keyHeaders: () => ({}), request: async (url) => url });
// The server routes still live under /quarto/ even though the JS identifiers
// no longer say "quarto".
assert.equal(await api.resultBundle("saved"), "/api/documents/paper/quarto/bundles/saved");

assert.deepEqual(documentResultsIdentity({ execution_engine: "quarto", draft_format: "markdown" }), { execution_engine: "quarto", draft_format: "markdown" });
assert.equal(resultEngineOf({ engine: "quarto" }), "quarto");
assert.equal(draftFormatOf({ draft_format: "typst" }), "typst");
assert.equal(resultEngineOf({ execution_engine: "none", draft_format: "markdown" }), "none");
assert.throws(() => resultEngineOf({ schema: "librepaper-quarto-bundle/v1", render_id: "missing-engine", cells: [] }), /required/i);
assert.throws(() => draftFormatOf({ source_format: "typst" }), /required/i);
assert.throws(() => resultEngineOf({ execution_engine: "calepin" }), /unsupported browser results engine/i);
assert.throws(() => documentResultsIdentity({ execution_engine: "quarto", draft_format: "typst" }), /Markdown draft format/i);
assert.throws(() => validateResultsManifest({ engine: "quarto", assets: [{ path: "runtime.typ", role: "draft-dependency" }] }), /asset role/i);
assert.throws(() => validateResultsManifest({ engine: "quarto", assets: [{ path: "plot.png" }] }), /role is required/i);

const digest = await sha256("current output");
const manifest = { schema: "librepaper-quarto-bundle/v1", engine: "quarto", render_id: "current", cells: [{ id: "main.qmd#cell-1", outputs: [{ ordinal: 0, kind: "text", text: "current output", content_sha256: digest }] }], assets: [] };
const current = resultItems(manifest)[0];
assert.equal(current.digest, digest);
assert.deepEqual(resultAnchor(manifest, current), { render_id: "current", cell_id: "main.qmd#cell-1", output_ordinal: 0, content_sha256: digest, coordinate_system: "percent", width: 0, height: 0 });
console.log("results identity: explicit engines, roles, validation, and comment anchors passed");
