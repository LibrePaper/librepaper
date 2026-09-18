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

// What a role is offered, where that is less than what the conditions above
// would otherwise allow. Somebody holding a read link is given the document
// and nothing around it: there is no panel they can act in, and a rail of
// icons opening columns that only report on work they cannot do is a
// workspace pretending they have one. A commenter is given the one panel
// their link is for, and nothing else -- not the changes queue, not the
// agent, not the history, none of which a comment link can touch.
//
// An editor and an owner are not named here: what they are offered is the
// `when` rules alone. Neither is a role nobody has answered with yet -- the
// document always says which one this caller holds, so no role means the
// document has not answered, and a rail drawn then is one audience's
// furniture put up and taken down again as soon as the reply lands.
const ROLE_PANELS = { reader: [], commenter: ["collaboration"], "": [] };

// Every panel this browser is offered, in rail order.
export const tabsFor = (who) => {
  const only = ROLE_PANELS[who?.role || ""];
  return only
    ? TABS.filter((tab) => only.includes(tab.id))
    : TABS.filter((tab) => !tab.when || tab.when(who));
};

// The same rail, one level up: the places in an account rather than the panels
// in a document. The landing page draws these through the same components, so
// the column of icons does not move when a project is opened -- only what is
// written in it changes.
//
// Two of the icons are borrowed from the list above on purpose. `history` is
// Recent here and History there, `users` is Shared with me here and Share
// there; in both cases the icon means the same thing one level apart, which is
// the rule that lets the rail be read as one thing across the navigation.
export const PROJECT_TABS = [
  { id: "projects", says: "Projects", icon: "folder" },
  { id: "recent", says: "Recent", icon: "history" },
  { id: "shared", says: "Shared with me", icon: "users" },
  { id: "favorites", says: "Favorites", icon: "star" },
  // Last, and after a gap the stylesheet puts there: the trash is a place you
  // go on purpose, and never the one you land on next to Favorites by being
  // slightly off with the pointer.
  { id: "trash", says: "Trash", icon: "trash" },
];

export const PROJECT_TAB_IDS = PROJECT_TABS.map((tab) => tab.id);

// The values the stored panel preference may take, "" being the closed column.
export const PANEL_IDS = ["", ...TABS.map((tab) => tab.id)];

// The element a rail button controls, named the same way wherever it is drawn
// so the two rails point at the one panel rather than at two guesses.
export const slotId = (id) => `sidebar-panel-${id}`;
