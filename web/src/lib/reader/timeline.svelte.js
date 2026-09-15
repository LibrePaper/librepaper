// The list of versions, and what restoring one involves.
//
// This owns the manifest -- the rows, whether the last read of it succeeded,
// and what the deployment says about durability -- together with the state of
// a restore being put to the reader. The comparison between a selected
// version and the current source is not here: that is `history-source`, which
// the page owns alongside this.
//
// A restore replaces every file in the project, so it is the one action in
// the reader that can destroy work nobody has seen. The signature of the
// project is therefore passed in as a value rather than read from a session
// here: what a confirmation is worth depends on it being the same project
// that was on screen when the question was asked, and a caller that hands
// over what it read cannot accidentally confirm against a newer one.

import * as defaultHistory from "../history.js";
import { authHeaders, keyHeaders } from "../api.js";

export function createTimeline({
  slug,
  key = "",
  history = defaultHistory,
  fetcher = globalThis.fetch,
  // Reported rather than thrown: a failed restore leaves the dialog open and
  // says why, and nothing above here has a better place to put it.
  problem = () => {},
  // The restore succeeded, so whatever was being compared against is stale.
  onrestored = async () => {},
  disposed = () => false,
}) {
  const state = $state({
    checkpoints: [],
    durability: null,
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

  let generation = 0;
  // The project as it stood when this restore was put to the reader. Not
  // state: nothing draws it, and it is compared rather than shown.
  let baseline = "";

  /// Read the manifest. Only the newest read may write what it found: a
  /// slower earlier one finishing later would otherwise replace it.
  async function load() {
    const mine = ++generation;
    try {
      const loaded = await history.loadWithStatus(slug, keyHeaders(key));
      if (disposed() || mine !== generation) return;
      state.checkpoints = loaded.checkpoints;
      state.durability = loaded.durability;
      state.problem = "";
    } catch (error) {
      if (disposed() || mine !== generation) return;
      // A failed refresh is not evidence that the previous save state still
      // applies; keep the old rows for selection but make status unknown.
      state.durability = null;
      state.problem = error.message || "the history could not be read";
    }
  }

  /// Put a restore to the reader, against the project as `signature` says it
  /// now stands.
  function ask(sha, signature) {
    if (!state.canEdit || !sha) return;
    state.restoreSha = sha;
    baseline = signature;
    state.restoreMoved = false;
    state.restoring = true;
  }

  /// Go ahead with it, if the project is still the one the reader was shown.
  ///
  /// Somebody else -- or this reader in another tab -- may have written to
  /// the project while the dialog was open. What restoring discards is then
  /// not what was confirmed, so the confirmation is asked for again against
  /// the project as it now stands.
  async function confirm(signature) {
    const sha = state.restoreSha;
    if (!state.canEdit || !sha) return;
    if (signature !== baseline) {
      baseline = signature;
      state.restoreMoved = true;
      return;
    }
    state.restoreBusy = true;
    try {
      const response = await fetcher(`/api/documents/${slug}/restore`, {
        method: "POST",
        headers: authHeaders(key, "application/json"),
        body: JSON.stringify({ sha }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.error || "that checkpoint could not be restored");
      state.restoring = false;
      state.restoreMoved = false;
      await load();
      await onrestored();
    } catch (error) {
      problem(error.message || "that checkpoint could not be restored");
    } finally {
      state.restoreBusy = false;
    }
  }

  /// Give a version a name people will recognize it by.
  async function name(sha, given) {
    try {
      await history.label(slug, sha, given, keyHeaders(key));
    } catch (error) {
      problem(error.message || "that checkpoint could not be named");
      return;
    }
    await load();
  }

  /// How a version is described when it is being restored.
  function describe(sha) {
    const point = state.checkpoints.find((entry) => entry.sha === sha);
    return point ? `${point.label ? `${point.label} · ` : ""}${new Date(point.at).toLocaleString()}` : "this version";
  }

  return { state, load, ask, confirm, name, describe };
}
