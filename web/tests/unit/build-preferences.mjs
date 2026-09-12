import assert from "node:assert/strict";
import test from "node:test";
import { defaults, read, update } from "../../src/lib/build-preferences.js";

function store() {
  const values = new Map();
  return { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
}

test("build preferences are scoped and automatic clears tool options", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    const selected = update(scope, "latex", { selection: "tool", backend: "local", tool: "latexmk", engine: "xelatex" });
    assert.equal(selected.tool, "latexmk");
    const automatic = update(scope, "latex", { selection: "automatic" });
    assert.deepEqual(automatic, { selection: "automatic", backend: "auto", output: "pdf", format: "latex" });
    assert.equal(read({ ...scope, user: "bob" }, "latex").selection, "automatic");
    assert.deepEqual(defaults("typst"), { selection: "automatic", backend: "auto", output: "pdf", format: "typst" });
  } finally { globalThis.localStorage = previous; }
});

test("tool, preset, and options stay isolated by origin and document", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "github:alice", document: "paper" };
    update(scope, "latex", { selection: "tool", backend: "local", tool: "latexmk", engine: "xelatex", preset: "fast", options: { draft: true } });
    const saved = read(scope, "latex");
    assert.equal(saved.preset, "fast");
    assert.equal(saved.options.draft, true);
    assert.equal(read({ ...scope, origin: "https://other.test" }, "latex").selection, "automatic");
    assert.equal(read({ ...scope, document: "other" }, "latex").selection, "automatic");
  } finally { globalThis.localStorage = previous; }
});

test("provider handles keep same display names isolated", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const alice = { origin: "https://a.test", user: "github:alice", document: "paper" };
    const bob = { origin: "https://a.test", user: "github:bob", document: "paper" };
    update(alice, "latex", { selection: "tool", backend: "browser", tool: "tex", engine: "xelatex" });
    assert.equal(read(bob, "latex").selection, "automatic");
  } finally { globalThis.localStorage = previous; }
});
