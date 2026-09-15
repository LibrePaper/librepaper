const CACHE = "librepaper-shell-v1";
const CORE = ["/", "/reader.html", "/assets/librepaper-icon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then(async (cache) => {
    for (const url of CORE) {
      try { await cache.add(url); } catch { /* deployment may route the shell differently */ }
    }
  }));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

async function cacheResponse(request) {
  const response = await fetch(request);
  const shellResource = request.mode === "navigate" || ["script", "style", "worker", "font"].includes(request.destination);
  if (response.ok && shellResource && request.method === "GET" && new URL(request.url).origin === self.location.origin) {
    const cache = await caches.open(CACHE);
    await cache.put(request, response.clone());
  }
  return response;
}

self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET" || new URL(event.request.url).origin !== self.location.origin) return;
  event.respondWith((async () => {
    try {
      return await cacheResponse(event.request);
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
