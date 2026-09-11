// Where a passage went.
//
// A comment records what it was made about -- the quotation -- and, since the
// timeline, which checkpoint it was made on. When the passage is still in the
// document the reader finds it and there is nothing to say. When it is not,
// the reader has been saying "Needs re-anchoring", which tells the person who
// wrote the comment nothing they did not already know. What they want is where
// it went, and the two records together answer that: the passage was in the
// text at the comment's own checkpoint and is not in the text now, so there is
// a checkpoint between the two where it stopped being found.
//
// Finding it is a search, not a walk. "Found" only goes one way -- a passage
// that came back would be a passage that was never gone in a way anybody means
// -- so a bisection finds the moment in a handful of checkpoints rather than
// all of them, and each one it looks at is cached for every other comment that
// asks about the same moment.
//
// This is deliberately not the word-level diff. What replaced the passage is
// `history.js`'s `wordDiff` and needs the diff crate in the browser; what is
// here needs nothing beyond the checkpoints, and it is the half of
// the question a reviewer actually asks.
//
// A comment with a source anchor is searched for in the source file itself,
// which is why it is preferred over the rendered quote whenever there is one:
// it needs no render and, for a LaTeX document, no PDF either.

import { anchorOne, flatten } from "./anchor.js";
import * as figures from "./figures.js";
import * as history from "./history.js";
import * as renderers from "./renderers.js";
import { projectHtml, validateProjection, PROJECTION_VERSION } from "./diff-display.js";

// Passage lookups can outlive a single comment card in a long-lived reader.
// Keep enough history for the usual review, while making navigation across
// many documents unable to retain every source and rendered string forever.
const CACHE_LIMIT = 64;

// A checkpoint SHA can be shared by documents and a reader can arrive with a
// link key. Partition entries by the supplied authentication context. The
// value is kept only in this private in-memory key and is never sent or
// logged; the server still checks authorization on every miss.
const scopes = new Map();
let nextScope = 1;
let cacheGeneration = 0;
function authScope(headers) {
  const entries = typeof Headers !== "undefined" && headers instanceof Headers
    ? Array.from(headers.entries())
    : Object.entries(headers || {});
  const fingerprint = JSON.stringify(entries
    .filter(([name]) => name.toLowerCase() !== "x-librepaper-client")
    .map(([name, value]) => `${name.toLowerCase()}:${String(value)}`)
    .sort());
  // Cache keys contain an opaque process-local scope, never bearer tokens or
  // link keys. Clearing the passage cache retires these scopes on sign-out or
  // document-link changes.
  if (!scopes.has(fingerprint)) scopes.set(fingerprint, `scope-${nextScope++}`);
  return scopes.get(fingerprint);
}

const cacheKey = (slug, sha, headers) => `${slug}\u0000${sha}\u0000${authScope(headers)}`;

