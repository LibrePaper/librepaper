// Who is offered which panel.
//
// This is one list because the column is drawn in three places -- the rail
// beside it, the row along the bottom of a narrow window, and the column
// itself -- and because the reader asks the same list again before offering a
// panel from a menu. So what is checked here is the rule, not any one of the
// places that reads it.
import assert from "node:assert/strict";
import { tabsFor, TABS } from "../../src/lib/panels.js";

const offered = (who) => tabsFor(who).map((tab) => tab.id);

// The two roles that arrive by a link. A read link is a document and the
// pending changes in it, read only: no other panel is one they could act in,
// and a rail of icons opening columns that report on work they cannot do is a
// workspace they do not have.
assert.deepEqual(
  offered({ role: "reader", mayEdit: false, editing: false, canSeeSharing: false, canPublish: false }),
  ["changes"],
  "a read link is offered the changes, read only",
);

// A comment link is the comments and the changes, and only those: not the
// agent, not the history, none of which the link can touch.
assert.deepEqual(
  offered({ role: "commenter", mayEdit: false, editing: false, canSeeSharing: false, canPublish: false }),
  ["collaboration", "changes"],
  "a comment link is offered the comments and the changes",
);

// Neither of them is offered the source, which is the same statement: the
// panels that stand beside a source pane are not on their list.
for (const role of ["reader", "commenter"]) {
  const list = offered({ role, mayEdit: false, editing: false, canSeeSharing: false, canPublish: false });
  for (const forbidden of ["files", "outline", "agent", "history", "diagnostics", "share"]) {
    assert.ok(!list.includes(forbidden), `${role} is not offered ${forbidden}`);
  }
}

// An editor keeps everything the conditions allow; the role rules take nothing
// away from somebody who can work here.
assert.deepEqual(
  offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: true }),
  TABS.map((tab) => tab.id),
  "an editor with the source open is offered every panel",
);
assert.deepEqual(
  offered({ role: "owner", mayEdit: true, editing: true, canSeeSharing: true, canPublish: true }),
  TABS.map((tab) => tab.id),
  "and so is an owner",
);

// The conditions still apply above the role rules: a panel whose `when` is
// false is not offered to an editor either.
assert.ok(
  !offered({ role: "editor", mayEdit: true, editing: false, canSeeSharing: false, canPublish: false })
    .includes("diagnostics"),
  "diagnostics waits on the source pane, whatever the role",
);

// Before the document has answered there is no role, and the rail stays empty
// rather than putting up one audience's furniture and taking it down again
// when the reply lands.
assert.deepEqual(offered({ mayEdit: false, editing: false }), [], "an unanswered document offers nothing");
assert.deepEqual(offered({ role: "", mayEdit: false, editing: false }), [], "and neither does an empty role");
assert.deepEqual(offered(undefined), [], "nor no reader at all");

// The Symbols and Math palettes are overlays now, so no panel offers them.
assert.deepEqual(
  offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: false }).slice(0, 2),
  ["files", "outline"],
  "an editor with the source open is offered files and outline first",
);
assert.ok(
  !offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: false }).some((id) => id === "symbols" || id === "math"),
  "no palette is a panel",
);

console.log("panels: a read link gets the changes, a comment link gets the comments and changes, an editor gets the workspace");
