import assert from "node:assert/strict";
import { documentPlaceFor, documentRangeFor, sourcePlaceFor } from "../../src/lib/sync.js";

// The renderers set quotes, dashes and ellipses as typography, so the source
// and the document spell the same words differently. Both sides blank them, so
// every caret in the sentence still finds its place.
{
  const source = "It's a model we don't trust -- the \"best\" one... isn't it? We'll see what it's worth.\n";
  const rendered = "It’s a model we don’t trust – the “best” one… isn’t it? We’ll see what it’s worth.";
  for (let caret = 0; caret < source.length; caret++) {
    assert.notEqual(documentPlaceFor(source, caret, rendered, "markdown"), null, `caret ${caret}`);
  }

  // And back again: a place in the document lands on the word it is at in the
  // source. "trust" starts a word in both texts, so the caret lands on it.
  const back = sourcePlaceFor(rendered, rendered.indexOf("trust"), source, "markdown");
  assert.equal(back, source.indexOf("trust"));
}

// A non-breaking space in the document is a space to the reader, and the
// source has an ordinary one.
{
  const source = "see Figure 1 for the whole thing here";
  assert.notEqual(documentPlaceFor(source, 0, source, "markdown"), null);
  const rendered = "see Figure 1 for the whole thing here";
  assert.notEqual(documentPlaceFor(source, 0, rendered, "markdown"), null);
}

// A citation, a link and a cross-reference are spelled differently in the
// document. The caret in that paragraph must land in that paragraph, not in
// the next one, which is what the line-start fallback used to find.
{
  const source = "As shown in @smith2020 the estimator is consistent under weak conditions, see [the appendix](https://example.org/a/very/long/url) for the proof and [@smith2020, p. 4] for a discussion of the variance bound.\n\nA second paragraph that is entirely plain prose without any markup at all in it.\n";
  const rendered = "As shown in Smith (2020) the estimator is consistent under weak conditions, see the appendix for the proof and (Smith, 2020, p. 4) for a discussion of the variance bound. A second paragraph that is entirely plain prose without any markup at all in it.";
  const secondAt = rendered.indexOf("A second");
  const end = source.indexOf("\n\nA second");
  for (let caret = 0; caret < end; caret++) {
    const result = documentPlaceFor(source, caret, rendered, "markdown");
    assert.notEqual(result, null, `caret ${caret}`);
    assert.ok(result.at < secondAt, `caret ${caret} landed at ${result.at}, in the second paragraph`);
  }
}

// A comment on punctuation keeps the selection's own ends. Comment ranges do
// not blank typography, so the "..." is a place, and "stop." keeps its period.
{
  assert.deepEqual(documentRangeFor("Wait...", 4, 7, "Wait...", "markdown"), { start: 4, end: 7 });
  assert.deepEqual(
    documentRangeFor("He said: stop. Then left.", 9, 14, "He said: stop. Then left.", "markdown"),
    { start: 9, end: 14 },
  );
}
