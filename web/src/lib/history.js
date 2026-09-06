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

const asked = (headers) => ({ ...SHELL_HEADERS, ...headers });

/// Every checkpoint of a document, oldest first, as the manifest holds them.
export async function load(slug, headers = {}) {
  const response = await fetch(`/api/documents/${slug}/history`, { headers: asked(headers) });
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
/// which is what git prints and what `komodoc label` accepts.
export const shortSha = (sha) => (sha || "").slice(0, 7);

/// The day a checkpoint belongs to, in the reader's own timezone, because a
/// history is read as "Tuesday" and Tuesday is where the reader is.
function dayOf(at) {
  const when = new Date(at);
  return Number.isNaN(when.getTime()) ? "" : when.toLocaleDateString();
}

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
