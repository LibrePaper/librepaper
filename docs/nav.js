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
  { path: "local-app", label: "Local app" },
  { path: "backups", label: "Local backups" },
  {
    label: "Computational notebooks",
    pages: [
      { path: "notebooks/quarto", label: "Quarto" },
      { path: "notebooks/calepin", label: "Calepin" },
    ],
  },
  { path: "collaborate", label: "Collaborate" },
  { path: "agents", label: "Agents" },
  { path: "cli", label: "CLI" },
  {
    label: "Self-hosting",
    pages: [
      { path: "host/simple", label: "Simple deployment" },
      { path: "host/advanced", label: "Advanced features" },
    ],
  },
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
  { path: "privacy", label: "Privacy and data retention" },
];
