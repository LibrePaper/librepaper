// The map of the settings dialog: the categories and when each is offered.
// The dialog draws its navigation from this and searches it; what a category
// shows is a component beside this file.
//
// Browser-scoped pages: Editor, Rendering. Computer-scoped pages: Tools, Agents,
// Diagnostics. Account-scoped pages: Backups, Account.

// `offered` answers with the document's format, whether this browser may edit
// it, and whether somebody is signed in. `terms` are the words somebody might
// type when looking for a row and not finding its title.
const always = (_context) => true;
const editor = ({ mayEdit }) => mayEdit;
const build = always;
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
    id: "rendering", says: "Rendering", offered: always,
    note: "This browser",
    entries: [
      { id: "render-latex-engine", says: "LaTeX engine", terms: "engine pdflatex xelatex automatic compiler" },
      { id: "render-latex-files", says: "Downloaded LaTeX files", terms: "latex compiler cache clear free space packages storage" },
      { id: "render-markdown-tool", says: "Markdown renderer", terms: "markdown browser pandoc quarto render tool" },
      { id: "rendering-profile", says: "Quarto profile", terms: "quarto profile render preview" },
      { id: "rendering-parameters", says: "Quarto parameters", terms: "quarto params parameters json render preview" },
    ],
  },
  {
    id: "tools", says: "Tools", offered: always,
    note: "This computer",
    entries: [
      { id: "tools-connection", says: "Companion", terms: "companion connect install running status" },
      { id: "tools-approvals", says: "Waiting for your answer", terms: "allow deny approval pending requests access" },
      { id: "tools-list", says: "Programs", terms: "tools tool status version available missing rescan scan local quarto pandoc calepin typst zotero" },
      { id: "tools-quarto", says: "Quarto", terms: "quarto executable path arguments version" },
      { id: "tools-calepin", says: "Calepin", terms: "calepin typst executable path arguments version" },
      { id: "tools-zotero", says: "Zotero", terms: "zotero citations bibliography library references local api" },
      { id: "tools-sites", says: "Connected sites", terms: "pairing revoke origin website site access" },
      { id: "tools-folders", says: "Project folders", terms: "bindings folder grant revoke directory project access" },
    ],
  },
  {
    id: "agents", says: "AI agents", offered: always,
    note: "This computer",
    entries: [
      { id: "agents-list", says: "Agents on this computer", terms: "agent agents claude codex opencode pi detected configured custom remove adapter setup" },
      { id: "agents-add", says: "Add an agent", terms: "agent add custom command executable arguments" },
    ],
  },
  {
    id: "backups", says: "Backups", offered: always,
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
  {
    id: "diagnostics", says: "Diagnostics", offered: always,
    note: "This computer",
    separated: true,
    entries: [
      { id: "diagnostics-address", says: "Companion address", terms: "address port url localhost host version disconnect" },
      { id: "diagnostics-startup", says: "Start at login", terms: "startup login background quit companion" },
      { id: "diagnostics-report", says: "Check local setup", terms: "doctor troubleshoot diagnostics report rescan" },
      { id: "diagnostics-activity", says: "Activity", terms: "jobs previews sessions cancel stop output logs running" },
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
