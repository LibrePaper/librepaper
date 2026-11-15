import assert from "node:assert/strict";
import { diagnosticContext } from "../../src/lib/assistant-review.js";

const tree = { main: "paper.md", texts: { "paper.md": "first\nold error\nlast" } };
const diagnostic = diagnosticContext({ severity: "error", message: "Invalid", line: 2 }, tree, "captured-sha");
tree.texts["paper.md"] = "new text";
assert.equal(diagnostic.source, "old error");
assert.equal(diagnostic.revision, "captured-sha");
assert.equal(diagnostic.file, "paper.md");

// Several diagnostics on one text share a single split, CRLF and out-of-range lines included.
const crlfTree = { main: "paper.md", texts: { "paper.md": "alpha\r\nbeta\r\ngamma" } };
const crlfSources = [1, 2, 3, 4].map((line) =>
  diagnosticContext({ severity: "warning", message: "Check", line }, crlfTree, "rev").source);
assert.deepEqual(crlfSources, ["alpha", "beta", "gamma", ""]);

// Switching to a different text returns that text's lines, not the cached ones.
const otherTree = { main: "paper.md", texts: { "paper.md": "x\ny" } };
assert.equal(diagnosticContext({ severity: "error", message: "Oops", line: 2 }, otherTree, "rev").source, "y");
assert.equal(diagnosticContext({ severity: "error", message: "Oops", line: 2 }, crlfTree, "rev").source, "beta");
console.log("assistant-review: captured diagnostic context passed");
