// Browser adapter for the reusable project session. CodeMirror and Reader keep
// this compatibility API; editor integrations supply their own persistence,
// reference fetcher, and presence identity to project-session.js.
import { IndexeddbPersistence } from "y-indexeddb";
import { authHeaders } from "./api.js";
import { cacheName } from "./collab-cache.js";
import { durableProjectPersistence } from "./offline-projects.js";
import { projectIdentity } from "./project-identity.js";
import { createProjectSession, randomPresenceId } from "./project-session.js";
import { keyFor } from "./storage.js";

export { uniquePresences } from "./project-session.js";

const TAB_PRESENCE_KEY = "librepaper-presence-tab";
let fallbackTabPresence = "";
const PRESENCE_COLOURS = ["#2f5bd0", "#c2410c", "#15803d", "#7c3aed", "#be123c", "#0e7490"];

function browserPresenceId() {
  try {
    const existing = sessionStorage.getItem(TAB_PRESENCE_KEY);
    if (existing) return existing;
    const id = randomPresenceId();
    sessionStorage.setItem(TAB_PRESENCE_KEY, id);
    return id;
  } catch {
    return fallbackTabPresence ||= randomPresenceId();
  }
}

function indexedDbPersistence(name, identity) {
  return {
    open(doc, events) {
      const store = new IndexeddbPersistence(name, doc);
      const durable = identity ? durableProjectPersistence(identity).open(doc, {
        ...events,
        hydrated() {},
      }) : null;
      Promise.all([store.whenSynced, durable?.hydration]).then(events.hydrated, events.failed);
      return {
        flush: () => durable?.flush?.() || Promise.resolve(),
        close() {
          durable?.close?.();
          store.destroy();
        },
      };
    },
  };
}

function browserReferenceFetcher({ slug, key = "", fetcher = globalThis.fetch }) {
  return async (reference) => {
    const response = await fetcher(reference, {
      credentials: "same-origin",
      headers: authHeaders(key || keyFor(slug)),
    });
    if (!response.ok) throw new Error("could not fetch the document");
    return new Uint8Array(await response.arrayBuffer());
  };
}

export function join({ slug, createdAt = "", key = "", persistence, fetchReference, ...options }) {
  const identity = slug && createdAt
    ? projectIdentity({ server: globalThis.location?.origin, slug, createdAt })
    : null;
  const browserStore = persistence === undefined && slug && options.mayEdit !== false
    && typeof indexedDB !== "undefined"
    ? indexedDbPersistence(cacheName(slug, createdAt), identity)
    : persistence;
  return createProjectSession({
    ...options,
    presenceId: options.presenceId || browserPresenceId,
    presenceColor: options.presenceColor || PRESENCE_COLOURS[Math.floor(Math.random() * PRESENCE_COLOURS.length)],
    persistence: browserStore,
    fetchReference: fetchReference || browserReferenceFetcher({ slug, key }),
  });
}
