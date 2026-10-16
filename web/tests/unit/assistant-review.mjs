import assert from "node:assert/strict";
import { diagnosticContext } from "../../src/lib/assistant-review.js";

const tree = { main: "paper.md", texts: { "paper.md": "first\nold error\nlast" } };
const diagnostic = diagnosticContext({ severity: "error", message: "Invalid", line: 2 }, tree, "captured-sha");
tree.texts["paper.md"] = "new text";
assert.equal(diagnostic.source, "old error");
assert.equal(diagnostic.revision, "captured-sha");
assert.equal(diagnostic.file, "paper.md");
console.log("assistant-review: captured diagnostic context passed");
