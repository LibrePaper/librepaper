// The programs the desktop app reports, as a list of rows. The Tools page and
// the Desktop app page both draw from it.

// Programs worth listing even when the desktop app did not report them.
export const COMMON = ["Quarto", "Calepin", "Zotero"];

function items(capabilities) {
  const tools = capabilities && capabilities.tools && typeof capabilities.tools === "object" ? capabilities.tools : {};
  const entries = Object.entries(tools);
  if (capabilities?.calepin) entries.push(["Calepin", capabilities.calepin]);
  if (capabilities?.zotero) entries.push(["Zotero", capabilities.zotero]);
  if (capabilities?.quarto?.tool) entries.push(["Quarto", capabilities.quarto.tool]);
  for (const builder of Array.isArray(capabilities?.builders) ? capabilities.builders : []) if (builder?.id) entries.push([builder.id, builder]);
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
