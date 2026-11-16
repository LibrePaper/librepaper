// Stop iPhone and iPad Safari from zooming the page when a small field is focused.
//
// Safari zooms into any input, textarea or contenteditable whose font size is
// under 16px, and the zoom sticks because the app's touch-action rules stop a
// pinch from zooming back out. iOS has ignored maximum-scale for user pinch
// zoom since iOS 10, but it still suppresses this focus zoom, so the cap is
// added here at startup for iOS only. It is kept out of the static HTML because
// it blocks pinch zoom on Android and fails the accessibility gate.

const IOS = /iP(hone|ad|od)/;

function isIOS(nav) {
  return IOS.test(nav.platform) || (nav.platform === "MacIntel" && nav.maxTouchPoints > 1);
}

/// Append `maximum-scale=1` to the viewport meta on iOS, once.
export function preventFocusZoom(doc = document, nav = navigator) {
  if (!isIOS(nav)) return;
  const meta = /** @type {HTMLMetaElement | null} */ (doc.querySelector('meta[name="viewport"]'));
  if (!meta || meta.content.includes("maximum-scale")) return;
  meta.content += ",maximum-scale=1";
}
