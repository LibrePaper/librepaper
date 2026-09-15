// Where an orphaned comment's passage went, and what replaced it.
//
// A comment that can no longer be anchored used to be labelled "Needs
// re-anchoring", which told the person who wrote it nothing they did not
// already know. What is useful is the version at which the passage stopped
// being found, and the text that stands where it stood. Both are answered by
// reading old versions, which takes a request per comment, so this owns the
// answers and the bookkeeping that keeps a slow walk from overwriting a
// newer one.
//
// Three things can make a walk stale: a newer walk, a change to the source,
// and a change to the visible text. All three are checked between every
// request, which is why `now()` is asked for the current pair rather than
// handed one: a walk that started a second ago has to compare itself against
// what is true at the moment it is about to write.

import * as defaultPassages from "../passages.js";
import { keyHeaders } from "../api.js";

export function createPassageTrace({
  slug,
  key = "",
  passages = defaultPassages,
  // The source generation and the visible text, as they stand right now.
  now = () => ({ source: 0, visible: "" }),
  // The manifest, read only if a walk needs one and none has been read.
  loadCheckpoints = async () => [],
}) {
  const state = $state({ went: {}, replacements: {} });

  let generation = 0;
  // What the last completed walk was of. A walk with the same source, the
  // same visible text and the same comments would reach the same answers, so
  // it is not made.
  let last = null;

  async function trace({ comments, tree, checkpoints = [] }) {
    const { source, visible } = now();
    const lost = comments.filter((comment) => comment.orphaned && !comment.region);
    const ids = lost.map((comment) => `${comment.id}:${comment.revision}`).join("|");
    if (last?.source === source && last?.visible === visible && last?.ids === ids) return;
    last = { source, visible, ids };
    const mine = ++generation;
    const went = {};
    const replacements = {};
    let list = checkpoints;
    if (lost.length && !list.length) list = await loadCheckpoints();
    const current = () => {
      const state_ = now();
      return mine === generation && state_.source === source && state_.visible === visible;
    };
    for (const comment of lost) {
      if (!current()) return;
      try {
        const point = await passages.wentAt(slug, comment, list, keyHeaders(key));
        if (point) went[comment.id] = point;
        if (!comment.revision) continue;
        const oldText = comment.source
          ? await passages.sourceTextAt(slug, comment.revision, comment.source.path, keyHeaders(key))
          : await passages.textAt(slug, comment.revision, keyHeaders(key));
        const against = comment.source ? tree.texts[comment.source.path] ?? "" : visible;
        const replacement = await passages.replacementAt(oldText, against, comment.source || comment);
        if (replacement !== null) replacements[comment.id] = replacement;
      } catch {
        // A missing checkpoint cannot establish a replacement. The walk is
        // forgotten rather than recorded, so the next one tries again.
        if (mine === generation) last = null;
      }
    }
    if (current()) {
      state.went = went;
      state.replacements = replacements;
    }
  }

  return { state, trace };
}
