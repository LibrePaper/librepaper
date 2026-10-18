// The diagnostics report: route patterns, message scrubbing, and the canary
// check that a marker placed in every private place never reaches the report.
import assert from "node:assert/strict";

const MARKER = "zqxcanary";
const values = new Map();
const define = (name, value) => Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });

define("localStorage", {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, value),
  get length() { return values.size; },
  key: (index) => [...values.keys()][index] ?? null,
});
define("location", { origin: "https://app.example.org", pathname: `/docs/${MARKER}-slug`, hash: "" });
define("navigator", { userAgent: "TestAgent/1.0", language: "en", onLine: true, platform: "TestOS" });
define("screen", { width: 1920, height: 1080 });
define("innerWidth", 1000);
define("innerHeight", 700);
define("devicePixelRatio", 2);
define("fetch", async () => ({ json: async () => ({ version: "9.9.9" }) }));

const { routePattern, scrubMessage } = await import("../../src/lib/support/redact.js");
const { report } = await import("../../src/lib/crash.js");
const { watchNetwork } = await import("../../src/lib/support/network.js");
const { provide } = await import("../../src/lib/support/providers.js");
const { collectReport, formatReport } = await import("../../src/lib/support/report.js");

assert.equal(routePattern(`/api/documents/my-thesis-x7k2/comments?key=abc`), "/api/documents/:slug/comments");
assert.equal(routePattern("/docs/my-thesis/"), "/docs/:slug/");
assert.equal(routePattern("/assets/index-AbC123.js"), "/assets/index-AbC123.js");
assert.equal(routePattern("https://other.example.com/api/files/a.tex?k=1#x"), "other.example.com/api/:id/:file");
assert.equal(routePattern("/api/items/12345"), "/api/:id/:id");
// A folder name chosen by somebody never survives, with or without digits.
assert.equal(routePattern("/docs/my-thesis/chapters-on-grief/intro.tex"), "/docs/:slug/:id/:file");
assert.equal(routePattern("/ws/my-thesis"), "/ws/:slug");

assert.equal(scrubMessage('Unexpected token in "secret words"'), 'Unexpected token in "..."');
assert.ok(!scrubMessage("bad lp_abcdef123456 token").includes("abcdef"));
assert.ok(!scrubMessage("mail me at person@example.com now").includes("person"));
assert.ok(!scrubMessage("failed at /home/vincent/thesis/main.tex").includes("vincent"));
assert.ok(!scrubMessage("failed at C:\\Users\\vincent\\main.tex").includes("vincent"));
assert.equal(scrubMessage("word ".repeat(100), 120).length, 123);
assert.ok(scrubMessage("word ".repeat(100), 120).endsWith("..."));

const text = formatReport([{ title: "One", lines: ["plain", ["key", "value"]] }]);
assert.ok(text.startsWith("LibrePaper diagnostics\n"));
assert.ok(text.includes("## One\nplain\nkey: value\n"));

provide("document", () => ({ id: `doc-${MARKER}`, format: "markdown", role: "owner", mayEdit: true }));
provide("account", () => ({ id: `acct-${MARKER}`, signedIn: true }));
provide("engines", () => ({ format: "latex", compiling: false, latex: { phase: "done", ok: true, diagnostics: 2, errors: 1 } }));
provide("buildLog", () => `compiling ${MARKER}\nline two`);

report(new Error(`Unexpected token in "${MARKER} paragraph"`), "the page");
report(new Error(MARKER), "the editor");

const fake = {
  fetch: async () => ({ status: 404, ok: false, headers: new Headers({ "x-request-id": "r1" }) }),
};
const original = fake.fetch;
const undo = watchNetwork(fake);
assert.notEqual(fake.fetch, original);
await fake.fetch(`/api/documents/${MARKER}-slug/files/${MARKER}.tex?key=lp_${MARKER}`);
undo();
assert.equal(fake.fetch, original);

values.set(`librepaper-build-v3:${JSON.stringify(["https://app.example.org", `user-${MARKER}`, "latex"])}`, JSON.stringify({ selection: "tool", backend: "browser" }));
values.set("librepaper-build-v3:anonymous-id", `anon-${MARKER}`);
values.set("librepaper-keymap", JSON.stringify("vim"));
values.set("librepaper-keys", JSON.stringify({ [`${MARKER}-slug`]: `lp_${MARKER}` }));

const plain = await collectReport();
assert.ok(!plain.includes(MARKER), "the report leaks the marker");
assert.ok(plain.includes("## Recent problems"));
assert.ok(plain.includes("id=r1"));
assert.ok(plain.includes("Keymap: vim"));
assert.ok(plain.includes("Build latex: selection=tool backend=browser"));

const full = await collectReport({ includeBuildLog: true });
assert.ok(full.includes(MARKER), "the build log was asked for");

console.log("support-report: all checks passed");
