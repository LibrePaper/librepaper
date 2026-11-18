import { CONTEXT_LIMIT } from "./assistant.js";

// The bounding for diagnostics sent to an agent, shared by the chat client
// and the compile reply so both stay identical.
// Cut by code points so a surrogate pair is never split.
function clip(text, limit) {
  return text.length <= limit ? text : Array.from(text).slice(0, limit).join("");
}

export function compactDiagnostic(item) {
  if (!item || typeof item !== "object") return { severity: "error", message: String(item || "Candidate verification failed.") };
  const compact = {};
  for (const key of ["severity", "file", "line", "column", "message", "source"]) {
    if (item[key] === undefined || item[key] === null) continue;
    compact[key] = typeof item[key] === "string" ? clip(item[key], key === "source" ? 2048 : 1024) : item[key];
  }
  return compact;
}

export function boundedDiagnostics(value) {
  const all = Array.isArray(value) ? value.map(compactDiagnostic) : [];
  const ordered = [...all.filter((item) => item.severity === "error"), ...all.filter((item) => item.severity !== "error")];
  const selected = [];
  let skipped = 0;
  const encoded = (items) => new TextEncoder().encode(JSON.stringify(items)).byteLength;
  for (const item of ordered) {
    if (encoded([...selected, item]) <= CONTEXT_LIMIT) selected.push(item);
    else skipped++;
  }
  if (!skipped) return selected;
  const truncation = { severity: "warning", message: `${skipped} additional preview diagnostics were omitted.` };
  while (selected.length && encoded([...selected, truncation]) > CONTEXT_LIMIT) selected.pop();
  return [...selected, truncation];
}
