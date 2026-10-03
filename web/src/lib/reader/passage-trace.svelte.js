// Where an orphaned comment's passage went, and what replaced it.
//
// A comment that can no longer be anchored used to be labelled "Needs
// re-anchoring", which told the person who wrote it nothing they did not
// already know. What is useful is the version at which the passage stopped
// being found, and the text that stands where it stood. Both are answered by
// reading old versions, so this batches comments over the same ordered label
// walk and owns the bookkeeping that keeps a slow scan from overwriting a
// newer one.
//
// Three things can make a walk stale: a newer walk, a change to the source,
// and a change to the visible text. All three are checked between async
// steps, which is why `now()` is asked for the current pair rather than
// handed one: a walk that started a second ago has to compare itself against
// what is true at the moment it is about to write.

import * as defaultPassages from "../passages.js";
import { keyHeaders } from "../api.js";
import { createGeneration } from "./generation.js";
import { MOMENT } from "../moment.js";

export function createPassageTrace({
  slug,
  key = "",
  passages = defaultPassages,
  // The source generation and the visible text, as they stand right now.
  now = () => ({ source: 0, visible: "" }),
  // The manifest, read only if a walk needs one and none has been read.
  loadLabels = async () => [],
  // What each of this document's files is called, by its stable id: a
  // comment names the file it is about by id, and an old label is read
  // by path.
  paths = () => new Map(),
}) {
  const state = $state({ went: {}, replacements: {} });

  const walks = createGeneration();
  // What the last completed walk was of. A walk with the same source, the
  // same visible text and the same comments would reach the same answers, so
  // it is not made.
  let last = null;

  async function trace({ comments, tree, labels = [] }) {
    const { source, visible } = now();
    const lost = comments.filter((comment) => comment.orphaned);
    const ids = lost
      .map((comment) => `${comment.id}:${comment.original_anchor?.frontier || ""}`)
      .join("|");
    if (last?.source === source && last?.visible === visible && last?.ids === ids) return;
    last = { source, visible, ids };
    const stale = walks.begin();
    const went = {};
    const replacements = {};
    let list = labels;
    if (lost.length && !list.length) list = await loadLabels();
    const current = () => {
      const state_ = now();
      return !stale() && state_.source === source && state_.visible === visible;
    };
    const traced = lost.map((comment) => passages.tracedBy(comment, paths()));
    let points = Array(lost.length).fill(null);
    if (passages.wentAtMany) {
      try {
        points = await passages.wentAtMany(slug, traced, list, keyHeaders(key));
      } catch {
        if (!stale()) last = null;
      }
    } else {
      // Test seams and older adapters can still supply the one-comment API.
      for (const [index, item] of traced.entries()) {
        points[index] = await passages.wentAt(slug, item, list, keyHeaders(key));
      }
    }
    if (!current()) return;
    for (const [index, comment] of lost.entries()) {
      if (!current()) return;
      try {
        const item = traced[index];
        const point = points[index] || null;
        if (point) went[comment.id] = point;
        if (!item?.frontier) {
          continue;
        }
        const oldText = await passages.sourceTextAt(
          slug, MOMENT + item.frontier, { file_id: item.file_id }, keyHeaders(key),
        );
        if (!current()) return null;
        const against = item.path ? tree.texts[item.path] ?? null : null;
        const replacement = await passages.replacementAt(oldText, against, item.selector);
        if (replacement !== null) replacements[comment.id] = replacement;
      } catch {
        // A missing label cannot establish a replacement. The walk is
        // forgotten rather than recorded, so the next one tries again.
        if (!stale()) last = null;
      }
    }
    if (current()) {
      state.went = went;
      state.replacements = replacements;
    }
  }

  return { state, trace };
}
