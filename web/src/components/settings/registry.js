// The map of the settings dialog: the categories and when each is offered.
// The dialog draws its navigation from this and searches it; what a category
// shows is a component beside this file.
//
// Build preferences belong to this browser and user. The local app category
// manages the companion running on this computer.

// `offered` answers with the document's format and whether this browser may
// edit it. `terms` are the words somebody might type when looking for a row
// and not finding its title.
const editor = ({ mayEdit }) => mayEdit;
const build = ({ format, mayEdit }) => ["latex", "typst", "markdown", "quarto"].includes(format) && mayEdit;
const latex = ({ format, mayEdit }) => format === "latex" && mayEdit;
const quarto = ({ format, mayEdit }) => format === "quarto" && mayEdit;
const local = ({ format, mayEdit }) => ["typst", "markdown", "quarto"].includes(format) && mayEdit;
// The account is the deployment's, not the document's: whoever is signed in
// is offered it whatever they happen to have open.
const account = ({ signedIn }) => Boolean(signedIn);

export const CATEGORIES = [
  {
    id: "editor", says: "Editor", offered: editor,
    entries: [
      { id: "editor-keys", says: "Keys", terms: "vim emacs keymap keyboard bindings modal source standard" },
      { id: "editor-shortcuts", says: "Keyboard shortcuts", terms: "shortcut shortcuts keys keyboard chord binding hotkey accelerator palette command undo redo find" },
    ],
  },
  {
    id: "storage", says: "Storage", offered: (context) => latex(context) || account(context),
    entries: [
      { id: "storage-account", says: "Account storage", terms: "quota usage space used limit bytes", offered: account },
      { id: "storage-retention", says: "Versions", terms: "checkpoints versions history retention prune cleanup label publish restore", offered: account },
      { id: "storage-latex", says: "Downloaded LaTeX files", terms: "cache clear free space packages compiler", offered: latex },
    ],
  },
  {
    id: "build", says: "Build", offered: build,
    note: "Only this browser and user.",
    entries: [
      { id: "build-tool", says: "Build tool", terms: "compiler engine browser local companion automatic latex typst markdown quarto" },
      { id: "build-engine", says: "Engine", terms: "pdflatex xelatex lualatex" },
    ],
  },
  {
    id: "rendering", says: "Quarto", offered: quarto,
    note: "How this browser previews the document. Not shared.",
    entries: [
      { id: "rendering-profile", says: "Profile", terms: "quarto profile render preview" },
      { id: "rendering-parameters", says: "Parameters", terms: "quarto params parameters json render preview" },
    ],
  },
  {
    id: "local", says: "Local app", offered: local,
    note: "The LibrePaper app running on this computer.",
    entries: [
      { id: "local-status", says: "Connection", terms: "connect disconnect retry status" },
      { id: "local-pairing", says: "Pairing code", terms: "pair code allow site" },
      { id: "local-address", says: "Address", terms: "port url localhost host" },
      { id: "local-binding", says: "Binding ID", terms: "quarto hosted", offered: quarto },
      { id: "local-tools", says: "Available tools", terms: "versions latex quarto biber" },
      { id: "local-doctor", says: "Check local setup", terms: "doctor troubleshoot diagnostics report" },
    ],
  },
  {
    id: "account", says: "Account", offered: account,
    note: "This account on this deployment, not this document.",
    entries: [
      { id: "account-erase", says: "Erase this account", terms: "erase delete account remove close gdpr right erasure forget" },
    ],
  },
];

// The categories this browser is offered for this document, in order.
export function offered(context) {
  return CATEGORIES.filter((category) => category.offered(context));
}

// The rows a query finds, grouped by category: every offered category whose
// name matches, with all its rows, and every category with a row that does.
export function search(query, context) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const matches = (text) => words.every((word) => text.toLowerCase().includes(word));
  const found = [];
  for (const category of offered(context)) {
    const rows = category.entries.filter((entry) => !entry.offered || entry.offered(context));
    const entries = matches(category.says) ? rows : rows.filter((entry) => matches(`${entry.says} ${entry.terms}`));
    if (entries.length) found.push({ category, entries });
  }
  return found;
}
