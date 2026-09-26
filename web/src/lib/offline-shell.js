export async function registerOfflineShell() {
  if (!("serviceWorker" in navigator) || !globalThis.isSecureContext) return null;
  try {
    await navigator.serviceWorker.register("/offline-sw.js", { scope: "/" });
    return await navigator.serviceWorker.ready;
  } catch {
    return null;
  }
}

export async function cacheCurrentShell() {
  const registration = await registerOfflineShell();
  const worker = registration?.active;
  if (!worker) return false;
  const resources = performance.getEntriesByType?.("resource") || [];
  const urls = [location.href, location.origin + "/"];
  for (const entry of resources) {
    if (!["script", "link"].includes(entry.initiatorType)) continue;
    const url = new URL(entry.name, location.href);
    if (url.origin === location.origin) urls.push(url.href);
  }
  // A writer may have reached the document through a shared link and never
  // loaded the landing bundle under service-worker control. Cache the entry
  // page and its static imports now so a cold offline start can list projects.
  const landing = await fetch("/", { cache: "no-store" });
  if (!landing.ok) throw new Error("could not prepare the offline start page");
  const markup = new DOMParser().parseFromString(await landing.text(), "text/html");
  for (const element of markup.querySelectorAll("script[src], link[rel=stylesheet][href], link[rel=modulepreload][href]")) {
    const value = element.getAttribute("src") || element.getAttribute("href");
    const url = new URL(value, location.origin);
    if (url.origin === location.origin) urls.push(url.href);
  }
  const channel = new MessageChannel();
  const completed = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("offline shell preparation timed out")), 15000);
    channel.port1.onmessage = (event) => {
      clearTimeout(timeout);
      event.data?.ok ? resolve() : reject(new Error(event.data?.error || "could not cache the offline shell"));
    };
  });
  worker.postMessage({ type: "cache-shell", urls: [...new Set(urls)] }, [channel.port2]);
  await completed;
  return true;
}
