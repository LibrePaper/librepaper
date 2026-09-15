// The sidebar manifest for the static docs site. A section is a heading and
// an ordered list of pages; a page is the path of its markdown source,
// relative to this file and without the extension, and the label the sidebar
// shows for it. web/tools/build-site.mjs reads this once, renders it into
// every page's <aside>, and marks whichever entry matches the page being
// built as the current one -- so adding a page here is the whole of adding it
// to the sidebar, and the order below is the order it appears in.
export const nav = [
  {
    title: "Start",
    pages: [
      { path: "start/index", label: "Start" },
      { path: "start/sandbox", label: "The sandbox" },
    ],
  },
  {
    title: "Authoring",
    pages: [
      { path: "authoring/index", label: "Authoring" },
      { path: "authoring/latex", label: "LaTeX" },
      { path: "authoring/typst", label: "Typst" },
      { path: "authoring/quarto", label: "Quarto" },
    ],
  },
  {
    title: "Collaborate",
    pages: [
      { path: "collaborate/share", label: "Share" },
      { path: "collaborate/edit", label: "Edit in the browser" },
      { path: "collaborate/history", label: "History" },
      { path: "collaborate/review", label: "Review" },
    ],
  },
  {
    title: "Agents",
    pages: [{ path: "agents", label: "Agents" }],
  },
  {
    title: "CLI",
    pages: [
      { path: "cli/index", label: "CLI" },
      { path: "cli/publish", label: "Publish" },
    ],
  },
  {
    title: "Host",
    pages: [
      { path: "host/index", label: "Host" },
      { path: "host/access", label: "Access" },
      { path: "host/storage", label: "Storage" },
    ],
  },
  {
    title: "Privacy",
    pages: [{ path: "privacy", label: "Privacy" }],
  },
  {
    title: "Internals",
    pages: [{ path: "internals", label: "Internals" }],
  },
];
