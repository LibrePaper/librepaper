// The list of versions, and what restoring one involves.
//
// This owns the manifest -- the rows and whether the last read of it
// succeeded -- together with the state of a restore being put to the reader.
// The comparison between a selected version and the current source is not
// here: that is `history-source`, which the page owns alongside this.
//
// A restore replaces every file in the project, so it is the one action in
// the reader that can destroy work nobody has seen. The signature of the
// project is therefore passed in as a value rather than read from a session
// here: what a confirmation is worth depends on it being the same project
// that was on screen when the question was asked, and a caller that hands
// over what it read cannot accidentally confirm against a newer one.
//
// A restore also carries `expected_frontier` (SPEC-server-is-a-log.md §7.1,
// room-v2.md "restore | expected_frontier equals the head frontier"): the
// server refuses a restore whose caller has not said what head it is
// replacing, so that a concurrent edit nobody here has seen is never
// silently discarded. The frontier, like the signature, is the caller's to
// read and hand over -- this module has no session to read one from.

import * as defaultHistory from "../history.js";
import { authHeaders, keyHeaders } from "../api.js";
import { createGeneration } from "./generation.js";

export function createTimeline({
  slug,
  key = "",
  history = defaultHistory,
  fetcher = globalThis.fetch,
  // The restore succeeded, so whatever was being compared against is stale.
  onrestored = async () => {},
  disposed = () => false,
}) {
  const state = $state({
    labels: [],
    problem: "",
    // The restore being put to the reader, if one is.
    restoring: false,
    restoreSha: "",
    restoreBusy: false,
    // Set when the project moved under an open dialog, so the question can be
    // asked again against what it now says.
    restoreMoved: false,
    canEdit: false,
  });

  const reads = createGeneration();
  // The project as it stood when this restore was put to the reader. Not
  // state: nothing draws it, and it is compared rather than shown.
  let baseline = "";
  // The base64 Loro frontier of that same moment, sent as `expected_frontier`
  // on confirm. Kept apart from `baseline`, a JSON tree signature, because the
  // server's precondition is stated in frontiers, not in file contents.
  let restoreFrontier = "";

  /// Read the manifest. Only the newest read may write what it found: a
  /// slower earlier one finishing later would otherwise replace it.
  async function load() {
    const stale = reads.begin();
    try {
      const loaded = await history.loadWithStatus(slug, keyHeaders(key));
      if (disposed() || stale()) return;
      state.labels = loaded.labels;
      state.problem = "";
    } catch (error) {
      if (disposed() || stale()) return;
      // A failed refresh is not evidence that the previous save state still
      // applies; keep the old rows for selection.
      state.problem = error.message || "The version history could not be read.";
    }
  }

  /// Put a restore to the reader, against the project as `signature` says it
  /// now stands. `frontier` is that same moment's base64 Loro frontier, read
  /// by the caller from its own session (§7.1's `expected_frontier`).
  function ask(sha, signature, frontier = "") {
    if (!state.canEdit || !sha) return;
    state.restoreSha = sha;
    baseline = signature;
    restoreFrontier = frontier;
    state.restoreMoved = false;
    state.restoring = true;
  }

  /// Go ahead with it, if the project is still the one the reader was shown.
  ///
  /// Somebody else -- or this reader in another tab -- may have written to
  /// the project while the dialog was open. What restoring discards is then
  /// not what was confirmed, so the confirmation is asked for again against
  /// the project as it now stands.
  async function confirm(signature, frontier = restoreFrontier) {
    const sha = state.restoreSha;
    if (!state.canEdit || !sha) return;
    if (signature !== baseline) {
      baseline = signature;
      restoreFrontier = frontier;
      state.restoreMoved = true;
      return;
    }
    state.restoreBusy = true;
    try {
      const response = await fetcher(`/api/documents/${slug}/restore`, {
        method: "POST",
        headers: authHeaders(key, "application/json"),
        body: JSON.stringify({ sha, expected_frontier: restoreFrontier }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        // The server's own wording already says what to do (§7.1: refresh
        // and try again when the head moved under this restore), so it is
        // shown as-is rather than folded into one generic failure.
        throw new Error(payload.error || "That version could not be restored. The project is unchanged.");
      }
      state.restoring = false;
      state.restoreMoved = false;
      await load();
      await onrestored();
    } catch (error) {
      // Reported rather than thrown: the restore dialog stays open and the
      // history panel keeps the line, so both places the reader might be
      // looking already say why. A toast would be a third.
      state.problem = error.message || "That version could not be restored. The project is unchanged.";
    } finally {
      state.restoreBusy = false;
    }
  }

  /// Give a version a name people will recognize it by.
  async function name(sha, given) {
    try {
      await history.label(slug, sha, given, keyHeaders(key));
    } catch (error) {
      state.problem = error.message || "That version could not be named.";
      return;
    }
    await load();
  }

  /// How a version is described when it is being restored.
  function describe(sha) {
    const point = state.labels.find((entry) => entry.sha === sha);
    return point ? `${point.label ? `${point.label} · ` : ""}${new Date(point.at).toLocaleString()}` : "this version";
  }

  return { state, load, ask, confirm, name, describe };
}
