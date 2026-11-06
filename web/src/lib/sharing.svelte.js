// The sharing state the Share and Agent panels both read: the document's
// links and agent tokens, the share actions that change them, and the copy
// feedback. Both panels need the same load, the same change and the same
// copy, so the rules live here once rather than in each panel.
//
// The owner of a panel calls `reset()` when it closes. A request or copy that
// is still in flight then cannot put a dismissed value back on screen.

import { onDestroy } from "svelte";
import { getPrivate, post } from "./api.js";
import { m } from "./paraglide/messages.js";

/**
 * @param {{ slug: () => string, canShare: () => boolean }} options
 */
export function createSharing({ slug, canShare }) {
  let sharing = $state(null);
  let busy = $state(false);
  let loading = $state(false);
  let error = $state("");
  let generation = 0;
  let copied = $state("");
  let copyFallback = $state("");
  let copyTimer;
  // Bumped whenever a copy is dismissed, so a copy that settles afterwards
  // cannot put a dismissed value back on screen.
  let copyGeneration = 0;
  onDestroy(() => clearTimeout(copyTimer));

  async function load() {
    const documentSlug = slug();
    const request = ++generation;
    loading = true;
    copyFallback = "";
    error = "";
    try {
      const answer = canShare() ? await getPrivate(`/api/documents/${documentSlug}/share`) : { links: [], tokens: [] };
      if (request !== generation) return;
      sharing = answer;
    } catch (failure) {
      if (request === generation) error = failure.message || m.share_error_load();
    } finally {
      if (request === generation) loading = false;
    }
  }

  // Runs one share action and answers the server's reply, or null.
  async function change(body) {
    if (busy || !canShare()) return null;
    const documentSlug = slug();
    const request = generation;
    busy = true;
    error = "";
    try {
      const answer = await post(`/api/documents/${documentSlug}/share`, body);
      if (documentSlug !== slug() || request !== generation) return null;
      copyFallback = "";
      sharing = answer;
      return answer;
    } catch (failure) {
      if (request === generation) error = failure.message || m.share_error_update();
      return null;
    } finally { busy = false; }
  }

  async function copy(value) {
    const attempt = copyGeneration;
    try {
      await navigator.clipboard.writeText(value);
      if (attempt !== copyGeneration) return;
      copied = value;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = ""), 1800);
      copyFallback = "";
      error = "";
    } catch {
      if (attempt !== copyGeneration) return;
      copyFallback = value;
      error = m.share_error_copy_blocked();
    }
  }

  // Drops the manual-copy fallback and any error, and makes a copy that is
  // still settling irrelevant.
  function clearCopy() {
    copyGeneration += 1;
    copyFallback = "";
    error = "";
  }

  // The panel closed: anything in flight is stale.
  function reset() {
    generation += 1;
    clearCopy();
  }

  return {
    get sharing() { return sharing; },
    get busy() { return busy; },
    get loading() { return loading; },
    get error() { return error; },
    get copied() { return copied; },
    get copyFallback() { return copyFallback; },
    load,
    change,
    copy,
    clearCopy,
    reset,
  };
}
