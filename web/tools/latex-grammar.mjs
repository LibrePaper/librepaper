// Builds the LaTeX parser from its Lezer grammar.
//
// Reads src/lib/latex-language/latex.grammar and writes parser.js and
// parser.terms.js beside it. Run it after editing the grammar:
//
//   node tools/latex-grammar.mjs
//
// The generated files are checked in, so a normal build does not need the
// generator at all.
import { readFileSync, writeFileSync } from "node:fs";
import { buildParserFile } from "@lezer/generator";

const dir = new URL("../src/lib/latex-language/", import.meta.url);
const text = readFileSync(new URL("latex.grammar", dir), "utf8");
const { parser, terms } = buildParserFile(text, { fileName: "latex.grammar", moduleStyle: "es" });
writeFileSync(new URL("parser.js", dir), parser);
writeFileSync(new URL("parser.terms.js", dir), terms);
console.log("wrote src/lib/latex-language/parser.js and parser.terms.js");
