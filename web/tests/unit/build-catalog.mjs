import assert from "node:assert/strict";
import test from "node:test";
import { capabilityFor, operationsFor, supportsOperation } from "../../src/lib/build-catalog.js";

test("v1 preview capability does not imply snapshot build", () => {
  const caps = { builders: [{ id: "quarto", available: true, preview: true }] };
  assert.deepEqual(operationsFor(caps, "quarto"), [{ kind: "preview", workspace_modes: ["bound"] }]);
  assert.equal(supportsOperation(caps, "quarto", "build", "snapshot"), false);
  assert.equal(supportsOperation(caps, "quarto", "preview", "bound"), true);
});

test("legacy capability fields map only proven adapters", () => {
  const caps = { tools: { pdflatex: { available: true }, xelatex: { available: true } }, quarto: { tool: { available: true } }, calepin: { found: true } };
  assert.deepEqual(capabilityFor(caps, "tex").engines, ["pdflatex", "xelatex"]);
  assert.equal(supportsOperation(caps, "tex", "build", "snapshot"), true);
  assert.equal(supportsOperation(caps, "quarto", "preview", "bound"), true);
  assert.equal(supportsOperation(caps, "calepin", "build", "snapshot"), false);
});
