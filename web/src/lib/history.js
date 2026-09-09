// The timeline: what the document used to say, and when.
//
// The server writes a manifest at every checkpoint and serves it whole. This
// module is the reader's side of it: fetching the manifest and one
// checkpoint's files, naming a checkpoint, and -- the only part with any
// judgement in it -- turning a flat list of checkpoints into the list a person
// reads.
//
// That last part is a pure function, `timeline`, because it is the part worth
// checking: a history of two hundred marks is not a list anybody scrolls.

import { SHELL_HEADERS } from "./api.js";
import { day as isoDay } from "./dates.js";
import * as renderers from "./renderers.js";

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// Every checkpoint of a document, oldest first, as the manifest holds them.
export async function load(slug, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history`, {
    headers: asked(headers),
    // History is private and labels can change. Never let the browser answer
    // from an HTTP cache after a link has been rotated or revoked.
    cache: "no-store",
  });
  if (!response.ok) throw new Error("this document's history is not readable");
  const payload = await response.json();
  return Array.isArray(payload.checkpoints) ? payload.checkpoints : [];
}

/// What the document said at one checkpoint: which file was the document, the
/// tree, and the text of every file in it. The figures are named by digest and
/// fetched from where figures always come from, so this is texts alone.
export async function checkpoint(slug, sha, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}`, {
    headers: asked(headers),
    // The tree and bodies are immutable, but this response also contains
    // mutable labels and is access-controlled by the current request.
    cache: "no-store",
  });
  if (!response.ok) throw new Error("that checkpoint is not readable");
  return await response.json();
}

