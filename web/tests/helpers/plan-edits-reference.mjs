// A frozen copy of the planner as it was before it was made linear, for the
// fuzz in tests/unit/track-changes.mjs to compare the real one against. Do not
// edit it to follow src/lib/track-changes.js.

export function planEditsReference(before, state, edits, me, { run = null, now, mintId, untracked = false }) {
  let cur = before;
  let changes = state.changes.map((c) => ({ ...c, state: "kept" }));
  let gaps = state.gaps.map((g) => ({ ...g, fragments: g.fragments.map((f) => ({ ...f })), state: "kept" }));
  const originalChanges = new Set(state.changes.map((c) => c.id));
  const originalGaps = new Set(state.gaps.map((g) => g.id));
  const removedChanges = new Map();
  const removedGaps = new Map();
  let adj = 0;
  for (const edit of edits) {
    const ef = edit.from + adj;
    const et = edit.to + adj;
    const insert = edit.insert;
    if (ef === et && insert.length === 0) continue;
    const step = planOneReference(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked);
    for (const c of changes) if (!step.changes.some((x) => x.id === c.id) && originalChanges.has(c.id)) removedChanges.set(c.id, { ...c, state: "removed" });
    for (const g of gaps) if (!step.gaps.some((x) => x.id === g.id) && originalGaps.has(g.id)) removedGaps.set(g.id, { ...g, state: "removed" });
    changes = step.changes;
    gaps = step.gaps;
    cur = cur.slice(0, ef) + insert + cur.slice(et);
    adj += insert.length - (et - ef);
  }
  for (const c of changes) if (!originalChanges.has(c.id) && c.state === "kept") c.state = "set";
  for (const g of gaps) if (!originalGaps.has(g.id) && g.state === "kept") g.state = "set";
  return {
    changes: [...changes, ...removedChanges.values()],
    gaps: [...gaps, ...removedGaps.values()],
  };
}

function hasSpanReference(c) {
  return c.to > c.from;
}

