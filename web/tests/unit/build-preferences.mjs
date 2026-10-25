import assert from "node:assert/strict";
import test from "node:test";
import { defaults, read, update, readDefaultPreview, writeDefaultPreview } from "../../src/lib/build-preferences.js";

function store() {
  const values = new Map();
  return { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
}

const keyOf = (origin, user, format) => `librepaper-build-v3:${JSON.stringify([origin, user, format])}`;

test("build preferences are scoped and automatic clears the engine", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    const selected = update(scope, "latex", { selection: "tool", engine: "xelatex" });
    assert.equal(selected.engine, "xelatex");
    assert.equal(selected.tool, undefined);
    const automatic = update(scope, "latex", { selection: "automatic" });
    assert.deepEqual(automatic, { selection: "automatic", backend: "browser", output: "html", format: "latex" });
    assert.equal(read({ ...scope, user: "bob" }, "latex").selection, "automatic");
    // Every format starts on HTML, the paged ones included, and in the browser.
    assert.deepEqual(defaults("typst"), { selection: "automatic", backend: "browser", output: "html", format: "typst" });
    assert.deepEqual(defaults("markdown"), { selection: "automatic", backend: "browser", output: "html", format: "markdown" });
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
    // last time, and the engine beside it is still remembered.
    update(scope, "latex", { selection: "tool", engine: "xelatex", output: "pdf" });
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
    update(scope, "latex", { selection: "tool", engine: "xelatex", options: { draft: true } });
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
    update(alice, "latex", { selection: "tool", engine: "xelatex" });
    assert.equal(read(bob, "latex").selection, "automatic");
  } finally { globalThis.localStorage = previous; }
});

test("each format renders in the browser or on this computer", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    for (const format of ["latex", "typst", "markdown"]) {
      assert.equal(read(scope, format).backend, "browser");
      assert.equal(update(scope, format, { backend: "local" }).backend, "local");
      assert.equal(read(scope, format).backend, "local");
      assert.equal(update(scope, format, { backend: "browser" }).backend, "browser");
    }
    // The choice belongs to the format, the person and the origin.
    update(scope, "typst", { backend: "local" });
    assert.equal(read(scope, "latex").backend, "browser");
    assert.equal(read({ ...scope, user: "bob" }, "typst").backend, "browser");
    assert.equal(read({ ...scope, origin: "https://other.test" }, "typst").backend, "browser");
    // A value that is neither choice reads back as the browser.
    globalThis.localStorage.setItem(keyOf("https://a.test", "alice", "markdown"), JSON.stringify({ selection: "automatic", backend: "somewhere" }));
    assert.equal(read(scope, "markdown").backend, "browser");
    // Other formats have no computer to render on.
    assert.equal(update(scope, "html", { backend: "local" }).backend, "browser");
  } finally { globalThis.localStorage = previous; }
});

test("quarto has no browser engine so it is always local", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    assert.equal(defaults("quarto").backend, "local");
    assert.equal(read(scope, "quarto").backend, "local");
    assert.equal(update(scope, "quarto", { backend: "browser" }).backend, "local");
  } finally { globalThis.localStorage = previous; }
});

test("choosing the engine or going automatic keeps where the format renders", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    update(scope, "latex", { backend: "local" });
    assert.equal(update(scope, "latex", { selection: "tool", engine: "xelatex" }).backend, "local");
    assert.equal(update(scope, "latex", { selection: "automatic" }).backend, "local");
    assert.equal(read(scope, "latex").backend, "local");
  } finally { globalThis.localStorage = previous; }
});

test("records from before the tool choice read back by where they render", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    // The old tool name is dropped, and the old "auto" is the browser.
    globalThis.localStorage.setItem(keyOf("https://a.test", "alice", "markdown"), JSON.stringify({ selection: "tool", backend: "local", tool: "quarto" }));
    const markdown = read(scope, "markdown");
    assert.equal(markdown.backend, "local");
    assert.equal(markdown.tool, undefined);
    globalThis.localStorage.setItem(keyOf("https://a.test", "alice", "latex"), JSON.stringify({ selection: "automatic", backend: "auto", tool: "tex" }));
    const latex = read(scope, "latex");
    assert.equal(latex.backend, "browser");
    assert.equal(latex.tool, undefined);
  } finally { globalThis.localStorage = previous; }
});

test("the profile and parameters are no longer kept", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    const chosen = update(scope, "quarto", { output: "pdf" });
    assert.equal(chosen.profile, undefined);
    assert.equal(chosen.parameters, undefined);
    const raw = JSON.parse(globalThis.localStorage.getItem(keyOf("https://a.test", "alice", "quarto")));
    assert.equal("profile" in raw, false);
    assert.equal("parameters" in raw, false);
  } finally { globalThis.localStorage = previous; }
});

test("the default preview is stored per format and the View menu choice is not", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    assert.equal(readDefaultPreview(scope, "latex"), "html");
    const chosen = writeDefaultPreview(scope, "latex", "pdf");
    assert.equal(chosen.output, "pdf");
    assert.equal(read(scope, "latex").output, "pdf");
    assert.equal(read(scope, "typst").output, "html");
    assert.equal(read({ ...scope, user: "bob" }, "latex").output, "html");
    // The View menu choice lasts for the visit: it is returned but not stored.
    assert.equal(update(scope, "latex", { output: "html" }).output, "html");
    assert.equal(read(scope, "latex").output, "pdf");
  } finally { globalThis.localStorage = previous; }
});

test("a stored default preview survives changing the engine and going automatic", () => {
  const previous = globalThis.localStorage;
  globalThis.localStorage = store();
  try {
    const scope = { origin: "https://a.test", user: "alice", document: "paper" };
    writeDefaultPreview(scope, "latex", "pdf");
    const tool = update(scope, "latex", { selection: "tool", engine: "xelatex" });
    assert.equal(tool.output, "pdf");
    const automatic = update(scope, "latex", { selection: "automatic" });
    assert.equal(automatic.output, "pdf");
  } finally { globalThis.localStorage = previous; }
});
