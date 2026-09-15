import assert from "node:assert/strict";
import { createRenderDiagnostics } from "../../src/lib/reader/render-diagnostics.js";

let editing = true;
let local = [];
let visible = [];
let delivered = [];
const diagnostics = createRenderDiagnostics({
  active: () => editing,
  local: () => local,
  update: (list) => (visible = list),
  deliver: (list) => (delivered = list),
  painterOptions: { delay: 0, setTimer: (paint) => { paint(); return 1; }, clearTimer: () => {} },
});

diagnostics.painter.rendered({
  page: null,
  diagnostics: [{ severity: "error", message: "render failed", file: "main.qmd", line: 2 }],
});
assert.deepEqual(visible.map((item) => item.message), ["render failed"]);

diagnostics.bibliography({
  diagnostics: [
    { severity: "warning", message: "missing citation", file: "main.qmd", line: 4 },
    { severity: "error", message: "render failed", file: "main.qmd", line: 2 },
  ],
}, { main: "main.qmd", texts: { "main.qmd": "text" } });
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation"]);

local = [{ severity: "error", message: "local tool unavailable", source: "local-app" }];
diagnostics.refresh();
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation", "local tool unavailable"]);
assert.deepEqual(delivered, visible);

editing = false;
local = [{ severity: "error", message: "hidden" }];
diagnostics.refresh();
assert.deepEqual(visible.map((item) => item.message), ["render failed", "missing citation", "local tool unavailable"]);

console.log("render diagnostics tests passed");
