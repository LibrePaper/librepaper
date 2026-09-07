// Redlines: turning the history panel's word-diff hunks into paint
// instructions for the frame, and working out whose name belongs on them.
// `docs/specs/track-changes.md`, "Browser: redlines", is the contract.
//
// Pure functions only, so `web/checks/redlines.mjs` can check both without a
// browser: the hunk shapes come from `history.hunks` (see `Reader.svelte`'s
// `computeHistoryChanges`, which stamps a rendered-text `position` onto each
// one), and the checkpoint shape is the manifest's `{sha, at, by, why,
// label}`, oldest first.

// Who made the change: the author of every checkpoint strictly after the
// baseline, up to and including the compare point when the reader picked
// one (through the live document otherwise). One name when they all agree,
// "several people" when they do not, and "" when there is nothing to
// attribute -- the baseline is not on the list, or nothing followed it.
export function attribution(checkpoints = [], baselineSha, targetSha = null) {
  const baselineIndex = checkpoints.findIndex((point) => point.sha === baselineSha);
  if (baselineIndex < 0) return "";
  const endIndex = targetSha
    ? checkpoints.findIndex((point) => point.sha === targetSha)
    : checkpoints.length - 1;
  if (endIndex < baselineIndex) return "";
  const authors = new Set(
    checkpoints
      .slice(baselineIndex + 1, endIndex + 1)
      .map((point) => point.by)
      .filter(Boolean),
  );
  if (authors.size === 0) return "";
  if (authors.size === 1) return [...authors][0];
  return "several people";
}

// One item per hunk, in the shape the frame's `redlines()` painter expects:
// `{start, end, kind:"insert", who}` for text now present, `{at, kind:
// "delete", text, who}` for text no longer there. A hunk that both removed
// and added text (`kind: "replace"`) becomes both an insert and a delete
// item sharing one offset -- what was there struck through immediately
// before what replaced it, the way the passage reads.
export function itemsFor(hunks = [], who = "") {
  const items = [];
  for (const hunk of hunks) {
    const start = Number(hunk.position) || 0;
    if (hunk.kind === "insert" || hunk.kind === "replace") {
      const insert = hunk.insert || "";
      if (insert) items.push({ start, end: start + insert.length, kind: "insert", who });
    }
    if (hunk.kind === "delete" || hunk.kind === "replace") {
      const deleted = hunk.old || "";
      if (deleted) items.push({ at: start, kind: "delete", text: deleted, who });
    }
  }
  return items;
}
