// The map of the settings dialog: the categories and when each is offered.
// The dialog draws its navigation from this and searches it; what a category
// shows is a component beside this file.
//
// Build preferences and downloaded LaTeX files live in this browser. Local
// folder paths, companion settings, and integrations live on this computer.

// `offered` answers with the document's format, whether this browser may edit
// it, whether somebody is signed in, and the build tool chosen for it (`tool`).
// `terms` are the words somebody might type when looking for a row and not
// finding its title.
const always = (_context) => true;
const editor = ({ mayEdit }) => mayEdit;
const build = always;
const projectBinding = ({ format }) => ["typst", "markdown", "quarto"].includes(format);
// The account is the deployment's, not the document's: whoever is signed in
// is offered it whatever they happen to have open.
const account = ({ signedIn }) => Boolean(signedIn);

export const CATEGORIES = [
  {
    id: "editor", says: "Editor", offered: editor,
    note: "This browser",
    entries: [
      { id: "editor-keys", says: "Keys", terms: "vim emacs keymap keyboard bindings modal source standard" },
      { id: "editor-shortcuts", says: "Keyboard shortcuts", terms: "shortcut shortcuts keys keyboard chord binding hotkey accelerator palette command undo redo find" },
    ],
  },
  {
    id: "render", says: "Render", offered: build,
    entries: [
      { id: "render-tool", says: "Build tool", terms: "compiler engine pdflatex xelatex lualatex render browser local companion automatic latex typst markdown quarto" },
      { id: "render-output", says: "Output", terms: "pdf html format preview export", offered: projectBinding },
      { id: "render-folder", says: "Project folder", terms: "quarto typst markdown folder binding entrypoint local", offered: projectBinding },
      { id: "render-latex-files", says: "Downloaded LaTeX files", terms: "latex compiler cache clear free space packages storage" },
      { id: "typst-status", says: "Typst status", terms: "typst available version browser local companion disconnected" },
      { id: "quarto-status", says: "Quarto status", terms: "quarto available version" },
      { id: "quarto-executable", says: "Quarto executable", terms: "quarto path executable" },
      { id: "quarto-arguments", says: "Quarto arguments", terms: "quarto arguments options" },
      { id: "rendering-profile", says: "Quarto profile", terms: "quarto profile render preview" },
      { id: "rendering-parameters", says: "Quarto parameters", terms: "quarto params parameters json render preview" },
      { id: "quarto-execution", says: "Quarto local code execution", terms: "quarto run code permission execute" },
      { id: "calepin-status", says: "Calepin status", terms: "calepin available version" },
      { id: "calepin-executable", says: "Calepin executable", terms: "calepin path executable" },
      { id: "calepin-arguments", says: "Calepin arguments", terms: "calepin arguments options command" },
    ],
  },
  {
    id: "integrations", says: "Integrations", offered: always,
    note: "This computer",
    entries: [
      { id: "zotero-status", says: "Zotero", terms: "zotero available version citations bibliography library references" },
    ],
  },
  {
    id: "local", says: "Companion", offered: always,
    note: "This computer",
    entries: [
      { id: "local-status", says: "Connection", terms: "connect disconnect retry status install installer setup linux macos windows allow site agent claude codex pi opencode zotero quarto companion" },
      { id: "local-address", says: "Companion address", terms: "address port url localhost host version disconnect" },
      { id: "local-startup", says: "Start at login", terms: "startup login background standalone companion" },
      { id: "local-doctor", says: "Check local setup", terms: "doctor troubleshoot diagnostics report" },
    ],
  },
  {
    id: "backups", says: "Backups", offered: always,
    note: "Your account",
    entries: [
      { id: "backup-enable", says: "Automatic backups", terms: "zip archive schedule frequency interval account all projects" },
      { id: "backup-destination", says: "Backup folder", terms: "destination choose directory folder local companion" },
      { id: "backup-frequency", says: "Frequency", terms: "minutes schedule interval" },
    ],
  },
  {
    id: "account", says: "Account", offered: always,
    note: "Your account",
    entries: [
      { id: "remote-status", says: "Remote connection", terms: "connected disconnected offline online server address url sync status collaboration" },
      { id: "storage-account", says: "Account storage", terms: "quota usage space used limit bytes", offered: account },
      { id: "account-erase", says: "Erase this account", terms: "erase delete account remove close gdpr right erasure forget", offered: account },
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

// Whether a category's row is offered here: what the dialog asks before it
// draws the row, so the page and the search agree.
export function has(category, id, context) {
  return category.entries.some((entry) => entry.id === id && (!entry.offered || entry.offered(context)));
}