function planOneReference(cur, changes, gaps, ef, et, insert, me, run, now, mintId, untracked) {
  const len = insert.length;
  const delta = len - (et - ef);
  // Own changes this edit touches: a span overlapping or adjacent, or a
  // fragment in a gap inside the closed range.
  const swallowedGaps = gaps.filter((g) => g.at >= ef && g.at <= et);
  const owned = new Set();
  // A span of mine joins when it touches the edit. A change of mine with no
  // span joins when one of its fragments sits in a swallowed gap. A span of
  // mine elsewhere stays its own change: one change holds one span, and a
  // span never widens over text that is not its own.
  // Spans never merge: a change holds one span, and a record is never
  // emptied for another record's sake. Only the first own span touching
  // the edit takes it; another own span there is handled like any other
  // span, so it shrinks, splits or moves and keeps its record and its text.
  const touching = changes.filter((c) => c.who === me.who && hasSpanReference(c) && c.from <= et && c.to >= ef)
    .sort((a, b) => a.from - b.from || (a.id < b.id ? -1 : 1));
  if (touching.length > 0) owned.add(touching[0].id);
  for (const g of swallowedGaps) for (const f of g.fragments) {
    const c = changes.find((x) => x.id === f.change);
    // A fragment read as a change of its own is not joined: it has no record
    // to merge into, and it never leaves the gap it is in.
    if (c && c.who === me.who && !hasSpanReference(c) && !c.orphan) owned.add(c.id);
  }
  // Pure deletion of text inside inserted spans only, by nobody of mine: the
  // spans shrink and nothing is recorded, unless original text is involved.
  const spanAt = (p) => changes.find((c) => hasSpanReference(c) && c.from <= p && p < c.to);
  const originalRuns = [];
  {
    let p = ef;
    while (p < et) {
      const s = spanAt(p);
      if (s) { p = Math.min(et, s.to); continue; }
      let q = p;
      while (q < et && !spanAt(q)) q++;
      originalRuns.push({ from: p, to: q });
      p = q;
    }
  }
  // Is there anything to record? Original text deleted, or text inserted.
  // An untracked edit records nothing and makes no change of mine; it still
  // moves, shrinks and splits spans and merges the gaps it collapses, so the
  // records keep describing the text.
  const records = !untracked && (originalRuns.length > 0 || len > 0);
  if (untracked) owned.clear();
  let mine = null;
  if (records) {
    const first = [...owned].map((id) => changes.find((c) => c.id === id)).sort((a, b) => a.from - b.from)[0];
    mine = first
      ? { ...first, run: run ?? first.run, at: now, state: "set" }
      : { id: mintId(), who: me.who, author: me.author, run, at: now, from: ef, to: ef, state: "set" };
  }
  // Build the merged gap: walk ef..et, gaps at p then the original char at p.
  // With text inserted, what sat at `et` was after the deleted text and goes
  // after the new span; everything else goes before it. When a span or a
  // fragment of mine sits at `et` it joins the new change, and then the whole
  // of what sat there goes before the span: my deleted text belongs with it.
  const ownAtEnd = changes.some((c) => c.who === me.who && hasSpanReference(c) && c.from === et && owned.has(c.id))
    || swallowedGaps.some((g) => g.at === et && g.fragments.some((f) => owned.has(f.change)));
  const splitAtEnd = len > 0 && !ownAtEnd;
  const merged = [];
  const after = [];
  // Adjacent pieces of one change join into one fragment only while neither is
  // stored yet. A stored fragment is never rewritten or dropped here: its
  // author's other session may be extending it, or a reviewer deciding its
  // change, at this very moment, and either write would be lost. A stored
  // fragment keeps its id and rank through the walk, so writing the gap back
  // changes nothing about it unless the order asks for it.
  const pushFragment = (list, change, text, id, rank) => {
    const last = list[list.length - 1];
    if (last && last.change === change && !last.id && !id) last.text += text;
    else list.push({ change, text, ...(id ? { id } : {}), ...(typeof rank === "number" ? { rank } : {}) });
  };
  for (let p = ef; p <= et; p++) {
    const target = splitAtEnd && p === et ? after : merged;
    // Untracked edits can leave two gaps on one point; every one of them is taken.
    for (const g of swallowedGaps) if (g.at === p) for (const f of g.fragments) pushFragment(target, mine && owned.has(f.change) ? mine.id : f.change, f.text, f.id, f.rank);
    if (p < et && !spanAt(p) && records) pushFragment(merged, mine.id, cur[p]);
  }
  // Spans.
  const outChanges = [];
  const newSpans = [];
  for (const c of changes) {
    if (mine && owned.has(c.id)) {
      // Mine, or an own deletion whose fragments now carry the merged id. The
      // latter's record stays, untouched and empty: another session of the
      // author may be adding to it at this moment, and prune drops it once it
      // has rested empty.
      if (c.id !== mine.id) outChanges.push({ ...c, from: ef + len, to: ef + len, emptied: true });
      continue;
    }
    if (!hasSpanReference(c)) {
      // A pure-deletion change: its place follows its fragments. Keep it unless
      // all its fragments vanished (handled below).
      outChanges.push(c);
      continue;
    }
    // A span whose boundary sits exactly where text is inserted is written
    // again: a cursor left on a deleted character there could drift across
    // the new text, and the write re-pins it only if it did.
    if (c.to <= ef) { outChanges.push(len > 0 && c.to === ef ? { ...c, state: "set" } : c); continue; }
    if (c.from >= et) { outChanges.push({ ...c, from: c.from + delta, to: c.to + delta, ...(len > 0 && c.from === et ? { state: "set" } : {}) }); continue; }
    // Foreign span overlapping the edit.
    if (c.from < ef && c.to > et && len > 0) {
      // Split around the inserted text.
      outChanges.push({ ...c, to: ef, state: "set" });
      newSpans.push({ ...c, id: mintId(), from: ef + len, to: c.to + delta, state: "set" });
      continue;
    }
    const from = c.from < ef ? c.from : ef + len;
    const to = c.to > et ? c.to + delta : (c.from < ef ? ef : ef + len);
    if (to > from) outChanges.push({ ...c, from, to, state: "set" });
    else {
      // The whole span went from this view. The record stays, untouched: its
      // author, in this or another session, may be extending it at this
      // moment, and a stale cursor resolves to the right place anyway. Prune
      // drops it once it has rested empty for a while.
      outChanges.push({ ...c, from: ef + len, to: ef + len, emptied: true });
    }
  }
  outChanges.push(...newSpans);
  if (mine) {
    // The merged span: my overlapping spans plus the inserted text.
    let from = ef;
    let to = ef + len;
    for (const id of owned) {
      const c = changes.find((x) => x.id === id);
      if (!hasSpanReference(c)) continue;
      if (c.from < from) from = c.from;
      if (c.to > et) to = Math.max(to, c.to + delta);
    }
    mine.from = from;
    mine.to = to;
    outChanges.push(mine);
  }
  // Gaps.
  const outGaps = [];
  for (const g of gaps) {
    if (g.at < ef) { outGaps.push(g); continue; }
    if (g.at > et) { outGaps.push({ ...g, at: g.at + delta }); continue; }
  }
  const ids = swallowedGaps.map((g) => g.id);
  if (merged.length > 0) {
    // One gap per position: gaps already at the span's start, which held text
    // replaced by its earlier part, take the new fragments after their own.
    const pos = mine ? mine.from : ef;
    const there = outGaps.filter((g) => g.at === pos);
    if (there.length > 0) {
      const list = [];
      for (const g of there) for (const f of g.fragments) pushFragment(list, f.change, f.text, f.id, f.rank);
      for (const f of merged) pushFragment(list, f.change, f.text, f.id, f.rank);
      const rest = outGaps.filter((g) => g.at !== pos);
      rest.push({ ...there[0], fragments: list, state: "set" });
      outGaps.length = 0;
      outGaps.push(...rest);
    } else outGaps.push({ id: ids.shift() ?? mintId(), at: pos, fragments: merged, state: "set" });
  }
  if (after.length > 0) {
    outGaps.push({ id: ids.shift() ?? mintId(), at: ef + len, fragments: after, state: "set" });
  }
  // The changes that merged into mine are gone; their fragments anywhere
  // else now belong to mine.
  if (mine) {
    for (let i = 0; i < outGaps.length; i++) {
      const g = outGaps[i];
      if (!g.fragments.some((f) => owned.has(f.change) && f.change !== mine.id)) continue;
      const list = [];
      for (const f of g.fragments) pushFragment(list, owned.has(f.change) ? mine.id : f.change, f.text, f.id, f.rank);
      outGaps[i] = { ...g, fragments: list, state: "set" };
    }
  }
  // A change with no span and no fragment is gone.
  const referenced = new Set(outGaps.flatMap((g) => g.fragments.map((f) => f.change)));
  // A change whose span went but whose deletions remain is a deletion now,
  // not an empty record.
  for (const c of outChanges) if (c.emptied && referenced.has(c.id)) delete c.emptied;
  const live = outChanges.filter((c) => hasSpanReference(c) || referenced.has(c.id) || c.emptied);
  live.sort((a, b) => a.from - b.from || a.to - b.to);
  outGaps.sort((a, b) => a.at - b.at);
  // Pure-deletion changes take their place from their first fragment.
  for (const c of live) if (!hasSpanReference(c)) {
    const g = outGaps.find((x) => x.fragments.some((f) => f.change === c.id));
    if (g && c.from !== g.at) { c.from = g.at; c.to = g.at; }
  }
  return { changes: live, gaps: outGaps };
}
