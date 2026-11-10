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

// Lines are joined with a trailing newline, as a file is.
const lines = (...rows) => `${rows.join("\n")}\n`;
// Compiled: 1 \begin, 2 blank, 3 Alpha, 4 blank, 5 Beta, 6 blank, 7 Gamma, 8 blank, 9 \end.
const page = lines("\\begin{document}", "", "Alpha line.", "", "Beta line.", "", "Gamma line.", "", "\\end{document}");
const other = "Other text.\n";

// (a) Enter in the middle of line 3 splits it. Typing on line 8 of the current
// text, which is line 7 of the compiled text, keeps sending patches.
sent.length = 0;
live.compiled({ "main.tex": page, "a.tex": other });
const split = lines("\\begin{document}", "", "Alpha", "line.", "", "Beta line.", "", "Gamma line.", "", "\\end{document}");
live.edited({ "main.tex": split, "a.tex": other }, "main.tex");
assert.equal(sent.length, 0, "splitting a line sends nothing");
let typed = split;
for (const word of ["Gamma linex.", "Gamma linexy.", "Gamma linexyz.", "Gamma linexyzw."]) {
  typed = typed.replace(/Gamma line[^\n]*\./, word);
  const before = sent.length;
  live.edited({ "main.tex": typed, "a.tex": other }, "main.tex");
  assert.equal(sent.length, before + 1, `typing "${word}" sends a patch`);
  assert.equal(sent.at(-1).patch.line, 7, "line 8 of the current text is line 7 of the compile");
}

// (b) The second half of the split line (line 4 now) sends nothing.
const beforeHalf = sent.length;
typed = typed.replace("line.", "lin.");
live.edited({ "main.tex": typed, "a.tex": other }, "main.tex");
assert.equal(sent.length, beforeHalf, "an edit on the new half of a split line sends nothing");

// (c) Edits above a split keep their number. Here the split is on line 5.
live.compiled({ "main.tex": lines("\\begin{document}", "", "Top line.", "", "Alpha line.", "", "\\end{document}"), "a.tex": other });
const topSplit = lines("\\begin{document}", "", "Top line.", "", "Alpha", "line.", "", "\\end{document}");
const beforeAbove = sent.length;
live.edited({ "main.tex": topSplit, "a.tex": other }, "main.tex");
assert.equal(sent.length, beforeAbove, "splitting a line sends nothing");
live.edited({ "main.tex": topSplit.replace("Top line.", "Top lines."), "a.tex": other }, "main.tex");
assert.equal(sent.length, beforeAbove + 1);
assert.equal(sent.at(-1).patch.line, 3, "a line above the split keeps its number");

// (d) Joining two lines above an edit shifts it the other way. Deleting the
// newline after line 4 (blank) puts "Beta line." on line 4 and removes a line.
live.compiled({ "main.tex": page, "a.tex": other });
const joined = lines("\\begin{document}", "", "Alpha line.", "Beta line.", "", "Gamma line.", "", "\\end{document}");
const beforeJoin = sent.length;
live.edited({ "main.tex": joined, "a.tex": other }, "main.tex");
assert.equal(sent.length, beforeJoin, "joining lines sends nothing");
live.edited({ "main.tex": joined.replace("Gamma line.", "Gamma linee."), "a.tex": other }, "main.tex");
assert.equal(sent.length, beforeJoin + 1);
assert.equal(sent.at(-1).patch.line, 7, "line 6 of the current text is line 7 of the compile");

// (f) A newline in a change to two paths at once is still recorded for main.tex:
// the split is sent nothing, and typing below it is mapped through it.
live.compiled({ "main.tex": page, "a.tex": other });
const beforeBoth = sent.length;
live.edited({ "main.tex": split, "a.tex": "Other texts.\n" }, "main.tex");
assert.equal(sent.length, beforeBoth, "two paths changed at once send nothing");
live.edited({ "main.tex": split.replace("Gamma line.", "Gamma linex."), "a.tex": "Other texts.\n" }, "main.tex");
assert.equal(sent.length, beforeBoth + 1);
assert.equal(sent.at(-1).patch.line, 7, "the split recorded during the two-path change is mapped");

// (e) After compiled(), line numbers are direct again.
sent.length = 0;
live.compiled({ "main.tex": split, "a.tex": other });
live.edited({ "main.tex": split.replace("Gamma", "Gammas"), "a.tex": other }, "main.tex");
assert.equal(sent.length, 1);
assert.equal(sent[0].patch.line, 8, "after the compile, line 8 is line 8");

// reset() forgets everything: nothing is patched until a compile.
sent.length = 0;
live.reset();
live.edited(base, "main.tex");
live.edited({ ...base, "main.tex": doc("The quicxk brown fox jumps.") }, "main.tex");
assert.equal(sent.length, 0);
