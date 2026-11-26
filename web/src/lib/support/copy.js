// Building the diagnostics report and putting it on the clipboard.
//
// Shared by the diagnostics panel in settings and by the crash notices, so a
// person who hits a crash can copy the report in one click before reloading.

import { desktop, desktopReport } from "../desktop/client.js";
import { collectReport } from "./report.js";

/// The full diagnostics text, including the desktop app's report when running
/// inside the desktop app.
export async function buildReport({ includeBuildLog = false } = {}) {
  let desktopText = "";
  if (desktop()) {
    try { desktopText = await desktopReport(); }
    catch (error) { desktopText = `Desktop app report unavailable: ${error?.message || "unknown error"}`; }
  }
  return collectReport({ includeBuildLog, desktopText });
}

/// Copies the report and resolves to its text. Rejects when nothing could be
/// copied.
export async function copyReport({ includeBuildLog = false, fallbackArea = null } = {}) {
  // The clipboard is written in the same task as the click. Safari and
  // WebKitGTK refuse a write that waits for the report first, but they accept
  // a ClipboardItem whose content is still a promise.
  const building = buildReport({ includeBuildLog });
  try {
    if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) throw new Error("no ClipboardItem");
    const blob = building.then((report) => new Blob([report], { type: "text/plain" }));
    await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
    return await building;
  } catch {
    const report = await building;
    try {
      await navigator.clipboard.writeText(report);
    } catch (error) {
      // The selection copy needs the text in a field, so put it there first.
      // The crash notices have no field of their own; a hidden one stands in,
      // or the copy would "succeed" with nothing selected.
      const area = fallbackArea || document.body.appendChild(document.createElement("textarea"));
      if (!fallbackArea) {
        area.setAttribute("readonly", "");
        area.style.cssText = "position:fixed;top:0;left:0;opacity:0;";
      }
      try {
        area.value = report;
        area.select();
        if (!document.execCommand("copy")) throw error;
      } finally {
        if (!fallbackArea) area.remove();
      }
    }
    return report;
  }
}
