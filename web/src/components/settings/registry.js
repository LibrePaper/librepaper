// The map of the settings dialog: the categories and when each is offered.
// The dialog draws its navigation from this and searches it; what a category
// shows is a component beside this file.
//
// Most settings belong to this browser alone. The two that do not say so in
// their `note`, shown under the category's title: the LaTeX compiler choice
// is shared with everyone editing the document, and the local app is the one
// running on this computer.

// `offered` answers with the document's format and whether this browser may
// edit it. `terms` are the words somebody might type when looking for a row
// and not finding its title.
const everyone = () => true;
const editor = ({ mayEdit }) => mayEdit;
const latex = ({ format, mayEdit }) => format === "latex" && mayEdit;
const quarto = ({ format, mayEdit }) => format === "quarto" && mayEdit;
const local = ({ format, mayEdit }) => (format === "latex" || format === "quarto") && mayEdit;

export const CATEGORIES = [
  {
    id: "editor", says: "Editor", offered: editor,
    entries: [{ id: "editor-keys", says: "Keys", terms: "vim emacs keymap keyboard bindings modal source standard" }],
  },
  {
    id: "dictation", says: "Dictation", offered: everyone,
    entries: [
      { id: "dictation-backend", says: "Speech recognition", terms: "backend browser device whisper privacy audio" },
      { id: "dictation-model", says: "Model", terms: "whisper parakeet download size" },
      { id: "dictation-language", says: "Language", terms: "detect automatically" },
      { id: "dictation-status", says: "Loaded model", terms: "webgpu cpu wasm device running" },
      { id: "dictation-downloads", says: "Downloaded models", terms: "storage cache clear remove free space whisper" },
    ],
  },
  {
    id: "storage", says: "Storage", offered: latex,
    entries: [
      { id: "storage-latex", says: "Downloaded LaTeX files", terms: "cache clear free space packages compiler" },
    ],
  },
  {
    id: "compiler", says: "Compiler", offered: latex,
    note: "Shared with everyone who edits this document.",
    entries: [
      { id: "compiler-engine", says: "PDF compiler", terms: "engine pdflatex xelatex lualatex automatic" },
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
