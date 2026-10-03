// The reader's initial metadata load. It owns the two requests it starts and
// invalidates their callbacks when the reader is torn down or restarted.
import { keyHeaders, me as defaultWhoami } from "../api.js";
import { preparedProject } from "../offline-projects.js";
import { createGeneration } from "./generation.js";

/** @typedef {{ name?: string, handle?: string, provider?: string, [key: string]: unknown }} ReaderIdentity */
/** @typedef {{ title?: string, docs_origin?: string, document_id?: string, created_at?: string, role?: string, can_edit?: boolean, can_moderate?: boolean, can_see_sharing?: boolean, [key: string]: unknown }} ReaderDocument */
/** @typedef {Error & { status?: number }} ReaderBootError */

/** @param {{
 * slug: string, key?: string, fetcher?: typeof fetch,
 * whoami?: () => Promise<ReaderIdentity>,
 * findLocal?: (query: { server?: string, slug: string }) => Promise<{ document: ReaderDocument } | null>,
 * onIdentity?: (identity: ReaderIdentity) => void,
 * onDocument?: (document: ReaderDocument) => void,
 * onError?: (error: ReaderBootError) => void,
 * }} options
 */
export function createReaderBoot({
  slug,
  key = "",
  fetcher = globalThis.fetch,
  whoami = defaultWhoami,
  findLocal = preparedProject,
  onIdentity = () => {},
  onDocument = () => {},
  onError = () => {},
}) {
  const attempts = createGeneration();
  let disposed = false;
  const controllers = new Set();

  async function requestDocument(stale) {
    const controller = new AbortController();
    controllers.add(controller);
    try {
      const response = await fetcher(`/api/documents/${slug}`, {
        headers: keyHeaders(key),
        signal: controller.signal,
      });
      if (!response.ok) {
        const failure = Object.assign(new Error("not found"), { status: response.status });
        throw failure;
      }
      const document_ = await response.json();
      if (!disposed && !stale()) onDocument(document_);
    } catch (error) {
      if (error?.name === "AbortError" || disposed || stale()) return;
      // A cached copy can bridge a network outage, but it must not turn an
      // explicit server refusal (especially a sign-in requirement) into
      // access to a document the current account cannot open.
      if (!error?.status) {
        try {
          const cached = await findLocal({ server: globalThis.location?.origin, slug });
          if (cached && !disposed && !stale()) {
            onDocument({ ...cached.document, offline_prepared: true });
            return;
          }
        } catch { /* unavailable local storage falls through to the normal error */ }
      }
      if (!disposed && !stale()) onError(error);
    } finally {
      controllers.delete(controller);
    }
  }

  async function start() {
    if (disposed) return;
    const stale = attempts.begin();
    // Identity is deliberately independent of document access. A failed
    // identity endpoint still leaves the document request useful, while a
    // late identity response can never rename a replaced reader session.
    Promise.resolve().then(() => whoami()).then((who) => {
      if (!disposed && !stale()) onIdentity(who || {});
    }).catch(() => {
      if (!disposed && !stale()) onIdentity({});
    });
    return requestDocument(stale);
  }

  function dispose() {
    disposed = true;
    attempts.cancel();
    for (const controller of controllers) controller.abort();
    controllers.clear();
  }

  return { start, dispose };
}
