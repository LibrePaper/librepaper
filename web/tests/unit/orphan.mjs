// Source placement decides whether an editor's passage survived; a reader
// sees only the rendered page. See `orphanState` in
// `../../src/lib/orphan.js`.
import assert from "node:assert/strict";
import { orphanState } from "../../src/lib/orphan.js";
import { aboutWholeDocument } from "../../src/lib/anchor.js";

// Found on the page and in the source: not orphaned, not source-only.
{
  const state = orphanState({ renderedFound: true, sourceFound: true, sourceKnown: true });
  assert.equal(state.orphaned, false);
  assert.equal(state.inSourceOnly, false);
}

// Found only in the source: not orphaned, but the page cannot reveal it.
{
  const state = orphanState({ renderedFound: false, sourceFound: true, sourceKnown: true });
  assert.equal(state.orphaned, false);
  assert.equal(state.inSourceOnly, true);
}

// Found in neither: orphaned.
{
  const state = orphanState({ renderedFound: false, sourceFound: false, sourceKnown: true });
  assert.equal(state.orphaned, true);
  assert.equal(state.inSourceOnly, false);
}

// A repeated rendered quote cannot revive a source passage the server says
// was deleted.
{
  const state = orphanState({ renderedFound: true, sourceFound: false, sourceKnown: true });
  assert.equal(state.orphaned, true);
  assert.equal(state.inSourceOnly, false);
}

assert.equal(orphanState({ renderedFound: false, sourceFound: false, wholeDocument: true }).orphaned, false);
assert.equal(orphanState({ renderedFound: true, sourceFound: false }).orphaned, false);

// A remark about the whole document is about something that is still here,
// and both roads to that answer have to reach it: an editor is told outright
// by the anchor, a reader works it out from a comment that quoted nothing and
// pointed nowhere. Otherwise a reader's general remark is badged as a passage
// missing from the page.
{
  const editor = { original_anchor: { kind: "document" }, presentation: {} };
  const reader = { presentation: { rendered_exact: "", rendered_position_utf16: null } };
  const passage = { presentation: { rendered_exact: "the interval" } };
  const point = { presentation: { rendered_exact: "", rendered_position_utf16: 42 } };
  assert.equal(aboutWholeDocument(editor), true);
  assert.equal(aboutWholeDocument(reader), true);
  assert.equal(aboutWholeDocument(passage), false);
  assert.equal(aboutWholeDocument(point), false);
  // An editor's anchor is the answer even when the page quoted nothing.
  assert.equal(aboutWholeDocument({ original_anchor: { kind: "source_text" }, presentation: {} }), false);
  assert.equal(orphanState({
    renderedFound: false, sourceFound: false, wholeDocument: aboutWholeDocument(reader),
  }).orphaned, false);
}

console.log("orphan: ok");
