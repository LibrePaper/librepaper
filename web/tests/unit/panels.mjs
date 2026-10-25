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

// The two roles that arrive by a link. A read link is the project's files and
// the pending changes in it, read only: link holders already hold every source
// file, so the explorer hides nothing, and no other panel is one they could act
// in.
assert.deepEqual(
  offered({ role: "reader", mayEdit: false, editing: false, canSeeSharing: false, canPublish: false }),
  ["files", "changes"],
  "a read link is offered the files and the changes, read only",
);

// A comment link is the files, the comments and the changes, and only those:
// not the agent, not the history, none of which the link can touch.
assert.deepEqual(
  offered({ role: "commenter", mayEdit: false, editing: false, canSeeSharing: false, canPublish: false }),
  ["files", "collaboration", "changes"],
  "a comment link is offered the files, the comments and the changes",
);

// Neither of them is offered the outline or the panels that act on a source
// pane: those stand beside a source pane, which a link holder does not have.
for (const role of ["reader", "commenter"]) {
  const list = offered({ role, mayEdit: false, editing: false, canSeeSharing: false, canPublish: false });
  for (const forbidden of ["outline", "agent", "history", "diagnostics", "share"]) {
    assert.ok(!list.includes(forbidden), `${role} is not offered ${forbidden}`);
  }
}

// The Git panel waits for the desktop app.
assert.ok(
  !offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: true }).includes("git"),
  "no desktop app, no Git panel",
);

// An editor keeps everything the conditions allow; the role rules take nothing
// away from somebody who can work here.
assert.deepEqual(
  offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: true, desktop: true }),
  TABS.map((tab) => tab.id),
  "an editor with the source open is offered every panel",
);
assert.deepEqual(
  offered({ role: "owner", mayEdit: true, editing: true, canSeeSharing: true, canPublish: true, desktop: true }),
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

// A folder on this computer has no server behind it: collaboration, the agent,
// history, sharing and Git are not offered, whatever else the reader may do.
{
  const list = offered({ role: "owner", mayEdit: true, editing: true, canSeeSharing: true, canPublish: true, desktop: true, local: true });
  for (const hidden of ["collaboration", "agent", "history", "share", "git"]) {
    assert.ok(!list.includes(hidden), `a local project is not offered ${hidden}`);
  }
  assert.deepEqual(list, ["files", "outline", "changes", "diagnostics"], "a local project keeps the files, outline, changes and diagnostics");
}

// Before the document has answered there is no role, and the rail stays empty
// rather than putting up one audience's furniture and taking it down again
// when the reply lands.
assert.deepEqual(offered({ mayEdit: false, editing: false }), [], "an unanswered document offers nothing");
assert.deepEqual(offered({ role: "", mayEdit: false, editing: false }), [], "and neither does an empty role");
assert.deepEqual(offered(undefined), [], "nor no reader at all");

// The Symbols and Math palettes are overlays, so no panel offers them.
assert.deepEqual(
  offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: false }).slice(0, 2),
  ["files", "outline"],
  "an editor with the source open is offered files and outline first",
);
assert.ok(
  !offered({ role: "editor", mayEdit: true, editing: true, canSeeSharing: false, canPublish: false }).some((id) => id === "symbols" || id === "math"),
  "no palette is a panel",
);

console.log("panels: a read link gets the files and changes, a comment link gets the files, comments and changes, an editor gets the workspace");
