import assert from "node:assert/strict";
import { previewNotes, unrenderableMain } from "../../src/lib/reader/preview-notes.js";

// --- no notes when file is previewable ----------------------------------------

let notes = previewNotes({
  openPath: "main.tex",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 0, "previewable open file gives no notes");

// --- info note when .bib file is open ----------------------------------------

notes = previewNotes({
  openPath: "refs.bib",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for non-previewable file");
assert.equal(notes[0].severity, "info", "BibTeX note has info severity");
assert.match(
  notes[0].message,
  /BibTeX files have no preview/,
  "BibTeX note names the file type"
);
assert.match(
  notes[0].message,
  /main\.tex/,
  "note names the main file"
);
assert.equal(notes[0].file, "refs.bib", "note has the open file");

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
  openPath: "refs.bib",
  openIsText: true,
  mainPath: "main.xyz",
  formatOf: () => null,
});
assert.equal(notes.length, 1, "one note when main is unrenderable");
assert.equal(notes[0].severity, "warning", "unrenderable main is a warning");

// --- open .json file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "data.json",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .json file");
assert.match(
  notes[0].message,
  /JSON files have/,
  ".json file named as JSON type"
);

// --- open .csv file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "data.csv",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .csv file");
assert.match(
  notes[0].message,
  /CSV files have/,
  ".csv file named as CSV type"
);

// --- open .yaml file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "config.yaml",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .yaml file");
assert.match(
  notes[0].message,
  /YAML files have/,
  ".yaml file named as YAML type"
);

// --- open .yml file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "config.yml",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .yml file");
assert.match(
  notes[0].message,
  /YAML files have/,
  ".yml file named as YAML type"
);

// --- open .cls file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "mystyle.cls",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .cls file");
assert.match(
  notes[0].message,
  /LaTeX class and style files have/,
  ".cls file named as LaTeX class and style"
);

// --- open .sty file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "mystyle.sty",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .sty file");
assert.match(
  notes[0].message,
  /LaTeX class and style files have/,
  ".sty file named as LaTeX class and style"
);

// --- open .lua file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "script.lua",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .lua file");
assert.match(
  notes[0].message,
  /Lua files have/,
  ".lua file named as Lua"
);

// --- open .txt file opens appropriate message ----------------------------------------

notes = previewNotes({
  openPath: "notes.txt",
  openIsText: true,
  mainPath: "main.tex",
  formatOf: (path) => (path.endsWith(".tex") ? "latex" : null),
});
assert.equal(notes.length, 1, "one note for .txt file");
assert.match(
  notes[0].message,
  /Plain text files have/,
  ".txt file named as Plain text"
);

// --- note ends with period when mainPath is empty ----------------------------------------

notes = previewNotes({
  openPath: "refs.bib",
  openIsText: true,
  mainPath: null,
  formatOf: () => null,
});
assert.equal(notes.length, 1, "one note when mainPath is null");
assert.match(
  notes[0].message,
  /of their own\.$/,
  "note ends after 'of their own' when mainPath is empty"
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
