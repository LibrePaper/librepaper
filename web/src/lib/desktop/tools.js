// The programs the desktop app reports, as a list of rows. The Tools page and
// the Desktop app page both draw from it.

// Programs worth listing even when the desktop app did not report them.
export const COMMON = ["Zotero"];

// The program a render command starts: its first argument, without the folder.
export function programOf(command) {
  const first = Array.isArray(command) ? String(command[0] || "") : "";
  return first.split(/[\\/]/).pop().replace(/\.exe$/i, "");
}

// One entry per program the render commands start. A program shared by
// several outputs is listed once, found if any of its commands found it.
function renderers(capabilities) {
  const records = capabilities?.renderers && typeof capabilities.renderers === "object" ? capabilities.renderers : {};
  const programs = new Map();
  for (const record of Object.values(records)) {
    const name = programOf(record?.command);
    if (!name) continue;
    const old = programs.get(name);
    if (!old || (!old.available && record.found) || (!old.version && record.version)) {
      programs.set(name, { available: Boolean(record.found), version: record.version || "" });
    }
  }
  return [...programs.entries()];
}

function items(capabilities) {
  const tools = capabilities && capabilities.tools && typeof capabilities.tools === "object" ? capabilities.tools : {};
  const entries = [...Object.entries(tools), ...renderers(capabilities)];
  if (capabilities?.zotero) entries.push(["Zotero", capabilities.zotero]);
  const unique = new Map();
  for (const [name, tool] of entries) {
    const key = name.toLowerCase();
    const old = unique.get(key);
    if (!old || (!old.tool?.available && tool?.available) || (!old.tool?.version && tool?.version)) unique.set(key, { name, tool });
  }
  return [...unique.values()];
}

export function toolItems(capabilities) {
  const found = items(capabilities);
  for (const name of COMMON) if (!found.some((entry) => entry.name.toLowerCase() === name.toLowerCase())) found.push({ name, tool: null });
  return found.map(({ name, tool }) => ({ key: name.toLowerCase(), title: name.charAt(0).toUpperCase() + name.slice(1), tool }));
}
