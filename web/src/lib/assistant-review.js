// A writing pass stays one reviewable unit even when its annotations are
// interleaved with ordinary comments. Input order is the reader's document order.
export function reviewGroups(comments) {
  const groups = [];
  const passes = new Map();
  for (const comment of comments) {
    const pass = comment.motivation === "editing" && typeof comment.pass === "string" ? comment.pass : "";
    if (!pass) {
      groups.push({ key: `comment:${comment.id}`, pass: "", comments: [comment] });
      continue;
    }
    let group = passes.get(pass);
    if (!group) {
      group = { key: `pass:${pass}`, pass, comments: [] };
      passes.set(pass, group);
      groups.push(group);
    }
    group.comments.push(comment);
  }
  return groups.map((group) => ({ ...group, pending: group.comments.filter((comment) => !comment.resolved && !comment.outcome) }));
}

export function diagnosticContext(item, tree, revision) {
  const file = item.file || tree.main || "";
  const line = Number.isInteger(item.line) && item.line > 0 ? item.line : 0;
  return { ...item, file, line, revision,
    source: line ? (tree.texts?.[file] || "").split(/\r?\n/)[line - 1] || "" : "" };
}

// Await each decision so failures are individually visible and successful
// decisions survive a later refusal. There is deliberately no bulk accept.
export async function rejectPass(items, reject) {
  const results = [];
  for (const item of items) {
    if (item.resolved || item.outcome || item.deciding) continue;
    try {
      await reject(item);
      results.push({ id: item.id, rejected: true });
    } catch (error) {
      results.push({ id: item.id, rejected: false, error: error.message || "Rejection failed." });
    }
  }
  return results;
}
