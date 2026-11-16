import assert from "node:assert/strict";
import { tokenizeSource } from "../../src/lib/visual-markup/core/tokenize.ts";
import { walk } from "../../src/lib/visual-markup/core/tokens.ts";
import { markdown } from "../../src/lib/visual-markup/languages/markdown/index.ts";
import { org } from "../../src/lib/visual-markup/languages/org/index.ts";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

await loadOrgSyntaxForTests();

// The pipe-table tokens of a source, in tokenizer order.
function tablesOf(language, source) {
  const found = [];
  walk(tokenizeSource(language, source), token => {
    if (token.name === "mdtable") found.push(token);
  });
  return found;
}

// Org: rows and the rule come back as text, #+TBLFM stays, and the rule keeps its place.
{
  const source = "| a | b |\n|---+---|\n| c | d |\n#+TBLFM: $2=$1";
  const [token] = tablesOf(org, source);
  assert.ok(token, "an Org table is an mdtable token");
  assert.deepEqual(org.table.parse(source, token), [["a", "b"], ["c", "d"]]);
  assert.equal(org.table.locate(source, token, source.indexOf("d")).row, 1);
  assert.equal(org.table.locate(source, token, source.indexOf("d")).col, 1);

  const text = source.slice(token.from, token.to);
  assert.ok(text.endsWith("#+TBLFM: $2=$1"));
  const original = "| a | b |\n|---+---|\n| c | d |";
  assert.equal(
    org.table.serialize([["x", "yy"], ["z", "w"]], token, text),
    text.replace(original, "| x | yy |\n|---+----|\n| z | w  |")
  );
}

// Markdown: a caption above the table is outside the cells, and the separator keeps its alignment.
{
  const source = "Table: Fruit\n\n| Name | Qty |\n|:-----|----:|\n| apple | 3 |";
  const [token] = tablesOf(markdown, source);
  assert.ok(token, "a Markdown table is an mdtable token");
  assert.deepEqual(markdown.table.parse(source, token), [["Name", "Qty"], ["apple", "3"]]);
  assert.equal(markdown.table.locate(source, token, source.indexOf("apple") + 1).row, 1);
  assert.equal(markdown.table.locate(source, token, source.indexOf("apple") + 1).col, 0);

  const text = source.slice(token.from, token.to);
  const original = "| Name | Qty |\n|:-----|----:|\n| apple | 3 |";
  assert.equal(
    markdown.table.serialize([["Name", "Qty"], ["apple", "10"]], token, text),
    text.replace(original, "| Name  | Qty |\n|:------|----:|\n| apple |  10 |")
  );
}
