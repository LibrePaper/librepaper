import assert from "node:assert/strict";
import { previewNotes } from "../../src/lib/reader/preview-notes.js";

const note = (openPath, target, openIsText = true) => previewNotes({ openPath, openIsText, target });

// --- no notes when the open file previews itself ---------------------------

assert.deepEqual(note("main.tex", { path: "main.tex", reason: "alone" }), [], "a file drawn as itself has no note");

// --- no notes when the preview is locked elsewhere -------------------------

assert.deepEqual(note("notes.dat", { path: "book.tex", reason: "locked" }), [], "the lock chip speaks for a locked preview");

// --- a file with nothing to draw --------------------------------------------

let notes = note("refs/notes.dat", { path: null, reason: "none" });
assert.equal(notes.length, 1, "one note for a file with no preview");
assert.equal(notes[0].severity, "info", "the note is information, never an error");
assert.equal(notes[0].message, "notes.dat has no preview of its own.", "the note names the file");
assert.equal(notes[0].file, "refs/notes.dat", "the note points at the open file");
assert.equal(notes[0].source, "preview");

// --- a file drawn inside its includer ----------------------------------------

notes = note("chapters/intro.tex", { path: "thesis.tex", reason: "inside" });
assert.equal(notes.length, 1, "one note for an included file");
assert.equal(notes[0].message, "intro.tex is shown inside thesis.tex.", "the note names the includer");
assert.equal(notes[0].file, "chapters/intro.tex");

// --- nothing for binary files or no open file --------------------------------

assert.deepEqual(note("image.png", { path: null, reason: "none" }, false), [], "no note when the file is not text");
assert.deepEqual(note("", { path: null, reason: "none" }), [], "no note without an open file");
assert.deepEqual(note(null, { path: null, reason: "none" }), [], "no note without an open path");

console.log("preview notes tests passed");
