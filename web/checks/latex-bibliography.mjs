// `latex/bibliography.js` over hand-written aux/bcf/log fixtures, the way
// SPEC "Local Biber operation" step 3 and interfaces.md section 2.8 step 3
// insist detection has to work: real files and diagnostics, not a source
// regex. Small enough to type inline; that is the point of keeping the
// module pure.
import assert from "node:assert/strict";
import * as bibliography from "../src/lib/latex/bibliography.js";

const enc = new TextEncoder();

// --- Classic BibTeX --------------------------------------------------------
{
  const aux = enc.encode('\\relax\n\\bibstyle{plain}\n\\bibdata{refs}\n\\citation{knuth1984}\n');
  const outputs = { "main.aux": aux };
  const log = "LaTeX Warning: Citation `knuth1984' on page 1 undefined on input line 3.\n";
  const tree = { main: "main.tex", texts: { "main.tex": "" }, assets: { "refs.bib": enc.encode("@book{knuth1984,}"), "plain.bst": enc.encode("") } };
  const found = bibliography.inspect({ stem: "main", outputs, log, tree });
  assert.equal(found.bibtex, true);
  assert.equal(found.bibtex8, false);
  assert.equal(found.biber, false);
  assert.deepEqual(found.bibFiles, ["refs.bib"]);
  assert.deepEqual(found.styleFiles, ["plain.bst"]);
}

// A resolved .bbl and no undefined-citation warning: nothing left to do.
{
  const aux = enc.encode("\\bibdata{refs}\n\\citation{knuth1984}\n");
  const outputs = { "main.aux": aux, "main.bbl": enc.encode("\\begin{thebibliography}...\\end{thebibliography}") };
  const tree = { main: "main.tex", texts: {}, assets: { "refs.bib": enc.encode("") } };
  const found = bibliography.inspect({ stem: "main", outputs, log: "", tree });
  assert.equal(found.bibtex, false);
}

// --- Biber (a .bcf and the log's own request line) --------------------------
{
  const bcf =
    '<?xml version="1.0"?><bcf:controlfile>' +
    '<bcf:datasource type="file" datatype="bibtex">references.bib</bcf:datasource>' +
    "</bcf:controlfile>";
  const outputs = { "main.aux": enc.encode("\\relax\n"), "main.bcf": enc.encode(bcf) };
  const log = "Package biblatex Warning: Please (re)run Biber on the file: main\n";
  const tree = { main: "main.tex", texts: {}, assets: { "references.bib": enc.encode("") } };
  const found = bibliography.inspect({ stem: "main", outputs, log, tree });
  assert.equal(found.biber, true);
  assert.equal(found.bibtex, false);
  assert.deepEqual(found.bibFiles, ["references.bib"]);
  assert.ok(found.bcf);
}

// --- Nested \@input aux files -------------------------------------------
{
  const main = enc.encode("\\relax\n\\@input{chapters/one.aux}\n");
  const nested = enc.encode("\\bibdata{shared}\n\\citation{a}\n");
  const outputs = { "main.aux": main, "chapters/one.aux": nested };
  const log = "No file main.bbl.\n";
  const tree = { main: "main.tex", texts: {}, assets: { "shared.bib": enc.encode("") } };
  const found = bibliography.inspect({ stem: "main", outputs, log, tree });
  assert.deepEqual(found.auxPaths.sort(), ["chapters/one.aux", "main.aux"]);
  assert.equal(found.bibtex, true);
  assert.deepEqual(found.bibFiles, ["shared.bib"]);
}

// --- Project-local bib beside a nested main file ----------------------------
{
  const aux = enc.encode("\\bibdata{local}\n\\citation{a}\n");
  const outputs = { "paper/main.aux": aux };
  const log = "No file paper/main.bbl.\n";
  const tree = { main: "paper/main.tex", texts: {}, assets: { "paper/local.bib": enc.encode("") } };
  const found = bibliography.inspect({ stem: "paper/main", outputs, log, tree });
  assert.deepEqual(found.bibFiles, ["paper/local.bib"]);
}

// --- makeindex and configuration/style files --------------------------------
{
  const outputs = { "main.aux": enc.encode(""), "main.idx": enc.encode("\\indexentry{alpha}{1}\n") };
  const tree = {
    main: "main.tex",
    texts: { "main.tex": "" },
    assets: { "biber.conf": enc.encode(""), "custom.bbx": enc.encode(""), "custom.cbx": enc.encode("") },
  };
  const found = bibliography.inspect({ stem: "main", outputs, log: "", tree });
  assert.equal(found.makeindex, true);
  assert.deepEqual(found.configFiles.sort(), ["biber.conf", "custom.bbx", "custom.cbx"]);
}

// --- Identity: must change on citation, .bib byte, .bst, engine, release,
// tool; must NOT change for a prose-only edit (none of those inputs move). --
{
  const bcfA = enc.encode('<bcf:datasource type="file">refs.bib</bcf:datasource>');
  const filesA = { "refs.bib": enc.encode("@book{a,}"), "plain.bst": enc.encode("style-a") };
  const base = { kind: "biber", controlBytes: bcfA, files: filesA, engine: "pdflatex", release: "r1", tool: "biber" };

  const id1 = await bibliography.identity(base);
  const id1Again = await bibliography.identity({ ...base, controlBytes: enc.encode(bcfA.toString()) });
  assert.equal(id1, await bibliography.identity(base), "identical input is deterministic");

  // A prose-only edit changes neither the control bytes nor any bib/style
  // file: same identity, so the bbl is reused and Biber does not rerun.
  const proseIdentity = await bibliography.identity(base);
  assert.equal(proseIdentity, id1);

  // A citation change shows up in the bcf (a new datasource entry or a
  // different citation set changes biblatex's own bcf bytes).
  const bcfB = enc.encode('<bcf:datasource type="file">refs.bib</bcf:datasource><!--cite:b-->');
  const idCitation = await bibliography.identity({ ...base, controlBytes: bcfB });
  assert.notEqual(idCitation, id1);

  // A changed .bib byte.
  const idBibByte = await bibliography.identity({ ...base, files: { ...filesA, "refs.bib": enc.encode("@book{a,note={x}}") } });
  assert.notEqual(idBibByte, id1);

  // A changed .bst.
  const idStyle = await bibliography.identity({ ...base, files: { ...filesA, "plain.bst": enc.encode("style-b") } });
  assert.notEqual(idStyle, id1);

  // Engine, release and tool each move the identity independently.
  assert.notEqual(await bibliography.identity({ ...base, engine: "xelatex" }), id1);
  assert.notEqual(await bibliography.identity({ ...base, release: "r2" }), id1);
  assert.notEqual(await bibliography.identity({ ...base, tool: "biber-2.20" }), id1);

  void id1Again;
}

console.log("latex bibliography: detection and identity fixtures passed");
