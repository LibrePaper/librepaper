import assert from "node:assert/strict";
import { previewNotes, unrenderableMain } from "../../src/lib/reader/preview-notes.js";
import { previewFormatOf } from "../../src/lib/formats.js";

// --- no notes when file is previewable ----------------------------------------

let notes = previewNotes({
  openPath: "main.tex",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 0, "previewable open file gives no notes");

// --- generic info note when a text file has no preview ------------------------

notes = previewNotes({
  openPath: "notes.dat",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for non-previewable file");
assert.equal(notes[0].severity, "info", "generic note has info severity");
assert.match(
  notes[0].message,
  /^This file has no preview of its own; the preview shows main\.tex\.$/,
  "generic note names the main file"
);
assert.equal(notes[0].file, "notes.dat", "note has the open file");

// --- generic message for unknown extension -----------------------------------

notes = previewNotes({
  openPath: "data.xyz",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for unknown extension");
assert.match(
  notes[0].message,
  /This file has no preview/,
  "unknown extension uses generic message"
);

// --- no note when open file equals main -----------------------------------------------

notes = previewNotes({
  openPath: "main.tex",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 0, "no note when open file equals main");

// --- no note when open file is binary -------------------------------------------

notes = previewNotes({
  openPath: "image.png",
  openIsText: false,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 0, "no note when file is not text");

// --- warning when main has no format ------------------------------------------

notes = previewNotes({
  openPath: "main.xyz",
  openIsText: true,
  mainPath: "main.xyz",
  formatOf: () => null,
});
assert.equal(notes.length, 1, "warning when main file has no format");
assert.equal(notes[0].severity, "warning", "no-format note is a warning");
assert.match(
  notes[0].message,
  /Nothing to preview/,
  "warning mentions nothing to preview"
);
assert.match(
  notes[0].message,
  /main\.xyz/,
  "warning names the main file"
);

// --- no note for main file when open file text but main unrenderable ----

notes = previewNotes({
  openPath: "notes.dat",
  openIsText: true,
  mainPath: "main.xyz",
  formatOf: () => null,
});
assert.equal(notes.length, 1, "one note when main is unrenderable");
assert.equal(notes[0].severity, "warning", "unrenderable main is a warning");

// --- text files with a preview, so no note ------------------------------------

for (const openPath of [
  "data.json", "config.yaml", "config.yml", "config.toml", "notes.txt", "refs.bib",
  "bundle.csl", "script.r", "script.py", "script.jl", "script.lua", "mystyle.sty",
  "mystyle.cls", "mystyle.bst", "site.css", "data.csv", "DATA.CSV",
]) {
  notes = previewNotes({
    openPath,
    openIsText: true,
    mainPath: "main.tex",
    formatOf: previewFormatOf,
  });
  assert.equal(notes.length, 0, `no note for ${openPath}, which previews as text`);
}

// --- a text file with no preview format gets the generic note under the preview mapping ---

notes = previewNotes({
  openPath: "x.foo",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: previewFormatOf,
});
assert.equal(notes.length, 1, "one note for .foo file under the preview mapping");
assert.match(
  notes[0].message,
  /^This file has no preview of its own; the preview shows main\.tex\.$/,
  ".foo file gets the generic message"
);

// --- note ends with period when mainPath is empty ----------------------------------------

notes = previewNotes({
  openPath: "notes.dat",
  openIsText: true,
  mainPath: null,
  formatOf: () => null,
});
assert.equal(notes.length, 1, "one note when mainPath is null");
assert.match(
  notes[0].message,
  /of its own\.$/,
  "note ends after 'of its own' when mainPath is empty"
);

// --- unrenderableMain names the main file when it has no format ----------------

let message = unrenderableMain({
  mainPath: "data.json",
  formatOf: () => null,
});
assert.match(message, /not a format LibrePaper renders/, "unrenderable main gives a message");
assert.match(message, /data\.json/, "message names the main file");

// --- unrenderableMain is empty when the main file renders ----------------------

message = unrenderableMain({
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(message, "", "no message when main file renders");

// --- unrenderableMain is empty when there is no main file ----------------------

message = unrenderableMain({
  mainPath: "",
  formatOf: () => null,
});
assert.equal(message, "", "no message when main path is empty");

console.log("preview notes tests passed");
