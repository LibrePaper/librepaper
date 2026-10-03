// The sidebar manifest for the static docs site.
//
// An entry is either a link on its own -- `{ path, label }` -- or a group
// with `pages` under it. A group whose own `path` is set is a page as well as
// a heading: "Authoring" is both the landing page and the name of the three
// beneath it, so the sidebar says it once rather than repeating it as the
// first child of itself. A group without a `path` is a heading only, for a
// set of pages with no landing page of its own.
//
// A path is relative to this file and without the extension.
// web/tools/build-site.mjs reads this once, renders it into every page's
// <aside>, and marks whichever entry matches the page being built -- so
// adding a page here is the whole of adding it to the sidebar, and the order
// below is the order it appears in.
export const nav = [
  { path: "what", label: "What is LibrePaper" },
  { path: "install", label: "Install" },
  {
    path: "authoring/index",
    label: "Authoring",
    pages: [
      { path: "authoring/latex", label: "LaTeX" },
      { path: "authoring/typst", label: "Typst" },
    ],
  },
  {
    label: "Computational notebooks",
    pages: [
      { path: "notebooks/quarto", label: "Quarto" },
      { path: "notebooks/calepin", label: "Calepin" },
    ],
  },
  {
    label: "Collaborate",
    pages: [
      { path: "collaborate/share", label: "Share" },
      { path: "collaborate/edit", label: "Edit in the browser" },
      { path: "collaborate/history", label: "History" },
      { path: "collaborate/review", label: "Review" },
    ],
  },
  { path: "agents", label: "Agents" },
  { path: "cli", label: "CLI" },
  { path: "backups", label: "Local backups" },
  { path: "host", label: "Self-hosting" },
  {
    path: "architecture/index",
    label: "Architecture",
    pages: [
      { path: "architecture/document", label: "Document and storage" },
      { path: "architecture/review", label: "Comments and track changes" },
      { path: "architecture/rendering", label: "Rendering and live sync" },
      { path: "architecture/companion", label: "Companion and agents" },
      { path: "architecture/building", label: "Building from source" },
    ],
  },
  { path: "privacy", label: "Privacy" },
];
