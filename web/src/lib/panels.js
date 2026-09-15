// What each of the column's panels is called in icons. The rail of icons and
// the row of them at the bottom of a narrow window are the same set of panels
// shown twice, and an icon that differs between the two is a panel a reader
// cannot recognize when the window changes shape -- so the mapping is written
// here once rather than in each of them.
const ICONS = {
  files: "folder",
  outline: "list",
  collaboration: "comment",
  changes: "pencil",
  agent: "bot",
  history: "history",
  diagnostics: "triangle-alert",
  share: "users",
};

export const iconFor = (id) => ICONS[id] ?? "sliders";
