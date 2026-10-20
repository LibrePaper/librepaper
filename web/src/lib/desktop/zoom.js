// How much of the desktop app's page zoom a canvas has to make up for.
//
// WebKit (Linux and macOS) leaves page zoom out of devicePixelRatio, so a
// canvas drawn at the ratio alone is stretched by the zoom and comes out
// blurry. WebView2 on Windows already folds zoom into the ratio, and a plain
// browser has no app zoom, so both of those need no correction.
import { desktop } from "./client.js";

function factor(level) {
  const app = desktop();
  if (!app || /Windows/.test(globalThis.navigator?.userAgent ?? "")) return 1;
  const zoom = Number(level ?? app.zoom);
  return Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
}

/// Calls `listener` now with the correction factor, and again whenever the
/// shell changes the zoom. Returns the unsubscribe.
///
/// The level in the init script is the one the window opened at, so a page
/// loaded after a later zoom also asks the shell for the level it has now.
export function onPageZoom(listener) {
  const heard = (event) => listener(factor(event.detail));
  listener(factor());
  globalThis.window?.addEventListener("librepaper:zoom", heard);
  const app = desktop();
  if (app && !/Windows/.test(globalThis.navigator?.userAgent ?? "")) {
    fetch(`${app.base}shell/zoom`)
      .then((response) => (response.ok ? response.json() : null))
      .then((body) => {
        if (typeof body?.zoom === "number") listener(factor(body.zoom));
      })
      .catch(() => {});
  }
  return () => globalThis.window?.removeEventListener("librepaper:zoom", heard);
}
