// The current-state projection a non-editor reads the document from.
//
// This owns the projection the server last sent for a document, the etag it
// came with, and the schedule that re-fetches it. A reader has no session and
// no source of its own, so this projection is the whole of what they see; an
// editor never uses it, and every entry point does nothing while `active()`
// says so. What it takes in return are effects it cannot perform itself --
// saying something to the reader, painting, re-pointing the preview -- which
// arrive as `onProject` and `onProblem`.
//
// The state is `state.project` (the last projection that was stored) and
// `state.unreadable` (set only while this browser has never shown one, so the
// document pane can say why it is empty rather than sit blank).

import { authHeaders } from "../api.js";
import { m } from "../paraglide/messages.js";

/**
 * @typedef {{ project_digest?: string, texts?: Record<string, string>, retained?: Record<string, string>, changes?: unknown, tree?: { files?: Record<string, { kind: string, id?: string, digest?: string }>, settings?: unknown } }} Projection
 */

/**
 * @param {{
 *   slug: string,
 *   key?: string,
 *   fetcher?: typeof fetch,
 *   active?: () => boolean,
 *   hidden?: () => boolean,
 *   onProject?: (project: Projection) => void,
 *   onProblem?: (message: string) => void,
 *   delay?: number,
 *   setTimer?: (fn: () => void, ms: number) => unknown,
 *   clearTimer?: (timer: any) => void,
 * }} options
 */
export function createProjectionSource({
  slug,
  key = "",
  fetcher = globalThis.fetch,
  // False for an editor: every entry point below does nothing.
  active = () => true,
  // A hidden tab defers the scheduled refresh until it is shown again.
  hidden = () => false,
  // Called after a new projection is stored.
  onProject = (_project) => {},
  // Called after a failed fetch.
  onProblem = (_message) => {},
  delay = 1000,
  setTimer = setTimeout,
  clearTimer = clearTimeout,
}) {
  // Set only when this browser has never successfully shown a projection for
  // this document: an empty pane has nothing else to say for itself, so this
  // is what the document pane renders instead of staying blank while `settled`
  // silently ends the loading state (§2.2).
  /** @type {{ project: Projection | null, unreadable: string }} */
  const state = $state({ project: null, unreadable: "" });

  // The etag of the projection held, sent back so an unchanged one answers 304.
  let etag = "";
  // The fetch in flight, if any; a refresh asked for meanwhile shares it.
  /** @type {Promise<void> | null} */
  let inflight = null;
  // A refresh was asked for while one was in flight or the tab was hidden.
  let pending = false;
  /** @type {ReturnType<typeof setTimer> | null} */
  let timer = null;
  let disposed = false;

  /// Fetch the projection if it has changed. An editor, or a disposed source,
  /// fetches nothing. A request made while one is in flight joins that one and
  /// leaves a follow-up scheduled for when it lands.
  /** @returns {Promise<void> | undefined} */
  function refresh() {
    if (disposed || !active()) return;
    if (inflight) {
      pending = true;
      return inflight;
    }
    const headers = authHeaders(key);
    if (etag) headers["If-None-Match"] = etag;
    inflight = (async () => {
      const response = await fetcher(`/api/documents/${encodeURIComponent(slug)}/project`, { headers });
      if (response.status === 304) return;
      if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        // A document whose projection exceeds a resource bound answers 503
        // and stays unreadable on the server until recovered (§2.2); its
        // editors can still type and export locally, but this browser is a
        // reader with no source of its own to fall back to. Carry the status
        // so the catch below can say that plainly rather than the generic
        // "could not be loaded" a transient network failure gets.
        const failure = Object.assign(new Error(body.error || m.readerc_project_load_failed()), { status: response.status });
        throw failure;
      }
      const next = await response.json();
      if (disposed || !active()) return;
      etag = response.headers.get("etag") || `"${next.project_digest}"`;
      state.project = next;
      state.unreadable = "";
      onProject(next);
    })().catch((/** @type {any} */ error) => {
      if (disposed) return;
      const message = error?.status === 503
        ? (error.message || m.readerc_too_large())
        : (error?.message || m.readerc_project_load_failed());
      // Never leave the pane spinning on a project this browser has not yet
      // shown anything for: a reader with no source of its own has nothing
      // else to paint while this stays broken, so the loading state has to
      // end here rather than waiting on a render that is never coming.
      if (!state.project) state.unreadable = message;
      onProblem(message);
    }).finally(() => {
      inflight = null;
      if (pending) schedule();
    });
    return inflight;
  }

  /// Ask for a refresh after a delay. Nothing is scheduled while the tab is
  /// hidden: the request is kept, and `resume` schedules it once the tab shows.
  function schedule() {
    pending = true;
    if (disposed || hidden() || timer !== null) return;
    timer = setTimer(() => {
      timer = null;
      pending = false;
      void refresh();
    }, delay);
  }

  /// Called when the tab becomes visible: a refresh that was deferred while it
  /// was hidden is scheduled now.
  function resume() {
    if (pending) schedule();
  }

  /// The source tree a download or a render reads when there is no session:
  /// the projection in the shape the session would give. Nothing until a
  /// projection has arrived.
  function tree() {
    const project = state.project;
    if (!project) return { main: "", texts: {}, digests: {} };
    const digests = {};
    for (const [path, file] of Object.entries(project.tree?.files || {})) {
      if (file?.kind === "asset" && file.digest) digests[path] = file.digest;
    }
    return {
      main: "",
      texts: { ...(project.texts || {}) },
      retained: { ...(project.retained || {}) },
      digests,
      settings: project.tree?.settings || null,
    };
  }

  /// The owner is going away: nothing scheduled runs and no response that
  /// lands afterwards is stored or reported.
  function dispose() {
    disposed = true;
    clearTimer(timer);
    timer = null;
  }

  return { state, refresh, schedule, resume, tree, dispose };
}
