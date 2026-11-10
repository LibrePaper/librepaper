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

// A line-count change blocks patches for that path until compiled().
live.compiled(base);
live.edited({ ...base, "main.tex": doc("The quick brown fox jumps.\n") }, "main.tex");
assert.equal(sent.length, 0);
live.edited({ ...base, "main.tex": doc("The quick brown fox jumps.\n") .replace("fox", "foxx") }, "main.tex");
assert.equal(sent.length, 0, "still blocked after the line count moved");
live.compiled({ ...base, "main.tex": doc("The quick brown foxx jumps.\n") });
live.edited({ ...base, "main.tex": doc("The quick brown foxxx jumps.\n") }, "main.tex");
assert.equal(sent.length, 1, "patches resume after compiled()");

// reset() forgets everything: nothing is patched until a compile.
sent.length = 0;
live.reset();
live.edited(base, "main.tex");
live.edited({ ...base, "main.tex": doc("The quicxk brown fox jumps.") }, "main.tex");
assert.equal(sent.length, 0);
