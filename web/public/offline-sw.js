// The version is part of the name, so a change here is a clean sweep rather
// than a migration. Old caches are deleted on activate: without that, every
// deployment's digest-named assets accumulate forever in a browser that never
// asked to keep them, and nothing ever removes the ones no page can reach.
const CACHE = "librepaper-shell-v2";
const CORE = ["/", "/reader.html", "/assets/librepaper-icon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then(async (cache) => {
    for (const url of CORE) {
      try { await cache.add(url); } catch { /* deployment may route the shell differently */ }
    }
  }));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => event.waitUntil((async () => {
  const names = await caches.keys();
  await Promise.all(
    names.filter((name) => name.startsWith("librepaper-shell-") && name !== CACHE)
      .map((name) => caches.delete(name)),
  );
  await self.clients.claim();
})()));

async function cacheResponse(request, event) {
  const response = await fetch(request);
  const shellResource = request.mode === "navigate" || ["script", "style", "worker", "font"].includes(request.destination);
  if (response.ok && shellResource && request.method === "GET" && new URL(request.url).origin === self.location.origin) {
    // Kept, but not waited for. Awaiting the write put a cache round trip in
    // front of every script, stylesheet and font the page was already
    // blocked on -- paying, on the critical path, for a copy that only
    // matters the next time the network is gone. `waitUntil` keeps the
    // worker alive until it lands without holding the response back.
    const stored = caches.open(CACHE).then((cache) => cache.put(request, response.clone()));
    if (event) event.waitUntil(stored.catch(() => {}));
    else stored.catch(() => {});
  }
  return response;
}

self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET" || new URL(event.request.url).origin !== self.location.origin) return;
  // The API and the room socket are never the worker's business. Nothing here
  // caches them and nothing here can serve them offline, so passing them
  // through leaves them exactly as fast as they were before a service worker
  // existed, rather than routing every request the reader makes through this
  // handler to reach the same `fetch`.
  const { pathname } = new URL(event.request.url);
  if (pathname.startsWith("/api/") || pathname.startsWith("/ws/")) return;
  event.respondWith((async () => {
    try {
      return await cacheResponse(event.request, event);
    } catch {
      const cached = await caches.match(event.request, { ignoreSearch: false });
      if (cached) return cached;
      const url = new URL(event.request.url);
      if (event.request.mode === "navigate" && url.pathname.startsWith("/docs/")) {
        const reader = await caches.match("/reader.html");
        if (reader) return reader;
      }
      if (event.request.mode === "navigate") {
        const landing = await caches.match("/");
        if (landing) return landing;
      }
      throw new Error("resource is unavailable offline");
    }
  })());
});

self.addEventListener("message", (event) => {
  if (event.data?.type !== "cache-shell") return;
  event.waitUntil((async () => {
    try {
      const cache = await caches.open(CACHE);
      for (const url of event.data.urls || []) await cache.add(url);
      event.ports[0]?.postMessage({ ok: true });
    } catch (error) {
      event.ports[0]?.postMessage({ ok: false, error: error?.message || "could not cache offline shell" });
    }
  })());
});
