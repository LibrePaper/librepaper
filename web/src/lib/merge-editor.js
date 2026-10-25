// MergeEditor is fetched through this one promise because WebKit resolves a
// second concurrent import() of a module whose graph has a top-level await
// (loro-web.js) before the module has run. That crashed History on iPhones
// with "yt is not a function". One import call, shared by every caller,
// cannot hit that.

// Runs load once and shares its promise. A rejection is forgotten so that a
// later call can try again.
export function importOnce(load) {
  let pending = null;
  return () => {
    if (!pending) {
      pending = load().catch((error) => {
        pending = null;
        throw error;
      });
    }
    return pending;
  };
}

export const loadMergeEditor = importOnce(() =>
  import("../components/MergeEditor.svelte").then((module) => module.default),
);
