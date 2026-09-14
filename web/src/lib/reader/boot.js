// The reader's initial metadata load. It owns the two requests it starts and
// invalidates their callbacks when the reader is torn down or restarted.
import { keyHeaders, me as defaultWhoami } from "../api.js";
import { preparedProject } from "../offline-projects.js";


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
  let generation = 0;
  let disposed = false;
  const controllers = new Set();

  async function requestDocument(current) {
    const controller = new AbortController();
    controllers.add(controller);
    try {
      const response = await fetcher(`/api/documents/${slug}`, {
        headers: keyHeaders(key),
        signal: controller.signal,
      });
      if (!response.ok) throw new Error("not found");
      const document_ = await response.json();
      if (!disposed && current === generation) onDocument(document_);
    } catch (error) {
      if (error?.name === "AbortError" || disposed || current !== generation) return;
      try {
        const cached = await findLocal({ server: globalThis.location?.origin, slug });
        if (cached && !disposed && current === generation) {
          onDocument({ ...cached.document, offline_prepared: true });
          return;
        }
      } catch { /* unavailable local storage falls through to the normal error */ }
      if (!disposed && current === generation) onError(error);
    } finally {
      controllers.delete(controller);
    }
  }

  async function start() {
    if (disposed) return;
    const current = ++generation;
    // Identity is deliberately independent of document access. A failed
    // identity endpoint still leaves the document request useful, while a
    // late identity response can never rename a replaced reader session.
    Promise.resolve().then(() => whoami()).then((who) => {
      if (!disposed && current === generation) onIdentity(who || {});
    }).catch(() => {
      if (!disposed && current === generation) onIdentity({});
    });
    return requestDocument(current);
  }

  function dispose() {
    disposed = true;
    generation += 1;
    for (const controller of controllers) controller.abort();
    controllers.clear();
  }

  return { start, dispose };
}
