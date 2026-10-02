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
const build = ({ format }) => ["latex", "typst", "markdown", "quarto"].includes(format);
const latex = ({ format, mayEdit }) => format === "latex" && mayEdit;
const quarto = ({ format, mayEdit }) => format === "quarto" && mayEdit;
const local = (_context) => true;
const remote = (_context) => true;
const backups = (_context) => true;
const projectBinding = ({ format }) => ["typst", "markdown", "quarto"].includes(format);
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
      { id: "storage-latex", says: "Downloaded LaTeX files", terms: "cache clear free space packages compiler", offered: latex },
    ],
  },
  {
    id: "render", says: "Render", offered: build,
    note: "Only this browser and user.",
    entries: [
      { id: "render-tool", says: "Build tool", terms: "compiler render browser local companion automatic latex typst markdown quarto" },
      { id: "render-engine", says: "Engine", terms: "pdflatex xelatex lualatex render compiler" },
    ],
  },
  {
    id: "local", says: "Local companion", offered: local,
    note: "LibrePaper on this computer",
    entries: [
      { id: "local-status", says: "Connection", terms: "connect disconnect retry status install installer setup linux macos windows allow site agent claude codex pi opencode zotero quarto companion" },
      { id: "local-address", says: "Companion address", terms: "address port url localhost host version disconnect" },
      { id: "local-binding", says: "Project folder", terms: "quarto typst markdown folder binding hosted", offered: projectBinding },
      { id: "local-startup", says: "Start at login", terms: "startup login background standalone companion", offered: local },
      { id: "local-doctor", says: "Check local setup", terms: "doctor troubleshoot diagnostics report" },
      { id: "quarto-status", says: "Quarto status", terms: "quarto available version" },
      { id: "quarto-executable", says: "Quarto executable", terms: "quarto path executable" },
      { id: "quarto-arguments", says: "Quarto arguments", terms: "quarto arguments options" },
      { id: "rendering-profile", says: "Quarto profile", terms: "quarto profile render preview", offered: quarto },
      { id: "rendering-parameters", says: "Quarto parameters", terms: "quarto params parameters json render preview", offered: quarto },
      { id: "quarto-execution", says: "Quarto local code execution", terms: "quarto run code permission execute", offered: quarto },
      { id: "calepin-status", says: "Calepin status", terms: "calepin available version" },
      { id: "calepin-executable", says: "Calepin executable", terms: "calepin path executable" },
      { id: "calepin-arguments", says: "Calepin arguments", terms: "calepin arguments options" },
      { id: "zotero-status", says: "Zotero status", terms: "zotero available version citations bibliography library references" },
    ],
  },
  {
    id: "remote", says: "Remote connection", offered: remote,
    note: "The LibrePaper server for this project.",
    entries: [
      { id: "remote-status", says: "Connection", terms: "connected offline server address sync status" },
    ],
  },
  {
    id: "backups", says: "Backups", offered: backups,
    note: "Every project available to this account, including shared projects",
    entries: [
      { id: "backup-enable", says: "Automatic backups", terms: "zip archive schedule frequency interval account all projects" },
      { id: "backup-destination", says: "Backup folder", terms: "destination choose directory folder local companion" },
      { id: "backup-frequency", says: "Frequency", terms: "minutes schedule interval" },
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
