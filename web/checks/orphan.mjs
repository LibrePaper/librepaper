// Whether a comment counts as orphaned, in the world where it can be found
// two ways: on the rendered page and in the source. See `orphanState` in
// `../src/lib/orphan.js`.
import assert from "node:assert/strict";
import { orphanState } from "../src/lib/orphan.js";

// Found on the page and in the source: not orphaned, not source-only.
{
  const state = orphanState({ renderedFound: true, sourceFound: true, region: false });
  assert.equal(state.orphaned, false);
  assert.equal(state.inSourceOnly, false);
}

// Found only in the source: not orphaned, but the page cannot reveal it.
{
  const state = orphanState({ renderedFound: false, sourceFound: true, region: false });
  assert.equal(state.orphaned, false);
  assert.equal(state.inSourceOnly, true);
}

// Found in neither: orphaned.
{
  const state = orphanState({ renderedFound: false, sourceFound: false, region: false });
  assert.equal(state.orphaned, true);
  assert.equal(state.inSourceOnly, false);
}

// Found on the page but not in the source (no source anchor yet, or one that
// only ever lived in the rendering): still not orphaned, and not source-only
// either, since the page still has it.
{
  const state = orphanState({ renderedFound: true, sourceFound: false, region: false });
  assert.equal(state.orphaned, false);
  assert.equal(state.inSourceOnly, false);
}

// A region annotation is never orphaned and never source-only, whatever the
// rendered or source answers say -- it has no source anchor and is placed by
// the agent rather than by either quotation.
{
  for (const renderedFound of [true, false]) {
    for (const sourceFound of [true, false]) {
      const state = orphanState({ renderedFound, sourceFound, region: true });
      assert.equal(state.orphaned, false);
      assert.equal(state.inSourceOnly, false);
    }
  }
}

console.log("orphan: ok");
