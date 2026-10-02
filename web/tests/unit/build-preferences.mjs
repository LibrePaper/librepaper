import assert from "node:assert/strict";
import test from "node:test";
import { defaults, read, update, readQuartoOptions, writeQuartoOptions } from "../../src/lib/build-preferences.js";

function store() {
  const values = new Map();
  return { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
}

test("build preferences are scoped and automatic clears tool options", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    const selected = update(scope, "latex", { selection: "tool", backend: "browser", tool: "tex", engine: "xelatex" });
    assert.equal(selected.tool, "tex");
    const automatic = update(scope, "latex", { selection: "automatic" });
    assert.deepEqual(automatic, { selection: "automatic", backend: "auto", output: "html", format: "latex" });
    assert.equal(read({ ...scope, user: "bob" }, "latex").selection, "automatic");
    // Every format starts on HTML, the paged ones included.
    assert.deepEqual(defaults("typst"), { selection: "automatic", backend: "auto", output: "html", format: "typst" });
    assert.deepEqual(defaults("markdown"), { selection: "automatic", backend: "auto", output: "html", format: "markdown" });
  } finally { globalThis.localStorage = previous; }
});

test("a paged output lasts the visit and is not read back", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    const chosen = update(scope, "typst", { output: "pdf" });
    // The caller renders with what it was handed, so the choice it just made
    // comes back.
    assert.equal(chosen.output, "pdf");
    // Entering the project again is entering it on HTML, whatever was chosen
    // last time -- and the tool beside it is still remembered.
    update(scope, "latex", { selection: "tool", backend: "browser", tool: "tex", engine: "xelatex", output: "pdf" });
    const again = read(scope, "latex");
    assert.equal(again.output, "html");
    assert.equal(again.engine, "xelatex");
  } finally { globalThis.localStorage = previous; }
});

test("preferences shared across documents, isolated by origin and format", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "github:alice", document: "paper" };
    update(scope, "latex", { selection: "tool", backend: "browser", tool: "tex", engine: "xelatex", options: { draft: true } });
    const saved = read(scope, "latex");
    assert.equal(saved.options.draft, true);
    // Same origin and user with different document: preference is shared
    assert.equal(read({ ...scope, document: "other" }, "latex").selection, "tool");
    assert.equal(read({ ...scope, document: "other" }, "latex").engine, "xelatex");
    // Different origin: preference is isolated
    assert.equal(read({ ...scope, origin: "https://other.test" }, "latex").selection, "automatic");
    // Different format: preference is isolated
    assert.equal(read(scope, "typst").selection, "automatic");
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

test("quarto options are shared between markdown and quarto, not stored in per-format record", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    // Write quarto options
    const opts = writeQuartoOptions(scope, { profile: "test-profile", parameters: { key: "value" } });
    assert.deepEqual(opts, { profile: "test-profile", parameters: { key: "value" } });
    // Read markdown: quarto options are merged in
    const markdown = read(scope, "markdown");
    assert.equal(markdown.profile, "test-profile");
    assert.deepEqual(markdown.parameters, { key: "value" });
    // Read quarto: quarto options are merged in
    const quarto = read(scope, "quarto");
    assert.equal(quarto.profile, "test-profile");
    assert.deepEqual(quarto.parameters, { key: "value" });
    // Choosing a renderer keeps them in what comes back, and the per-format
    // record does not hold its own copy.
    const chosen = update(scope, "quarto", { selection: "tool", backend: "local", tool: "quarto" });
    assert.equal(chosen.profile, "test-profile");
    const raw = JSON.parse(globalThis.localStorage.getItem(`librepaper-build-v3:${JSON.stringify(["https://a.test", "alice", "quarto"])}`));
    assert.equal(raw.tool, "quarto");
    assert.equal(raw.profile, undefined);
    assert.equal(raw.parameters, undefined);
  } finally { globalThis.localStorage = previous; }
});

test("local typst preference reads back as browser typst", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    // Simulate a stored old preference with backend "local" and tool "typst"
    const key = `librepaper-build-v3:${JSON.stringify(["https://a.test", "alice", "typst"])}`;
    globalThis.localStorage.setItem(key, JSON.stringify({
      selection: "tool",
      backend: "local",
      tool: "typst",
    }));
    // Read it back
    const preference = read(scope, "typst");
    assert.equal(preference.backend, "browser");
    assert.equal(preference.tool, "typst");
    assert.equal(preference.selection, "tool");
  } finally { globalThis.localStorage = previous; }
});
