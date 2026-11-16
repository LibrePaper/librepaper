// Builds the BibTeX parser from its Lezer grammar.
//
// Reads src/lib/bibtex-language/bibtex.grammar and writes parser.js and
// parser.terms.js beside it. Run it after editing the grammar:
//
//   node tools/bibtex-grammar.mjs
//
// The generated files are checked in, so a normal build does not need the
// generator at all.
import { readFileSync, writeFileSync } from "node:fs";
import { buildParserFile } from "@lezer/generator";

const dir = new URL("../src/lib/bibtex-language/", import.meta.url);
const text = readFileSync(new URL("bibtex.grammar", dir), "utf8");
const { parser, terms } = buildParserFile(text, { fileName: "bibtex.grammar", moduleStyle: "es" });
writeFileSync(new URL("parser.js", dir), parser);
writeFileSync(new URL("parser.terms.js", dir), terms);
console.log("wrote src/lib/bibtex-language/parser.js and parser.terms.js");
