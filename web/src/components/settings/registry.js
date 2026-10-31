// The map of the settings dialog: the categories and when each is offered.
// The dialog draws its navigation from this and searches it; what a category
// shows is a component beside this file.
//
// The categories are the same on every page: the landing page and a project.
// App-scoped page: General. Browser-scoped pages: Editor, Rendering.
// Computer-scoped pages: Tools, AI agents. Account-scoped pages: Backups, Account.

import { desktop } from "../../lib/desktop/client.js";
import { m } from "../../lib/paraglide/messages.js";

// `offered` answers with whether somebody is signed in; a row may also ask
// whether this is the desktop app. `terms` are the
// words somebody might type when looking for a row and not finding its title.
const always = (_context) => true;
// The account is the deployment's, not the document's: whoever is signed in
// is offered it whatever they happen to have open.
const account = ({ signedIn }) => Boolean(signedIn);

export const CATEGORIES = [
  {
    id: "general", says: m.registry_general(), offered: always,
    note: m.registry_note_this_app(),
    entries: [
      { id: "general-language", says: m.common_language(), terms: m.registry_general_language_terms() },
      { id: "appearance-theme", says: m.common_theme(), terms: m.registry_appearance_theme_terms() },
      { id: "appearance-colours", says: m.common_colours(), terms: m.registry_appearance_colours_terms() },
      { id: "general-close", says: m.common_when_the_window_closes(), terms: m.registry_general_close_terms(), offered: () => Boolean(desktop()) },
      { id: "diagnostics-report", says: m.common_diagnostics(), terms: m.registry_diagnostics_report_terms() },
    ],
  },
  {
    id: "editor", says: m.common_editor(), offered: always,
    note: m.registry_note_this_browser(),
    entries: [
      { id: "appearance-font", says: m.common_font(), terms: m.registry_appearance_font_terms() },
      { id: "appearance-font-size", says: m.common_font_size(), terms: m.registry_appearance_font_size_terms() },
      { id: "appearance-line-height", says: m.common_line_height(), terms: m.registry_appearance_line_height_terms() },
      { id: "editor-keys", says: m.common_key_bindings(), terms: m.registry_editor_keys_terms() },
      { id: "editor-completion", says: m.common_autocomplete(), terms: m.registry_editor_completion_terms() },
      { id: "editor-brackets", says: m.common_close_brackets(), terms: m.registry_editor_brackets_terms() },
      { id: "editor-check", says: m.common_code_check(), terms: m.registry_editor_check_terms() },
      { id: "editor-proofing", says: m.common_grammar_and_spelling(), terms: m.registry_editor_proofing_terms() },
      { id: "editor-dialect", says: m.common_english_dialect(), terms: m.registry_editor_dialect_terms() },
      { id: "editor-shortcuts", says: m.common_keyboard_shortcuts(), terms: m.registry_editor_shortcuts_terms() },
    ],
  },
  {
    id: "rendering", says: m.registry_rendering(), offered: always,
    note: m.registry_note_this_browser(),
    entries: [
      { id: "render-latex-preview", says: m.registry_render_latex_preview(), terms: m.registry_render_latex_preview_terms() },
      { id: "render-latex-engine", says: m.common_latex_engine(), terms: m.registry_render_latex_engine_terms() },
      { id: "render-latex-files", says: m.common_downloaded_latex_files(), terms: m.registry_render_latex_files_terms() },
      { id: "render-typst-preview", says: m.registry_render_typst_preview(), terms: m.registry_render_typst_preview_terms() },
      { id: "render-markdown-preview", says: m.registry_render_markdown_preview(), terms: m.registry_render_markdown_preview_terms() },
      { id: "render-quarto-preview", says: m.registry_render_quarto_preview(), terms: m.registry_render_quarto_preview_terms() },
      { id: "render-latex-where", says: m.registry_render_latex_where(), terms: m.registry_render_latex_where_terms() },
      { id: "render-typst-where", says: m.registry_render_typst_where(), terms: m.registry_render_typst_where_terms() },
      { id: "render-markdown-where", says: m.registry_render_markdown_where(), terms: m.registry_render_markdown_where_terms() },
      { id: "render-latex-pdf", says: m.registry_render_latex_pdf(), terms: m.registry_render_latex_pdf_terms() },
      { id: "render-typst-pdf", says: m.registry_render_typst_pdf(), terms: m.registry_render_typst_pdf_terms() },
      { id: "render-markdown-html", says: m.registry_render_markdown_html(), terms: m.registry_render_markdown_html_terms() },
      { id: "render-quarto-html", says: m.registry_render_quarto_html(), terms: m.registry_render_quarto_html_terms() },
    ],
  },
  {
    id: "tools", says: m.registry_tools(), offered: always,
    note: m.common_this_computer(),
    entries: [
      { id: "tools-programs", says: m.common_programs(), terms: m.registry_tools_programs_terms() },
      { id: "tools-zotero", says: "Zotero", terms: m.registry_tools_zotero_terms() },
      { id: "tools-activity", says: m.common_activity(), terms: m.registry_tools_activity_terms() },
    ],
  },
  {
    id: "agents", says: m.registry_agents(), offered: always,
    note: m.common_this_computer(),
    entries: [
      { id: "agents-list", says: m.common_agents_on_this_computer(), terms: m.registry_agents_list_terms() },
      { id: "agents-add", says: m.registry_agents_add(), terms: m.registry_agents_add_terms() },
    ],
  },
  {
    id: "backups", says: m.registry_backups(), offered: always,
    entries: [
      { id: "backup-enable", says: m.common_automatic_backups(), terms: m.registry_backup_enable_terms() },
      { id: "backup-destination", says: m.common_backup_folder(), terms: m.registry_backup_destination_terms() },
      { id: "backup-frequency", says: m.common_frequency(), terms: m.registry_backup_frequency_terms() },
    ],
  },
  {
    id: "account", says: m.registry_account(), offered: always,
    note: m.registry_note_your_account(),
    entries: [
      { id: "remote-status", says: m.common_remote_connection(), terms: m.registry_remote_status_terms() },
      { id: "storage-account", says: m.common_account_storage(), terms: m.registry_storage_account_terms(), offered: account },
      { id: "account-erase", says: m.common_erase_this_account(), terms: m.registry_account_erase_terms(), offered: account },
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
