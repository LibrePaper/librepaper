import assert from "node:assert/strict";
import { createProjectSession } from "../../src/lib/project-session.js";

// tree() keeps each text's body between builds (`bodyCache`). These tests
// check that its output never differs from a build with nothing cached.

const rules = { extensions: [".md"], text_extensions: [".md"], asset_extensions: [], derived_extensions: [],
  max_path: 200, max_files: 200, log_quota_bytes: 1000000, storage: { per_owner: 1000000 } };

const quiet = (presence) => {
  const session = createProjectSession({ send: () => {}, onState: () => {}, presenceId: () => presence });
  session.setRules(rules);
  return session;
};

// Everything `from` has, imported into `to`. Both sides see the same history.
const sync = (from, to) => to.doc.import(from.doc.export({ mode: "update" }));

// The tree a session that has never built one would produce for the same
// history: its cache starts empty, so nothing is reused.
function uncached(source) {
  const copy = quiet("uncached");
  try {
    sync(source, copy);
    return copy.tree();
  } finally {
    copy.leave();
  }
}

{
  const a = quiet("cache-a");
  const b = quiet("cache-b");
  try {
    const alpha = a.addText("alpha.md", "one");
    const beta = a.addText("beta.md", "two");
    sync(a, b);
    assert.deepStrictEqual(a.tree(), uncached(a), "first build matches an uncached build");
    assert.deepStrictEqual(b.tree(), uncached(b), "a peer's first build matches too");

    // Edits to one text: the edited text is new and the other is unchanged.
    a.textOf(alpha).insert(3, " edited");
    a.doc.commit();
    assert.equal(a.tree().texts["alpha.md"], "one edited");
    assert.equal(a.tree().texts["beta.md"], "two");
    assert.deepStrictEqual(a.tree(), uncached(a), "after one edit");

    a.textOf(beta).insert(0, ">");
    a.doc.commit();
    assert.deepStrictEqual(a.tree(), uncached(a), "after an edit to the other text");

    // A rename moves the path and keeps the body.
    a.renameFile(alpha, "gamma.md");
    assert.deepStrictEqual(Object.keys(a.tree().texts).sort(), ["beta.md", "gamma.md"]);
    assert.equal(a.tree().texts["gamma.md"], "one edited");
    assert.deepStrictEqual(a.tree(), uncached(a), "after a rename");

    // An added file, then a deleted one.
    const delta = a.addText("delta.md", "new");
    assert.deepStrictEqual(a.tree(), uncached(a), "after an added file");
    a.removeFile(beta);
    assert.equal(a.tree().texts["beta.md"], undefined);
    assert.deepStrictEqual(a.tree(), uncached(a), "after a deleted file");

    // Remote edits and a remote addition, imported from the other session.
    sync(a, b);
    b.textOf(alpha).insert(0, "B ");
    b.doc.commit();
    b.addText("eps.md", "from b");
    b.doc.commit();
    sync(b, a);
    assert.equal(a.tree().texts["gamma.md"], "B one edited");
    assert.equal(a.tree().texts["eps.md"], "from b");
    assert.deepStrictEqual(a.tree(), uncached(a), "after a remote update");
    assert.deepStrictEqual(b.tree(), uncached(b), "the peer after its own update");

    // A text edited in an uncommitted transaction is visible to tree(), which
    // builds without the cache while anything is pending.
    a.textOf(delta).insert(0, "P");
    assert.equal(a.doc.getPendingTxnLength() > 0, true, "the edit is still pending");
    assert.equal(a.tree().texts["delta.md"], "Pnew", "an uncommitted edit is visible");
    a.doc.commit();
    assert.equal(a.tree().texts["delta.md"], "Pnew");
    assert.deepStrictEqual(a.tree(), uncached(a), "after the pending edit is committed");
  } finally {
    a.leave();
    b.leave();
  }
}