function remember(cache, key, value) {
  cache.set(key, value);
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

// One rendering per document/checkpoint, whatever asks for it. A document
// under review has a handful of comments on one or two moments, so this is
// nearly always one entry deep.
const htmlRendered = new Map();
const projections = new Map();
const projectionSizes = new Map();
const htmlSizes = new Map();
const HTML_CACHE_ENTRIES = 32;
const HTML_CACHE_BYTES = 12 * 1024 * 1024;

function assetEvidence(tree, gathered) {
  const paths = {};
  for (const [path, digest] of Object.entries(tree.digests || {})) {
    if (path.length > 4_096) continue;
    if (!Object.prototype.hasOwnProperty.call(gathered?.assets || {}, path)) continue;
    const value = String(digest || "");
    if (!/^[0-9a-f]{64}$/.test(value)) continue;
    const url = gathered?.urls?.[path];
    paths[path] = {
      digest: value,
      ...(typeof url === "string" && url && url.length <= 1_024 ? { url } : {}),
    };
  }
  return { paths };
}

function rememberHtml(key, value) {
  htmlRendered.set(key, value);
  while (htmlRendered.size > HTML_CACHE_ENTRIES) {
    const oldest = htmlRendered.keys().next().value;
    if (oldest === undefined) break;
    htmlRendered.delete(oldest);
    htmlSizes.delete(oldest);
  }
}

function accountHtml(key, html) {
  htmlSizes.set(key, typeof html === "string" ? html.length * 2 : 0);
  let total = [...htmlSizes.values()].reduce((sum, bytes) => sum + bytes, 0);
  while (total > HTML_CACHE_BYTES && htmlRendered.size) {
    const oldest = htmlRendered.keys().next().value;
    if (oldest === undefined) break;
    htmlRendered.delete(oldest);
    total -= htmlSizes.get(oldest) || 0;
    htmlSizes.delete(oldest);
  }
}

/// The visible text of a checkpoint, derived from its contemporary HTML
/// render. Historical PDFs are intentionally not used for passage lookup.
///
/// It is not the agent's own walk -- that runs in another origin, over a live
/// DOM -- so the two can differ at a whitespace boundary. That is what
/// `anchorOne`'s flattened second pass is for, and it is why this is used to
/// answer "is the passage here" rather than to place anything.
export async function htmlAt(slug, sha, headers = {}, services = {}) {
  const generation = cacheGeneration;
  const pointApi = services.history || history;
  const point = await pointApi.checkpoint(slug, sha, headers);
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  return (await renderTree(slug, point, headers, services)).html;
}

// Both the reader's clean target and the comparison use this exact render.
// Captured current trees use this entry point directly, without pretending
// their digest is a retained checkpoint that can be fetched from the server.
export async function renderTree(slug, point, headers = {}, services = {}) {
  const generation = cacheGeneration;
  const scope = authScope(headers);
  const rendererApi = services.renderers || renderers;
  const figureApi = services.figures || figures;
  const configuration = services.configuration || (rendererApi.htmlConfiguration
    ? await rendererApi.htmlConfiguration(point) : { identity: services.identity || JSON.stringify(point.settings || {}) });
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  if (!configuration || typeof configuration.identity !== "string" || !configuration.identity) {
    throw new Error("Historical renderer configuration is unavailable");
  }
  const key = JSON.stringify([point.storage_id || slug, point.tree_sha || point.sha,
    scope, configuration.identity, PROJECTION_VERSION]);
  if (htmlRendered.has(key)) return htmlRendered.get(key);
  const pending = (async () => {
    const tree = {
      main: point.main,
      texts: point.texts || {},
      settings: point.settings || {},
      digests: { ...point.digests },
      files: point.files || {},
    };
    for (const [path, file] of Object.entries(point.files || {})) {
      if (file.kind !== "text") tree.digests[path] = file.sha;
    }
    // Historical comparisons always use the current HTML-capable renderer.
    // In particular, never extract text from a saved PDF: a PDF is an
    // ordinary latest-publication artifact, not the history display source.
    const gathered = point.assets && Object.keys(tree.digests).every((path) => point.assets[path])
      ? { assets: point.assets, urls: point.urls || {} }
      : await figureApi.gather(slug, tree.digests, headers, { strict: true });
    const capturedAssets = gathered?.assets;
    if (!capturedAssets || !Object.keys(tree.digests).every((path) =>
      Object.prototype.hasOwnProperty.call(capturedAssets, path))) {
      throw new Error("Captured comparison assets are unavailable");
    }
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    const result = await rendererApi.render(
      { ...tree, assets: gathered.assets, urls: gathered.urls },
      point.label || "Document",
      { format: "html", configuration },
    );
    if (typeof result?.html !== "string") throw new Error("This endpoint could not be rendered as HTML. Use source comparison.");
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    return {
      ...result,
      cacheKey: key,
      rendererIdentity: configuration.identity,
      // Projection must be able to distinguish verified captured assets from
      // arbitrary renderer markup. Keep only path/digest/URL evidence; bytes
      // never cross into the projection or frame payload.
      assetEvidence: assetEvidence(tree, gathered),
    };
  })();
  rememberHtml(key, pending);
  pending.then(
    (value) => {
      if (htmlRendered.get(key) === pending) accountHtml(key, value.html);
    },
    () => { if (htmlRendered.get(key) === pending) htmlRendered.delete(key); },
  );
  return pending;
}

export async function textAt(slug, sha, headers = {}, services = {}) {
  const html = await htmlAt(slug, sha, headers, services);
  return typeof html === "string" ? visibleText(html) : null;
}

// One checkpoint fetch per document/sha, whatever asks for it -- the same
// sharing `textAt` gives the rendering, kept separately because a comment can
// have both a source anchor and, on an older version, a rendered one, and the
// two must not evict each other.
const points = new Map();

/// Drop historical passage responses when a reader changes link or signs out.
/// In-flight calls may finish, but their identity checks cannot repopulate a
/// map entry that has since been replaced.
export function clearPassageCache() {
  cacheGeneration++;
  htmlRendered.clear();
  htmlSizes.clear();
  projections.clear();
  projectionSizes.clear();
  points.clear();
  scopes.clear();
}

/** Render and project a captured tree without putting it in history caches. */
export async function projectionTree(tree, title = "Document", services = {}) {
  const result = await renderTree(services.slug || tree.storage_id || "", { ...tree, label: title }, services.headers || {}, services);
  const projection = projectHtml(result.html, { ...(services.projection || {}), assetEvidence: result.assetEvidence });
  return validateProjection(projection) ? projection : { ...projection, complete: false, incomplete: "invalid-projection" };
}

export async function captureTree(slug, tree, headers = {}) {
  const generation = cacheGeneration;
  const digests = { ...tree.digests };
  for (const [path, file] of Object.entries(tree.files || {})) if (file.kind !== "text") digests[path] = file.sha;
  const gathered = await figures.gather(slug, digests, headers, { strict: true });
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  return { ...tree, digests, assets: Object.fromEntries(Object.entries(gathered.assets).map(([path, bytes]) => [path, bytes.slice()])), urls: { ...gathered.urls } };
}

/** Historical endpoint projection, keyed by checkpoint and authorization scope. */
export async function projectionAt(slug, sha, headers = {}, services = {}) {
  const generation = cacheGeneration;
  // Revalidate retained membership and authorization before a private cache hit.
  const point = await (services.history || history).checkpoint(slug, sha, headers);
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  const result = await renderTree(slug, point, headers, services);
  if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
  const key = result.cacheKey;
  if (projections.has(key)) return projections.get(key);
  const projection = projectHtml(result.html, { ...(services.projection || {}), assetEvidence: result.assetEvidence });
  const value = validateProjection(projection) ? projection : { ...projection, complete: false, incomplete: "invalid-projection" };
  remember(projections, key, value);
  projectionSizes.set(key, JSON.stringify(value).length * 2);
  for (const old of projectionSizes.keys()) if (!projections.has(old)) projectionSizes.delete(old);
  let total = [...projectionSizes.values()].reduce((sum, bytes) => sum + bytes, 0);
  while (total > HTML_CACHE_BYTES && projections.size) {
    const old = projections.keys().next().value;
    projections.delete(old);
    total -= projectionSizes.get(old) || 0;
    projectionSizes.delete(old);
  }
  return value;
}

/// The text of one file of a checkpoint, as it was written. Unlike `textAt`
/// this asks for nothing but the tree the server already has on hand: no
/// render, no figures gathered, no PDF fetched. A path the checkpoint does
/// not have -- the file did not exist yet, or has since been renamed away --
/// answers null, which `holds` already reads as "not here" rather than as
/// "unknown", the same as an empty page would.
export async function sourceTextAt(slug, sha, path, headers = {}) {
  const key = cacheKey(slug, sha, headers);
  if (!points.has(key)) {
    const pending = history.checkpoint(slug, sha, headers);
    remember(points, key, pending);
    // A failed request must not poison this checkpoint forever. The identity
    // check preserves a newer retry if eviction or another caller replaced it.
    pending.catch(() => {
      if (points.get(key) === pending) points.delete(key);
    });
  }
  const pending = points.get(key);
  const generation = cacheGeneration;
  try {
    const point = await pending;
    if (generation !== cacheGeneration) throw new Error("Comparison authorization changed");
    const texts = point.texts || {};
    return Object.prototype.hasOwnProperty.call(texts, path) ? texts[path] : null;
  } finally {
    // Coalesce simultaneous lookups, but revalidate authorization and
    // retained membership before every later use of a private source tree.
    if (points.get(key) === pending) points.delete(key);
  }
}

// Source-only provenance support. This deliberately uses checkpoint source
// bytes and the existing bounded source diff; it never calls renderTree,
// htmlAt, or any compiler for an intermediate checkpoint.
function mapSourceSpan(span, edits) {
  let shift = 0;
  let start = span.start;
  const end = span.end;
  const pieces = [];
  for (const edit of edits) {
    const at = Number(edit.at) || 0;
    const removed = Number(edit.delete) || 0;
    const editEnd = at + removed;
    const delta = (edit.insert || "").length - removed;
    if (editEnd <= start) { shift += delta; continue; }
    if (at >= end) break;
    if (at > start) pieces.push({ ...span, start: start + shift, end: at + shift });
    shift += delta;
    start = Math.max(start, editEnd);
  }
  if (start < end) pieces.push({ ...span, start: start + shift, end: end + shift });
  return pieces;
}

/**
 * Build bounded source provenance for a retained checkpoint chain. The
 * returned ranges are in final-target source coordinates and are marked
 * validated only because they come from exact source diffs, never quotation
 * matching. Pure deletions intentionally produce no target span and remain
 * unlabelled by the refinement layer.
 */
export async function sourceProvenance({
  slug, baseline, checkpoints = [], target, headers = {}, history: historyApi = history,
  changedPaths = [], maxIntervals = 12,
} = {}) {
  if (!baseline || !target || target._current || !Array.isArray(checkpoints)
    || checkpoints.length === 0 || checkpoints.length > maxIntervals) return null;
  const points = [baseline, ...checkpoints];
  const paths = new Set(changedPaths);
  for (const point of points) for (const path of Object.keys(point?.texts || {})) paths.add(path);
  const source = new Map();
  const read = async (point, path) => {
    const key = `${point.sha}\u0000${path}`;
    if (source.has(key)) return source.get(key);
    let value;
    if (point.texts && Object.prototype.hasOwnProperty.call(point.texts, path)) {
      value = point.texts[path];
    } else if (historyApi.checkpoint) {
      const fetched = await historyApi.checkpoint(slug, point.sha, headers);
      value = fetched?.texts?.[path];
    } else {
      value = await sourceTextAt(slug, point.sha, path, headers);
    }
    source.set(key, typeof value === "string" ? value : "");
    return source.get(key);
  };
  const editsByStep = [];
  const rangesByStep = checkpoints.map(() => []);
  const sourceBytesByStep = checkpoints.map(() => 0);
  const diffWorkByStep = checkpoints.map(() => 0);
  for (let step = 0; step < checkpoints.length; step += 1) {
    const oldPoint = points[step];
    const newPoint = points[step + 1];
    const allEdits = new Map();
    for (const path of paths) {
      const oldText = await read(oldPoint, path);
      const newText = await read(newPoint, path);
      if (oldText === newText) continue;
      sourceBytesByStep[step] += oldText.length + newText.length;
      const edits = await historyApi.wordDiff(oldText, newText);
      diffWorkByStep[step] += edits.length;
      allEdits.set(path, edits);
      for (const hunk of historyApi.hunks(oldText, newText, edits)) {
        const insert = hunk.insert || "";
        if (!insert) continue;
        rangesByStep[step].push({
          path, start: Number(hunk.position) || 0,
          end: (Number(hunk.position) || 0) + insert.length,
          validated: true,
        });
      }
    }
    editsByStep.push(allEdits);
  }
  // Move each interval's inserted source spans through every later source
  // edit so they can be compared with final-target projection ranges.
  for (let step = 0; step < rangesByStep.length; step += 1) {
    let carried = rangesByStep[step];
    for (let later = step + 1; later < editsByStep.length; later += 1) {
      carried = carried.flatMap((span) => mapSourceSpan(span, editsByStep[later].get(span.path) || []));
    }
    rangesByStep[step] = carried;
  }
  return checkpoints.map((point, index) => ({
    sha: point.sha,
    original_parent: point.original_parent,
    authorship: point.authorship,
    sourceRanges: rangesByStep[index],
    sourceBytes: sourceBytesByStep[index],
    diffWork: diffWorkByStep[index],
  }));
}

/// The words of a page, with the markup and the things that are not words
/// taken out.
export function visibleText(html) {
  return projectHtml(html).text;
}

/// Whether a quotation is in a text, by the same match that anchors it in the
/// document: exactly, or with whitespace flattened on both sides.
export function holds(text, comment) {
  return typeof text === "string" && Boolean(anchorOne(text, comment, flatten(text)));
}

/// The first checkpoint at which a comment's passage was no longer found.
///
/// `checkpoints` is the manifest, oldest first. The search starts at the
/// comment's own checkpoint -- or at the oldest the manifest still has, for a
/// comment made before that was recorded -- and ends at the newest, where the
/// caller has already established that the passage is gone. Returns the
/// manifest entry, or null when there is nothing to say: no history to look
/// in, or a passage that turns out still to be there.
export async function wentAt(slug, comment, checkpoints, headers = {}, at = textAt, atSource = sourceTextAt) {
  if (!checkpoints?.length) return null;
  // A comment from before checkpoints were recorded on one, or one whose
  // checkpoint has since been shed, is read as made on the oldest moment the
  // manifest still has. That is the earliest thing that could be true of it.
  const own = checkpoints.findIndex((point) => point.sha === comment.revision);
  let low = own >= 0 ? own : 0;
  let high = checkpoints.length - 1;
  if (low >= high) return null;

  // A source anchor is searched for in the source file it names, at every
  // checkpoint, and a comment without one falls back to the rendered
  // quotation exactly as before -- the two are just different ways of
  // reading a checkpoint into a string to run `holds` against.
  const source = comment.source;
  const selector = source || comment;
  const read = source
    ? (sha) => atSource(slug, sha, source.path, headers)
    : (sha) => at(slug, sha, headers);

  // The passage has to have been there to have gone. A comment whose own
  // checkpoint does not hold it is one whose quotation this cannot reason
  // about -- a figure annotation, or a passage the renderer no longer emits --
  // and saying nothing is better than naming a moment at random.
  const ownText = await read(checkpoints[low].sha);
  if (typeof ownText !== "string" || !holds(ownText, selector)) return null;
  const newestText = await read(checkpoints[high].sha);
  if (typeof newestText !== "string" || holds(newestText, selector)) return null;

  // Invariant: it is in `low` and not in `high`. Each step halves the gap, so
  // the answer costs about five renders on a history of thirty.
  while (high - low > 1) {
    const middle = (low + high) >> 1;
    const middleText = await read(checkpoints[middle].sha);
    if (typeof middleText !== "string") return null;
    if (holds(middleText, selector)) low = middle;
    else high = middle;
  }
  return checkpoints[high];
}

/// Finds what replaced a comment's quotation between two versions. The
/// returned text is the quoted old range transformed through the edits, so
/// unchanged words between two replacements remain visible (`blue fox at calm
/// river`, rather than just the two inserted fragments). Null means the
/// quotation could not be anchored or the versions did not replace any part of
/// it. A whitespace-only insertion is retained because it can be the only
/// change inside a quoted source selection.
export async function replacementAt(oldText, newText, selector, diffApi) {
  if (typeof oldText !== "string" || typeof newText !== "string" || !selector?.exact) return null;
  const found = anchorOne(oldText, selector, flatten(oldText));
  if (!found) return null;
  const compute = diffApi || ((before, after) => history.wordDiff(before, after));
  const edits = await compute(oldText, newText);
  const overlapping = edits.filter((edit) => {
    const start = Number(edit.at) || 0;
    const end = start + (Number(edit.delete) || 0);
    // An insertion at the quoted boundary belongs to the replacement only
    // when it is inside the selection, not when it merely follows it.
    return (end > found.start && start < found.end)
      || (end === start && start >= found.start && start < found.end);
  });
  if (!overlapping.length) return null;
  const start = found.start;
  const end = found.end;
  let cursor = start;
  let replacement = "";
  for (const edit of edits) {
    const editStart = Number(edit.at) || 0;
    const editEnd = editStart + (Number(edit.delete) || 0);
    if (editEnd <= start) continue;
    if (editStart >= end) break;
    if (editStart > cursor) replacement += oldText.slice(cursor, Math.min(editStart, end));
    let insert = edit.insert || "";
    // A token hunk can begin just before the quote or end just after it. Drop
    // unchanged outside context when the edit gives us a defensible boundary.
    // When that context also changed, the diff cannot identify which part of
    // the insertion replaces the selected passage.
    if (editStart < start) {
      const outside = oldText.slice(editStart, start);
      if (outside && insert) {
        if (!insert.startsWith(outside)) return null;
        insert = insert.slice(outside.length);
      }
    }
    if (editEnd > end) {
      const outside = oldText.slice(end, editEnd);
      if (outside && insert) {
        if (!insert.endsWith(outside)) return null;
        insert = insert.slice(0, -outside.length);
      }
    }
    if (insert && (editEnd > start || (editStart >= start && editStart < end))) {
      replacement += insert;
    }
    cursor = Math.max(cursor, Math.min(editEnd, end));
  }
  if (cursor < end) replacement += oldText.slice(cursor, end);
  return replacement;
}
