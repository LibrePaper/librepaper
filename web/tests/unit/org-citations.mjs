// Org citations: keys in cite groups, insertion, bibliography declarations and
// registration, run against the same helpers the Markdown tests use.
import assert from "node:assert/strict";
import { citedKeys } from "../../src/lib/citation-keys.js";
import { citationContext, insertCitation, selectedBibliographyFiles, bibliographyRegistration } from "../../src/lib/bibliography.js";

const apply = (text, edit) => text.slice(0, edit.from) + edit.insert + text.slice(edit.to);

// --- Keys -------------------------------------------------------------------
const source = [
  "#+TITLE: Test",
  "#+BIBLIOGRAPHY: refs.bib",
  "Some [cite:@doe2020] and [cite/t:see @roe2021 p. 3; @poe_1999].",
  "An @notacite in prose.",
  "#+BEGIN_SRC org",
  "[cite:@insrc]",
  "#+END_SRC",
  "# [cite:@commented]",
  "Code ~[cite:@verbatim]~ here.",
].join("\n");

const found = citedKeys(source, "org");
assert.deepEqual(found.map((cite) => cite.key), ["doe2020", "roe2021", "poe_1999"]);
// Offsets follow the Markdown convention: the key without its "@".
for (const cite of found) assert.equal(source.slice(cite.from, cite.to), cite.key);
assert.deepEqual(citedKeys("[cite:@a;@b]", "org").map((cite) => cite.key), ["a", "b"]);
assert.deepEqual(citedKeys("[cite:@a]", "org"), [{ key: "a", from: 7, to: 8 }]);
// A key without a closing bracket on its line is not a citation.
assert.deepEqual(citedKeys("[cite:@open\nmore @text", "org"), []);
assert.deepEqual(citedKeys("mail a@b.com and [cite:@x]", "org").map((cite) => cite.key), ["x"]);

// --- Context and insertion ---------------------------------------------------
// The key being typed after "@" inside a group is the context.
{
  const text = "See [cite:@do";
  assert.deepEqual(citationContext(text, text.length, "org"), { kind: "org", from: 11, to: 13, query: "do", trigger: "@" });
}
assert.equal(citationContext("See [cite:see text", 17, "org"), null);

// In prose, a new group is inserted at the cursor.
assert.equal(insertCitation("Hello world", 5, "doe2020", "org"), "Hello[cite:@doe2020] world");

// Inside an existing closed group, the key is added before the "]" after a ";".
{
  const text = "See [cite:@a p. 3] now";
  assert.equal(insertCitation(text, text.indexOf("]"), "doe", "org"), "See [cite:@a p. 3;@doe] now");
}
// An open group at the end of the line gets the key without a second ";".
assert.equal(insertCitation("[cite:@a; ", 10, "doe", "org"), "[cite:@a; @doe");
// A partial key after "@" is replaced, as in Markdown.
assert.equal(insertCitation("[cite:@d", 8, "doe2020", "org"), "[cite:@doe2020");

// --- Bibliography declarations ----------------------------------------------
assert.deepEqual(
  selectedBibliographyFiles("#+BIBLIOGRAPHY: refs.bib\n#+bibliography: \"more/other.bib\"\nText", "org"),
  ["more/other.bib", "refs.bib"],
);
assert.deepEqual(selectedBibliographyFiles("Text with no declaration", "org"), []);

// No declaration: the keyword goes after the header's last #+KEY: line.
{
  const text = "#+TITLE: Test\n\nBody [cite:@a]";
  const edit = bibliographyRegistration(text, "refs.bib", "org");
  assert.deepEqual(edit, { from: 14, to: 14, insert: "#+BIBLIOGRAPHY: refs.bib\n" });
  assert.equal(apply(text, edit), "#+TITLE: Test\n#+BIBLIOGRAPHY: refs.bib\n\nBody [cite:@a]");
}
// With no header, the keyword goes at the top.
assert.deepEqual(bibliographyRegistration("Body", "refs.bib", "org"), { from: 0, to: 0, insert: "#+BIBLIOGRAPHY: refs.bib\n" });
// A declared bibliography needs no registration.
assert.equal(bibliographyRegistration("#+BIBLIOGRAPHY: a.bib\nText", "refs.bib", "org"), null);
