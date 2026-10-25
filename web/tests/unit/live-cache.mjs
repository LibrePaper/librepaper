import assert from "node:assert/strict";
import { stamp, matches, pick, shouldStartLive, holding } from "../../src/lib/reader/live-cache.js";

// A page is named by engine, output and source, and has no name before a sync.
assert.equal(stamp("quarto", "html", "abc"), "live:quarto:html:abc");
assert.equal(stamp("quarto", undefined, "abc"), "live:quarto::abc");
assert.equal(stamp("quarto", "html", ""), "");
assert.equal(stamp("", "html", "abc"), "");
assert.notEqual(stamp("quarto", "html", "abc"), stamp("quarto", "pdf", "abc"));
assert.notEqual(stamp("quarto", "html", "abc"), stamp("calepin", "html", "abc"));

// A record matches only its own identity, and never an empty one.
const html = { kind: "html", html: "<p>x</p>", identity: stamp("quarto", "html", "abc") };
assert.equal(matches(html, stamp("quarto", "html", "abc")), true);
assert.equal(matches(html, stamp("quarto", "html", "def")), false);
assert.equal(matches(html, ""), false);
assert.equal(matches({ ...html, identity: "" }, ""), false);
assert.equal(matches(null, "x"), false);
// A browser draft of the same source carries a different identity.
assert.equal(matches({ kind: "html", html: "d", identity: "abc" }, stamp("quarto", "html", "abc")), false);
const pdf = { kind: "pdf", bytes: new Uint8Array([1]), identity: "id" };
assert.equal(matches(pdf, "id"), true);
assert.equal(matches({ kind: "pdf", bytes: new Uint8Array(0), identity: "id" }, "id"), false);

assert.equal(pick([null, { ...html, identity: "no" }, html], html.identity), html);
assert.equal(pick([null], html.identity), null);

// The live preview starts unless the file is held or being looked up.
assert.equal(shouldStartLive({ path: "a.qmd" }), true);
assert.equal(shouldStartLive({ held: "a.qmd", path: "a.qmd" }), false);
assert.equal(shouldStartLive({ lookup: "a.qmd", path: "a.qmd" }), false);
assert.equal(shouldStartLive({ held: "b.qmd", path: "a.qmd" }), true);
assert.equal(shouldStartLive({ path: "" }), false);
assert.equal(holding({ held: "a.qmd", path: "a.qmd" }), true);
assert.equal(holding({ lookup: "a.qmd", path: "a.qmd" }), true);
assert.equal(holding({ held: "", lookup: "", path: "" }), false);
console.log("live-cache ok");