/// Names a checkpoint, or takes its name away with an empty one. Editors only,
/// which the server decides and this does not pretend to know.
export async function label(slug, sha, text, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history/${sha}`, {
    method: "PATCH",
    headers: { ...asked(headers), "content-type": "application/json" },
    body: JSON.stringify({ label: text }),
  });
  if (!response.ok) {
    const said = await response.json().catch(() => ({}));
    throw new Error(said.error || "that checkpoint could not be named");
  }
  return await response.json();
}

/// The shortest name for a checkpoint that is still a name: seven characters,
/// which is what git prints and what `librepaper label` accepts.
export const shortSha = (sha) => (sha || "").slice(0, 7);

/// The shared word-level diff, with a test seam for callers that already have
/// an engine or for checks that do not load a browser worker. The engine's
/// offsets are UTF-16 code units, which are also JavaScript string offsets.
export async function wordDiff(oldText, newText, format = "markdown", services = {}) {
  const compute = services.diff || renderers.diff;
  const edits = await compute(oldText || "", newText || "", format);
  return Array.isArray(edits) ? edits : [];
}

// Keep the shorter name convenient for panels that treat this as the history
// operation, while retaining the explicit name for code that distinguishes it
// from a visual or source diff.
export const diff = wordDiff;

/// Turns UTF-16 edits into displayable hunks. Context is measured in words and
/// is taken from the old and new text independently, so an insertion has old
/// context and a deletion still has the words that survived around it.
export function hunks(oldText, newText, edits = [], context = 6) {
  return edits.map((edit) => {
    const at = Number(edit.at) || 0;
    const deleted = Number(edit.delete) || 0;
    const oldEnd = at + deleted;
    const insert = typeof edit.insert === "string" ? edit.insert : "";
    const oldRange = windowAround(oldText || "", at, oldEnd, context);
    const newAt = newOffset(edits, edit);
    const newRange = windowAround(newText || "", newAt, newAt + insert.length, context);
    return {
      at,
      delete: deleted,
      insert,
      old: (oldText || "").slice(at, oldEnd),
      before: oldRange.before,
      after: oldRange.after,
      current: newRange.value,
      currentBefore: newRange.before,
      currentAfter: newRange.after,
      kind: deleted && insert ? "replace" : deleted ? "delete" : "insert",
    };
  });
}

/// Hunks that belong to one passage, read as one change. The word diff
/// reports "not" -> "Survived" and "shown" -> "the Auditions" as two edits;
/// a person reading a heading that was rewritten sees one. Two hunks are one
/// change when the second begins inside the first's trailing context -- that
/// is, within a few words -- and the unchanged words between them are then
/// known, because the window holds them.
///
/// Each group carries the pieces to draw it as a line of prose: `before`, the
/// `parts` (an alternating list of `{ keep }` and `{ old, insert }`), and
/// `after`, plus the `position` and `length` of the whole span in the new
/// text, which is where the frame is asked to scroll. The hunks are expected
/// in text order with `position` stamped on each, as `Reader` stamps them.
export function coalesce(hunks = []) {
  const groups = [];
  for (const hunk of hunks) {
    const position = Number(hunk.position) || 0;
    const insert = hunk.insert || "";
    const window = hunk.currentAfter || hunk.suffix || "";
    const last = groups[groups.length - 1];
    if (last) {
      const gap = position - last.end;
      if (gap >= 0 && gap <= last.window.length) {
        if (gap > 0) last.parts.push({ keep: last.window.slice(0, gap) });
        last.parts.push({ old: hunk.old || "", insert });
        last.hunks.push(hunk);
        last.end = position + insert.length;
        last.window = window;
        last.after = window;
        last.length = last.end - last.position;
        continue;
      }
    }
    groups.push({
      path: hunk.path,
      position,
      end: position + insert.length,
      length: insert.length,
      before: hunk.currentBefore || hunk.prefix || "",
      after: window,
      window,
      parts: [{ old: hunk.old || "", insert }],
      hunks: [hunk],
    });
  }
  return groups.map(({ window, end, ...group }) => group);
}

// The new-text offset of an edit is its old offset plus all prior insertions
// and deletions. Edits from librepaper-text are sorted and non-overlapping.
function newOffset(edits, edit) {
  let offset = Number(edit.at) || 0;
  for (const prior of edits) {
    if (prior === edit) break;
    offset += (prior.insert || "").length - (Number(prior.delete) || 0);
  }
  return offset;
}

function wordStarts(text) {
  const starts = [];
  const pattern = /\S+/g;
  for (let match; (match = pattern.exec(text));) starts.push([match.index, pattern.lastIndex]);
  return starts;
}

function windowAround(text, start, end, context) {
  const words = wordStarts(text);
  const left = words.filter(([, right]) => right <= start).slice(-context);
  const right = words.filter(([at]) => at >= end).slice(0, context);
  const from = left.length ? left[0][0] : 0;
  const to = right.length ? right[right.length - 1][1] : text.length;
  return {
    value: text.slice(start, end),
    before: text.slice(from, start),
    after: text.slice(end, to),
  };
}

/// How much a checkpoint changed the document's size relative to its parent,
/// for the density bar beside its row. `null` when there is nothing to draw:
/// no parent, an unlisted parent, or either end missing its `size`. The width
/// is scaled by the logarithm of the change, not the change itself, because a
/// five-byte fix and a five-kilobyte paste are both worth a bar and neither
/// should swallow the other -- the log keeps a small edit visible and a large
/// one from running off the sidebar. `grew` says which theme colour the bar
/// takes; the width is already rounded to a whole pixel, and the label reads
/// the way a diff stat does.
const SIZE_BAR_MIN = 2;
const SIZE_BAR_MAX = 48;
// Where the log scale saturates: a checkpoint that changed the document by
// this many bytes or more draws the widest bar. Fifty kilobytes is a lot of
// prose to add or remove in one sitting.
const SIZE_BAR_SATURATION = Math.log2(50_000);
export function sizeDelta(point, checkpoints) {
  if (!point?.parent || typeof point.size !== "number") return null;
  const parent = (checkpoints || []).find((candidate) => candidate.sha === point.parent);
  if (!parent || typeof parent.size !== "number") return null;
  const delta = point.size - parent.size;
  if (!delta) return null;
  const magnitude = Math.log2(Math.abs(delta) + 1);
  const width = Math.round(
    Math.max(SIZE_BAR_MIN, Math.min(SIZE_BAR_MAX, (magnitude / SIZE_BAR_SATURATION) * SIZE_BAR_MAX)),
  );
  return {
    grew: delta > 0,
    width,
    title: `${delta > 0 ? "+" : "−"}${Math.abs(delta)} byte${Math.abs(delta) === 1 ? "" : "s"}`,
  };
}

/// The day a checkpoint belongs to, in the reader's own timezone, because a
/// history is read as "Tuesday" and Tuesday is where the reader is. Written
/// the way every other date in the app is written -- see `dates.js`.
const dayOf = isoDay;

/// How many unlabelled marks by one person in a row are shown before the
/// middle of the run is folded away. Two is not a run; three is the smallest
/// number where folding hides anything at all.
const RUN = 3;

/// The manifest as a list somebody reads: newest first, grouped by day, and
/// with runs of unlabelled checkpoints by one person folded to their first and
/// last.
///
/// The folding is the whole point. A working afternoon is thirty quiet
/// checkpoints by one author, and thirty rows of the same name and the same
/// reason say less than three do. A labelled checkpoint never folds, because a
/// label is somebody saying this moment matters; nor does a run of two, since
/// folding one row saves nothing and costs a click.
///
/// Returns `[{ day, rows }]`, where a row is either `{ kind: "point", point }`
/// or `{ kind: "folded", first, last, hidden }` -- `hidden` being the
/// checkpoints between them, which the panel offers to open.
export function timeline(checkpoints) {
  const newest = [...(checkpoints || [])].reverse();
  const days = [];
  for (const point of newest) {
    const day = dayOf(point.at);
    const last = days[days.length - 1];
    if (last && last.day === day) last.points.push(point);
    else days.push({ day, points: [point] });
  }
  return days.map(({ day, points }) => ({ day, rows: fold(points) }));
}

function fold(points) {
  const rows = [];
  let at = 0;
  while (at < points.length) {
    const point = points[at];
    if (point.label) {
      rows.push({ kind: "point", point });
      at += 1;
      continue;
    }
    let end = at;
    while (
      end + 1 < points.length &&
      !points[end + 1].label &&
      points[end + 1].by === point.by
    ) {
      end += 1;
    }
    const run = points.slice(at, end + 1);
    if (run.length < RUN) {
      for (const one of run) rows.push({ kind: "point", point: one });
    } else {
      rows.push({
        kind: "folded",
        first: run[0],
        last: run[run.length - 1],
        hidden: run.slice(1, -1),
      });
    }
    at = end + 1;
  }
  return rows;
}
