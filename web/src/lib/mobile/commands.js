// The phone has no menu bar, so the desktop File, Edit and View menus become
// lists in a sheet. These builders offer the same commands under the same
// conditions as the menus in the Reader, with the same ids, so a command
// means one thing whichever way it was reached. The More sheet holds git,
// share, history and settings, so they are not listed here.

import { m } from "../paraglide/messages.js";

// An export that cannot run yet is shown disabled with its reason, the way
// the desktop menu shows it, rather than hidden.
function exportItem(id, label, state) {
  const item = { id, label, disabled: !state.ok };
  if (!state.ok && state.reason) item.reason = state.reason;
  return item;
}

/// The groups of the File sheet for the Reader's current state. `c` carries the flags the Reader already computes for its menus.
export function fileCommands(c) {
  const demoLabel = (label) => (c.demo ? `${label} ${c.demoHint}` : label);

  const create = c.mayEdit
    ? [
        { id: "new-file", label: m.readerd_new_file() },
        { id: "new-folder", label: m.readerd_new_folder() },
        { id: "upload", label: demoLabel(m.readerd_upload_files()), disabled: c.demo },
      ]
    : [];

  const download = [];
  if (c.mayEdit) download.push({ id: "download", label: m.common_project() });
  download.push(exportItem("download-html", "HTML", c.exportable.html));
  download.push(exportItem("download-pdf", "PDF", c.exportable.pdf));
  download.push(exportItem("download-docx", "DOCX", c.exportable.docx));
  if (c.canExportComments && !c.demo) download.push({ id: "download-comments", label: m.common_comments() });
  if (c.mayEdit) {
    const source = { id: "download-source", label: m.common_source(), disabled: !c.hasOpenFile };
    if (!c.hasOpenFile) source.reason = m.readerd_download_source_reason();
    download.push(source);
  }

  const save = [];
  if (c.signedIn || c.demo) save.push({ id: "save-template", label: demoLabel(m.readerd_save_as_template_item()), disabled: c.demo });
  if (c.mayEdit && c.sourceFormatLatex && c.compilesHere) save.push({ id: "compile", label: m.common_compile_now() });

  const quit = c.desktop ? [{ id: "quit", label: m.readerd_quit() }] : [];

  return [
    { items: create },
    { label: m.common_download(), items: download },
    { items: save },
    { items: quit },
  ].filter((group) => group.items.length > 0);
}

/// The Edit sheet, from the Reader's editGroups ([id, label] pairs) and the commands it can run now.
export function editCommands(editGroups, can) {
  return editGroups
    .map((group) => ({ items: group.map(([id, label]) => ({ id, label, disabled: !can[id] })) }))
    .filter((group) => group.items.length > 0);
}

/// The View sheet. The phone has its own Preview and Source switch and no split, so the layout, editor side and ratio items of the desktop menu are left out.
export function viewCommands(c) {
  const lock = [
    { id: "lock-preview", label: m.common_lock_preview_on_file(), disabled: !c.can["lock-preview"] },
    ...(c.can["unlock-preview"] ? [{ id: "unlock-preview", label: m.common_unlock_the_preview(), disabled: false }] : []),
  ];

  // The formats are named as the desktop menu names them. They are not translated.
  const format = [
    { id: "format-html", label: "HTML", checked: c.selectedFormat === "html", disabled: !c.selectableFormat },
    { id: "format-pdf", label: "PDF", checked: c.selectedFormat === "pdf", disabled: !c.selectableFormat },
  ];

  const markup = [
    { id: "linked", label: m.readerd_keep_in_step(), checked: c.linked },
    { id: "visual-markup", label: m.readerd_visual_markup(), checked: c.visual },
  ];
  if (c.runCodeOffered) markup.push({ id: "run-code", label: m.readerd_run_code(), checked: c.localExecution });

  const size = [
    { id: "text-bigger", label: m.readerd_bigger_text() },
    { id: "text-smaller", label: m.readerd_smaller_text() },
    { id: "text-reset", label: m.readerd_reset_text_size() },
  ];

  return [
    { items: lock },
    { items: format },
    { items: markup },
    { items: size },
  ].filter((group) => group.items.length > 0);
}
