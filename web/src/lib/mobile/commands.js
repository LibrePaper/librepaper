// The phone has no menu bar, so the desktop File, Edit and View menus become
// lists in a sheet. These builders offer the same commands under the same
// conditions as the menus in the Reader, with the same ids, so a command
// means one thing whichever way it was reached. The More sheet holds git,
// share, history and settings, so they are not listed here.

import { m } from "../paraglide/messages.js";
import { labelOf } from "../commands.js";

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

  const create = c.mayEdit && !c.suggestOnly
    ? [
        { id: "new-file", label: labelOf("new-file") },
        { id: "new-folder", label: labelOf("new-folder") },
        { id: "upload", label: demoLabel(labelOf("upload")), disabled: c.demo },
      ]
    : [];

  const download = [];
  if (c.mayEdit) download.push({ id: "download", label: labelOf("download") });
  download.push(exportItem("download-html", labelOf("download-html"), c.exportable.html));
  download.push(exportItem("download-pdf", labelOf("download-pdf"), c.exportable.pdf));
  download.push(exportItem("download-docx", labelOf("download-docx"), c.exportable.docx));
  if (c.canExportComments && !c.demo) download.push({ id: "download-comments", label: labelOf("download-comments") });
  if (c.mayEdit) {
    const source = { id: "download-source", label: labelOf("download-source"), disabled: !c.hasOpenFile };
    if (!c.hasOpenFile) source.reason = m.readerd_download_source_reason();
    download.push(source);
  }

  const save = [];
  if ((c.signedIn && !c.suggestOnly) || c.demo) save.push({ id: "save-template", label: demoLabel(m.readerd_save_as_template_item()), disabled: c.demo });
  if (c.mayEdit && c.sourceFormatLatex && c.compilesHere) save.push({ id: "compile", label: labelOf("compile") });

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
    { id: "lock-preview", label: labelOf("lock-preview"), disabled: !c.can["lock-preview"] },
    ...(c.can["unlock-preview"] ? [{ id: "unlock-preview", label: labelOf("unlock-preview"), disabled: false }] : []),
  ];

  // The formats are named as the desktop menu names them. They are not translated.
  const format = [
    { id: "format-html", label: "HTML", checked: c.selectedFormat === "html", disabled: !c.selectableFormat },
    { id: "format-pdf", label: "PDF", checked: c.selectedFormat === "pdf", disabled: !c.selectableFormat },
  ];

  /** @type {{ id: string, label: string, checked?: boolean }[]} */
  const markup = [
    { id: "linked", label: labelOf("linked"), checked: c.linked },
    { id: "visual-markup", label: labelOf("visual-markup"), checked: c.visual },
  ];
  // Quarto and Typst run their code on an explicit action rather than a switch.
  if (c.runCodeOffered && c.explicitRun) markup.push({ id: "run-now", label: c.running ? m.runbutton_stop() : m.runbutton_run() });
  else if (c.runCodeOffered) markup.push({ id: "run-code", label: m.readerd_run_code(), checked: c.localExecution });

  const size = [
    { id: "text-bigger", label: labelOf("text-bigger") },
    { id: "text-smaller", label: labelOf("text-smaller") },
    { id: "text-reset", label: labelOf("text-reset") },
  ];

  return [
    { items: lock },
    { items: format },
    { items: markup },
    { items: size },
  ].filter((group) => group.items.length > 0);
}
