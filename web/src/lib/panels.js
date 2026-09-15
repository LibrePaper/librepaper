// The column's panels: what each one is called, the icon it is known by, and
// who is offered it.
//
// One list, because a panel is shown in three places -- the rail beside the
// column, the row along the bottom of a narrow window, and the column itself
// -- and a panel that differs between them is a panel a reader cannot
// recognize when the window changes shape. It used to be three lists: the
// labels here, the icons beside them, and a third copy of who may see each
// one written again as conditions in the column's markup, where a rule that
// disagreed with this one would silently show an empty panel.
//
// `when` is asked about the reader, not about the document: `mayEdit` is the
// role the document answers with, `editing` whether the source pane is open.
export const TABS = [
  { id: "files", says: "Files", icon: "folder", when: (who) => who.mayEdit },
  { id: "outline", says: "Outline", icon: "list", when: (who) => who.mayEdit },
  { id: "collaboration", says: "Collaboration", icon: "comment" },
  { id: "changes", says: "Changes", icon: "pencil" },
  { id: "agent", says: "Agent", icon: "bot" },
  // History is readable by link-holders too: reviewers need the “since”
  // view even when they cannot edit or restore the live source.
  { id: "history", says: "History", icon: "history" },
  { id: "diagnostics", says: "Diagnostics", icon: "triangle-alert", when: (who) => who.editing },
  { id: "share", says: "Share", icon: "users", when: (who) => who.canSeeSharing || who.canPublish },
];

// Every panel this browser is offered, in rail order.
export const tabsFor = (who) => TABS.filter((tab) => !tab.when || tab.when(who));

// The values the stored panel preference may take, "" being the closed column.
export const PANEL_IDS = ["", ...TABS.map((tab) => tab.id)];

// The element a rail button controls, named the same way wherever it is drawn
// so the two rails point at the one panel rather than at two guesses.
export const slotId = (id) => `sidebar-panel-${id}`;
