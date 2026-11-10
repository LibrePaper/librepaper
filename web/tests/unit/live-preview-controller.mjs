// createLivePreview: one changed .tex path sends a patch, anything doubtful
// sends nothing.
import assert from "node:assert/strict";
import { createLivePreview } from "../../src/lib/live-preview/controller.js";

const doc = (body) => `\\begin{document}\n\n${body}\n\n\\end{document}\n`;
const sent = [];
const live = createLivePreview({ send: (message) => sent.push(message) });
const base = { "main.tex": doc("The quick brown fox jumps."), "a.tex": "Other text.\n", "notes.md": "hi" };

// One changed path sends a patch.
live.compiled(base);
live.edited({ ...base, "main.tex": doc("The quicxk brown fox jumps.") }, "main.tex");
assert.equal(sent.length, 1);
assert.equal(sent[0].type, "preview-patch");
assert.equal(sent[0].patch.kind, "prose");
assert.equal(sent[0].patch.file, "main.tex");

// Two paths changed at once send nothing.
sent.length = 0;
live.compiled(base);
live.edited({ ...base, "main.tex": doc("The quicxk brown fox jumps."), "a.tex": "Other texts.\n" }, "main.tex");
assert.equal(sent.length, 0);

// A non-tex path is ignored.
live.compiled(base);
live.edited({ ...base, "notes.md": "hi there" }, "main.tex");
assert.equal(sent.length, 0);

// Lines are mapped to the compiled text. The compiled body is on line 3.
const two = (x, y) => doc(`${x}\n\n${y}`);
const lined = { "main.tex": two("First paragraph here.", "Second paragraph here."), "a.tex": "Other text.\n" };
sent.length = 0;
live.compiled(lined);
// Insert a new paragraph line near the top.
const inserted = two("First paragraph here.\n\nNew inserted words.", "Second paragraph here.");
live.edited({ ...lined, "main.tex": inserted }, "main.tex");
assert.equal(sent.length, 0, "the insertion itself sends nothing");
// An edit on a later line is sent with its line shifted back by two.
const later = inserted.replace("Second paragraph", "Seconds paragraph");
live.edited({ ...lined, "main.tex": later }, "main.tex");
assert.equal(sent.length, 1);
assert.equal(sent[0].patch.line, 5, "compiled line of the second paragraph");
// An edit above the insertion keeps its number.
sent.length = 0;
live.edited({ ...lined, "main.tex": later.replace("First paragraph", "Firsts paragraph") }, "main.tex");
assert.equal(sent.length, 1);
assert.equal(sent[0].patch.line, 3);
// An edit on the inserted line sends nothing.
sent.length = 0;
const current = later.replace("First paragraph", "Firsts paragraph");
live.edited({ ...lined, "main.tex": current.replace("inserted words", "inserted wordss") }, "main.tex");
assert.equal(sent.length, 0);
// After compiled(), numbers are direct again.
sent.length = 0;
live.compiled({ ...lined, "main.tex": current.replace("inserted words", "inserted wordss") });
live.edited({ ...lined, "main.tex": current.replace("inserted words", "inserted wordss").replace("Seconds", "Secondss") }, "main.tex");
assert.equal(sent.length, 1);
assert.equal(sent[0].patch.line, 7);

// reset() forgets everything: nothing is patched until a compile.
sent.length = 0;
live.reset();
live.edited(base, "main.tex");
live.edited({ ...base, "main.tex": doc("The quicxk brown fox jumps.") }, "main.tex");
assert.equal(sent.length, 0);
