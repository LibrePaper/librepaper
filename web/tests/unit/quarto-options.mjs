import assert from "node:assert/strict";
import { test } from "node:test";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { pandocExtensions } from "../../src/lib/pandoc-markdown.js";
import { optionRanges } from "../../src/lib/quarto-options.js";

function ranges(doc) {
  const state = EditorState.create({
    doc,
    extensions: markdown({ base: markdownLanguage, extensions: pandocExtensions }),
  });
  ensureSyntaxTree(state, state.doc.length, 1000);
  return optionRanges(state).map((r) => [doc.slice(r.from, r.to), doc.slice(r.from, r.keyTo)]);
}

test("#| lines at the top of a Quarto cell are options", () => {
  const doc = "text\n\n```{r}\n#| label: fig-a\n#| echo: false\nx <- 1\n```\n";
  assert.deepEqual(ranges(doc), [["#| label: fig-a", "#| label"], ["#| echo: false", "#| echo"]]);
});

test("a #| line after code is not an option", () => {
  assert.deepEqual(ranges("```{python}\n#| echo: false\nx = 1\n#| label: late\n```\n"), [["#| echo: false", "#| echo"]]);
  assert.deepEqual(ranges("```{r}\nx <- 1\n#| echo: false\n```\n"), []);
});

test("a plain fence has no options", () => {
  assert.deepEqual(ranges("```r\n#| echo: false\nx <- 1\n```\n"), []);
  assert.deepEqual(ranges("```\n#| echo: false\n```\n"), []);
});

test("an empty cell and an unfinished cell do not fail", () => {
  assert.deepEqual(ranges("```{r}\n```\n"), []);
  assert.deepEqual(ranges("```{r}\n#| echo: false"), [["#| echo: false", "#| echo"]]);
});
