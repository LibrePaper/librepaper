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
// A restore also carries `expected_frontier`.
// The server refuses a restore whose caller has not said what head it is
// replacing, so that a concurrent edit nobody here has seen is never
// silently discarded. The frontier, like the signature, is the caller's to
// read and hand over -- this module has no session to read one from.

import * as defaultHistory from "../history.js";
import { labelOrder } from "../history.js";
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
    // Loaded points, oldest first.
    labels: [],
    // True after the first successful read of the newest page.
    loaded: false,
    // True when no older page remains to be read.
    complete: false,
    loadingMore: false,
    // Message from the last failed older-page read, "" otherwise.
    moreProblem: "",
    // Saved points, newest first, and how reading them went.
    saved: [],
    savedLoaded: false,
    savedLoading: false,
    savedProblem: "",
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
  // A current-label request can commit while its HTTP response is lost. Keep
  // its identity until a successful response so an explicit retry replays it.
  const pendingCurrentLabels = new Map();

  // The loaded points by sha. `state.labels` is always these, in `labelOrder`.
  const rows = new Map();
  // The cursor for the next older page, and whether any older page has been
  // read. Until one has, the newest pages are the only evidence of where the
  // loaded rows end, so a refresh may set the cursor.
  let cursor = null;
  let olderLoaded = false;
  // The older page being read, shared by callers that ask while it runs.
  let olderFetch = null;
  const savedReads = createGeneration();

  function publish() {
    state.labels = [...rows.values()].sort(labelOrder);
  }

  function merge(points) {
    for (const point of points) rows.set(point.sha, point);
  }

  function newestSeq() {
    let newest = 0;
    for (const point of rows.values()) if (point.seq > newest) newest = point.seq;
    return newest;
  }

  /// Read the newest page. Later calls are refreshes: they read newer pages
  /// until one holds a row already known, and replace rows by sha, so older
  /// pages already loaded are kept. Only the newest read may write what it
  /// found: a slower earlier one finishing later would otherwise replace it.
  async function load() {
    const stale = reads.begin();
    const known = newestSeq();
    const refresh = state.loaded;
    try {
      const pages = [];
      let page = await history.loadPage(slug, keyHeaders(key));
      if (disposed() || stale()) return;
      pages.push(page);
      while (refresh && page.next !== null && !page.labels.some((point) => point.seq <= known)) {
        page = await history.loadPage(slug, keyHeaders(key), { after: page.next });
        if (disposed() || stale()) return;
        pages.push(page);
      }
      for (const found of pages) merge(found.labels);
      if (!olderLoaded) {
        cursor = page.next;
        state.complete = page.next === null;
      }
      state.loaded = true;
      state.problem = "";
      publish();
    } catch (error) {
      if (disposed() || stale()) return;
      // A failed refresh is not evidence that the previous save state still
      // applies; keep the old rows for selection.
      state.problem = error.message || "The version history could not be read.";
    }
  }

  /// Read the next older page and add its rows. A no-op when everything is
  /// loaded or nothing is loaded yet; while an older page is being read, the
  /// same read is returned to the caller.
  async function loadMore() {
    if (olderFetch) return olderFetch;
    if (state.complete || !state.loaded) return;
    olderFetch = fetchOlder().finally(() => {
      olderFetch = null;
    });
    return olderFetch;
  }

  async function fetchOlder() {
    const from = cursor;
    state.loadingMore = true;
    try {
      const page = await history.loadPage(slug, keyHeaders(key), { after: from });
      if (disposed()) return;
      merge(page.labels);
      olderLoaded = true;
      // A refresh may have moved the cursor while this page was read; its
      // cursor then stands, and this page's rows are merged all the same.
      if (cursor === from) {
        cursor = page.next;
        state.complete = page.next === null;
      }
      state.moreProblem = "";
      publish();
    } catch (error) {
      if (disposed()) return;
      // The rows already shown stay, and the reader can try again.
      state.moreProblem = error.message || "Older versions could not be read.";
    } finally {
      state.loadingMore = false;
    }
  }

  /// Read older pages until the oldest loaded point is at or before `iso`, or
  /// nothing older remains, or a page fails.
  async function loadUntil(iso) {
    const until = Date.parse(iso);
    while (state.loaded && !state.complete && !state.moreProblem && !disposed()) {
      const oldest = state.labels[0];
      if (!oldest || !(Date.parse(oldest.at) > until)) return;
      await loadMore();
    }
  }

  /// Read every page of saved points into `state.saved`, newest first.
  async function loadSaved() {
    const stale = savedReads.begin();
    state.savedLoading = true;
    try {
      const saved = [];
      let after = null;
      do {
        const page = await history.loadPage(slug, keyHeaders(key), { after, named: true });
        if (disposed() || stale()) return;
        saved.push(...page.labels);
        after = page.next;
      } while (after !== null);
      state.saved = saved.sort((left, right) => labelOrder(right, left));
      state.savedLoaded = true;
      state.savedProblem = "";
    } catch (error) {
      if (disposed() || stale()) return;
      state.savedProblem = error.message || "Saved versions could not be read.";
    } finally {
      if (!stale()) state.savedLoading = false;
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
    const normalizedLabel = Array.from(
      given.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/gu, "")
        .trim()
        .split(/\s+/u)
        .join(" "),
    ).slice(0, 120).join("");
    const operationKey = `${sha}\0${normalizedLabel}`;
    let requestId;
    if (sha === "current") {
      requestId = pendingCurrentLabels.get(operationKey);
      if (!requestId) {
        requestId = globalThis.crypto?.randomUUID?.();
        if (!requestId) {
          state.problem = "This browser cannot safely retry a current label.";
          return;
        }
        pendingCurrentLabels.set(operationKey, requestId);
      }
    }
    try {
      await history.label(slug, sha, given, keyHeaders(key), requestId);
    } catch (error) {
      state.problem = error.message || "That version could not be named.";
      return;
    }
    if (sha === "current") pendingCurrentLabels.delete(operationKey);
    // A refresh only reads the newest pages, so an older row that was renamed
    // is changed here as well, or it would keep its old name.
    const renamed = rows.get(sha);
    if (renamed) {
      rows.set(sha, { ...renamed, label: normalizedLabel });
      publish();
    }
    await load();
    if (state.savedLoaded) await loadSaved();
  }

  /// How a version is described when it is being restored.
  function describe(sha) {
    const point = state.labels.find((entry) => entry.sha === sha);
    return point
      ? `${point.label ? `${point.label} · ` : ""}${new Date(point.at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}`
      : "this version";
  }

  return { state, load, loadMore, loadUntil, loadSaved, ask, confirm, name, describe };
}
